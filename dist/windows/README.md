# Windows icon

`zeron.ico` contains 16, 24, 32, 48, 64, 128, and 256 pixel PNG frames made from
`../icon/zeron-solid-macos-Dark.png` by `scripts/make-app-icons.py`. An `.exe`'s
icon can't follow the system's light or dark setting, and this one reads on
either.

`apps/zeron/build.rs` compiles `zeron.rc` into the Windows executable for
both debug and release builds. Resource ID 1 is required by GPUI's Windows
icon loader. No adjacent image file is needed at runtime; the installer
(`zeron.iss`) uses the same `zeron.ico` for its own window and the uninstall
entry.
