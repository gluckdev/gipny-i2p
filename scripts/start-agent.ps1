# Task Scheduler starts this bootstrap before the executable, so Windows never
# has to overwrite a running image. Installation/data ACLs are set by installer.
[CmdletBinding()]
param([string]$ConfigPath = (Join-Path $PSScriptRoot 'install.json'))

function Quote-GipnyArgument([string]$Value) {
    '"' + ([regex]::Replace([regex]::Replace($Value, '(\\*)"', '$1$1\"'), '(\\+)$', '$1$1')) + '"'
}

# Journal paths are basenames, never arbitrary destinations from JSON.
function Read-GipnyUpdateJournal([string]$InstallDir) {
    $path = Join-Path $InstallDir '.agent-update-state.json'
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { return $null }
    $item = Get-Item -LiteralPath $path
    if ($item.Length -gt 65536 -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'Invalid update journal.' }
    $record = Get-Content -LiteralPath $path -Raw | ConvertFrom-Json
    if ($record.Schema -ne 1 -or $record.Backup -isnot [string] -or
        $record.Backup -cnotmatch '^\.gipny-agent-backup-[a-f0-9]{32}\.exe$' -or
        $record.Sha256 -isnot [string] -or $record.Sha256 -cnotmatch '^[a-f0-9]{64}$' -or
        $record.OldSha256 -isnot [string] -or $record.OldSha256 -cnotmatch '^[a-f0-9]{64}$' -or
        $record.Accepted -isnot [bool]) { throw 'Invalid update journal.' }
    return $record
}

