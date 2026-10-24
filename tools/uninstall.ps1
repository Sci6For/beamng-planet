[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)][string]$Manifest,
    [Parameter(Mandatory=$true)][AllowEmptyString()][string]$CurrentLaunchOptions,
    [switch]$DryRun,
    [string]$PythonExe = 'python'
)
$ErrorActionPreference = 'Stop'
$taskArguments = @((Join-Path $PSScriptRoot 'planet_install.py'), 'uninstall', '--manifest', $Manifest,
    "--current-launch-options=$CurrentLaunchOptions")
if ($DryRun) { $taskArguments += '--dry-run' }
& $PythonExe @taskArguments
if ($LASTEXITCODE -ne 0) { throw 'Planet uninstall refused. See diagnostic above; Steam was not changed.' }
