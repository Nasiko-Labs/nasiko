#!/usr/bin/env bash
# End-to-end check of A2A discovery in `nasiko status`: start a real CrewAI agent and a
# real LangGraph server, run status, and expect all three agents in its A2A table.
#
# The first run installs both frameworks into virtualenvs under .work/ (needs
# network); later runs reuse them. Both agents run on free ports picked here.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
work="$here/.work"
python=${PYTHON:-python3}
mkdir -p "$work"

venv() { # venv <name> <packages...>
  [ -x "$work/venv-$1/bin/python" ] && return
  echo "Installing $1 into $work/venv-$1 ..."
  "$python" -m venv "$work/venv-$1"
  "$work/venv-$1/bin/pip" install -q "${@:2}"
}
venv crewai 'crewai[a2a]==1.15.28' uvicorn
venv langgraph 'langgraph-cli[inmem]==0.4.37'

free_port() {
  "$python" -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])'
}
crewai_port=$(free_port)
langgraph_port=$(free_port)

pids=()
trap 'kill "${pids[@]}" 2>/dev/null || true' EXIT

"$work/venv-crewai/bin/python" "$here/crewai_agent.py" "$crewai_port" > "$work/crewai.log" 2>&1 &
pids+=($!)
# Served from a copy, so the dev server's state stays out of the source tree.
rm -rf "$work/app" && cp -R "$here/langgraph" "$work/app"
(cd "$work/app" && LANGSMITH_TRACING=false exec "$work/venv-langgraph/bin/langgraph" \
  dev --no-browser --no-reload --port "$langgraph_port") > "$work/langgraph.log" 2>&1 &
pids+=($!)

wait_for() { # wait_for <name> <url>
  for _ in $(seq 120); do curl -sf --noproxy '*' -o /dev/null "$2" && return; sleep 1; done
  echo "FAIL: $1 did not start; see $work/$1.log" >&2
  tail -5 "$work/$1.log" >&2
  exit 1
}
echo "Waiting for the agents ..."
wait_for crewai "http://127.0.0.1:$crewai_port/.well-known/agent-card.json"
wait_for langgraph "http://127.0.0.1:$langgraph_port/info"

# `status` exits non-zero when no cluster is connected; the agent tables print regardless.
output=$(cargo run -q -p nasiko -- status || true)
echo "$output"

for agent in crew-researcher planner writer; do
  if ! grep -qE "^ *$agent " <<< "$output"; then
    echo "FAIL: $agent was not discovered (logs in $work)" >&2
    exit 1
  fi
done
echo "PASS: all agents discovered"
