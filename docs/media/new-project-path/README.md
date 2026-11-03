# New project: typed path

Captured on macOS on 2026-10-24 from commit `3a71e34a` (on `1ec71a31`). The fixture runs the production Shell against a real engine assembled in process and served over IPC, listing a scratch home at `/tmp/zeron-demo/ana`. It registers no agent harness, so no installed agent CLI starts, and it passes `ListDrives` through a filter that keeps the boot volume only, so the Locations step doesn't show the capturing Mac's other disks. Frames come from the window's own renderer (`render_to_image`), not an OS screen recording.

Build and run:

```bash
cargo build -p zeron-ui --example new-project-path-fixture --features project-path-fixture
target/debug/examples/new-project-path-fixture <output>
```

Executable: `target/debug/examples/new-project-path-fixture`; SHA256 `a36346ab8d504c261ab37e395181c96df1c0ae1c0a7baebd410725cd042b6794`. Every step is a window key or mouse event; typed text goes through the focused field's input handler. The run passed its 16 checks of the palette state and the engine's project registry, Esc through the window's real key dispatch included (`result.txt`; a failed check exits 1).

This folder is the fixture's output as written, plus `new-project-path.gif` (12 fps, 960 px, encoded from the MP4 with ffmpeg). Each still is the full window, `<name>.png`, and the palette area, `<name>-palette.png` (600 × 560 pt at 2×):

- `01-path-field-on-device`: ⌘L on the device list opens the path field on the highlighted device, prefilled with `~/`.
- `02-completion-ghost`: `~/co` previews `de` from the device's listing; ⇥ completes it to `~/code/`.
- `03-completed-path`: ⇥ per segment reaches `~/code/opensource/zeron/`.
- `04-project-added`: ⌘↵ opened and added it; the app lands on the new project.
- `05-crumbs-with-edit`: while browsing, the pen sits at the trail's end and the footer offers ⌘L.
- `06-path-field-selected`: the pen turns the trail into the open folder's path, `~/code/work/`, selected.
- `07-edited-path-opened`: `da` and ⇥ give `~/code/work/dashboard/`; ↵ opens it and the trail comes back.
- `08-missing-folder`: a typo keeps the field and says why; the list below it doesn't change.

These captures validate rendering and the keyboard and mouse flow against a real engine's `ListFolders` and `createSpace` on the local device. Remote devices (`targetDeviceId`) and Windows paths are covered by unit tests, not captures.

Validation: `cargo test -p zeron-ui --lib -- pickers:: shell::spaces::` (137 passed; Esc and Tab are dispatched through a drawn window, and fail without their fixes); `cargo clippy -p zeron-ui --lib --tests --example new-project-path-fixture --features project-path-fixture` (no findings on changed lines); rustfmt on the changed hunks.
