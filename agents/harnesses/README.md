# Platform harness images

Coding-agent harnesses (OpenCode, Claude Code, Codex) speak ACP: newline-delimited JSON-RPC over
stdio. Sandboxes are reached only over the network, so each image runs `harness-shim`
(`harness-shim/`), which starts the harness's ACP command and exposes its stdio as a WebSocket.

## Contract

- `GET /acp` upgrades to a WebSocket. Each Text frame is one JSON-RPC line in either direction.
  Binary frames close the socket with 1003.
- At most one client. A new connection closes the previous one with 1012 and takes over.
- The harness outlives client disconnects (sandboxes are frozen between turns). While nobody is
  attached the shim buffers the harness's output (`--buffer-lines`, default 10000) and flushes it,
  in order, on the next attach. When full it drops the oldest notifications first and never drops a
  frame carrying an `id`, so permission prompts (`session/request_permission`) survive.
- `GET /healthz` returns 200 `{"child":"running","attached":bool,"buffered":n}`, or 503 once the
  harness has exited. When the harness exits the shim closes the socket with 1011 and exits with the
  harness's code.
- A repeated `initialize` is answered by the shim. A sandbox resume means a fresh client that
  re-sends `initialize` to a harness that is already initialised. The shim caches the child's
  response to the first `initialize`, answers any later one from that cache (with the new request
  id), and does not forward it to the child.
- If the harness's stdout closes while it is still running, or one line exceeds 64 MiB, the shim
  terminates it and exits 1. A client that stops answering WebSocket pings (every 20 s) is
  detached; its undelivered frames stay buffered.
- Optional auth: if `HARNESS_SHIM_TOKEN` is set, `/acp` requires `Authorization: Bearer <token>`
  (401 otherwise). Unset means open, relying on network isolation.
- The command comes from `-- <cmd> args` or `HARNESS_CMD`; the images set `HARNESS_CMD`. It is
  split on whitespace, with no quoting.
- Session files: a harness that reads its settings from disk (Codex's `config.toml`) gets them
  written by the shim before every spawn, from the `files` block of `POST /control/configure`
  (handshake mode) or `HARNESS_FILES` (env mode, the same JSON array of
  `{"path", "content", "mode"?}`; absolute paths only, mode `0600` unless given). `HARNESS_FILES`
  is removed from the harness's environment.

## The supervisor

Every harness image also carries `harness-supervisor` (same crate), for providers that run
many sessions of one image in a single pod (one user's sessions of that harness). It starts one
`harness-shim` per session, in handshake mode, in the session's own directory `<root>/<id>` (named
by the session id; it is the shim's workspace and holds its `HOME`), on the port the platform tells
it: the platform owns port placement, the supervisor never picks one. Requests for one session are
handled one at a time; different sessions proceed in parallel. With `HARNESS_SCRATCH_ROOT` set, each
session also gets a scratch directory `<scratch root>/<id>`, and its `TMPDIR` is that directory's
`tmp/`. The supervisor confines no session: the pod holds one user's sessions, so the pod is the
boundary.

- `GET /healthz`: `200 {"sessions": n}` (running sessions); the only route without the bearer.
- `PUT /sessions/{id}` `{"port": p, "shim_token"?: "<bearer>"}`: start the session's shim on port
  `p` and answer `200 {"port", "generation"}` once it serves `/healthz`; a running session on the
  same port is answered as is. `shim_token` becomes the shim's `HARNESS_SHIM_TOKEN`, so the shim
  demands it from its first request instead of taking the bearer from its first `configure`; a
  running shim keeps the bearer it was started with. A shim that does not come up is stopped and
  the answer is `500`; the next `PUT` starts it afresh. `400` for an id that is not one plain
  path segment (`[A-Za-z0-9._-]`, not `.`/`..`), a port below 1024 or equal to the supervisor's
  own, an empty `shim_token`, or a malformed body; `409` when the pod is full, `p` is another
  session's, or the session runs on another port.
- `DELETE /sessions/{id}` (`?purge=true`): SIGINT to the shim's process group, SIGKILL 15 s later
  (the shim's own 10 s grace for its harness plus a margin); the session stays listed as
  `exited` and the next `PUT` restarts it in the same directory. `purge` also removes the
  directory, then forgets the session; it removes `<root>/<id>` even for a session the
  supervisor does not know (one it forgot by restarting). `204`; `400` for an invalid id.
- `GET /sessions`: every session, `{id, port, generation, started_at, state, stats}`, where `stats`
  (`cpu_millicores`, `memory_bytes`, `processes`, `sampled_at`) covers the shim's descendants, the
  same figures as the shim's `/control/stats`, and is `null` once the shim has exited or when it
  could not be sampled.
- `GET /sessions/{id}`: one of those, `404` when absent.

Env: `HARNESS_SUPERVISOR_TOKEN` (required: the bearer, kept out of the shims' environment),
`HARNESS_SUPERVISOR_LISTEN` (`0.0.0.0:7001`), `HARNESS_SUPERVISOR_MAX_SESSIONS` (`8`),
`HARNESS_SHIM_BINARY` (`/usr/local/bin/harness-shim`), `HARNESS_WORKSPACE_ROOT` (`/workspace`),
`HARNESS_SCRATCH_ROOT` (unset: no scratch directories).
On SIGTERM or SIGINT it stops every session the same way before exiting.

## What the images do not contain

No model, provider, endpoint or permission setting is baked in. The server renders those per
session from the user's default LLM config and the HITL toggle (the *harness profile*,
`server/src/sessions/profile.rs`): `OPENCODE_CONFIG_CONTENT` for OpenCode, `CODEX_HOME` plus
a written `config.toml` for Codex, `ANTHROPIC_MODEL` for Claude Code. The images only pin the
harness and turn off what an immutable, egress-restricted sandbox cannot do (self-update, catalog
fetches).

## Provider requirements

The images run as uid 65534 with `HOME=/workspace/home`. The sandbox provider must make
`/workspace` writable by that uid; the shim creates `$HOME` at start (warning only on failure).

## Build

```sh
just build-harness-images [TAG]   # needs the x86_64-unknown-linux-musl target and Docker
```

On macOS the musl cross-link needs `musl-cross` or `cargo-zigbuild`.

Harness and adapter versions are pinned in each Dockerfile (`ARG`s) and passed by the recipe:
OpenCode `1.19.5`; Claude Code `2.1.312` with `@zed-industries/claude-code-acp` `0.16.2`; Codex
`0.165.0` with `@zed-industries/codex-acp` `0.16.0` (npm releases current when pinned). Bump them
together in the Dockerfile and the recipe.

The image names and tags the platform's harness catalog points at (`nasiko/harness-*`) are
placeholders: set `HARNESS_IMAGE_PREFIX`, or edit the catalog rows, to point at images you built
and pushed.

The recipe builds the static shim and supervisor, copies both into each image directory for the
build (git-ignored), and tags `nasiko/harness-{opencode,claude-code,codex}:TAG`.
