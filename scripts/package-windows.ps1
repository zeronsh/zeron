param(
    [Parameter(Mandatory)][string]$ReleasesUrl
)
$ErrorActionPreference = 'Stop'
if (-not $ReleasesUrl.StartsWith('https://')) { throw 'Release feed must use HTTPS' }
$root = Split-Path $PSScriptRoot -Parent

function Get-WindowsPackageArch([string]$Path) {
    # Match zeron-update's `std::env::consts::ARCH` so the standalone .exe
    # name agrees with crates/update/src/windows.rs::artifact. Read the built
    # executable's PE machine type rather than this PowerShell process's
    # architecture: x64 PowerShell under ARM64 emulation reports X64 no matter
    # which toolchain rustc used.
    $stream = [IO.File]::OpenRead($Path)
    try {
        $reader = [IO.BinaryReader]::new($stream)
        $stream.Position = 0x3C
        $stream.Position = $reader.ReadUInt32()
        if ($reader.ReadUInt32() -ne 0x4550) { throw "Not a PE executable: $Path" }
        $machine = $reader.ReadUInt16()
    } finally { $stream.Dispose() }
    switch ($machine) {
        0x8664 { 'x86_64' }
        0xAA64 { 'aarch64' }
        default { throw ('Unsupported Windows executable machine type: 0x{0:X4}' -f $machine) }
    }
}

Push-Location $root
try {
    cargo build --release --locked -p zeron
    if ($LASTEXITCODE -ne 0) { throw 'Windows build failed' }
    # Explicit pipes also work for the GUI-subsystem executable in CI. A
    # PowerShell collection match does not populate the scalar $Matches map.
    $probe = [Diagnostics.ProcessStartInfo]::new()
    $probe.FileName = (Resolve-Path -LiteralPath './target/release/zeron.exe').Path
    $probe.Arguments = '--version'
    $probe.UseShellExecute = $false
    $probe.CreateNoWindow = $true
    $probe.RedirectStandardOutput = $true
    $probe.RedirectStandardError = $true
    $process = [Diagnostics.Process]::Start($probe)
    try {
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        if (-not $process.WaitForExit(10000)) {
            $process.Kill()
            throw 'Executable version probe timed out'
        }
        $versionMatch = [regex]::Match($stdout.Result.Trim(), '\Azeron (\d+\.\d+\.\d+)\z')
        if ($process.ExitCode -ne 0 -or -not $versionMatch.Success) {
            throw "Cannot read executable version: $($stderr.Result)"
        }
        $version = $versionMatch.Groups[1].Value
    } finally { $process.Dispose() }
    $out = Join-Path $root 'target/package'
    $arch = Get-WindowsPackageArch $probe.FileName
    $stage = Join-Path $out "zeron-$version-windows-$arch"
    New-Item -ItemType Directory -Force -Path $stage | Out-Null
    Copy-Item -LiteralPath './target/release/zeron.exe' -Destination (Join-Path $stage 'zeron.exe')
    @{ releases_url = $ReleasesUrl } | ConvertTo-Json | Set-Content -Encoding utf8NoBOM -LiteralPath (Join-Path $stage 'zeron-update.json')
    Copy-Item -LiteralPath 'LICENSE','THIRD_PARTY_NOTICES.md' -Destination $stage
    $licenses = Join-Path $stage 'licenses/fonts'
    New-Item -ItemType Directory -Force -Path $licenses | Out-Null
    Copy-Item -Path 'crates/ui/assets/fonts/licenses/*' -Destination $licenses
    Compress-Archive -Path "$stage/*" -DestinationPath "$stage.zip" -Force
    Copy-Item -LiteralPath './target/release/zeron.exe' -Destination "$stage.exe"
    $file = Split-Path "$stage.exe" -Leaf
    $hash = (Get-FileHash -LiteralPath "$stage.exe" -Algorithm SHA256).Hash.ToLowerInvariant()
    @{ version = $version; files = @{ $file = @{ sha256 = $hash } } } |
        ConvertTo-Json -Depth 4 | Set-Content -Encoding utf8NoBOM -LiteralPath (Join-Path $out 'manifest.json')
    Write-Host "Packaged $stage.zip"
} finally { Pop-Location }
