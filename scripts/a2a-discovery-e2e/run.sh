#!/usr/bin/env bash
# End-to-end check of local A2A agents, against a real CrewAI agent and a real LangGraph server.
#
# 1. Discovery: `nasiko agents discover` lists all three agents in its table (no server needed).
# 2. Registration + LLM routing: against a real nasiko-server (needs `just infra`), discovery
#    registers the agents; `nasiko connect planner` writes the LangGraph `.env`; after a restart
#    the agent's model call goes through Nasiko and discovery shows it CONNECTED `yes`;
#    `nasiko disconnect planner` restores the `.env` exactly.
#
# Pass --discovery-only to skip step 2. The first run installs both frameworks into
# virtualenvs under .work/ (needs network); later runs reuse them. Everything runs on free
# ports picked here, the CLI uses its own HOME under .work/, and step 2 uses its own database,
# so your ~/.nasiko and your dev database are never touched.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
oss=$(cd "$here/../.." && pwd)
work="$here/.work"
python=${PYTHON:-python3}
discovery_only=false
[ "${1:-}" = "--discovery-only" ] && discovery_only=true
mkdir -p "$work"

venv() { # venv <name> <packages...>; reinstalls when the package list changes
  local dir="$work/venv-$1" wanted="${*:2}"
  [ -x "$dir/bin/python" ] && [ "$(cat "$dir/.packages" 2>/dev/null)" = "$wanted" ] && return
  echo "Installing $1 into $dir ..."
  rm -rf "$dir"
  "$python" -m venv "$dir"
  "$dir/bin/pip" install -q "${@:2}"
  echo "$wanted" > "$dir/.packages"
}
venv crewai 'crewai[a2a]==1.15.28' uvicorn
venv langgraph 'langgraph-cli[inmem]==0.4.37' 'langchain-openai==1.7.3'

free_port() {
  "$python" -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])'
}
crewai_port=$(free_port)
langgraph_port=$(free_port)

pids=()
trap 'kill "${pids[@]}" 2>/dev/null || true' EXIT

fail() { echo "FAIL: $*" >&2; exit 1; }
wait_for() { # wait_for <name> <url>
  for _ in $(seq 120); do curl -sf --noproxy '*' -o /dev/null "$2" && return; sleep 1; done
  tail -5 "$work/$1.log" >&2
  fail "$1 did not start; see $work/$1.log"
}

echo "Building the CLI and server ..."
(cd "$oss" && cargo build -q -p nasiko -p nasiko-server)
target=$(cd "$oss" && cargo metadata -q --no-deps --format-version 1 |
  "$python" -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])')
export HOME_E2E="$work/home"
rm -rf "$HOME_E2E" && mkdir -p "$HOME_E2E/.nasiko"
nasiko() { HOME="$HOME_E2E" NO_PROXY='*' no_proxy='*' "$target/debug/nasiko" "$@"; }

"$work/venv-crewai/bin/python" "$here/crewai_agent.py" "$crewai_port" > "$work/crewai.log" 2>&1 &
pids+=($!)

# Served from a copy, so the dev server's state and the `.env` stay out of the source tree.
app="$work/app"
rm -rf "$app" && cp -R "$here/langgraph" "$app"
printf 'OPENAI_API_KEY=sk-agent-own-key\nKEEP_ME=1\n' > "$app/.env"
cp "$app/.env" "$work/env.before"
langgraph_pid=
start_langgraph() {
  (cd "$app" && LANGSMITH_TRACING=false NO_PROXY='*' no_proxy='*' \
    exec "$work/venv-langgraph/bin/langgraph" dev --no-browser --no-reload --port "$langgraph_port") \
    >> "$work/langgraph.log" 2>&1 &
  langgraph_pid=$!
  pids+=($langgraph_pid)
  wait_for langgraph "http://127.0.0.1:$langgraph_port/info"
}
: > "$work/langgraph.log"

echo "Waiting for the agents ..."
start_langgraph
wait_for crewai "http://127.0.0.1:$crewai_port/.well-known/agent-card.json"

# ── 1. Discovery ─────────────────────────────────────────────────────────────
output=$(nasiko agents discover)
echo "$output"
for agent in crew-researcher planner writer; do
  grep -qE "^ *$agent " <<< "$output" || fail "$agent was not discovered (logs in $work)"
done
echo "PASS: all agents discovered"

# `status` is unchanged: it probes nothing, so it lists no A2A agent. (No cluster yet, so it
# exits non-zero; its output is still printed.)
status_output=$(nasiko status 2>&1 || true)
for agent in crew-researcher planner writer; do
  ! grep -qE "^ *$agent " <<< "$status_output" || fail "status listed $agent; it must not probe"
done
echo "ok: status lists no A2A agents"
$discovery_only && exit 0

