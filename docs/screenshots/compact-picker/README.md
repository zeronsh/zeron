# Compact picker runtime evidence

Captured from clean `codex/compact-picker` source `6ec139a2` (rebased on `main` `26e2b0dd`) on 2026-09-30. The fixture uses synthetic catalogs and temporary settings (default Zeron accent), with no real conversation or IPC server.

Build and run: `cargo run -p zeron-ui --features compact-picker-fixture --example compact-picker-fixture -- <output>`.
Executable: `target/debug/examples/compact-picker-fixture`; SHA256 `e5d97ac0f1ae1cb9a5b3afe047398435acc2c39460e599dad2c6745f5face7f5`. Exited successfully with the fixture PASS marker.

All keyboard assertions passed for effort selection, fast tier and favorite identity across dark/light and 840/360 logical widths, height 560. The included captures were visually inspected: both header buttons sit one card inset from the card's top and sides, the title, rail and option labels share one leading edge, fast mode shows an accent glyph on a neutral plate, the slider thumb is a plain glass handle, and fills use the theme's accent fill token. Static screenshots do not establish animation performance or live provider compatibility.

`switch-states.png` is not produced by the committed fixture: it was captured from the same source with a local, uncommitted fixture patch that adds one on and one off `toggle_switch`, then cropped (dark above, light below). Measured mark contrast against the surface beneath: dark 3.13 (bar) / 2.95 (oval), light 3.02 / 3.23.

- `compact-panel-dark-wide.png`: SHA256 `9aa1d38d637a3c1fd70b7a616b1d945c30cecc6741913af15c4056940cc590f5`
- `compact-panel-light-wide.png`: SHA256 `e74b07d800de2cf5234d13b3c9d0d283cab2abff68bddfe22b06952ce4dda3f2`
- `compact-fast-keyboard.png`: SHA256 `313331b7f545e9b11fd610796b7b72adada1b4b43cf7662c778ad73764c71073`
- `compact-model-list-light-narrow.png`: SHA256 `3ae45a842492448d02177b4a0a9d5d3a38ff5c1e56d2debd5f74fca8ebb63d31`
- `switch-states.png`: SHA256 `4f5021ea3be5f8cdc297fe7f1e6774f113654da8e71cf8e12a5b8434dcd012ce`
