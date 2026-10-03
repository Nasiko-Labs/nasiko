#!/usr/bin/env bash
# Local tester for P1 (compact tool schemas) and P2 (request classifier).
# Not part of the PR — a convenience harness so you can see before/after yourself.
#
#   ./demo/test.sh            # run everything (offline evals + live app if up)
#   ./demo/test.sh offline    # only the offline side-by-side evals (no services)
#   ./demo/test.sh live       # only the live app checks (needs server + Postgres)
set -uo pipefail
cd "$(dirname "$0")/.."
# shellcheck disable=SC1090
source "$HOME/.cargo/env" 2>/dev/null || true
export PATH="/opt/homebrew/opt/postgresql@17/bin:$PATH"

MODE="${1:-all}"
P1_EVAL=eval-data/compact-tools-eval.json
P2_EVAL=eval-data/classifier-eval.json
OWNER=091ecc7c-b852-4b0d-a2d1-906d2015f258
GITHUB_AGENT=121cfb99-6894-4f94-af31-a4d13045f057   # pinned — used for P1
GENERAL_AGENT=19fb9132-8adf-4b14-8bf6-16dbea6fa340   # un-pinned — exercises P2 Level 3

bold() { printf '\n\033[1m== %s ==\033[0m\n' "$1"; }
note() { printf '   \033[2m%s\033[0m\n' "$1"; }

# ----------------------------------------------------------------------------
# P1 offline — same tools, native vs compact token count (deterministic, no net)
# ----------------------------------------------------------------------------
p1_offline() {
  bold "P1 offline — compact tool schemas (baseline → compact tokens)"
  EVAL_SET="$P1_EVAL" OUT=/tmp/p1_out.jsonl EMIT_TOKENS=1 \
    cargo run --release -q -p nasiko-llm-router --example compact_tools_eval || return 1
  python3 - <<'PY'
import json
base = comp = 0
print(f"   {'case':<30}{'baseline':>9}{'compact':>9}{'saved':>8}")
for line in open('/tmp/p1_out.jsonl'):
    o = json.loads(line)
    if 'baseline_tokens' not in o:
        continue
    b, c = o['baseline_tokens'], o['compact_tokens']
    base += b; comp += c
    print(f"   {o['id'][:30]:<30}{b:>9}{c:>9}{o.get('saved_pct', 0):>7.1f}%")
if base:
    print('   ' + '-' * 56)
    print(f"   {'TOTAL':<30}{base:>9}{comp:>9}{100*(1-comp/base):>7.1f}%")
PY
}

# ----------------------------------------------------------------------------
# P2 offline — regex (current router default) vs heuristic (ours), side by side
# ----------------------------------------------------------------------------
p2_offline() {
  bold "P2 offline — regex (current) vs heuristic (ours)"
  for B in regex heuristic; do
    EVAL_SET="$P2_EVAL" OUT="/tmp/p2_$B.jsonl" CLASSIFIER_BACKEND="$B" \
      cargo run --release -q -p nasiko-llm-router --example classifier_eval || return 1
  done
  python3 - <<'PY'
import json
load = lambda p: {json.loads(l)['id']: json.loads(l) for l in open(p)}
r, h = load('/tmp/p2_regex.jsonl'), load('/tmp/p2_heuristic.jsonl')
print(f"   {'id':<10}{'regex_type':<22}{'heuristic_type':<22}{'cplx':>5}{'conf':>7}")
for i in r:
    hi = h.get(i, {})
    print(f"   {i[:10]:<10}{r[i]['request_type']:<22}{hi.get('request_type',''):<22}"
          f"{hi.get('complexity',''):>5}{hi.get('confidence',0):>7.2f}")
PY
  note "regex = fixed complexity 3 / confidence 0.5; heuristic adds a real complexity + vote-margin confidence."
}

# ----------------------------------------------------------------------------
# Live helpers (real gateway: JWT + a seeded flow carrying the W3C traceparent)
# ----------------------------------------------------------------------------
mint() { AGENT_JWT_SECRET=dev-agent-secret-change-me \
  cargo run --release -q -p nasiko-llm-router --example mint_token -- "$1" "$OWNER" 2>/dev/null | tail -1; }

seed_flow() { # $1=trace $2=agent $3=metadata-json
  PGPASSWORD=nasiko psql -U nasiko -h localhost -d nasiko_dev -q -P pager=off \
    -c "INSERT INTO flows(flow_id,user_id,status,metadata,created_at) VALUES ('$1','$OWNER','running','$3',now());
        INSERT INTO flow_participants(flow_id,agent_id) VALUES ('$1','$2');" >/dev/null
}

