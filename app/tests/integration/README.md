# Track 2 Integration Tests — LLM Gateway

These four tests verify the end-to-end behaviour of the platform-managed LLM
gateway introduced in Track 2. They exercise real Docker containers (via
docker-compose), real HTTP calls to LiteLLM, and optionally real provider
endpoints. Every test is **skippable** — missing provider keys or a missing
Docker daemon causes a graceful skip, not a failure.

---

## Prerequisites

| Requirement | Why needed |
|---|---|
| Docker Engine 24+ | Tests start services via `docker compose up -d` |
| Docker Compose v2 | `docker compose` (not `docker-compose`) subcommand |
| Python 3.12 | Test runner language |
| `pytest`, `pytest-asyncio`, `httpx` | Test framework and HTTP client |
| `openai>=1.57.0` | OpenAI SDK used to make LLM calls through gateway |
| `arize-phoenix>=12.0.0` | Optional — used for span correlation test (Test 4) |
| `.nasiko-local.env` with `LITELLM_MASTER_KEY` set | Required for all key-dependent tests |
| `OPENAI_API_KEY` set | Required for Tests 2, 3, 4 (live LLM calls) |
| `ANTHROPIC_API_KEY` set | Required for Test 3 full rotation (OpenAI → Anthropic) |

A full `.nasiko-local.env` starting point:

```bash
cp .nasiko-local.env.example .nasiko-local.env
# Then fill in:
#   LITELLM_MASTER_KEY=sk-<your-random-key>
#   LITELLM_SALT_KEY=<fernet-key>
#   OPENAI_API_KEY=sk-...
#   ANTHROPIC_API_KEY=sk-ant-...  (optional, for Test 3 full coverage)
```

---

## Running Locally

The conftest.py fixture handles the full compose lifecycle. Just run pytest:

```bash
# From repo root
pytest app/tests/integration/ -v
```

If you want to start the stack manually first and skip compose management:

```bash
# Start only the required services
docker compose \
  --env-file .nasiko-local.env \
  -f docker-compose.local.yml \
  up -d \
  litellm-postgres llm-gateway phoenix-observability redis mongodb

# Run tests (conftest detects services are already up)
pytest app/tests/integration/ -v

# Teardown (after tests)
docker compose --env-file .nasiko-local.env -f docker-compose.local.yml down -v
```

To run a single test file:

```bash
pytest app/tests/integration/test_gateway_reachable_from_agents_network.py -v
```

To run tests that don't need real API keys (reachability only):

```bash
pytest app/tests/integration/test_gateway_reachable_from_agents_network.py -v
```

---

## CI Behaviour

The `.github/workflows/ci.yml` `integration-tests` job:

1. Generates a `.nasiko-local.env` file inline with a random `LITELLM_MASTER_KEY`
   and `LITELLM_SALT_KEY` (no hardcoded secrets).
2. Reads `OPENAI_API_KEY` and `ANTHROPIC_API_KEY` from GitHub Repository Secrets
   (`${{ secrets.OPENAI_API_KEY }}`). If the secret is not set, the variable is
   empty and the relevant tests skip gracefully.
3. Starts the partial compose stack (gateway + postgres + phoenix + redis + mongodb).
4. Runs `pytest app/tests/integration/ -v --tb=short`.
5. On failure, dumps `docker compose logs` for `llm-gateway`, `litellm-postgres`,
   and `phoenix-observability` to aid debugging.
6. Tears down the stack with `docker compose down -v` (always, even on failure).

**Required GitHub Secrets** (all optional — tests skip without them):

| Secret | Used by |
|---|---|
| `OPENAI_API_KEY` | Tests 2, 3, 4 (live LLM calls) |
| `ANTHROPIC_API_KEY` | Test 3 (full Anthropic rotation) |
| `LITELLM_MASTER_KEY` | All key-dependent tests (auto-generated if absent) |

The `integration-tests` job **depends on** the `lint` job — it only starts if
`black --check` passes. It does not depend on `typecheck` to allow parallel execution.

---

## Test Descriptions

### Test 1 — `test_gateway_reachable_from_agents_network.py`

Verifies that the LLM gateway is up and reachable from two network perspectives:

- **Host-level (`:4100`):** `GET /health/liveliness` returns HTTP 200. Confirms
  the gateway process started and the proxy is alive.
- **Model list:** `GET /v1/models` returns a list that includes `default-model`,
  the rotation alias configured in `cli/setup/litellm/config.yaml`.
- **Intra-network (`agents-net:4000`):** Spins up a throwaway container on
  `agents-net` and runs `curl http://llm-gateway:4000/health/liveliness`. This is
  the exact network path agent containers use. If this sub-test fails, deployed
  agents cannot reach the gateway by internal DNS name.

