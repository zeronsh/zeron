# Agent tools on this device

In **Settings → General → Agent tools**, turn off **Zeron MCP** to start new agents without the built-in server. This does not remove user-configured or plugin-provided MCP servers. The master switch remembers the individual choices:

- **Chat and visual tools** (on by default): chat delegation, visuals and built-in integrations. Switching them off also removes their initialization instructions.
- **Computer use** (macOS, off by default): tools for observing and interacting with apps on this Mac. Install [CuaDriver.app](https://cua.ai/docs/cua-driver) in `/Applications` (tested with 0.36); do not extract the executable from the bundle. A developer can point `ZERON_CUA_DRIVER` at the bundled executable instead. No driver is downloaded or installed by Zeron. If the driver cannot start, its tools do not appear.

Computer use relies on macOS Accessibility and Screen Recording permissions. The packaged app includes the window indicator and a physical Escape monitor (requires Input Monitoring). Stop cancels an in-flight action; an action already dispatched to an app may have taken effect. Inspect the window before retrying. Browser-specific tools are omitted from the catalog unless `ZERON_COMPUTER_BROWSER=1`; `ZERON_COMPUTER_USE=0` disables the driver regardless of the saved choice.

## Keep Mac awake

**Settings → General → Keep Mac awake** is independent of MCP. While Zeron is open, it prevents system sleep during fresh local agent activity, including background work. It releases sleep after 20 seconds of inactivity, when switched off, or on quit. This affects the whole Mac, including when the lid is closed. A closed, undocked Mac can sleep immediately after the grace period. Zeron does not take ownership if sleep was already disabled.

The first activation asks an administrator to allow just `/usr/bin/pmset disablesleep 0` and `/usr/bin/pmset disablesleep 1` for the current numeric user ID. The rule remains after switching off. To revoke it, turn off the setting, then have an administrator remove `/etc/sudoers.d/zeron-keep-awake-<uid>`. No administrator prompt is shown on launch.
