<#
.SYNOPSIS
    Phase 1 Verification & Benchmark for Nasiko Resilient Request Layer
.DESCRIPTION
    Runs 9 checks in order: service health, Prometheus metrics, admin stats,
    cache benchmark, semantic cache, rate limit + queue, adaptive limits,
    HTTP cache headers, and saves results.
#>

$ErrorActionPreference = "Continue"
$ProjectRoot = Split-Path -Parent $PSScriptRoot

$results = @{}
$passCount = 0
$failCount = 0
$warnCount = 0

function Write-Check {
    param([string]$Name, [string]$Status, [string]$Detail)
    $color = switch ($Status) {
        "PASS" { "Green" }
        "FAIL" { "Red" }
        "WARN" { "Yellow" }
        default { "White" }
    }
    Write-Host "`n[$Status] $Name" -ForegroundColor $color
    if ($Detail) { Write-Host "  $Detail" -ForegroundColor Gray }
    $script:results[$Name] = @{ status = $Status; detail = $Detail }
    switch ($Status) {
        "PASS" { $script:passCount++ }
        "FAIL" { $script:failCount++ }
        "WARN" { $script:warnCount++ }
    }
}

Write-Host "=" * 70
Write-Host "  NASIKO RESILIENT REQUEST LAYER - PHASE 1 VERIFICATION" -ForegroundColor Cyan
Write-Host "  $(Get-Date -Format 'yyyy-MM-ddTHH:mm:ssK')"
Write-Host "=" * 70

# ============================================================================
# CHECK 1: All services healthy
# ============================================================================
Write-Host "`n" + "=" * 70
Write-Host "  CHECK 1: All services healthy" -ForegroundColor Yellow
Write-Host "=" * 70

Write-Host "`n--- Docker Compose Status ---"
docker compose -f "$ProjectRoot\docker-compose.local.yml" --env-file "$ProjectRoot\.nasiko-local.env" ps

Write-Host "`n--- Kong Health ---"
try {
    $kongHealth = Invoke-RestMethod -Uri "http://localhost:9100/health" -ErrorAction Stop
    Write-Host ($kongHealth | ConvertTo-Json -Depth 5)
    Write-Check -Name "Kong Health" -Status "PASS" -Detail "Kong gateway is healthy"
} catch {
    Write-Check -Name "Kong Health" -Status "FAIL" -Detail $_.Exception.Message
}

Write-Host "`n--- Router Health ---"
try {
    $routerHealth = Invoke-RestMethod -Uri "http://localhost:8081/router/health" -ErrorAction Stop
    Write-Host ($routerHealth | ConvertTo-Json -Depth 5)
    Write-Check -Name "Router Health" -Status "PASS" -Detail "Router service is healthy"
} catch {
    try {
        $routerHealth = Invoke-RestMethod -Uri "http://localhost:8081/health" -ErrorAction Stop
        Write-Host ($routerHealth | ConvertTo-Json -Depth 5)
        Write-Check -Name "Router Health" -Status "PASS" -Detail "Router service is healthy (direct endpoint)"
    } catch {
        Write-Check -Name "Router Health" -Status "FAIL" -Detail $_.Exception.Message
    }
}

# ============================================================================
# CHECK 2: Prometheus metrics are real
# ============================================================================
Write-Host "`n" + "=" * 70
Write-Host "  CHECK 2: Prometheus metrics are real" -ForegroundColor Yellow
Write-Host "=" * 70

try {
    $metrics = Invoke-WebRequest -Uri "http://localhost:8081/metrics" -ErrorAction Stop
    $metricsText = $metrics.Content
    Write-Host $metricsText.Substring(0, [Math]::Min(2000, $metricsText.Length))

    $expectedMetrics = @(
        "gateway_cache_hits_total",
        "gateway_cache_misses_total",
        "gateway_cache_hit_ratio",
        "gateway_queue_depth",
        "gateway_adaptive_limit_current"
    )

    $missing = @()
    foreach ($m in $expectedMetrics) {
        if ($metricsText -notmatch $m) {
            $missing += $m
        }
    }

    if ($missing.Count -eq 0) {
        Write-Check -Name "Prometheus Metrics" -Status "PASS" -Detail "All 5 expected metrics found"
    } else {
        Write-Check -Name "Prometheus Metrics" -Status "FAIL" -Detail "Missing metrics: $($missing -join ', ')"
    }
} catch {
    Write-Check -Name "Prometheus Metrics" -Status "FAIL" -Detail $_.Exception.Message
}

