[CmdletBinding()]
param(
    [Alias('--slow')]
    [switch]$Slow
)

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$demoRoot = Join-Path $repoRoot 'ignore/prototype-demo'
$daemonData = Join-Path $demoRoot 'daemon-data'
$uiData = Join-Path $demoRoot 'ui-data'
$projectRoot = Join-Path $demoRoot 'project'
$stdoutLog = Join-Path $demoRoot 'daemon.stdout.log'
$stderrLog = Join-Path $demoRoot 'daemon.stderr.log'
$appExe = Join-Path $repoRoot 'target/release/glitch-flow.exe'
$probeExe = Join-Path $repoRoot 'target/release/examples/rpc_probe.exe'
$daemon = $null
$ui = $null
$rpcUrl = $null
$ownsDaemonPort = $false

function Get-StableId([string]$Name) {
    $md5 = [System.Security.Cryptography.MD5]::Create()
    try {
        $bytes = $md5.ComputeHash([System.Text.Encoding]::UTF8.GetBytes("chaos-agent-prototype-demo:$Name"))
    } finally {
        $md5.Dispose()
    }
    # Give deterministic demo IDs UUID-shaped values without relying on an
    # extra module or changing IDs each time the launcher is run.
    $bytes[6] = ($bytes[6] -band 0x0f) -bor 0x30
    $bytes[8] = ($bytes[8] -band 0x3f) -bor 0x80
    return ([Guid]::new($bytes)).ToString('D').ToLowerInvariant()
}

