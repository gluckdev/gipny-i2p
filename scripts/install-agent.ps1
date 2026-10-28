[CmdletBinding()]
param(
    [string]$Master,
    [string]$Name,
    [string]$DataDir,
    [switch]$Uninstall
)

function Test-GipnyMasterCardShape([string]$Card) {
    return $Card -cmatch '^gipny:v2:' -or $Card -cmatch '^gipny:v3:[A-Za-z0-9_-]{135,307}$'
}

function Get-GipnyReleaseChecksum {
    param([string]$Manifest, [string]$AssetName)
    $found = @()
    foreach ($line in ($Manifest -split '\r?\n')) {
        if (-not $line.Trim()) { continue }
        if ($line -cnotmatch '^([A-Fa-f0-9]{64}) [ *](.+)$') { throw 'Malformed checksum manifest.' }
        $digest = $Matches[1].ToLowerInvariant()
        $filename = $Matches[2] -replace '^\./', ''
        if ($filename -ceq $AssetName) { $found += $digest }
    }
    if ($found.Count -ne 1) { throw 'Checksum manifest must contain exactly one entry for the agent archive.' }
    return $found[0]
}

# Invoke with: irm https://raw.githubusercontent.com/gluckdev/gipny-i2p/main/scripts/install-agent.ps1 | iex
# Then: Install-GipnyAgent -Master '<card>' (in an elevated PowerShell).
function Install-GipnyAgent {
    [CmdletBinding()]
    param(
        [string]$Master,
        [string]$Name,
        [string]$DataDir,
        [switch]$Uninstall
    )
    $ErrorActionPreference = 'Stop'
    $admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole(
        [Security.Principal.WindowsBuiltInRole]::Administrator)
    if (-not $admin) { throw 'Open PowerShell as Administrator to install/remove the startup task.' }
    $taskName = 'GipnyAgent'
    $installDir = Join-Path $env:ProgramFiles 'gipny-agent'
    $configPath = Join-Path $installDir 'install.json'
    if (-not $DataDir) {
        if (Test-Path $configPath) { $DataDir = (Get-Content $configPath -Raw | ConvertFrom-Json).DataDir }
        else { $DataDir = Join-Path $env:ProgramData 'gipny-agent' }
    }
    $DataDir = [IO.Path]::GetFullPath($DataDir)
    if ($DataDir.TrimEnd('\') -eq [IO.Path]::GetPathRoot($DataDir).TrimEnd('\') -or
        $DataDir.TrimEnd('\') -in @($env:USERPROFILE, $env:ProgramFiles, $env:ProgramData, $env:windir)) {
        throw 'DataDir must be a separate directory for the agent.'
    }
    $marker = Join-Path $DataDir '.gipny-agent-install'
    if ($Uninstall) {
        if ((Test-Path $DataDir) -and -not (Test-Path $marker)) { throw "Installer marker missing in $DataDir; refusing to delete data." }
        $task = Get-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
        if ($task) {
            Stop-ScheduledTask -TaskName $taskName
            Unregister-ScheduledTask -TaskName $taskName -Confirm:$false
        }
        # Stop-ScheduledTask can return before the process releases its files.
        Start-Sleep -Seconds 2
        if (Test-Path $installDir) { Remove-Item $installDir -Recurse -Force }
        if (Test-Path $DataDir) { Remove-Item $DataDir -Recurse -Force }
        Write-Host 'GIPNY agent removed.'
        return
    }
    if (-not $Master) { $Master = Read-Host 'Your GIPNY contact card (the agent will contact you)' }
    if (-not (Test-GipnyMasterCardShape $Master)) { throw 'Use a gipny:v3 (or v2) master card containing its relay address.' }
    if (-not $Name -and (Test-Path -LiteralPath $configPath)) {
        $saved = Get-Content -LiteralPath $configPath -Raw | ConvertFrom-Json
        if ($saved.Name) { $Name = [string]$saved.Name }
    }
    if (-not $Name) { $Name = @('Murka', 'Barsik', 'Matroskin', 'Felix', 'Luna', 'Nala', 'Simba', 'Tigra', 'Tom', 'Bagheera') | Get-Random }
    # Win32 command-line quoting (backslashes before quotes and at the end).
    function Quote-AgentArgument([string]$Value) {
        '"' + ([regex]::Replace([regex]::Replace($Value, '(\\*)"', '$1$1\"'), '(\\+)$', '$1$1')) + '"'
    }
    $archValue = $env:PROCESSOR_ARCHITEW6432
    if (-not $archValue) { $archValue = $env:PROCESSOR_ARCHITECTURE }
    switch ($archValue.ToUpperInvariant()) {
        'AMD64' { $arch = 'amd64' }
        'ARM64' { $arch = 'arm64' }
        default { throw "Unsupported Windows architecture: $archValue" }
    }
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    $release = Invoke-RestMethod 'https://api.github.com/repos/gluckdev/gipny-i2p/releases/latest'
    $version = ([string]$release.tag_name) -replace '^v', ''
    if ($version -cnotmatch '^[0-9]+\.[0-9]+\.[0-9]+([+-][A-Za-z0-9.+-]+)?$') { throw 'Invalid release version.' }
    $assetName = "gipny-agent_${version}_windows-$arch.tar.gz"
    $assets = @($release.assets | Where-Object { $_.name -ceq $assetName })
    $manifests = @($release.assets | Where-Object { $_.name -ceq 'SHA256SUMS.txt' })
    if ($assets.Count -ne 1 -or $manifests.Count -ne 1) { throw 'Release must contain exactly one matching agent archive and SHA256SUMS.txt.' }
    $temp = Join-Path ([IO.Path]::GetTempPath()) ('gipny-agent-' + [guid]::NewGuid())
    New-Item $temp -ItemType Directory | Out-Null
    try {
        $archive = Join-Path $temp 'agent.tar.gz'
        Invoke-WebRequest $assets[0].browser_download_url -OutFile $archive -UseBasicParsing
        if ((Get-Item -LiteralPath $archive).Length -ne $assets[0].size) { throw 'Agent archive size mismatch.' }
        $checksumFile = Join-Path $temp 'SHA256SUMS.txt'
        Invoke-WebRequest $manifests[0].browser_download_url -OutFile $checksumFile -UseBasicParsing
        if ((Get-Item -LiteralPath $checksumFile).Length -gt 1MB) { throw 'Oversized checksum manifest.' }
        $expected = Get-GipnyReleaseChecksum -Manifest (Get-Content -LiteralPath $checksumFile -Raw) -AssetName $assetName
        if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant() -cne $expected) { throw 'Agent archive checksum mismatch.' }
        $members = & tar -tzf $archive
        if ($LASTEXITCODE -ne 0) { throw 'Could not list archive (Windows tar.exe required).' }
        if ($members | Where-Object { $_ -match '(^[/\\]|(^|[/\\])\.\.([/\\]|$)|^[A-Za-z]:)' }) { throw 'Unsafe archive paths.' }
        & tar -xzf $archive -C $temp
        if ($LASTEXITCODE -ne 0) { throw 'Could not extract archive.' }
        $exe = @(Get-ChildItem $temp -Recurse -File -Filter 'gipny-agent.exe')
        $seed = @(Get-ChildItem $temp -Recurse -File -Filter 'i2pd-netdb-seed.tar.gz')
        $bootstrap = @(Get-ChildItem $temp -Recurse -File -Filter 'start-agent.ps1')
        if ($exe.Count -ne 1 -or $seed.Count -ne 1 -or $bootstrap.Count -ne 1) { throw 'Archive must contain an executable, network seed and startup bootstrap.' }
        if ($Master.StartsWith('gipny:v3:')) {
            & $exe[0].FullName '--check-master-card' $Master *> $null
            if ($LASTEXITCODE -ne 0) { throw 'Invalid card or this agent does not support v3; version 0.4.19 or newer is required.' }
        }
        if (Get-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue) {
            Stop-ScheduledTask -TaskName $taskName
            Start-Sleep -Seconds 2
        }
        New-Item $installDir -ItemType Directory -Force | Out-Null
        New-Item $DataDir -ItemType Directory -Force | Out-Null
        # Headless identities must only be readable by administrators and SYSTEM.
        foreach ($dir in @($installDir, $DataDir)) {
            & icacls $dir /inheritance:r /grant:r '*S-1-5-18:(OI)(CI)F' '*S-1-5-32-544:(OI)(CI)F' | Out-Null
            if ($LASTEXITCODE -ne 0) { throw "Could not secure $dir" }
        }
        Copy-Item $exe[0].FullName (Join-Path $installDir 'gipny-agent.exe') -Force
        Copy-Item $seed[0].FullName (Join-Path $installDir 'i2pd-netdb-seed.tar.gz') -Force
        Copy-Item $bootstrap[0].FullName (Join-Path $installDir 'start-agent.ps1') -Force
        # A manual reinstall already selected/verified latest. Do not let an
        # older queued automatic update roll it back at the next task start.
        $pendingAgent = Join-Path $DataDir 'pending_agent'
        if (Test-Path -LiteralPath $pendingAgent) { Remove-Item -LiteralPath $pendingAgent -Recurse -Force }
        New-Item $marker -ItemType File -Force | Out-Null
        $agentArgs = @('--data', $DataDir, '--master', $Master, '--name', $Name)
        @{ DataDir = $DataDir; Name = $Name; Version = $version; AgentArgs = $agentArgs } | ConvertTo-Json | Set-Content $configPath -Encoding UTF8
        $arguments = '-NoProfile -NonInteractive -ExecutionPolicy Bypass -File ' +
            (Quote-AgentArgument (Join-Path $installDir 'start-agent.ps1')) + ' -ConfigPath ' + (Quote-AgentArgument $configPath)
        $action = New-ScheduledTaskAction -Execute (Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe') -Argument $arguments -WorkingDirectory $DataDir
        $trigger = New-ScheduledTaskTrigger -AtStartup
        $settings = New-ScheduledTaskSettingsSet -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) -ExecutionTimeLimit ([TimeSpan]::Zero) -StartWhenAvailable -MultipleInstances IgnoreNew -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
        $principal = New-ScheduledTaskPrincipal -UserId 'SYSTEM' -LogonType ServiceAccount -RunLevel Highest
        Register-ScheduledTask -TaskName $taskName -Action $action -Trigger $trigger -Settings $settings -Principal $principal -Force | Out-Null
        Start-ScheduledTask -TaskName $taskName
        Write-Host "GIPNY agent installed and started. Data: $DataDir"
        Write-Host "Status: Get-ScheduledTask -TaskName $taskName | Get-ScheduledTaskInfo"
        Write-Host 'The agent will appear in the master contacts after connecting to i2p.'
    } finally {
        Remove-Item $temp -Recurse -Force
    }
}

# Direct -File invocation accepts the same parameters. With irm | iex the
# function is loaded without starting an interactive installation.
if ($PSBoundParameters.Count -gt 0) {
    Install-GipnyAgent @PSBoundParameters
}