# ============================================================================
# CHECK 3: Admin stats endpoint
# ============================================================================
Write-Host "`n" + "=" * 70
Write-Host "  CHECK 3: Admin stats endpoint" -ForegroundColor Yellow
Write-Host "=" * 70

try {
    $headers = @{ "X-Admin-API-Key" = "local-admin-key" }
    $adminStats = Invoke-RestMethod -Uri "http://localhost:8081/admin/stats/runtime" -Headers $headers -ErrorAction Stop
    Write-Host ($adminStats | ConvertTo-Json -Depth 5)

    $expectedKeys = @("cache_hits_total", "cache_misses_total", "cache_hit_ratio", "errors_total")
    $missingKeys = @()
    foreach ($k in $expectedKeys) {
        if (-not ($adminStats.PSObject.Properties.Name -contains $k)) {
            $missingKeys += $k
        }
    }

    $hasPerAgent = ($adminStats.PSObject.Properties.Name -contains "per_agent")
    if (-not $hasPerAgent) { $missingKeys += "per_agent" }

    if ($missingKeys.Count -eq 0) {
        Write-Check -Name "Admin Stats" -Status "PASS" -Detail "All expected keys present"
    } else {
        Write-Check -Name "Admin Stats" -Status "FAIL" -Detail "Missing keys: $($missingKeys -join ', ')"
    }
} catch {
    Write-Check -Name "Admin Stats" -Status "FAIL" -Detail $_.Exception.Message
}

# ============================================================================
# CHECK 4: CACHE BENCHMARK
# ============================================================================
Write-Host "`n" + "=" * 70
Write-Host "  CHECK 4: CACHE BENCHMARK (most important)" -ForegroundColor Yellow
Write-Host "=" * 70

Write-Host "`n--- Step A: First request (cold) ---"
$sw1 = [System.Diagnostics.Stopwatch]::StartNew()
try {
    $resp1 = Invoke-WebRequest -Uri "http://localhost:9100/router/route?query=translate+hello+to+French" -ErrorAction Stop
    $sw1.Stop()
    $firstMs = $sw1.ElapsedMilliseconds
    $resp1Text = $resp1.Content
    Write-Host "First call: ${firstMs}ms"
    Write-Host $resp1Text.Substring(0, [Math]::Min(500, $resp1Text.Length))
    $resp1Text | Out-File -FilePath "$ProjectRoot\scripts\resp1.json" -Encoding utf8
} catch {
    $sw1.Stop()
    $firstMs = $sw1.ElapsedMilliseconds
    Write-Host "First call FAILED after ${firstMs}ms: $($_.Exception.Message)" -ForegroundColor Red
    $resp1Text = ""
}

Write-Host "`n--- Step B: Second request (should be cached) ---"
$sw2 = [System.Diagnostics.Stopwatch]::StartNew()
try {
    $resp2 = Invoke-WebRequest -Uri "http://localhost:9100/router/route?query=translate+hello+to+French" -ErrorAction Stop
    $sw2.Stop()
    $secondMs = $sw2.ElapsedMilliseconds
    $resp2Text = $resp2.Content
    Write-Host "Second call (should be cached): ${secondMs}ms"
    Write-Host $resp2Text.Substring(0, [Math]::Min(500, $resp2Text.Length))
    $resp2Text | Out-File -FilePath "$ProjectRoot\scripts\resp2.json" -Encoding utf8
} catch {
    $sw2.Stop()
    $secondMs = $sw2.ElapsedMilliseconds
    Write-Host "Second call FAILED after ${secondMs}ms: $($_.Exception.Message)" -ForegroundColor Red
    $resp2Text = ""
}

