[CmdletBinding()]
param(
    [switch]$Checks,

    [switch]$NoDiscord,

    [switch]$Installer,

    [switch]$Fast,

    [switch]$NoFrontendBundle
)

$ErrorActionPreference = "Stop"

$ScriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $ScriptRoot

# Every external tool in this script — cargo, npm, esbuild — writes ordinary
# progress to stderr. Under Windows PowerShell 5.1, `$ErrorActionPreference =
# "Stop"` turns the first stderr line into a terminating NativeCommandError
# before `$LASTEXITCODE` is ever read, so a *successful* step aborts the build.
# (PowerShell 7 does not do this, which is why the script worked in some
# terminals and not others.) Merge the streams for the duration of the call,
# print what the tool printed, and judge the step only by its exit code.
function Invoke-NativeStep {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Label,

        [Parameter(Mandatory = $true)]
        [string]$Exe,

        [Parameter(Mandatory = $true)]
        [string[]]$CommandArgs
    )

    Write-Host ""
    Write-Host "==> $Label" -ForegroundColor Cyan
    Write-Host "$Exe $($CommandArgs -join ' ')" -ForegroundColor DarkGray

    $previous = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        & $Exe @CommandArgs 2>&1 | ForEach-Object {
            # Native stderr arrives as an ErrorRecord whose exception message
            # is the line the tool wrote; anything else is plain output.
            $line = if ($_ -is [System.Management.Automation.ErrorRecord]) {
                $_.Exception.Message
            } else {
                $_
            }
            Write-Host $line
        }
    }
    finally {
        $ErrorActionPreference = $previous
    }
    if ($LASTEXITCODE -ne 0) {
        throw "Step failed: $Label"
    }
}

function Invoke-CargoStep {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Label,

        [Parameter(Mandatory = $true)]
        [string[]]$CommandArgs
    )

    Invoke-NativeStep -Label $Label -Exe "cargo" -CommandArgs $CommandArgs
}

function Invoke-NpmStep {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Label,

        [Parameter(Mandatory = $true)]
        [string[]]$CommandArgs
    )

    Invoke-NativeStep -Label $Label -Exe "npm" -CommandArgs $CommandArgs
}

$BundleFrontend = -not $NoFrontendBundle

$Features = @()
if (-not $NoDiscord) {
    $Features += "discord"
    $Features += "discord-voice"
}
if ($BundleFrontend) {
    $Features += "production-frontend"
}
$CoreFeatureArgs = @()
if ($Features.Count -gt 0) {
    $CoreFeatureArgs += @("--features", ($Features -join ","))
}

Write-Host "OSAgent launcher build" -ForegroundColor Green
Write-Host "Checks     : $Checks"
Write-Host "Discord    : $(-not $NoDiscord)"
Write-Host "Installer  : $Installer"
Write-Host "Fast       : $Fast"
Write-Host "Frontend   : $BundleFrontend"

if ($Fast -and $Checks) {
    Write-Host "Note: -Fast skips slow checks (clippy, fmt). Tests still run." -ForegroundColor Yellow
}

if ($Checks) {
    if (-not $Fast) {
        Invoke-CargoStep -Label "Check core formatting" -CommandArgs @("fmt", "--", "--check")
        Invoke-CargoStep -Label "Run core clippy" -CommandArgs @("clippy", "--all-targets", "--all-features", "--", "-D", "warnings")
    }
    Invoke-CargoStep -Label "Run core tests" -CommandArgs @("test", "--all-features", "--verbose")
    if (-not $Fast) {
        Invoke-CargoStep -Label "Check launcher formatting" -CommandArgs @("fmt", "--manifest-path", "launcher/Cargo.toml", "--all", "--", "--check")
        Invoke-CargoStep -Label "Run launcher clippy" -CommandArgs @("clippy", "--manifest-path", "launcher/Cargo.toml", "--all-targets", "--all-features", "--", "-D", "warnings")
    }
}

if ($BundleFrontend) {
    $FrontendDir = Join-Path $ScriptRoot "frontend"
    if (-not (Test-Path (Join-Path $FrontendDir "node_modules"))) {
        Invoke-NpmStep -Label "Install frontend deps" -CommandArgs @("install", "--prefix", $FrontendDir)
    }
    Invoke-NpmStep -Label "Bundle frontend (esbuild)" -CommandArgs @("run", "build", "--prefix", $FrontendDir)
}

$Profile = if ($Fast) { "dev-release" } else { "release" }
$ProfileArg = @("--profile", $Profile)

Invoke-CargoStep -Label "Build core ($Profile)" -CommandArgs (@("build") + $ProfileArg + $CoreFeatureArgs)
Invoke-CargoStep -Label "Build updater ($Profile)" -CommandArgs (@("build", "--manifest-path", "updater/Cargo.toml") + $ProfileArg)

$ProfileDir = if ($Fast) { "dev-release" } else { "release" }
$env:OSAGENT_CORE_SOURCE = Join-Path $ScriptRoot "target/$ProfileDir/osagent.exe"
$env:OSAGENT_UPDATER_SOURCE = Join-Path $ScriptRoot "updater/target/$ProfileDir/osagent-updater.exe"
Invoke-CargoStep -Label "Build launcher ($Profile) (embeds core + updater)" -CommandArgs (@("build", "--manifest-path", "launcher/Cargo.toml") + $ProfileArg)

if ($Installer) {
    if ($Fast) {
        Write-Host ""
        Write-Host "WARNING: -Installer with -Fast uses dev-release profile. Use without -Fast for production installer." -ForegroundColor Yellow
    }
    Push-Location (Join-Path $ScriptRoot "launcher")
    try {
        Invoke-NativeStep -Label "Build launcher installer (NSIS + WebView2 bootstrapper)" -Exe "cargo" -CommandArgs @("tauri", "build", "--bundles", "nsis")
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
