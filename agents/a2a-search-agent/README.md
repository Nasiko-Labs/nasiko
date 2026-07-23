# Web Search Agent

An autonomous research and web search agent for the Nasiko platform built using the A2A SDK.

## Capabilities

The `a2a-search-agent` can execute the following skills:
- **Search Web**: Perform internet searches via DuckDuckGo to answer specific queries or gather context.
- **Read Webpage**: Fetch text content from a given URL to read detailed articles, documentation, or news.

## Requirements

- Python 3.10+
- Docker and Docker Compose (optional for local deployment)
- An OpenAI, OpenRouter, or MiniMax API Key.

## Setup & Running Locally

1. Create a `.env` file in this directory based on the following template:
```env
OPENAI_API_KEY=your_api_key_here
```

2. Run the agent using Docker:
```bash
docker compose up --build -d
```

3. Alternatively, run the agent using Python:
```bash
pip install -r pyproject.toml
./run_with_phoenix.sh
```

## Examples

You can test the agent locally using the A2A CLI or standard HTTP JSON-RPC calls.

- "Search the web for the latest news on AI"
- "Who won the Superbowl in 2024?"
- "Read the content of this webpage: https://en.wikipedia.org/wiki/Artificial_intelligence and summarize its history."