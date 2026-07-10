package main

import (
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"iter"
	"log"
	"net"
	"net/http"
	"net/url"
	"regexp"
	"strconv"
	"strings"
	"time"

	"github.com/a2aproject/a2a-go/v2/a2a"
	"github.com/a2aproject/a2a-go/v2/a2asrv"
	// [nasiko:imports]
)

type githubExecutor struct{}

var _ a2asrv.AgentExecutor = (*githubExecutor)(nil)

func (*githubExecutor) Execute(ctx context.Context, execCtx *a2asrv.ExecutorContext) iter.Seq2[a2a.Event, error] {
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

func (*githubExecutor) Cancel(ctx context.Context, execCtx *a2asrv.ExecutorContext) iter.Seq2[a2a.Event, error] {
	return func(yield func(a2a.Event, error) bool) {}
}

// repoRef matches an owner/repo token, optionally followed by a /path inside the repo.
var repoRef = regexp.MustCompile(`\b([A-Za-z0-9](?:[A-Za-z0-9-]*[A-Za-z0-9])?)/([A-Za-z0-9_.-]+)((?:/[A-Za-z0-9_.\- ]+)*)`)

func handleQuery(ctx context.Context, query string) (string, error) {
	query = strings.TrimSpace(query)
	if query == "" {
		return usage(), nil
	}
	lower := strings.ToLower(query)

	m := repoRef.FindStringSubmatch(query)
	wantsReadme := strings.Contains(lower, "readme")
	wantsFiles := containsAny(lower, "ls ", "list ", "files", "contents", "browse", "tree", "navigate", "structure", "what's in", "whats in")

	switch {
	case m != nil && wantsReadme:
		return getReadme(ctx, m[1], m[2])
	case m != nil && (wantsFiles || m[3] != ""):
		path := strings.Trim(m[3], "/ ")
		return listContents(ctx, m[1], m[2], path)
	case m != nil && !containsAny(lower, "search", "find "):
		return repoInfo(ctx, m[1], m[2])
	default:
		return searchRepos(ctx, cleanSearchQuery(query))
	}
}

func searchRepos(ctx context.Context, query string) (string, error) {
	apiURL := fmt.Sprintf("https://api.github.com/search/repositories?q=%s&sort=stars&order=desc&per_page=10", url.QueryEscape(query))
	var data struct {
		TotalCount int `json:"total_count"`
		Items      []struct {
			FullName    string `json:"full_name"`
			Description string `json:"description"`
			Stars       int    `json:"stargazers_count"`
			Language    string `json:"language"`
			HTMLURL     string `json:"html_url"`
		} `json:"items"`
	}
	if err := githubGet(ctx, apiURL, "", &data); err != nil {
		return "", err
	}
	if len(data.Items) == 0 {
		return fmt.Sprintf("No repositories found for %q.", query), nil
	}

	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("Top repositories for %q (%d total):\n\n", query, data.TotalCount))
	for i, r := range data.Items {
		lang := r.Language
		if lang == "" {
			lang = "n/a"
		}
		sb.WriteString(fmt.Sprintf("%d. %s — ★ %d, %s\n   %s\n   %s\n",
			i+1, r.FullName, r.Stars, lang, firstLine(r.Description), r.HTMLURL))
	}
	return sb.String(), nil
}

