# Harness Sessions

How to run coding harnesses (Claude Code, Codex, OpenCode) as per-user sessions on Nasiko, and
what the platform does with them. This guide covers harness agents only; framework agents that
are parked when idle are described in [Parked agents](AGENT_LIFECYCLE.md#parked-agents).

## 1. What a harness session is

A coding harness is a platform-built image that runs a coding agent and speaks ACP (the Agent
Client Protocol) through a small shim, `harness-shim`, which exposes the harness's stdio as a
WebSocket. Each enabled harness is represented by one platform agent, seeded at server startup
while coding sessions are switched on (see [Turning sessions on](#turning-sessions-on)).
A session is one conversation between one user and one harness: it is unique by A2A `contextId`,
which is the session id, and belongs to the user who created it. The session's sandbox exists only
while a turn is running, and is suspended when the session goes idle. The workspace under
`/workspace` persists across suspends, so files the harness wrote are still there on the next turn.
Three harnesses ship in the catalog (`harness_catalog`): `opencode`, `claude-code` and `codex`.

A harness agent belongs to one user and is never shared. Making one public, granting it to a user
or a group, or giving it to another owner is refused with `409` and the code
`agent_not_shareable`, and a grant that exists anyway gives nobody access: someone else's harness
agent is not shown in agent listings, routing or the A2A registry. The platform's catalog agents,
the ones sessions are started from, are public and stay so: making one private is refused the same
way. A user's own harness agent is private, and making it private (again) is always allowed.
`POST /api/agents/{id}/grants/agents` is not a share and is not refused: it lists the agents the
harness agent may itself call.

### Turning sessions on

Coding sessions are off by default. `HARNESS_SESSIONS_ENABLED=true` turns them on; any other value,
or none, leaves them off. The server reads it once at startup, so changing it takes a restart.

- **Off.** Startup creates no harness agents and hides (soft-deletes) the ones an earlier start
  created, so no agent list, chat picker or registry offers them. `GET /api/harnesses` returns an
  empty list, and `POST /api/sessions` is refused with `403` and code `harness_sessions_disabled`.
  A turn sent to a harness is refused the same way, as a JSON-RPC error (`-32610`) carrying that
  body. Sessions opened earlier stay readable, and can be suspended or deleted, but run no turns.
- **On.** Startup restores the hidden harness agents with their old ids, so sessions opened on them
  before still resolve, and creates any that are missing. Seeding also needs `AGENT_JWT_SECRET`
  (section 7).

## 2. Lifecycle

A session's `runtime_state` moves through these states:

| State | Meaning |
|-------|---------|
| `parked` | No turn is running and the platform believes no sandbox is live. A new session starts here. |
| `starting` | A turn holds the session and is acquiring or resuming its sandbox. |
| `running` | A turn is driving the sandbox, or the last turn ended and the sandbox is still up awaiting the idle rule. |
| `stopping` | A suspend has claimed the session and is stopping the sandbox. |

The normal cycle is `parked → starting → running → stopping → parked`.

What survives a suspend depends on the provider. On the Docker provider, suspend is `docker stop`,
not `pause`, so a suspended sandbox holds no RAM: chatting again starts the same container, and
what survives is the files in `/workspace` and the ACP session id, which the next turn uses to
restore the harness's conversation. Running processes and anything held in memory are lost. Every
provider keeps files only: a backgrounded job or subagent does not survive a suspend. Background
processes count toward the session's CPU (so a dev server left running keeps the session from
looking idle) and stop with the session, even one that detached from its shell or daemonized itself:
the shim adopts them, and stops every process left in its tree when it exits. A provider
that runs several sessions in one pod delivers each session's settings after the sandbox is
attached (see section 8).

### Where a session's files live

A session's files live in two places: the durable workspace, and a scratch area on the local disk
of the machine that runs the sandbox.

| What | Where | Kept |
|------|-------|------|
| The repository, `HOME` (the harness's own configuration and conversation state), and everything else in the working tree, a project's own `node_modules`, `.venv` or `vendor` included | The workspace, `/workspace` | Across suspends |
| Package caches and build output | The scratch area, `/scratch/<sandbox key>/` (`s-<uuid>`, one directory per session, so sessions sharing a pod never mix) | While the sandbox's container or pod exists; then gone, and rebuilt by the next install or build |

The workspace can be a network filesystem, where every file operation is a round trip and the I/O
capacity is shared by every sandbox in the cluster. Installs and builds create and read thousands of
small files, so the server points those tools at the scratch area through the sandbox env, the same in
both shim modes:

| Variable | Folder under `/scratch/<sandbox key>/` |
|----------|----------------------------------------|
| `XDG_CACHE_HOME` | `cache`: the harness's own downloads (OpenCode's provider packages), and every tool that follows the XDG spec |
| `npm_config_cache` | `npm` |
| `YARN_CACHE_FOLDER` | `yarn` |
| `PIP_CACHE_DIR` | `pip` |
| `UV_CACHE_DIR` | `uv` |
| `CARGO_TARGET_DIR` | `cargo-target`: a Rust build's output lands here, not in the repository's `target/`, so a built binary is `$CARGO_TARGET_DIR/debug/<bin>`, not `./target/debug/<bin>` |
| `GOCACHE` | `go-build` |
| `GOMODCACHE` | `go-mod` |

`UV_LINK_MODE=copy` is set as well: uv hard-links packages from its cache into a project's `.venv`,
which cannot cross from the scratch area to the workspace.

A variable the session's profile sets keeps the profile's value. An `env`-mode sandbox created before
the scratch area existed keeps the env it was created with, and so its caches in `HOME`, until it is
created again.

How long the scratch area lasts depends on the provider (Docker is the only one this edition ships; the
pod, shared-runtime and harness-agent providers below come with other distributions). On Docker it
is a volume of the sandbox's own: a suspend keeps it, a release removes it, and Docker does not cap its size. A provider that
deletes the sandbox's pod on suspend loses it on every suspend. On a shared runtime pod it lasts while
the pod runs: a suspend keeps it, and a release removes the session's directory. On a user's own
harness agent it lasts while the agent's pod runs, which is until the agent is parked. Providers that run
sandboxes in pods give each session a configured size of scratch (a pod that holds several sessions
gets it once per session they may hold, and they share the total), cap the pod's whole local disk use
at that size, and evict a pod that goes over it.

A turn that arrives while a suspend is in progress waits for it to settle, up to 15 seconds
(`SUSPEND_WAIT`), and then resumes the sandbox. If the suspend is still running after that, the
turn is refused with `409` and the client should retry after a moment.

## 3. Talking to a harness

There are two entry points, both A2A JSON-RPC (see [A2A_PROTOCOL.md](A2A_PROTOCOL.md)):

| Entry point | Notes |
|-------------|-------|
| `POST /api/agents/{id}` | The agent proxy. `{id}` is the harness agent's UUID. |
| `POST /api/orchestrator/a2a` | Dispatch. Set the `agent_id` metadata to the harness agent's name or UUID. |

The `contextId` of a `message/send` or `message/stream` is the session id:

- A message with a `contextId` that does not exist yet creates a session for the caller on that
  harness. A message with no `contextId` also starts a new session.
- A `contextId` that belongs to another user is answered `404`, the same as one that does not
  exist, so session ids cannot be probed.
- A message while the session already has a turn running is `409`. Wait for the turn to end.

The response is an SSE stream. While the harness works, it carries `working` status updates
tagged with `metadata.kind` set to `tool_call`, `thought` or `plan`: a tool call or plan carries a
small data part (identity and status, never the tool's raw input or output), and a thought carries
its text. Reply text arrives in further `working` updates, and the stream ends with a terminal
`completed` update that carries the full reply. Permission prompts for tool use surface as
human-in-the-loop approvals in the same way they do for other coding agents on the platform.
An approval lives only as long as the turn that asked it. While the turn waits on it the turn is
live, which keeps the session from being suspended, for at most `AGENT_CALL_TIMEOUT_SECS`. When the
turn ends (it completes, reaches `AGENT_CALL_TIMEOUT_SECS`, is cancelled, or loses the session), the
approvals it asked that nobody answered are closed. An answer given after that does not resume the
work; send a new message.

Only users drive harness sessions. An agent calling through the proxy cannot start a turn on one
(the request is refused with a JSON-RPC invalid-request error).

### Tasks

A turn is an A2A task: its id is the `taskId` on the stream's events, and its `contextId` is the
session. Three task methods work on both entry points (either spelling: `tasks/get` or `GetTask`,
`tasks/cancel` or `CancelTask`, `tasks/resubscribe` or `SubscribeToTask`; on dispatch, set the
`agent_id` metadata as for a message):

- `tasks/get {id}` returns the Task. Its state is `working` while the turn runs (its lease is
  live), `input-required` while it waits on a permission approval (the status message names the
  approval), and afterwards `completed` (the status message carries the reply), `failed` or
  `canceled`. A turn whose server replica died, so that nothing renews its lease, reads `failed`.
- `tasks/cancel {id}` stops the running turn: the harness is sent `session/cancel`, and the reply
  is the Task in `canceled` (it waits up to 12 seconds for the turn to wind down, and the turn is
  ended as canceled after 10 seconds even if the harness ignores the cancel, and in that case the
  sandbox's harness process is restarted so the next turn starts clean). A permission approval the
  turn was waiting on is closed as canceled. A task that is not
  running is `-32002` (TaskNotCancelable). A cancel reaches a turn running on another server
  replica through the session row, within one lease heartbeat.
- `tasks/resubscribe {id}` opens an SSE stream of the running turn's frames, in the same format as
  `message/stream`. It delivers frames **from the moment it subscribes onward**: what was sent
  before is not replayed. Each subscriber may fall up to 256 frames behind; one that falls further
  skips the frames it missed. It works on any server replica: the replica running the turn also
  publishes every frame to the Redis channel `nasiko:turn:<turn_id>:frames`, and a replica that
  does not run the turn subscribes to it, sends a Task snapshot, then relays the frames as SSE until
  the terminal one. A frame published before the subscriber joined is not replayed (the same rule as on
  the owning replica); the snapshot covers the small window between it and subscribing. The
  session row is still checked, every 5 seconds, so a turn whose replica died ends the stream with
  its recorded state. Redis is optional here: if it cannot be reached the replica answers with the
  snapshot and then each change of state polled from the row once a second (state changes and the
  final reply, not every chunk). A Redis failure never fails a turn or a resubscribe.

Only the running turn and the session's most recent finished turn are known. Any other task id,
and any task of another user's session, is `-32001` (TaskNotFound). JSON-RPC errors are returned
with HTTP 200, as for every other agent.

Poll through the proxy (`POST /api/agents/{id}`) or use `tasks/resubscribe`: the dispatch route
shares the 30-requests-a-minute A2A bucket with messages, so polling there competes with chat. The
proxy answers task methods without opening a flow or spending flow-guard budget.

The turn no longer depends on the `message/stream` connection: if that client disconnects, the
turn keeps running (it can be re-attached with `tasks/resubscribe` or stopped with `tasks/cancel`)
until it finishes or reaches `AGENT_CALL_TIMEOUT_SECS`.

## 4. Session API

Routes are under `/api` and require a signed-in user. Bodies follow
[API_CONVENTIONS.md](API_CONVENTIONS.md): `{"data": ...}` for success, `{"error", "code"}` for
failure.

| Route | Purpose | Success | Errors |
|-------|---------|---------|--------|
| `GET /api/harnesses` | Enabled harnesses | `200` | |
| `POST /api/sessions` | Create a session `{harness, repo?}` | `201` | `401` `unauthorized`, `400` `invalid_repo`, `400` `unknown_harness`, `403` `harness_sessions_disabled`, `503` `harness_not_ready`, `500` |
| `GET /api/sessions` | List your sessions, newest activity first. Query: `harness`, `state`, `limit` (1-200, default 50), `cursor`; a superuser may add `user_id` | `200`, a cursor-paged list of the object below | `400` `invalid_parameter`, `403` `forbidden` (`user_id` of someone else), `500` |
| `GET /api/sessions/{id}` | Read your session | `200` | `404` `session_not_found` |
| `DELETE /api/sessions/{id}` | Release the sandbox, revoke its token, remove the session (owner or superuser) | `204` | `404` `session_not_found`, `409` `session_busy`, `500` |
| `POST /api/sessions/{id}/suspend` | Suspend its sandbox | `204`, or `202` while another suspend is finishing | `404`, `409` `session_busy`, `502` `sandbox_error`, `500` |

Creating a session, and starting each turn on one, first asks the server's session admission
(`SessionAdmission`, `server/src/sessions/admission.rs`), before anything is written for either.
`POST /api/sessions` checks the `repo` first, then the harness, and only then refuses with `403`
`harness_sessions_disabled` while `HARNESS_SESSIONS_ENABLED` is off, whatever the admission says.
The open-source build admits every session; a distribution whose sessions can add an agent (one per
user and harness) installs its own. Its refusal (for instance a `402` at an agent limit) answers
`POST /api/sessions` as it stands, and a turn as a JSON-RPC error under the request's `id`: code
`-32610`, the refusal's `error` as the message and its body as `data`, on a `200` over
`/api/agents/{id}` and with the refusal's status over `/api/orchestrator/a2a`.

The `repo` is cloned (`--depth 1`) into `/workspace` by the shim inside the sandbox on its first
start, so source never passes through the control plane. A user who has connected GitHub clones
private `github.com` repositories with their own token; every other repository is cloned
anonymously, and a failed clone is reported in the first turn and retried the next time the
sandbox starts.

Suspend answers `204` when there is nothing to do (no sandbox, or already parked). It answers
`409` while a turn is using the session; a turn that starts during the suspend wins.

Delete answers `409` while a turn (or a suspend) holds the session; a running turn is never
killed, so suspend first. The sandbox is released on a best-effort basis (a failure is logged
and the idle reaper's orphan pass is the backstop); the row is removed either way.

List the harnesses:

```http
GET /api/harnesses
```

```json
{
  "data": [
    {
      "id": "opencode",
      "name": "OpenCode",
      "image": "opencode:0.1.0",
      "protocol_version": 1,
      "enabled": true
    }
  ],
  "has_more": false,
  "next_cursor": null,
  "prev_cursor": null
}
```

Create a session:

```http
POST /api/sessions
Content-Type: application/json

{ "harness": "opencode", "repo": "https://github.com/acme/app.git" }
```

```json
{
  "data": {
    "session_id": "4f0c6c1e-…",
    "harness": "opencode",
    "agent_id": "b2d1…",
    "runtime_state": "parked",
    "sandbox_ref": null,
    "last_turn_at": null,
    "suspended_at": null,
    "workspace_repo": "https://github.com/acme/app.git"
  }
}
```

`GET /api/sessions/{id}` returns the same object. Someone else's session is
`404 {"error": "session not found", "code": "session_not_found"}`.

Suspend a session, then the answer while another suspend is finishing:

```http
POST /api/sessions/4f0c6c1e-…/suspend
```

```http
HTTP/1.1 204 No Content
```

```json
{ "data": { "status": "suspending" } }
```

(the second body is the `202` answer.) A session a turn is using returns
`409 {"error": "a turn is using this session", "code": "session_busy"}`.

The optional `repo` is stored as the session's `workspace_repo`. It must be a URL that:

- uses `https` or `ssh`;
- is on a host in `GIT_CLONE_ALLOWED_HOSTS` (default `github.com,gitlab.com,bitbucket.org`);
- embeds no credentials (no password, and no username except `git@`-style on `ssh`);
- is at most 2048 characters.

The scp form `git@github.com:org/repo.git` is rejected with the hint
`use ssh://git@host/org/repo.git`.

## 5. CLI

```sh
nasiko harnesses                                  # list harnesses you can start a session on
nasiko session new --harness opencode             # create a session (add --json for the raw object)
nasiko session new --harness opencode --repo https://github.com/acme/app.git
nasiko session show <id>                          # state, sandbox, last turn, suspension, repo
nasiko session ls [--harness ID] [--state S] [--all] [--json]   # your sessions (--all follows every page)
nasiko session suspend <id>                       # stop the sandbox; chatting again resumes it
nasiko session rm <id> [--yes]                    # delete it (asks first); refused while a turn runs
nasiko chat --session <id>                        # chat in that session; the session picks the agent
```

`session rm` answers "a turn is running; `nasiko session suspend <id>` first" when the server
refuses with `409`.

## 6. Idle policy and timeouts

A background reaper suspends a session only when all of these hold:

1. Its last turn ended more than `CODING_AGENT_IDLE_TTL_SECS` ago, with no live turn (a turn whose
   lease has lapsed counts as dead).
2. The sandbox's CPU stayed at or below `SESSION_CPU_QUIESCENT_MILLICORES` for a full
   `SESSION_IDLE_GRACE_SECS` window, measured inside the idle TTL. A busy sample restarts the window.
3. The sandbox made no model or MCP call for the session since the last turn.

A permission approval never holds an idle session up: it lives only as long as its turn (section
3), and a live turn already keeps the session out of the reaper's reach. The one approval that can
outlive its turn is one asked by a turn whose server replica stopped mid-turn; nothing can answer
it, and the session is suspended by the rules above like any other. Every suspend, whether the
reaper's, `POST /api/sessions/{id}/suspend` or a recovery, closes the approvals the session left
open, since the process that asked them is gone.

| Variable | Default | Meaning |
|----------|---------|---------|
| `CODING_AGENT_IDLE_TTL_SECS` | `900` | Idle time before a session is a suspend candidate. `0` turns suspension off. Sessions only: framework agents are parked by `AGENT_PARK_IDLE_SECS` ([Parked agents](AGENT_LIFECYCLE.md#parked-agents)). |
| `SESSION_IDLE_GRACE_SECS` | `120` | How long CPU must stay quiet. |
| `SESSION_CPU_QUIESCENT_MILLICORES` | `20` | CPU at or below this counts as quiet. |
| `SESSION_REAPER_INTERVAL_SECS` | `30` | How often the reaper scans. Clamped to 5-600. |
| `CODING_AGENT_TURN_LEASE_SECS` | `60` | How long a turn's lock on a session lasts without renewal. Bounds how long a crashed turn blocks the next one. |
| `CODING_AGENT_LEASE_HEARTBEAT_SECS` | `20` | How often a running turn renews its lease. |
| `AGENT_CALL_TIMEOUT_SECS` | `600` | Per-turn deadline. Also caps how long a turn waits for a human approval. |
| `SANDBOX_PROVIDER` | `docker` | Where sandboxes run. Docker is the only provider in this edition: `pod`, `shared-pod` and `harness-agent` are accepted but leave this edition with an unavailable provider that refuses every sandbox operation, so no turn can run; those providers come with other distributions. An unknown value fails startup. |
| `HARNESS_SHIM_MODE` | `env` | Read by the shim inside the sandbox, not by the server: `env` or `handshake` (section 8). Set by the provider on the sandbox, not by an operator. |
| `HARNESS_SHIM_LISTEN` | `0.0.0.0:7000` | Read by the shim: the address it serves on. The `--listen` flag wins. |
| `HARNESS_WORKSPACE` | `/workspace` | Read by the shim: the session's working directory and clone target. Must be absolute. The `--workspace` flag wins. |
| `HARNESS_IMAGE_PREFIX` | `nasiko/harness-` | Registry prefix of the harness images; override it to pull from a mirror. |

The server validates these at startup and refuses to boot when they conflict:

- `CODING_AGENT_IDLE_TTL_SECS` must be `0` or at least `SESSION_IDLE_GRACE_SECS`.
- `CODING_AGENT_TURN_LEASE_SECS` must be positive, and `CODING_AGENT_LEASE_HEARTBEAT_SECS` must be
  positive and at most half of it, so a live turn renews at least twice per lease.
- `AGENT_CALL_TIMEOUT_SECS` must not exceed 3600, the lifetime of the per-turn token.

## 7. Identity and credentials

A harness calls back into the platform: its model requests go to the platform's LLM router and its
tool calls go to the MCP gateway, both as the session's user. Two tokens make that work.

The per-sandbox token is placed in the sandbox environment (as `OPENAI_API_KEY`,
`ANTHROPIC_API_KEY` and `NASIKO_SESSION_TOKEN`, with `MCP_GATEWAY_URL` and
`MCP_GATEWAY_CONNECT_URL` alongside). It lives 30 days, is stored encrypted at rest so the sandbox
can be rebuilt, and is replaced when less than 24 hours remain. A replaced token reaches the
sandbox by stopping it before the turn starts it again: a provider whose resumed sandbox takes the
new environment suspends it, and one that fixes the environment when it creates the sandbox
(Docker) releases it, which keeps the workspace. It is revoked when the session is deleted, when the
sandbox is cleared, and when it is renewed. Treat it as readable by any process inside the sandbox,
including the code the harness runs.

The per-turn token lives one hour and is handed to the harness only as the ACP MCP header. It is
revoked when the turn ends.

Both are signed with `AGENT_JWT_SECRET`. If it is unset, harness agents are not seeded and the
server logs an error at startup. Rotating the secret invalidates every sandbox's token: the next
turn on each sandbox notices and replaces it, as above. The shim's WebSocket endpoint
(`/acp`) is authenticated with a per-sandbox bearer derived from the same secret.

### The profile

The harness images carry no model, provider or endpoint. On every turn the server renders the
session's *profile* — which model to ask the router for, and the harness-specific settings that
point it there — and sends it with the rest of the sandbox's configuration:

| Harness | What the profile sets |
|---------|-----------------------|
| OpenCode | `OPENCODE_CONFIG_CONTENT`: the router registered as an OpenAI-compatible provider (`openai-chat`) at `OPENAI_BASE_URL`, authenticated with `OPENAI_API_KEY`; `model` and `small_model`; `autoupdate: false`; and, when HITL is on, `edit`/`bash`/`webfetch` permissions set to `ask` |
| Codex | `~/.codex/config.toml` (Codex's default `$HOME/.codex`; `CODEX_HOME` is not set, so it follows the session's own HOME) naming the router as a Responses-API provider (`nasiko`) read from `OPENAI_API_KEY`, plus `model`. The HITL approval flags stay on the command line and win over the file |
| Claude Code | `ANTHROPIC_MODEL` and `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1`; the router is reached through `ANTHROPIC_BASE_URL`/`ANTHROPIC_API_KEY` from the base env |

The model is the user's default LLM config (`/api/llm-configs`, the row with `is_default`; its
`model`, else its tier models in the router's own order), else the platform default
(`DEFAULT_PROVIDER`/`DEFAULT_MODEL`). Each harness speaks one provider dialect to the router
(OpenCode and Codex the OpenAI one, Claude Code the Anthropic one), and the router routes a
harness agent's calls by that dialect, so a model on another provider is not rendered: the platform
default is used when it is on the dialect, otherwise the harness keeps its own default and the
turn's failure says so. The profile is rendered per turn, so a changed config reaches a sandbox
on its next turn where the shim runs in handshake mode (it restarts the harness on a changed
configuration); an env-mode sandbox keeps the profile it was created with until it is released.

Files the profile needs on disk (Codex's `config.toml`) travel as `HARNESS_FILES`, a JSON array of
`{"path", "content", "mode"?}`, which the configure body turns into its `files` block; the shim
writes them (absolute paths, mode `0600` unless given) before every spawn and keeps the variable
out of the harness's environment. A leading `~` in a file path expands against the
session's `HOME` in both modes. Env values expand only in handshake mode (the configure body's
`env`); an env-mode child inherits the process environment unexpanded.

Two more variables carry the repository: `WORKSPACE_REPO` (the URL) and `WORKSPACE_REPO_TOKEN`
(the user's GitHub token, set only for `https://github.com/` repositories of a GitHub-connected
user). Like the sandbox token, the latter is readable by any process in the sandbox; the shim
keeps it out of the harness's environment and passes it to git only through a private,
short-lived `GIT_ASKPASS` file.

## 8. The shim

`harness-shim` runs inside every sandbox. It starts the harness's ACP command, serves the
command's stdio as a WebSocket at `/acp`, answers `GET /healthz`, and exposes a small control API.
Before a turn connects, the server uses that API to make sure the shim runs the settings this turn
wants.

### Two modes

`HARNESS_SHIM_MODE` selects where a session's settings come from.

| Mode | Settings arrive | Used by |
|------|-----------------|---------|
| `env` (default) | In the shim's process environment, set when the sandbox is created. The harness starts at once and the shim exits when it does. | Providers whose sandbox environment is chosen per sandbox, such as Docker |
| `handshake` | By `POST /control/configure`, after the sandbox is attached. The shim binds its port and answers `/healthz` first, and starts nothing until it is configured. | Shared-runtime providers, whose pod is created before any of its sessions is known |

In handshake mode the configuration is held in memory only, so a resumed shim, a new process, is
configured again. If the harness process exits, a handshake shim keeps serving
(`child` reads `exited`) and starts it again on the next configure or restart.

The shared-runtime and per-user harness-agent providers described here are not part of this
edition; they come with other distributions. On a shared-runtime provider many shims share one pod under a pod-level supervisor
(`harness-supervisor`). Each session gets its own directory, which its shim serves as its workspace
(`--workspace` / `HARNESS_WORKSPACE`), and the server runs the ACP session there. When the pod has a
scratch area (`HARNESS_SCRATCH_ROOT`), each session also gets its own scratch directory in it,
`<root>/<sandbox key>`, the directory the server's env points at, and its `TMPDIR` is that
directory's `tmp/`, which the supervisor creates (node-local, and writable where the pod's root
filesystem is not). The supervisor confines a session to those two directories when confinement is on,
and a release removes both.

A provider may instead give each user one harness agent per harness: an agent of kind `harness`
owned by that user (named `harness-<harness id>`, never public and never shared), created by the
platform from the harness image on the user's first session for it. Its pod runs the supervisor, and
every session of that user and harness is a shim in it, with its directory in the agent's writable
folder and its scratch directory on the pod's local disk. The agent is parked while none of its
sessions runs and woken by the next turn; what was in the scratch area goes with the parked pod.
Calling that agent directly starts a session on it, as for the platform's own harness agent. The
platform deploys it: restarting or redeploying it through the agent lifecycle API, or uploading an
agent under its name, is refused with `409`.

### The control API

Control calls use the same port as `/acp`. A provider may put the sandbox behind a TLS endpoint with its
own CA; it then hands the server that CA with the attach, and the server trusts it for the WebSocket and
for every control call.

| Route | Purpose |
|-------|---------|
| `GET /healthz` | `200` `{"child": "waiting" \| "running" \| "exited", ...}`. In `env` mode a gone child is `503`; a handshake shim stays `200`. Never needs the bearer. |
| `POST /control/configure` | Handshake mode only (`409` in `env` mode). Body: `{"token": "<bearer>", "env": {..}, "workspace": {"repo", "token", "timeout_secs"} \| null, "files": [{"path", "content", "mode"?}], "env_hash": "<hex>"}` (`files` may be omitted). `204` when accepted. `400` for a body that does not parse, an empty `token`, or no harness command (neither `HARNESS_CMD` in `env` nor a default). `401` when a bearer is already set and the call lacks it. |
| `GET /control/status` | `200` with `mode` (`env` or `handshake`), `configured`, `env_hash` (the stored configuration), `running_env_hash` (what the live child was started with, `null` unless running), `child_generation` (spawn counter), `child` (`waiting`, `running` or `exited`), `evicting`, and `workspace` (the directory the shim serves; the server runs the ACP session there). |
| `GET /control/stats` | `200` with `processes` (`null` when `/proc` cannot be read, never `0`), `cpu_millicores`, `memory_bytes`, `sampled_at` and `evicting`, taken from the sandbox's own `/proc` for the shim's process tree. Works in both modes. On Linux the shim is a child subreaper, so a process whose parent exited (a backgrounded job whose shell exited, a server that daemonized itself) is re-parented to the shim and still counted. |
| `POST /control/restart` | Replaces the harness child, for a turn that ended while the child was blocked and may not answer the next prompt. `200` `{"child_generation": n}` once the new child is up; `409` while no child has been started yet; `500` `{"error": ...}` when the spawn failed (a handshake shim keeps serving); `503` while shutting down. |

Once a bearer is set, every route except `/healthz` needs `Authorization: Bearer <token>`; a missing
or wrong one is `401`. While none is set (an `env` shim without `HARNESS_SHIM_TOKEN`, or a handshake
shim before its first `configure`), requests are accepted without one. A repeated `configure` with the same `env_hash` changes
nothing. One with a different `env_hash` stores the new settings without touching a running child:
they apply on the next `POST /control/restart`, so configuring never interrupts a running turn. If
the child is gone, a configure starts it from the stored settings at once.

### The bearer

In `env` mode the bearer is `HARNESS_SHIM_TOKEN`, or none when unset. In handshake mode the first
`configure` claims it: the `token` in that call becomes the bearer, and from then on `/acp` and every
control route require it. The claim is atomic, so two simultaneous first calls cannot both win, and a
later `configure` carrying a different token is ignored (the bearer is never rotated). The server
derives the bearer from the session and `AGENT_JWT_SECRET`, so it can recompute it on any replica.

**Security note.** Until the first `configure`, whoever reaches the shim's port first sets the
bearer and the harness command (`HARNESS_CMD` in `env`). The shim has no other defence in that
window, so the sandbox's network isolation is the guard: only the platform's own path may reach the
port. Do not expose the port of an unconfigured handshake shim to anything else.

### What the server sends

Before every turn's WebSocket connect, the server reads `GET /control/status` and acts on it:

| Shim reports | Server does |
|--------------|-------------|
| `mode` is not `handshake` (an `env` shim, or an older shim with no `/control/status`, which answers `404`) | Connects as before |
| Not configured | `configure`, then connect |
| Configured with a different `env_hash`, child running | `configure`, then `restart`, then connect |
| Configured with a different `env_hash`, child not running | `configure`, then connect |
| Same `env_hash`, but the child runs an older one | `restart`, then connect |
| Same `env_hash`, child current | Connect |

After a `configure` or `restart` the server does not connect until the harness is running on the wanted
configuration: it polls `status` until `child` is `running` and `running_env_hash` equals the wanted
hash (or `child_generation` has reached the one the restart answered with), and fails the turn if
the child exits or the budget passes. The budget is 15 s, plus the repository clone timeout
(`WORKSPACE_CLONE_TIMEOUT_SECS`, default 300 s) after a first configure, because the shim clones before it
starts the harness. `restart` itself is synchronous: it answers only once the new child is spawned.

The configure body carries the env the turn would otherwise have set on the sandbox, except that
`WORKSPACE_REPO`, `WORKSPACE_REPO_TOKEN` and `WORKSPACE_CLONE_TIMEOUT_SECS` move into the
`workspace` block, so the harness's own environment never holds the git token, and `HARNESS_FILES`
moves into the `files` block. The `env_hash` is taken before that last move, so a changed file is a
changed configuration and restarts the harness. The shim clones the repository, then writes the
files, then starts the harness.

### Eviction

If the provider drains the node under a sandbox, it may signal the shim's process with `SIGTERM`. In
handshake mode the shim does not exit: it sets `evicting` (shown on `status` and `stats`) and keeps
relaying. A provider can also report `evicting` itself, as a shared-runtime provider does for a pod
that is being deleted. The server's idle reaper suspends an `evicting` session at once, busy or not,
unless a turn holds it. In `env` mode `SIGTERM` ends the shim as before.

## 9. Limitations

- On a provider whose shim runs in `env` mode (Docker, pods), the profile and the HITL setting are
  frozen into a sandbox when it is created, so a change reaches an existing sandbox only after it
  is released and created again. Handshake-mode sandboxes pick them up on the next turn.
- The router routes a harness agent's calls by the dialect the harness speaks and the harness
  agent row's own config, not the session user's: a user's default config on another provider
  is not used, and a config attached to the harness agent (or its owner's default) overrides the
  model the profile asks for.
- Of the A2A methods, harness agents answer `message/send`, `message/stream` and the three task
  methods above. Task history (`tasks/list`) and push notifications are not supported.
- `tasks/resubscribe` on a replica that does not run the turn streams every chunk only while Redis
  is reachable (otherwise state changes and the final reply, polled from the row), never replays
  frames sent before subscribing, and `tasks/get` polling on the dispatch route counts against
  its rate limit.
- Only OpenCode, Claude Code and Codex ship.
