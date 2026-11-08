# One rig row on one throwaway engine (scripts/provider-update-rig/README.md,
# "Starting a throwaway engine"), with the isolation the task asks for: its own
# port, data dir, PATH, USERPROFILE/HOME, APPDATA, LOCALAPPDATA, and every
# provider path pinned (the target to its sandboxed install, the rest to a
# path that doesn't exist).
param(
  [Parameter(Mandatory)] [ValidateSet("before","after")] [string] $Label,
  [Parameter(Mandatory)] [string] $Row,
  [Parameter(Mandatory)] [string] $Target,
  [Parameter(Mandatory)] [string] $Launcher,
  [int] $Port = 27911,
  [int] $Seconds = 240,
  [string[]] $ExtraPath = @(),
  [switch] $CheckOnly,
  [string] $AfterApply = "",  # run while applywatch runs (W3, W6)
  [string] $BeforeExe = ""     # default: ..\zeron-before\target-before\debug\zeron-engine.exe
)
$ErrorActionPreference = "Stop"
$rig = "$env:TEMP\zeron-rig"
$repo = (Resolve-Path "$PSScriptRoot\..\..").Path
$exe = if ($Label -eq "after") { "$repo\target\debug\zeron-engine.exe" } else { if ($BeforeExe) { $BeforeExe } else { "$repo\..\zeron-before\target-before\debug\zeron-engine.exe" } }
$c = "$repo\scripts\provider-update-rig\rigclient\target\debug\rigclient.exe"
if ($Port -eq 27654) { throw "never the real engine's port" }

$run = "$rig\runs\$Label-$Row"
if (Test-Path $run) { Remove-Item -Recurse -Force $run }
$data = "$run\data"; $home_ = "$run\home"
foreach ($d in $data, $home_, "$home_\AppData\Roaming", "$home_\AppData\Local", "$run\tmp") { New-Item -ItemType Directory -Force $d | Out-Null }

$vars = @{
  ZERON_DATA_DIR = $data; ZERON_IPC_PORT = "$Port"
  ZERON_EDGE_URL = "http://127.0.0.1:1"; ZERON_DISABLE_NOTIFICATIONS = "1"; ZERON_AUTO_UPDATE = "0"
  ZERON_NO_LOGIN_SHELL = "1"
  USERPROFILE = $home_; HOME = $home_
  APPDATA = "$home_\AppData\Roaming"; LOCALAPPDATA = "$home_\AppData\Local"
  TEMP = "$run\tmp"; TMP = "$run\tmp"
  PATH = (@($ExtraPath) + @("C:\Windows\System32", "C:\Windows", "C:\Windows\System32\Wbem", "C:\Windows\System32\WindowsPowerShell\v1.0")) -join ";"
  RUST_LOG = "info,zeron_engine=debug"
}
$pins = @{ "claude-code"="CLAUDE_CODE_EXECUTABLE"; codex="CODEX_EXECUTABLE"; grok="GROK_EXECUTABLE"; devin="DEVIN_EXECUTABLE"
           fx="FX_EXECUTABLE"; hermes="HERMES_EXECUTABLE"; pi="PI_EXECUTABLE"; opencode="OPENCODE_EXECUTABLE"
           muse="MUSE_EXECUTABLE"; antigravity="ANTIGRAVITY_ACP_EXECUTABLE"; cursor="CURSOR_SDK_SHIM_EXECUTABLE" }
foreach ($h in $pins.Keys) { $vars[$pins[$h]] = if ($h -eq $Target) { $Launcher } else { "$run\not-installed\$h.exe" } }

$saved = @{}
foreach ($k in $vars.Keys) { $saved[$k] = [Environment]::GetEnvironmentVariable($k); [Environment]::SetEnvironmentVariable($k, $vars[$k]) }
$t0 = Get-Date
$engine = Start-Process -PassThru -NoNewWindow $exe -RedirectStandardOutput "$run\engine.out.log" -RedirectStandardError "$run\engine.err.log"
foreach ($k in $saved.Keys) { [Environment]::SetEnvironmentVariable($k, $saved[$k]) }
"engine pid $($engine.Id) ($Label, $exe) port $Port"
$engine.Id | Out-File "$run\engine.pid"

$ErrorActionPreference = "Continue"
# Windows PowerShell 5.1 drops the `"` inside native arguments: escape them.
function Rc { param([Parameter(ValueFromRemainingArguments)] $rest) $a = $rest | ForEach-Object { "$_".Replace('"', '\"') }; & $c @a 2>&1 | ForEach-Object { "$_" } }
try {
foreach ($h in "claude-code","codex","cursor","devin","grok","hermes","pi","opencode","antigravity","muse","fx") {
  $on = if ($h -eq $Target) { 'true' } else { 'false' }
  $r = Rc call $Port $data SetHarnessEnabled "{`"harness`":`"$h`",`"enabled`":$on}"; "enable $h=$on -> " + ("$r" -replace '^(.{0,60}).*', '$1')
}
$sw = [Diagnostics.Stopwatch]::StartNew()
$check = Rc call $Port $data CheckHarnessUpdates "{`"harness`":`"$Target`"}"
"CHECK ({0:N2}s): {1}" -f $sw.Elapsed.TotalSeconds, ($check -join " ")
if ("$check" -match 'panicked') { throw "check call failed" }
if (-not $CheckOnly) {
  if ($AfterApply) {
    $job = Start-Job -ScriptBlock { param($c,$p,$d,$t,$s) & $c applywatch $p $d $t $s 2>&1 | ForEach-Object { "$_" } } -ArgumentList $c,$Port,$data,$Target,$Seconds
    & ([scriptblock]::Create($AfterApply))
    Receive-Job -Wait -AutoRemoveJob $job | ForEach-Object { "APPLYWATCH $_" }
  } else {
    Rc applywatch $Port $data $Target $Seconds
  }
}
} finally {
  $null = & $c call $Port $data StopEngine "{}"
  if (-not $engine.WaitForExit(20000)) { "engine didn't stop; killing pid $($engine.Id)"; Stop-Process -Id $engine.Id -Force }
  "engine exited after {0:N1}s" -f ((Get-Date) - $t0).TotalSeconds
}
$oldPath = $env:PATH; $env:PATH = (@($ExtraPath) + @("C:\Windows\System32", "C:\Windows")) -join ";"
$va = if ($Target -eq "grok") { "version" } else { "--version" }
"LAUNCHER AFTER: " + ((& $Launcher $va 2>&1) -join " ")
$env:PATH = $oldPath

