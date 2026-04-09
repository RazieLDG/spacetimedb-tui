#Requires -Version 5.1
<#
.SYNOPSIS
    Installer for spacetimedb-tui on Windows.

.DESCRIPTION
    Downloads the latest (or a pinned) pre-built release archive from
    GitHub, extracts it, and installs the binary into a user-local
    directory. Optionally appends that directory to the user PATH so
    the command works in new terminals without manual PATH editing.

.PARAMETER Version
    Release tag to install. Defaults to "latest".

.PARAMETER InstallDir
    Target directory. Defaults to "$env:LOCALAPPDATA\Programs\spacetimedb-tui".

.PARAMETER AddToPath
    If specified, the install directory is prepended to the per-user
    PATH environment variable (persistent, takes effect in new shells).

.EXAMPLE
    irm https://raw.githubusercontent.com/RazieLDG/spacetimedb-tui/main/scripts/install.ps1 | iex

.EXAMPLE
    # Pin a version, install into Program Files-style path, update PATH:
    .\install.ps1 -Version v0.1.0 -AddToPath

.NOTES
    Requires PowerShell 5.1+ (ships with Windows 10/11). TLS 1.2 is
    forced so the script works on stock installations where the
    default security protocol is too weak for github.com.
#>

[CmdletBinding()]
param(
    [string]$Version = $env:STDB_TUI_VERSION,
    [string]$InstallDir = $env:STDB_TUI_INSTALL_DIR,
    [switch]$AddToPath
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'  # faster Invoke-WebRequest

# ── Constants ──────────────────────────────────────────────────────
$Repo   = 'RazieLDG/spacetimedb-tui'
$BinName = 'spacetimedb-tui'
$Target  = 'x86_64-pc-windows-msvc'

if (-not $Version)     { $Version = 'latest' }
if (-not $InstallDir)  { $InstallDir = Join-Path $env:LOCALAPPDATA "Programs\spacetimedb-tui" }

# ── Helpers ────────────────────────────────────────────────────────
function Write-Info($msg)  { Write-Host "==> $msg"           -ForegroundColor Cyan }
function Write-Ok($msg)    { Write-Host "ok: $msg"           -ForegroundColor Green }
function Write-Warn2($msg) { Write-Host "warn: $msg"         -ForegroundColor Yellow }
function Write-Err($msg)   { Write-Host "error: $msg"        -ForegroundColor Red; exit 1 }

# Force TLS 1.2 — default on older PS 5.1 is TLS 1.0/1.1 which
# github.com rejects.
[System.Net.ServicePointManager]::SecurityProtocol =
    [System.Net.SecurityProtocolType]::Tls12

# ── Resolve version ────────────────────────────────────────────────
if ($Version -eq 'latest') {
    Write-Info "Resolving latest release from github.com/$Repo..."
    try {
        $resp = Invoke-WebRequest `
            -Uri "https://github.com/$Repo/releases/latest" `
            -MaximumRedirection 0 -ErrorAction SilentlyContinue
    } catch {
        $resp = $_.Exception.Response
    }
    $redirect = $null
    if ($resp.Headers.Location)       { $redirect = $resp.Headers.Location }
    elseif ($resp.Headers['Location']) { $redirect = $resp.Headers['Location'] }
    if (-not $redirect) {
        try {
            $api = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest"
            $Version = $api.tag_name
        } catch {
            Write-Err "Could not determine latest version. Try passing -Version v0.1.0 manually."
        }
    } else {
        $Version = [System.IO.Path]::GetFileName($redirect.ToString())
    }
    if (-not $Version -or $Version -eq 'latest') {
        Write-Err "Could not determine latest version — no releases yet?"
    }
    Write-Ok "Latest version: $Version"
}

# ── Download + extract ─────────────────────────────────────────────
$archiveName = "$BinName-$Version-$Target.zip"
$url = "https://github.com/$Repo/releases/download/$Version/$archiveName"

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("stdb-tui-install-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
    $zipPath = Join-Path $tmp $archiveName
    Write-Info "Downloading $archiveName..."
    try {
        Invoke-WebRequest -Uri $url -OutFile $zipPath -UseBasicParsing
    } catch {
        Write-Err "Download failed. URL: $url`n$_"
    }

    Write-Info "Extracting archive..."
    Expand-Archive -Path $zipPath -DestinationPath $tmp -Force

    $staged = Join-Path $tmp "$BinName-$Version-$Target"
    $exePath = Join-Path $staged "$BinName.exe"
    if (-not (Test-Path $exePath)) {
        Write-Err "Archive layout unexpected — could not find $BinName.exe inside."
    }

    # ── Install ────────────────────────────────────────────────────
    if (-not (Test-Path $InstallDir)) {
        New-Item -ItemType Directory -Path $InstallDir | Out-Null
    }
    $destExe = Join-Path $InstallDir "$BinName.exe"
    Copy-Item -Path $exePath -Destination $destExe -Force
    Write-Ok "Installed $BinName $Version -> $destExe"
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}

# ── PATH management ────────────────────────────────────────────────
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
$onPath = ($userPath -split ';') -contains $InstallDir

if (-not $onPath) {
    if ($AddToPath) {
        $newPath = if ([string]::IsNullOrEmpty($userPath)) { $InstallDir } else { "$InstallDir;$userPath" }
        [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
        Write-Ok "Added $InstallDir to user PATH. Open a new terminal for the change to take effect."
    } else {
        Write-Warn2 "$InstallDir is not on your PATH."
        Write-Host  "       rerun with -AddToPath or add it manually:"
        Write-Host  "         [Environment]::SetEnvironmentVariable('Path', '$InstallDir;' + [Environment]::GetEnvironmentVariable('Path','User'), 'User')"
    }
}

# ── Smoke test ─────────────────────────────────────────────────────
try {
    $out = & $destExe --version 2>$null
    if ($LASTEXITCODE -eq 0 -and $out) { Write-Ok $out }
} catch {
    # Non-fatal — some builds don't ship --version.
}

Write-Ok "Done. Run '$BinName --help' (in a fresh terminal if PATH was updated) to get started."
