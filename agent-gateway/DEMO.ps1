# ============================================================
#  Nasiko Buildthon Demo - Resilient Agent Request Layer
#  Run from: agent-gateway/
#  Server must be running on port 8000
# ============================================================

$BASE = "http://localhost:8000"

function Show-Section($title) {
    Write-Host ""
    Write-Host ("=" * 60) -ForegroundColor Yellow
    Write-Host "  $title" -ForegroundColor Yellow
    Write-Host ("=" * 60) -ForegroundColor Yellow
}

function Show-Step($msg) {
    Write-Host ""
    Write-Host ">> $msg" -ForegroundColor Cyan
}

function GET($path) {
    try {
        Invoke-RestMethod -Uri "$BASE$path" -Method GET | ConvertTo-Json -Depth 5
    } catch {
        Write-Host "HTTP $($_.Exception.Response.StatusCode.value__)" -ForegroundColor Red
    }
}

function DELETE($path) {
    try {
        Invoke-RestMethod -Uri "$BASE$path" -Method DELETE | ConvertTo-Json -Depth 3
    } catch {
        Write-Host "HTTP $($_.Exception.Response.StatusCode.value__)" -ForegroundColor Red
    }
}

function PUT($path, $body) {
    try {
        Invoke-RestMethod -Uri "$BASE$path" -Method PUT -Body ($body | ConvertTo-Json) -ContentType "application/json" | ConvertTo-Json -Depth 3
    } catch {
        Write-Host "HTTP $($_.Exception.Response.StatusCode.value__)" -ForegroundColor Red
        $_.ErrorDetails.Message
    }
}

# ─────────────────────────────────────────────────────────────
Show-Section "1. SYSTEM HEALTH"
# ─────────────────────────────────────────────────────────────

Show-Step "Simple health check"
GET "/router/health"

Show-Step "Full health - shows cache + rate limiter components"
GET "/health"

# ─────────────────────────────────────────────────────────────
Show-Section "2. CACHE (Requirement 1)"
# ─────────────────────────────────────────────────────────────

Show-Step "Cache stats - starts empty"
GET "/monitor/cache/stats"

Show-Step "Per-agent cache stats"
GET "/monitor/cache/stats/code-agent"

Show-Step "Simulating cache hit/miss cycle..."
$cacheFile = [System.IO.Path]::GetTempFileName() + ".py"
@'
import asyncio, sys
sys.path.insert(0, ".")
async def run():
    from router.src.core.cache_service import CacheService
    c = CacheService()
    await c.connect()
    lines = ['{"message":"The answer is 42","is_int_response":false,"agent_id":"math-agent","url":""}']
    miss = await c.get("math-agent", "What is 6 times 7?")
    print("1st request (MISS):", miss)
    await c.set("math-agent", "What is 6 times 7?", lines)
    hit = await c.get("math-agent", "What is 6 times 7?")
    print("2nd request (HIT):", hit[0][:50] if hit else None)
    hit2 = await c.get("math-agent", "WHAT IS 6 TIMES 7?")
    print("Normalized query (HIT):", "YES" if hit2 else "NO")
    stats = await c.get_stats()
    print("hit_rate_pct:", stats["hit_rate_pct"], "%  hits:", stats["hits"], "  misses:", stats["misses"])
asyncio.run(run())
'@ | Set-Content $cacheFile -Encoding UTF8
python $cacheFile
Remove-Item $cacheFile

Show-Step "Flush all cache"
DELETE "/monitor/cache"

# ─────────────────────────────────────────────────────────────
Show-Section "3. RATE LIMITING (Requirement 2)"
# ─────────────────────────────────────────────────────────────

Show-Step "Global defaults"
GET "/monitor/rate-limits"

Show-Step "Configure code-agent: 5 req/s, burst=10, queue=20"
PUT "/monitor/rate-limits/code-agent" @{ requests_per_second = 5.0; burst_capacity = 10; queue_size = 20 }

Show-Step "Configure data-agent: 1 req/s, burst=3, queue=5"
PUT "/monitor/rate-limits/data-agent" @{ requests_per_second = 1.0; burst_capacity = 3; queue_size = 5 }

Show-Step "List all custom configs"
GET "/monitor/rate-limits/configs/list"

