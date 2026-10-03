# C1 — terminate the agent container while a dashboard stream is open.
# Start a long Translator turn first, then run this so the kill is in-flight.

param(
    [string]$ContainerId = ""
)

$ErrorActionPreference = "Stop"

if (-not $ContainerId) {
    $ContainerId = docker ps --filter "name=nasiko-agent" --format "{{.ID}}" | Select-Object -First 1
}

if (-not $ContainerId) {
    Write-Error "No nasiko-agent container. Pass -ContainerId explicitly."
}

Write-Host "Target: $ContainerId"
docker ps --filter "id=$ContainerId" --format "{{.ID}} {{.Names}} {{.Status}} {{.Ports}}"
Write-Host ""
Write-Host "In the dashboard, start a long stream now. Then press Enter to docker kill."
$null = Read-Host

docker kill $ContainerId
Write-Host "Kill sent. Container list:"
docker ps --filter "name=nasiko-agent" --format "{{.ID}} {{.Names}} {{.Status}}"
Write-Host ""
Write-Host "Record: did the chat stop or error (I8)? Does catalog/dashboard status leave Active (I11)?"
Write-Host "Prefer restart from Nasiko, not only docker start."
