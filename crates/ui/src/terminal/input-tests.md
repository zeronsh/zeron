# Web terminal input regression

`web-input-tests` enables the browser terminal input bridge in native GPUI tests;
it is off in normal desktop builds. The fixture renders the real `TerminalPanel`
and `ComposerInput` and dispatches real GPUI events, with no repaint between taps.
It needs a zui revision with the synchronous `DispatchEventResult.text_input_focus`
bridge. The currently committed desktop and web pins predate that bridge.

With that runtime resolved, run:

```sh
cargo test --locked -p zeron-ui --features web-input-tests --lib terminal::panel::input_tests -- --nocapture
cargo test --locked -p zeron-ui --features web-input-tests --lib terminal:: -- --nocapture
```

Until the runtime is published, use a disposable workspace under `target/` rather
than editing release pins/locks: copy the root manifests, `crates/`, and
`apps/zeron/`; link `vendor/` and `scripts/` to the original workspace; add local
`[patch."https://github.com/zeronsh/zui"]` entries for `gpui`, `gpui_macros`,
`gpui_platform`, and `gpui_tokio`. Resolve its lock once, then run the commands
above there. Linux needs the normal WebKitGTK/JSON-GLib and GPUI development
libraries even though the test renderer is headless.

Also compile the real WASM app against the same runtime, without the test feature.
A successful native fixture or WASM build does not prove OS keyboard behavior.

## Phone checks (iOS Safari and Android Chrome)

- From neutral focus, tap the terminal grid: keyboard opens on the first tap.
- Tap composer, then terminal: keyboard remains usable and text goes to the
  selected surface. Repeat after hiding the OS keyboard.
- Tap blank chat with terminal still logically focused: keyboard does not reopen.
- Type ASCII, emoji and composed text; press Enter and Backspace.
- Scroll/cancel a touch gesture over the terminal: do not open the keyboard.
- Check existing mouse/pen selection, hardware keys, arrows and Ctrl+C.

Terminal tab/chrome actions that explicitly request terminal focus retain that
intent. Newly created, not-yet-painted terminal bodies need a grid tap once
visible, like other newly mounted editors; this does not change delayed autofocus.
