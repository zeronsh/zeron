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

The installer starts the daemon immediately and keeps it running across reboots. No sign-in or sync configuration is required. It also adds Zeron to your application launcher: a per-user `zeron.desktop` and icon under `~/.local/share` (or `$XDG_DATA_HOME`), rewritten each time the installer runs.

The desktop sidebar browser also needs the [Linux browser runtime](docs/reference/linux-browser.md).

Day-to-day:

```bash
zeron status      # local/synced mode and engine status
zeron update      # update to the latest release
zeron daemon start|stop|restart|status
```

## Optional multi-device sync

Sign in only when you want to open your account's synced workspace. Authentication changes the profile selected by the next engine start, so stop the daemon before changing it:

```bash
zeron daemon stop
zeron login
zeron daemon start
```

You can then start an agent on one synced device and follow or drive it from another. An always-on machine such as a VPS can keep those agents working after you close your laptop.

Signed-in devices can also send each other files and folders of any size over a direct, temporary connection (relayed when no direct path is available): "Send to device…" in the file tree, or an agent's `send_files` / `fetch_files` tools ("send me the build", "fetch the logs from the build server"). Received items land in `~/Zeron Transfers/<device>/`. See [device file transfer](docs/file-transfer.md).

Devices signed in to the same synced account are trusted with remote workspace access. A device controlling a workspace on another device can list, read, and write its files; enabling `Show ignored files` also makes gitignored files such as `.env` available remotely. `.git` is always excluded. Only sign in devices you trust with the full contents of your workspaces.

Signing in does not upload, move, or import existing local sessions. Local sessions and their attachments remain under the local profile and reappear when you return to local-only mode:

```bash
zeron daemon stop
zeron logout
zeron daemon start
```

`zeron login` and `zeron logout` refuse to modify credentials while an engine owns the data directory. The desktop app follows the same next-restart profile boundary.

On macOS: use the desktop release, or build `zeron` from source and run `zeron daemon install` to install the launchd service.

On Windows: run the `zeron-<version>-windows-x86_64-setup.exe` installer from the [latest release](https://github.com/zeronsh/zeron/releases/latest). It installs for your user without administrator rights, adds Zeron to the Start menu, and appears in Settings → Apps for uninstalling. A portable ZIP is also published; keep `zeron-update.json` beside `zeron.exe` for in-app updates. See the [development notes](docs/reference/windows-development.md) for source builds.

## Updates

The desktop app checks for a new release when it starts, every hour while it runs, and when you come back to it after the machine slept. A new version downloads in the background; the sidebar then offers **Update ready — restart to apply**, and if you don't restart, it installs the next time you quit Zeron. Check by hand with **Zeron → Check for Updates…** on macOS, or **Check for updates** in the account menu (bottom of the sidebar) on Windows and Linux. Set `ZERON_AUTO_UPDATE=0` to be notified without the background download.

Linux desktop installs from the release tarball's `install.sh` use the same `~/.zeron/app` layout as the curl installer, so they update in place too. A daemon installed as a service restarts into a newer installed version once no agent run or terminal is active; `zeron update` updates headless installs on demand.

## Sponsors

Thank you to [The Context Company](https://www.thecontextcompany.com/) for sponsoring Zeron.

You can help fund Zeron's development too. Individuals and companies are welcome to [become a sponsor on GitHub](https://github.com/sponsors/zeronsh).

---

Developing or curious how it works? [![Ask DeepWiki](https://deepwiki.com/badge.svg)](https://deepwiki.com/zeronsh/zeron) or check out [ARCHITECTURE.md](ARCHITECTURE.md).

Licensed under the [MIT License](LICENSE).
