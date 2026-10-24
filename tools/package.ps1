[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)][string]$OutputDirectory,
    [string]$BackendExe = (Join-Path $PSScriptRoot '../artifacts/backend-planet6/dist/BeamNG.Earth.exe'),
    [string]$SourceDirectory = (Join-Path $PSScriptRoot '../../beamng-earth-audit/1.5.3-beta/source/BeamNG.Earth-011d6b54e42407b2f3a6deaddc47305cf2325a47'),
    [string]$GameRoot = 'C:/GameDev/Steam/steamapps/common/BeamNG.drive',
    [string]$PythonExe = 'python',
    [switch]$LanCandidate
)
$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path $PSScriptRoot -Parent
if (-not [IO.Path]::IsPathFullyQualified($OutputDirectory)) { throw 'OutputDirectory must be absolute.' }
if (Test-Path -LiteralPath $OutputDirectory) { throw 'OutputDirectory already exists; choose a fresh directory.' }
if ($LanCandidate) {
    if (-not $PSBoundParameters.ContainsKey('GameRoot')) { throw 'LAN candidates require explicit GameRoot.' }
    & $PythonExe (Join-Path $PSScriptRoot 'build_planet_lan.py') --output $OutputDirectory --source $SourceDirectory --game $GameRoot
    if ($LASTEXITCODE -ne 0) { throw 'LAN candidate build failed; retained output is not accepted.' }
    exit 0
}
if ((Get-FileHash -LiteralPath $BackendExe -Algorithm SHA256).Hash.ToLowerInvariant() -ne 'f1187d7decaf37e3b7a62b637921c972bd311def3ee4229d509f67024ec2dabd') {
    throw 'Backend is not the approved BeamNG.Earth 1.5.4-planet6 executable.'
}
& cargo build --offline --release --locked --manifest-path (Join-Path $repoRoot 'companion/Cargo.toml')
if ($LASTEXITCODE -ne 0) { throw 'Release build failed.' }
& $PythonExe (Join-Path $PSScriptRoot 'package_planet.py') --output $OutputDirectory --backend $BackendExe --source $SourceDirectory --game $GameRoot
if ($LASTEXITCODE -ne 0) { throw 'Planet package validation failed; no package published.' }