func repoInfo(ctx context.Context, owner, repo string) (string, error) {
	apiURL := fmt.Sprintf("https://api.github.com/repos/%s/%s", owner, repo)
	var data struct {
		FullName      string   `json:"full_name"`
		Description   string   `json:"description"`
		Stars         int      `json:"stargazers_count"`
		Forks         int      `json:"forks_count"`
		OpenIssues    int      `json:"open_issues_count"`
		Language      string   `json:"language"`
		License       *struct{ Name string `json:"name"` } `json:"license"`
		DefaultBranch string   `json:"default_branch"`
		Topics        []string `json:"topics"`
		HTMLURL       string   `json:"html_url"`
		UpdatedAt     string   `json:"updated_at"`
	}
	if err := githubGet(ctx, apiURL, "", &data); err != nil {
		return "", err
	}

	license := "none"
	if data.License != nil {
		license = data.License.Name
	}
	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("%s\n%s\n\n", data.FullName, firstLine(data.Description)))
	sb.WriteString(fmt.Sprintf("Stars: %d | Forks: %d | Open issues: %d\n", data.Stars, data.Forks, data.OpenIssues))
	sb.WriteString(fmt.Sprintf("Language: %s | License: %s | Default branch: %s\n", data.Language, license, data.DefaultBranch))
	if len(data.Topics) > 0 {
		sb.WriteString(fmt.Sprintf("Topics: %s\n", strings.Join(data.Topics, ", ")))
	}
	sb.WriteString(fmt.Sprintf("Last updated: %s\n%s\n", data.UpdatedAt, data.HTMLURL))
	return sb.String(), nil
}

func listContents(ctx context.Context, owner, repo, path string) (string, error) {
	apiURL := fmt.Sprintf("https://api.github.com/repos/%s/%s/contents/%s", owner, repo, url.PathEscape(path))
	var entries []struct {
		Name string `json:"name"`
		Type string `json:"type"`
		Size int    `json:"size"`
	}
	if err := githubGet(ctx, apiURL, "", &entries); err != nil {
		// A file path returns an object, not an array — fall back to raw content.
		return getFile(ctx, owner, repo, path)
	}

	loc := owner + "/" + repo
	if path != "" {
		loc += "/" + path
	}
	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("Contents of %s:\n\n", loc))
	for _, e := range entries {
		if e.Type == "dir" {
			sb.WriteString(fmt.Sprintf("  %s/\n", e.Name))
		} else {
			sb.WriteString(fmt.Sprintf("  %s (%d bytes)\n", e.Name, e.Size))
		}
	}
	return sb.String(), nil
}

func getFile(ctx context.Context, owner, repo, path string) (string, error) {
	apiURL := fmt.Sprintf("https://api.github.com/repos/%s/%s/contents/%s", owner, repo, url.PathEscape(path))
	raw, err := githubGetRaw(ctx, apiURL, "application/vnd.github.raw+json")
	if err != nil {
		return "", err
	}
	return truncate(fmt.Sprintf("%s/%s/%s:\n\n%s", owner, repo, path, raw), 8000), nil
}

func getReadme(ctx context.Context, owner, repo string) (string, error) {
	apiURL := fmt.Sprintf("https://api.github.com/repos/%s/%s/readme", owner, repo)
	raw, err := githubGetRaw(ctx, apiURL, "application/vnd.github.raw+json")
	if err != nil {
		return "", err
	}
	return truncate(fmt.Sprintf("README of %s/%s:\n\n%s", owner, repo, raw), 8000), nil
}

func githubGet(ctx context.Context, apiURL, accept string, out any) error {
	raw, err := githubGetRaw(ctx, apiURL, accept)
	if err != nil {
		return err
	}
	return json.Unmarshal([]byte(raw), out)
}

func githubGetRaw(ctx context.Context, apiURL, accept string) (string, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, apiURL, nil)
	if err != nil {
		return "", err
	}
	// GitHub rejects requests without a User-Agent.
	req.Header.Set("User-Agent", "nasiko-github-agent")
	req.Header.Set("X-GitHub-Api-Version", "2022-11-28")
	if accept == "" {
		accept = "application/vnd.github+json"
	}
	req.Header.Set("Accept", accept)

	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()
	body, err := io.ReadAll(io.LimitReader(resp.Body, 1<<20))
	if err != nil {
		return "", err
	}

	switch {
	case resp.StatusCode == http.StatusNotFound:
		return "", fmt.Errorf("not found: %s", apiURL)
	case resp.StatusCode == http.StatusForbidden || resp.StatusCode == http.StatusTooManyRequests:
		if resp.Header.Get("X-RateLimit-Remaining") == "0" {
			return "", fmt.Errorf("GitHub rate limit exceeded (unauthenticated: 60 req/hour); resets %s", rateLimitReset(resp))
		}
		return "", fmt.Errorf("GitHub returned %d: %s", resp.StatusCode, firstLine(string(body)))
	case resp.StatusCode >= 400:
		return "", fmt.Errorf("GitHub returned %d: %s", resp.StatusCode, firstLine(string(body)))
	}
	return string(body), nil
}

