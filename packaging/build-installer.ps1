# Builds the installer, end to end: the release binary, the bundled modules, and
# the setup program.
#
# Output: packaging\output\iris-setup-<version>.exe
#
# The version is read from Cargo.toml and handed to Inno Setup, so it is written in
# exactly one place. The release workflow runs this same script: what CI publishes
# is what a local build produces.

param(
    # Package target\release\iris.exe as it is, without building it: the release
    # workflow hands over the binary CI built and tested for the same commit.
    [switch]$UseBuiltBinary
)

$ErrorActionPreference = 'Stop'

$racine = Split-Path -Parent $PSScriptRoot

$manifeste = Get-Content (Join-Path $racine 'Cargo.toml') -Raw
$version = [regex]::Match($manifeste, '(?ms)^\[workspace\.package\].*?^version\s*=\s*"([^"]+)"').Groups[1].Value
if (-not $version) { throw "No version under [workspace.package] in Cargo.toml" }
Write-Host "Iris $version"

if ($UseBuiltBinary) {
    $binaire = Join-Path $racine 'target\release\iris.exe'
    if (-not (Test-Path $binaire)) { throw "No binary at $binaire" }
    Write-Host "Using the binary already built: $binaire"
} else {
    & cargo build --release -p iris-app
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed with exit code $LASTEXITCODE" }
}

& (Join-Path $PSScriptRoot 'build-plugins.ps1')

# Where Inno Setup lands depends on how it was installed.
$iscc = @(
    (Get-Command iscc -ErrorAction SilentlyContinue).Source,
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    "$env:ProgramFiles\Inno Setup 6\ISCC.exe"
) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
if (-not $iscc) { throw "Inno Setup 6 not found. Install it: winget install JRSoftware.InnoSetup" }

& $iscc /Q "/DAppVersion=$version" (Join-Path $PSScriptRoot 'iris.iss')
if ($LASTEXITCODE -ne 0) { throw "iscc failed with exit code $LASTEXITCODE" }

$sortie = Join-Path $PSScriptRoot "output\iris-setup-$version.exe"
if (-not (Test-Path $sortie)) { throw "Built nothing at $sortie" }
Write-Host "Built $sortie"
