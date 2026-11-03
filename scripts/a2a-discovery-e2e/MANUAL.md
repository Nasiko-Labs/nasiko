# Local A2A agents: test by hand

The steps `run.sh` automates, as commands to paste one block at a time, so you can watch each
step on your own machine (written for macOS; Linux works the same). Everything stays in a
scratch folder: a separate `HOME` for the CLI and a separate database, so your `~/.nasiko` and
your dev database are not touched.

To run it all unattended instead: `just infra && just test-discovery`.

## 0. Once: infra, UI, binaries, scratch folder

Run from the repo's `oss/` folder.

```sh
export PYTHON=${PYTHON:-python3}
"$PYTHON" -c 'import sys; assert sys.version_info >= (3, 10), "need Python 3.10+, got " + sys.version'

just infra
(cd .. && just build-ui)
cargo build -p nasiko -p nasiko-server

export OSS=$PWD
export TARGET=$(cargo metadata -q --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')
export T=$(mktemp -d)
mkdir -p "$T/home/.nasiko"
nasiko() { HOME="$T/home" "$TARGET/debug/nasiko" "$@"; }
echo "scratch: $T"
```

LangGraph needs Python 3.10 or newer, and macOS's own `python3` is 3.9: if the check fails, run
`brew install python@3.12` and `export PYTHON=$(brew --prefix python@3.12)/bin/python3.12`
first (`just test-discovery` reads `PYTHON` too).

`just infra` starts Postgres, Redis and S3 in containers, plus the trace collector, Tempo and
Loki the control plane reads calls from. `just build-ui` (from the repo root; it needs Node)
builds the web UI the server embeds; without it the control plane shows "UI not built". `$T` is
the scratch folder (agent, virtualenv, logs, CLI home). `nasiko` is now a shell function that runs the freshly built CLI
with the scratch `HOME`. Keep this terminal open for every step below.

## 1. A LangGraph agent that calls a model

```sh
"$PYTHON" -m venv "$T/venv"
"$T/venv/bin/pip" install -q 'langgraph-cli[inmem]==0.4.37' 'langchain-openai==1.7.3'

cp -R "$OSS/scripts/a2a-discovery-e2e/langgraph" "$T/app"
cd "$T/app"
printf 'OPENAI_API_KEY=sk-agent-own-key\nKEEP_ME=1\n' > .env
cp .env "$T/env.before"

LANGSMITH_TRACING=false "$T/venv/bin/langgraph" dev --no-browser --no-reload --port 2024 > "$T/langgraph.log" 2>&1 &
echo $! > "$T/langgraph.pid"
sleep 15 && curl -s localhost:2024/info | head -c 120; echo
```

`langgraph.json` has `"env": ".env"`, so the server reads `.env` at startup.

## 2. Discovery, with no server

```sh
nasiko agents discover
```

Expect the usual coding-agent table, plus `planner` and `writer` rows with an A2A CARD URL on
port 2024. CONNECTED is `no`.

```sh
nasiko status
```

Expect the coding agents only: `status` sends nothing to local ports. It ends with
`Error: no active cluster` because no cluster is connected yet.

## 3. A Nasiko server with a stand-in OpenAI

```sh
python3 "$OSS/scripts/a2a-discovery-e2e/mock_openai.py" 9911 "$T/upstream.log" &
echo $! > "$T/mock.pid"

docker compose -f "$OSS/docker-compose.infra.yml" exec -T postgres \
  psql -U nasiko -d nasiko_dev -c 'DROP DATABASE IF EXISTS nasiko_manual' -c 'CREATE DATABASE nasiko_manual'

DATABASE_URL=postgres://nasiko:nasiko@localhost:5432/nasiko_manual REDIS_URL=redis://localhost:6379 \
  S3_ENDPOINT=http://localhost:9000 S3_ACCESS_KEY=nasiko S3_SECRET_KEY=nasiko123 S3_REGION=us-east-1 \
  SECRETS_ENCRYPTION_KEY=kmd9Ic4cgD9YYE/owDL3QSzpKI/BZ3pmVZQ67X+eABc= \
  JWT_SECRET=manual-jwt-secret-0123456789abcdef AGENT_JWT_SECRET=manual-agent-secret-0123456789 \
  ADMIN_USERNAME=admin ADMIN_PASSWORD=manual-admin CP_BIND=127.0.0.1:18080 \
  AGENT_RUNTIME=simulated OPENAI_API_BASE=http://127.0.0.1:9911 PLATFORM_OPENAI_API_KEY=sk-nasiko-platform \
  OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317 TEMPO_URL=http://localhost:3200 LOKI_URL=http://localhost:3100 \
  TRACE_USAGE_SYNC_SECS=10 \
  "$TARGET/debug/nasiko-server" > "$T/server.log" 2>&1 &
echo $! > "$T/server.pid"
sleep 10 && curl -s localhost:18080/health; echo
```

With podman, use `podman compose` in the `psql` line. Port 9911 and 18080 must be free. The
`OTEL_*`, `TEMPO_URL` and `LOKI_URL` lines point the server at the collector, Tempo and Loki that
`just infra` runs, which turns on observability; `TRACE_USAGE_SYNC_SECS=10` makes the control
plane pick up calls within 10 s instead of 2 minutes.

