# Nasiko Coding Agent

The Nasiko Coding Agent is an A2A agent that reads, writes, tests, and refactors code in a
contained workspace. It runs a ReAct loop against an OpenAI-compatible model, using specialized
tools to inspect a project, make targeted changes, execute commands, and iterate on test
failures.

Example requests:

```text
Create a Rust library with an add function and unit tests.
Find every use of the deprecated function and replace it safely.
Run the test suite, diagnose the failures, and fix them.
Refactor this module into smaller files without changing behavior.
```

## Capabilities

The model can use these workspace tools:

- `read_file` reads a whole file or an inclusive line range.
- `list_directory` lists a directory tree while skipping `.git`, `target`, and `node_modules`.
- `search_code` searches with ripgrep and falls back to grep.
- `write_file` creates or replaces a file.
- `edit_file` applies exact, unique search-and-replace edits.
- `run_command` executes a shell command with a timeout.
- `run_tests` detects Rust, Node.js, Python, or Go projects and selects a conventional test
  command.

The agent streams working-status updates while it uses tools and returns a text artifact when
the task completes. A request can use at most 12 ReAct iterations.

## Workspace and security model

All file paths are resolved relative to `WORKSPACE_DIR`. Absolute paths and `..` traversal
outside that root are rejected. Commands run through `sh -c` with a 120-second default timeout,
and captured output is limited to 64 KiB.

This path containment is not a complete security boundary. In the current implementation,
commands execute in the same container as the agent. Only run code that is appropriate for that
container's trust level.

The current image provides a shell, Git, ripgrep, and the Rust toolchain. Although the agent can
detect Node.js, Python, and Go projects, their test commands require corresponding runtime images
that are not included yet.

`SANDBOX_MODE=remote` is reserved for a future Nasiko-managed, per-request sandbox backend and
currently fails with a “not implemented” error. Use `SANDBOX_MODE=local`, which means local to
the agent container—not the developer's host.

## Project layout

```text
coding/
├── AgentCard.json          # Nasiko identity, capabilities, skills, and release version
├── Cargo.toml              # Standalone Rust crate and dependencies
├── Dockerfile              # Builder and Rust development runtime
├── justfile                # Legacy image build recipes; not required by the Nasiko ADLC
└── src/
    ├── main.rs             # A2A server, Agent Card, and ReAct loop
    ├── project.rs          # Project-language and test-command detection
    ├── tools.rs            # Coding tool definitions and dispatch
    └── sandbox/
        ├── mod.rs          # Sandbox abstraction and backend selection
        └── local.rs        # Contained file access and command execution
```

The crate is intentionally standalone rather than a member of the repository's root Cargo
workspace. The Nasiko CLI can therefore build it directly from this directory.

## Configuration

The agent reads these environment variables:

- `OPENAI_API_KEY` is required for the ReAct loop.
- `OPENAI_BASE_URL` defaults to `https://api.openai.com/v1` and may point to any compatible
  `/chat/completions` endpoint or the Nasiko LLM gateway.
- `OPENAI_MODEL` defaults to `gpt-4o-mini`.
- `WORKSPACE_DIR` defaults to the process directory; the image sets it to `/workspace`.
- `SANDBOX_MODE` defaults to `local`. The `remote` backend is not implemented.
- `PORT` defaults to `8000`.
- `RUST_LOG` controls runtime log filtering.

No `.env` file is required when the agent is deployed to Nasiko. Nasiko injects configured
secrets and platform settings when the container starts.

For local testing with `nasiko run`, create an untracked `.env`:

```dotenv
OPENAI_API_KEY=replace-me
OPENAI_BASE_URL=https://api.openai.com/v1
OPENAI_MODEL=gpt-4o-mini
SANDBOX_MODE=local
```

Do not commit real credentials.

## Prerequisites

- The `nasiko` CLI
- Docker or Podman running as the container runtime used internally by the CLI
- An OpenAI API key or access to a compatible model endpoint

No host Rust toolchain, `just`, or manual container commands are required. The Nasiko CLI builds
the Rust binary inside the Dockerfile's builder stage.

## Agent Development Lifecycle (ADLC)

The Nasiko Agent Development Lifecycle (ADLC) covers validation, local testing, deployment,
operation, and iteration. This agent is already scaffolded, so its ADLC starts with validation
rather than `nasiko new`.

### 1. Validate the existing agent

From this directory:

```sh
cd agents/coding
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
  "Create a small Rust library in the workspace and add passing unit tests."
nasiko chat http://localhost:8000 --tui
```

The local agent starts with an empty `/workspace`. `nasiko run` does not mount the host project,
and its workspace is removed when the development container is replaced. Local testing should
therefore use disposable sample files created by the agent.

For a build-only check:

```sh
nasiko build . --tag coding:0.1.0
```

`nasiko build` creates the image without starting it. Both commands manage the container runtime
for you.

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
nasiko secrets set OPENAI_API_KEY replace-me --agent coding
nasiko secrets ls --agent coding
```

Secret precedence is: inline `nasiko deploy -e` values, agent-specific secrets, then vault
secrets. Updating a secret does not change a running container; recreate it with:

```sh
nasiko restart coding
```

Remove a vault secret with `nasiko secrets rm OPENAI_API_KEY`. Add `--agent coding` when removing
the agent-specific override.

#### Build and deploy

Deploy with persistent Nasiko writable storage mounted at the coding workspace:

```sh
nasiko deploy . \
  --name coding \
  --port 8000 \
  --writable \
  --writable-path /workspace
