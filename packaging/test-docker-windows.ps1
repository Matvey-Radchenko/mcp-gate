# CI-only infrastructure. The gateway and Docker CLI still run natively on Windows.
param([switch]$Cleanup)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
$name = $env:MCP_GATE_WSL_DISTRO
if ($name -notmatch '^mcp-gate-ci-[0-9]+-[0-9]+$') { throw 'Expected a dedicated CI distribution name' }
$owner = Join-Path $env:RUNNER_TEMP 'mcp-gate-wsl-owner.txt'
function Distributions {
    $names = & wsl.exe --list --quiet
    if ($LASTEXITCODE -ne 0) { throw 'Cannot inspect WSL distribution ownership' }
    @($names -replace '\x00', '' | ForEach-Object { $_.Trim([char]0xfeff).Trim() })
}
if ($Cleanup) {
    if (Test-Path -LiteralPath $owner) {
        if ([IO.File]::ReadAllText($owner) -ne $name) { throw 'WSL ownership record changed' }
        if ((Distributions) -contains $name) {
            & wsl.exe --unregister $name
            if ($LASTEXITCODE -ne 0) { throw 'Cannot remove the owned CI distribution' }
        }
        Remove-Item -LiteralPath $owner
    }
    exit 0
}
if ((Distributions) -contains $name) { throw 'Refusing an existing distribution' }
& wsl.exe --version
if ($LASTEXITCODE -ne 0) { throw 'This runner does not provide WSL2' }
$root = Join-Path $env:RUNNER_TEMP $name
[IO.Directory]::CreateDirectory($root) | Out-Null
$image = Join-Path $root 'rootfs.tar.gz'
# Microsoft's WSL distribution registry identifies these exact Ubuntu 24.04.4 bytes.
$url = 'https://releases.ubuntu.com/24.04.4/ubuntu-24.04.4-wsl-amd64.wsl'
$sha256 = '9b2f7730dc68227dd04a9f3e5eab86ad85caf556b8606ad94f1f29ff5c4fd3f5'
Invoke-WebRequest -Uri $url -OutFile $image -TimeoutSec 300
if ((Get-FileHash -Algorithm SHA256 -LiteralPath $image).Hash.ToLowerInvariant() -ne $sha256) {
    throw 'WSL image checksum mismatch'
}
[IO.File]::WriteAllText($owner, $name)
& wsl.exe --import $name (Join-Path $root 'distro') $image --version 2
if ($LASTEXITCODE -ne 0) { throw 'Native runner could not start the WSL2 fixture' }
Remove-Item -LiteralPath $image
$setup = @'
set -eu
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq docker.io
systemctl stop docker.service docker.socket
'@
$setup = $setup.Replace("`r`n", "`n")
& wsl.exe --distribution $name --user root --exec sh -lc $setup
if ($LASTEXITCODE -ne 0) { throw 'Cannot prepare the owned Docker engine' }
# Keep this PowerShell process alive for the entire test, including WSL's console
# and redirected streams. A live engine in one CI step did not survive step exit.
$stderr = Join-Path $root 'dockerd-stderr.log'
$stdout = Join-Path $root 'dockerd-stdout.log'
$engine = Start-Process -FilePath 'wsl.exe' -ArgumentList @(
    '--distribution', $name, '--user', 'root', '--exec', 'dockerd',
    '--host=unix:///var/run/docker.sock', '--host=tcp://127.0.0.1:23759', '--tls=false'
) -NoNewWindow -RedirectStandardOutput $stdout -RedirectStandardError $stderr -PassThru
# Only WSL localhost forwarding is used. Never expose this CI-only daemon on a LAN interface.
$deadline = [DateTime]::UtcNow.AddSeconds(120)
do {
    if ($engine.HasExited) {
        Get-Content -LiteralPath $stderr -Tail 40
        throw "Owned Docker engine exited with status $($engine.ExitCode)"
    }
    try {
        if ((Invoke-RestMethod -Uri 'http://127.0.0.1:23759/_ping' -TimeoutSec 2) -eq 'OK') { break }
    } catch { }
    if ([DateTime]::UtcNow -ge $deadline) {
        Get-Content -LiteralPath $stderr -Tail 40
        throw 'WSL Docker engine did not become reachable through localhost'
    }
    Start-Sleep -Milliseconds 250
} while ($true)
$env:DOCKER_HOST = 'tcp://127.0.0.1:23759'
Remove-Item Env:DOCKER_CONTEXT -ErrorAction SilentlyContinue
$env:DOCKER_CONFIG = Join-Path $root 'client'
[IO.Directory]::CreateDirectory($env:DOCKER_CONFIG) | Out-Null
& docker.exe version
if ($LASTEXITCODE -ne 0) { throw 'Native Docker CLI cannot reach the test engine' }
$env:DOCKER_BINARY = (Get-Command docker.exe).Source
try {
    Write-Output '::group::Prepare and verify the reviewed fixture image'
    & docker.exe pull $env:DOCKER_IMAGE
    if ($LASTEXITCODE -ne 0) { throw 'Cannot pull the reviewed fixture image' }
    $nodeVersion = & docker.exe run --rm --pull=never $env:DOCKER_IMAGE node --version
    if ($LASTEXITCODE -ne 0 -or ([string]$nodeVersion).Trim() -ne 'v24.20.0') {
        throw 'The pinned container did not return its expected Node version'
    }
    Write-Output $nodeVersion
    Write-Output '::endgroup::'
    Write-Output '::group::Native Windows gateway and Docker CLI ownership'
    & cargo test --locked --all-features --test docker -- --ignored
    if ($LASTEXITCODE -ne 0) { throw 'Native Docker acceptance failed' }
    if ($engine.HasExited) { throw 'The owned engine exited during acceptance' }
} catch {
    Get-Content -LiteralPath $stderr -Tail 40
    throw
} finally {
    Write-Output '::endgroup::'
}
