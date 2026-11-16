# Disposable shipping CI: verify payloads and the app installed by our NSIS.
param(
    [Parameter(Mandatory=$true)][string]$BuildDirectory,
    [Parameter(Mandatory=$true)][ValidateSet('x64','arm64')][string]$Architecture
)
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_OS -ne 'Windows' -or -not $env:RUNNER_TEMP) {
    throw 'Windows package verification requires an isolated GitHub Windows runner.'
}
$native = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString().ToLowerInvariant()
if ($native -ne $Architecture -or $env:GIPNY_WINDOWS_ARCH -ne $Architecture) {
    throw 'Shipping verification must run on the actual target OS architecture.'
}
$build = (Resolve-Path -LiteralPath $BuildDirectory).Path
$app = Join-Path $build 'gipny.exe'
$agent = Join-Path $build 'gipny-agent.exe'
$setups = @(Get-ChildItem -LiteralPath (Join-Path $build 'bundle\nsis') -Filter '*-setup.exe' -File)
if ($setups.Count -ne 1 -or -not $setups[0].Name.EndsWith("_${Architecture}-setup.exe", [StringComparison]::Ordinal)) {
    throw 'Expected exactly one installer with the target architecture name.'
}
$receiptDir = Join-Path $PWD 'windows-verification'
[void](New-Item -ItemType Directory -Force -Path $receiptDir)
$receipt = Join-Path $receiptDir 'native-payloads.json'
$installDir = Join-Path $env:RUNNER_TEMP ('gipny-install-' + [Guid]::NewGuid().ToString('N'))
# /D must be the final, unquoted NSIS switch; its entire suffix is the path.
$installer = Start-Process -FilePath $setups[0].FullName -ArgumentList "/S /D=$installDir" -PassThru
if (-not $installer.WaitForExit(300000)) {
    $installer.Kill()
    throw 'NSIS installation exceeded the five-minute deadline.'
}
if ($installer.ExitCode -ne 0) { throw "NSIS installer failed: $($installer.ExitCode)" }
$installed = Join-Path $installDir 'gipny.exe'
foreach ($file in @($app, $agent, $installed)) {
    if (-not (Test-Path -LiteralPath $file -PathType Leaf)) { throw "Missing native payload: $file" }
}
$appHash = (Get-FileHash -LiteralPath $app -Algorithm SHA256).Hash
if ((Get-FileHash -LiteralPath $installed -Algorithm SHA256).Hash -ne $appHash) {
    throw 'The installed app is not the exact executable built in this job.'
}
$arguments = @('scripts/verify-windows-pe.py', '--architecture', $Architecture,
    '--receipt', $receipt, '--file', $app, '--file', $agent, '--file', $installed)
# App-local DLLs are payloads. The x86 NSIS uninstaller is not a native app.
foreach ($dll in @(Get-ChildItem -LiteralPath $installDir -Filter '*.dll' -File -Recurse)) {
    $arguments += @('--file', $dll.FullName)
}
& python3 @arguments
if ($LASTEXITCODE -ne 0) { throw 'Native PE architecture verification failed.' }
$result = Get-Content -LiteralPath $receipt -Raw | ConvertFrom-Json
$result | Add-Member -NotePropertyName installer -NotePropertyValue @{
    name = $setups[0].Name
    sha256 = (Get-FileHash -LiteralPath $setups[0].FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    installed_app_sha256 = $appHash.ToLowerInvariant()
    silent_install_passed = $true
    profile_runtime_tested = $false
}
$result | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $receipt -Encoding utf8
Write-Host 'Installed app equals the built payload; native PE architecture verified. Profile runtime is a separate gate.'