Write-Host "`n--- Step C: Speedup calculation ---"
if ($secondMs -gt 0 -and $firstMs -gt 0) {
    $ratio = [math]::Round($firstMs / $secondMs, 1)
    Write-Host "First call:  ${firstMs}ms"
    Write-Host "Second call: ${secondMs}ms"
    Write-Host "Speedup:     ${ratio}x faster on cache hit"

    if ($ratio -ge 3) {
        Write-Check -Name "Cache Speedup" -Status "PASS" -Detail "Meets 3x minimum threshold (${ratio}x)"
    } else {
        Write-Check -Name "Cache Speedup" -Status "FAIL" -Detail "Below 3x threshold (${ratio}x) - check cache is working"
    }
} else {
    Write-Check -Name "Cache Speedup" -Status "FAIL" -Detail "Could not calculate speedup (firstMs=$firstMs, secondMs=$secondMs)"
}

Write-Host "`n--- Step D: Footer verification ---"
if ($resp1Text -match "Request layer:(.*)") {
    Write-Host "Response 1 footer: $($Matches[0])"
} else {
    Write-Host "WARNING: footer not found in response 1" -ForegroundColor Yellow
}

if ($resp2Text -match "Request layer:(.*)") {
    Write-Host "Response 2 footer: $($Matches[0])"
} else {
    Write-Host "WARNING: footer not found in response 2" -ForegroundColor Yellow
}

Write-Host "`n--- Step E: Cache stats verification ---"
try {
    $headers = @{ "X-Admin-API-Key" = "local-admin-key" }
    $cacheStats = Invoke-RestMethod -Uri "http://localhost:8081/admin/stats/runtime" -Headers $headers -ErrorAction Stop
    $cacheHits = if ($cacheStats.cache_hits_total) { $cacheStats.cache_hits_total } else { 0 }
    $cacheMisses = if ($cacheStats.cache_misses_total) { $cacheStats.cache_misses_total } else { 0 }
    $cacheRatio = if ($cacheStats.cache_hit_ratio) { $cacheStats.cache_hit_ratio } else { 0 }

    Write-Host "Cache hits:   $cacheHits"
    Write-Host "Cache misses: $cacheMisses"
    Write-Host "Hit ratio:    $([math]::Round($cacheRatio * 100, 1))%"

    if ($cacheHits -ge 1) {
        Write-Check -Name "Cache Hit Recorded" -Status "PASS" -Detail "Cache recorded $cacheHits hit(s)"
    } else {
        Write-Check -Name "Cache Hit Recorded" -Status "FAIL" -Detail "No cache hits recorded"
    }
} catch {
    Write-Check -Name "Cache Hit Recorded" -Status "FAIL" -Detail $_.Exception.Message
}

# ============================================================================
# CHECK 5: SEMANTIC CACHE BENCHMARK
# ============================================================================
Write-Host "`n" + "=" * 70
Write-Host "  CHECK 5: SEMANTIC CACHE BENCHMARK" -ForegroundColor Yellow
Write-Host "=" * 70

Write-Host "`n--- Step A: Priming cache ---"
try {
    Invoke-WebRequest -Uri "http://localhost:9100/router/route?query=translate+hello+to+French" -ErrorAction Stop | Out-Null
    Write-Host "Cache primed"
} catch {
    Write-Host "Cache prime failed: $($_.Exception.Message)" -ForegroundColor Red
}

Write-Host "`n--- Step B: Paraphrased query ---"
$swSem = [System.Diagnostics.Stopwatch]::StartNew()
try {
    $semResp = Invoke-WebRequest -Uri "http://localhost:9100/router/route?query=translate+hi+to+French" -ErrorAction Stop
    $swSem.Stop()
    $semMs = $swSem.ElapsedMilliseconds
    $semText = $semResp.Content
    Write-Host "Semantic query: ${semMs}ms"
    Write-Host $semText.Substring(0, [Math]::Min(500, $semText.Length))
    $semText | Out-File -FilePath "$ProjectRoot\scripts\semantic.json" -Encoding utf8
} catch {
    $swSem.Stop()
    $semMs = $swSem.ElapsedMilliseconds
    Write-Host "Semantic query FAILED after ${semMs}ms: $($_.Exception.Message)" -ForegroundColor Red
    $semText = ""
}

