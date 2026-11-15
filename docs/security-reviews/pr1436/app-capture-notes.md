# Native app captures for PR #1436

Built and ran the real zeron-zgx native desktop app on Linux/X11 from PR head 4b983c6c83e0bf94e8fb6a01ed8ae0b1bd122541.

Build: `cd apps/zgx && cargo build --locked`.

Added only the attached security_audit_fixture.luau under apps/zgx/ui/ in an isolated worktree. It wraps the existing shell_fixture.luau and uses the PR's actual chat, composer, model picker, access dropdown and activity components. The fixture supplies synthetic catalog/session/tool-review data and holds it against the host's disconnected-engine clear; no private chats, live model calls, or displayed shell commands were executed.

Run: `ZGX_INSPECT_DIR=/tmp/pr1436-zgx-inspect ZERON_ZGX_ENGINE_SPAWN=0 XDG_CONFIG_HOME=/tmp/pr1436-zgx-config XDG_DATA_HOME=/tmp/pr1436-zgx-data ./target/debug/zeron-zgx fixture=security_audit_fixture.luau --inspect`.

Captures use ImageMagick `import -screen -window <native-window-id>`; images are direct native-window screenshots including its open popups. No HTML recreation, generated imagery, compositing, or visual retouching.

- app-auto-picker.png: native Auto picker on Codex/GPT-6, with approved/denied/timed-out review shield states in the transcript.
- app-devin-auto.png: clicked Model, expanded the folded model row, clicked Devin, chose Fusion, closed Model, opened Access. The access picker still shows Auto despite Devin's fixture catalog advertising auto=false. This reproduces the UI retention portion of P1; the Rust regression establishes the backend fallback separately.
- app-review-badges.png: transcript rendering of the synthetic tool-review states. This validates the PR's UI rendering; it is not proof of a live provider denial or timeout.

The application inspector reported no runtime script errors after the capture interactions. The existing native host's absence of an engine is intentional in this fixture.