This test has **no provider key dependency** — it only tests gateway liveness and
network topology, not LLM routing. It always runs in CI even without API keys.

### Test 2 — `test_sample_agent_calls_llm_via_gateway.py`

Verifies the core Track 2 contract: an agent can make LLM calls with no hardcoded
provider key in its source.

- **Source audit:** Scans `agents/a2a-gateway-demo/src/` for hardcoded API keys
  (regex match on `sk-...` pattern, ANTHROPIC_API_KEY literals, etc.). Zero
  violations are required. This test runs without a live stack.
- **Live LLM call:** Mints a virtual key via the gateway, then makes an
  `OpenAI.chat.completions.create()` call using `base_url=http://localhost:4100`
  and the virtual key as `api_key`. The gateway routes to the real provider. Asserts
  a non-empty response is returned.
- **Default-model alias:** Repeats the live call using `model="default-model"` to
  verify the rotation alias is functional.

Skips gracefully if `OPENAI_API_KEY` is absent.

### Test 3 — `test_provider_rotation_via_config.py`

Verifies that changing `cli/setup/litellm/config.yaml` and restarting the gateway
container switches providers without any agent code change.

Steps:
1. Call `default-model` → success (OpenAI-backed).
2. Edit config.yaml in-place: swap `default-model`'s `model:` from
   `openai/gpt-4o-mini` to `anthropic/claude-3-5-haiku-20241022`.
3. `docker compose restart llm-gateway`. Wait for healthcheck (up to 60s).
4. Call `default-model` again with the same virtual key → success (Anthropic-backed).
5. Teardown: restore config.yaml and restart gateway.

The test uses the same virtual key, the same model alias, and the same call site
in all steps — zero agent changes. This directly satisfies PS Acceptance Criterion 3.

Skips gracefully if either `OPENAI_API_KEY` or `ANTHROPIC_API_KEY` is absent.

### Test 4 — `test_gateway_span_parented_to_agent_trace.py`

Verifies W3C traceparent context propagation from the test caller through the
gateway to Phoenix.

Steps:
1. Generate a synthetic `traceparent` header with a known `traceId` and `parentSpanId`.
2. Make an LLM call through the gateway with `default_headers={"traceparent": ...}`.
3. Wait 15 seconds for LiteLLM's OTLP batch exporter to flush spans to Phoenix.
4. Query Phoenix for spans with the matching `traceId` (via arize-phoenix SDK or
   GraphQL).
5. Assert: a span with name matching `litellm*` exists for our `traceId`.
6. If `parentId` is available: assert it matches our injected `parentSpanId`.

This proves the observability chain: a real deployed agent using Langtrace would
have the same propagation — its OTel-instrumented OpenAI SDK automatically injects
`traceparent`, and LiteLLM creates a child span under that trace.

Skips gracefully if `OPENAI_API_KEY` is absent. Falls back to a soft skip (with
a note) if neither the Phoenix SDK nor GraphQL API are reachable for span queries.

---

## Known Limitations

1. **No mock provider** — Tests 2, 3, and 4 require real provider API keys
   (`OPENAI_API_KEY` and/or `ANTHROPIC_API_KEY`). Without keys, these tests skip
   gracefully but do not exercise the live LLM routing path.

2. **Provider rotation skips OpenAI-only setups** — Test 3 requires both
   `OPENAI_API_KEY` and `ANTHROPIC_API_KEY` for full coverage. If only OpenAI is
   available, the rotation is not demonstrated to a different provider class, only
   within OpenAI (gpt-4o-mini → gpt-4o). The test skips rather than doing a partial
   rotation to avoid false confidence.

3. **Span correlation depends on Phoenix latency** — Test 4 waits 15 seconds for
   span export. On a slow CI runner or a heavily loaded Phoenix instance, spans may
   not arrive in time. If this causes flakiness, increase `SPAN_EXPORT_WAIT_S` in
   the test file.

4. **Config.yaml format sensitivity** — Test 3 does a string replacement on the
   config file. If the file format changes (e.g., different indentation, YAML
   anchors), the replacement may not match and the test skips with a clear message
   rather than making an incorrect config change.

5. **`make start-nasiko` wipes volumes** — Running `make start-nasiko` between test
   runs destroys the `litellm-postgres-data` volume, which removes all virtual keys
   from LiteLLM's Postgres DB. Re-running the tests after a `make start-nasiko`
   will mint fresh keys (the conftest `mint_virtual_key` fixture always mints on
   each test session).

6. **Port conflicts** — The gateway is exposed on host port `4100`. If another
   process occupies this port, the compose stack will fail to start. The conftest
   will surface a clear error from docker-compose.
