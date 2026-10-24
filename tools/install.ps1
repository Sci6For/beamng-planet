[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)][string]$Package,
    [Parameter(Mandatory=$true)][string]$Destination,
    [Parameter(Mandatory=$true)][string]$Profile,
    [Parameter(Mandatory=$true)][string]$GameRoot,
    [Parameter(Mandatory=$true)][string]$CacheDirectory,
    [Parameter(Mandatory=$true)][AllowEmptyString()][string]$OriginalLaunchOptions,
    [switch]$DryRun,
    [string]$PythonExe = 'python'
)
$ErrorActionPreference = 'Stop'
$taskArguments = @((Join-Path $PSScriptRoot 'planet_install.py'), 'install', '--package', $Package,
    '--destination', $Destination, '--profile', $Profile, '--game-root', $GameRoot,
    '--cache-directory', $CacheDirectory, "--original-launch-options=$OriginalLaunchOptions")
if ($DryRun) { $taskArguments += '--dry-run' }
& $PythonExe @taskArguments
if ($LASTEXITCODE -ne 0) { throw 'Planet install refused. See diagnostic above; Steam was not changed.' }
