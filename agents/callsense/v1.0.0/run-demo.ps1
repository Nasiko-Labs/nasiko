# CallSense agent demo
# Usage: .\run-demo.ps1  or  .\run-demo.ps1 -BusinessId "your-uuid"
param(
    [string]$AgentUrl = "http://localhost:8000",
    [string]$BusinessId = "",
    [string]$Phone = "+6591234567"
)

Write-Host ""
Write-Host "CallSense Agent Demo" -ForegroundColor Cyan
Write-Host "Agent: $AgentUrl"
Write-Host ""
Write-Host "1) Text transcript (VALSEA sentiment only)"
Write-Host "2) Audio file .wav (full VALSEA STT + sentiment)"
Write-Host ""
$choice = Read-Host "Choose 1 or 2"

if (-not $BusinessId) {
    $BusinessId = Read-Host "Enter business_id UUID"
}
if (-not $BusinessId) { throw "business_id is required" }

if ($choice -eq "1") {
    $transcript = Read-Host "Enter caller message (or press Enter for default)"
    if (-not $transcript) {
        $transcript = "Hello, I want to book an appointment for tomorrow afternoon."
    }
    $body = @{
        business_id  = $BusinessId
        phone_number = $Phone
        transcript   = $transcript
    } | ConvertTo-Json

} elseif ($choice -eq "2") {
    $wavPath = Read-Host "Full path to .wav file (or Enter to skip)"
    if ($wavPath -and (Test-Path $wavPath)) {
        $bytes = [IO.File]::ReadAllBytes($wavPath)
        $b64   = [Convert]::ToBase64String($bytes)
        $body  = @{
            business_id  = $BusinessId
            phone_number = $Phone
            audio_base64 = $b64
            language     = "singlish"
        } | ConvertTo-Json
    } else {
        Write-Host "No WAV supplied - sending text instead." -ForegroundColor Yellow
        $body = @{
            business_id  = $BusinessId
            phone_number = $Phone
            transcript   = "Hello, I want to check my order status please."
        } | ConvertTo-Json
    }

} else {
    throw "Invalid choice - enter 1 or 2"
}

Write-Host ""
Write-Host "POST $AgentUrl/process-audio ..." -ForegroundColor Green
try {
    $result = Invoke-RestMethod -Uri "$AgentUrl/process-audio" `
        -Method Post -ContentType "application/json" -Body $body -TimeoutSec 120

    Write-Host ""
    Write-Host "SUCCESS" -ForegroundColor Green
    Write-Host "call_id : $($result.call_id)"
    Write-Host "intent  : $($result.intent)"
    Write-Host "response: $($result.response)"
    if ($result.audio_base64) {
        Write-Host "audio   : $($result.audio_base64.Length) chars (ElevenLabs via Next.js)"
    }
    Write-Host ""
    $result | ConvertTo-Json -Depth 4

} catch {
    Write-Host "FAILED" -ForegroundColor Red
    Write-Host $_.Exception.Message
    if ($_.ErrorDetails.Message) { Write-Host $_.ErrorDetails.Message }
}