function Save-GipnyUpdateJournal([string]$InstallDir, $Record, [switch]$Replace) {
    $path = Join-Path $InstallDir '.agent-update-state.json'
    $temp = Join-Path $InstallDir ('.agent-update-state-' + [guid]::NewGuid().ToString('N') + '.tmp')
    try {
        $output = [IO.File]::Open($temp, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
        try {
            $bytes = [Text.Encoding]::UTF8.GetBytes(($Record | ConvertTo-Json -Compress))
            $output.Write($bytes, 0, $bytes.Length)
            $output.Flush($true)
        } finally { $output.Dispose() }
        if ($Replace) { [IO.File]::Replace($temp, $path, $null) }
        else { [IO.File]::Move($temp, $path) }
    } finally { if (Test-Path -LiteralPath $temp) { Remove-Item -LiteralPath $temp -Force -ErrorAction SilentlyContinue } }
}

function Get-GipnyImageHash([string]$Path) {
    $item = Get-Item -LiteralPath $Path
    if ($item -isnot [IO.FileInfo] -or $item.Length -eq 0 -or $item.Length -gt 1GB -or
        ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'Invalid agent image.' }
    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

# Recover only known, hash-bound images. Do not find/kill a PID from a journal:
# after a bootstrap crash it may belong to somebody else. A locked image fails
# safely with the old backup retained for a later restart/manual inspection.
function Restore-GipnyAgentImage([string]$InstallDir) {
    $record = Read-GipnyUpdateJournal $InstallDir
    if (-not $record) { return }
    $path = Join-Path $InstallDir '.agent-update-state.json'
    $current = Join-Path $InstallDir 'gipny-agent.exe'
    $backup = Join-Path $InstallDir $record.Backup
    $currentHash = Get-GipnyImageHash $current
    if ($record.Accepted) {
        if ($currentHash -cne $record.Sha256) { throw 'Accepted update image changed; preserving recovery journal.' }
        if (Test-Path -LiteralPath $backup) {
            if ((Get-GipnyImageHash $backup) -cne $record.OldSha256) { throw 'Update backup checksum mismatch.' }
            Remove-Item -LiteralPath $backup -Force
        }
        Remove-Item -LiteralPath $path -Force
        return
    }
    if (-not (Test-Path -LiteralPath $backup)) {
        if ($currentHash -cne $record.OldSha256) { throw 'Unfinished update has no valid recovery image.' }
    } else {
        if ((Get-GipnyImageHash $backup) -cne $record.OldSha256) { throw 'Update backup checksum mismatch.' }
        if ($currentHash -ceq $record.Sha256) {
            $failed = Join-Path $InstallDir ('.gipny-agent-failed-' + [guid]::NewGuid().ToString('N') + '.exe')
            [IO.File]::Replace($backup, $current, $failed)
            Remove-Item -LiteralPath $failed -Force -ErrorAction SilentlyContinue
        } elseif ($currentHash -ceq $record.OldSha256) {
            Remove-Item -LiteralPath $backup -Force
        } else { throw 'Current agent image changed; preserving recovery journal.' }
    }
    Remove-Item -LiteralPath $path -Force
}

function Update-GipnyAgentImage {
    [CmdletBinding()]
    param([string]$InstallDir, [string]$DataDir)
    $pending = Join-Path $DataDir 'pending_agent'
    $ready = Join-Path $pending 'ready.json'
    if (-not (Test-Path -LiteralPath $ready -PathType Leaf)) { return $null }
    $next = $null
    try {
        if (Read-GipnyUpdateJournal $InstallDir) { throw 'An earlier update must be recovered before replacement.' }
        $readyItem = Get-Item -LiteralPath $ready
        if ($readyItem.Length -gt 65536 -or ($readyItem.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'Invalid update manifest.' }
        $manifest = Get-Content -LiteralPath $ready -Raw | ConvertFrom-Json
        if ($manifest.file -isnot [string] -or $manifest.sha256 -isnot [string] -or $manifest.version -isnot [string] -or
            $manifest.file -cnotmatch '^agent-[A-Za-z0-9_-]+\.exe$' -or
            $manifest.sha256 -cnotmatch '^[a-f0-9]{64}$' -or
            $manifest.version -cnotmatch '^[0-9]+\.[0-9]+\.[0-9]+([+-][A-Za-z0-9.+-]+)?$') { throw 'Invalid update manifest.' }
        $source = Join-Path $pending $manifest.file
        $null = Get-GipnyImageHash $source
        $current = Join-Path $InstallDir 'gipny-agent.exe'
        $suffix = [guid]::NewGuid().ToString('N')
        $next = Join-Path $InstallDir ('.gipny-agent-update-' + $suffix + '.exe')
        $backupName = '.gipny-agent-backup-' + $suffix + '.exe'
        $backup = Join-Path $InstallDir $backupName
        $output = [IO.File]::Open($next, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
        try {
            $inputFile = [IO.File]::OpenRead($source)
            try { $inputFile.CopyTo($output) } finally { $inputFile.Dispose() }
            $output.Flush($true)
        } finally { $output.Dispose() }
        if ((Get-GipnyImageHash $next) -cne $manifest.sha256) { throw 'Staged executable checksum mismatch.' }
        $record = @{ Schema = 1; Backup = $backupName; Sha256 = $manifest.sha256;
            OldSha256 = (Get-GipnyImageHash $current); Accepted = $false }
        Save-GipnyUpdateJournal $InstallDir $record
        [IO.File]::Replace($next, $current, $backup)
        # Pending source/manifest survive until readiness, as does old image.
        return @{ Applied = $true; Backup = $backup; Record = $record; Ready = $ready;
            Source = $source; File = $manifest.file; Sha256 = $manifest.sha256; Version = $manifest.version }
    } catch {
        # A journal written before a failed Replace is safe to recover too.
        [Console]::Error.WriteLine('GIPNY agent update could not be applied: ' + $_.Exception.Message)
        return $null
    } finally {
        if ($next -and (Test-Path -LiteralPath $next)) { Remove-Item -LiteralPath $next -Force -ErrorAction SilentlyContinue }
    }
}

function Complete-GipnyPending($Applied, [bool]$Accepted) {
    if (Test-Path -LiteralPath $Applied.Ready -PathType Leaf) {
        $item = Get-Item -LiteralPath $Applied.Ready
        if ($item.Length -le 65536 -and -not ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
            $manifest = Get-Content -LiteralPath $Applied.Ready -Raw | ConvertFrom-Json
            if ($manifest.file -ceq $Applied.File -and $manifest.sha256 -ceq $Applied.Sha256 -and $manifest.version -ceq $Applied.Version) {
                if ($Accepted) { Remove-Item -LiteralPath $Applied.Ready -Force }
                else {
                    $rejected = Join-Path (Split-Path -Parent $Applied.Ready) ('rejected-' + [guid]::NewGuid().ToString('N') + '.json')
                    [IO.File]::Move($Applied.Ready, $rejected)
                }
            }
        }
    }
    if ($Accepted) { Remove-Item -LiteralPath $Applied.Source -Force -ErrorAction SilentlyContinue }
}

function Wait-GipnyAgentReady {
    param([Diagnostics.Process]$Process, [string]$Path, [string]$Nonce, [int]$TimeoutSeconds)
    $watch = [Diagnostics.Stopwatch]::StartNew()
    while ($watch.Elapsed.TotalSeconds -lt $TimeoutSeconds) {
        if ($Process.HasExited) { throw 'Updated agent exited before readiness.' }
        if (Test-Path -LiteralPath $Path -PathType Leaf) {
            $item = Get-Item -LiteralPath $Path
            if ($item.Length -gt 256 -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'Invalid agent readiness marker.' }
            $expected = 'gipny-agent-ready-v1' + [char]10 + $Nonce + [char]10 + $Process.Id + [char]10
            if ([IO.File]::ReadAllText($Path) -cne $expected) { throw 'Agent readiness marker does not match this child.' }
            if ($Process.HasExited) { throw 'Updated agent exited at readiness.' }
            return
        }
        Start-Sleep -Milliseconds 100
    }
    throw 'Updated agent readiness timed out.'
}

function Start-GipnyAgent {
    param([string]$Configuration, [ValidateRange(1,3600)][int]$ReadinessTimeoutSeconds = 600)
    $ErrorActionPreference = 'Stop'
    $installDir = Split-Path -Parent ([IO.Path]::GetFullPath($Configuration))
    $lock = [IO.File]::Open((Join-Path $installDir '.start.lock'), [IO.FileMode]::OpenOrCreate,
        [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
    $marker = $null
    $process = $null
    try {
        $config = Get-Content -LiteralPath $Configuration -Raw | ConvertFrom-Json
        if (-not $config.DataDir -or -not $config.AgentArgs) { throw 'Reinstall the agent to create bootstrap configuration.' }
        Restore-GipnyAgentImage $installDir
        $applied = Update-GipnyAgentImage -InstallDir $installDir -DataDir $config.DataDir
        if (-not $applied) { Restore-GipnyAgentImage $installDir }
        $info = New-Object Diagnostics.ProcessStartInfo
        $info.FileName = Join-Path $installDir 'gipny-agent.exe'
        $info.WorkingDirectory = $config.DataDir
        $info.UseShellExecute = $false
        $info.Arguments = (($config.AgentArgs | ForEach-Object { Quote-GipnyArgument ([string]$_) }) -join ' ')
        # Never inherit stale tickets from a caller or pass them on argv.
        $info.EnvironmentVariables.Remove('GIPNY_AGENT_READY_PATH')
        $info.EnvironmentVariables.Remove('GIPNY_AGENT_READY_NONCE')
        if ($applied) {
            $random = New-Object byte[] 32
            $rng = [Security.Cryptography.RandomNumberGenerator]::Create()
            try { $rng.GetBytes($random) } finally { $rng.Dispose() }
            $nonce = ([BitConverter]::ToString($random)).Replace('-', '').ToLowerInvariant()
            $marker = Join-Path ([IO.Path]::GetFullPath($config.DataDir)) ('.gipny-agent-start-' + $nonce + '.ready')
            if (Test-Path -LiteralPath $marker) { throw 'Agent readiness path already exists.' }
            $info.EnvironmentVariables['GIPNY_AGENT_READY_PATH'] = $marker
            $info.EnvironmentVariables['GIPNY_AGENT_READY_NONCE'] = $nonce
        }
        try {
            $process = [Diagnostics.Process]::Start($info)
            if ($applied) {
                Wait-GipnyAgentReady -Process $process -Path $marker -Nonce $nonce -TimeoutSeconds $ReadinessTimeoutSeconds
                # Persist acceptance before deleting backup: recovery after a
                # bootstrap crash can distinguish a healthy committed update.
                $applied.Record.Accepted = $true
                Save-GipnyUpdateJournal $installDir $applied.Record -Replace
                try { Complete-GipnyPending $applied $true } catch { [Console]::Error.WriteLine('Accepted update staging cleanup failed; retaining recovery state.') }
                try { Restore-GipnyAgentImage $installDir } catch { [Console]::Error.WriteLine('Accepted update backup cleanup deferred until next start.') }
            }
        } catch {
            if (-not $applied) { throw }
            if ($process) {
                # This is the process handle returned above, never a PID/name
                # lookup or a kill of unrelated/background agent processes.
                if (-not $process.HasExited) { $process.Kill() }
                if (-not $process.WaitForExit(30000)) { throw 'Updated child did not exit; backup retained without replacing a running image.' }
                $process.Dispose(); $process = $null
            }
            Restore-GipnyAgentImage $installDir
            try { Complete-GipnyPending $applied $false }
            catch { [Console]::Error.WriteLine('Rejected staging cleanup failed; previous executable was restored.') }
            [Console]::Error.WriteLine('GIPNY agent update did not become ready; restored the previous executable.')
            $info.EnvironmentVariables.Remove('GIPNY_AGENT_READY_PATH')
            $info.EnvironmentVariables.Remove('GIPNY_AGENT_READY_NONCE')
            $process = [Diagnostics.Process]::Start($info)
        }
        $process.WaitForExit()
        return $process.ExitCode
    } finally {
        if ($process) { $process.Dispose() }
        if ($marker) {
            Remove-Item -LiteralPath $marker -Force -ErrorAction SilentlyContinue
            Remove-Item -LiteralPath ([IO.Path]::ChangeExtension($marker, 'tmp')) -Force -ErrorAction SilentlyContinue
        }
        $lock.Dispose()
    }
}

if ($MyInvocation.InvocationName -ne '.') {
    try { exit (Start-GipnyAgent -Configuration $ConfigPath) }
    catch { [Console]::Error.WriteLine($_.Exception.Message); exit 1 }
}
