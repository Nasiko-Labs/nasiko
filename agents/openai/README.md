# OpenAI Research Agent

A Wikipedia research agent built with the OpenAI Agents SDK, exposing the A2A protocol.

## What it does

Accepts a natural language question, searches Wikipedia for relevant information, and returns a concise answer.

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

`http://localhost:10003/`
