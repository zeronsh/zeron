# Access mode screenshots

Real Linux desktop application captures taken on 2026-10-27 from this branch, built with `cargo build -p zeron`. The application ran in Xvfb (Mesa lavapipe software Vulkan) at a 1280 × 800 window size against a separate local engine with temporary data directories, one empty demo project, the mock harness (which honors every mode and plan mode, and asks before its scripted command unless it has full access) and a device renamed to “Demo workstation.” Input was driven with `xdotool`; images were captured from the X display with ImageMagick `import`. They are unedited screenshots, not mockups, and are not bundled into the application.

- `supervised-approval.png`: a supervised chat's agent asks before running `cargo test --workspace`; the question replaces the composer until it is answered.
- `supervised-approved.png`: after **Yes**, the command runs. The composer's access chip reads **Supervised** beside the model chip.
- `access-menu.png`: the chip's menu: each mode with what it allows, then the plan-mode switch.
- `plan-toggle.png`: plan mode switched on; the chip adds **Plan** in the accent.
- `plan-reply.png`: the next message runs in plan mode and comes back as a plan with nothing changed. The chat's mode survived an application restart before this capture.
- `new-chat-menu.png`: the new-chat canvas remembers the last mode picked for a new chat (here **Auto-accept edits**); the menu opens below like its neighbours.
- `new-chat-auto-accept-edits.png`, `new-chat-full-access.png`: the chip per mode. Full access, the default and every chat's mode before this change, reads in the muted tone.
