# Zeron

Control your coding agents (Claude Code, Codex, Cursor, Devin, Grok, Hermes, Pi, Antigravity) locally by default, with optional multi-device sync.

*English | [简体中文](README.zh-CN.md)*

![Zeron driving a Claude Code session with a live branch diff sidebar](apps/landing/public/assets/app-screenshot.jpg)

Every device runs a small engine that stores sessions on that device. A new installation starts in local-only mode without an account or a network connection.

## Install and run locally (Linux)

```bash
curl -fsSL https://zeron.sh/install.sh | sh
zeron status
```

The installer starts the daemon immediately and keeps it running across reboots. No sign-in or sync configuration is required.

The desktop sidebar browser also needs the [Linux browser runtime](docs/reference/linux-browser.md).

Day-to-day:

```bash
zeron status      # local/synced mode and engine status
zeron update      # update to the latest release
zeron daemon start|stop|restart|status
```

## Optional multi-device sync

Use a private workspace through Tailscale or sign in to Zeron Cloud.

### Private workspaces through Tailscale

A private workspace synchronizes through a hub you run on your own computer. Connect all devices to the same Tailscale network and enable MagicDNS and HTTPS before setup. No Zeron Cloud account is required.

Open **Settings → Workspace** to create or join a private workspace, invite devices, and manage access. An **agent server** runs agents against its repositories. A **client** controls agents on other computers. The hub can also run agents.

To create a hub from the command line, close the desktop app and stop the daemon first:

```bash
zeron daemon stop
zeron private create "Team workspace" --role server
zeron daemon install
zeron private pair --role client
zeron private status
```

The invitation works once and expires after five minutes. Open its QR link in the desktop or iOS app, or enter the hub address and code in **Join private workspace**.

To join from another computer, close its desktop app and stop its daemon, then run:

```bash
zeron private join https://hub.example.ts.net:8443 --code 123456 --name "Laptop" --role client
zeron daemon install
```

Replace the example address and code with the invitation values. Use `--role server` when the invited computer will run agents. Create an invitation with the same role on the hub.

```bash
zeron private status             # Show the workspace and paired devices
zeron private pair --role server # Invite another agent server from the hub
zeron private revoke DEVICE_ID   # Revoke a paired device from the hub
zeron private disable           # Disconnect this device, or disable hub access
zeron private enable            # Restore private access
zeron private leave             # Select Local mode for the next engine start
```

Restart the engine after creating, joining, or leaving a workspace through the CLI. The desktop setup flow handles the restart. **Run in background** keeps the host available after you close its window.

Private sync uses Tailscale Serve on HTTPS port `8443`, with the hub bound to `127.0.0.1:27655`. It does not fall back to Zeron Cloud. Agent providers still receive requests, and paired devices can access the workspace's files and agent controls. Local, private, and cloud work use separate profiles.

See [private workspace setup and access management](docs/private-workspaces.md) for details.

### Zeron Cloud

Sign in only when you want to open your account's synced workspace. Authentication changes the profile selected by the next engine start, so stop the daemon before changing it:

```bash
zeron daemon stop
zeron login
zeron daemon start
```

You can then start an agent on one synced device and follow or drive it from another. An always-on machine such as a VPS can keep those agents working after you close your laptop.

Devices signed in to the same synced account are trusted with remote workspace access. A device controlling a workspace on another device can list, read, and write its files; enabling `Show ignored files` also makes gitignored files such as `.env` available remotely. `.git` is always excluded. Only sign in devices you trust with the full contents of your workspaces.

Signing in does not upload, move, or import existing local sessions. Local sessions and their attachments remain under the local profile and reappear when you return to local-only mode:

```bash
zeron daemon stop
zeron logout
zeron daemon start
```

`zeron login` and `zeron logout` refuse to modify credentials while an engine owns the data directory. The desktop app follows the same next-restart profile boundary.

On macOS: use the desktop release, or build `zeron` from source and run `zeron daemon install` to install the launchd service.

On Windows: extract the portable release ZIP and run `zeron.exe`. Keep `zeron-update.json` beside it for in-app updates. See the [development notes](docs/reference/windows-development.md) for source builds.

---

Developing or curious how it works? [![Ask DeepWiki](https://deepwiki.com/badge.svg)](https://deepwiki.com/zeronsh/zeron) or check out [ARCHITECTURE.md](ARCHITECTURE.md).

Licensed under the [MIT License](LICENSE).