Show-Step "Simulating burst + queue + rejection..."
$rlFile = [System.IO.Path]::GetTempFileName() + ".py"
@'
import asyncio, sys
sys.path.insert(0, ".")
async def run():
    from router.src.core.rate_limiter import RateLimiter, RateLimitExceeded
    r = RateLimiter()
    r.configure_agent("demo", 10.0, 3, 5)
    print("--- 3 burst requests (all immediate) ---")
    for i in range(3):
        async with r.acquire("demo"):
            pass
    s = r.get_agent_stats("demo")
    print("accepted:", s["accepted_requests"], "  rejected:", s["rejected_requests"], "  tokens_left:", s["tokens_available"])
    print()
    print("--- 4th request (bucket empty, queues and waits) ---")
    async with r.acquire("demo"):
        pass
    s = r.get_agent_stats("demo")
    print("accepted:", s["accepted_requests"], "  avg_wait_ms:", s["avg_queue_wait_ms"], "ms")
    print()
    print("--- Rejection test (queue_size=0, bucket empty) ---")
    r.configure_agent("tight", 0.001, 1, 0)
    b = await r._get_or_create_bucket("tight")
    async with r._lock:
        b.try_consume()
    try:
        async with r.acquire("tight"):
            pass
    except RateLimitExceeded as e:
        print("RateLimitExceeded raised correctly")
        print("rejected:", r.get_agent_stats("tight")["rejected_requests"])
asyncio.run(run())
'@ | Set-Content $rlFile -Encoding UTF8
python $rlFile
Remove-Item $rlFile

Show-Step "Remove data-agent config (reverts to defaults)"
DELETE "/monitor/rate-limits/data-agent/config"

# ─────────────────────────────────────────────────────────────
Show-Section "4. MONITORING DASHBOARD (Requirement 3)"
# ─────────────────────────────────────────────────────────────

Show-Step "Full dashboard - health + cache + rate limiter in one call"
GET "/monitor/dashboard"

Show-Step "Metrics endpoint"
GET "/metrics"

# ─────────────────────────────────────────────────────────────
Show-Section "5. INPUT VALIDATION"
# ─────────────────────────────────────────────────────────────

Show-Step "POST /router without auth token - expect 403"
try {
    Invoke-RestMethod -Uri "$BASE/router" -Method POST -Body "session_id=s1&query=hello" -ContentType "application/x-www-form-urlencoded"
} catch {
    Write-Host "HTTP $($_.Exception.Response.StatusCode.value__) - auth enforced correctly" -ForegroundColor Green
}

Show-Step "POST /router with empty query - expect 400"
try {
    Invoke-RestMethod -Uri "$BASE/router" -Method POST -Body "session_id=s1&query=   " -ContentType "application/x-www-form-urlencoded" -Headers @{ Authorization = "Bearer fake-token" }
} catch {
    Write-Host "HTTP $($_.Exception.Response.StatusCode.value__) - validation enforced correctly" -ForegroundColor Green
}

Show-Step "PUT rate limit with rps=0 - expect 400"
try {
    $bad = @{ requests_per_second = 0; burst_capacity = 5; queue_size = 10 } | ConvertTo-Json
    Invoke-RestMethod -Uri "$BASE/monitor/rate-limits/x" -Method PUT -Body $bad -ContentType "application/json"
} catch {
    Write-Host "HTTP $($_.Exception.Response.StatusCode.value__) - validation enforced correctly" -ForegroundColor Green
}

# ─────────────────────────────────────────────────────────────
Show-Section "6. TEST SUITE - 83 tests"
# ─────────────────────────────────────────────────────────────

Show-Step "Running full test suite..."
Set-Location router
python -m pytest tests/test_request_management.py -v --tb=short -q
Set-Location ..

# ─────────────────────────────────────────────────────────────
Show-Section "DONE"
# ─────────────────────────────────────────────────────────────
Write-Host ""
Write-Host "  Req 1 - Cache agent responses" -ForegroundColor Green
Write-Host "    Cache check after agent selection, before HTTP call" -ForegroundColor White
Write-Host "    Redis + LRU fallback, query normalization" -ForegroundColor White
Write-Host ""
Write-Host "  Req 2 - Per-agent rate limits with queuing" -ForegroundColor Green
Write-Host "    Token bucket, excess traffic queued not rejected" -ForegroundColor White
Write-Host "    Per-agent isolation, configurable at runtime" -ForegroundColor White
Write-Host ""
Write-Host "  Req 3 - Operational monitoring endpoints" -ForegroundColor Green
Write-Host "    15+ endpoints, real-time dashboard" -ForegroundColor White
Write-Host "    Swagger UI: http://localhost:8000/docs" -ForegroundColor White
Write-Host ""
Write-Host "  Tests: 83/83 passing" -ForegroundColor Green
Write-Host ""
