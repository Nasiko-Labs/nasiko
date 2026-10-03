# C5 — connect to the published agent port, not the control plane.
# Classifies loopback reachability. Does not print secrets.

$ErrorActionPreference = "Stop"

$agents = docker ps --filter "name=nasiko-agent" --format "{{.ID}} {{.Names}} {{.Ports}}"
if (-not $agents) {
    Write-Error "No nasiko-agent container is running."
}

Write-Host "Agents:"
Write-Host $agents
Write-Host ""

$ids = docker ps --filter "name=nasiko-agent" --format "{{.ID}}"
foreach ($id in $ids) {
    Write-Host "docker port $id"
    docker port $id
    $mapped = docker port $id 8000/tcp
    if ($mapped -match "127\.0\.0\.1:(\d+)") {
        $port = $Matches[1]
        $card = "http://127.0.0.1:$port/.well-known/agent-card.json"
        Write-Host "GET $card"
        curl.exe -sS -D - --max-time 10 $card
        Write-Host ""
        Write-Host "POST $port/ with a forged Bearer (body is a probe, not a secret)"
        $tmp = Join-Path $env:TEMP "nasiko-c5-a2a.json"
        '{"jsonrpc":"2.0","id":"c5","method":"message/send","params":{"message":{"messageId":"c5","role":"user","parts":[{"kind":"text","text":"ping"}]}}}' | Set-Content -Path $tmp -Encoding ascii
        curl.exe -sS -D - --max-time 15 -X POST "http://127.0.0.1:$port/" -H "Content-Type: application/json" -H "Authorization: Bearer not-a-nasiko-session" --data-binary "@$tmp"
        Write-Host ""
    }
}

Write-Host "Unauthenticated control plane (must not invoke the agent):"
curl.exe -sS -D - --max-time 10 -H "Accept: application/json" http://127.0.0.1:8080/api/agents/00000000-0000-0000-0000-000000000000/
Write-Host ""
Write-Host "Record TCP success as ENVIRONMENT on Docker Desktop loopback. A forged Bearer must not become platform ACL."
