#!/usr/bin/env bash
# B7 smoke test — requires metrics dev server + Nasiko stack + Bearer token.
set -euo pipefail

BASE="${METRICS_BASE_URL:-http://localhost:3003}"
TOKEN="${NASIKO_BEARER_TOKEN:-}"

if [[ -z "$TOKEN" ]]; then
  echo "Set NASIKO_BEARER_TOKEN (JWT from nasiko-credentials-*.json access_token)" >&2
  exit 1
fi

AUTH=(-H "Authorization: Bearer ${TOKEN}")

echo "== GET /api/health =="
curl -sf "${BASE}/api/health" | head -c 200
echo -e "\n"

echo "== GET /api/agents =="
AGENTS_JSON=$(curl -sf "${BASE}/api/agents" "${AUTH[@]}")
echo "$AGENTS_JSON" | head -c 300
echo -e "\n"

AGENT_ID=$(echo "$AGENTS_JSON" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d['agents'][0]['agentId'] if d.get('agents') else '')" 2>/dev/null || true)
if [[ -z "$AGENT_ID" ]]; then
  echo "No agents in list — deploy an agent first" >&2
  exit 1
fi

echo "== GET /api/metrics?agent=${AGENT_ID}&window=24h =="
METRICS_JSON=$(curl -sf "${BASE}/api/metrics?agent=${AGENT_ID}&window=24h" "${AUTH[@]}")
echo "$METRICS_JSON" | python3 -m json.tool 2>/dev/null | head -40

python3 - <<'PY' "$METRICS_JSON"
import json, sys
m = json.loads(sys.argv[1])
required = ["agent", "window", "avg_latency_ms", "success", "error", "uptime_pct", "series_24h"]
for k in required:
    assert k in m, f"missing key: {k}"
assert len(m["series_24h"]) == 24, "series_24h must have 24 buckets"
assert m["uptime_pct"] is not None, "uptime_pct should be set after B6"
print("OK — metrics contract validated")
PY

echo "Smoke test passed."
