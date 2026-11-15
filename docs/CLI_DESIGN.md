# Nasiko CLI Design

## Overview

`nasiko` is the agent-developer CLI: build, test, deploy, and monitor agents against a Nasiko
control plane. It is a synchronous Rust binary (ureq for HTTP, no tokio) so it compiles fast and
starts instantly.

State on disk:

- `~/.nasiko/config.json` — registered clusters, active cluster, auth tokens, artifact registry URL
- `~/.nasiko/sessions.json` — the current conversation per agent, so `nasiko chat` continues it
- `~/.nasiko/integrations/` — local coding-agent state (connections, the session-report queue)
- `.nasiko/agent.json` in an agent project — the deployed agent's ID binding

`nasiko up` keeps no state of its own: it runs the Docker Compose stack of the repository clone it
is run in, configured by that clone's `.env`.

A separate platform-admin CLI for infrastructure provisioning and organization management ships
with other distributions of Nasiko.

## Design Principles

- **Flat commands** — no nested subgroups for the core workflow (`nasiko build`, not
  `nasiko agent build`)
- **Workflow-ordered help** — Setup → Create → Test → Operate
- **Docker-like UX** — `ps`, `logs`, `rm`, `stop`, `start`, `restart`, `scale`
- **Active cluster** — remote commands operate against the cluster selected with `nasiko use <name>`
- **Local-first** — `nasiko up`, run in a clone of the repository, starts the full platform with
  Docker Compose

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
  up         Start Nasiko locally from a clone of the repository (Docker Compose)
  down       Stop the local stack (data is kept)
  connect    Register a CP by URL (`connect <url> [--name N]`, `connect <name> --url <url>`,
             --login=false to only register), or connect local coding agents
             (`connect [claude|codex|opencode|cursor]`: LLM routing through the Nasiko LLM
             router, MCP connectors through the gateway, session reporting — flags --agent,
             --config, --no-routing, --mcp/--no-mcp, --reporting/--no-reporting,
             --content/--no-content, --force, -y)
  disconnect Disconnect local coding agents (--routing, --reporting, --force, -y)
  use        Switch active cluster
  clusters   List configured control planes
  status     Cluster health + the local coding agents (exits 1 if unhealthy);
             `status <agent>` for one coding agent in detail; -j for the cluster as JSON
  auth       login (SSO in the browser when the CP has it, else username/password;
             --sso / --password to choose) / status / logout / whoami
  doctor     Check Docker, cluster reachability, login, and registry setup, with fixes
  completions  Print a shell tab-completion script (bash/zsh/fish/...)
  man        Print the man page (troff format)

Create:
  new        Scaffold a new agent project (templates embedded via include_dir!)
  skill      Manage agent skills — add / remove / list / search / info
  card       Generate or update AgentCard.json
  validate   Validate agent directory structure

Test:
  claude     Run Claude Code through the Nasiko LLM router (--agent, --config)
  build      Build agent Docker image
  run        Build + run agent locally
  chat       Send a message via A2A protocol. Continues the current conversation for
             the agent by default; --new starts a fresh one, --session-id switches to
             an existing one. --agent for a deployed agent, --tui for full-screen,
             --resume (TUI), --coding-agent (assert the target is an ACP coding agent),
             --session <id> to chat in a harness session
  sessions          List chat sessions
  create-session    Create a new session on the active cluster
  history           Show message history for a session
  delete-session    Delete a session
  harnesses         List the coding-agent harnesses a session can run on
  session           Harness sessions: new / show / ls / suspend / rm (see HARNESS_SESSIONS.md)
  context-strategy  get / set how your chat history is selected (pacms / topk / lastk)
  budget            get / set how much chat history a request carries (low / medium / high)

Operate:
  push       Build + push image to the cluster OCI registry (no deploy)
  deploy     Build + push + deploy to active cluster (--dry-run shows what would deploy)
  upload     Upload source zip/dir; the server builds + deploys (no local Docker needed)
             (push / deploy / upload: --coding-agent registers an ACP coding agent,
             human approval on by default; --no-hitl runs it unattended)
  import     Deploy from an OCI/artifact-registry reference (the server pulls it)
  ps         List running agents (-j, -w to refresh)
  logs       Agent container logs (-f live-tails via SSE; --level, --search, --since)
  stop       Stop agent container
  start      Start a stopped agent
  restart    Restart agent container (picks up new secrets/env)
  scale      Scale agent container to N replicas
  rm         Terminate + deregister agent
  deployments  Deployment-level ops — ls / get / restart
  reupload   Re-upload source and rebuild an existing deployed agent
  versions   List version history for a deployed agent
  rollback   Roll back a deployed agent to a previous version (or -t a given one)
  secrets    Manage encrypted secrets — set / get / ls / rm (vault-wide, or --agent)
  llm-config Reusable LLM configs — create / update / delete / list / set-default /
             attach / detach / get / providers
  model-registry  Platform tier→model mappings the smart router uses — ls / set
  observe    Observability — sessions / session / turns / trace-detail / span /
             project-stats / finops-dashboard / insights
  maf        Multi-agent flow workflows — workflow (list / create / get / update / delete /
             drafts / promote / run / executions), execution (list / get / result / stop),
             trace (split one instruction into steps, run it, report each stage)

