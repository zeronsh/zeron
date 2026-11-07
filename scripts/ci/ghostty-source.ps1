# Fetch the Ghostty source libghostty-vt builds from and point GHOSTTY_SOURCE_DIR
# at it, for Windows CI. ghostty-vt-sys downloads it itself through ureq's
# native-tls, which rejects the runner's certificate chain ("unable to find any
# user-specified roots"); curl.exe uses Schannel's own roots. The URL, checksum
# and root folder come from the GHOSTTY.lock of the ghostty-vt-sys in Cargo.lock.
$ErrorActionPreference = 'Stop'

$package = (cargo metadata --format-version 1 --locked | ConvertFrom-Json).packages |
  Where-Object name -eq 'ghostty-vt-sys' | Select-Object -First 1
if (-not $package) { throw 'ghostty-vt-sys is not in Cargo.lock' }
$lock = @{}
foreach ($line in Get-Content (Join-Path (Split-Path $package.manifest_path) 'GHOSTTY.lock')) {
  if ($line -match '^\s*(\w+)\s*=\s*(.+?)\s*$') { $lock[$Matches[1]] = $Matches[2] }
}

$archive = Join-Path $env:RUNNER_TEMP 'libghostty-vt-source.tar.gz'
curl.exe -fsSL --retry 5 -o $archive $lock.url
if ($LASTEXITCODE -ne 0) { throw "download failed: $($lock.url)" }
$actual = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash
if ($actual -ne $lock.sha256) { throw "sha256 mismatch for $($lock.url): $actual" }

$dir = Join-Path $env:RUNNER_TEMP 'ghostty-source'
New-Item -ItemType Directory -Force -Path $dir | Out-Null
tar -xzf $archive -C $dir
if ($LASTEXITCODE -ne 0) { throw 'failed to extract the Ghostty source' }
$source = Join-Path $dir $lock.root
if (-not (Test-Path (Join-Path $source 'build.zig'))) { throw "no build.zig in $source" }
"GHOSTTY_SOURCE_DIR=$source" >> $env:GITHUB_ENV
Write-Host "Ghostty source $($lock.version) in $source"