Write-Host "`n--- Step C: Semantic hit check ---"
if ($semText -match "semantic cache hit") {
    Write-Check -Name "Semantic Cache" -Status "PASS" -Detail "Semantic cache hit detected"
} else {
    Write-Check -Name "Semantic Cache" -Status "WARN" -Detail "No semantic cache hit - check if SEMANTIC_CACHE_ENABLED=true in env"
}

Write-Host "`n--- Step D: Full stats ---"
try {
    $headers = @{ "X-Admin-API-Key" = "local-admin-key" }
    $semStats = Invoke-RestMethod -Uri "http://localhost:8081/admin/stats/runtime" -Headers $headers -ErrorAction Stop
    Write-Host ($semStats | ConvertTo-Json -Depth 5)
} catch {
    Write-Host "Could not fetch stats: $($_.Exception.Message)" -ForegroundColor Red
}

# ============================================================================
# CHECK 6: RATE LIMIT + QUEUE BENCHMARK
# ============================================================================
Write-Host "`n" + "=" * 70
Write-Host "  CHECK 6: RATE LIMIT + QUEUE BENCHMARK" -ForegroundColor Yellow
Write-Host "=" * 70

Write-Host "`n--- Step A: Set low rate limit ---"
try {
    $body = '{"rpm": 2, "queue_depth": 10, "queue_timeout_seconds": 30}'
    $resp = Invoke-RestMethod -Uri "http://localhost:8081/admin/limits/a2a-translator" -Method Put -ContentType "application/json" -Body $body -ErrorAction Stop
    Write-Host "Rate limit set to 2 RPM"
    Write-Host ($resp | ConvertTo-Json -Depth 3)
} catch {
    Write-Host "Failed to set rate limit: $($_.Exception.Message)" -ForegroundColor Red
}

Write-Host "`n--- Step B: Fire 5 concurrent requests ---"
$jobs = @()
for ($i = 0; $i -lt 5; $i++) {
    $jobs += Start-Job -ScriptBlock {
        param($idx)
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        try {
            $r = Invoke-WebRequest -Uri "http://localhost:9100/router/route?query=translate+word${idx}+to+French" -TimeoutSec 60 -ErrorAction Stop
            $sw.Stop()
            @{ req = $idx; ms = $sw.ElapsedMilliseconds; status = "OK"; response = $r.Content.Substring(0, [Math]::Min(200, $r.Content.Length)) }
        } catch {
            $sw.Stop()
            @{ req = $idx; ms = $sw.ElapsedMilliseconds; status = "ERROR"; response = $_.Exception.Message }
        }
    } -ArgumentList $i
}

$jobResults = $jobs | Wait-Job -Timeout 120 | Receive-Job
$jobs | Remove-Job -Force

$fast = @($jobResults | Where-Object { $_.ms -le 1000 })
$slow = @($jobResults | Where-Object { $_.ms -gt 1000 })

foreach ($r in ($jobResults | Sort-Object { $_.req })) {
    Write-Host "  Request $($r.req): $($r.ms)ms [$($r.status)]"
}

Write-Host "`nFast (likely immediate): $($fast.Count)"
Write-Host "Slow (likely queued):    $($slow.Count)"

if ($slow.Count -gt 0) {
    Write-Check -Name "Queue Absorption" -Status "PASS" -Detail "Queue absorbed $($slow.Count) excess request(s)"
} else {
    Write-Check -Name "Queue Absorption" -Status "WARN" -Detail "All requests fast - may need lower RPM limit or requests hit cache"
}

Write-Host "`n--- Step C: Queue stats ---"
try {
    $headers = @{ "X-Admin-API-Key" = "local-admin-key" }
    $qStats = Invoke-RestMethod -Uri "http://localhost:8081/admin/stats/runtime" -Headers $headers -ErrorAction Stop
    $agents = if ($qStats.per_agent) { $qStats.per_agent } elseif ($qStats.rate_limits) { $qStats.rate_limits } else { @{} }
    foreach ($prop in $agents.PSObject.Properties) {
        $agent = $prop.Name
        $stats = $prop.Value
        Write-Host "  ${agent}: queued=$($stats.queued ?? 0), rejected=$($stats.rejected ?? 0), queue_depth=$($stats.queue_depth ?? 0)"
    }
} catch {
    Write-Host "Could not fetch queue stats: $($_.Exception.Message)" -ForegroundColor Red
}

