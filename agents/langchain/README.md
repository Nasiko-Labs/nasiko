# LangChain Research Agent

A Wikipedia research agent built with LangChain (direct tool-calling agent, not LangGraph), exposing the A2A protocol.

## What it does

Accepts a natural language question, uses a `create_tool_calling_agent` with gpt-4o and a Wikipedia search tool, and streams progress events back to the client.

## Environment variables

| Variable | Required | Description |
|----------|----------|-------------|
| `OPENAI_API_KEY` | Yes | OpenAI API key; the server exits at startup without it |
| `OPENAI_MODEL` | No | Model name; defaults to `gpt-4o` |
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
docker build -t langchain-agent .
docker run -p 8000:8000 \
  -e OPENAI_API_KEY=sk-... \
  langchain-agent
```

The container listens on port `8000` (the `Dockerfile`'s `CMD`); the Agent Card is at
`http://localhost:8000/.well-known/agent-card.json`. Run outside a container (with the packages the `Dockerfile` installs),
`python src/__main__.py` defaults to `--host localhost --port 8000`.
