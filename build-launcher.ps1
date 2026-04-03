[CmdletBinding()]
param(
    [switch]$Checks,

    [switch]$NoDiscord,

    [switch]$Installer
)

$ErrorActionPreference = "Stop"

$ScriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $ScriptRoot

function Invoke-CargoStep {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Label,

        [Parameter(Mandatory = $true)]
        [string[]]$Args
    )

    Write-Host ""
    Write-Host "==> $Label" -ForegroundColor Cyan
    Write-Host "cargo $($Args -join ' ')" -ForegroundColor DarkGray

    & cargo @Args
    if ($LASTEXITCODE -ne 0) {
        throw "Step failed: $Label"
    }
}

$CoreFeatureArgs = @()
if (-not $NoDiscord) {
    $CoreFeatureArgs += @("--features", "discord")
}

Write-Host "OSAgent launcher build" -ForegroundColor Green
Write-Host "Checks  : $Checks"
Write-Host "Discord : $(-not $NoDiscord)"
Write-Host "Installer: $Installer"

if ($Checks) {
    Invoke-CargoStep -Label "Check core formatting" -Args @("fmt", "--", "--check")
    Invoke-CargoStep -Label "Run core clippy" -Args @("clippy", "--all-targets", "--all-features", "--", "-D", "warnings")
    Invoke-CargoStep -Label "Run core tests" -Args @("test", "--all-features", "--verbose")
    Invoke-CargoStep -Label "Check launcher formatting" -Args @("fmt", "--manifest-path", "launcher/Cargo.toml", "--all", "--", "--check")
    Invoke-CargoStep -Label "Run launcher clippy" -Args @("clippy", "--manifest-path", "launcher/Cargo.toml", "--all-targets", "--all-features", "--", "-D", "warnings")
}

Invoke-CargoStep -Label "Build core" -Args (@("build", "--release") + $CoreFeatureArgs)
Invoke-CargoStep -Label "Build updater" -Args @("build", "--manifest-path", "updater/Cargo.toml", "--release")

$env:OSAGENT_CORE_SOURCE = Join-Path $ScriptRoot "target/release/osagent.exe"
$env:OSAGENT_UPDATER_SOURCE = Join-Path $ScriptRoot "updater/target/release/osagent-updater.exe"
Invoke-CargoStep -Label "Build launcher (embeds core + updater)" -Args @("build", "--manifest-path", "launcher/Cargo.toml", "--release")

if ($Installer) {
    Write-Host ""
    Write-Host "==> Build launcher installer (NSIS + WebView2 bootstrapper)" -ForegroundColor Cyan
    Write-Host "cargo tauri build --bundles nsis" -ForegroundColor DarkGray

    Push-Location (Join-Path $ScriptRoot "launcher")
    try {
        & cargo tauri build --bundles nsis
        if ($LASTEXITCODE -ne 0) {
            throw "Step failed: Build launcher installer"
        }
    }
    finally {
        Pop-Location
    }
}

$CoreBinary = Join-Path $ScriptRoot "target/release/osagent.exe"
$UpdaterBinary = Join-Path $ScriptRoot "updater/target/release/osagent-updater.exe"
$LauncherBinary = Join-Path $ScriptRoot "launcher/target/release/osagent-launcher.exe"
$InstallerBinary = Join-Path $ScriptRoot "launcher/target/release/bundle/nsis"

Write-Host ""
Write-Host "Build complete" -ForegroundColor Green
Write-Host "Core    : $CoreBinary"
Write-Host "Updater : $UpdaterBinary"
Write-Host "Launcher: $LauncherBinary"
if ($Installer) {
    Write-Host "Installer dir: $InstallerBinary"
}
