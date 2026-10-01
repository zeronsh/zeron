# SSH direct mode (Android)

The Android app can drive a Zeron engine on one of your own computers without
a Zeron account and without the edge relay. The phone logs in to the computer
over SSH and opens a `direct-tcpip` channel to the engine's loopback IPC port
(`127.0.0.1:27654`, or `ZERON_IPC_PORT`). Nothing new listens on the network:
the engine keeps its loopback-only port, and the only port you expose is sshd.

```
phone ──SSH (22)──▶ sshd on the computer ──direct-tcpip──▶ 127.0.0.1:27654 (Zeron engine)
```

The client side lives in `crates/client/src/direct` (SSH via `russh`), the FFI
in `crates/mobile/src/client_ffi/direct.rs`, and the Machines screens in
`apps/android`. The engine is unchanged; the phone speaks the same `EngineRpc`
the desktop UI uses and mirrors the registry and transcript streams into its
local documents, so sessions, the composer and commands work as in synced mode.

## Security boundary

- **SSH login is the trust decision.** Anyone who can log in as the OS user can
  already reach the engine's loopback port, so direct mode grants the phone the
  same access a local process of that user has — no more. It adds no listener,
  no token and no engine-side code.
- **Host keys are pinned on first use.** The first connection shows the
  server's `SHA256:` fingerprint and connects only after you trust it. A changed
  key later is refused ("Host key changed") until you explicitly trust the new
  one.
- **Credentials stay on the phone.** The phone generates its own Ed25519 key
  (or imports one, or uses a password). Private keys and passwords are encrypted
  with an AES-GCM key held in the Android Keystore and stored in app-private
  preferences; `allowBackup` is off. They are only ever sent to the SSH server,
  and the diagnostics text the app can copy never includes them (it does
  include the user name and host address).
- The tunnel only ever targets `127.0.0.1:<engine port>` on the computer.

## Set up a Windows computer

Run these in an **administrator** PowerShell.

### 1. Install and start OpenSSH Server

```powershell
Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0
Start-Service sshd
Set-Service sshd -StartupType Automatic
```

The installer normally adds a firewall rule. Check it:

```powershell
Get-NetFirewallRule -Name *OpenSSH-Server* | Select-Object Name, Enabled, Profile
```

If nothing is listed, add one:

```powershell
New-NetFirewallRule -Name OpenSSH-Server-In-TCP -DisplayName "OpenSSH Server (sshd)" -Enabled True -Direction Inbound -Protocol TCP -Action Allow -LocalPort 22
```

### 2. Authorize the phone's public key

On the phone, open **Settings → Machines → This phone's SSH key** and tap
**Copy public key**. Get the line (`ssh-ed25519 AAAA… zeron-…`) to the computer.

For an administrator account, Windows OpenSSH reads
`administrators_authorized_keys`, not `%USERPROFILE%\.ssh\authorized_keys`:

```powershell
$key = 'ssh-ed25519 AAAA...paste the whole line from the phone...'
Add-Content -Path C:\ProgramData\ssh\administrators_authorized_keys -Value $key -Encoding ascii
icacls C:\ProgramData\ssh\administrators_authorized_keys /inheritance:r /grant "Administrators:F" /grant "SYSTEM:F"
```

> If the file's permissions are wrong (for example, inherited read access for
> Users), sshd silently ignores it and the phone reports an authentication
> failure.

For a non-administrator account:

```powershell
New-Item -ItemType Directory -Force $env:USERPROFILE\.ssh | Out-Null
Add-Content -Path $env:USERPROFILE\.ssh\authorized_keys -Value $key -Encoding ascii
```

You can also choose **Password** on the phone instead of a key (the Windows
account password; for a Microsoft account, its password).

### 3. Find the computer's address

```powershell
ipconfig
```

Use the **IPv4 address** of the active adapter. The phone must reach it: same
LAN, or an overlay network such as Tailscale or ZeroTier (use that address).

