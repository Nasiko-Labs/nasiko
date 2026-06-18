# Claude SDK Research Agent

A Wikipedia research agent built with the Anthropic Python SDK, exposing the A2A protocol.

## What it does

Accepts a natural language question, searches Wikipedia for relevant information using Claude's tool use, and returns a concise answer.

## Quick start

```bash
cp .env.example .env
# Fill in your API key
python src/__main__.py
```

## Docker

```bash
docker compose up
```

## Environment variables

| Variable | Required | Description |
|----------|----------|-------------|
| ANTHROPIC_API_KEY | Yes | Anthropic API key |

## A2A endpoint

`http://localhost:10002/`
