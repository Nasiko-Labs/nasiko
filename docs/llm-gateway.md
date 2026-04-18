# Nasiko LLM Gateway Guide

## Overview

The Nasiko LLM gateway is a self-hosted LiteLLM proxy that centralizes all provider credentials (OpenAI, Anthropic, OpenRouter) in one place. Agents deployed through Nasiko receive a per-agent virtual key and a single gateway endpoint instead of raw provider keys. This means a provider credential leak is limited to the gateway config, not spread across every agent in the platform.

---

## Quick Start

```bash
# 0. Install Python deps (the litellm-setup CLI needs motor, cryptography, httpx, etc.)
uv sync                              # if using uv (recommended)
# or: pip install -e .                # plain pip equivalent

# 1. Copy the env template
cp .nasiko-local.env.example .nasiko-local.env

# 2. Generate LITELLM_MASTER_KEY, LITELLM_SALT_KEY, and LITELLM_POSTGRES_PASSWORD
python3 cli/setup/setup.py litellm init

# 3. Set your provider keys in .nasiko-local.env
#    (open the file and fill in OPENAI_API_KEY and, optionally, ANTHROPIC_API_KEY)

# 4. Start the full compose stack (llm-gateway and litellm-postgres start automatically)
docker compose --env-file .nasiko-local.env -f docker-compose.local.yml up -d

# 5. Confirm the gateway is healthy (host port 4100 maps to internal 4000)
curl http://localhost:4100/health/liveliness
# LiteLLM v1.83+ returns the plain string "I'm alive!"; older versions returned
# {"status":"healthy"}. Either shape means the gateway is up.

# 6. Start the orchestrator and redis listener
make start-nasiko
```

The gateway is now reachable at `http://llm-gateway:4000` from inside the agent network and at `http://localhost:4100` from the host.

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│  agents-net / app-network (Docker bridge)                       │
│                                                                 │
│  ┌─────────────────┐  HTTP/4000  ┌──────────────────────────┐  │
│  │  Agent container │ ──────────► │  llm-gateway             │  │
│  │  (OPENAI_BASE_URL│             │  (LiteLLM proxy)         │  │
│  │   OPENAI_API_KEY)│             │  ghcr.io/berriai/litellm │  │
│  └─────────────────┘             │  :v1.83.3-stable          │  │
│                                  └──────┬──────────┬──────────┘  │
│                                         │          │             │
│               HTTPS (provider API) ◄────┘          │             │
│                 openai.com                         │  OTel/HTTP  │
│                 api.anthropic.com                  │  /v1/traces │
│                 openrouter.ai                      ▼             │
│                                        ┌──────────────────────┐  │
│                                        │  phoenix-observability│  │
│                                        │  :6006               │  │
│                                        └──────────────────────┘  │
│                                                                 │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │  litellm-postgres  :5432 (internal only)                │   │
│  │  LiteLLM_VerificationToken table (virtual key store)    │   │
│  └─────────────────────────────────────────────────────────┘   │
│                                                                 │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │  mongodb :27017                                         │   │
│  │  nasiko.virtual_keys collection (operational index,     │   │
│  │  Fernet-encrypted key values)                           │   │
│  └─────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘

Host access: http://localhost:4100  (port 4100 → internal 4000)
Admin API:   Authorization: Bearer $LITELLM_MASTER_KEY
```

---

## Provider Configuration

Provider configuration lives in `cli/setup/litellm/config.yaml`. This file is bind-mounted into the gateway container at startup. **Changes to this file require a gateway restart to take effect.**

### Current model list

```yaml
model_list:
  - model_name: gpt-4o-mini
    litellm_params:
      model: openai/gpt-4o-mini
      api_key: os.environ/OPENAI_API_KEY

  - model_name: claude-haiku
    litellm_params:
      model: anthropic/claude-3-5-haiku-20241022
      api_key: os.environ/ANTHROPIC_API_KEY

  # Rotation alias — agents always call "default-model"
  - model_name: default-model
    litellm_params:
      model: openai/gpt-4o-mini
      api_key: os.environ/OPENAI_API_KEY
