# The zgx app (release.yml's stable, zgx-beta.yml's beta), with -ZgxVersion
# and -ZgxEngine: the zgx interface (apps/zgx, its `beta` feature) as
# zeron.exe, and beside it app\<version>\ (a beta's: beta\<version>\) with
# its scripts, plugins and the zeron.exe it's given, built from the same
# commit, as engine\zeron.exe (a Windows release has no separate engine: it
# runs as `zeron.exe engine`). Users keep their data and device.
#
# A stable release also gets the standalone .exe, which is all the old app's
# updater installs: that zeron.exe brings in its folder from the zip when it
# first runs (apps/zgx/src/windows_install.rs). A beta has none.
#
# Without -ZgxVersion: the old app (apps/zeron), one self-contained zeron.exe,
# kept for a release that has to go back to it.
param(
    [Parameter(Mandatory)][string]$ReleasesUrl,
    [string]$ZgxVersion,
    [string]$ZgxEngine
)
$ErrorActionPreference = 'Stop'
if (-not $ReleasesUrl.StartsWith('https://')) { throw 'Release feed must use HTTPS' }
if ($ZgxVersion -and -not (Test-Path -LiteralPath $ZgxEngine -PathType Leaf)) {
    throw "-ZgxEngine: the engine's zeron.exe"
}
$beta = $ZgxVersion -match '-beta\.\d+$'

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

function Get-ZeronVersion([string]$Path) {
    # Explicit pipes also work for the GUI-subsystem executable in CI. A
    # PowerShell collection match does not populate the scalar $Matches map.
    $probe = [Diagnostics.ProcessStartInfo]::new()
    $probe.FileName = $Path
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
        $versionMatch = [regex]::Match($stdout.Result.Trim(), '\Azeron (\d+\.\d+\.\d+(?:-beta\.\d+)?)\z')
        if ($process.ExitCode -ne 0 -or -not $versionMatch.Success) {
            throw "Cannot read executable version: $($stderr.Result)"
        }
        $versionMatch.Groups[1].Value
    } finally { $process.Dispose() }
}

function Copy-Scripts([string]$From, [string]$To) {
    # zgx's generated types stay out. Robocopy's codes below 8 are success.
    robocopy $From $To /E /NFL /NDL /NJH /NJS /NP /XD .zgx /XF .lock .DS_Store | Out-Null
    if ($LASTEXITCODE -ge 8) { throw "Copying $From failed ($LASTEXITCODE)" }
    $global:LASTEXITCODE = 0
}