### 4. Keep the Zeron engine running

Leaving the Zeron desktop app open is enough. To run the engine headless at
logon instead, find the executable (while Zeron runs):

```powershell
(Get-Process zeron).Path
```

and register a logon task with that path:

```powershell
$exe = "C:\path\to\zeron.exe"
schtasks /Create /TN "Zeron Headless" /SC ONLOGON /RL LIMITED /TR "\"$exe\" headless"
schtasks /Run /TN "Zeron Headless"
```

### 5. Check

```powershell
netstat -ano | findstr 27654     # expect 127.0.0.1:27654 ... LISTENING
netstat -ano | findstr ":22 "    # sshd
```

### 6. Note the host key fingerprint

```powershell
ssh-keygen -lf C:\ProgramData\ssh\ssh_host_ed25519_key.pub
```

The output looks like `256 SHA256:… (ED25519)`. Compare it with the
fingerprint the phone shows on first connection before tapping **Trust**.

## macOS and Linux

Any OpenSSH server that allows TCP forwarding works. On macOS enable
**System Settings → General → Sharing → Remote Login**; on Linux install and
start `openssh-server`. Append the phone's public key to
`~/.ssh/authorized_keys`, and get the fingerprint with
`ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub`. The engine must be running
(`zeron daemon status`).

## Add the machine on the phone

**Settings → Machines → +**:

| Field | Value |
| --- | --- |
| Name | Anything, e.g. `My PC` |
| Host | The address from step 3 |
| SSH port | `22` |
| Zeron port | `27654` (default) |
| User | The OS user name (on Windows, the part after `\` in `whoami`) |
| Sign in with | This phone's key (recommended) / Import key / Password |

Tap **Test**, compare the fingerprint, tap **Trust**, wait for
"Connected · Zeron 0.2.x answered in … ms", then **Save & Connect**.

## Troubleshooting

- **Connect fails or times out:** check the address, that both devices are on a
  reachable network, that port 22 is allowed, and that sshd is running
  (`Get-Service sshd` on Windows).
- **Authentication fails:** check the key file location and its permissions
  (`icacls` above) and the user name. On Windows, sshd logs to
  `Get-WinEvent -LogName OpenSSH/Operational -MaxEvents 20 | Format-List TimeCreated, Message`.
- **SSH works but the engine doesn't answer:** Zeron isn't running (steps 4–5).
- **Tunnel refused:** make sure `sshd_config` doesn't set
  `AllowTcpForwarding no`, then restart sshd.
- **Connected, but the sessions list is empty or keeps loading:** the sessions
  page shows the link state (connecting, loading sessions, or the error). Tap
  **Details** (or **Settings → Connection Details**) for the engine version,
  frames and rows received per stream (`WatchDevices`, `WatchSpaces`,
  `WatchChats`, `WatchSessions`) and a link log. **Copy** puts that text on the
  clipboard: no keys or passwords, but it names the user and host, so redact
  those before posting it anywhere. If a stream sends nothing for 20
  seconds, the app names it and reconnects.
- **Zeron on the computer updated; does the app need updating too?** Usually
  not. Unknown fields are ignored and unknown enum values (status, effort,
  harness, …) are shown as defaults, which Details records as "read with
  unknown values ignored". An unknown message or tool kind in a transcript
  shows as an "Unsupported content" placeholder and the rest renders normally;
  new notification-only messages are ignored. A row that still can't be read
  is skipped (not deleted), and the banner and Details say how many were
  skipped or repaired. Patch releases (0.2.97 → 0.2.98) are silent; an engine
  whose major or minor version is newer than the app was tested with (0.3.x)
  gets a dismissible card on the sessions page suggesting an app update. It
  keeps working either way.

## Current limits

- No attachments (images or files) in direct mode yet.
- The shared message queue is off: a message sent while a turn is running is
  delivered as a steer into that turn, not queued.
- Pins and sections are stored on the phone only.
- Presence is shown only for the computer the engine runs on.