# Build a live request (the fixture's tool set + a user message) from the P1 fixture.
build_p1_request() {
  python3 - "$P1_EVAL" <<'PY' > /tmp/p1_live_req.json
import json, sys
tools = json.load(open(sys.argv[1]))['tools']
json.dump({
    "model": "gpt-4o",
    "messages": [{"role": "user",
        "content": "Schedule a 30-minute design review on Monday at 3pm with riya@example.com, then email the team the agenda."}],
    "tools": tools,
}, open('/tmp/p1_live_req.json', 'w'))
PY
}

# POST the gateway with a fresh trace/flow each attempt; retry transient upstream 502s.
# Echoes the winning TRACE on success (empty on give-up). $1=agent $2=metadata $3=data-ref
post_gateway() {
  local agent="$1" meta="$2" data="$3" trace span jwt code
  jwt=$(mint "$agent")
  for _ in 1 2 3 4; do
    trace=$(openssl rand -hex 16); span=$(openssl rand -hex 8)
    seed_flow "$trace" "$agent" "$meta"
    code=$(curl -s -o /dev/null -w "%{http_code}" -X POST http://localhost:8080/v1/chat/completions \
      -H "Authorization: Bearer $jwt" -H "Content-Type: application/json" \
      -H "traceparent: 00-$trace-$span-01" --data "$data")
    if [ "$code" = "200" ]; then printf '   gateway HTTP 200\n' >&2; echo "$trace"; return 0; fi
    printf '   gateway HTTP %s (transient — retrying)\n' "$code" >&2; sleep 1
  done
  echo ""
}

p1_live() {
  bold "P1 live — real gateway call, savings written to token_usage.metadata.compact"
  build_p1_request
  local TRACE; TRACE=$(post_gateway "$GITHUB_AGENT" '{}' "@/tmp/p1_live_req.json")
  [ -z "$TRACE" ] && { note "upstream kept failing; skip (transient Bedrock 502)"; return; }
  sleep 1
  printf '   token_usage.metadata.compact ='
  PGPASSWORD=nasiko psql -U nasiko -h localhost -d nasiko_dev -qt -P pager=off \
    -c "SELECT metadata->'compact' FROM token_usage WHERE session_id='$TRACE' ORDER BY created_at DESC LIMIT 1;"
  note "saved_bytes>0 means the compact form of the tool schemas is smaller than native JSON."
}

p2_live() {
  bold "P2 live — heuristic classifier drives the Level-3 routing decision"
  local CTX; CTX="p2-$(openssl rand -hex 3)"   # unique per run, or a repeat conv_id hits the Level-2 cache
  local TRACE; TRACE=$(post_gateway "$GENERAL_AGENT" "{\"context_id\":\"$CTX\",\"mode\":\"continue\"}" \
    '{"model":"gpt-4o","messages":[{"role":"user","content":"Write a Python function that parses a CSV file and returns rows where the amount column exceeds a threshold, with error handling for malformed lines."}]}')
  [ -z "$TRACE" ] && { note "upstream kept failing; skip (transient Bedrock 502)"; return; }
  sleep 1
  # Proof from the DB (reliable; the server log is block-buffered under nohup). The agent's
  # configured model is openai.gpt-oss-20b — a different model here means Level-3 classification
  # ran and its request_type drove the tier → model override.
  echo "   token_usage (agent default = openai.gpt-oss-20b):"
  PGPASSWORD=nasiko psql -U nasiko -h localhost -d nasiko_dev -qt -P pager=off \
    -c "SELECT '     routed_model=' || model || '  input_tokens=' || input_tokens
         FROM token_usage WHERE session_id='$TRACE' ORDER BY created_at DESC LIMIT 1;"
  note "routed_model != openai.gpt-oss-20b ⇒ the classifier drove the tier. (Backend 'heuristic' vs 'regex' is shown conclusively in the P2 offline table above / server log.)"
}

live_checks() {
  if ! curl -s -o /dev/null http://localhost:8080/health; then
    note "server:8080 not up — skipping live checks (run: cd server && cargo run -p nasiko-server)"
    return
  fi
  if ! PGPASSWORD=nasiko psql -U nasiko -h localhost -d nasiko_dev -qtc "SELECT 1" >/dev/null 2>&1; then
    note "Postgres not reachable — skipping live checks"
    return
  fi
  p1_live
  p2_live
}

case "$MODE" in
  offline) p1_offline; p2_offline ;;
  live)    live_checks ;;
  all)     p1_offline; p2_offline; live_checks ;;
  *) echo "usage: $0 [offline|live|all]"; exit 2 ;;
esac

bold "done"
note "Flags live in server/.env: TOKEN_COMPACT_TOOLS (P1), CLASSIFIER_HEURISTIC (P2). Flip one, restart the server, re-run './demo/test.sh live' to see before/after."
