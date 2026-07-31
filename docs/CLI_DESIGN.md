# Nasiko CLI Design

## Overview

`nasiko` is the agent-developer CLI: build, test, deploy, and monitor agents against a Nasiko
control plane. It is a synchronous Rust binary (ureq for HTTP, no tokio) so it compiles fast and
starts instantly.

State on disk:

- `~/.nasiko/config.json` — registered clusters, active cluster, auth tokens, artifact registry URL
- `~/.nasiko/.env` — local-stack configuration written/edited by `nasiko up`
  (DockerHub org, `OPENAI_API_KEY`, model overrides, GitHub OAuth, `SEED_AGENTS`)
- `~/.nasiko/bin/` — cached control-plane binary extracted by `nasiko up`

A separate platform-admin CLI for infrastructure provisioning and organization management ships
with the Nasiko enterprise edition.

## Design Principles

- **Flat commands** — no nested subgroups for the core workflow (`nasiko build`, not
  `nasiko agent build`)
- **Workflow-ordered help** — Setup → Create → Test → Operate
- **Docker-like UX** — `ps`, `logs`, `rm`, `stop`, `start`, `restart`, `scale`
- **Active cluster** — remote commands operate against the cluster selected with `nasiko use <name>`
- **Local-first** — `nasiko up` gives a full platform in seconds

## Personas

| Persona | Interface | Workflow |
|---------|-----------|---------|
| **Agent Developer** | `nasiko` | new → run → chat → deploy → logs |
| **Platform Contributor** | `nasiko` + source | local infra → `cargo run` → test |
| **End User** | UI only | Interacts with agents through the web interface |

## Command Reference

The command surface is defined in `cli/src/main.rs` (top-level groups + grouped help text) and
`cli/src/lib.rs` (clap subcommand enums + dispatch).

```
Setup:
  up         Start local Nasiko cluster (pulls the CP image from DockerHub)
  down       Stop local cluster
  connect    Register a CP by URL
  use        Switch active cluster
  clusters   List configured control planes
  status     Control plane health + metrics
  auth       Authentication (login / status / logout / whoami)

Create:
  new        Scaffold a new agent project (templates embedded via include_dir!)
  skill      Manage agent skills — add / remove / list / search / info
  card       Generate or update AgentCard.json
  validate   Validate agent directory structure

Test:
  build      Build agent Docker image
  run        Build + run agent locally
  chat       Send a message via A2A protocol (--tui for full-screen, --agent for a
             specific deployed agent, --session-id / --resume for sessions)
  sessions          List chat sessions
  create-session    Create a new session on the active cluster
  history           Show message history for a session
  delete-session    Delete a session

Operate:
  push       Build + push image to the cluster OCI registry (no deploy)
  deploy     Build + push + deploy to active cluster
  upload     Upload source zip/dir; the server builds + deploys (no local Docker needed)
  ps         List running agents
  logs       Stream agent container logs (-f live-tails via SSE)
  stop       Stop agent container
  start      Start a stopped agent
  restart    Restart agent container (picks up new secrets/env)
  scale      Scale agent container to N replicas
  rm         Terminate + deregister agent
  secrets    Manage encrypted secrets — set / get / ls / rm (vault-wide, or --agent)
  observe    Observability — sessions / session / trace-detail / span /
             project-stats / finops-dashboard / insights
  maf        Multi-agent flow workflows — workflow (list/create/get/update/delete/run/
             executions), execution (list/get/result)

Agents (registry + lifecycle):
  agents ls              List all deployed agents
  agents get             Get details for a specific agent (--agent-id or --name)
  agents deploy          Deploy an agent from a .zip or directory
  agents search          Search the public Nasiko Artifact Registry
  agents info            Get artifact details from the registry
  agents frameworks      List available frameworks
  agents list-uploaded   List agents uploaded by the current user
  agents chat            Chat directly with a locally running agent

GitHub:
  github status      Show GitHub connection status
  github repos       List accessible GitHub repositories
  github connect     Connect GitHub account via OAuth
  github disconnect  Disconnect GitHub
  github clone       Clone a GitHub repo and deploy as an agent

Registry:
  registry connect      Connect to an artifact registry
  registry disconnect   Disconnect from the artifact registry
  registry status       Show connected registry
  registry search       Semantic discovery by natural-language query (alias: discover)
  registry list         List all artifacts in the registry

MCP Gateway:
  mcp catalog        Browse connectable services (Composio toolkits + custom MCP servers)
  mcp connect        Connect — by --connector-id, --toolkit name, or auto-register a --url
  mcp connections    List your own connections
  mcp disconnect     Disconnect from a connector
  mcp toolkit        Composio toolkit auth-configs (admin) — list / register / update / delete
  mcp connector      Custom MCP server connectors — list / probe / register / update / delete /
                     share (list / add / remove — per-user or --public)
  mcp credential     Per-connector stored credential — set / status / delete
  mcp oauth          OAuth 2.1 authorization — authorize / status / revoke
  mcp agent-tools    Per-agent connector access + tool permissions — connectors / enable /
                     disable / tools / rules / set-rule (allow|ask|block glob) / reset
```

