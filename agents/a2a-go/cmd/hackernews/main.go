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
	"time"

	"github.com/a2aproject/a2a-go/v2/a2a"
	"github.com/a2aproject/a2a-go/v2/a2asrv"
)

type hnExecutor struct{}

var _ a2asrv.AgentExecutor = (*hnExecutor)(nil)

func (*hnExecutor) Execute(ctx context.Context, execCtx *a2asrv.ExecutorContext) iter.Seq2[a2a.Event, error] {
	return func(yield func(a2a.Event, error) bool) {
		userText := extractText(execCtx.Message)
		result, err := handleQuery(ctx, userText)
		if err != nil {
			yield(nil, err)
			return
		}
		yield(a2a.NewMessage(a2a.MessageRoleAgent, a2a.NewTextPart(result)), nil)
	}
}

func (*hnExecutor) Cancel(ctx context.Context, execCtx *a2asrv.ExecutorContext) iter.Seq2[a2a.Event, error] {
	return func(yield func(a2a.Event, error) bool) {}
}

func handleQuery(ctx context.Context, query string) (string, error) {
	lower := strings.ToLower(query)
	if strings.Contains(lower, "top") || strings.Contains(lower, "front page") || strings.Contains(lower, "trending") {
		return getTopStories(ctx)
	}
	return searchStories(ctx, query)
}

func getTopStories(ctx context.Context) (string, error) {
	resp, err := http.Get("https://hacker-news.firebaseio.com/v0/topstories.json")
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()

	var ids []int
	if err := json.NewDecoder(resp.Body).Decode(&ids); err != nil {
		return "", err
	}

	limit := 10
	if len(ids) < limit {
		limit = len(ids)
	}

	var sb strings.Builder
	sb.WriteString("Top Hacker News Stories:\n\n")

	for i, id := range ids[:limit] {
		story, err := getItem(ctx, id)
		if err != nil {
			continue
		}
		sb.WriteString(fmt.Sprintf("%d. %s\n", i+1, story.Title))
		sb.WriteString(fmt.Sprintf("   Points: %d | Comments: %d | By: %s\n", story.Score, story.Descendants, story.By))
		if story.URL != "" {
			sb.WriteString(fmt.Sprintf("   URL: %s\n", story.URL))
		}
		sb.WriteString(fmt.Sprintf("   Posted: %s\n\n", time.Unix(story.Time, 0).Format("2006-01-02 15:04")))
	}
	return sb.String(), nil
}

func searchStories(ctx context.Context, query string) (string, error) {
	apiURL := fmt.Sprintf("https://hn.algolia.com/api/v1/search?query=%s&tags=story&hitsPerPage=10", url.QueryEscape(query))

	resp, err := http.Get(apiURL)
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()

	var data struct {
		Hits []struct {
			Title    string `json:"title"`
			URL      string `json:"url"`
			Author   string `json:"author"`
			Points   int    `json:"points"`
			Comments int    `json:"num_comments"`
			Created  string `json:"created_at"`
		} `json:"hits"`
		NbHits int `json:"nbHits"`
	}

	if err := json.NewDecoder(resp.Body).Decode(&data); err != nil {
		return "", err
	}

	if len(data.Hits) == 0 {
		return fmt.Sprintf("No Hacker News stories found for %q.", query), nil
	}

	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("Hacker News search results for %q (%d total):\n\n", query, data.NbHits))

	for i, hit := range data.Hits {
		sb.WriteString(fmt.Sprintf("%d. %s\n", i+1, hit.Title))
		sb.WriteString(fmt.Sprintf("   Points: %d | Comments: %d | By: %s\n", hit.Points, hit.Comments, hit.Author))
		if hit.URL != "" {
			sb.WriteString(fmt.Sprintf("   URL: %s\n", hit.URL))
		}
		sb.WriteString("\n")
	}
	return sb.String(), nil
}

type hnItem struct {
	Title       string `json:"title"`
	URL         string `json:"url"`
	By          string `json:"by"`
	Score       int    `json:"score"`
	Descendants int    `json:"descendants"`
	Time        int64  `json:"time"`
}

func getItem(ctx context.Context, id int) (*hnItem, error) {
	resp, err := http.Get(fmt.Sprintf("https://hacker-news.firebaseio.com/v0/item/%d.json", id))
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	var item hnItem
	if err := json.NewDecoder(resp.Body).Decode(&item); err != nil {
		return nil, err
	}
	return &item, nil
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

var port = flag.Int("port", 5003, "Port to listen on")

func main() {
	flag.Parse()
	addr := fmt.Sprintf("0.0.0.0:%d", *port)

	agentCard := &a2a.AgentCard{
		Name:        "Hacker News Agent",
		Description: "Browse and search Hacker News. Get top stories, search by topic, find trending tech discussions.",
		SupportedInterfaces: []*a2a.AgentInterface{
			a2a.NewAgentInterface(fmt.Sprintf("http://0.0.0.0:%d/a2a", *port), a2a.TransportProtocolJSONRPC),
		},
		DefaultInputModes:  []string{"text"},
		DefaultOutputModes: []string{"text"},
		Capabilities:       a2a.AgentCapabilities{Streaming: false},
		Skills: []a2a.AgentSkill{
			{
				ID:          "top_stories",
				Name:        "Top Stories",
				Description: "Get the current top stories from Hacker News front page",
				Tags:        []string{"hackernews", "tech", "trending", "news"},
				Examples:    []string{"Show top HN stories", "What's trending on Hacker News?", "Front page stories"},
			},
			{
				ID:          "search_stories",
				Name:        "Search Stories",
				Description: "Search Hacker News for stories about a specific topic",
				Tags:        []string{"hackernews", "search", "tech"},
				Examples:    []string{"Search HN for Rust programming", "Find stories about AI agents", "Hacker News posts about Go"},
			},
		},
	}

	handler := a2asrv.NewHandler(&hnExecutor{})

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
	log.Printf("Hacker News Agent listening on %s", addr)
	log.Fatal(http.Serve(listener, mux))
}
