# Pull request destination captures

Captured on macOS on 2026-10-24 by the opt-in `pull-request-destination-fixture` example: the production `Shell` in an offscreen window that is never activated, a real engine and IPC server on temporary data, the local `gh`, and native mouse events on a message's links. The session and its message are synthetic; the links point at real zeronsh/zeron pull requests. A link that leaves Zeron opens in the default browser for real.

- `before-*.png`: `main` at `1ec71a31` (v0.2.120), with this fixture copied in.
- `after-*.png`: this branch.

Captures read the window back with `render_to_image`, which leaves out WebKit's page: a Browser tab shows its tabs and address bar over an empty page. Each frame follows a press on empty space, which dismisses link tooltips.

| Check | `main` | This branch |
| --- | --- | --- |
| `default-browser`: Default browser, click #1005 | FAIL: Browser tab on #1005 | PASS: nothing opens in Zeron |
| `links-off`: Open links in Zeron off, click #1012 | FAIL: pull request view on #1012 | PASS: nothing opens in Zeron |
| `next-link`: click #997 with links off, then #1019 once with links on | FAIL: the press does nothing | PASS: Browser tab on #1019 |
| `in-app-browser`: In-app browser, click #1020 | no such option | PASS: Browser tab on #1020 |
| `pull-request-view`: Pull request view, click #1013 | PASS | PASS |

"Nothing opens in Zeron": no Browser tab, and the frame is identical to the one before the press.

Regenerate:

```sh
cargo build -p zeron-ui --example pull-request-destination-fixture \
  --features pull-request-fixture,browser-fixture
bash scripts/run-macos-browser-fixture.sh \
  target/debug/examples/pull-request-destination-fixture /tmp/pull-request-destination
```

The run writes `result.txt` and exits non-zero when a check fails. These files document the PR and are not bundled into the app.
