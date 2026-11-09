# Windows desktop frost disappears during maximize and restore

Reported on this machine: the blur of windows/desktop behind Zeron disappears
while maximizing or restoring, and returns when the transition completes.
This is the native desktop glass, not a wallpaper or an in-app backdrop blur.

Status: open. Do not count the PR #1219 build as verification of this behavior.
Its zgx pin, `21f8fa29fd8af4f190c2ba6e939aa8ce688e5d09`, ignores `Glass::plain`
on Windows and uses a DWM Mica backdrop for UnderWindowBackground. Newer zgx
added the live desktop frost in `8c6e0ba3` and transparency handling in
`61cf0daf`. The current target branch pins `bf4a3de0504edd0c769d5d8c19c8c2a6c39c400d`,
whose window_chrome.rs uses an acrylic accent for plain glass.

Reproduce against that newer backend, with Transparency effects enabled and
a patterned window behind Zeron. Observe the whole maximize/restore animation,
also checking a manual edge resize. Verify that blur stays present, activation
still works, and turning Transparency effects off still produces opaque glass.
Repeat with keyboard shortcuts, title-bar double click, and caption buttons.

A possible mitigation is disabling DWM window transitions on glass windows
(`DWMWA_TRANSITIONS_FORCEDISABLED`); it removes the native animation, so it is
not equivalent to preserving blur throughout it. It has not been implemented
or validated here. Avoid undocumented accent-flag guesses and repeated backdrop
resets on every frame. The current PR's renderer needs updating/integration
before a fix can be tested against the reported desktop-frost behavior.

Reference: https://learn.microsoft.com/en-us/windows/win32/api/dwmapi/ne-dwmapi-dwmwindowattribute
