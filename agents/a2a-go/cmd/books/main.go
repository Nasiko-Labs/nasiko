package main

import (
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"iter"
	"log"
	"net"
	"net/http"
	"net/url"
	"strings"

	"github.com/a2aproject/a2a-go/v2/a2a"
	"github.com/a2aproject/a2a-go/v2/a2asrv"
)

type booksExecutor struct{}

var _ a2asrv.AgentExecutor = (*booksExecutor)(nil)

func (*booksExecutor) Execute(ctx context.Context, execCtx *a2asrv.ExecutorContext) iter.Seq2[a2a.Event, error] {
	return func(yield func(a2a.Event, error) bool) {
		userText := extractText(execCtx.Message)
		result, err := searchBooks(ctx, userText)
		if err != nil {
			yield(nil, err)
			return
		}
		yield(a2a.NewMessage(a2a.MessageRoleAgent, a2a.NewTextPart(result)), nil)
	}
}

func (*booksExecutor) Cancel(ctx context.Context, execCtx *a2asrv.ExecutorContext) iter.Seq2[a2a.Event, error] {
	return func(yield func(a2a.Event, error) bool) {}
}

func searchBooks(ctx context.Context, query string) (string, error) {
	apiURL := fmt.Sprintf("https://openlibrary.org/search.json?q=%s&limit=5&fields=title,author_name,first_publish_year,subject,isbn,number_of_pages_median,ratings_average", url.QueryEscape(query))

	resp, err := http.Get(apiURL)
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()

	var data struct {
		NumFound int `json:"numFound"`
		Docs     []struct {
			Title            string   `json:"title"`
			AuthorName       []string `json:"author_name"`
			FirstPublishYear int      `json:"first_publish_year"`
			Subject          []string `json:"subject"`
			ISBN             []string `json:"isbn"`
			Pages            int      `json:"number_of_pages_median"`
			Rating           float64  `json:"ratings_average"`
		} `json:"docs"`
	}

	if err := json.NewDecoder(resp.Body).Decode(&data); err != nil {
		return "", err
	}

	if data.NumFound == 0 {
		return fmt.Sprintf("No books found for %q. Try a different search term.", query), nil
	}

	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("Found %d books for %q (showing top %d):\n\n", data.NumFound, query, len(data.Docs)))

	for i, doc := range data.Docs {
		authors := "Unknown"
		if len(doc.AuthorName) > 0 {
			authors = strings.Join(doc.AuthorName, ", ")
		}
		sb.WriteString(fmt.Sprintf("%d. %s\n", i+1, doc.Title))
		sb.WriteString(fmt.Sprintf("   Author(s): %s\n", authors))
		if doc.FirstPublishYear > 0 {
			sb.WriteString(fmt.Sprintf("   First published: %d\n", doc.FirstPublishYear))
		}
		if doc.Pages > 0 {
			sb.WriteString(fmt.Sprintf("   Pages: %d\n", doc.Pages))
		}
		if doc.Rating > 0 {
			sb.WriteString(fmt.Sprintf("   Rating: %.1f/5\n", doc.Rating))
		}
		if len(doc.Subject) > 0 {
			subjects := doc.Subject
			if len(subjects) > 3 {
				subjects = subjects[:3]
			}
			sb.WriteString(fmt.Sprintf("   Subjects: %s\n", strings.Join(subjects, ", ")))
		}
		sb.WriteString("\n")
	}
	return sb.String(), nil
}

func extractText(msg *a2a.Message) string {
	if msg == nil {
		return ""
	}
	var parts []string
	for _, p := range msg.Parts {
		if t := p.Text(); t != "" {
			parts = append(parts, t)
		}
	}
	return strings.Join(parts, " ")
}

var port = flag.Int("port", 5002, "Port to listen on")

func main() {
	flag.Parse()
	addr := fmt.Sprintf("0.0.0.0:%d", *port)

	agentCard := &a2a.AgentCard{
		Name:        "Books Agent",
		Description: "Search and discover books using the Open Library API. Find details like authors, publish dates, ratings, and subjects.",
		SupportedInterfaces: []*a2a.AgentInterface{
			a2a.NewAgentInterface(fmt.Sprintf("http://0.0.0.0:%d/a2a", *port), a2a.TransportProtocolJSONRPC),
		},
		DefaultInputModes:  []string{"text"},
		DefaultOutputModes: []string{"text"},
		Capabilities:       a2a.AgentCapabilities{Streaming: false},
		Skills: []a2a.AgentSkill{
			{
				ID:          "search_books",
				Name:        "Search Books",
				Description: "Search for books by title, author, or topic",
				Tags:        []string{"books", "search", "library", "reading"},
				Examples:    []string{"Search for books about machine learning", "Find books by Isaac Asimov", "Books about the history of Rome"},
			},
		},
	}

	handler := a2asrv.NewHandler(&booksExecutor{})

	mux := http.NewServeMux()
	mux.Handle("/a2a", a2asrv.NewJSONRPCHandler(handler))
	mux.Handle(a2asrv.WellKnownAgentCardPath, a2asrv.NewStaticAgentCardHandler(agentCard))
	mux.HandleFunc("/health", func(w http.ResponseWriter, r *http.Request) {
		w.Write([]byte("ok"))
	})

	listener, err := net.Listen("tcp", addr)
	if err != nil {
		log.Fatalf("Failed to listen: %v", err)
	}
	log.Printf("Books Agent listening on %s", addr)
	log.Fatal(http.Serve(listener, mux))
}