Log the scratch CLI in, without the password prompt:

```sh
curl -s -X POST localhost:18080/api/auth/login -H 'content-type: application/json' \
  -d '{"username":"admin","password":"manual-admin"}' |
  python3 -c 'import json,sys; t=json.load(sys.stdin)["token"]; json.dump({"active":"manual","clusters":{"manual":{"url":"http://127.0.0.1:18080","username":"admin","token":t}}}, open(sys.argv[1],"w"))' \
  "$T/home/.nasiko/config.json"
```

## 4. Registration

```sh
nasiko agents discover
```

Logged in now, so discovery also registers `planner` and `writer` with the server. CONNECTED
is still `no`. `status` now shows them too, read back from the server (it probes nothing):

```sh
nasiko status
```

Expect, after the coding agents, an `A2A agents:` section listing `planner` and `writer` with
LLM ROUTING `off`. The server has them:

```sh
docker compose -f "$OSS/docker-compose.infra.yml" exec -T postgres \
  psql -U nasiko -d nasiko_manual -c "select name, status, discovered_on from agents"
```

Expect `planner` and `writer`, status `registered`, `discovered_on` your Mac's name.

## 5. Connect

```sh
nasiko connect planner
cat "$T/app/.env"
nasiko agents discover
nasiko status
```

