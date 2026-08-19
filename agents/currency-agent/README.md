# Nasiko Currency Agent

A streaming A2A agent that converts amounts between currencies. OpenAI parses the natural-language
request; a fixed USD-based rate table then computes the result so the arithmetic stays
deterministic.

Example requests:

```text
100 USD to EUR
50 GBP to INR
How much is 1000 yen in dollars?
```

Supported currencies: USD, EUR, GBP, JPY, INR, CAD, AUD, CHF, CNY, BRL.

The agent does not fetch live market rates. Callers should treat the conversion as a
demonstration using the baked-in table, not as a source of financial truth.

## Architecture

1. The A2A server receives a text prompt.
2. `CurrencyExecutor` creates or resumes an in-memory A2A task.
3. OpenAI extracts `{amount, from, to}` as JSON.
4. The local rate table converts the amount through USD.
5. The formatted conversion is returned as a text artifact.

The implementation uses the official OpenAI Python SDK and defaults to `gpt-4o-mini`.

## Project layout

```text
currency-agent/
├── AgentCard.json          # Nasiko deployment identity, capabilities, skills, and version
├── Dockerfile              # Python 3.13 runtime image
├── pyproject.toml          # Python project metadata and dependencies
└── src/
    ├── __main__.py         # A2A server, OpenAI parsing, and conversion
    └── telemetry.py        # OpenTelemetry initialization
```

The Nasiko CLI manages validation, local execution, testing, deployment, and operations. The
Dockerfile defines the runtime, but developers do not need to invoke Docker or Compose directly.

## Configuration

The agent reads these environment variables:

- `OPENAI_API_KEY` is required for OpenAI API access.
- `OPENAI_MODEL` defaults to `gpt-4o-mini`.
- `OPENAI_BASE_URL` is optional and may point to any OpenAI-compatible `/chat/completions` endpoint.
- `HOST_OVERRIDE` replaces the URL advertised by the runtime Agent Card when set.
- `OTEL_EXPORTER_OTLP_ENDPOINT` enables OTLP trace and metric export.
- `OTEL_SERVICE_NAME` defaults to `currency-agent`.

The image listens on internal port `8000`. The Nasiko CLI maps or publishes this port through
`nasiko run`, `nasiko deploy`, or `nasiko upload`.

No `.env` file is required when the agent is deployed to Nasiko. Nasiko injects configured
secrets and observability settings when the container starts.

For local testing, create an untracked `.env`:

```dotenv
OPENAI_API_KEY=replace-me
OPENAI_MODEL=gpt-4o-mini
```

Do not commit real credentials.

## Prerequisites

- The `nasiko` CLI
- Docker or Podman running as the container runtime used internally by the CLI
- An OpenAI API key with available quota

No local Python environment, `uv`, or manual container commands are required. The Nasiko CLI
builds the Python runtime from the Dockerfile.

## Agent Development Lifecycle (ADLC)

The Nasiko Agent Development Lifecycle (ADLC) covers validation, local testing, deployment,
operation, and iteration. This agent is already scaffolded, so its ADLC starts with validation
rather than `nasiko new`.

### 1. Validate the existing agent

From this directory:

```sh
cd agents/currency-agent
nasiko validate .
```

Validation checks the Dockerfile, `AgentCard.json`, required metadata, and source layout. The
runtime must bind to `0.0.0.0`, serve an Agent Card at
`/.well-known/agent-card.json`, and accept A2A JSON-RPC requests.

### 2. Build, run, and test locally

After creating the untracked `.env`, let the Nasiko CLI build and start the agent:

```sh
nasiko run . --port 8000
```

`nasiko run` builds the development image, starts the agent, maps the selected host port to
container port `8000`, and loads `.env`. No Nasiko control plane or connected cluster is
required.

In another terminal, test the agent through the Nasiko CLI:

```sh
nasiko chat http://localhost:8000 "100 USD to EUR"
nasiko chat http://localhost:8000 --tui
```

For a build-only check:

```sh
nasiko build . --tag currency-agent:1.0.0
```

`nasiko build` creates the image without starting the agent. Both commands manage the container
runtime for you.

### 3. Deploy to Nasiko

Start or connect to a Nasiko control plane and select the cluster:

```sh
# Connect to a remote cluster
nasiko connect https://nasiko.example.com --name prod
nasiko auth login

# Or register an already-running local control plane
nasiko connect http://localhost:8080 --name local

# Or let the CLI create and register a local cluster
nasiko up

nasiko clusters
nasiko use local
```

Commands below operate against the active cluster.

#### Configure secrets

Store the provider credential as an encrypted Nasiko secret:

```sh
# Vault scope: available to every agent you own
nasiko secrets set OPENAI_API_KEY replace-me
nasiko secrets ls

# Agent scope: overrides the vault secret for this agent
nasiko secrets set OPENAI_API_KEY replace-me \
  --agent currency-agent
nasiko secrets ls --agent currency-agent
```

Secret precedence is: inline `nasiko deploy -e` values, agent-specific secrets, then vault
secrets. Updating a secret does not change a running container; recreate it with:

```sh
nasiko restart currency-agent
```

Remove a vault secret with `nasiko secrets rm <KEY>`. Add
`--agent currency-agent` when removing an agent-specific secret.

#### Build and deploy

From `agents/currency-agent`:

```sh
nasiko deploy . \
  --name currency-agent \
  --port 8000
```

`nasiko deploy`:

1. Builds a `linux/amd64` image for the Nasiko runtime.
2. Pushes it to Nasiko's embedded OCI registry.
3. Registers or updates the agent.
4. Starts its container with configured secrets and observability settings.
5. Writes `.nasiko/agent.json` to bind this directory to the deployment.

