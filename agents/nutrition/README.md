# Nutrition Assistant

An A2A agent that answers nutrition questions using live data from
[USDA FoodData Central](https://fdc.nal.usda.gov/) and
[Open Food Facts](https://world.openfoodfacts.org/).

The agent can:

- Search for whole foods and return calories and macronutrients per 100 g.
- Retrieve detailed vitamins, minerals, and fat breakdowns for a USDA FDC ID.
- Look up packaged foods by product name or barcode.
- Compare two to five foods side by side.

The model is instructed to use a data tool for every answer instead of relying on its own
nutrition knowledge.

## Tools

- `search_food` searches USDA FoodData Central and returns FDC IDs plus common macros.
- `get_nutrition` returns a detailed nutrient breakdown for one FDC ID.
- `open_food_facts` searches packaged products and returns labels, ingredients, and Nutri-Score.
- `compare_foods` compares two to five USDA FDC IDs in a single result.

Example questions:

```text
How many calories and how much protein are in 100 g of chicken breast?
Compare brown rice and quinoa.
Show the ingredients and Nutri-Score for barcode 3017620422003.
Which nutrients are present in an avocado?
```

## Project layout

```text
nutrition/
├── AgentCard.json       # A2A identity, capabilities, skills, and version
├── Cargo.toml           # Rust package and dependencies
├── Dockerfile           # Builds the binary in a rust:1-alpine stage, ships it FROM scratch
├── justfile             # Image build and push recipes
└── src/
    ├── main.rs          # A2A server, LLM loop, and streaming responses
    ├── tools.rs         # USDA and Open Food Facts integrations
    ├── mcp.rs           # Optional client for the platform's MCP gateway
    └── telemetry.rs     # OpenTelemetry and GenAI spans
```

## Configuration

The agent reads the following environment variables:

- `OPENAI_API_KEY` is required for direct LLM access. Nasiko may replace it with a short-lived
  gateway credential when its LLM router is configured.
- `OPENAI_BASE_URL` defaults to `https://api.openai.com/v1`. It can point to any compatible
  `/chat/completions` API or the Nasiko LLM gateway.
- `OPENAI_MODEL` defaults to `gpt-4o-mini`.
- `USDA_API_KEY` defaults to the shared `DEMO_KEY`. A personal key is strongly recommended
  because the demo key is heavily rate-limited.
- `PORT` defaults to `8000`.
- `RUST_LOG` controls log filtering and defaults to `info`.
- `OTEL_EXPORTER_OTLP_ENDPOINT` enables OTLP trace export when set.
- `OTEL_SERVICE_NAME` defaults to `nasiko-nutrition-agent`.
- `OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT=NO_CONTENT` disables prompt and response
  content on telemetry spans.
- `MCP_GATEWAY_URL` and `MCP_GATEWAY_TOKEN` are injected by Nasiko at deploy time. When both are
  set, the agent can also use tools from the platform's MCP gateway; without them it runs with its
  built-in tools only.

No `.env` file is required when the agent runs on Nasiko. The platform injects configured
credentials into the container at runtime.

For direct local development only, you may create an untracked `.env`:

```dotenv
OPENAI_API_KEY=replace-me
OPENAI_MODEL=gpt-4o-mini
USDA_API_KEY=replace-me
RUST_LOG=info
```

Do not commit real credentials.

## Prerequisites

- Docker (or Podman). The image compiles the agent itself, so no host Rust toolchain is needed
  to build it.
- The `nasiko` CLI for validation, local A2A testing, and deployment
- Optional: Rust stable and Cargo, for a native `cargo run` edit/build cycle
- Optional: [`just`](https://github.com/casey/just), for the `build`/`push`/`release` recipes

## Agent development lifecycle

This project is already scaffolded, so its lifecycle starts with validation rather than
`nasiko new`.

### 1. Validate the existing agent

From this directory, verify that the Dockerfile, Agent Card, and source layout satisfy the
Nasiko project checks:

```sh
nasiko validate .
```

The runtime contract is equally important: the container must serve its Agent Card at
`/.well-known/agent-card.json`, implement A2A JSON-RPC, and return HTTP 200 to health checks.
This agent answers both single-response and streaming A2A message requests.

### 2. Build, run, and test locally

The recommended local workflow uses the Nasiko CLI. It builds and starts the container directly;
no control plane or connected cluster is required.

```sh
cd agents/nutrition
nasiko run . --port 8000
```

The first build compiles the Rust binary inside Docker, so it takes a few minutes; later builds
reuse Docker's layer cache. After the container starts, chat with it over A2A:

```sh
nasiko chat http://localhost:8000
nasiko chat http://localhost:8000 \
  "Compare the protein and fiber in brown rice and quinoa."
nasiko chat http://localhost:8000 --tui
```

Check the running container's Agent Card with:

```sh
curl http://localhost:8000/.well-known/agent-card.json
```

For a native Rust edit/build cycle, run the process without Docker:

```sh
set -a
source .env
set +a
cargo run
```

The native server also binds to `0.0.0.0:8000` by default. `nasiko run` loads a local `.env` when
one exists. When the CLI is connected to a control plane, it can also fetch that platform's
fallback `OPENAI_*` configuration for the local container. Agent-specific secrets are only
injected into deployed containers, so use a local `.env` or exported key if the credential is
stored only at agent scope.

#### Building the image

The `Dockerfile` is a multi-stage build: a `rust:1-alpine` stage runs `cargo build --release`
(a static musl binary), and the final `FROM scratch` image holds only that binary and the CA
certificate bundle required for HTTPS calls. Any Docker build of this directory works:

```sh
nasiko build .     # the Nasiko way
just build         # docker build -t ${DOCKERHUB_USER:-nasiko}/nutrition .
```

`just push` pushes that tag, and `just release` does both. Set `DOCKER=podman` to use Podman
(the justfile picks it automatically when `/usr/bin/podman` exists).

### 3. Deploy to Nasiko

Start or connect to a Nasiko control plane, then authenticate:

```sh
# Remote cluster
nasiko connect https://nasiko.example.com --name prod
nasiko auth login

# Or register an already-running local control plane
nasiko connect http://localhost:8080 --name local

# Or let the CLI create and register a local cluster
nasiko up

nasiko clusters
nasiko use local
```

Commands in the rest of this section use whichever cluster is active.

Build and deploy:

```sh
nasiko deploy . --port 8000
```

`nasiko deploy`:

1. Builds a `linux/amd64` image for the control-plane runtime.
2. Pushes it to Nasiko's embedded OCI registry.
3. Registers or updates the agent.
4. Starts its container.
5. Writes `.nasiko/agent.json` to bind this directory to the deployed agent.

Keep `.nasiko/` out of source control.

The agent is named after the `name` in `AgentCard.json`, `nutrition_assistant`. Docker image and
registry names cannot contain spaces or uppercase letters, so keep the card's `name` a lowercase,
registry-safe identifier; pass `--name` to deploy under a different one.

#### Secrets

Configure credentials in the Nasiko platform as encrypted agent or vault secrets. They are
injected when the container starts, so they do not need to exist in this directory:

```sh
# Vault scope: available to every agent you own
nasiko secrets set OPENAI_API_KEY replace-me
nasiko secrets ls

# Agent scope: overrides a vault secret for this agent
nasiko secrets set OPENAI_API_KEY replace-me --agent nutrition_assistant
nasiko secrets set USDA_API_KEY replace-me --agent nutrition_assistant
nasiko secrets ls --agent nutrition_assistant
nasiko restart nutrition_assistant
```

Secret precedence is: inline `nasiko deploy -e` values, then agent-specific secrets, then vault
secrets. Updating a secret does not alter an existing container; run `nasiko restart` to recreate
the container with the new value. Remove a secret with `nasiko secrets rm <KEY>` and add
`--agent nutrition_assistant` when removing an agent-specific value.

#### Server-side build

`nasiko upload .` (or the dashboard's source upload) sends this directory to the server, which
builds the same `Dockerfile` and deploys it, with no local Docker needed. The Dockerfile's
`ENTRYPOINT` is what the server's upload check looks for in a non-Python agent.

### 4. Operate the deployed agent

Every command operates against the active cluster selected by `nasiko use`.

Inspect the deployment and control its lifecycle:

```sh
nasiko ps
nasiko ps --json
nasiko logs nutrition_assistant
nasiko logs nutrition_assistant -n 200
nasiko stop nutrition_assistant
nasiko start nutrition_assistant
nasiko restart nutrition_assistant
nasiko scale nutrition_assistant 2
nasiko status
```

`stop` followed by `start` pauses and resumes the existing configuration. `restart` destroys and
recreates the container, which is required to pick up changed secrets or environment variables.

Chat directly with the deployed agent:

```sh
nasiko chat nutrition_assistant \
  "Compare the protein and fiber in brown rice and quinoa."
```

Control-plane session IDs begin with `ses_` and must be resumed through the deployed agent, not
through the standalone `http://localhost:8000` container. List sessions and continue one with:

```sh
nasiko sessions

# Resume in the TUI; --resume opens the TUI automatically.
nasiko chat --agent nutrition_assistant --resume <session-id>

# Send one message to the existing session.
nasiko chat --agent nutrition_assistant \
  --session-id <session-id> \
  "Continue"
```

If the session belongs to the orchestrator rather than this agent, omit
`--agent nutrition_assistant`. Replace `<session-id>` with the actual ID without angle brackets.

Open the observability experience after the agent has handled traffic:

```sh
nasiko observe sessions
nasiko observe session <session-id>
```

Sessions, distributed traces, spans, token usage, and cost are available automatically. The
agent's telemetry implementation joins Nasiko's incoming trace and records its LLM/tool activity;
deployment also injects the platform's observability configuration.

Remove the deployment when it is no longer needed:

```sh
nasiko rm --name nutrition_assistant
nasiko rm --name nutrition_assistant -f
```

This terminates and deregisters the agent. It is different from `stop`, which keeps the
registration and configuration.

### 5. Version and iterate

The `version` in `AgentCard.json` becomes the deployed image tag. For a new release:

1. Change the agent source.
2. Bump `version` in `AgentCard.json`. It is the version Nasiko uses: `AgentCard.json` wins over
   `Cargo.toml`, whose package version is not read while the card has one.
3. Run the local validation and chat workflow again.
4. Redeploy.

```sh
nasiko validate .
nasiko run . --port 8000
nasiko chat http://localhost:8000 "What can you do?"
nasiko deploy . --name nutrition_assistant --port 8000
```

Agent names are unique per cluster. Redeploying `nutrition_assistant` updates that agent in
place, even when the version has not changed. `nasiko restart` does not rebuild an image or
change its version.

Roll back to the previous deployed version, or select a specific version:

```sh
nasiko rollback --name nutrition_assistant
nasiko rollback --name nutrition_assistant --version 1.0.0
```

For CI, build, push, and deploy the new version:

```sh
nasiko deploy . --name nutrition_assistant --port 8000
```

## Building a compatible Nasiko agent

This project illustrates the core contract expected from a containerized Nasiko agent:

1. Provide a valid `AgentCard.json` describing identity, version, skills, transport, and
   capabilities.
2. Provide a Dockerfile that produces a self-contained image.
3. Bind the service to `0.0.0.0`, not only `127.0.0.1`.
4. Honor the `PORT` environment variable and default to port `8000`.
5. Expose an A2A JSON-RPC handler and the standard Agent Card endpoint.
6. Emit valid A2A status and artifact events when streaming is advertised.
7. Read credentials and provider endpoints from environment variables; never bake them into the
   image.
8. Build for `linux/amd64` when deploying to the current Nasiko control-plane runtime.
9. Use a lowercase, registry-safe agent name and keep versions consistent across project files.
10. Treat `.nasiko/agent.json` as local deployment state rather than source.

This agent compiles in a Docker builder stage; other languages can do the same or copy
interpreted source directly.

## Troubleshooting

### `invalid reference format`

Docker received a name it cannot use as an image name (spaces or uppercase letters). Keep the
`name` in `AgentCard.json` lowercase and registry-safe, or deploy with an explicit one:

```sh
nasiko deploy . --name nutrition_assistant
```

### `exec format error`

The image architecture does not match the host. Use `nasiko deploy`, which builds the deployment
image for `linux/amd64`.

### USDA requests return 403 or 429

The shared `DEMO_KEY` has been throttled. Set a personal `USDA_API_KEY`; the agent intentionally
does not retry rate-limited requests.

### LLM requests fail with 401

Set `OPENAI_API_KEY`, or verify that the Nasiko LLM gateway is configured and injecting
`OPENAI_BASE_URL` and its short-lived credential.

## Further reading

- [Agent development lifecycle](../../docs/AGENT_LIFECYCLE.md)
- [Nasiko project README](../../README.md)
- [A2A agents and frameworks](https://docs.nasiko.com/adlc/a2a-agents)
- [Running and chatting locally](https://docs.nasiko.com/adlc/build-run-test)
- [Deploying and managing agents](https://docs.nasiko.com/adlc/deploy)
- [Versions and lifecycle](https://docs.nasiko.com/adlc/versions-lifecycle)
- [A2A protocol specification](https://github.com/a2aproject/a2a-spec)
