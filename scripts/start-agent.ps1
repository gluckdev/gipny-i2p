# Task Scheduler starts this bootstrap before the executable, so Windows never
# has to overwrite a running image. Installation/data ACLs are set by installer.
[CmdletBinding()]
param([string]$ConfigPath = (Join-Path $PSScriptRoot 'install.json'))

function Quote-GipnyArgument([string]$Value) {
    '"' + ([regex]::Replace([regex]::Replace($Value, '(\\*)"', '$1$1\"'), '(\\+)$', '$1$1')) + '"'
}

function Update-GipnyAgentImage {
    [CmdletBinding()]
    param([string]$InstallDir, [string]$DataDir)
    $pending = Join-Path $DataDir 'pending_agent'
    $ready = Join-Path $pending 'ready.json'
    if (-not (Test-Path -LiteralPath $ready -PathType Leaf)) { return $null }
    $next = $null
    $backup = $null
    try {
        if ((Get-Item -LiteralPath $ready).Length -gt 65536) { throw 'Oversized update manifest.' }
        $manifest = Get-Content -LiteralPath $ready -Raw | ConvertFrom-Json
        if ($manifest.file -isnot [string] -or $manifest.sha256 -isnot [string] -or $manifest.version -isnot [string] -or
            $manifest.file -cnotmatch '^agent-[A-Za-z0-9_-]+\.exe$' -or
            $manifest.sha256 -cnotmatch '^[a-f0-9]{64}$' -or
            $manifest.version -cnotmatch '^[0-9]+\.[0-9]+\.[0-9]+([+-][A-Za-z0-9.+-]+)?$') {
            throw 'Invalid update manifest.'
        }
        $source = Join-Path $pending $manifest.file
        $item = Get-Item -LiteralPath $source
        if (-not ($item -is [IO.FileInfo]) -or $item.Length -eq 0 -or
            $item.Length -gt 1GB -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
            throw 'Invalid staged executable.'
        }
        $current = Join-Path $InstallDir 'gipny-agent.exe'
        $suffix = [guid]::NewGuid().ToString('N')
        $next = Join-Path $InstallDir ('.gipny-agent-update-' + $suffix + '.exe')
        $backup = Join-Path $InstallDir ('.gipny-agent-backup-' + $suffix + '.exe')
        $output = [IO.File]::Open($next, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
        try {
            $inputFile = [IO.File]::OpenRead($source)
            try { $inputFile.CopyTo($output) } finally { $inputFile.Dispose() }
            $output.Flush($true)
        } finally { $output.Dispose() }
        if ((Get-FileHash -LiteralPath $next -Algorithm SHA256).Hash.ToLowerInvariant() -cne $manifest.sha256) {
            throw 'Staged executable checksum mismatch.'
        }
        # Same-volume atomic replacement. On failure current and ready remain.
        [IO.File]::Replace($next, $current, $backup)
        Remove-Item -LiteralPath $ready -Force -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath $source -Force -ErrorAction SilentlyContinue
        return @{ Applied = $true; Backup = $backup }
    } catch {
        # Keep the old executable and pending request usable on the next start.
        [Console]::Error.WriteLine('GIPNY agent update could not be applied: ' + $_.Exception.Message)
        return $null
    } finally {
        if ($next -and (Test-Path -LiteralPath $next)) { Remove-Item -LiteralPath $next -Force -ErrorAction SilentlyContinue }
    }
}

function Start-GipnyAgent {
    param([string]$Configuration)
    $ErrorActionPreference = 'Stop'
    $installDir = Split-Path -Parent ([IO.Path]::GetFullPath($Configuration))
    # A second manual/bootstrap start must not race replacement or open profile.
    $lock = [IO.File]::Open((Join-Path $installDir '.start.lock'), [IO.FileMode]::OpenOrCreate,
        [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
    try {
        $config = Get-Content -LiteralPath $Configuration -Raw | ConvertFrom-Json
        if (-not $config.DataDir -or -not $config.AgentArgs) { throw 'Reinstall the agent to create bootstrap configuration.' }
        $applied = Update-GipnyAgentImage -InstallDir $installDir -DataDir $config.DataDir
        $info = New-Object Diagnostics.ProcessStartInfo
        $info.FileName = Join-Path $installDir 'gipny-agent.exe'
        $info.WorkingDirectory = $config.DataDir
        $info.UseShellExecute = $false
        $info.Arguments = (($config.AgentArgs | ForEach-Object { Quote-GipnyArgument ([string]$_) }) -join ' ')
        try { $process = [Diagnostics.Process]::Start($info) }
        catch {
            if (-not $applied) { throw }
            # A valid hash does not guarantee a loadable executable. Restore
            # the previous image if Windows cannot even launch the new one.
            $failed = Join-Path $installDir ('.gipny-agent-failed-' + [guid]::NewGuid().ToString('N') + '.exe')
            [IO.File]::Replace($applied.Backup, $info.FileName, $failed)
            Remove-Item -LiteralPath $failed -Force -ErrorAction SilentlyContinue
            [Console]::Error.WriteLine('GIPNY agent update could not start; restored the previous executable.')
            $process = [Diagnostics.Process]::Start($info)
            $applied = $null
        }
        if ($applied) { Remove-Item -LiteralPath $applied.Backup -Force -ErrorAction SilentlyContinue }
        try {
            $process.WaitForExit()
            return $process.ExitCode
        } finally { $process.Dispose() }
    } finally { $lock.Dispose() }
}

if ($MyInvocation.InvocationName -ne '.') {
    try { exit (Start-GipnyAgent -Configuration $ConfigPath) }
    catch { [Console]::Error.WriteLine($_.Exception.Message); exit 1 }
}