```

### Adding a new provider (example: Azure OpenAI)

```yaml
model_list:
  - model_name: azure-gpt4
    litellm_params:
      model: azure/gpt-4
      api_key: os.environ/AZURE_API_KEY
      api_base: os.environ/AZURE_API_BASE
      api_version: "2024-02-15-preview"
```

Then add `AZURE_API_KEY` and `AZURE_API_BASE` to `.nasiko-local.env` and restart:

```bash
docker compose --env-file .nasiko-local.env -f docker-compose.local.yml restart llm-gateway
```

### Switching the default model (provider rotation)

Edit only the `default-model` entry in `config.yaml`:

```yaml
# Before (OpenAI):
  - model_name: default-model
    litellm_params:
      model: openai/gpt-4o-mini
      api_key: os.environ/OPENAI_API_KEY

# After (Anthropic):
  - model_name: default-model
    litellm_params:
      model: anthropic/claude-3-5-haiku-20241022
      api_key: os.environ/ANTHROPIC_API_KEY
```

Restart the gateway. Zero agent code changes are required.

---

## Virtual-Key Lifecycle

The gateway issues per-agent virtual keys so that no agent ever sees a raw provider credential. The orchestrator mints a key automatically at agent deploy time. You can also manage keys manually via the CLI.

### Minting

**Automatic path (orchestrator):** When `_deploy_agent_container()` runs in `orchestrator/redis_stream_listener.py`, it calls `POST /key/generate` on the LiteLLM admin API. The returned key is stored in the MongoDB `virtual_keys` collection (Fernet-encrypted via `BaseRepository`) and injected into the agent container as `OPENAI_BASE_URL` and `OPENAI_API_KEY`. The orchestrator re-uses an existing active key if one is already recorded in MongoDB for that agent (idempotent on redeploy).

**Manual path (CLI):**

```bash
nasiko-setup litellm mint --agent my-agent --owner user123
# Output: Virtual key minted: sk-virt-xxxx... (stored in MongoDB)
```

### Rotation

OSS LiteLLM does not support rotate-in-place (that is an Enterprise feature). Rotation is implemented as create-new + delete-old:

```bash
nasiko-setup litellm rotate --agent my-agent
# 1. Calls POST /key/generate → new key
# 2. Calls DELETE /key/{old_key} → removes old key from LiteLLM Postgres
# 3. Updates MongoDB virtual_keys record (rotated_at timestamp)
# Prints: "Key rotated. Restart the agent container to inject the new key."
```

**Why agent restart is required:** The virtual key is injected as an env var at deploy time. LiteLLM Postgres no longer recognizes the old key after deletion, so the agent will start receiving 401s. A restart causes the orchestrator to inject the new key from MongoDB.

**Why restart-based rotation (decision rationale):**
- **OSS limitation.** `POST /key/{key}/regenerate` (rotate-in-place with a `grace_period` overlap window) is an Enterprise-tier LiteLLM endpoint. It is not present in the `ghcr.io/berriai/litellm:v1.83.3-stable` OSS image.
- **Scope discipline.** The PS instructs us not to add an unnecessary proxy layer. Building a thin in-house shim between agents and LiteLLM to perform hot-key-swap on a config signal would duplicate what LiteLLM Enterprise already does — out of scope for a 36-hour submission.
- **Acceptable cost.** Restart-based rotation causes seconds of downtime per agent. There is no silent key-validity overlap window where both old and new keys are simultaneously accepted, which reduces the risk of stale credentials being exercised after rotation.

### Revocation

```bash
nasiko-setup litellm revoke --agent my-agent
# 1. Calls DELETE /key/{key} on LiteLLM admin API
# 2. Marks MongoDB record: active=false, revoked_at=<now>
# The agent's next LLM call returns HTTP 401 from the gateway.
```

### Listing and inspecting keys

```bash
# List all keys in MongoDB (key values are masked):
nasiko-setup litellm list-keys

