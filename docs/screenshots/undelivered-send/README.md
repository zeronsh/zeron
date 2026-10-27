# A session left before its first message lands

Captured on macOS 27.0.1 on 2026-10-26 by `undelivered-send-repro`, a driver kept out of this change. It is in the [gist](https://gist.github.com/alexandre-vl/8c96bd29631c286b8f9511645c207d71) with `run.sh`, each run's transitions and every second of samples. The driver runs the production `Shell` in a window that is never shown or focused, with an in-process engine, the mock harness and synthetic keys. It generates a 12,000-file git project, so the host takes several seconds to check out a session's worktree. Then it starts two sessions in "New worktree" with the same prompt:

- **Left right after sending**: left with ⌘N 0.6 s after Enter, before the host has written the message.
- **Stayed until it landed**: the control, kept open 45 s.

- `*-before.png`: `main` at `325384d8`, with the driver copied in.
- `*-after.png`: this change on `325384d8`, before its rebase onto `962a3207`.
- `before-after.png`: `working-before.png` and `working-after.png` side by side.

Captures read the window back with `render_to_image`, cropped to the Sessions list: `working-*` and `compact-*` about 145 s after the send, `question-*` at the first sample, 62 s after the send on `main` and 95 s on this branch.

| Check: the left session's row | Engine | `main` | This branch |
| --- | --- | --- | --- |
| `working`: a long turn, regular and compact sidebar | working, then idle | FAIL: Working, then Failed past 120 s | PASS: Working, then Done |
| `question`: the turn asks a question | awaitingInput | FAIL: Working, then Failed | PASS: Input |
| `done`: a quick turn | idle | FAIL: Working, then Failed | PASS: Done |
| `error`: the turn fails at once | errored | FAIL: Working, then Failed (not delivered) | PASS: Failed (errored) |
| The stayed session's row, every run | | PASS | PASS |

In every run the left session was left 0.60 to 0.62 s after Enter. Its message landed 9.5 to 10.7 s after Enter on `main` and 12.4 to 13.2 s on this branch, under heavier load from other processes. On this branch its row never read "not delivered".

Regenerate, with the driver copied to `crates/ui/examples/`:

```sh
cargo build -p zeron-ui --features appshots-fixture --example undelivered-send-repro
./run.sh target/debug/examples/undelivered-send-repro after
```

These files document the PR and are not bundled into the app.
