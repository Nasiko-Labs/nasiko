# LangChain Research Agent

A Wikipedia research agent built with LangChain (direct tool-calling agent, not LangGraph), exposing the A2A protocol.

## What it does

Accepts a natural language question, uses a `create_tool_calling_agent` with gpt-4o and a Wikipedia search tool, and streams progress events back to the client.

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
| OPENAI_API_KEY | Yes | OpenAI API key |

## A2A endpoint

`http://localhost:10007/`