# Get full key info (LiteLLM Postgres + MongoDB record):
nasiko-setup litellm info --agent my-agent
```

### Two-level storage model

| Store | What it holds | Authority |
|---|---|---|
| LiteLLM Postgres (`litellm-postgres-data` volume) | `LiteLLM_VerificationToken` rows — the gateway validates requests against this table | Authoritative for key validity |
| Nasiko MongoDB `virtual_keys` collection | `{agent_id, virtual_key (encrypted), active, created_at, rotated_at, revoked_at}` | Operational index: orchestrator reads this to decide which key to inject |

If the two stores diverge (e.g., after `make start-nasiko` wipes the Postgres volume), re-run `nasiko-setup litellm mint --agent <name>` for each agent or redeploy agents through the platform to auto-mint.

---

## LiteLLM vs Portkey — Design Rationale

Both tools are MIT-licensed and support 100+ providers with an OpenAI-compatible endpoint. The trade-off came down to three concrete requirements for this track.

**Virtual-key admin API.** LiteLLM ships `POST /key/generate`, `DELETE /key/{key}`, `GET /key/info`, and related endpoints as first-class OSS features, backed by Postgres. Portkey's OSS self-hosted gateway (`portkeyai/gateway`) does not expose a REST admin API for creating or deleting virtual keys without the Portkey hosted control plane. GitHub issue #1190 in the Portkey gateway repo confirmed this gap as of April 2026.

**OTel export to Phoenix.** LiteLLM's `otel` callback with `OTEL_EXPORTER=otlp_http` exports to any OTLP endpoint, including Nasiko's existing Phoenix collector at `:6006/v1/traces`. Portkey's OTel/tracing is a hosted/enterprise feature; the OSS self-hosted image does not include it.

**Fail-clear behavior.** The PS fixed design requirement states: if the gateway is down, model requests must fail clearly with no fallback and no queueing. LiteLLM supports `num_retries: 0` and the absence of any `fallbacks` key achieves this directly. No equivalent disable is documented for the Portkey OSS gateway.

**Where Portkey is stronger.** Portkey's self-hosted image is lighter (Node.js, ~59 MB vs LiteLLM's Python image), requires no database for basic routing, and offers a richer hosted-tier feature set (guardrails, caching, prompt management). For a 36-hour hackathon targeting fully self-hosted capabilities, none of those strengths are actionable — the virtual-key REST API and OTel export gaps are blocking.

**Supply-chain note.** LiteLLM PyPI packages v1.82.7 and v1.82.8 were compromised in a ~40-minute window on March 24, 2026. Docker images were not affected. This gateway is pinned to `ghcr.io/berriai/litellm:v1.83.3-stable` (post-incident, published with cosign signing). See the Security section for details.

---

## Observability

### How gateway spans appear in Phoenix

The gateway exports spans via `otlp_http` to `http://phoenix-observability:6006/v1/traces`. Spans appear in Phoenix under the project name `llm-gateway` with the span name `litellm-acompletion`.

Agent-side, Langtrace (injected by `orchestrator/instrumentation_injector.py`) auto-instruments the OpenAI SDK. Every call the agent makes emits a W3C `traceparent` header. LiteLLM honors incoming `traceparent` headers by default (opt-out would require `OTEL_IGNORE_CONTEXT_PROPAGATION=true`, which is not set).

Result: the agent-turn span is the parent; the `litellm-acompletion` span is the child. Both share the same `traceId`. In Phoenix, you can expand the agent trace and see the gateway span nested under it, with the upstream provider call as a further child.

### W3C traceparent propagation path

```
Agent container
  └─ Langtrace instruments AsyncOpenAI client
       └─ Emits traceparent header on every HTTP call to the gateway
            └─ LiteLLM reads traceparent → creates child span
                 └─ Exports span to Phoenix (same traceId as agent)
```

See `docs/images/phoenix-gateway-trace.png` for a reference screenshot of the parent-child trace structure in Phoenix.

---

## Security

### Master key rotation

`LITELLM_MASTER_KEY` is the admin credential for the LiteLLM API. Anyone with this key can list, mint, and revoke all virtual keys on the gateway.

To rotate it:

1. Generate a new key: `python3 -c "import secrets; print('sk-' + secrets.token_hex(32))"`
2. Update `LITELLM_MASTER_KEY` in `.nasiko-local.env`.
3. Restart the gateway: `docker compose ... restart llm-gateway`.
4. All existing virtual keys remain valid (they are stored in Postgres independently of the master key). Only admin API access changes.

