# Nasiko DevOps Agent

A streaming A2A chat agent that answers DevOps questions with live lookups. It runs a short
tool loop against an OpenAI-compatible model and must back claims with tool output rather than
model memory.

Example requests:

```text
What's the status of the kubernetes/kubernetes repo?
Show recent CI runs for tokio-rs/tokio.
Find official PostgreSQL images on Docker Hub.
Is https://httpbin.org/status/200 healthy?
How do I set up GitHub Actions for Rust?
```

The agent looks up repositories, CI runs, container images, endpoint health, and documentation.
It does not deploy, scale, or change infrastructure.

## Tools

The model can call these tools, with at most four tool-call rounds per request:

- `github_repo_info` returns stars, forks, open issues, language, license, and last push for a
  GitHub repository.
- `github_actions_runs` lists recent GitHub Actions workflow runs for a repository.
- `docker_hub_search` searches Docker Hub and reports official images, stars, and pull counts.
- `check_endpoint` probes an HTTP URL for status code, latency, and content type (10-second
  timeout).
- `web_search` searches the public web for current documentation and troubleshooting.

GitHub requests use the unauthenticated public API with a `User-Agent` header. Rate limits are
tighter without a token, and private repositories are not visible.

## Project layout

```text
devops-agent/
├── AgentCard.json          # Nasiko identity, capabilities, skills, and release version
├── Cargo.toml              # Standalone Rust crate and dependencies
├── Dockerfile              # Alpine builder and scratch runtime
├── justfile                # Legacy Docker Hub image recipes; not required by the Nasiko ADLC
└── src/
    ├── main.rs             # A2A server, Agent Card, and tool loop
    ├── tools.rs            # GitHub, Docker Hub, endpoint, and web-search tools
    └── telemetry.rs        # OpenTelemetry and GenAI spans
```

The crate is intentionally standalone rather than a member of the repository's root Cargo
workspace. The Nasiko CLI can therefore build it directly from this directory.

The Nasiko CLI manages validation, local execution, testing, deployment, and operations. The
Dockerfile defines the runtime, but developers do not need to invoke Docker, Compose, `cargo`,
or `just` directly.

## Configuration

The agent reads these environment variables:

- `OPENAI_API_KEY` is required for the tool loop.
- `OPENAI_BASE_URL` defaults to `https://api.openai.com/v1` and may point to any compatible
  `/chat/completions` endpoint or the Nasiko LLM gateway.
- `OPENAI_MODEL` defaults to `gpt-4o-mini`.
- `PORT` defaults to `8000`.
- `RUST_LOG` controls runtime log filtering and defaults to `info`.
- `OTEL_EXPORTER_OTLP_ENDPOINT` enables OTLP trace export when set.
- `OTEL_SERVICE_NAME` defaults to `nasiko-devops-agent`.
- `OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT=NO_CONTENT` disables prompt and response
  content on telemetry spans.

The image listens on internal port `8000`. The Nasiko CLI maps or publishes this port through
`nasiko run` or `nasiko deploy`.

No `.env` file is required when the agent is deployed to Nasiko. Nasiko injects configured
secrets and observability settings when the container starts.

For local testing, create an untracked `.env`:

```dotenv
OPENAI_API_KEY=replace-me
OPENAI_MODEL=gpt-4o-mini
RUST_LOG=info
```

Do not commit real credentials.

## Prerequisites

- The `nasiko` CLI
- Docker or Podman running as the container runtime used internally by the CLI
- An OpenAI API key or access to a compatible model endpoint

No host Rust toolchain, `just`, or manual container commands are required. The Nasiko CLI builds
the static binary inside the Dockerfile's builder stage.

## Agent Development Lifecycle (ADLC)

The Nasiko Agent Development Lifecycle (ADLC) covers validation, local testing, deployment,
operation, and iteration. This agent is already scaffolded, so its ADLC starts with validation
rather than `nasiko new`.

### 1. Validate the existing agent

From this directory:

```sh
cd agents/devops-agent
nasiko validate .
```

Validation checks the Dockerfile, `AgentCard.json`, required metadata, and source layout. The
runtime must bind to `0.0.0.0`, serve its card at `/.well-known/agent-card.json`, and accept A2A
JSON-RPC requests.

### 2. Build, run, and test locally

After creating the untracked `.env`, let the Nasiko CLI build and start the agent:

```sh
nasiko run . --port 8000
```

`nasiko run` builds the development image, starts the agent, maps the selected host port to
container port `8000`, and loads `.env`. No Nasiko control plane or connected cluster is
required.

In another terminal, test it through the Nasiko CLI:

```sh
nasiko chat http://localhost:8000 \
  "What's the status of the kubernetes/kubernetes repo?"
nasiko chat http://localhost:8000 --tui
```

For a build-only check:

```sh
nasiko build . --tag devops-agent:1.0.0
```

`nasiko build` creates the image without starting the agent. Both commands manage the container
runtime for you. The first build compiles the Rust crate inside Docker and can take several
minutes.

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

Store the model credential as an encrypted Nasiko secret:

```sh
# Vault scope: available to every agent you own
nasiko secrets set OPENAI_API_KEY replace-me
nasiko secrets ls

# Agent scope: overrides the vault secret for this agent
nasiko secrets set OPENAI_API_KEY replace-me \
  --agent devops-agent
nasiko secrets ls --agent devops-agent
```

Secret precedence is: inline `nasiko deploy -e` values, agent-specific secrets, then vault
secrets. Updating a secret does not change a running container; recreate it with:

