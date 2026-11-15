# Claude SDK Synthesizer Agent

A synthesis agent built with the Anthropic Python SDK, exposing the A2A protocol.

## What it does

Takes raw research, data or a question and streams back a clear, structured Markdown report: a
summary, key findings and conclusions. It calls the Messages API directly and has no tools, so it
works best on material other agents gathered.

## Environment variables

| Variable | Required | Description |
|----------|----------|-------------|
| `ANTHROPIC_API_KEY` | Yes (or `DEEPSEEK_API_KEY`) | Key for the Messages API endpoint |
| `ANTHROPIC_BASE_URL` | No | Messages API endpoint; defaults to `https://api.deepseek.com/anthropic` |
| `MODEL` | No | Model name; defaults to `deepseek-v4-flash` |
| `HOST_OVERRIDE` | No | URL to advertise on the Agent Card instead of `http://<host>:<port>/` |

To use Anthropic itself, set `ANTHROPIC_BASE_URL=https://api.anthropic.com` and a Claude model in `MODEL`.

## Run on Nasiko

From this directory, with the `nasiko` CLI connected and logged in (see the repository README):

```bash
nasiko secrets set ANTHROPIC_API_KEY sk-ant-...      # vault secrets reach every agent you own
nasiko deploy .                               # build locally, push and deploy
# or, without local Docker: nasiko upload .   # the server builds it
```

## Run with Docker

```bash
docker build -t claude-sdk-agent .
docker run -p 8000:8000 \
  -e ANTHROPIC_API_KEY=sk-ant-... \
  -e ANTHROPIC_BASE_URL=https://api.anthropic.com \
  -e MODEL=<claude-model> \
  claude-sdk-agent
```

The container listens on port `8000` (the `Dockerfile`'s `CMD`); the Agent Card is at
`http://localhost:8000/.well-known/agent-card.json`. Run outside a container (with the packages the `Dockerfile` installs),
`python src/__main__.py` defaults to `--host localhost --port 8000`.
