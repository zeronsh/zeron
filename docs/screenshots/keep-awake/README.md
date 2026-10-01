# Keep computer awake

Native macOS GPUI exports of the production General settings page, captured with an isolated local workspace. The frames show the default prompt mode, all three choices, app-open mode, Off, and the 900px minimum desktop width. They contain no personal workspace data. The fixture renders settings; it does not start agent prompts or prove Windows/Linux runtime behavior.

Reproduce from the repository root:

```sh
cargo run --locked -p zeron-ui --features appshots-fixture \
  --example keep-awake-fixture -- docs/screenshots/keep-awake
```

The device-local `keepAwake` preference defaults to `whilePromptRunning` when loading existing settings. `whileAppOpen` keeps the inhibitor through window closure/minimization until the application quits; `off` releases it immediately. Prompt mode watches all live workspace sessions, including background conversations, remote runs, and permission/input waits. It releases on completion, stop, failure, or liveness expiry (45 seconds plus at most the 5-second expiry check).

macOS and Windows use native idle-system/display assertions through `keepawake`; Windows acquisition and release remain on one worker thread. Linux prefers the XDG desktop Inhibit portal (Idle), with a logind idle inhibitor when the portal is unavailable. Calls run outside the UI thread and failed acquisitions retry every 30 seconds while still requested. Sleep prevention remains subject to the desktop's power policy and explicit user sleep.

Regression coverage:

```sh
cargo test --locked -p zeron-ui --lib keep_awake -- --test-threads=1
cargo test --locked -p zeron-ui --lib settings:: -- --test-threads=1
# macOS only: verifies both assertions appear in pmset and disappear on release
cargo test --locked -p zeron-ui --lib native_macos_assertions -- --ignored
```
