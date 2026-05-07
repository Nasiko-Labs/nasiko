# Nasiko LLM Gateway

This directory contains the configuration for the **LiteLLM** LLM router gateway.

## What It Does

The gateway provides a centralized, OpenAI-compatible API endpoint for all agents.
Agents send LLM requests to the gateway using a **virtual key** — they never see
or need the actual provider API keys (OpenRouter, OpenAI, etc.).

## Files

- `litellm_config.yaml` — Model routing and provider configuration
- `key_provisioner.py` — Script to generate virtual keys for agents

## Quick Start

The gateway auto-deploys with `docker compose up`. See [docs/llm-gateway.md](../docs/llm-gateway.md) for full details.

## Changing Providers

Edit `litellm_config.yaml` to add/remove models and providers, then restart:

```bash
docker compose -f docker-compose.local.yml --env-file .nasiko-local.env restart litellm-gateway
```

No agent code changes needed.
