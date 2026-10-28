$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot '..\start-agent.ps1')
. (Join-Path $PSScriptRoot '..\install-agent.ps1')

function Assert-True($Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
}
function Assert-Throws([scriptblock]$Action) {
    $threw = $false
    try { $null = & $Action } catch { $threw = $true }
    Assert-True $threw 'Expected failure did not occur.'
}

$hash = 'a' * 64
Assert-True ((Get-GipnyReleaseChecksum "$hash  agent.tar.gz`n" 'agent.tar.gz') -ceq $hash) 'Valid manifest rejected.'
Assert-True ((Get-GipnyReleaseChecksum "$($hash.ToUpperInvariant()) *./agent.tar.gz" 'agent.tar.gz') -ceq $hash) 'Binary/uppercase checksum rejected.'
Assert-Throws { Get-GipnyReleaseChecksum "$hash  other.tar.gz" 'agent.tar.gz' }
Assert-Throws { Get-GipnyReleaseChecksum "$hash  agent.tar.gz`n$hash  agent.tar.gz" 'agent.tar.gz' }
Assert-Throws { Get-GipnyReleaseChecksum "bad  agent.tar.gz" 'agent.tar.gz' }

$root = Join-Path ([IO.Path]::GetTempPath()) ('gipny-bootstrap-test-' + [guid]::NewGuid().ToString('N'))
$install = Join-Path $root 'installed'
$data = Join-Path $root 'data'
$pending = Join-Path $data 'pending_agent'
New-Item $install, $pending -ItemType Directory -Force | Out-Null
$current = Join-Path $install 'gipny-agent.exe'
$source = Join-Path $pending 'agent-synthetic.exe'
$ready = Join-Path $pending 'ready.json'
function Stage([string]$Checksum, [string]$Filename = 'agent-synthetic.exe') {
    @{file = $Filename; sha256 = $Checksum; version = '0.4.19'} | ConvertTo-Json | Set-Content $ready -Encoding UTF8
}
# These real managed child fixtures are compiled by Windows GitHub Actions,
# never downloaded from an opaque binary source or compiled on the workstation.
$fixtures = @{}
$fixtureDir = Join-Path $root 'fixtures'
New-Item $fixtureDir -ItemType Directory -Force | Out-Null
foreach ($mode in @('old', 'ready', 'crash', 'timeout', 'badnonce', 'badpid')) {
    $code = @'
using System;
using System.IO;
using System.Text;
using System.Threading;
public static class AgentFixtureMODE_TOKEN {
    public static int Main(string[] args) {
        string mode = "MODE_TOKEN";
        string path = Environment.GetEnvironmentVariable("GIPNY_AGENT_READY_PATH");
        string nonce = Environment.GetEnvironmentVariable("GIPNY_AGENT_READY_NONCE");
        File.AppendAllText(Path.Combine(Environment.CurrentDirectory, "launches.txt"), mode + "\n");
        if (mode == "old") return path == null && nonce == null ? 0 : 91;
        if (mode == "crash") return 17;
        if (mode == "timeout") { Thread.Sleep(15000); return 19; }
        if (path == null || nonce == null || nonce.Length != 64) return 92;
        foreach (string arg in args) if (arg.Contains(nonce) || arg.Contains(path)) return 93;
        int pid = System.Diagnostics.Process.GetCurrentProcess().Id;
        string body = "gipny-agent-ready-v1\n" + (mode == "badnonce" ? new String('0', 64) : nonce)
            + "\n" + (mode == "badpid" ? pid + 1 : pid) + "\n";
        string temp = Path.ChangeExtension(path, "tmp");
        using (FileStream file = new FileStream(temp, FileMode.CreateNew, FileAccess.Write, FileShare.None)) {
            byte[] bytes = Encoding.UTF8.GetBytes(body); file.Write(bytes, 0, bytes.Length); file.Flush(true);
        }
        File.Move(temp, path);
        Thread.Sleep(mode == "ready" ? 1000 : 15000);
        return 0;
    }
}
'@
    $binary = Join-Path $fixtureDir ($mode + '.exe')
    Add-Type -TypeDefinition ($code.Replace('MODE_TOKEN', $mode)) -OutputAssembly $binary -OutputType ConsoleApplication
    $fixtures[$mode] = $binary
}
$config = Join-Path $install 'install.json'
@{DataDir = $data; AgentArgs = @('fixture', 'argument with spaces')} | ConvertTo-Json | Set-Content $config -Encoding UTF8
function Run-StartupCase([string]$Mode, [int]$Timeout = 5) {
    Copy-Item $fixtures.old $current -Force
    $oldDigest = (Get-FileHash $current -Algorithm SHA256).Hash
    Copy-Item $fixtures[$Mode] $source -Force
    $newDigest = (Get-FileHash $source -Algorithm SHA256).Hash.ToLowerInvariant()
    Stage $newDigest
    $code = Start-GipnyAgent -Configuration $config -ReadinessTimeoutSeconds $Timeout
    Assert-True ($code -eq 0) ('Child/rollback failed for ' + $Mode)
    $expected = if ($Mode -eq 'ready') { $newDigest.ToUpperInvariant() } else { $oldDigest }
    Assert-True ((Get-FileHash $current -Algorithm SHA256).Hash -ceq $expected) ('Wrong installed image after ' + $Mode)
    Assert-True (-not (Test-Path $ready)) ('Attempted update remained automatically runnable after ' + $Mode)
    Assert-True (-not (Test-Path (Join-Path $install '.agent-update-state.json'))) ('Recovery journal leaked after ' + $Mode)
    Assert-True (@(Get-ChildItem $install -Filter '.gipny-agent-backup-*').Count -eq 0) ('Backup was not resolved after ' + $Mode)
    Assert-True (@(Get-ChildItem $data -Filter '.gipny-agent-start-*').Count -eq 0) ('Readiness marker leaked after ' + $Mode)
    if ($Mode -eq 'ready') { Assert-True (-not (Test-Path $source)) 'Accepted staging image remained.' }
    else { Assert-True (Test-Path $source) 'Rejected package was lost instead of retained for inspection.' }
}
$bystander = $null
try {
    [IO.File]::WriteAllText($current, 'old image')
    [IO.File]::WriteAllText($source, 'new image')
    $digest = (Get-FileHash $source -Algorithm SHA256).Hash.ToLowerInvariant()
    Stage $hash
    $result = Update-GipnyAgentImage $install $data
    Assert-True (-not $result) 'Bad checksum was installed.'
    Assert-True (([IO.File]::ReadAllText($current)) -ceq 'old image') 'Old image changed after checksum failure.'
    Assert-True (Test-Path $ready) 'Checksum failure lost pending update.'
    Stage $digest '..\escape.exe'
    Assert-True (-not (Update-GipnyAgentImage $install $data)) 'Traversal filename accepted.'
    Stage $digest
    $held = [IO.File]::Open($current, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
    try {
        Assert-True (-not (Update-GipnyAgentImage $install $data)) 'Running/locked image was replaced.'
        Assert-True (Test-Path $ready) 'Locked image failure lost pending update.'
        Assert-True (([IO.File]::ReadAllText($current)) -ceq 'old image') 'Locked image changed.'
    } finally { $held.Dispose() }
    Restore-GipnyAgentImage $install
    $result = Update-GipnyAgentImage $install $data
    Assert-True ($result.Applied) 'Valid update was not installed.'
    Assert-True (([IO.File]::ReadAllText($current)) -ceq 'new image') 'Wrong image installed.'
    Assert-True (([IO.File]::ReadAllText($result.Backup)) -ceq 'old image') 'Backup did not preserve previous image.'
    Assert-True (Test-Path $ready) 'Enqueue/replacement removed manifest before readiness.'
    Assert-True (Test-Path $source) 'Enqueue/replacement removed source before readiness.'
    $result.Record.Accepted = $true
    Save-GipnyUpdateJournal $install $result.Record -Replace
    Complete-GipnyPending $result $true
    Restore-GipnyAgentImage $install
    Assert-True (-not (Test-Path $ready)) 'Accepted update stayed pending.'
    Assert-True (-not (Test-Path $result.Backup)) 'Accepted backup cleanup failed.'
    Assert-True (-not (Update-GipnyAgentImage $install $data)) 'Accepted update ran twice.'

    # Actual native loader failure: old managed child must restart without a
    # readiness ticket. A matching SHA256 alone cannot make a binary loadable.
    Copy-Item $fixtures.old $current -Force
    $oldDigest = (Get-FileHash $current -Algorithm SHA256).Hash
    [IO.File]::WriteAllText($source, 'not a loadable Windows executable')
    Stage ((Get-FileHash $source -Algorithm SHA256).Hash.ToLowerInvariant())
    Assert-True ((Start-GipnyAgent -Configuration $config -ReadinessTimeoutSeconds 5) -eq 0) 'Loader failure did not restart old child.'
    Assert-True ((Get-FileHash $current -Algorithm SHA256).Hash -ceq $oldDigest) 'Loader failure did not restore old image.'

    Run-StartupCase ready
    Run-StartupCase crash
    Run-StartupCase badnonce
    Run-StartupCase badpid
    # A different process with the same executable basename must survive the
    # updated-child timeout. Only our Process.Start handle may be terminated.
    $bystanderDir = Join-Path $root 'bystander'
    New-Item $bystanderDir -ItemType Directory -Force | Out-Null
    $bystanderExe = Join-Path $bystanderDir 'gipny-agent.exe'
    Copy-Item $fixtures.timeout $bystanderExe
    $info = New-Object Diagnostics.ProcessStartInfo
    $info.FileName = $bystanderExe; $info.WorkingDirectory = $data; $info.UseShellExecute = $false
    $info.EnvironmentVariables.Remove('GIPNY_AGENT_READY_PATH'); $info.EnvironmentVariables.Remove('GIPNY_AGENT_READY_NONCE')
    $bystander = [Diagnostics.Process]::Start($info)
    Run-StartupCase timeout 1
    Assert-True (-not $bystander.HasExited) 'Timeout killed an unrelated agent process.'
    $bystander.Kill(); $bystander.WaitForExit(); $bystander.Dispose(); $bystander = $null

    # Simulate bootstrap death after Replace but before a child/handshake.
    Copy-Item $fixtures.old $current -Force
    $oldDigest = (Get-FileHash $current -Algorithm SHA256).Hash
    Copy-Item $fixtures.crash $source -Force
    Stage ((Get-FileHash $source -Algorithm SHA256).Hash.ToLowerInvariant())
    $result = Update-GipnyAgentImage $install $data
    Assert-True (Test-Path $result.Backup) 'Interrupted update has no recovery image.'
    Restore-GipnyAgentImage $install
    Assert-True ((Get-FileHash $current -Algorithm SHA256).Hash -ceq $oldDigest) 'Interrupted bootstrap did not restore old image.'
    Assert-True (Test-Path $ready) 'Recovery lost the pending request.'
    Assert-True ((Start-GipnyAgent -Configuration $config -ReadinessTimeoutSeconds 5) -eq 0) 'Recovered request did not safely roll back its failing child.'

    # Never follow a journal-controlled traversal or restore tampered bytes.
    $journal = Join-Path $install '.agent-update-state.json'
    @{Schema = 1; Backup = '..\outside.exe'; Sha256 = $hash; OldSha256 = $hash; Accepted = $false} |
        ConvertTo-Json | Set-Content $journal -Encoding UTF8
    Assert-Throws { Restore-GipnyAgentImage $install }
    Assert-True ((Get-FileHash $current -Algorithm SHA256).Hash -ceq $oldDigest) 'Invalid journal changed executable.'
    Remove-Item $journal -Force
    Copy-Item $fixtures.crash $source -Force
    Stage ((Get-FileHash $source -Algorithm SHA256).Hash.ToLowerInvariant())
    $result = Update-GipnyAgentImage $install $data
    [IO.File]::WriteAllText($result.Backup, 'tampered backup')
    Assert-Throws { Restore-GipnyAgentImage $install }
    Assert-True (Test-Path $journal) 'Tampered backup discarded recovery journal.'
    Copy-Item $fixtures.old $result.Backup -Force
    Restore-GipnyAgentImage $install
    Assert-True ((Get-FileHash $current -Algorithm SHA256).Hash -ceq $oldDigest) 'Known recovery image was not restored.'
    Assert-True (@(Get-ChildItem $install -Filter '.gipny-agent-update-*').Count -eq 0) 'Owned temporary files leaked.'
    Assert-True (@(Get-ChildItem $install -Filter '.gipny-agent-failed-*').Count -eq 0) 'Failed replacement image leaked.'
    Write-Host 'Windows agent checksum/replacement/readiness/rollback/recovery regressions: PASS'
} finally {
    if ($bystander) { if (-not $bystander.HasExited) { $bystander.Kill(); $bystander.WaitForExit() }; $bystander.Dispose() }
    Remove-Item $root -Recurse -Force
}
