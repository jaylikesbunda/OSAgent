[CmdletBinding()]
param(
    [switch]$Checks,

    [switch]$NoDiscord,

    [switch]$Installer,

    [switch]$Fast
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
Write-Host "Checks   : $Checks"
Write-Host "Discord  : $(-not $NoDiscord)"
Write-Host "Installer: $Installer"
Write-Host "Fast     : $Fast"

if ($Fast -and $Checks) {
    Write-Host "Note: -Fast skips slow checks (clippy, fmt). Tests still run." -ForegroundColor Yellow
}

if ($Checks) {
    if (-not $Fast) {
        Invoke-CargoStep -Label "Check core formatting" -Args @("fmt", "--", "--check")
        Invoke-CargoStep -Label "Run core clippy" -Args @("clippy", "--all-targets", "--all-features", "--", "-D", "warnings")
    }
    Invoke-CargoStep -Label "Run core tests" -Args @("test", "--all-features", "--verbose")
    if (-not $Fast) {
        Invoke-CargoStep -Label "Check launcher formatting" -Args @("fmt", "--manifest-path", "launcher/Cargo.toml", "--all", "--", "--check")
        Invoke-CargoStep -Label "Run launcher clippy" -Args @("clippy", "--manifest-path", "launcher/Cargo.toml", "--all-targets", "--all-features", "--", "-D", "warnings")
    }
}

$Profile = if ($Fast) { "dev-release" } else { "release" }
$ProfileArg = @("--profile", $Profile)

Invoke-CargoStep -Label "Build core ($Profile)" -Args (@("build") + $ProfileArg + $CoreFeatureArgs)
Invoke-CargoStep -Label "Build updater ($Profile)" -Args (@("build", "--manifest-path", "updater/Cargo.toml") + $ProfileArg)

$ProfileDir = if ($Fast) { "dev-release" } else { "release" }
$env:OSAGENT_CORE_SOURCE = Join-Path $ScriptRoot "target/$ProfileDir/osagent.exe"
$env:OSAGENT_UPDATER_SOURCE = Join-Path $ScriptRoot "updater/target/$ProfileDir/osagent-updater.exe"
Invoke-CargoStep -Label "Build launcher ($Profile) (embeds core + updater)" -Args (@("build", "--manifest-path", "launcher/Cargo.toml") + $ProfileArg)

if ($Installer) {
    if ($Fast) {
        Write-Host ""
        Write-Host "WARNING: -Installer with -Fast uses dev-release profile. Use without -Fast for production installer." -ForegroundColor Yellow
    }
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

$ProfileDir = if ($Fast) { "dev-release" } else { "release" }
$CoreBinary = Join-Path $ScriptRoot "target/$ProfileDir/osagent.exe"
$UpdaterBinary = Join-Path $ScriptRoot "updater/target/$ProfileDir/osagent-updater.exe"
$LauncherBinary = Join-Path $ScriptRoot "launcher/target/$ProfileDir/osagent-launcher.exe"
$InstallerBinary = Join-Path $ScriptRoot "launcher/target/release/bundle/nsis"

Write-Host ""
Write-Host "Build complete" -ForegroundColor Green
Write-Host "Core    : $CoreBinary"
Write-Host "Updater : $UpdaterBinary"
Write-Host "Launcher: $LauncherBinary"
if ($Installer) {
    Write-Host "Installer dir: $InstallerBinary"
}
