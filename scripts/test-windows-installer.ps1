# Install, inspect, and uninstall the packaged per-user installer
# (dist/windows/zeron.iss) silently. Registers and removes the real per-user
# uninstall entry and zeron:// handler, so it refuses to run outside CI unless
# -Force is given.
param(
    [string]$Setup,
    [switch]$Force
)
$ErrorActionPreference = 'Stop'
if (-not $env:CI -and -not $Force) {
    throw 'This test installs and uninstalls Zeron for the current user; pass -Force to run it outside CI'
}
if (-not $Setup) {
    $Setup = Get-ChildItem (Join-Path $PSScriptRoot '../target/package') -Filter 'zeron-*-windows-*-setup.exe' |
        Select-Object -First 1 -ExpandProperty FullName
}
if (-not $Setup) { throw 'No zeron-*-setup.exe under target/package' }
$Setup = (Resolve-Path -LiteralPath $Setup).Path
$match = [regex]::Match((Split-Path $Setup -Leaf), '\Azeron-(\d+\.\d+\.\d+)-windows-[a-z0-9_]+-setup\.exe\z')
if (-not $match.Success) { throw "Unexpected installer name: $Setup" }
$version = $match.Groups[1].Value
$uninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{AD5DEC34-E254-467B-8F24-8127EBAF4DA6}_is1'
$protocolKey = 'HKCU:\Software\Classes\zeron'
$environmentKey = 'HKCU:\Environment'
$shortcut = Join-Path ([Environment]::GetFolderPath('Programs')) 'Zeron.lnk'
$root = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }
$dir = Join-Path $root "zeron installer test $([guid]::NewGuid().ToString('N'))"

function Invoke-Checked([string]$File, [string[]]$Arguments, [string]$What) {
    $process = Start-Process -FilePath $File -ArgumentList $Arguments -Wait -PassThru
    if ($process.ExitCode -ne 0) { throw "$What exited with $($process.ExitCode)" }
}

function Wait-Until([scriptblock]$Condition, [string]$What) {
    $deadline = (Get-Date).AddSeconds(90)
    while (-not (& $Condition)) {
        if ((Get-Date) -gt $deadline) { throw "Timed out waiting for $What" }
        Start-Sleep -Milliseconds 250
    }
}

function Test-UserPathHas([string]$Entry) {
    $key = Get-Item -LiteralPath $environmentKey
    $raw = $key.GetValue('Path', '', 'DoNotExpandEnvironmentNames')
    return (($raw -split ';') -contains $Entry)
}

$log = Join-Path $root 'zeron-setup.log'
Invoke-Checked $Setup @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', "/DIR=`"$dir`"", "/LOG=`"$log`"") 'Setup'
$exe = Join-Path $dir 'zeron.exe'
foreach ($file in @('zeron.exe', 'zeron-update.json', 'LICENSE', 'THIRD_PARTY_NOTICES.md', 'licenses/fonts', 'unins000.exe')) {
    if (-not (Test-Path -LiteralPath (Join-Path $dir $file))) { throw "Installed file missing: $file" }
}
Write-Output 'PASS: installed files'

# The update marker makes the in-app updater manage this install.
$config = Get-Content -Raw -LiteralPath (Join-Path $dir 'zeron-update.json') | ConvertFrom-Json
if (-not $config.releases_url.StartsWith('https://')) { throw "Unexpected update feed: $($config.releases_url)" }
Write-Output 'PASS: update-managed install'

$info = New-Object Diagnostics.ProcessStartInfo
$info.FileName = $exe
$info.Arguments = '--version'
$info.UseShellExecute = $false
$info.CreateNoWindow = $true
$info.RedirectStandardOutput = $true
$info.RedirectStandardError = $true
$process = [Diagnostics.Process]::Start($info)
try {
    $stdout = $process.StandardOutput.ReadToEndAsync()
    $null = $process.StandardError.ReadToEndAsync()
    if (-not $process.WaitForExit(10000)) { $process.Kill(); throw 'Installed version probe timed out' }
    if ($process.ExitCode -ne 0 -or $stdout.Result.Trim() -ne "zeron $version") {
        throw "Installed executable reports '$($stdout.Result.Trim())', expected 'zeron $version'"
    }
} finally { $process.Dispose() }
Write-Output "PASS: installed executable is $version"

$entry = Get-ItemProperty -LiteralPath $uninstallKey
if ($entry.DisplayVersion -ne $version) { throw "DisplayVersion is '$($entry.DisplayVersion)', expected '$version'" }
if ($entry.DisplayName -ne 'Zeron') { throw "DisplayName is '$($entry.DisplayName)'" }
$installed = [IO.Path]::GetFullPath($entry.InstallLocation).TrimEnd('\')
if ($installed -ne [IO.Path]::GetFullPath($dir).TrimEnd('\')) { throw "InstallLocation is '$installed'" }
$command = (Get-ItemProperty -LiteralPath "$protocolKey\shell\open\command").'(default)'
if ($command -ne "`"$exe`" `"%1`"") { throw "zeron:// handler is '$command'" }
if (-not (Test-Path -LiteralPath $shortcut)) { throw "Start menu shortcut missing: $shortcut" }
Write-Output 'PASS: uninstall entry, zeron:// handler, Start menu shortcut'

if (-not (Test-UserPathHas $dir)) { throw "User PATH lacks '$dir'" }
Invoke-Checked $Setup @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', "/DIR=`"$dir`"", "/LOG=`"$log`"") 'Setup rerun'
$entries = @(((Get-Item -LiteralPath $environmentKey).GetValue('Path', '', 'DoNotExpandEnvironmentNames') -split ';') | Where-Object { $_ -eq $dir })
if ($entries.Count -ne 1) { throw "User PATH lists '$dir' $($entries.Count) times" }
Write-Output 'PASS: install directory on the user PATH once'

# Leftovers an in-app update can leave behind must go with the uninstall.
Set-Content -LiteralPath (Join-Path $dir 'zeron.exe.old') -Value 'previous image'
New-Item -ItemType Directory -Path (Join-Path $dir '.zeron-update-test') | Out-Null
Set-Content -LiteralPath (Join-Path $dir '.zeron-update-test/zeron.exe') -Value 'staged'

# The uninstaller re-launches itself from a temporary copy and returns early;
# wait for its effects rather than for the process.
Invoke-Checked (Join-Path $dir 'unins000.exe') @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART') 'Uninstall'
Wait-Until { -not (Test-Path -LiteralPath $uninstallKey) } 'the uninstall entry to disappear'
Wait-Until { -not (Test-Path -LiteralPath $exe) } 'zeron.exe to be removed'
foreach ($leftover in @('zeron.exe.old', '.zeron-update-test', 'zeron-update.json', 'licenses')) {
    Wait-Until { -not (Test-Path -LiteralPath (Join-Path $dir $leftover)) } "$leftover to be removed"
}
if (Test-Path -LiteralPath $protocolKey) { throw 'zeron:// handler survived uninstall' }
if (Test-Path -LiteralPath $shortcut) { throw 'Start menu shortcut survived uninstall' }
Wait-Until { -not (Test-UserPathHas $dir) } 'the user PATH entry to disappear'
Write-Output 'PASS: uninstall removes the install, update leftovers, registrations, and the PATH entry'
