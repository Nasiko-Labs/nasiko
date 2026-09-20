# C7 — burst POST /api/orchestrator/a2a until 429.
# Set NASIKO_TOKEN to a user session. Do not commit that value.

$ErrorActionPreference = "Stop"

if (-not $env:NASIKO_TOKEN) {
    Write-Error "Set NASIKO_TOKEN to a user Bearer token (not committed)."
}

$body = '{"jsonrpc":"2.0","method":"message/stream","id":"c7","params":{"message":{"messageId":"c7","role":"user","parts":[{"text":"rate-limit probe"}]}}}'
$tmp = Join-Path $env:TEMP "nasiko-c7-a2a.json"
Set-Content -Path $tmp -Value $body -Encoding ascii

$statuses = @()
for ($i = 1; $i -le 40; $i++) {
    $out = curl.exe -sS -o "$env:TEMP\nasiko-c7-body.txt" -w "%{http_code}" --max-time 15 `
        -X POST http://127.0.0.1:8080/api/orchestrator/a2a `
        -H "Authorization: Bearer $env:NASIKO_TOKEN" `
        -H "Content-Type: application/json" `
        -H "A2A-Version: 1.0" `
        --data-binary "@$tmp"
    $statuses += $out
    Write-Host "$i $out"
    if ($out -eq "429") {
        Write-Host "Body:"
        Get-Content "$env:TEMP\nasiko-c7-body.txt"
        break
    }
}

Write-Host "Statuses: $($statuses -join ',')"
Write-Host "Expect a 429 with a non-empty body (I7) within 40 calls."
