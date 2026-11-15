# OpenAI Research Agent

A research agent built with the OpenAI Agents SDK, exposing the A2A protocol.

## What it does

Accepts a natural language question, gathers facts with its `web_search` (DuckDuckGo instant
answers) and `search_wikipedia` tools, and returns a concise answer.

## Environment variables

| Variable | Required | Description |
|----------|----------|-------------|
| `OPENAI_API_KEY` | Yes | Key for the chat completions endpoint |
| `OPENAI_BASE_URL` | No | OpenAI-compatible endpoint; unset uses the OpenAI client's default |
| `MODEL` | No | Model name; defaults to `deepseek-v4-flash`, so set it (for example `gpt-4o-mini`) when you use OpenAI |
| `HOST_OVERRIDE` | No | URL to advertise on the Agent Card instead of `http://<host>:<port>/` |

## Run on Nasiko

From this directory, with the `nasiko` CLI connected and logged in (see the repository README):

```bash
nasiko secrets set OPENAI_API_KEY sk-...      # vault secrets reach every agent you own
nasiko deploy .                               # build locally, push and deploy
# or, without local Docker: nasiko upload .   # the server builds it
```

## Run with Docker

```bash
docker build -t openai-agent .
docker run -p 8000:8000 \
  -e OPENAI_API_KEY=sk-... \
  -e MODEL=gpt-4o-mini \
  openai-agent
```

The container listens on port `8000` (the `Dockerfile`'s `CMD`); the Agent Card is at
`http://localhost:8000/.well-known/agent-card.json`. Run outside a container (with the packages the `Dockerfile` installs),
`python src/__main__.py` defaults to `--host localhost --port 8000`.
