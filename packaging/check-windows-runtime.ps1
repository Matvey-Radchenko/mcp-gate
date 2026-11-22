$ErrorActionPreference = 'Stop'
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
$arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString().ToLowerInvariant()
$dumpbin = & $vswhere -latest -find "VC/Tools/MSVC/**/bin/Host$arch/$arch/dumpbin.exe" | Select-Object -Last 1
if (-not $dumpbin -and $arch -eq 'arm64') {
    # The x64 tool can inspect ARM64 executables under Windows emulation.
    $dumpbin = & $vswhere -latest -find 'VC/Tools/MSVC/**/bin/Hostx64/x64/dumpbin.exe' | Select-Object -Last 1
}
if (-not $dumpbin) { throw 'Cannot inspect release DLL dependencies: dumpbin is unavailable' }
$output = & $dumpbin /DEPENDENTS target/release/mcp-gate.exe
if ($LASTEXITCODE -ne 0) { throw 'Cannot inspect the native release executable' }
$output | Write-Output
if (($output -join "`n") -match '(?i)\b(?:vcruntime\d+(?:_\d+)?d?|msvcp\d+(?:_\d+)?d?|ucrtbased)\.dll\b') {
    throw 'Release requires an external VC runtime; rebuild with static CRT linkage'
}
Write-Output 'Native release does not require a separate Visual C++ redistributable'