Agents (registry + lifecycle + local coding agents):
  agents ls              List all deployed agents
  agents get             Get details for a specific agent (--agent-id or --name)
  agents deploy          Deploy an agent from a .zip or directory
  agents search          Search the public Nasiko Artifact Registry
  agents info            Get artifact details from the registry
  agents frameworks      List available frameworks
  agents list-uploaded   List agents uploaded by the current user
  agents chat            Chat directly with a locally running agent
  agents setup           Connect every coding agent on this machine and import its past
                         sessions (--no-import, --wait)
  agents import          Import the past sessions of the coding agents already connected
  agents status          Show how the import is going and what is still waiting to send
  agents sync            Deliver queued coding-agent events (--retry-rejected)

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
                     share (list / add / remove — per-user or --public) /
                     upload (a .zip) / upload-github / build-status / logs (uploaded servers) /
                     consumers / pin / unpin / pinned / recent / grant-agent / revoke-agent
  mcp credential     Per-connector stored credential — set / status / delete
  mcp oauth          OAuth 2.1 authorization — authorize / status / revoke
  mcp agent-tools    Per-agent connector access + tool permissions — connectors / enable /
                     disable / tools / rules / set-rule (allow|ask|block glob) / reset
  mcp share-targets  Search users to share a connector with
```

### Permission boundary

All `nasiko` commands use deployer-level or public APIs. An agent developer can deploy and manage
their own agents but cannot manage users, teams, or platform settings — those are admin surfaces.

## How `up` Works

`nasiko up` (`cli/src/commands/dev.rs`) runs the README's Docker Compose quick start for you, so it
must be run inside a clone of the repository
(`git clone https://github.com/Nasiko-Labs/nasiko.git && cd nasiko`):

1. Finds the checkout: the nearest directory, the current one or above, holding
   `docker-compose.yml` and `server/Dockerfile` (or that pair under `oss/`). Outside a clone it
   fails and prints the clone command.
2. Creates `.env` from `.env.example` there if there is none. Its defaults work on the local
   machine; change them before anyone else can reach the server.
3. Runs `docker compose up -d --build` in the checkout (Podman's `compose` when that is the
   container tool found), which builds and starts the server and its infrastructure. The first
   build takes a few minutes.
4. Waits up to 120 s for `http://localhost:8080/health` (the server runs its migrations first); on
   a timeout it warns and points at `docker compose logs -f server`.
5. Registers `http://localhost:8080` as cluster `local` and tells you to `nasiko auth login`
   (`admin` / `changeme`, or the `ADMIN_PASSWORD` in `.env`).

`nasiko down` runs `docker compose down` in the same checkout. Data is kept in the Compose volumes
(`docker compose down -v` there deletes it).

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
the target. Each agent has a current conversation, remembered in `~/.nasiko/sessions.json`: chat
continues it by default, `--new` starts a fresh one and `--session-id <id>` switches to an existing
one (either becomes the new current conversation).

