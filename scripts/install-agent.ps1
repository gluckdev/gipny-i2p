[CmdletBinding()]
param(
    [string]$Master,
    [string]$Name = $env:COMPUTERNAME,
    [string]$DataDir,
    [switch]$Uninstall
)

# Invoke with: irm https://raw.githubusercontent.com/gluckdev/gipny-i2p/main/scripts/install-agent.ps1 | iex
# Then: Install-GipnyAgent -Master '<card>' (in an elevated PowerShell).
function Install-GipnyAgent {
    [CmdletBinding()]
    param(
        [string]$Master,
        [string]$Name = $env:COMPUTERNAME,
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
    if (-not $Master) { $Master = Read-Host 'Master card' }
    if ($Master -notlike 'gipny:v2:*') { throw 'Use a gipny:v2 master card containing its relay address.' }
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
    $assets = @($release.assets | Where-Object { $_.name -like "gipny-agent_*_windows-$arch.tar.gz" })
    if ($assets.Count -ne 1) { throw "Release has no unique gipny-agent_*_windows-$arch.tar.gz asset." }
    $temp = Join-Path ([IO.Path]::GetTempPath()) ('gipny-agent-' + [guid]::NewGuid())
    New-Item $temp -ItemType Directory | Out-Null
    try {
        $archive = Join-Path $temp 'agent.tar.gz'
        Invoke-WebRequest $assets[0].browser_download_url -OutFile $archive -UseBasicParsing
        $members = & tar -tzf $archive
        if ($LASTEXITCODE -ne 0) { throw 'Could not list archive (Windows tar.exe required).' }
        if ($members | Where-Object { $_ -match '(^[/\\]|(^|[/\\])\.\.([/\\]|$)|^[A-Za-z]:)' }) { throw 'Unsafe archive paths.' }
        & tar -xzf $archive -C $temp
        if ($LASTEXITCODE -ne 0) { throw 'Could not extract archive.' }
        $exe = @(Get-ChildItem $temp -Recurse -File -Filter 'gipny-agent.exe')
        $seed = @(Get-ChildItem $temp -Recurse -File -Filter 'i2pd-netdb-seed.tar.gz')
        if ($exe.Count -ne 1 -or $seed.Count -ne 1) { throw 'Archive must contain an agent executable and a network seed.' }
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
        New-Item $marker -ItemType File -Force | Out-Null
        @{ DataDir = $DataDir } | ConvertTo-Json | Set-Content $configPath -Encoding UTF8
        $arguments = '--data ' + (Quote-AgentArgument $DataDir) + ' --master ' + (Quote-AgentArgument $Master) + ' --name ' + (Quote-AgentArgument $Name)
        $action = New-ScheduledTaskAction -Execute (Join-Path $installDir 'gipny-agent.exe') -Argument $arguments -WorkingDirectory $DataDir
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