```sh
nasiko restart devops-agent
```

Remove a vault secret with `nasiko secrets rm <KEY>`. Add
`--agent devops-agent` when removing an agent-specific secret.

#### Build and deploy

From `agents/devops-agent`:

```sh
nasiko deploy . \
  --name devops-agent \
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

Do not use `nasiko upload` for this Rust-only agent yet. The current server-side source validator
requires a Python entrypoint and rejects the project before building its Dockerfile.
`nasiko deploy` is the supported path because it builds the image locally and uploads it to
Nasiko.

### 4. Operate the deployed agent

Inspect and manage the deployment:

```sh
nasiko ps
nasiko ps --json
nasiko logs devops-agent
nasiko logs devops-agent -n 200
nasiko logs devops-agent -n 200 -f
nasiko stop devops-agent
nasiko start devops-agent
nasiko restart devops-agent
nasiko scale devops-agent 2
nasiko status
```

Chat with the deployed agent:

```sh
nasiko chat devops-agent "Show recent CI runs for tokio-rs/tokio"
```

List and resume control-plane sessions:

```sh
nasiko sessions
nasiko chat --agent devops-agent --resume <session-id>
nasiko chat --agent devops-agent \
  --session-id <session-id> \
  "Now find official PostgreSQL images on Docker Hub."
```

Control-plane session IDs begin with `ses_` and must be resumed through the deployed agent, not
through the standalone `http://localhost:8000` development agent. The `--resume` option opens
the TUI.

Open Nasiko observability after the agent has handled traffic:

```sh
nasiko observe sessions
nasiko observe session <session-id>
```

The telemetry bootstrap joins Nasiko's incoming trace context and records A2A and GenAI spans
when `OTEL_EXPORTER_OTLP_ENDPOINT` is set. Nasiko injects the collector endpoint during
deployment.

Remove the deployment when it is no longer needed:

```sh
nasiko rm --name devops-agent
nasiko rm --name devops-agent -f
```

`stop` preserves registration and configuration. `rm` terminates and deregisters the agent.

### 5. Version and iterate

The version in `AgentCard.json` becomes the deployed image tag. For each release:

1. Change the source.
2. Bump `AgentCard.json`.
3. Keep `Cargo.toml` and the runtime Agent Card in `src/main.rs` aligned.
4. Validate and test locally.
5. Redeploy the same agent name.

`AgentCard.json`, `Cargo.toml`, and the runtime Agent Card in `src/main.rs` all report `1.0.0`.

```sh
nasiko validate .
nasiko run . --port 8000
nasiko chat http://localhost:8000 \
  "What's the status of the kubernetes/kubernetes repo?"
nasiko deploy . \
  --name devops-agent \
  --port 8000
```

Redeploying the same name updates the agent in place. `nasiko restart` recreates the existing
container to pick up changed secrets or environment variables; it does not rebuild the image.

Roll back to the previous deployment or a specific version:

```sh
nasiko rollback --name devops-agent
nasiko rollback --name devops-agent --version 1.0.0
```

## Building a compatible Nasiko agent

This project demonstrates the core contract for a streaming Rust agent on Nasiko:

1. Provide a valid, registry-safe `AgentCard.json`.
2. Provide a Dockerfile that creates a self-contained `linux/amd64` image.
3. Bind to `0.0.0.0` and honor `PORT`.
4. Serve the standard Agent Card endpoint and an A2A JSON-RPC handler.
5. Emit valid working status, artifact, and completed events when streaming is advertised.
6. Read the OpenAI credential and model name from environment variables.
7. Initialize telemetry at process start so incoming `traceparent` joins the session.
8. Treat `.nasiko/agent.json` as local deployment state rather than source.
9. Keep project, runtime, and deployment metadata aligned.

## Troubleshooting

### Provider requests fail with 401

Verify `OPENAI_API_KEY` is a valid OpenAI API key. After changing the Nasiko secret, run:

```sh
nasiko restart devops-agent
```

### Provider reports that the model is unavailable

Set `OPENAI_MODEL` to a model available to your OpenAI account, or set `OPENAI_BASE_URL` to a
compatible endpoint that serves that model.

### GitHub lookups fail or return rate-limit errors

The tools call the public GitHub API without a token. Wait and retry, or ask about a public
repository. Private repositories are not accessible.

### Endpoint checks time out

`check_endpoint` waits at most 10 seconds. Confirm the URL includes a scheme (`https://…`) and
that the target is reachable from the agent container.

### `just` is not required

Do not run `just build` or `just push` as part of the Nasiko ADLC. Those recipes tag and push a
Docker Hub image. `nasiko deploy` builds from the Dockerfile, which compiles the crate in the
builder stage.

### Source upload is rejected

Use `nasiko deploy`, not `nasiko upload`. The current source-upload validator requires a Python
entrypoint and does not accept this Rust-only source tree.

## Further reading

- [Agent Development Lifecycle (ADLC)](../../docs/AGENT_LIFECYCLE.md)
- [Nasiko project README](../../README.md)
- [A2A agents and frameworks](https://docs.nasiko.com/adlc/a2a-agents)
- [Running and chatting locally](https://docs.nasiko.com/adlc/build-run-test)
- [Deploying and managing agents](https://docs.nasiko.com/adlc/deploy)
- [Versions and lifecycle](https://docs.nasiko.com/adlc/versions-lifecycle)
- [A2A protocol specification](https://github.com/a2aproject/a2a-spec)
