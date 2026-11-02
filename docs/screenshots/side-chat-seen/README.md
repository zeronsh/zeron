# A side chat's run ending on screen

Captured on macOS 27.0.1 on 2026-10-29 by `side-chat-seen-fixture` (`crates/ui/examples/`). It runs the production `Shell` in an active window, over a real engine with isolated data attached as at launch: the open chat's session card docked beside its transcript, one of its side chats in the right pane, and a run queued on that side chat, which a scripted mock harness answers. No agent runs and nothing leaves the machine.

- `before.png`: `main` at `369d4281` with the fixture and its hook added, two seconds after the side chat's run ended.
- `after.png`: this change on `369d4281`, before its rebase onto `6aa4d5af`, at the same moment.
- `running.png`: the same build, during the run. The lane row spins and the card's toggle counts one running lane, as on `main`.
- `before-after.png`: `before.png` over `after.png`.

Captures read the window back with `render_to_image`, cropped to the session card and the right pane, and reduced to 256 colors.

| Two seconds after the run ended | `main` | This branch |
| --- | --- | --- |
| Window active | yes | yes |
| The side chat's lane row | FAIL: completed (the check) | PASS: idle |
| Its `lastSeenAt` | none | 16 ms after its last message |

Regenerate:

```sh
cargo run -p zeron-ui --features appshots-fixture --example side-chat-seen-fixture -- <dir>
```

It writes `running.png`, `finished.png` and `result.txt`, and prints PASS or FAIL, or INCONCLUSIVE when its window had lost focus as the run ended. The fixture activates its own window, since a chat is only marked seen while its window is active.

These files document the PR and are not bundled into the app.
