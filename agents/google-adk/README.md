# Google ADK Research Agent

A Wikipedia research agent built with Google ADK (open-source), exposing the A2A protocol.

## What it does

Accepts a natural language question, uses a Google ADK Runner with Gemini 2.0 Flash and a Wikipedia search tool to answer questions, streaming progress events back to the client.

## Environment variables

| Variable | Required | Description |
|----------|----------|-------------|
| `GOOGLE_API_KEY` | Yes | Google AI Studio API key; the server exits at startup without it |
| `HOST_OVERRIDE` | No | URL to advertise on the Agent Card instead of `http://<host>:<port>/` |

## Run on Nasiko

From this directory, with the `nasiko` CLI connected and logged in (see the repository README):

```bash
nasiko secrets set GOOGLE_API_KEY AIza...      # vault secrets reach every agent you own
nasiko deploy .                               # build locally, push and deploy
# or, without local Docker: nasiko upload .   # the server builds it
```

## Run with Docker

```bash
docker build -t google-adk-agent .
docker run -p 8000:8000 \
  -e GOOGLE_API_KEY=AIza... \
  google-adk-agent
```

The container listens on port `8000` (the `Dockerfile`'s `CMD`); the Agent Card is at
`http://localhost:8000/.well-known/agent-card.json`. Run outside a container (with the packages the `Dockerfile` installs),
`python src/__main__.py` defaults to `--host localhost --port 8000`.