Keep `.nasiko/` out of source control. The explicit deployment name matches the registry-safe
name in `AgentCard.json`.

#### Server-side source build

This Python project provides `src/__main__.py`, so it also supports Nasiko's source-upload flow:

```sh
nasiko upload . \
  --name currency-agent \
  --version 1.0.0 \
  --port 8000
```

`nasiko upload` packages the source and lets the active Nasiko cluster build it. Use
`nasiko deploy` for a local CLI-managed build and upload, or `nasiko upload` when the build
should happen on the server.

### 4. Operate the deployed agent

Inspect and manage the deployment:

```sh
nasiko ps
nasiko ps --json
nasiko logs currency-agent
nasiko logs currency-agent -n 200
nasiko logs currency-agent -n 200 -f
nasiko stop currency-agent
nasiko start currency-agent
nasiko restart currency-agent
nasiko scale currency-agent 2
nasiko status
```

Chat with the deployed agent:

```sh
nasiko chat currency-agent "How much is 50 GBP in INR?"
```

List and resume control-plane sessions:

```sh
nasiko sessions
nasiko chat --agent currency-agent --resume <session-id>
nasiko chat --agent currency-agent \
  --session-id <session-id> \
  "Convert that same amount to JPY instead."
```

Control-plane session IDs begin with `ses_` and must be resumed through the deployed agent, not
through the standalone `http://localhost:8000` development agent. The `--resume` option opens
the TUI.

Open Nasiko observability after the agent has handled traffic:

```sh
nasiko observe sessions
nasiko observe session <session-id>
```

The telemetry bootstrap joins Nasiko's incoming trace context and instruments the A2A server,
HTTP client, and OpenAI SDK when the corresponding instrumentation packages are available.
Nasiko injects the collector endpoint during deployment.

Remove the deployment when it is no longer needed:

```sh
nasiko rm --name currency-agent
nasiko rm --name currency-agent -f
```

`stop` preserves registration and configuration. `rm` terminates and deregisters the agent.

### 5. Version and iterate

The version in `AgentCard.json` becomes the deployed image tag. For each release:

1. Change the source.
2. Bump `AgentCard.json`.
3. Keep `pyproject.toml` and the runtime Agent Card in `src/__main__.py` aligned.
4. Validate and test locally.
5. Redeploy the same agent name.

```sh
nasiko validate .
nasiko run . --port 8000
nasiko chat http://localhost:8000 "100 USD to EUR"
nasiko deploy . \
  --name currency-agent \
  --port 8000
```

Redeploying the same name updates the agent in place. `nasiko restart` recreates the existing
container to pick up changed secrets or environment variables; it does not rebuild the image.

Roll back to the previous deployment or a specific version:

```sh
nasiko rollback --name currency-agent
nasiko rollback --name currency-agent --version 1.0.0
```

## Building a compatible Nasiko agent

This project demonstrates the core contract for a streaming Python agent on Nasiko:

1. Provide a valid, registry-safe `AgentCard.json`.
2. Provide a Dockerfile that creates a self-contained `linux/amd64` image.
3. Bind to `0.0.0.0` and use the configured deployment port.
4. Serve the standard Agent Card endpoint and an A2A JSON-RPC handler.
5. Emit valid working status, artifact, and completed events when streaming is advertised.
6. Read the OpenAI credential and model name from environment variables.
7. Initialize telemetry before importing instrumented server libraries.
8. Treat `.nasiko/agent.json` as local deployment state rather than source.
9. Keep project, runtime, and deployment metadata aligned.

## Troubleshooting

### Provider requests fail with 401

Verify `OPENAI_API_KEY` is a valid OpenAI API key. After changing the Nasiko secret, run:

```sh
nasiko restart currency-agent
```

### Provider reports that the model is unavailable

Set `OPENAI_MODEL` to a model available to your OpenAI account, or set `OPENAI_BASE_URL` to a
compatible endpoint that serves that model.

### Nasiko reports `-32601 Method not found`

The current agent uses `a2a-sdk==1.1.2` and A2A protocol 1.0. This error usually means the
deployed container still uses an older image. Bump the release version if needed and run
`nasiko deploy` again; `nasiko restart` does not rebuild the image.

### The Agent Card advertises the wrong URL

Nasiko manages routing for deployed agents. If direct runtime testing needs a different
advertised URL, set `HOST_OVERRIDE` to the externally reachable base URL.

### Docker build cannot find `src/` or `pyproject.toml`

Build from this directory with `nasiko build .` or `nasiko deploy .`. The Dockerfile copies
`pyproject.toml` and `src/` from the agent directory itself, not from the repository root.

### Source upload fails

Ensure `src/__main__.py`, `Dockerfile`, and `AgentCard.json` are at their current project paths.
They satisfy Nasiko's Python source-upload validator.

## Further reading

- [Agent Development Lifecycle (ADLC)](../../docs/AGENT_LIFECYCLE.md)
- [Nasiko project README](../../README.md)
- [A2A agents and frameworks](https://docs.nasiko.com/adlc/a2a-agents)
- [Running and chatting locally](https://docs.nasiko.com/adlc/build-run-test)
- [Deploying and managing agents](https://docs.nasiko.com/adlc/deploy)
- [Versions and lifecycle](https://docs.nasiko.com/adlc/versions-lifecycle)
- [OpenAI Python SDK](https://github.com/openai/openai-python)
- [A2A protocol specification](https://github.com/a2aproject/a2a-spec)
