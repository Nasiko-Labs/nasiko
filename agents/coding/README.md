# nasiko-coding-agent

A deployable A2A agent that develops, tests, and refactors code inside a sandboxed workspace.
It runs an inline ReAct loop (reason → call a tool → observe → repeat) over an OpenAI-compatible
chat model, with specialized coding tools confined to a workspace directory.

Follows the deployable-agent pattern of `agents/paper` (A2A server via `a2a-server`,
`AgentExecutor` impl, `/jsonrpc` + agent-card HTTP endpoints), but is a **root workspace member**
and routes all file/shell access through a pluggable `Sandbox` backend.

## Status

| Phase | Scope | State |
|-------|-------|-------|
| **Phase 1** | Agent crate, coding tools, `LocalSandbox` (CLI-ready) | ✅ **Done & verified** |
| **Phase 2** | Orchestrator `exec` + CP `/api/sandboxes` + pre-warmed per-language pools + `RemoteSandbox` (CP deployment) | ⬜ Not started |

### Phase 1 — done

- `src/sandbox/mod.rs` — `Sandbox` trait (`read_file`, `read_file_raw`, `write_file`, `list_dir`,
  `exec`) + `ExecResult` + `from_env()` backend factory.
- `src/sandbox/local.rs` — `LocalSandbox` rooted at `WORKSPACE_DIR`. Lexical **path containment**
  rejects `..`/absolute escapes; `exec` runs `sh -c` with a wall-clock timeout and output
  truncation.
- `src/project.rs` — detects project language (Cargo.toml / package.json / pyproject / go.mod) and
  the default test command.
- `src/tools.rs` — `definitions()` + `execute()` for: `read_file`, `list_directory`, `search_code`
  (ripgrep, grep fallback), `write_file`, `edit_file` (**search/replace diff**, unique-match
  enforced), `run_command`, `run_tests`.
- `src/main.rs` — `CodingAgent` + `AgentExecutor` with the ReAct loop (`MAX_TURNS = 12`), a
  coding-focused system prompt, streamed `status_working` updates, and the `AgentCard`
  (skills: `code-edit`, `code-test`, `code-refactor`).
- `Dockerfile` — based on `rust:1-slim` (not `scratch`: the tools need a shell + toolchain),
  with `git` and `ripgrep` installed and `WORKSPACE_DIR=/workspace`.

**Verification:** 10 unit tests pass (path containment, `edit_file` not-found/not-unique,
exec exit codes, dir listing); `cargo check --workspace` is clean under `RUSTFLAGS="-D warnings"`;
the server boots and serves a correct `/.well-known/agent-card.json`.

### Phase 2 — pending

For control-plane deployment the agent will provision a fresh per-request, per-language sandbox,
run the SDLC in it, stream task updates, then release it. Backend decided (after research):
**Docker via the existing orchestrator, semi-trusted code, hardened containers, gVisor `runsc`
opt-in, pre-warmed per-language pools.** Work needed across three layers:

1. **Orchestrator** — add an `exec` primitive (and an idle, hardened sandbox deploy path).
2. **Control plane** — `/api/sandboxes` create/exec/delete + a pre-warmed per-language pool.
3. **Agent** — `src/sandbox/remote.rs` (`RemoteSandbox`) behind the existing `Sandbox` trait;
   `SANDBOX_MODE=remote` (currently returns a loud "not implemented" error).

Deferred behind the trait: Firecracker microVMs (untrusted tier), gVisor checkpoint/restore.

## Running (CLI / local mode)

```sh
OPENAI_API_KEY=sk-...            # required
OPENAI_BASE_URL=https://...      # optional, OpenAI-compatible
OPENAI_MODEL=gpt-4o-mini         # optional
WORKSPACE_DIR=/path/to/project   # workspace root (default: current dir)
SANDBOX_MODE=local               # default; 'remote' is Phase 2
PORT=8000                        # default 8080

cargo run -p nasiko-coding-agent
```

Then:

```sh
curl -s localhost:8000/.well-known/agent-card.json | jq .

curl -s localhost:8000/jsonrpc -d '{"jsonrpc":"2.0","id":"1","method":"message/send",
  "params":{"message":{"messageId":"m1","role":"user",
  "parts":[{"kind":"text","text":"add an add(a,b) fn to lib.rs and run the tests"}],
  "contextId":"c1"}}}'
```

## Environment variables

| Var | Default | Purpose |
|-----|---------|---------|
| `OPENAI_API_KEY` | — | LLM API key (required for the ReAct loop) |
| `OPENAI_BASE_URL` | `https://api.openai.com/v1` | OpenAI-compatible endpoint |
| `OPENAI_MODEL` | `gpt-4o-mini` | Model id |
| `WORKSPACE_DIR` | `.` | Workspace root the tools are confined to |
| `SANDBOX_MODE` | `local` | `local` (host workspace) or `remote` (Phase 2) |
| `PORT` | `8080` | HTTP listen port |
