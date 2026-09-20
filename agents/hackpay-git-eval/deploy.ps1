# Deploy HackPay git-eval agent to Nasiko
# Prerequisites: Docker healthy, then `nasiko up` (Enter through prompts)
# Usage: .\deploy.ps1

$ErrorActionPreference = "Stop"
$AgentDir = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $AgentDir

Write-Host "==> Checking Nasiko control plane at http://localhost:8080 ..."
try {
  $h = Invoke-WebRequest -Uri "http://localhost:8080/health" -TimeoutSec 5
  Write-Host "    CP healthy: $($h.StatusCode)"
} catch {
  Write-Host "ERROR: Nasiko CP not reachable. Run: nasiko up"
  exit 1
}

Write-Host "==> Connecting CLI ..."
nasiko connect http://localhost:8080 --name local 2>$null
nasiko auth login --username admin --password fKVBEzrLiboRdtWNnZcM 2>$null
# Fallback: try status
nasiko auth status

Write-Host "==> Deploying hackpay-git-eval (port 8000, Anakin secret from .env) ..."
nasiko deploy . --name hackpay-git-eval --port 8000 --env-file .env -y -v 1.0.0

Write-Host "==> Agents:"
nasiko ps
nasiko agents ls

Write-Host @"

Done. Wire HackPay:

  NASIKO_GIT_EVAL_URL=http://localhost:8080/api/agents/<agent-id>
  # or direct container if published:
  # NASIKO_GIT_EVAL_URL=http://127.0.0.1:<mapped>/evaluate

Test:
  nasiko chat hackpay-git-eval "{\"idea\":\"escrow\",\"githubUrl\":\"https://github.com/octocat/Hello-World\"}"

"@