func rateLimitReset(resp *http.Response) string {
	epoch, err := strconv.ParseInt(resp.Header.Get("X-RateLimit-Reset"), 10, 64)
	if err != nil {
		return "soon"
	}
	return time.Unix(epoch, 0).UTC().Format("15:04 UTC")
}

func cleanSearchQuery(q string) string {
	lower := strings.ToLower(q)
	for _, prefix := range []string{
		"search for ", "search ", "find repos for ", "find repositories for ",
		"find ", "look for ", "repos for ", "repositories for ",
	} {
		if strings.HasPrefix(lower, prefix) {
			q = q[len(prefix):]
			break
		}
	}
	return strings.TrimSpace(strings.TrimSuffix(strings.TrimSpace(q), "on github"))
}

func containsAny(s string, subs ...string) bool {
	for _, sub := range subs {
		if strings.Contains(s, sub) {
			return true
		}
	}
	return false
}

func firstLine(s string) string {
	if i := strings.IndexByte(s, '\n'); i >= 0 {
		return s[:i]
	}
	return s
}

func truncate(s string, n int) string {
	if len(s) <= n {
		return s
	}
	return s[:n] + "\n\n[truncated]"
}

func usage() string {
	return `GitHub navigator. Try:
  "search llm agent framework"       — search repositories
  "tokio-rs/tokio"                   — repository overview
  "readme of a2aproject/a2a-go"      — fetch a repo's README
  "list files in golang/go/src/net"  — browse a directory or file`
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

var port = flag.Int("port", 5004, "Port to listen on")

func main() {
	flag.Parse()
	addr := fmt.Sprintf("0.0.0.0:%d", *port)

	agentCard := &a2a.AgentCard{
		Name:        "GitHub Agent",
		Description: "Search GitHub repositories and navigate their contents using GitHub's public API (no API key required, rate-limited to 60 req/hour)",
		SupportedInterfaces: []*a2a.AgentInterface{
			a2a.NewAgentInterface(fmt.Sprintf("http://0.0.0.0:%d/a2a", *port), a2a.TransportProtocolJSONRPC),
		},
		DefaultInputModes:  []string{"text"},
		DefaultOutputModes: []string{"text"},
		Capabilities:       a2a.AgentCapabilities{Streaming: false},
		Skills: []a2a.AgentSkill{
			{
				ID:          "search_repos",
				Name:        "Search Repositories",
				Description: "Search GitHub repositories by keyword, sorted by stars",
				Tags:        []string{"github", "search", "repositories"},
				Examples:    []string{"search rust async runtime", "find repos for a2a protocol"},
			},
			{
				ID:          "repo_info",
				Name:        "Repository Overview",
				Description: "Get stars, forks, language, license, and topics for a repository",
				Tags:        []string{"github", "repository", "stats"},
				Examples:    []string{"tokio-rs/tokio", "tell me about golang/go"},
			},
			{
				ID:          "browse",
				Name:        "Browse Contents",
				Description: "List files in a repository directory, read a file, or fetch the README",
				Tags:        []string{"github", "files", "readme", "navigate"},
				Examples:    []string{"list files in golang/go/src/net", "readme of a2aproject/a2a-go"},
			},
		},
	}

	handler := a2asrv.NewHandler(&githubExecutor{})

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
	log.Printf("GitHub Agent listening on %s", addr)
	log.Fatal(http.Serve(listener, mux))
}
