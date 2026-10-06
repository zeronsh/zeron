param(
    [Parameter(Mandatory)][string]$ReleasesUrl
)
$ErrorActionPreference = 'Stop'
if (-not $ReleasesUrl.StartsWith('https://')) { throw 'Release feed must use HTTPS' }
$root = Split-Path $PSScriptRoot -Parent

function Find-InnoSetupCompiler {
    # Inno Setup 6 ships on GitHub's Windows runners; locally, install it with
    # `winget install JRSoftware.InnoSetup`.
    $command = Get-Command 'ISCC.exe' -ErrorAction SilentlyContinue
    if ($command) { return $command.Source }
    $bases = @(${env:ProgramFiles(x86)}, $env:ProgramFiles, "$env:LOCALAPPDATA/Programs") |
        Where-Object { $_ }
    foreach ($base in $bases) {
        $candidate = Join-Path $base 'Inno Setup 6/ISCC.exe'
        if (Test-Path -LiteralPath $candidate) { return $candidate }
    }
    throw 'Inno Setup 6 (ISCC.exe) is required to build the installer: winget install JRSoftware.InnoSetup'
}

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
    Copy-Item -LiteralPath 'crates/voice/NOTICE.md' -Destination (Join-Path $stage 'licenses/parakeet-v3.txt')
    Compress-Archive -Path "$stage/*" -DestinationPath "$stage.zip" -Force
    Copy-Item -LiteralPath './target/release/zeron.exe' -Destination "$stage.exe"
    # The per-user installer wraps the same staged directory (zeron-update.json
    # included), so installed copies update in place like the portable zip.
    $iscc = Find-InnoSetupCompiler
    & $iscc /Qp "/DAppVersion=$version" "/DArch=$arch" `
        "/DPackageDir=$([IO.Path]::GetFullPath($stage))" `
        "/DOutputDir=$([IO.Path]::GetFullPath($out))" `
        ([IO.Path]::GetFullPath((Join-Path $root 'dist/windows/zeron.iss')))
    if ($LASTEXITCODE -ne 0) { throw 'Installer build failed' }
    $setup = Join-Path $out "zeron-$version-windows-$arch-setup.exe"
    if (-not (Test-Path -LiteralPath $setup)) { throw "Installer not produced: $setup" }
    $file = Split-Path "$stage.exe" -Leaf
    $hash = (Get-FileHash -LiteralPath "$stage.exe" -Algorithm SHA256).Hash.ToLowerInvariant()
    @{ version = $version; files = @{ $file = @{ sha256 = $hash } } } |
        ConvertTo-Json -Depth 4 | Set-Content -Encoding utf8NoBOM -LiteralPath (Join-Path $out 'manifest.json')
    Write-Host "Packaged $stage.zip and $setup"
} finally { Pop-Location }
