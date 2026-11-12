# Linux 0.3.0 field notes

Observed the day [v0.3.0](https://github.com/zeronsh/zeron/releases/tag/v0.3.0) shipped (2026-11-12), on the release binary. `zeron --version` prints `zeron 0.3.0`, and `~/.zeron/app/current` points at `~/.zeron/app/0.3.0`.

These notes are from `~/.zeron/logs` between 19:57Z and 20:10Z, the first session after the update. Account emails, organization ids, device ids, and usage-slot ids are left out.

## Environment

- Ubuntu, GNOME, Wayland (`XDG_SESSION_TYPE=wayland`, `XDG_CURRENT_DESKTOP=ubuntu:GNOME`)
- Linux 7.0.0-42-generic, x86_64
- Cursor SDK 1.0.32, Codex 0.167.2, Claude Code 2.1.322
- Signed in. Access-token refresh succeeded (`ttl_s=3600`) and the registry room joined before the failures below.
- The window was relaunched several times in that window. The headed log has no panic. Each launch repeats the same warnings.

## Hosted model list returns 401 while the session is signed in

Every managed-engine start logs:

```text
INFO zeron_engine::hosted_models: Hosted model list not refreshed; keeping the last good one error=HTTP 401 Unauthorized
```

Seen at 20:02:34Z, 20:03:00Z, and 20:07:53Z, each time after a successful access-token refresh. The engine keeps the previous list, so the picker can show a stale hosted catalog and never says why. The edge-served hosted model list is new in 0.3.0.

## Model discovery warns "unavailable" with `error=None`, then serves the cache

On every window open, Cursor, Codex, and Claude Code each log `Model discovery unavailable` with `error=None`, then immediately `source="cache"`:

- Cursor `@cursor/sdk@1.0.32`, 48 models
- Codex 0.167.2, 8 models
- Claude Code 2.1.322, 11 models

Each line is written twice in the same millisecond. `crates/engine/src/model_catalogs.rs` warns on the discovery result, then falls back to disk. At 20:02:34Z Claude Code also logged `source="live"` in the same second as the unavailable warning, so a live read can succeed while the warning still fires. The empty error makes the warning useless, and the duplicate tasks double it.

## Usage probes fail, and a restart drops the backoff

Claude Code, active account:

- 20:02:04Z `step="usage" status=401 class="unauthorized"`, backoff 600s
- 20:03:13Z, after the engine was stopped and started again, `status=429 class="rate-limited" retry_after_s=3600`, backoff 3600s

401 and 403 usage failures also showed up on 9 and 10 November, before this release. What 0.3.0 still does is drop the backoff when the process restarts, so a relaunch inside the backoff turns a 401 into an hour-long 429.

Cursor, same session:

- 20:08:02Z `provider="cursor" step="grok" class="timeout" connect=false`

No status code. The probe gives up on the grok step and that account's usage stays unavailable. Earlier builds timed out on `step="usage"`. 0.3.0 is failing on `step="grok"`.

## Plugin signing keys are checked before sign-in is visible

At 20:00:40.689Z the headless engine logs `auth: signed in`, and on the same millisecond:

```text
INFO zeron_engine::plugins: plugins: couldn't check this device's signing key yet error=signed out
INFO zeron_engine::plugins: plugins: couldn't look for new signing keys yet error=signed out
```

`crates/engine/src/plugins/mod.rs` returns false on that error, so this device's key check is skipped for the sign-in that just succeeded. The headless process then shuts down over IPC and the managed engine takes the port. The key check does not retry in that process.

## Wayland chrome on GNOME

Every headed start logs both of these:

```text
INFO winit::platform_impl::linux::wayland::window::state: Blur manager unavailable, unable to change blur
WARN sctk_adwaita::buttons: Ignoring unknown button type:
```

The button type is an empty string. It is logged once or twice per launch (19:57Z, 20:02Z, 20:05Z, 20:07Z, 20:10Z). 0.3.0 asks for glass blur and clears the window's own title-bar buttons on Linux. GNOME Wayland has no blur manager, and the decoration request is handing Adwaita a blank button type.

## Viewport sockets reset when the window goes away

Each relaunch logs:

```text
WARN zeron_rpc::server: rpc: lost a viewport connection reason=reading the socket failed: WebSocket protocol error: Connection reset without closing handshake
```

Seen at 19:57:07Z, 20:02:20Z, 20:05:02Z, 20:07:49Z, and 20:08:09Z. The headed log has no panic. The UI process leaves without a closing handshake. A clean close belongs on `Reload Zeron`. If the window is dying on its own, these timestamps are the only trace.

## What still works in these logs

- IPC comes up on the engine port.
- Access-token refresh and the device-room host connect.
- Cached catalogs still load (48 / 8 / 11 models).
- The docs store finishes its WAL checkpoint on open and on sleep.
