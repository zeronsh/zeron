# Windows icon

`zeron.ico` contains 16, 24, 32, 48, 64, 128, and 256 pixel PNG frames made from
`../icon/zeron-solid-macos-Dark.png` by `scripts/make-app-icons.py`. An `.exe`'s
icon can't follow the system's light or dark setting, and this one reads on
either.

Both `apps/zgx/build.rs` (desktop app) and `apps/zeron/build.rs` (CLI/engine
binary) compile `zeron.rc` into the Windows executable for debug and release
builds. Resource ID 1 is the executable's first icon, used by the Windows
taskbar, Explorer, and `zgx-desktop`'s `window_chrome`. No adjacent image file
is needed at runtime; the installer (`zeron.iss`) uses the same `zeron.ico` for
its own window and the uninstall entry.
