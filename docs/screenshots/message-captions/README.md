# User-message captions runtime evidence

Captured on 2026-10-26 on Linux (Xvfb, Mesa lavapipe software Vulkan) from `message-captions-fixture`: an offline shell with one chat that switched from Claude Code to Codex and then received a message from another chat through Zeron's `send_message`. The `before-*` captures run the same fixture against `main` `255c540d` with this change's sources stashed.

Build and run: `cargo run -p zeron-ui --features appshots-fixture --example message-captions-fixture` (add `--capture <dir>` on platforms where `render_to_image` is implemented; on Linux the window was captured from the X display with `import -window root`, and hover/click driven with `xdotool`).

- The provider-switch seam has no divider and rides right-aligned over the message that switched, `USER_CAPTION_GAP` above its bubble.
- An agent-sent message shows `Message from [provider mark] Chat title` over its bubble; the routing header and the trailing short id leave the bubble. Hovering brightens the caption (`sender-hover-dark.png`); clicking opens the sending chat (`sender-click-dark.png`: the fixture's sender chat has no transcript of its own).

Static screenshots do not show the timing change: the engine now writes the seam together with the message instead of after the outgoing provider's runtime is replaced.

- `before-dark.png`: SHA256 `de2b0745c9bce45ff3a05f9b57432752caee872553e99d4e48e4a781e773ae70`
- `after-dark.png`: SHA256 `310149c94389097faf4d2510e3758f2772ae4aa150d375d9cf06eab590d6f963`
- `before-light.png`: SHA256 `48c234366795f05d90987355f5c40b89a51f26aaf0eba8154896a708c3d29405`
- `after-light.png`: SHA256 `1beec003998af34c94ced6d87da25bceb4602a8edff1939a07502f335745c7ba`
- `sender-hover-dark.png`: SHA256 `e85e20a87b3d08ddbba43b971bfa0d0a4798f7e328b4c19d26d14111e413d517`
- `sender-click-dark.png`: SHA256 `0158a61dd2e1f5312b38fca513a40dcb309d1a72fb19887b8b20e9cc6cba292d`