Push-Location $root
try {
    if ($ZgxVersion) {
        # A workspace of its own (its own lock, patches).
        $env:ZERON_BETA_VERSION = $ZgxVersion
        cargo build --release --locked --features beta --manifest-path apps/zgx/Cargo.toml --target-dir apps/zgx/target
        if ($LASTEXITCODE -ne 0) { throw 'Windows build failed' }
        $built = (Resolve-Path -LiteralPath './apps/zgx/target/release/zeron-zgx.exe').Path
    } else {
        cargo build --release --locked -p zeron
        if ($LASTEXITCODE -ne 0) { throw 'Windows build failed' }
        $built = (Resolve-Path -LiteralPath './target/release/zeron.exe').Path
    }
    $version = Get-ZeronVersion $built
    if ($ZgxVersion) {
        if ($version -ne $ZgxVersion) { throw "$built isn't the $ZgxVersion app (it says $version)" }
        # Production's only, and an engine with the shared command line, as
        # scripts/package-zgx-macos.sh requires of macOS's.
        $stagingMarks = 'staging\.zeron\.sh|zeron-edge-staging|client_01KWD0E9SYPV2WE4QN9TP9DP0W'
        foreach ($binary in @($built, $ZgxEngine)) {
            $text = [Text.Encoding]::Latin1.GetString([IO.File]::ReadAllBytes($binary))
            if ($text -match $stagingMarks) { throw "$binary carries staging's edge or sign-in client: $($Matches[0])" }
            if ($binary -eq $ZgxEngine -and -not $text.Contains('Multi-device controller for coding agents')) {
                throw 'The engine predates the shared command line'
            }
        }
        $text = $null
    }
    $out = Join-Path $root 'target/package'
    $arch = Get-WindowsPackageArch $built
    $stage = Join-Path $out "zeron-$version-windows-$arch"
    if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
    New-Item -ItemType Directory -Force -Path $stage | Out-Null
    Copy-Item -LiteralPath $built -Destination (Join-Path $stage 'zeron.exe')
    @{ releases_url = $ReleasesUrl } | ConvertTo-Json | Set-Content -Encoding utf8NoBOM -LiteralPath (Join-Path $stage 'zeron-update.json')
    Copy-Item -LiteralPath 'LICENSE','THIRD_PARTY_NOTICES.md' -Destination $stage
    $licenses = Join-Path $stage 'licenses/fonts'
    New-Item -ItemType Directory -Force -Path $licenses | Out-Null
    Copy-Item -Path 'crates/ui/assets/fonts/licenses/*' -Destination $licenses
    Copy-Item -LiteralPath 'crates/voice/NOTICE.md' -Destination (Join-Path $stage 'licenses/parakeet-v3.txt')
    # crates/update `windows::files_folder`: by the version's channel.
    $folder = if ($beta) { 'beta' } else { 'app' }
    if ($ZgxVersion) {
        $files = Join-Path $stage "$folder/$version"
        Copy-Scripts 'apps/zgx/ui' (Join-Path $files 'ui')
        Remove-Item -LiteralPath (Join-Path $files 'ui/tests') -Recurse -Force
        Copy-Scripts 'apps/zgx/plugins-builtin' (Join-Path $files 'plugins-builtin')
        # Each ships the canonical SDK, as scripts/package-zgx-macos.sh.
        foreach ($dir in Get-ChildItem -Directory -LiteralPath (Join-Path $files 'plugins-builtin')) {
            Copy-Item -LiteralPath 'apps/zgx/plugins-sdk/zeron.luau' -Destination (Join-Path $dir.FullName 'zeron.luau') -Force
        }
        New-Item -ItemType Directory -Force -Path (Join-Path $files 'engine') | Out-Null
        Copy-Item -LiteralPath $ZgxEngine -Destination (Join-Path $files 'engine/zeron.exe')
        Copy-Item -Path 'apps/zgx/fonts/*-OFL.txt' -Destination $licenses
    }
    Compress-Archive -Path "$stage/*" -DestinationPath "$stage.zip" -Force
    if (-not $beta) {
        Copy-Item -LiteralPath $built -Destination "$stage.exe"
    }
    # The per-user installer wraps the same staged directory (zeron-update.json
    # included), so installed copies update in place like the portable zip.
    $iscc = Find-InnoSetupCompiler
    $zgxDefines = if ($ZgxVersion) { @("/DFilesFolder=$folder", "/DFileVersion=$(($version -split '-')[0])") } else { @() }
    & $iscc /Qp "/DAppVersion=$version" "/DArch=$arch" @zgxDefines `
        "/DPackageDir=$([IO.Path]::GetFullPath($stage))" `
        "/DOutputDir=$([IO.Path]::GetFullPath($out))" `
        ([IO.Path]::GetFullPath((Join-Path $root 'dist/windows/zeron.iss')))
    if ($LASTEXITCODE -ne 0) { throw 'Installer build failed' }
    $setup = Join-Path $out "zeron-$version-windows-$arch-setup.exe"
    if (-not (Test-Path -LiteralPath $setup)) { throw "Installer not produced: $setup" }
    if (-not $beta) {
        $file = Split-Path "$stage.exe" -Leaf
        $hash = (Get-FileHash -LiteralPath "$stage.exe" -Algorithm SHA256).Hash.ToLowerInvariant()
        @{ version = $version; files = @{ $file = @{ sha256 = $hash } } } |
            ConvertTo-Json -Depth 4 | Set-Content -Encoding utf8NoBOM -LiteralPath (Join-Path $out 'manifest.json')
    }
    Write-Host "Packaged $stage.zip and $setup"
} finally { Pop-Location }
