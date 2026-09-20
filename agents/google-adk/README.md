# Google ADK Research Agent

A Wikipedia research agent built with Google ADK (open-source), exposing the A2A protocol.

## What it does

Accepts a natural language question, uses a Google ADK Runner with Gemini 2.0 Flash and a Wikipedia search tool to answer questions, streaming progress events back to the client.

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
| GOOGLE_API_KEY | Yes | Google AI Studio API key |

## A2A endpoint

`http://localhost:10006/`