```
nasiko chat "hello"                                        # orchestrator on the active cluster
nasiko chat my-agent "hi"                                  # resolves name → /api/agents/<uuid>
nasiko chat --agent my-agent "hi"                          # same, explicit flag
nasiko chat http://localhost:8000 "hello"                  # direct to a local agent
nasiko chat http://my-cp.com/api/orchestrator/a2a "route"  # explicit orchestrator URL
nasiko chat my-agent --new "start over"                    # fresh conversation
nasiko chat --session <id> "next step"                     # a harness session's turn
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
- Embedded agent templates and skills via `include_dir!`
- Artifact registry URL: `NASIKO_REGISTRY_URL` env var overrides the `registry_url` field in
  `~/.nasiko/config.json`

## Code Structure

Top-level (`cli/src/`):

| File | Owns |
|------|------|
| `main.rs` | CLI struct, Setup command group (`up`, `down`, `connect`, `disconnect`, `use`, `clusters`, `status`, `auth`, `budget`, `context-strategy`, `doctor`, `completions`, `man`), grouped help text, dispatch |
| `lib.rs` | All other clap subcommand enums (`AgentDevCommands`, `AgentOpsCommands`, `RegistrySubCommands`, `McpSubCommands`, ...) + dispatch functions |
| `api.rs` | Authenticated HTTP clients for the CP REST API, the CP OCI registry and the artifact registry, plus error-body helpers |
| `config.rs` | `~/.nasiko/config.json` — clusters, active cluster, tokens, registry URL |
| `hitl.rs` | Shared inline HITL prompt/resolve logic for every surface that can hit a pause (`chat`, `maf ... --wait`) |
| `oci.rs` | OCI image push: `docker save` tar → blobs + manifest to `/v2/*` |
| `session_store.rs` | The current conversation per agent (`~/.nasiko/sessions.json`), so `chat` continues it |
| `skill.rs` | Skill manifest types + embedded skills, shared by scaffold + skill commands |
| `util.rs` | Shared helpers (container binary detection, image-name parsing, version detection, archive extraction) |
| `version_prompt.rs` | Resolves the version a deploy publishes (flags, AgentCard.json, or a prompt) |

Commands (`cli/src/commands/`):

| File | Owns |
|------|------|
| `agents.rs` | Agent operations — registry (`agents ls/get/search/info/frameworks/list-uploaded/deploy`), lifecycle (`ps`, `logs`, `stop`, `start`, `restart`, `scale`, `rm`, `reupload`, `versions`, `rollback`), and chat-target resolution |
| `auth.rs` | `auth login/status/logout/whoami` |
| `build.rs` | `build` — Docker image builds |
| `card.rs` | `card` — AgentCard.json generation/update |
| `chat.rs` | A2A messaging — `chat` (orchestrator/agent/harness session), `agents chat` (direct URL) |
| `claude.rs` | Connect Claude Code to the Nasiko LLM router (`claude`, `connect claude`) and issue on-demand credentials |
| `cluster.rs` | `connect` (control plane), `use`, `clusters` |
| `codex.rs` | Connect Codex CLI to the Nasiko Responses router without replacing Codex auth |
| `coding_agent_router.rs` | Shared control-plane and state primitives for local coding-agent LLM routing |
| `context_selection.rs` | `budget` and `context-strategy` — per-user chat-history settings |
| `deploy.rs` | `deploy` — build + push + deploy pipeline |
| `deployments.rs` | `deployments ls/get/restart` |
| `dev.rs` | `up` / `down` (the clone's Compose stack) + `run` (local agent run) |
| `doctor.rs` | `doctor` — read-only preflight check-up with the fix for each problem |
| `github.rs` | GitHub integration |
| `harnesses.rs` | `harnesses` — the coding-agent harnesses a session can run on |
| `import.rs` | `import` — deploy from an OCI/artifact-registry reference |
| `integration/` | Detect local coding agents and report their sessions to Nasiko (hooks, queue, delivery, past-session import behind `agents setup/import/status/sync`) |
| `llm_config.rs` | `llm-config` — per-user library of reusable LLM routing configs, attached to agents |
| `local_agents.rs` | One surface for local coding agents (Claude Code, Codex, OpenCode, Cursor): `connect`, `disconnect`, `status` |
| `maf.rs` | `maf` — multi-agent flow workflow CRUD + run, drafts, execution inspection, `trace` |
| `mcp.rs` | MCP Gateway — catalog, connect, connectors (incl. upload), toolkits, credentials, oauth, agent-tools |
| `model_registry.rs` | `model-registry` — the platform's tier→model mappings |
| `observe.rs` | `observe` — sessions, turns, trace/span detail, project stats, FinOps dashboard, insights |
| `opencode.rs` | Connect OpenCode to the Nasiko LLM router with an isolated runtime plugin |
| `push.rs` | `push` — build + push without deploy |
| `registry.rs` | `registry` — artifact registry connect/browse/search |
| `scaffold.rs` | `new` — agent project scaffolding from embedded templates |
| `secrets.rs` | `secrets` — encrypted agent/vault secrets |
| `session.rs` | `session new/show/ls/suspend/rm` — harness sessions |
| `skill.rs` | `skill` — add/remove/list/search/info |
| `sso.rs` | Browser sign-in through the control plane's SSO (device grant) for `auth login` |
| `status.rs` | `status` — cluster health + metrics |
| `upload.rs` | `upload` — zip + multipart upload for server-side builds |
| `validate.rs` | `validate` — agent directory checks |
| `tui/` | Full-screen TUI chat (ratatui) + session commands (`sessions`, `create-session`, `history`, `delete-session`) |
| `tests/` | Unit tests for `deploy` and `push` |
