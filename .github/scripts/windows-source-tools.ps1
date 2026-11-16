# Use the Visual Studio installation on the runner directly, without a
# JavaScript action runtime. OpenSSL also needs native Windows Perl rather
# than Git Bash/MSYS Perl and its incompatible module/path handling.
param([ValidateSet('x64', 'arm64')][string]$TargetArchitecture)

$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_OS -ne 'Windows') {
    throw 'This build-tool setup runs only on GitHub Actions Windows runners.'
}
if (-not $env:GITHUB_ENV -or -not $env:GITHUB_PATH) {
    throw 'GitHub runner environment files are unavailable.'
}
$hostArchitecture = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString().ToLowerInvariant()
if ($hostArchitecture -notin @('x64', 'arm64') -or $env:RUNNER_ARCH.ToLowerInvariant() -ne $hostArchitecture) {
    throw 'The native Windows OS architecture must match an x64 or ARM64 GitHub runner.'
}
if (-not $TargetArchitecture) { $TargetArchitecture = $hostArchitecture }
$targetTriple = if ($TargetArchitecture -eq 'arm64') { 'aarch64-pc-windows-msvc' } else { 'x86_64-pc-windows-msvc' }
$vsTarget = if ($TargetArchitecture -eq 'x64') { 'amd64' } else { 'arm64' }
$vsHost = if ($hostArchitecture -eq 'x64') { 'amd64' } else { 'arm64' }
$component = if ($TargetArchitecture -eq 'arm64') { 'Microsoft.VisualStudio.Component.VC.Tools.ARM64' } else { 'Microsoft.VisualStudio.Component.VC.Tools.x86.x64' }
$originalPaths = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::OrdinalIgnoreCase)
foreach ($entry in ($env:PATH -split ';')) { [void]$originalPaths.Add($entry) }
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
if (-not (Test-Path -LiteralPath $vswhere -PathType Leaf)) {
    throw 'The GitHub Windows image must provide Microsoft vswhere.'
}
$installations = @(& $vswhere -latest -products '*' -requires $component -property installationPath -utf8)
if ($LASTEXITCODE -ne 0 -or $installations.Count -ne 1 -or [string]::IsNullOrWhiteSpace($installations[0])) {
    throw "A Visual Studio installation with $TargetArchitecture C++ tools is required."
}
$installation = $installations[0].Trim()
$launch = Join-Path $installation 'Common7\Tools\Launch-VsDevShell.ps1'
if (-not (Test-Path -LiteralPath $launch -PathType Leaf)) {
    throw 'The selected Visual Studio installation lacks Microsoft Launch-VsDevShell.ps1.'
}
# Microsoft recommends this entry point for build automation. Keep the
# checkout as the working directory and select both host and target explicitly.
& $launch -Arch $vsTarget -HostArch $vsHost -SkipAutomaticLocation | Out-Null
foreach ($name in @('VCToolsInstallDir', 'VCToolsVersion', 'VCINSTALLDIR', 'VSINSTALLDIR', 'INCLUDE', 'LIB', 'WindowsSdkDir', 'WindowsSDKVersion')) {
    if ([string]::IsNullOrWhiteSpace([Environment]::GetEnvironmentVariable($name, 'Process'))) {
        throw "Visual Studio initialization did not provide required toolchain variable $name."
    }
}
if (($env:VSCMD_ARG_HOST_ARCH -replace '^amd64$', 'x64') -ne $hostArchitecture -or
    ($env:VSCMD_ARG_TGT_ARCH -replace '^amd64$', 'x64') -ne $TargetArchitecture) {
    throw 'Visual Studio initialized an unexpected host or target architecture.'
}
$installationRoot = [IO.Path]::GetFullPath($installation).TrimEnd('\') + '\'
$toolsRoot = [IO.Path]::GetFullPath($env:VCToolsInstallDir)
if (-not $toolsRoot.StartsWith($installationRoot, [StringComparison]::OrdinalIgnoreCase)) {
    throw 'The active MSVC toolchain does not belong to the selected Visual Studio installation.'
}
$nativeBin = Join-Path $env:VCToolsInstallDir "bin\Host$hostArchitecture\$TargetArchitecture"
$nativeCl = Join-Path $nativeBin 'cl.exe'
$nativeLink = Join-Path $nativeBin 'link.exe'
foreach ($tool in @($nativeCl, $nativeLink)) {
    if (-not (Test-Path -LiteralPath $tool -PathType Leaf)) {
        throw 'The initialized MSVC toolchain lacks its requested compiler or linker.'
    }
}
if (-not [string]::Equals((Get-Command cl.exe -CommandType Application -ErrorAction Stop).Source, $nativeCl, [StringComparison]::OrdinalIgnoreCase)) {
    throw 'The developer environment selected a compiler outside the requested MSVC toolchain.'
}
# Persist only build-tool settings, never dump/export the process environment:
# it may also contain unrelated repository credentials supplied to the runner.
$toolchainVariables = @(
    'INCLUDE', 'LIB', 'LIBPATH', 'VCINSTALLDIR', 'VCToolsInstallDir',
    'VSINSTALLDIR', 'VisualStudioVersion', 'WindowsSdkDir', 'WindowsSDKVersion',
    'WindowsSDKLibVersion', 'WindowsSdkBinPath', 'WindowsLibPath',
    'UniversalCRTSdkDir', 'UCRTVersion', 'ExtensionSdkDir',
    'VSCMD_ARG_HOST_ARCH', 'VSCMD_ARG_TGT_ARCH', 'VSCMD_VER'
)
foreach ($name in $toolchainVariables) {
    $value = [Environment]::GetEnvironmentVariable($name, 'Process')
    if (-not [string]::IsNullOrEmpty($value)) {
        if ($value -match "[\r\n]") { throw "Multiline toolchain variable is unsupported: $name" }
        "$name=$value" | Out-File -FilePath $env:GITHUB_ENV -Append -Encoding utf8
    }
}
$addedPaths = [System.Collections.Generic.List[string]]::new()
$seenPaths = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::OrdinalIgnoreCase)
foreach ($entry in (@($nativeBin) + @($env:PATH -split ';'))) {
    if ($entry -and ($entry -eq $nativeBin -or -not $originalPaths.Contains($entry)) -and $seenPaths.Add($entry)) {
        if ($entry -match "[\r\n]") { throw 'Multiline toolchain PATH entry is unsupported.' }
        $addedPaths.Add($entry)
    }
}
# GITHUB_PATH prepends later entries first; reverse to retain VS tool ordering.
for ($index = $addedPaths.Count - 1; $index -ge 0; $index--) {
    $addedPaths[$index] | Out-File -FilePath $env:GITHUB_PATH -Append -Encoding utf8
}
$perl = $null
$perlCandidates = @('C:\Strawberry\perl\bin\perl.exe') + @(
    Get-Command perl.exe -CommandType Application -All -ErrorAction SilentlyContinue | ForEach-Object { $_.Source }
)
foreach ($candidate in ($perlCandidates | Select-Object -Unique)) {
    if (-not (Test-Path -LiteralPath $candidate -PathType Leaf)) { continue }
    & $candidate -MIPC::Cmd -MLocale::Maketext::Simple -e 'exit($^O eq q(MSWin32) ? 0 : 1)' 2>$null
    if ($LASTEXITCODE -eq 0) { $perl = $candidate; break }
}
if (-not $perl) { throw 'Native Windows Perl with OpenSSL Configure modules is required; MSYS Perl is not supported.' }
# Git Bash also ships an unrelated link.exe. Cargo must use the actual MSVC
# executable even when a login shell puts Git utilities first on PATH.
$linkerVariable = 'CARGO_TARGET_' + $targetTriple.ToUpperInvariant().Replace('-', '_') + '_LINKER'
"$linkerVariable=$nativeLink" | Out-File -FilePath $env:GITHUB_ENV -Append -Encoding utf8
foreach ($entry in @("CARGO_BUILD_TARGET=$targetTriple", "GIPNY_WINDOWS_TARGET=$targetTriple", "GIPNY_WINDOWS_ARCH=$TargetArchitecture", "GIPNY_WINDOWS_HOST_ARCH=$hostArchitecture", "GIPNY_MSVC_TOOLSET=$($env:VCToolsVersion)")) {
    if ($entry -match "[\r\n]") { throw 'Invalid multiline Windows toolchain metadata.' }
    $entry | Out-File -FilePath $env:GITHUB_ENV -Append -Encoding utf8
}
"OPENSSL_SRC_PERL=$perl" | Out-File -FilePath $env:GITHUB_ENV -Append -Encoding utf8
"PERL=$perl" | Out-File -FilePath $env:GITHUB_ENV -Append -Encoding utf8
(Split-Path -Parent $perl) | Out-File -FilePath $env:GITHUB_PATH -Append -Encoding utf8
Write-Host "Installed MSVC $hostArchitecture host / $TargetArchitecture target initialized; Windows Perl and linker selected."
