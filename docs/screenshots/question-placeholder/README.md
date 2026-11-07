# A question's placeholder in another session

Captured on macOS 27.0.1 on 2026-10-29 by `composer-stale-placeholder-fixture`, a driver kept out of this change. It is in the [gist](https://gist.github.com/alexandre-vl/ab79a6e9766e5ab307137648998084bf) with `run.sh`. The driver runs the production `Shell` in a window with an in-process engine. The viewport reads a copy of a real `ui-settings.json` (Geist at UI size 16, Zeron Light, frosted, a 292 pt sidebar), so the composer wraps where it does in the app. Two sessions in one project, both Claude Code with Opus 5.5 at X-High and full access:

- **Token rotation strategy**: its agent asked a question (an unresolved input part), so it shows the question panel.
- **Refresh token production check**: a turn running, nothing asked.

The driver opens the first through `Shell::fixture_open_chat`, which is what a sidebar row's click runs, waits for the panel, then opens the second the same way. The window is 902 × 760 pt, which makes the composer as wide as in the report (577 pt).

- `before.png`: `main` at `8b7afb30` (v0.2.129), with the driver copied in. The running session's composer reads "Type your own answer, or pick an option". That is the question's placeholder: its last word wrapped onto a second line that the compact composer's one-line viewport hides, and the overflow fade covers the lower half of the visible line.
- `after.png`: this change on `8b7afb30`. The composer reads "Do anything…".
- `before-after.png`: both, `main` above.

The control, which opens the running session without visiting the question first, reads "Do anything…" on `main` too.

Captures read the window back with `render_to_image` at 2× scale. They are cropped to the composer and flattened onto a light grey, as the frosted window is translucent.

Regenerate, with the driver copied to `crates/ui/examples/`:

```sh
cargo build -p zeron-ui --features appshots-fixture --example composer-stale-placeholder-fixture
./run.sh target/debug/examples/composer-stale-placeholder-fixture <ui-settings.json>
```

These files document the PR and are not bundled into the app.