`connect` asks two things first. **Which LLM providers** the agent calls: OpenAI is already
ticked, because its `.env` sets `OPENAI_API_KEY` (Anthropic would be ticked if it set an
`ANTHROPIC_*` variable); press enter. **Whether to give it your MCP tools** through the gateway:
answer `y` (the default is no, since it needs a few lines in the agent's code). Then it names the
file it will write (here the agent's existing `.env`) and asks before writing; after writing it
says the file now holds this agent's Nasiko key, so keep it out of version control. In `.env`,
expect your own `OPENAI_API_KEY` line commented out as `# nasiko-off: …`, and a block between
`# >>> nasiko` and `# <<< nasiko` with `OPENAI_BASE_URL=http://127.0.0.1:18080/v1` and this
agent's key, no `ANTHROPIC_*` lines, plus `MCP_GATEWAY_URL`, `MCP_GATEWAY_TOKEN` and
`MCP_GATEWAY_CONNECT_URL`: the MCP gateway variables a Nasiko-deployed agent gets. Because no
framework opens an MCP client from the environment on its own, `connect` also prints the few
lines of LangGraph code that do (CrewAI agents get CrewAI's). Step 5b runs them. Without a
terminal (`--yes`), `--provider openai,anthropic` and `--mcp` give the same answers.
`planner`'s CONNECTED is `waiting`, and `status` shows its LLM ROUTING `waiting`: LangGraph has
not reread `.env` yet.

### 5b. MCP tools through Nasiko

Give Nasiko an MCP server with one tool (`add`), let `planner` use it, then run the code
`connect` printed against the variables it wrote:

```sh
"$T/venv/bin/pip" install -q 'langchain-mcp-adapters==0.3.2'
"$T/venv/bin/python" "$OSS/scripts/a2a-discovery-e2e/mock_mcp.py" 9912 > "$T/mcp.log" 2>&1 &
echo $! > "$T/mcp.pid"
```

The stand-in server is on your own machine, which Nasiko refuses to call unless the server was
started with `MCP_ALLOW_PRIVATE_URLS=true`: add it to the server's line in step 3 and restart it
for this step. Then, in the control plane:

1. **Connect → MCP servers → Register server**: name `calc`, URL `http://127.0.0.1:9912/mcp`, no
   auth.
2. **Agents → planner → MCP**: switch `calc` on. Its tools appear (`add`), each `allow` or
   `block`; there is no `ask` for an agent running outside Nasiko, since nothing could answer it.

```sh
"$T/venv/bin/python" "$OSS/scripts/a2a-discovery-e2e/mcp_langgraph.py" "$T/app/.env"
```

Expect `5`. Outside a Nasiko flow the gateway lists only its search and call tools, so the
script finds `add` with `nasiko_search_tools` and runs it with `nasiko_call_tool`, as the agent's
model would; the call runs as you, the agent's owner. LLM ROUTING stays `waiting`: a tool call
is not an LLM call.

Two copies of one app (the same agent names, served from two folders) are two agents. To see
it, start a second copy on another port and discover again:

```sh
cp -R "$OSS/scripts/a2a-discovery-e2e/langgraph" "$T/app-copy"
cp "$T/env.before" "$T/app-copy/.env"
(cd "$T/app-copy" && LANGSMITH_TRACING=false "$T/venv/bin/langgraph" dev --no-browser --no-reload --port 2025 > "$T/langgraph-copy.log" 2>&1 & echo $! > "$T/langgraph-copy.pid")
sleep 15 && nasiko agents discover
```

Each copy has its own row, named by its catalog name: `planner` and `planner-<4 letters>` (the
A2A CARD column says which is on which port). `nasiko connect` and `nasiko disconnect` take
either name, or the card URL. Restarting a copy, even on another port, keeps its name, key and
history: a copy is known by its folder. Stop the copy before going on, so the steps below see
one `planner`: `kill $(cat "$T/langgraph-copy.pid")`.

## 6. Restart the agent; a call goes through Nasiko

```sh
kill $(cat "$T/langgraph.pid"); sleep 3
cd "$T/app"
LANGSMITH_TRACING=false "$T/venv/bin/langgraph" dev --no-browser --no-reload --port 2024 > "$T/langgraph.log" 2>&1 &
echo $! > "$T/langgraph.pid"
sleep 15

curl -s -X POST localhost:2024/runs/wait -H 'content-type: application/json' \
  -d '{"assistant_id":"planner","input":{"messages":[{"role":"user","content":"hi"}]}}' | grep -o '"routed"'
cat "$T/upstream.log"
nasiko agents discover
nasiko status
```

Expect `"routed"` (the stand-in's reply), `Bearer sk-nasiko-platform` in the upstream log (the
call went through Nasiko, which used its own key, never the agent's), `planner` CONNECTED `yes`,
and LLM ROUTING `on` in `status`.

## 7. See the call in the control plane

Open http://127.0.0.1:18080 and log in as `admin` / `manual-admin`. In the sidebar:

- **TokenOps → Monitor**: within 10 s of the call, the summary reads "planner drove 100% of it",
  Operations is 1, Tokens is 18 (the stand-in's 11 in + 7 out) and Active agents is 1 of 2.
  Further down, `planner` is the top agent under Highest spend. Use the **Agent** filter at the
  top to look at `planner` alone. With no call yet it shows nothing: it only lists agents that
  spent.
- **Connect → LLM router**: the Agents tab lists `planner` and `writer`. `planner`'s
  "Spend (30 d, router)" is `<$0.01 · 1 calls`; `writer` says "No routed calls". This page reads
  the router's own log, so it updates at once.
- **Agents**: `planner` and `writer` are in the catalog as "Not deployed": Nasiko knows them
  but never runs or calls them.
- **Observe → Sessions**: one session for `planner`, holding every call it routed (one session
  per agent for now). Each call is a turn: the latest user message and the model's reply. With
  the session id from the row, http://127.0.0.1:18080/sessions/<id>?view=ledger lists the turns
  with their tokens, and http://127.0.0.1:18080/chat/<id> shows the conversation. Make a second
  call first to see it grow:

  ```sh
  curl -s -X POST localhost:2024/runs/wait -H 'content-type: application/json' \
    -d '{"assistant_id":"planner","input":{"messages":[{"role":"user","content":"what about tomorrow"}]}}' > /dev/null
  ```

  The session is written after the reply is sent, never in the call's way, so it can lag the
  call by a moment. `writer` has none: it made no call.

One place that stays empty, as expected:

- The **View traces** drill-down on Monitor says "Trace drill-down needs a newer nasiko-server":
  it calls `/api/observability/finops/top-traces`, which the server does not have yet. That is
  the same for every agent today.

The router also records each call as a `gen_ai.chat` span (model, provider, tokens, the agent's
id) and exports it to Tempo. There is no UI for raw spans in this setup; to see one, query
Tempo's API with the agent's id:

```sh
ID=$(docker compose -f "$OSS/docker-compose.infra.yml" exec -T postgres \
  psql -U nasiko -d nasiko_manual -tAc "select id from agents where name = 'planner'")
curl -sG localhost:3200/api/search --data-urlencode "q={ span.gen_ai.agent.id = \"$ID\" }"
```

That returns the trace IDs; `curl -s localhost:3200/api/traces/<traceID>` shows one in full.

## 8. Disconnect

```sh
KEY=$(sed -n 's/^OPENAI_API_KEY=//p' "$T/app/.env")
nasiko disconnect planner
diff "$T/env.before" "$T/app/.env" && echo "restored"
curl -s -o /dev/null -w '%{http_code}\n' localhost:18080/v1/chat/completions -H "authorization: Bearer $KEY" \
  -H 'content-type: application/json' -d '{"model":"gpt-4o-mini","messages":[{"role":"user","content":"hi"}]}'
nasiko agents discover
nasiko status
```

Expect `restored` (byte for byte), then `401`: `disconnect` revoked the agent's LLM key, so a
copy of it kept anywhere else stops working too. `planner` is CONNECTED `no`, and LLM ROUTING
`off` in `status`. Its MCP gateway credential is revoked as well: the script from step 5b now
fails with HTTP 401. A later `nasiko connect planner` gives it new ones.

## 9. Clean up

```sh
kill $(cat "$T/langgraph.pid" "$T/server.pid" "$T/mock.pid" "$T/mcp.pid" 2>/dev/null)
docker compose -f "$OSS/docker-compose.infra.yml" exec -T postgres \
  psql -U nasiko -d nasiko_dev -c 'DROP DATABASE nasiko_manual'
rm -rf "$T"
```

Your real `~/.nasiko` was never used: every `nasiko` call above ran with `HOME=$T/home`.