Write-Host "`n--- Step D: Reset rate limit ---"
try {
    $body = '{"rpm": 10, "queue_depth": 50, "queue_timeout_seconds": 30}'
    Invoke-RestMethod -Uri "http://localhost:8081/admin/limits/a2a-translator" -Method Put -ContentType "application/json" -Body $body -ErrorAction Stop
    Write-Host "Rate limit restored to 10 RPM"
} catch {
    Write-Host "Failed to restore rate limit: $($_.Exception.Message)" -ForegroundColor Red
}

# ============================================================================
# CHECK 7: ADAPTIVE RATE LIMIT VERIFICATION
# ============================================================================
Write-Host "`n" + "=" * 70
Write-Host "  CHECK 7: ADAPTIVE RATE LIMIT VERIFICATION" -ForegroundColor Yellow
Write-Host "=" * 70

Write-Host "`n--- Router logs (adaptive/limit references) ---"
$adaptiveLogs = docker compose -f "$ProjectRoot\docker-compose.local.yml" --env-file "$ProjectRoot\.nasiko-local.env" logs nasiko-router 2>&1 | Select-String -Pattern "adaptive|limit adjusted|rpm" -CaseSensitive:$false | Select-Object -Last 20
if ($adaptiveLogs) {
    $adaptiveLogs | ForEach-Object { Write-Host "  $_" }
    Write-Check -Name "Adaptive Rate Limit" -Status "PASS" -Detail "Found adaptive rate limit log entries"
} else {
    Write-Host "  No adaptive rate limit logs found yet"
    $scheduledLogs = docker compose -f "$ProjectRoot\docker-compose.local.yml" --env-file "$ProjectRoot\.nasiko-local.env" logs nasiko-router 2>&1 | Select-String -Pattern "adaptive rate-limit loop started" -CaseSensitive:$false | Select-Object -Last 5
    if ($scheduledLogs) {
        $scheduledLogs | ForEach-Object { Write-Host "  $_" }
        Write-Check -Name "Adaptive Rate Limit" -Status "WARN" -Detail "Loop started but no adaptation events yet (runs every 60s)"
    } else {
        Write-Check -Name "Adaptive Rate Limit" -Status "WARN" -Detail "No adaptive logs found - feature may not be enabled"
    }
}

# ============================================================================
# CHECK 8: HTTP CACHE HEADERS
# ============================================================================
Write-Host "`n" + "=" * 70
Write-Host "  CHECK 8: HTTP CACHE HEADERS" -ForegroundColor Yellow
Write-Host "=" * 70

try {
    $resp = Invoke-WebRequest -Uri "http://localhost:9100/router/route?query=translate+hello+to+French" -ErrorAction Stop
    $cacheHeader = $resp.Headers["X-Cache"]
    $latencyHeader = $resp.Headers["X-Agent-Latency"]
    $cacheAgeHeader = $resp.Headers["X-Cache-Age"]
    $httpStatus = $resp.StatusCode

    Write-Host "HTTP Status:     $httpStatus"
    Write-Host "X-Cache:         $($cacheHeader ?? 'not present')"
    Write-Host "X-Agent-Latency: $($latencyHeader ?? 'not present')"
    Write-Host "X-Cache-Age:     $($cacheAgeHeader ?? 'not present')"

    $headersPresent = @()
    $headersMissing = @()
    if ($cacheHeader) { $headersPresent += "X-Cache=$cacheHeader" } else { $headersMissing += "X-Cache" }
    if ($latencyHeader) { $headersPresent += "X-Agent-Latency=$latencyHeader" } else { $headersMissing += "X-Agent-Latency" }

    if ($headersMissing.Count -eq 0) {
        Write-Check -Name "HTTP Cache Headers" -Status "PASS" -Detail "Headers present: $($headersPresent -join ', ')"
    } elseif ($headersPresent.Count -gt 0) {
        Write-Check -Name "HTTP Cache Headers" -Status "WARN" -Detail "Missing: $($headersMissing -join ', ')"
    } else {
        Write-Check -Name "HTTP Cache Headers" -Status "FAIL" -Detail "No cache headers found in response"
    }
} catch {
    Write-Check -Name "HTTP Cache Headers" -Status "FAIL" -Detail $_.Exception.Message
}