### Permission boundary

All `nasiko` commands use deployer-level or public APIs. An agent developer can deploy and manage
their own agents but cannot manage users, teams, or platform settings — those are admin surfaces.

## How `up` Works

`nasiko up` (`cli/src/commands/dev.rs`):

1. Ensures `~/.nasiko/.env` exists, prints the current values, and offers interactive editing
   (shell env vars take precedence over the file).
2. Starts infra via docker compose — Postgres (`:5432`), Redis (`:6379`), RustFS S3 (`:9000`).
   The compose file is embedded in the binary and written out on demand.
3. Pulls the CP Docker image (`nasiko/cp:latest`; override the org with `DOCKERHUB_USER`),
   extracts the static binary from the image, and caches it in `~/.nasiko/bin/`.
4. Runs the CP binary directly on `:8080` (native speed, no container overhead) and waits for
   `/health`.
5. Auto-connects the CLI to `http://localhost:8080` as cluster `local`.

`nasiko down` stops the CP process and the compose stack.

Platform contributors run the control plane from source instead: start the infra compose file,
then `cargo run -p nasiko-server`.

## How `deploy` / `push` / `upload` Work

`nasiko deploy .` from an agent directory:

1. Reads `AgentCard.json` for name + version
2. Builds the Docker image (auto-build step)
3. Pushes image layers + manifest to the CP's embedded OCI registry (`/v2/*`)
4. Registers or updates the agent and starts/restarts the container
5. Writes `.nasiko/agent.json` in the project (agent ID binding)

`nasiko deploy my-agent:v1` (an image reference) skips the build step. `nasiko push` does steps
1–3 only. `nasiko upload` needs no local Docker at all: it zips the source directory (or takes a
pre-made `.zip`), POSTs it multipart to `/api/agents/upload`, and the server's build worker builds
and deploys it (`202 Accepted` + a build ID you can watch with `nasiko logs` / the Builds UI).

## How `chat` Works

`nasiko chat` is an A2A protocol client. The positional target may be a full URL, an agent
name/UUID (resolved via the CP registry), or `orchestrator`; with no target it talks to the active
cluster's routing engine. A lone positional containing whitespace is treated as the message, not
the target.

```
nasiko chat "hello"                                        # orchestrator on the active cluster
nasiko chat my-agent "hi"                                  # resolves name → /api/agents/<uuid>
nasiko chat --agent my-agent "hi"                          # same, explicit flag
nasiko chat http://localhost:8000 "hello"                  # direct to a local agent
nasiko chat http://my-cp.com/api/orchestrator/a2a "route"  # explicit orchestrator URL
nasiko chat --tui                                          # full-screen TUI (ratatui)
```