If `LITELLM_MASTER_KEY` is leaked: immediately rotate it (steps above), then audit the LiteLLM access log (`docker logs llm-gateway`) for unauthorized `/key/` admin calls.

### LITELLM_SALT_KEY — set once, never rotate

`LITELLM_SALT_KEY` is used by LiteLLM to encrypt provider credentials stored in Postgres. **If this key is regenerated or changed, all existing virtual keys in the Postgres DB become invalid** — the gateway will reject them with decryption errors. The only recovery path is to revoke all keys, wipe the `litellm-postgres-data` volume, reinitialize with the new salt key, and re-mint all keys.

Generate it once with `nasiko-setup litellm init` and store it securely. Never commit it to source control.

### Supply-chain: pinned to v1.83.3-stable

LiteLLM PyPI packages v1.82.7 and v1.82.8 (published March 24, 2026) contained malicious code injected via a compromised maintainer account. The compromise window was approximately 40 minutes. Docker images published to `ghcr.io/berriai/litellm` were not affected because they pin their internal dependency to a requirements hash, not a floating PyPI version.

This gateway is pinned to `ghcr.io/berriai/litellm:v1.83.3-stable`, which post-dates the incident and is signed with cosign. Do not bump this image tag without verifying the new tag's cosign signature and release notes.

### Fernet encryption of MongoDB-persisted keys

The Nasiko MongoDB `virtual_keys` collection stores the LiteLLM-issued virtual key values encrypted with Fernet (via `BaseRepository._encrypt_data()`). The encryption key is `USER_CREDENTIALS_ENCRYPTION_KEY` in the environment. This protects keys at rest in MongoDB if the database is accessed directly. The key is decrypted only in memory when the orchestrator needs to inject it into an agent container.

---

## Troubleshooting

### 1. Gateway won't start: "Prisma migrations failed" or health check never passes

**Symptom:** `docker compose ... ps` shows `llm-gateway` as unhealthy; logs show migration errors.

**Fix:** LiteLLM runs Prisma migrations on startup, which requires `litellm-postgres` to be healthy first. The compose `depends_on` with `condition: service_healthy` handles this, but if Postgres is slow to start on a cold machine, the gateway may time out. Wait 30 seconds after Postgres becomes healthy, then restart the gateway:

```bash
docker compose --env-file .nasiko-local.env -f docker-compose.local.yml restart llm-gateway
```

Check Postgres first: `docker exec litellm-postgres pg_isready -U litellm`

### 2. Virtual key says "invalid" or returns HTTP 401

**Symptom:** Agent LLM calls return 401; `docker logs llm-gateway` shows "Invalid token" or "Key not found."

**Cause A — LITELLM_SALT_KEY was regenerated.** If the salt key changed after keys were minted, the stored keys can no longer be validated. Re-mint per agent:

```bash
nasiko-setup litellm revoke --agent <name>
nasiko-setup litellm mint --agent <name>
# Then restart the agent container so the new key is injected.
```

**Cause B — make start-nasiko wiped the Postgres volume.** `make start-nasiko` runs `docker volume rm $(docker volume ls -q)` which wipes `litellm-postgres-data`. All LiteLLM keys are gone. MongoDB records are now stale. Re-mint for each agent or redeploy through the platform.

**Cause C — Key was explicitly revoked.** Check `nasiko-setup litellm list-keys` for `active=false` on the affected agent.

### 3. Provider returns 401 or 403

**Symptom:** Gateway logs show an upstream 401 from openai.com or api.anthropic.com.

**Fix:** The provider key configured in `config.yaml` is invalid or expired. Check `OPENAI_API_KEY` (or `ANTHROPIC_API_KEY`, `OPENROUTER_API_KEY`) in `.nasiko-local.env`. The gateway does not rotate provider keys automatically.

### 4. Phoenix shows no gateway spans

**Symptom:** Agent spans appear in Phoenix but there is no `litellm-acompletion` child span.

**Fix:** Check that `phoenix-observability` is running and reachable from the gateway container:

```bash
docker exec llm-gateway curl -sf http://phoenix-observability:6006/v1/traces
# If this fails: phoenix is not on app-network or is not running.
```

