#!/usr/bin/env bash
set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PASS=0
FAIL=0
WARN=0
declare -A RESULTS

check() {
    local name="$1" status="$2" detail="$3"
    case "$status" in
        PASS) echo -e "\n\033[32m[PASS]\033[0m $name"; ((PASS++)) ;;
        FAIL) echo -e "\n\033[31m[FAIL]\033[0m $name"; ((FAIL++)) ;;
        WARN) echo -e "\n\033[33m[WARN]\033[0m $name"; ((WARN++)) ;;
    esac
    [ -n "$detail" ] && echo "  $detail"
    RESULTS["$name"]="$status: $detail"
}

echo "======================================================================"
echo "  NASIKO RESILIENT REQUEST LAYER - PHASE 1 VERIFICATION"
echo "  $(date -Iseconds)"
echo "======================================================================"

# ============================================================================
# CHECK 1: All services healthy
# ============================================================================
echo ""
echo "======================================================================"
echo "  CHECK 1: All services healthy"
echo "======================================================================"

echo "--- Docker Compose Status ---"
docker compose -f "$PROJECT_ROOT/docker-compose.local.yml" --env-file "$PROJECT_ROOT/.nasiko-local.env" ps

echo ""
echo "--- Kong Health ---"
if KONG_HEALTH=$(curl -sf http://localhost:9100/health 2>&1); then
    echo "$KONG_HEALTH" | python3 -m json.tool 2>/dev/null || echo "$KONG_HEALTH"
    check "Kong Health" "PASS" "Kong gateway is healthy"
else
    check "Kong Health" "FAIL" "Could not reach Kong at localhost:9100"
fi

echo ""
echo "--- Router Health ---"
if ROUTER_HEALTH=$(curl -sf http://localhost:8081/router/health 2>&1); then
    echo "$ROUTER_HEALTH" | python3 -m json.tool 2>/dev/null || echo "$ROUTER_HEALTH"
    check "Router Health" "PASS" "Router service is healthy"
elif ROUTER_HEALTH=$(curl -sf http://localhost:8081/health 2>&1); then
    echo "$ROUTER_HEALTH" | python3 -m json.tool 2>/dev/null || echo "$ROUTER_HEALTH"
    check "Router Health" "PASS" "Router service is healthy (direct endpoint)"
else
    check "Router Health" "FAIL" "Could not reach Router at localhost:8081"
fi

# ============================================================================
# CHECK 2: Prometheus metrics are real
# ============================================================================
echo ""
echo "======================================================================"
echo "  CHECK 2: Prometheus metrics are real"
echo "======================================================================"

METRICS=$(curl -sf http://localhost:8081/metrics 2>&1) || METRICS=""
echo "${METRICS:0:2000}"

MISSING=""
for m in gateway_cache_hits_total gateway_cache_misses_total gateway_cache_hit_ratio gateway_queue_depth gateway_adaptive_limit_current; do
    if ! echo "$METRICS" | grep -q "$m"; then
        MISSING="$MISSING $m"
    fi
done

if [ -z "$MISSING" ]; then
    check "Prometheus Metrics" "PASS" "All 5 expected metrics found"
else
    check "Prometheus Metrics" "FAIL" "Missing metrics:$MISSING"
fi

# ============================================================================
# CHECK 3: Admin stats endpoint
# ============================================================================
echo ""
echo "======================================================================"
echo "  CHECK 3: Admin stats endpoint"
echo "======================================================================"

ADMIN_STATS=$(curl -sf -H "X-Admin-API-Key: local-admin-key" http://localhost:8081/admin/stats/runtime 2>&1) || ADMIN_STATS=""
echo "$ADMIN_STATS" | python3 -m json.tool 2>/dev/null || echo "$ADMIN_STATS"

MISSING_KEYS=""
for k in cache_hits_total cache_misses_total cache_hit_ratio errors_total per_agent; do
    if ! echo "$ADMIN_STATS" | python3 -c "import sys,json; d=json.load(sys.stdin); assert '$k' in d" 2>/dev/null; then
        MISSING_KEYS="$MISSING_KEYS $k"
    fi
done

if [ -z "$MISSING_KEYS" ]; then
    check "Admin Stats" "PASS" "All expected keys present"
else
    check "Admin Stats" "FAIL" "Missing keys:$MISSING_KEYS"
fi

# ============================================================================
# CHECK 4: CACHE BENCHMARK
# ============================================================================
echo ""
echo "======================================================================"
echo "  CHECK 4: CACHE BENCHMARK (most important)"
echo "======================================================================"

echo "--- Step A: First request (cold) ---"
START=$(date +%s%N)
curl -sf "http://localhost:9100/router/route?query=translate+hello+to+French" > /tmp/resp1.json 2>/dev/null || true
END=$(date +%s%N)
FIRST_MS=$(( (END - START) / 1000000 ))
echo "First call: ${FIRST_MS}ms"
cat /tmp/resp1.json 2>/dev/null | head -c 500
echo ""

echo "--- Step B: Second request (should be cached) ---"
START=$(date +%s%N)
curl -sf "http://localhost:9100/router/route?query=translate+hello+to+French" > /tmp/resp2.json 2>/dev/null || true
END=$(date +%s%N)
SECOND_MS=$(( (END - START) / 1000000 ))
echo "Second call (should be cached): ${SECOND_MS}ms"
cat /tmp/resp2.json 2>/dev/null | head -c 500
echo ""

echo "--- Step C: Speedup calculation ---"
if [ "$SECOND_MS" -gt 0 ] && [ "$FIRST_MS" -gt 0 ]; then
    python3 -c "
first=$FIRST_MS
second=$SECOND_MS
if second > 0:
    ratio = first / second
    print(f'First call:  {first}ms')
    print(f'Second call: {second}ms')
    print(f'Speedup:     {ratio:.1f}x faster on cache hit')
    if ratio >= 3:
        print('PASS: meets 3x minimum threshold')
    else:
        print('FAIL: below 3x threshold')
"
    RATIO=$(python3 -c "print(round($FIRST_MS / $SECOND_MS, 1))")
    if python3 -c "exit(0 if $FIRST_MS / $SECOND_MS >= 3 else 1)"; then
        check "Cache Speedup" "PASS" "Meets 3x minimum threshold (${RATIO}x)"
    else
        check "Cache Speedup" "FAIL" "Below 3x threshold (${RATIO}x)"
    fi
else
    check "Cache Speedup" "FAIL" "Could not calculate (first=${FIRST_MS}ms, second=${SECOND_MS}ms)"
fi

echo "--- Step D: Footer verification ---"
grep -o "Request layer:.*" /tmp/resp1.json 2>/dev/null || echo "WARNING: footer not found in response 1"
grep -o "Request layer:.*" /tmp/resp2.json 2>/dev/null || echo "WARNING: footer not found in response 2"

echo "--- Step E: Cache stats verification ---"
curl -sf -H "X-Admin-API-Key: local-admin-key" \
  http://localhost:8081/admin/stats/runtime | python3 -c "
import sys, json
d = json.load(sys.stdin)
print(f'Cache hits:   {d.get(\"cache_hits_total\", 0)}')
print(f'Cache misses: {d.get(\"cache_misses_total\", 0)}')
ratio = d.get('cache_hit_ratio', 0)
print(f'Hit ratio:    {ratio:.1%}')
if d.get('cache_hits_total', 0) >= 1:
    print('PASS: cache recorded a hit')
else:
    print('FAIL: no cache hits recorded')
" 2>/dev/null

CACHE_HITS=$(curl -sf -H "X-Admin-API-Key: local-admin-key" http://localhost:8081/admin/stats/runtime | python3 -c "import sys,json;print(json.load(sys.stdin).get('cache_hits_total',0))" 2>/dev/null || echo "0")
if [ "$CACHE_HITS" -ge 1 ] 2>/dev/null; then
    check "Cache Hit Recorded" "PASS" "Cache recorded $CACHE_HITS hit(s)"
else
    check "Cache Hit Recorded" "FAIL" "No cache hits recorded"
fi

# ============================================================================
# CHECK 5: SEMANTIC CACHE BENCHMARK
# ============================================================================
echo ""
echo "======================================================================"
echo "  CHECK 5: SEMANTIC CACHE BENCHMARK"
echo "======================================================================"

echo "--- Step A: Priming cache ---"
curl -sf "http://localhost:9100/router/route?query=translate+hello+to+French" > /dev/null 2>&1 || true
echo "Cache primed"

echo "--- Step B: Paraphrased query ---"
START=$(date +%s%N)
curl -sf "http://localhost:9100/router/route?query=translate+hi+to+French" > /tmp/semantic.json 2>/dev/null || true
END=$(date +%s%N)
SEM_MS=$(( (END - START) / 1000000 ))
echo "Semantic query: ${SEM_MS}ms"
cat /tmp/semantic.json 2>/dev/null | head -c 500
echo ""

echo "--- Step C: Semantic hit check ---"
if grep -qo "semantic cache hit" /tmp/semantic.json 2>/dev/null; then
    check "Semantic Cache" "PASS" "Semantic cache hit detected"
else
    check "Semantic Cache" "WARN" "No semantic cache hit - check if SEMANTIC_CACHE_ENABLED=true in env"
fi

echo "--- Step D: Full stats ---"
curl -sf -H "X-Admin-API-Key: local-admin-key" \
  http://localhost:8081/admin/stats/runtime | python3 -c "
import sys, json
d = json.load(sys.stdin)
print(json.dumps(d, indent=2))
" 2>/dev/null || echo "Could not fetch stats"

# ============================================================================
# CHECK 6: RATE LIMIT + QUEUE BENCHMARK
# ============================================================================
echo ""
echo "======================================================================"
echo "  CHECK 6: RATE LIMIT + QUEUE BENCHMARK"
echo "======================================================================"

echo "--- Step A: Set low rate limit ---"
curl -sf -X PUT http://localhost:8081/admin/limits/a2a-translator \
  -H "Content-Type: application/json" \
  -d '{"rpm": 2, "queue_depth": 10, "queue_timeout_seconds": 30}' || true
echo "Rate limit set to 2 RPM"

echo "--- Step B: Fire 5 concurrent requests ---"
python3 << 'PYEOF'
import subprocess, time, json, threading

results = []
def fire(i):
    start = time.time()
    r = subprocess.run(
        ['curl', '-sf', f'http://localhost:9100/router/route?query=translate+word{i}+to+French'],
        capture_output=True, text=True, timeout=60
    )
    ms = (time.time() - start) * 1000
    results.append({'req': i, 'ms': round(ms), 'response': r.stdout[:200]})

threads = [threading.Thread(target=fire, args=(i,)) for i in range(5)]
[t.start() for t in threads]
[t.join() for t in threads]

for r in sorted(results, key=lambda x: x['req']):
    print(f"  Request {r['req']}: {r['ms']}ms")

queued = [r for r in results if r['ms'] > 1000]
fast = [r for r in results if r['ms'] <= 1000]
print(f"\nFast (likely immediate): {len(fast)}")
print(f"Slow (likely queued):    {len(queued)}")
if queued:
    print("PASS: queue is absorbing excess traffic")
else:
    print("INFO: all requests fast — may need lower RPM limit")
PYEOF

echo "--- Step C: Queue stats ---"
curl -sf -H "X-Admin-API-Key: local-admin-key" \
  http://localhost:8081/admin/stats/runtime | python3 -c "
import sys, json
d = json.load(sys.stdin)
agents = d.get('per_agent', d.get('rate_limits', {}))
for agent, stats in agents.items():
    print(f'  {agent}: queued={stats.get(\"queued\",0)}, rejected={stats.get(\"rejected\",0)}, queue_depth={stats.get(\"queue_depth\",0)}')
" 2>/dev/null || echo "Could not fetch queue stats"

echo "--- Step D: Reset rate limit ---"
curl -sf -X PUT http://localhost:8081/admin/limits/a2a-translator \
  -H "Content-Type: application/json" \
  -d '{"rpm": 10, "queue_depth": 50, "queue_timeout_seconds": 30}' || true
echo "Rate limit restored to 10 RPM"

# ============================================================================
# CHECK 7: ADAPTIVE RATE LIMIT VERIFICATION
# ============================================================================
echo ""
echo "======================================================================"
echo "  CHECK 7: ADAPTIVE RATE LIMIT VERIFICATION"
echo "======================================================================"

ADAPTIVE_LOGS=$(docker compose -f "$PROJECT_ROOT/docker-compose.local.yml" --env-file "$PROJECT_ROOT/.nasiko-local.env" logs nasiko-router 2>&1 | grep -iE "adaptive|limit adjusted|rpm" | tail -20)
if [ -n "$ADAPTIVE_LOGS" ]; then
    echo "$ADAPTIVE_LOGS"
    check "Adaptive Rate Limit" "PASS" "Found adaptive rate limit log entries"
else
    echo "  No adaptive rate limit logs found yet"
    SCHED_LOGS=$(docker compose -f "$PROJECT_ROOT/docker-compose.local.yml" --env-file "$PROJECT_ROOT/.nasiko-local.env" logs nasiko-router 2>&1 | grep -i "adaptive rate-limit loop started" | tail -5)
    if [ -n "$SCHED_LOGS" ]; then
        echo "$SCHED_LOGS"
        check "Adaptive Rate Limit" "WARN" "Loop started but no adaptation events yet (runs every 60s)"
    else
        check "Adaptive Rate Limit" "WARN" "No adaptive logs found - feature may not be enabled"
    fi
fi

# ============================================================================
# CHECK 8: HTTP CACHE HEADERS
# ============================================================================
echo ""
echo "======================================================================"
echo "  CHECK 8: HTTP CACHE HEADERS"
echo "======================================================================"

HEADERS=$(curl -v "http://localhost:9100/router/route?query=translate+hello+to+French" 2>&1 | grep -E "X-Cache|X-Agent-Latency|X-Cache-Age|< HTTP")
echo "$HEADERS"

MISSING_H=""
echo "$HEADERS" | grep -q "X-Cache" || MISSING_H="$MISSING_H X-Cache"
echo "$HEADERS" | grep -q "X-Agent-Latency" || MISSING_H="$MISSING_H X-Agent-Latency"

if [ -z "$MISSING_H" ]; then
    check "HTTP Cache Headers" "PASS" "X-Cache and X-Agent-Latency headers present"
else
    if echo "$HEADERS" | grep -qE "X-Cache|X-Agent-Latency"; then
        check "HTTP Cache Headers" "WARN" "Missing headers:$MISSING_H"
    else
        check "HTTP Cache Headers" "FAIL" "No cache headers found in response"
    fi
fi

# ============================================================================
# CHECK 9: Save benchmark results
# ============================================================================
echo ""
echo "======================================================================"
echo "  CHECK 9: Saving benchmark results"
echo "======================================================================"

python3 << PYEOF
import json, time, subprocess, os

results = {
    "timestamp": time.strftime("%Y-%m-%dT%H:%M:%SZ"),
    "benchmark_type": "phase1_verification",
    "checks": {}
}

r = subprocess.run(
    ['curl', '-sf', '-H', 'X-Admin-API-Key: local-admin-key',
     'http://localhost:8081/admin/stats/runtime'],
    capture_output=True, text=True
)
try:
    results["runtime_stats"] = json.loads(r.stdout)
except:
    results["runtime_stats"] = {"error": r.stdout}

r2 = subprocess.run(['curl', '-sf', 'http://localhost:8081/metrics'], capture_output=True, text=True)
results["prometheus_metrics_sample"] = r2.stdout[:500]

os.makedirs("$PROJECT_ROOT/docs/buildthon-demo-assets", exist_ok=True)
with open("$PROJECT_ROOT/docs/buildthon-demo-assets/latest-benchmark.json", "w") as f:
    json.dump(results, f, indent=2)

print("Benchmark saved to docs/buildthon-demo-assets/latest-benchmark.json")
print(json.dumps(results.get("runtime_stats", {}), indent=2))
PYEOF

# ============================================================================
# FINAL SUMMARY
# ============================================================================
echo ""
echo "======================================================================"
echo "  FINAL SUMMARY"
echo "======================================================================"
echo ""
echo "  Check                       | Status"
echo "  ----------------------------+--------"
for key in "${!RESULTS[@]}"; do
    printf "  %-28s| %s\n" "$key" "${RESULTS[$key]%%:*}"
done

TOTAL=$((PASS + FAIL + WARN))
echo ""
echo "  Total: $TOTAL  |  PASS: $PASS  |  FAIL: $FAIL  |  WARN: $WARN"

if [ "$FAIL" -eq 0 ]; then
    echo -e "\n  \033[32mALL CHECKS PASSED!\033[0m"
else
    echo -e "\n  \033[31m$FAIL CHECK(S) FAILED - see details above\033[0m"
fi
echo "======================================================================"