## Architecture

| Entity | What it is |
|--------|-----------|
| **Control Plane (CP)** | The server binary — sole ingress: agent registry, routing engine, A2A proxy, embedded OCI registry, observability, UI |
| **CP OCI Registry** | `/v2/*` endpoint on the CP — stores deployed agent Docker images (S3-backed) |
| **Artifact Registry** | Separate service — agent templates and skills (for `nasiko new`, `nasiko skill search`, `nasiko registry ...`) |
| **CLI** | Sync Rust binary — talks to the CP REST API + artifact registry |

## Implementation Notes

- **Sync** — no tokio, no async. `ureq` for HTTP, `clap` for args, `dialoguer` for prompts,
  `ratatui` for the TUI
- **Flat commands** via `#[command(flatten)]` on clap enum variants
- **Manual `override_help`** in `cli/src/main.rs` for grouped help text (clap 4 doesn't support
  grouped subcommand headings natively)
- Embedded agent templates via `include_dir!`; embedded infra compose file via `include_str!`
- Artifact registry URL: `NASIKO_REGISTRY_URL` env var overrides the `registry_url` field in
  `~/.nasiko/config.json`

## Code Structure

Top-level (`cli/src/`):

| File | Owns |
|------|------|
| `main.rs` | CLI struct, Setup command group, grouped help text, dispatch |
| `lib.rs` | All other clap subcommand enums (`AgentDevCommands`, `AgentOpsCommands`, `RegistrySubCommands`, `McpSubCommands`) + dispatch functions |
| `api.rs` | Authenticated HTTP client for the CP REST API |
| `config.rs` | `~/.nasiko/config.json` — clusters, active cluster, tokens, registry URL |
| `oci.rs` | OCI image push (layers + manifest to `/v2/*`) |
| `skill.rs` | Skill manifest types shared by scaffold + skill commands |
| `util.rs` | Shared helpers (container binary detection, formatting) |

Commands (`cli/src/commands/`):

| File | Owns |
|------|------|
| `agents.rs` | Agent operations — registry (`agents ls/get/search/info/frameworks/list-uploaded/deploy`), lifecycle (`ps`, `logs`, `stop`, `start`, `restart`, `scale`, `rm`), and chat-target resolution |
| `auth.rs` | `auth login/status/logout/whoami` |
| `build.rs` | `build` — Docker image builds |
| `card.rs` | `card` — AgentCard.json generation/update |
| `chat.rs` | A2A messaging — `chat` (orchestrator/agent), `agents chat` (direct URL) |
| `cluster.rs` | `connect`, `use`, `clusters` |
| `deploy.rs` | `deploy` — build + push + deploy pipeline |
| `dev.rs` | `up` / `down` local stack + `run` (local agent run) |
| `github.rs` | GitHub integration |
| `maf.rs` | `maf` — multi-agent flow workflow CRUD + run (`maf workflow ...`), execution inspection (`maf execution ...`) |
| `mcp.rs` | MCP Gateway — catalog, connect, connectors, toolkits, credentials, oauth, agent-tools |
| `observe.rs` | `observe` — sessions, trace/span detail, project stats, FinOps dashboard, insights |
| `push.rs` | `push` — build + push without deploy |
| `registry.rs` | `registry` — artifact registry connect/browse/search |
| `scaffold.rs` | `new` — agent project scaffolding from embedded templates |
| `secrets.rs` | `secrets` — encrypted agent/vault secrets |
| `skill.rs` | `skill` — add/remove/list/search/info |
| `status.rs` | `status` — cluster health + metrics |
| `upload.rs` | `upload` — zip + multipart upload for server-side builds |
| `validate.rs` | `validate` — agent directory checks |
| `tui/` | Full-screen TUI chat (ratatui) + session commands (`sessions`, `create-session`, `history`, `delete-session`) |
