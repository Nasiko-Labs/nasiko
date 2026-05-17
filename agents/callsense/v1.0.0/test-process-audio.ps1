# CallSense agent — local pipeline test
# Usage: .\test-process-audio.ps1
# Optional: .\test-process-audio.ps1 -BusinessId "your-uuid" -Phone "+6591234567"

param(
    [string]$AgentUrl = "http://localhost:8000",
    [string]$BusinessId = "test-biz",
    [string]$Phone = "+6591234567",
    [string]$Transcript = "Hello, I want to book an appointment for tomorrow afternoon."
)

Write-Host "==> Health check: $AgentUrl/health"
try {
    $health = Invoke-RestMethod -Uri "$AgentUrl/health" -Method Get
    $health | ConvertTo-Json
} catch {
    Write-Error "Agent not reachable. Is the container running? docker ps"
    exit 1
}

Write-Host "`n==> POST $AgentUrl/process-audio"
$body = @{
    business_id   = $BusinessId
    phone_number  = $Phone
    transcript    = $Transcript
    sentiment_score = 0.8
    sentiment_label = "positive"
} | ConvertTo-Json

try {
    $response = Invoke-RestMethod -Uri "$AgentUrl/process-audio" -Method Post `
        -ContentType "application/json" -Body $body
    Write-Host "Success. Response keys:" ($response.PSObject.Properties.Name -join ", ")
    if ($response.audio_base64) {
        Write-Host "audio_base64 length:" $response.audio_base64.Length
    }
    $response | ConvertTo-Json -Depth 5
} catch {
    Write-Error $_.Exception.Message
    if ($_.ErrorDetails.Message) { Write-Host $_.ErrorDetails.Message }
    exit 1
}