function Quote-NativeArgument([string]$Value) {
    # PowerShell 5.1 splits a JSON native argument at spaces, even after the
    # embedded quotes are escaped. Apply the Windows argv quoting rules once.
    $quoted = [Text.StringBuilder]::new()
    [void]$quoted.Append('"')
    $slashes = 0
    foreach ($char in $Value.ToCharArray()) {
        if ($char -eq '\') { $slashes++; continue }
        if ($char -eq '"') {
            [void]$quoted.Append(('\' * ($slashes * 2 + 1)))
            [void]$quoted.Append('"')
            $slashes = 0
            continue
        }
        if ($slashes) { [void]$quoted.Append(('\' * $slashes)); $slashes = 0 }
        [void]$quoted.Append($char)
    }
    if ($slashes) { [void]$quoted.Append(('\' * ($slashes * 2))) }
    [void]$quoted.Append('"')
    return $quoted.ToString()
}

function Invoke-RpcProbeProcess([string[]]$Arguments) {
    $start = [Diagnostics.ProcessStartInfo]::new($script:probeExe)
    $start.Arguments = (($Arguments | ForEach-Object { Quote-NativeArgument $_ }) -join ' ')
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $process = [Diagnostics.Process]::Start($start)
    try {
        $output = $process.StandardOutput.ReadToEnd()
        $errors = $process.StandardError.ReadToEnd()
        $process.WaitForExit()
        if ($process.ExitCode -ne 0) { throw "rpc_probe exit $($process.ExitCode): $errors" }
        return $output
    } finally { $process.Dispose() }
}

function Invoke-RpcProbe([string]$Method, [object]$Params) {
    $json = ConvertTo-Json -InputObject $Params -Depth 16 -Compress
    $payload = Invoke-RpcProbeProcess @($script:rpcUrl, $Method, $json)
    try {
        return ConvertFrom-Json -InputObject $payload
    } catch {
        throw "rpc_probe $Method returned invalid JSON: $payload"
    }
}

function Get-DemoTranscriptCount([string]$ChatId) {
    $json = ConvertTo-Json -InputObject @{ chatId = $ChatId } -Compress
    $output = Invoke-RpcProbeProcess @($script:rpcUrl, 'WatchDocMessages', $json, '--stream', '1')
    $frame = ConvertFrom-Json -InputObject $output
    if ($null -eq $frame.reset) { return 0 }
    return @($frame.reset).Count
}

function Get-DemoTicketSnapshot {
    $output = Invoke-RpcProbeProcess @($script:rpcUrl, 'WatchTickets', '{}', '--stream', '1')
    return ConvertFrom-Json -InputObject $output
}

function Set-FileIfMissing([string]$Path, [string]$Contents) {
    if (-not (Test-Path -LiteralPath $Path)) {
        $parent = Split-Path -Parent $Path
        New-Item -ItemType Directory -Path $parent -Force | Out-Null
        [System.IO.File]::WriteAllText($Path, $Contents, [System.Text.UTF8Encoding]::new($false))
    }
}

function Set-DemoEnvironment([hashtable]$Values) {
    $saved = @{}
    foreach ($key in $Values.Keys) {
        $saved[$key] = [Environment]::GetEnvironmentVariable($key, 'Process')
        $value = if ($null -eq $Values[$key]) { $null } else { [string]$Values[$key] }
        [Environment]::SetEnvironmentVariable($key, $value, 'Process')
    }
    return $saved
}

function Restore-DemoEnvironment([hashtable]$Saved) {
    foreach ($key in $Saved.Keys) {
        [Environment]::SetEnvironmentVariable($key, $Saved[$key], 'Process')
    }
}

function Get-FreeLoopbackPort {
    $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
    try {
        $listener.Start()
        return ([System.Net.IPEndPoint]$listener.LocalEndpoint).Port
    } finally {
        $listener.Stop()
    }
}

function Wait-DaemonReady([System.Diagnostics.Process]$Process, [int]$Port) {
    for ($attempt = 0; $attempt -lt 60; $attempt++) {
        if ($Process.HasExited) {
            throw "The demo daemon exited early with code $($Process.ExitCode). See $script:stderrLog"
        }
        $client = [System.Net.Sockets.TcpClient]::new()
        $connected = $false
        try {
            $connect = $client.ConnectAsync([System.Net.IPAddress]::Loopback, $Port)
            $connected = $connect.Wait(250) -and $client.Connected
        } catch {
            # Retry while the daemon is binding its local IPC listener.
        } finally {
            $client.Dispose()
        }
        if ($connected) {
            $listeners = @(Get-NetTCPConnection -State Listen -LocalPort $Port -ErrorAction SilentlyContinue)
            if ($listeners.OwningProcess -contains $Process.Id) {
                $script:ownsDaemonPort = $true
                return
            }
            if ($listeners.Count -gt 0) {
                $owners = ($listeners.OwningProcess | Sort-Object -Unique) -join ', '
                throw "Port $Port belongs to process(es) $owners, not the demo daemon ($($Process.Id)); refusing to seed or stop it."
            }
        }
        Start-Sleep -Milliseconds 250
    }
    throw "The demo daemon did not open its local IPC port $Port. See $script:stderrLog"
}

function Stop-DemoDaemon {
    if ($null -eq $script:daemon) { return }
    try {
        if (-not $script:daemon.HasExited) {
            # StopEngine is only sent over the port assigned to this exact
            # process. If graceful shutdown stalls, Kill targets its process
            # handle only; no name-wide process search or termination is used.
            if ($script:ownsDaemonPort) {
                try { Invoke-RpcProbe 'StopEngine' @{} | Out-Null } catch { }
            }
            if (-not $script:daemon.WaitForExit(5000)) {
                try { $script:daemon.Kill() } catch { }
                [void]$script:daemon.WaitForExit(5000)
            }
        }
    } finally {
        $script:daemon.Dispose()
        $script:daemon = $null
    }
}

function Seed-DemoProject {
    Set-FileIfMissing (Join-Path $projectRoot 'README.md') @'
# Glitch Flow Demo

A small Rust workspace for trying the prototype's local mock agent. The demo
does not require provider credentials or a sync account.
'@
    Set-FileIfMissing (Join-Path $projectRoot 'Cargo.toml') @'
[package]
name = "local-agent-demo"
version = "0.1.0"
edition = "2021"

[dependencies]
'@
    Set-FileIfMissing (Join-Path $projectRoot 'src/main.rs') @'
fn main() {
    println!("Local agent prototype ready");
}
'@
    Set-FileIfMissing (Join-Path $projectRoot 'src/lib.rs') @'
/// A small example for the prototype's workspace and code preview.
pub fn retry_delay(attempt: u32) -> u64 {
    250_u64.saturating_mul(2_u64.saturating_pow(attempt.min(6)))
}

#[cfg(test)]
mod tests {
    use super::retry_delay;

    #[test]
    fn retry_delay_grows_and_is_bounded() {
        assert_eq!(retry_delay(0), 250);
        assert_eq!(retry_delay(2), 1_000);
        assert_eq!(retry_delay(99), 16_000);
    }
}
'@

    # A real local repository makes the branch and worktree controls usable
    # in the seeded demo. Do not reset an existing demo checkout or user edits.
    if (-not (Test-Path -LiteralPath (Join-Path $projectRoot '.git'))) {
        & git -C $projectRoot init -b main | Out-Null
        if ($LASTEXITCODE -ne 0) { throw 'Could not initialize the demo Git repository' }
    }
    & git -C $projectRoot rev-parse --verify HEAD 2>$null | Out-Null
    if ($LASTEXITCODE -ne 0) {
        & git -C $projectRoot config user.name 'Glitch Flow Demo'
        & git -C $projectRoot config user.email 'demo@localhost'
        & git -C $projectRoot add -- README.md Cargo.toml src/main.rs src/lib.rs
        if ($LASTEXITCODE -ne 0) { throw 'Could not stage the demo Git files' }
        & git -C $projectRoot commit -m 'Initial demo workspace' | Out-Null
        if ($LASTEXITCODE -ne 0) { throw 'Could not commit the demo Git files' }
    }

    $device = Invoke-RpcProbe 'LocalDevice' @{}
    $deviceId = [string]$device.deviceId
    if ([string]::IsNullOrWhiteSpace($deviceId)) { throw 'LocalDevice returned no deviceId' }

    $spaceId = Get-StableId 'space'
    Invoke-RpcProbe 'Mutate' @{
        op = 'createSpace'
        spaceId = $spaceId
        deviceId = $deviceId
        path = $projectRoot
        name = 'Glitch Flow Prototype'
        gitDetected = $true
    } | Out-Null

    $samples = @(
        @{ title = 'Map the request pipeline'; branch = 'main'; ageHours = 0; prompt = 'Walk me through the local request pipeline and explain the retry helper in src/lib.rs.' },
        @{ title = 'Add a resilient retry policy'; branch = 'main'; ageHours = 2; prompt = 'Review the retry helper in src/lib.rs and outline a small reliability improvement.' },
        @{ title = 'Review the workspace boundaries'; branch = 'main'; ageHours = 14; prompt = 'Summarize the files in this local workspace and identify the main boundaries.' },
        @{ title = 'Polish the command palette'; branch = 'main'; ageHours = 27; prompt = 'Review the command palette and suggest a small improvement to finding and opening sessions.' }
    )

    $queuedNewRun = $false
    foreach ($sample in $samples) {
        $chatId = Get-StableId ("chat:" + $sample.title)
        Invoke-RpcProbe 'Mutate' @{
            op = 'createChat'
            chatId = $chatId
            spaceId = $spaceId
            branch = $sample.branch
            config = @{
                harness = 'mock'
                model = 'mock-fable-5'
                reasoning = $null
                sandbox = 'workspace-write'
            }
        } | Out-Null
        Invoke-RpcProbe 'Mutate' @{ op = 'renameChat'; chatId = $chatId; title = $sample.title } | Out-Null
        if ($null -ne $sample.prompt) {
            $runMarker = Join-Path $daemonData ("run-seeded-" + $chatId + '.json')
            $previouslyQueued = Test-Path -LiteralPath $runMarker
            # An interrupted first run can leave the marker but no messages.
            # Wait for a stale marker before retrying so an active mock run
            # cannot be submitted a second time during quick restarts.
            $retryEmpty = $previouslyQueued -and
                (Get-Item -LiteralPath $runMarker).LastWriteTime -lt (Get-Date).AddMinutes(-5) -and
                (Get-DemoTranscriptCount $chatId) -eq 0
            if (-not $previouslyQueued -or $retryEmpty) {
                # Write before dispatch: if the RPC response is lost, a rerun
                # will not submit the same non-idempotent mock run twice.
                [System.IO.File]::WriteAllText($runMarker, '{"queued":true}', [System.Text.UTF8Encoding]::new($false))
                try {
                    Invoke-RpcProbe 'QueueCommand' @{
                        chatId = $chatId
                        command = @{
                            kind = 'run'
                            messageId = if ($retryEmpty) { [Guid]::NewGuid().ToString('D') } else { Get-StableId ("message:" + $sample.title) }
                            request = @{
                                prompt = $sample.prompt
                                model = $null
                                reasoning = $null
                                modelOptions = @{}
                                cwd = $projectRoot
                                sandbox = 'workspace-write'
                                autoApprove = $true
                                resume = $null
                            }
                        }
                    } | Out-Null
                    $queuedNewRun = $true
                } catch {
                    Write-Warning "The first demo run could not be confirmed; its marker prevents a duplicate retry. $($_.Exception.Message)"
                }
            }
        }
    }

    # Mock runs update activity as their transcript arrives. Restore the
    # intended demo order after they finish, so the opening chat is coherent.
    if ($queuedNewRun) { Start-Sleep -Seconds $(if ($Slow) { 8 } else { 2 }) }
    foreach ($sample in $samples) {
        $chatId = Get-StableId ("chat:" + $sample.title)
        $timestamp = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() - [int64]($sample.ageHours * 60 * 60 * 1000)
        Invoke-RpcProbe 'Mutate' @{ op = 'setChatActivity'; chatId = $chatId; lastMessageAt = $timestamp } | Out-Null
    }

    # A compact planning board makes the native Tasks route reviewable in the
    # demo. Existing records are left alone so a user's edits survive reruns.
    $boardId = Get-StableId 'ticket-board'
    $ticketSnapshot = Get-DemoTicketSnapshot
    if (-not @($ticketSnapshot.boards | Where-Object id -eq $boardId).Count) {
        Invoke-RpcProbe 'MutateTicket' @{
            op = 'createBoard'; boardId = $boardId; name = 'Glitch Flow'
            description = 'Design, build, and ship the native workspace.'
        } | Out-Null
    }
    Invoke-RpcProbe 'MutateTicket' @{ op = 'linkBoardSpace'; boardId = $boardId; spaceId = $spaceId } | Out-Null

    $ticketSeeds = @(
        @{ key = 'epic:planning'; kind = 'epic'; parent = $null; title = 'Make work visible'; description = 'Bring planning and execution into one calm workspace.'; status = 'inProgress'; priority = 'high' },
        @{ key = 'issue:ticket-ui'; kind = 'issue'; parent = 'epic:planning'; title = 'Polish the ticket workspace'; description = 'Fast list and board views with a focused issue detail.'; status = 'inProgress'; priority = 'high' },
        @{ key = 'issue:agent-control'; kind = 'issue'; parent = 'epic:planning'; title = 'Give agents ticket control'; description = 'Create, update, comment, and link child chats through the native agent tools.'; status = 'todo'; priority = 'urgent' },
        @{ key = 'subissue:comments'; kind = 'issue'; parent = 'issue:ticket-ui'; title = 'Keep the activity readable'; description = 'Surface comments and linked conversations without clutter.'; status = 'backlog'; priority = 'medium' },
        @{ key = 'epic:devices'; kind = 'epic'; parent = $null; title = 'Work across devices'; description = 'Keep planning independent of the machine running the agent.'; status = 'todo'; priority = 'medium' },
        @{ key = 'issue:device-links'; kind = 'issue'; parent = 'epic:devices'; title = 'Link a board to a device folder'; description = 'Make the execution context explicit for every new conversation.'; status = 'done'; priority = 'medium' }
    )
    foreach ($seed in $ticketSeeds) {
        $ticketId = Get-StableId ("ticket:" + $seed.key)
        if (@($ticketSnapshot.tickets | Where-Object id -eq $ticketId).Count) { continue }
        $parentId = if ($null -ne $seed.parent) { Get-StableId ("ticket:" + $seed.parent) } else { $null }
        Invoke-RpcProbe 'MutateTicket' @{
            op = 'createTicket'; ticketId = $ticketId; boardId = $boardId
            kind = $seed.kind; parentTicketId = $parentId; title = $seed.title
            description = $seed.description; status = $seed.status; priority = $seed.priority
        } | Out-Null
    }
    $uiTicketId = Get-StableId 'ticket:issue:ticket-ui'
    $uiChatId = Get-StableId 'chat:Polish the command palette'
    Invoke-RpcProbe 'MutateTicket' @{ op = 'linkTicketChat'; ticketId = $uiTicketId; chatId = $uiChatId } | Out-Null
    $commentId = Get-StableId 'ticket-comment:demo'
    if (-not @($ticketSnapshot.comments | Where-Object id -eq $commentId).Count) {
        Invoke-RpcProbe 'MutateTicket' @{
            op = 'createComment'; commentId = $commentId; ticketId = $uiTicketId
            author = 'Glitch Flow'; body = 'A clear place to plan the work and follow its agent threads.'
        } | Out-Null
    }
}

try {
    Push-Location $repoRoot
    New-Item -ItemType Directory -Path $daemonData, $uiData, $projectRoot -Force | Out-Null

    Write-Host '▸ building the headed app and RPC probe (first run can take a few minutes)…'
    & cargo build --release --locked -p glitch-flow -q
    if ($LASTEXITCODE -ne 0) { throw "cargo build --release --locked -p glitch-flow failed with code $LASTEXITCODE" }
    & cargo build --release --locked -p zeron-rpc --example rpc_probe -q
    if ($LASTEXITCODE -ne 0) { throw "cargo build --release --locked -p zeron-rpc --example rpc_probe failed with code $LASTEXITCODE" }
    if (-not (Test-Path -LiteralPath $appExe)) { throw "Built app was not found: $appExe" }
    if (-not (Test-Path -LiteralPath $probeExe)) { throw "Built RPC probe was not found: $probeExe" }

    $port = Get-FreeLoopbackPort
    $script:rpcUrl = "ws://127.0.0.1:$port"
    Write-Host "▸ starting a hidden mock daemon on 127.0.0.1:$port"
    $daemonEnvironment = @{
        GLITCH_FLOW_DATA_DIR = $daemonData
        GLITCH_FLOW_IPC_PORT = [string]$port
        GLITCH_FLOW_HARNESS = 'mock'
        GLITCH_FLOW_MOCK_DELAY_MS = if ($Slow) { '350' } else { $null }
        RUST_LOG = 'warn'
    }
    $savedDaemonEnvironment = Set-DemoEnvironment $daemonEnvironment
    try {
        $script:daemon = Start-Process -FilePath $appExe -ArgumentList @('headless') -WorkingDirectory $repoRoot -WindowStyle Hidden -PassThru -RedirectStandardOutput $stdoutLog -RedirectStandardError $stderrLog
    } finally {
        Restore-DemoEnvironment $savedDaemonEnvironment
    }
    Wait-DaemonReady $daemon $port

    Write-Host '▸ seeding the local Rust project and four sample chats'
    # Create the demo records after readiness; deterministic IDs and the
    # engine's upsert semantics make this safe on every subsequent run.
    Seed-DemoProject

    Write-Host '▸ opening the headed prototype; close its window to stop this daemon'
    $uiEnvironment = @{
        GLITCH_FLOW_DATA_DIR = $uiData
        GLITCH_FLOW_IPC_PORT = [string]$port
        GLITCH_FLOW_HARNESS = 'mock'
        GLITCH_FLOW_MOCK_DELAY_MS = if ($Slow) { '350' } else { $null }
        RUST_LOG = 'warn'
    }
    $savedUiEnvironment = Set-DemoEnvironment $uiEnvironment
    try {
        $script:ui = Start-Process -FilePath $appExe -WorkingDirectory $repoRoot -PassThru -Wait
        if ($ui.ExitCode -ne 0) { throw "The headed app exited with code $($ui.ExitCode)" }
    } finally {
        Restore-DemoEnvironment $savedUiEnvironment
    }
} finally {
    Stop-DemoDaemon
    if ($null -ne $ui) { $ui.Dispose() }
    Pop-Location -ErrorAction SilentlyContinue
}
