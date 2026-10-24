# Embedded by the companion. Functions only: loading this file never changes Windows.
function Test-PlanetPort([string]$Ports) {
    foreach ($part in $Ports.Split(',')) {
        $part = $part.Trim()
        if ($part -eq '*' -or $part -eq '3698') { return $true }
        if ($part -match '^(\d+)-(\d+)$' -and [int]$Matches[1] -le 3698 -and [int]$Matches[2] -ge 3698) { return $true }
    }
    return $false
}
function Get-PlanetNetworkState($Policy, [string]$Application, [string]$RuleName) {
    foreach ($profile in @(1, 2, 4)) {
        if (($Policy.CurrentProfileTypes -band $profile) -and $Policy.FirewallEnabled($profile) -and $Policy.BlockAllInboundTraffic($profile)) {
            return 'Blocked'
        }
    }
    $coveredProfiles = 0
    foreach ($rule in $Policy.Rules) {
        # Respect disabled/edited Planet rules instead of silently undoing user choices.
        if ($rule.Name -eq $RuleName -and -not $rule.Enabled) { return 'Blocked' }
        if (-not $rule.Enabled -or $rule.Direction -ne 1 -or $rule.Protocol -notin @(17, 256)) { continue }
        if ($rule.Protocol -ne 256 -and -not (Test-PlanetPort $rule.LocalPorts)) { continue }
        $sameApp = $rule.ApplicationName -eq $Application
        $anyApp = [string]::IsNullOrEmpty($rule.ApplicationName) -or $rule.ApplicationName -eq '*'
        # An explicit block may be more specific than our allow rule. Never remove it.
        if ($rule.Action -eq 0 -and ($sameApp -or $anyApp) -and [string]::IsNullOrEmpty($rule.ServiceName) -and ($rule.Profiles -band $Policy.CurrentProfileTypes)) {
            return 'Blocked'
        }
        if ($rule.Action -eq 1 -and $sameApp -and $rule.RemoteAddresses -in @('*', 'LocalSubnet') -and
            $rule.LocalAddresses -eq '*' -and $rule.RemotePorts -in @('', '*') -and
            $rule.InterfaceTypes -eq 'All' -and [string]::IsNullOrEmpty($rule.ServiceName)) {
            $coveredProfiles = $coveredProfiles -bor $rule.Profiles
        }
    }
    if (($coveredProfiles -band 7) -eq 7) { return 'Ready' }
    if ($Policy.LocalPolicyModifyState -ne 0) { return 'Blocked' }
    # A name collision with a user-edited rule must not replace that rule.
    foreach ($rule in $Policy.Rules) { if ($rule.Name -eq $RuleName) { return 'Blocked' } }
    return 'NeedsSetup'
}
function Set-PlanetNetworkPermission($Policy, [string]$Application, [string]$RuleName,
    [scriptblock]$NewRule = { New-Object -ComObject HNetCfg.FWRule }) {
    $state = Get-PlanetNetworkState $Policy $Application $RuleName
    if ($state -ne 'NeedsSetup') { return $state }
    $rule = & $NewRule
    $rule.Name = $RuleName
    $rule.Description = 'Planet local-network hosting; installed server only, UDP 3698, local subnet.'
    $rule.ApplicationName = $Application
    $rule.Protocol = 17
    $rule.LocalPorts = '3698'
    $rule.RemotePorts = '*'
    $rule.LocalAddresses = '*'
    $rule.RemoteAddresses = 'LocalSubnet'
    $rule.Direction = 1
    $rule.Action = 1
    $rule.Profiles = 7
    $rule.InterfaceTypes = 'All'
    $rule.EdgeTraversal = $false
    $rule.Enabled = $true
    $null = $Policy.Rules.Add($rule)
    return (Get-PlanetNetworkState $Policy $Application $RuleName)
}
