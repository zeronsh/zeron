# Desktop windows

Zeron has one graphical application per data directory and multiple native
windows on macOS, Linux and Windows. Every window sees the same conversations,
messages, queued messages and running agents. Opening a conversation in a second
window adds a view; it does not start another agent or engine.

## Entry points

- File → New Window, the command palette, or `zeron --new-window`.
- macOS: right-click the running app's Dock icon → **New Window**, including
  when all of its windows are closed.
- macOS: **Cmd+Option+N**. Windows/Linux: **Ctrl+Alt+N**.
- New project remains **Cmd/Ctrl+Shift+N**; new chat remains **Cmd/Ctrl+N**.
- A conversation's context menu offers **Open in another window**.
- A normal second launch activates an existing window. Conversation links route
  to the most recently used window showing that chat, otherwise the most recently
  used window (opening one if needed). Notification clicks use the same policy.

Shortcuts remain configurable. If an existing customization already uses the
new-window default, that customization wins and New Window is initially unassigned.
The menu and command palette continue to work.

## Ownership

`AppRuntime` owns the engine handle, bootstrap, application subscriptions and
shutdown. A view's `AppState` subscribes to that runtime's catalogs while keeping
its own selected chat/project. `ChatStore` acquires one transcript and queue feed
per conversation. Views receive an initial snapshot followed by incremental
frames. Optimistic messages and pending-send state are shared. The last consumer
releases a feed; pending sends retain it until acknowledgement.

Each `Shell` keeps its own drafts, scroll positions, navigation history, panels,
editors, terminal views and browser surfaces. Closing a window releases these
views. A submitted send or application operation retains its task owner until
completion, including when its native window has closed.

`notification_service` observes the application owner once. Session transition
baselines, connectivity alerts and attention-sound coalescing are process-wide.
Adding a window never replays a completion sound or posts another copy of a banner.

`lifecycle` coordinates quit, update installation and account/runtime replacement
across all windows. Every file editor must save or resolve its discard decision;
cancelling any close cancels the pending application action. Editors are locked
during runtime replacement, and old account-specific surfaces and drafts are
cleared before displaying the replacement runtime. Update downloads and account
operations expose the same progress to every window.

## Preferences and persistence

The central settings store remains the only `ui-settings.json` writer. Global
preferences update all windows; per-window changes merge only the fields edited
by that window. Layout, selected chat and project filter live under a window key.
New windows inherit the source window's project and layout with an offset;
off-screen saved bounds fall back to centered placement.

Cold launches restore the main window. Additional window identities last for the
GUI session; their saved layout records are removed when they close. Drafts and
the number of additional windows are not restored after quitting. Legacy
`openTabs` is still readable but does not drive conversation navigation.

Closing one window preserves the others. macOS keeps the application and its
embedded engine alive after the last window closes; reopening creates a fresh
view. Windows/Linux quit on last-window close. Normal GUI shutdown shuts down an
embedded engine; a separately running daemon keeps running.

## Launch coordination

`ui.lock` is an OS-owned lock independent of `engine.lock`. The first GUI publishes
a loopback endpoint in `ui-endpoint.json`; subsequent launches authenticate with
the per-launch token and forward Activate, NewWindow or OpenUrl before setting up
logging or the engine. Request IDs prevent duplicate windows on acknowledgement
retries. Startup retries are bounded; a crash releases the OS lock and the next
owner replaces stale endpoint metadata. Headless and daemon commands bypass this
GUI protocol.

## Verification

```sh
cargo test --release --locked -p zeron-ui --lib -- --test-threads=1
cargo test --release --locked -p zeron --bin zeron
cargo build --release --locked -p zeron
bash scripts/test-multi-window-linux.sh target/release/zeron
```

The Linux probe uses a separate Xvfb/Openbox desktop and temporary data directory.
It checks CLI forwarding, Ctrl+Alt+N, three real windows sharing one engine,
closing the original window, and clean exit after closing the last one.

For native macOS, Linux/Wayland or Windows window creation and lifetime checks:

```sh
cargo build --release --locked -p zeron-ui --features multi-window-fixture --example multi-window-fixture
target/release/examples/multi-window-fixture /tmp/zeron-window-evidence
```

This fixture uses a temporary local engine and writes `result.txt` on success.
On macOS, also verify Dock → New Window with windows open, with the app hidden,
and after closing all windows while leaving the app running.
On Windows, `scripts/test-windows-lifecycle.ps1` additionally verifies three
native windows, forwarding, one engine, clean closing and reopening. Windows GUI
CI requires its existing interactive-desktop workflow-dispatch option. macOS and
Linux native probes are included in the UI workflow; platform results should only
be reported after running them on that platform.

Local validation for this change: all 1,056 UI tests and 11 executable tests
passed on Fedora, along with the native X11/Openbox launch probe and the native
window fixture under nested Weston/Wayland. macOS and Windows native probes are
prepared for their respective runners and were not executed on this Linux host.
