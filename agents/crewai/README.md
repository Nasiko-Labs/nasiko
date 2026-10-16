# Image Generator Agent

An A2A image-generation agent built with
[CrewAI](https://www.crewai.com/open-source), OpenAI, and the Google Gemini API.
CrewAI interprets the request and invokes a tool that creates or edits an image. The result is
returned as an A2A image artifact.

Example requests:

```text
Generate a photorealistic image of raspberry lemonade.
Create a minimal app icon of a mountain at sunrise.
Edit the previous image to use a blue background.
```

## Architecture

1. The A2A server receives a text prompt.
2. A CrewAI agent uses OpenAI to interpret the request.
3. The image-generation tool sends the prompt and any referenced prior image to Gemini.
4. The generated image is held in the in-memory session cache.
5. The A2A response returns the image bytes as an artifact.

The agent does not support streaming. Its cache is process-local and is cleared whenever the
container restarts.

## Project layout

```text
crewai/
├── AgentCard.json          # Nasiko identity, capabilities, skills, and version
├── Dockerfile              # Python 3.12 runtime image
├── pyproject.toml          # Python project metadata and dependencies
└── src/
    ├── __main__.py         # A2A server and runtime Agent Card
    ├── agent.py            # CrewAI workflow and Gemini image tool
    ├── agent_executor.py   # A2A request and artifact handling
    ├── in_memory_cache.py  # Generated-image session cache
    └── telemetry.py        # OpenTelemetry bootstrap
```

The Nasiko CLI manages validation, local execution, testing, deployment, and operations. The
Dockerfile defines the deployable runtime, but developers do not need to run Docker or Compose
commands directly.

## Configuration

The agent reads these environment variables:

- `OPENAI_API_KEY` is required at startup and is used by CrewAI's OpenAI model.
- `GOOGLE_API_KEY` is required when the image-generation tool calls Gemini.
- `HOST_OVERRIDE` optionally replaces the URL advertised by the runtime Agent Card.
- `OTEL_EXPORTER_OTLP_ENDPOINT` enables OTLP trace and metric export.
- `OTEL_SERVICE_NAME` defaults to `nasiko-agent`.

The agent image listens on internal port `8000`. The CLI maps or publishes that port as part of
`nasiko run`, `nasiko deploy`, or `nasiko upload`.

No `.env` file is required when the agent is deployed to Nasiko. Nasiko injects configured
secrets into the container at runtime. For local testing with `nasiko run`, create an untracked
`.env` if the credentials are not otherwise available:

```dotenv
OPENAI_API_KEY=replace-me
GOOGLE_API_KEY=replace-me
```

Do not commit real credentials.

## Prerequisites

- The `nasiko` CLI
- Docker or Podman running as the container runtime used internally by the CLI
- A valid OpenAI API key
- A Google API key with Gemini image-generation access and available quota

You do not need `just`, `uv`, a local Python environment, or manual container commands. The
Nasiko CLI builds the Python runtime from the Dockerfile.

## Agent Development Lifecycle (ADLC)

The Nasiko Agent Development Lifecycle (ADLC) covers validation, local testing, deployment,
operation, and iteration. This project is already scaffolded, so its ADLC starts with validation
rather than `nasiko new`.

### 1. Validate the existing agent

From this directory, validate the Dockerfile, Agent Card, and source layout:

```sh
cd agents/crewai
nasiko validate .
```

The runtime must bind to `0.0.0.0`, serve an Agent Card at
`/.well-known/agent-card.json`, and accept A2A JSON-RPC requests.

### 2. Build, run, and test locally

Create an untracked `.env` containing both API keys, then let the Nasiko CLI build and start the
agent:

```sh
nasiko run . --port 8000
```

`nasiko run` builds the development image, starts the agent, maps the selected host port to
container port `8000`, and loads the local `.env`. No local control plane or connected cluster
is required.

Agent-specific Nasiko secrets are only injected into deployed agents. For local runs, keep the
two credentials in the untracked `.env`.

In another terminal, send a prompt through the Nasiko CLI:

```sh
nasiko chat http://localhost:8000 \
  "Generate a minimal illustration of a red circle on a white background."
nasiko chat http://localhost:8000 --tui
```

For a build-only check:

```sh
nasiko build . --tag image-generator-agent:1.0.0
```

`nasiko build` creates the image without starting the agent. Both `nasiko run` and
`nasiko build` manage the container runtime for you.

### 3. Deploy to Nasiko

Start or connect to a control plane and select the cluster:

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

Commands below operate against whichever cluster is active.

#### Configure secrets

Store credentials as encrypted Nasiko secrets:

```sh
# Vault scope: available to every agent you own
nasiko secrets set OPENAI_API_KEY replace-me
nasiko secrets set GOOGLE_API_KEY replace-me
nasiko secrets ls

# Agent scope: overrides a vault secret for this deployment
nasiko secrets set OPENAI_API_KEY replace-me --agent image-generator-agent
nasiko secrets set GOOGLE_API_KEY replace-me --agent image-generator-agent
nasiko secrets ls --agent image-generator-agent
```

Secret precedence is: inline `nasiko deploy -e` values, agent-specific secrets, then vault
secrets. Updating a secret does not change an existing container; recreate it with:

```sh
nasiko restart image-generator-agent
```

Remove a vault secret with `nasiko secrets rm <KEY>`. Add
`--agent image-generator-agent` when removing an agent-specific secret.

#### Build and deploy

From `agents/crewai`:

```sh
nasiko deploy . \
  --name image-generator-agent \
  --port 8000
```

`nasiko deploy`:

1. Builds a `linux/amd64` image for the control-plane runtime.
2. Pushes it to Nasiko's embedded OCI registry.
3. Registers or updates the agent.
4. Starts its container with the configured secrets and observability settings.
5. Writes `.nasiko/agent.json` to bind this directory to the deployment.

Keep `.nasiko/` out of source control. The explicit name matches the registry-safe name in
`AgentCard.json` and makes the deployment target clear.

#### Server-side source build

Because this Python project provides `src/__main__.py`, it can also use Nasiko's source-upload
workflow:

```sh
nasiko upload . \
  --name image-generator-agent \
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
nasiko logs image-generator-agent
nasiko logs image-generator-agent -n 200
nasiko logs image-generator-agent -n 200 -f
nasiko stop image-generator-agent
nasiko start image-generator-agent
nasiko restart image-generator-agent
nasiko scale image-generator-agent 2
nasiko status
```

Chat with the deployed agent:

```sh
nasiko chat image-generator-agent \
  "Generate a watercolor painting of a lighthouse in a storm."
```

List and resume control-plane sessions:

```sh
nasiko sessions
nasiko chat --agent image-generator-agent --resume <session-id>
nasiko chat --agent image-generator-agent \
  --session-id <session-id> \
  "Make the sky darker."
```

Control-plane session IDs begin with `ses_` and must be resumed through the deployed agent, not
through the standalone `http://localhost:8000` development agent. The `--resume` option opens
the TUI automatically.

Open observability after the agent has handled traffic:

```sh
nasiko observe sessions
nasiko observe session <session-id>
```

Remove the deployment when it is no longer needed:

```sh
nasiko rm --name image-generator-agent
nasiko rm --name image-generator-agent -f
```

`stop` preserves the registration and configuration. `rm` terminates and deregisters the agent.

### 5. Version and iterate

The `version` in `AgentCard.json` becomes the deployed image tag. For each release:

1. Change the source.
2. Bump the version in `AgentCard.json`.
3. Keep `pyproject.toml` and the runtime Agent Card in `src/__main__.py` aligned.
4. Validate and test locally.
5. Redeploy the same agent name.

```sh
nasiko validate .
nasiko run . --port 8000
nasiko chat http://localhost:8000 "Generate a simple test image."
nasiko deploy . --name image-generator-agent --port 8000
```

Redeploying `image-generator-agent` updates it in place. `nasiko restart` only recreates the
existing container; it does not rebuild the image.

Roll back to the previous deployment or a specific version:

```sh
nasiko rollback --name image-generator-agent
nasiko rollback --name image-generator-agent --version 1.0.0
```

## Building a compatible Nasiko agent

This project demonstrates the core container contract for Nasiko agents:

1. Provide a valid `AgentCard.json` with identity, version, skills, transport, and capabilities.
2. Provide a Dockerfile that creates a self-contained `linux/amd64` image.
3. Bind the service to `0.0.0.0` and use the deployment port.
4. Serve the standard Agent Card endpoint and an A2A JSON-RPC handler.
5. Return generated files as A2A artifacts with the correct MIME type.
6. Read credentials from environment variables rather than baking them into the image.
7. Treat `.nasiko/agent.json` as local deployment state rather than source.
8. Initialize telemetry before importing instrumented server libraries.

## Troubleshooting

### The container exits with `OPENAI_API_KEY environment variable not set`

The agent checks for `OPENAI_API_KEY` before starting. Add it to the local `.env`, or configure
it as a Nasiko secret and restart the deployed agent.

### Gemini returns `404 ... model is not found`

`src/agent.py` currently references the retired experimental model
`gemini-2.0-flash-exp`. Replace it with an image-capable model available to your Google project,
then rebuild and redeploy. Model availability changes over time and can be checked with the
Google GenAI SDK.

### Gemini returns `429 RESOURCE_EXHAUSTED`

The key was accepted, but its project has no available quota for the selected image model.
Enable billing or image-generation quota in the Google project, then retry.

### Nasiko reports `-32601 Method not found`

This is an A2A protocol compatibility error, not an API-key error. The Dockerfile currently pins
`a2a-sdk==0.3.26`, whose server accepts legacy method names such as `message/send`. A control
plane sending newer method names such as `SendMessage` will fail before CrewAI or Gemini runs.
Align the agent SDK and server integration with the control plane, or add a control-plane
compatibility fallback.

### Deployment exceeds the Docker-load budget

Install `crewai`, not the much larger `crewai[tools]` extra, unless those additional tools are
actually required. The current Dockerfile intentionally uses the smaller base package. Rebuild
after changing dependencies so Docker does not reuse the old dependency layer.

### The Agent Card advertises the wrong URL

Nasiko rewrites routing for deployed agents. For direct container use, set `HOST_OVERRIDE` to
the externally reachable base URL if the generated runtime card should not advertise the
process's host and port.

## Further reading

- [Agent Development Lifecycle (ADLC)](../../docs/AGENT_LIFECYCLE.md)
- [Nasiko project README](../../README.md)
- [A2A agents and frameworks](https://docs.nasiko.com/adlc/a2a-agents)
- [Running and chatting locally](https://docs.nasiko.com/adlc/build-run-test)
- [Deploying and managing agents](https://docs.nasiko.com/adlc/deploy)
- [Versions and lifecycle](https://docs.nasiko.com/adlc/versions-lifecycle)
- [CrewAI documentation](https://docs.crewai.com/introduction)
- [Google Gemini API](https://ai.google.dev/gemini-api)
