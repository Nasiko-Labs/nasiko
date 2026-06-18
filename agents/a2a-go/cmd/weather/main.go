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
	// [nasiko:imports]
)

type weatherExecutor struct{}

var _ a2asrv.AgentExecutor = (*weatherExecutor)(nil)

func (*weatherExecutor) Execute(ctx context.Context, execCtx *a2asrv.ExecutorContext) iter.Seq2[a2a.Event, error] {
	return func(yield func(a2a.Event, error) bool) {
		userText := extractText(execCtx.Message)
		result, err := getWeather(ctx, userText)
		if err != nil {
			yield(nil, err)
			return
		}
		yield(a2a.NewMessage(a2a.MessageRoleAgent, a2a.NewTextPart(result)), nil)
	}
}

func (*weatherExecutor) Cancel(ctx context.Context, execCtx *a2asrv.ExecutorContext) iter.Seq2[a2a.Event, error] {
	return func(yield func(a2a.Event, error) bool) {}
}

func getWeather(ctx context.Context, query string) (string, error) {
	query = cleanQuery(query)
	lat, lon, city, err := geocode(ctx, query)
	if err != nil {
		return "", fmt.Errorf("could not find location %q: %w", query, err)
	}

	apiURL := fmt.Sprintf(
		"https://api.open-meteo.com/v1/forecast?latitude=%f&longitude=%f&current=temperature_2m,relative_humidity_2m,wind_speed_10m,weather_code&daily=temperature_2m_max,temperature_2m_min,weather_code&timezone=auto&forecast_days=3",
		lat, lon,
	)

	resp, err := http.Get(apiURL)
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()

	var data struct {
		Current struct {
			Temperature float64 `json:"temperature_2m"`
			Humidity    float64 `json:"relative_humidity_2m"`
			WindSpeed   float64 `json:"wind_speed_10m"`
			WeatherCode int     `json:"weather_code"`
		} `json:"current"`
		Daily struct {
			Time       []string  `json:"time"`
			TempMax    []float64 `json:"temperature_2m_max"`
			TempMin    []float64 `json:"temperature_2m_min"`
			WeatherCode []int    `json:"weather_code"`
		} `json:"daily"`
	}

	if err := json.NewDecoder(resp.Body).Decode(&data); err != nil {
		return "", err
	}

	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("Weather for %s:\n\n", city))
	sb.WriteString(fmt.Sprintf("Current: %s, %.1f°C, humidity %0.f%%, wind %.1f km/h\n\n",
		weatherDescription(data.Current.WeatherCode),
		data.Current.Temperature,
		data.Current.Humidity,
		data.Current.WindSpeed,
	))
	sb.WriteString("3-day forecast:\n")
	for i, date := range data.Daily.Time {
		sb.WriteString(fmt.Sprintf("  %s: %s, %.0f–%.0f°C\n",
			date,
			weatherDescription(data.Daily.WeatherCode[i]),
			data.Daily.TempMin[i],
			data.Daily.TempMax[i],
		))
	}
	return sb.String(), nil
}

func geocode(ctx context.Context, query string) (float64, float64, string, error) {
	apiURL := fmt.Sprintf("https://geocoding-api.open-meteo.com/v1/search?name=%s&count=1&language=en&format=json", url.QueryEscape(query))
	resp, err := http.Get(apiURL)
	if err != nil {
		return 0, 0, "", err
	}
	defer resp.Body.Close()

	var data struct {
		Results []struct {
			Latitude  float64 `json:"latitude"`
			Longitude float64 `json:"longitude"`
			Name      string  `json:"name"`
			Country   string  `json:"country"`
		} `json:"results"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&data); err != nil {
		return 0, 0, "", err
	}
	if len(data.Results) == 0 {
		return 0, 0, "", fmt.Errorf("no results")
	}
	r := data.Results[0]
	return r.Latitude, r.Longitude, fmt.Sprintf("%s, %s", r.Name, r.Country), nil
}

func cleanQuery(q string) string {
	q = strings.TrimSpace(q)
	for _, prefix := range []string{
		"weather in ", "weather for ", "weather at ",
		"forecast for ", "forecast in ",
		"what's the weather in ", "what's the weather like in ",
		"how's the weather in ", "temperature in ",
	} {
		if strings.HasPrefix(strings.ToLower(q), prefix) {
			q = q[len(prefix):]
			break
		}
	}
	return strings.TrimSpace(q)
}

func weatherDescription(code int) string {
	switch {
	case code == 0:
		return "Clear sky"
	case code <= 3:
		return "Partly cloudy"
	case code <= 48:
		return "Foggy"
	case code <= 57:
		return "Drizzle"
	case code <= 67:
		return "Rain"
	case code <= 77:
		return "Snow"
	case code <= 82:
		return "Rain showers"
	case code <= 86:
		return "Snow showers"
	case code >= 95:
		return "Thunderstorm"
	default:
		return "Unknown"
	}
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

var port = flag.Int("port", 5001, "Port to listen on")

func main() {
	flag.Parse()
	addr := fmt.Sprintf("0.0.0.0:%d", *port)

	agentCard := &a2a.AgentCard{
		Name:        "Weather Agent",
		Description: "Get current weather and 3-day forecasts for any city worldwide using Open-Meteo (no API key required)",
		SupportedInterfaces: []*a2a.AgentInterface{
			a2a.NewAgentInterface(fmt.Sprintf("http://0.0.0.0:%d/a2a", *port), a2a.TransportProtocolJSONRPC),
		},
		DefaultInputModes:  []string{"text"},
		DefaultOutputModes: []string{"text"},
		Capabilities:       a2a.AgentCapabilities{Streaming: false},
		Skills: []a2a.AgentSkill{
			{
				ID:          "current_weather",
				Name:        "Current Weather",
				Description: "Get the current weather conditions for a city",
				Tags:        []string{"weather", "temperature", "forecast"},
				Examples:    []string{"Weather in Tokyo", "Current temperature in London", "What's the weather like in New York?"},
			},
			{
				ID:          "forecast",
				Name:        "3-Day Forecast",
				Description: "Get a 3-day weather forecast for a city",
				Tags:        []string{"weather", "forecast", "planning"},
				Examples:    []string{"3 day forecast for Paris", "Will it rain in Berlin tomorrow?"},
			},
		},
	}

	handler := a2asrv.NewHandler(&weatherExecutor{})

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
	log.Printf("Weather Agent listening on %s", addr)
	log.Fatal(http.Serve(listener, mux))
}