# ============================================================================
# CHECK 9: Save benchmark results
# ============================================================================
Write-Host "`n" + "=" * 70
Write-Host "  CHECK 9: Saving benchmark results" -ForegroundColor Yellow
Write-Host "=" * 70

$benchmarkDir = "$ProjectRoot\docs\buildthon-demo-assets"
if (-not (Test-Path $benchmarkDir)) {
    New-Item -ItemType Directory -Path $benchmarkDir -Force | Out-Null
}

try {
    $headers = @{ "X-Admin-API-Key" = "local-admin-key" }
    $runtimeStats = Invoke-RestMethod -Uri "http://localhost:8081/admin/stats/runtime" -Headers $headers -ErrorAction SilentlyContinue
} catch {
    $runtimeStats = @{ error = $_.Exception.Message }
}

try {
    $prometheusMetrics = (Invoke-WebRequest -Uri "http://localhost:8081/metrics" -ErrorAction SilentlyContinue).Content
    $metricsSample = $prometheusMetrics.Substring(0, [Math]::Min(500, $prometheusMetrics.Length))
} catch {
    $metricsSample = "Could not fetch metrics"
}

$benchmark = @{
    timestamp = (Get-Date -Format "yyyy-MM-ddTHH:mm:ssZ")
    benchmark_type = "phase1_verification"
    checks = $results
    summary = @{
        total = $passCount + $failCount + $warnCount
        passed = $passCount
        failed = $failCount
        warnings = $warnCount
    }
    cache_benchmark = @{
        first_call_ms = $firstMs
        second_call_ms = $secondMs
        speedup_ratio = if ($secondMs -gt 0) { [math]::Round($firstMs / $secondMs, 1) } else { 0 }
    }
    runtime_stats = $runtimeStats
    prometheus_metrics_sample = $metricsSample
}

$benchmark | ConvertTo-Json -Depth 10 | Out-File -FilePath "$benchmarkDir\latest-benchmark.json" -Encoding utf8
Write-Host "Benchmark saved to docs/buildthon-demo-assets/latest-benchmark.json"

# ============================================================================
# FINAL SUMMARY
# ============================================================================
Write-Host "`n" + "=" * 70
Write-Host "  FINAL SUMMARY" -ForegroundColor Cyan
Write-Host "=" * 70

Write-Host "`n  Check                     | Status"
Write-Host "  --------------------------+--------"
foreach ($check in $results.GetEnumerator() | Sort-Object Name) {
    $statusColor = switch ($check.Value.status) {
        "PASS" { "Green" }
        "FAIL" { "Red" }
        "WARN" { "Yellow" }
    }
    $paddedName = $check.Name.PadRight(26)
    Write-Host -NoNewline "  $paddedName| "
    Write-Host $check.Value.status -ForegroundColor $statusColor
}

Write-Host "`n  Total: $($passCount + $failCount + $warnCount)  |  " -NoNewline
Write-Host "PASS: $passCount" -ForegroundColor Green -NoNewline
Write-Host "  |  " -NoNewline
Write-Host "FAIL: $failCount" -ForegroundColor Red -NoNewline
Write-Host "  |  " -NoNewline
Write-Host "WARN: $warnCount" -ForegroundColor Yellow

if ($failCount -eq 0) {
    Write-Host "`n  ALL CHECKS PASSED!" -ForegroundColor Green
} else {
    Write-Host "`n  $failCount CHECK(S) FAILED - see details above" -ForegroundColor Red
}

Write-Host "`n" + "=" * 70
