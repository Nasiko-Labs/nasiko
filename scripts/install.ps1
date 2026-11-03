# Nasiko CLI installer for Windows: published to the registry as nasiko/install-ps1 and served at
# https://registry.nasiko.dev/r/nasiko/install-ps1
#
# Usage (PowerShell):
#   irm https://registry.nasiko.dev/r/nasiko/install-ps1 | iex
#
# Environment overrides (see docs/CLI_INSTALL.md):
#   NASIKO_VERSION      version tag to install (default: latest)
#   NASIKO_INSTALL_DIR  install destination (default: %LOCALAPPDATA%\Programs\nasiko)
#   NASIKO_REGISTRY     registry host (default: registry.nasiko.dev)
#
# Like install.sh: no administrator rights, anonymous pulls, the binary verified against its
# content-addressed digest. Unlike install.sh it adds the install folder to the *user* PATH, the
# Windows convention for per-user tools (rustup and scoop do the same); nothing machine-wide changes.

$ErrorActionPreference = 'Stop'

$Registry = if ($env:NASIKO_REGISTRY) { $env:NASIKO_REGISTRY } else { 'registry.nasiko.dev' }
$Version = if ($env:NASIKO_VERSION) { $env:NASIKO_VERSION } else { 'latest' }
$Dir = if ($env:NASIKO_INSTALL_DIR) { $env:NASIKO_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\nasiko' }
$Api = "https://$Registry/v2/nasiko/nasiko"

function Fail($Message) {
    Write-Error "install: error: $Message"
    exit 1
}

# --- 1. platform: only windows/amd64 is built ---------------------------------
$Arch = switch ($env:PROCESSOR_ARCHITECTURE) {
    'AMD64' { 'amd64' }
    'ARM64' { 'amd64' }  # runs under the x64 emulation Windows on ARM includes
    default { Fail "unsupported architecture: $env:PROCESSOR_ARCHITECTURE" }
}
Write-Host "→ installing nasiko $Version for windows/$Arch"

# --- 2. resolve the multi-platform index to this platform's manifest ----------
try {
    $Index = Invoke-RestMethod -Uri "$Api/manifests/$Version" `
        -Headers @{ Accept = 'application/vnd.oci.image.index.v1+json' }
} catch { Fail "cannot fetch nasiko/nasiko:$Version from $Registry" }
$Child = ($Index.manifests | Where-Object { $_.platform.os -eq 'windows' -and $_.platform.architecture -eq $Arch } |
    Select-Object -First 1).digest
if (-not $Child) { Fail "release $Version has no windows/$Arch build" }

# --- 3. resolve the child manifest to its single binary layer ------------------
try {
    $Manifest = Invoke-RestMethod -Uri "$Api/manifests/$Child" `
        -Headers @{ Accept = 'application/vnd.oci.image.manifest.v1+json' }
} catch { Fail "cannot fetch manifest $Child" }
$Layer = $Manifest.layers[0].digest
if (-not $Layer) { Fail "manifest $Child has no layers" }

# --- 4. download and verify against the content-addressed digest ---------------
$Tmp = New-TemporaryFile
try {
    Write-Host "→ downloading $Layer"
    Invoke-WebRequest -Uri "$Api/blobs/$Layer" -OutFile $Tmp -UseBasicParsing
    $Got = 'sha256:' + (Get-FileHash -Algorithm SHA256 $Tmp).Hash.ToLower()
    if ($Got -ne $Layer) { Fail "digest mismatch: expected $Layer, got $Got" }

    # --- 5. install (per-user, no administrator) --------------------------------
    New-Item -ItemType Directory -Force -Path $Dir | Out-Null
    $Exe = Join-Path $Dir 'nasiko.exe'
    Move-Item -Force $Tmp $Exe
} finally {
    if (Test-Path $Tmp) { Remove-Item -Force $Tmp }
}

# --- 6. user PATH, verify, hand off ---------------------------------------------
$UserPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if (-not (($UserPath -split ';') -contains $Dir)) {
    $NewPath = if ($UserPath) { "$UserPath;$Dir" } else { $Dir }
    [Environment]::SetEnvironmentVariable('Path', $NewPath, 'User')
    Write-Host "→ added $Dir to your user PATH (open a new terminal to use it)"
}
Write-Host "✓ installed $(& $Exe --version) to $Exe"
