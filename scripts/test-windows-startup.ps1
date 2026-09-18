# Check the actual executable: startup must not allocate a console, and CLI
# help/errors must still reach redirected streams before normal initialization.
param([string]$Exe = (Join-Path $PSScriptRoot '../target/release/zeron.exe'))
$ErrorActionPreference = 'Stop'
$Exe = (Resolve-Path -LiteralPath $Exe).Path
$bytes = [IO.File]::ReadAllBytes($Exe)
$pe = [BitConverter]::ToInt32($bytes, 0x3c)
$subsystem = [BitConverter]::ToUInt16($bytes, $pe + 24 + 68)
if ($subsystem -ne 2) { throw "Expected Windows GUI subsystem (2), got $subsystem" }
Write-Output 'PASS: desktop executable does not allocate a console at startup'

foreach ($argument in @('--help', '--version', '--invalid-startup-test-option')) {
    $info = New-Object Diagnostics.ProcessStartInfo
    $info.FileName = $Exe
    $info.Arguments = $argument
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $process = [Diagnostics.Process]::Start($info)
    try {
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        if (-not $process.WaitForExit(10000)) {
            $process.Kill()
            throw "CLI probe timed out: $argument"
        }
        if ($argument -eq '--invalid-startup-test-option') {
            if ($process.ExitCode -eq 0 -or $stderr.Result -notmatch 'unexpected argument') {
                throw 'Argument error did not reach stderr with a failing exit code'
            }
        } elseif ($process.ExitCode -ne 0 -or $stdout.Result -notmatch 'zeron') {
            throw "CLI output or exit code failed: $argument"
        }
        Write-Output "PASS: $argument output and exit code"
    } finally {
        $process.Dispose()
    }
}
