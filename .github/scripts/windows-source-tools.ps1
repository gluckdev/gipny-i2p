# Rust's vendored OpenSSL must use native Windows Perl, not Git Bash/MSYS
# Perl (whose partial module set and Unix path rules break VC-WIN64A).
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_OS -ne 'Windows') {
    throw 'This build-tool setup runs only on GitHub Actions Windows runners.'
}
$perl = 'C:\Strawberry\perl\bin\perl.exe'
if (-not (Test-Path -LiteralPath $perl -PathType Leaf)) {
    throw 'The GitHub Windows image must provide native Strawberry Perl for OpenSSL source builds.'
}
& $perl -MIPC::Cmd -MLocale::Maketext::Simple -e 'exit($^O eq q(MSWin32) ? 0 : 1)'
if ($LASTEXITCODE -ne 0) { throw 'Native Perl or required OpenSSL Configure modules are unavailable.' }
"OPENSSL_SRC_PERL=$perl" | Out-File -FilePath $env:GITHUB_ENV -Append -Encoding utf8
"PERL=$perl" | Out-File -FilePath $env:GITHUB_ENV -Append -Encoding utf8
(Split-Path -Parent $perl) | Out-File -FilePath $env:GITHUB_PATH -Append -Encoding utf8
Write-Host 'Native Windows Perl selected for source builds; MSYS Perl is excluded.'