```

`nasiko deploy`:

1. Builds the Rust agent as a `linux/amd64` image.
2. Pushes it to Nasiko's embedded OCI registry.
3. Registers or updates the agent.
4. Starts the container with its secrets and persistent workspace.
5. Writes `.nasiko/agent.json` to bind this directory to the deployment.

Keep `.nasiko/` out of source control. The writable workspace survives restarts, redeployments,
and code updates. The initial workspace is empty; the agent can create a project or clone one
with `run_command` when credentials and network policy permit it.

Do not use `nasiko upload` for this Rust-only agent yet. The current server-side source validator
requires a Python entrypoint and rejects the project before building its Dockerfile.
`nasiko deploy` is the supported path because it builds the image locally and uploads it to
Nasiko.

### 4. Operate the deployed agent

Inspect the deployment and manage its lifecycle:

```sh
nasiko ps
nasiko ps --json
nasiko logs coding
nasiko logs coding -n 200
nasiko logs coding -n 200 -f
nasiko stop coding
nasiko start coding
nasiko restart coding
nasiko scale coding 2
nasiko status
```

Chat with the deployed agent:

```sh
nasiko chat coding \
  "Create a Rust library with an add function, write tests, and run them."
```

List and resume control-plane sessions:

```sh
nasiko sessions
nasiko chat --agent coding --resume <session-id>
nasiko chat --agent coding \
  --session-id <session-id> \
  "Continue by adding subtraction support."
```

Control-plane session IDs begin with `ses_` and must be resumed through the deployed agent, not
the standalone `http://localhost:8000` development agent. The `--resume` option opens the TUI.

Open Nasiko observability after the agent has handled traffic:

```sh
nasiko observe sessions
nasiko observe session <session-id>
```

The Nasiko control plane records session and routing activity. This agent does not currently
initialize an OpenTelemetry exporter for internal LLM and tool spans.

Remove the agent when it is no longer needed:

```sh
nasiko rm --name coding
nasiko rm --name coding -f
```

`stop` preserves the registration and workspace. `rm` deregisters the agent; confirm the
workspace retention policy before forced removal if it contains work you need.

### 5. Version and iterate

The version in `AgentCard.json` becomes the deployed image tag. For each release:

1. Change the source.
2. Bump `AgentCard.json`.
3. Keep `Cargo.toml` and the runtime Agent Card in `src/main.rs` aligned.
4. Validate and test locally.
5. Redeploy the same agent name.

The project currently declares `0.1.0` in `AgentCard.json` and `Cargo.toml`, while the runtime
card in `src/main.rs` reports `1.0.0`. Align these before the next release so clients and Nasiko
show the same version.

```sh
nasiko validate .
nasiko run . --port 8000
nasiko chat http://localhost:8000 "Create a file and verify its contents."
nasiko deploy . \
  --name coding \
  --port 8000 \
  --writable \
  --writable-path /workspace
```

Redeploying `coding` updates the agent in place and preserves writable storage. `nasiko restart`
recreates the existing container to pick up changed secrets or environment variables; it does
not rebuild the image.

Roll back to the previous deployment or a specific version:

```sh
nasiko rollback --name coding
nasiko rollback --name coding --version 0.1.0
```

## Building a compatible Nasiko coding agent

This project demonstrates the core contract for a coding agent on Nasiko:

1. Provide a valid, registry-safe `AgentCard.json`.
2. Build a self-contained `linux/amd64` image with the required development tools.
3. Bind to `0.0.0.0` and honor `PORT`.
4. Serve an A2A JSON-RPC endpoint and the standard Agent Card endpoint.
5. Stream valid A2A task status and artifact events.
6. Keep file and command access within an explicit workspace root.
7. Read model credentials and endpoint configuration from environment variables.
8. Use Nasiko writable storage for state that must survive container recreation.
9. Treat `.nasiko/agent.json` as local deployment state rather than source.
10. Keep versions consistent across package, runtime, and Agent Card metadata.

## Troubleshooting

### The agent cannot see the host project

This is expected with `nasiko run`. The CLI starts the image with its own `/workspace`; it does
not mount the directory containing the source. Test with disposable files created inside the
agent workspace, or deploy with Nasiko writable storage.

### `SANDBOX_MODE=remote is not implemented yet`

Set `SANDBOX_MODE=local`. In the current implementation, “local” means the workspace inside the
agent container. Per-request remote sandboxes are planned but not implemented.

### `npm`, `pytest`, or `go` is not found

The current runtime image only includes the Rust toolchain. Build a language-specific runtime
variant before asking the agent to execute Node.js, Python, or Go tests.

### LLM requests fail with 401

Set a valid `OPENAI_API_KEY`. For a deployed agent, update the Nasiko secret and run
`nasiko restart coding`.

### A path is rejected as outside the workspace

Use paths relative to `WORKSPACE_DIR`. Absolute paths and traversal above the workspace root are
intentionally rejected.

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
