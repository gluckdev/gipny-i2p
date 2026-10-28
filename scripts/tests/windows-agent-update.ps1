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
    $result = Update-GipnyAgentImage $install $data
    Assert-True ($result.Applied) 'Valid update was not installed.'
    Assert-True (([IO.File]::ReadAllText($current)) -ceq 'new image') 'Wrong image installed.'
    Assert-True (([IO.File]::ReadAllText($result.Backup)) -ceq 'old image') 'Backup did not preserve previous image.'
    Assert-True (-not (Test-Path $ready)) 'Applied update stayed pending.'
    Assert-True (-not (Test-Path $source)) 'Applied staging executable was not removed.'
    Assert-True (-not (Update-GipnyAgentImage $install $data)) 'Applied update ran twice.'
    Assert-True (@(Get-ChildItem $install -Filter '.gipny-agent-update-*').Count -eq 0) 'Owned temporary files leaked.'
    # Exercise the actual native launch failure and bootstrap rollback, not
    # merely a synthetic File.Replace. where.exe is a harmless system fixture.
    Copy-Item (Join-Path $env:SystemRoot 'System32\where.exe') $current -Force
    $oldDigest = (Get-FileHash $current -Algorithm SHA256).Hash
    [IO.File]::WriteAllText($source, 'not a loadable Windows executable')
    Stage ((Get-FileHash $source -Algorithm SHA256).Hash.ToLowerInvariant())
    $config = Join-Path $install 'install.json'
    @{DataDir = $data; AgentArgs = @('/q', 'cmd.exe')} | ConvertTo-Json | Set-Content $config -Encoding UTF8
    $code = Start-GipnyAgent -Configuration $config
    Assert-True ($code -eq 0) 'Restored executable did not start successfully.'
    Assert-True ((Get-FileHash $current -Algorithm SHA256).Hash -ceq $oldDigest) 'Launch failure did not restore the old executable.'
    Assert-True (@(Get-ChildItem $install -Filter '.gipny-agent-failed-*').Count -eq 0) 'Failed replacement image leaked.'
    Write-Host 'Windows agent checksum/staging/atomic replacement/locked image regressions: PASS'
} finally { Remove-Item $root -Recurse -Force }