# ── 2. Registration + LLM routing ────────────────────────────────────────────
psql_dev() { # runs SQL against the infra Postgres, with psql or through the infra container
  if command -v psql > /dev/null; then
    PGPASSWORD=nasiko PGOPTIONS='-c client_min_messages=warning' psql -q -h localhost -U nasiko -d nasiko_dev "$@"
  else
    "${DOCKER:-docker}" compose -f "$oss/docker-compose.infra.yml" exec -T -e PGOPTIONS='-c client_min_messages=warning' postgres \
      psql -q -U nasiko -d nasiko_dev "$@"
  fi
}
db=nasiko_discovery_e2e
psql_dev -c "DROP DATABASE IF EXISTS $db" -c "CREATE DATABASE $db" > /dev/null ||
  fail "cannot reach the infra Postgres; run \`just infra\` first (or pass --discovery-only)"

upstream_port=$(free_port)
server_port=$(free_port)
: > "$work/upstream.log"
"$python" "$here/mock_openai.py" "$upstream_port" "$work/upstream.log" > "$work/upstream.out" 2>&1 &
pids+=($!)

server="http://127.0.0.1:$server_port"
DATABASE_URL="postgres://nasiko:nasiko@localhost:5432/$db" REDIS_URL=redis://localhost:6379 \
  S3_ENDPOINT=http://localhost:9000 S3_ACCESS_KEY=nasiko S3_SECRET_KEY=nasiko123 S3_REGION=us-east-1 \
  SECRETS_ENCRYPTION_KEY=kmd9Ic4cgD9YYE/owDL3QSzpKI/BZ3pmVZQ67X+eABc= \
  JWT_SECRET=discovery-e2e-jwt-secret-0123456789 AGENT_JWT_SECRET=discovery-e2e-agent-secret-0123 \
  ADMIN_USERNAME=admin ADMIN_PASSWORD=discovery-e2e-admin CP_BIND="127.0.0.1:$server_port" \
  AGENT_RUNTIME=simulated OPENAI_API_BASE="http://127.0.0.1:$upstream_port" \
  PLATFORM_OPENAI_API_KEY=sk-nasiko-platform NO_PROXY='*' no_proxy='*' \
  "$target/debug/nasiko-server" > "$work/server.log" 2>&1 &
pids+=($!)
wait_for server "$server/health"

# Log in the way `nasiko auth login` does, without its prompt.
curl -sf --noproxy '*' -X POST "$server/api/auth/login" -H 'content-type: application/json' \
  -d '{"username":"admin","password":"discovery-e2e-admin"}' |
  "$python" -c '
import json, sys
token = json.load(sys.stdin)["token"]
cluster = {"url": sys.argv[1], "username": "admin", "token": token}
json.dump({"active": "e2e", "clusters": {"e2e": cluster}}, open(sys.argv[2], "w"))
' "$server" "$HOME_E2E/.nasiko/config.json"

connected_of() { # connected_of <agent>: the agent's CONNECTED column in `nasiko agents discover`
  nasiko agents discover 2>&1 | awk -v agent="$1" '$1 == agent && $2 == "yes" { print $3; exit }'
}
expect_connected() { # expect_connected <agent> <yes|waiting|no> <when>
  local actual
  actual=$(connected_of "$1")
  [ "$actual" = "$2" ] || fail "$1 CONNECTED is '$actual' $3, expected '$2'"
  echo "ok: $1 CONNECTED $2 $3"
}

expect_connected planner no "once registered"

connected=$(nasiko connect planner --yes 2>&1)
echo "$connected"
grep -q "now holds this agent's Nasiko key" <<< "$connected" ||
  fail "connect did not tell the user the file now holds the agent's key"
grep -q '^# >>> nasiko' "$app/.env" || fail "connect did not write Nasiko's block to $app/.env"
grep -q '^# nasiko-off: OPENAI_API_KEY=sk-agent-own-key$' "$app/.env" ||
  fail "connect did not set the agent's own OPENAI_API_KEY aside"
grep -q "^OPENAI_BASE_URL=$server/v1$" "$app/.env" || fail "connect wrote the wrong OPENAI_BASE_URL"
expect_connected planner waiting "after connect"

echo "Restarting LangGraph so it reads the new .env ..."
kill "$langgraph_pid"
for _ in $(seq 30); do curl -sf --noproxy '*' -o /dev/null "http://127.0.0.1:$langgraph_port/info" || break; sleep 1; done
sleep 1
start_langgraph
curl -sf --noproxy '*' -X POST "http://127.0.0.1:$langgraph_port/runs/wait" \
  -H 'content-type: application/json' \
  -d '{"assistant_id":"planner","input":{"messages":[{"role":"user","content":"hi"}]}}' |
  grep -q '"routed"' || fail "planner's run did not return the routed reply (see $work/langgraph.log)"
# The upstream sees Nasiko's platform key, never the agent's own.
grep -qx 'Bearer sk-nasiko-platform' "$work/upstream.log" ||
  fail "the model call did not come through Nasiko (upstream saw: $(cat "$work/upstream.log"))"
echo "ok: planner's model call went through Nasiko"
expect_connected planner yes "after a routed call"

nasiko disconnect planner
cmp -s "$app/.env" "$work/env.before" || fail "disconnect did not restore $app/.env"
echo "ok: disconnect restored .env"
expect_connected planner no "after disconnect"

echo "PASS: agents registered, LLM routing connected and disconnected"