Also confirm the OTel env vars are set correctly in the `llm-gateway` service in `docker-compose.local.yml`:

```
OTEL_EXPORTER=otlp_http
OTEL_ENDPOINT=http://phoenix-observability:6006/v1/traces
```

### 5. "make start-nasiko wiped my keys" — recovery

`make start-nasiko` calls `docker volume rm $(docker volume ls -q)`. This is intentional (clean-slate development), but it destroys `litellm-postgres-data`, which holds all virtual keys.

Recovery steps:

```bash
# 1. Re-start the gateway (Prisma will recreate tables on a fresh volume)
docker compose --env-file .nasiko-local.env -f docker-compose.local.yml up -d litellm-postgres llm-gateway

# 2. Re-mint keys for each agent that uses the gateway
nasiko-setup litellm mint --agent a2a-gateway-demo

# 3. Redeploy affected agents so the orchestrator injects the new key
# (or restart the agent containers manually with the new key in env)
```

For long-running development: run `docker compose up -d` first to bring up persistent services, then use `make start-nasiko` only when you specifically need to wipe and restart the orchestrator processes.

---

## Limitations (Hackathon Scope)

The following are known gaps between this implementation and a production-ready deployment. They are intentional scope decisions for a 36-hour submission.

1. **Key rotation requires agent restart.** OSS LiteLLM does not support rotate-in-place (`/key/regenerate` is an Enterprise-only endpoint). Rotation is create-new + delete-old + agent restart. See the "Why restart-based rotation" note in the §Virtual-Key Lifecycle → Rotation section for the full three-point rationale. A production system would use the Enterprise tier or implement a rolling-restart strategy.

### Ephemeral virtual-key storage

`make start-nasiko` runs `docker volume rm $(docker volume ls -q)`, which unconditionally wipes **all** Docker volumes — including `litellm-postgres-data`. Every virtual key minted in that Postgres instance is permanently lost.

**Why we accepted this:**
- The `Makefile` is explicitly out of scope per the PS and confirmed by organizer response ("do not touch the Makefile"). We cannot add a `--filter` to the volume-wipe command or introduce a volume-preserve target.
- `make start-nasiko` is a local-dev clean-slate reset, not a production operation. Treating key loss as a fatal issue would misrepresent the production path.
- Recovery is automatic: MongoDB records pointing at the gone keys become stale, but the orchestrator's `_ensure_virtual_key` re-mints a fresh key from LiteLLM on the agent's next deploy, overwriting the stale MongoDB record. No manual intervention is required.

**Production follow-up:** bind-mount the Postgres data directory to a host path outside Docker's managed-volume namespace (e.g., `./data/litellm-postgres:/var/lib/postgresql/data`) so `docker volume rm` cannot reach it. Alternatively, add an explicit `make preserve-gateway` target that excludes `litellm-postgres-data` from the wipe — but that requires a Makefile edit, which is blocked in this scope.

2. **No per-team or per-environment budget policies.** Virtual keys are created with a flat `max_budget: 5.0` (USD, 30-day window) and `rpm_limit: 60`. A production deployment would define budget tiers per team and enforce them through LiteLLM's team/organization primitives.

3. **No rate-limit enforcement at the gateway level in this demo.** The `rpm_limit` is set but the demo environment does not exercise rate-limit behavior or test it in CI.

4. **No Kubernetes chart.** The gateway runs in `docker-compose.local.yml`. A K8s `Deployment` + `Service` + `ConfigMap` skeleton exists as a stretch item but is not included in this PR. The `cli/k8s/charts/nasiko-platform/` pattern can be followed when adding K8s support.

5. **Config reload requires restart.** LiteLLM does not hot-reload `config.yaml`. Switching the default model requires `docker compose ... restart llm-gateway` plus a brief (~20 second) downtime.

6. **Fernet encryption key (`USER_CREDENTIALS_ENCRYPTION_KEY`) is required.** If this key is absent from the `nasiko-redis-listener` container env, MongoDB virtual key storage falls back to plaintext. Ensure this variable is set in `.nasiko-local.env` (it is already present in the existing nasiko stack configuration).
