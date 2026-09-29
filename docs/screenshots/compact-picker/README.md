# Compact picker runtime evidence

Captured from clean `codex/compact-picker` source `4ea3e527` on 2026-09-29. The fixture uses synthetic catalogs and temporary settings (default Zeron accent), with no real conversation or IPC server.

Build and run: `cargo run -p zeron-ui --features compact-picker-fixture --example compact-picker-fixture -- <output>`.
Executable: `target/debug/examples/compact-picker-fixture`; SHA256 `033dc163140e81aba3e22790f2efdb94271f73bc8cd9c95aeac10e363025fc5c`. Exited successfully with the fixture PASS marker.

All keyboard assertions passed for effort selection, fast tier and favorite identity across dark/light and 840/360 logical widths, height 560. The included captures were visually inspected: both header buttons sit one card inset from the card's top and sides, the title, rail and option labels share one leading edge, fast mode shows an accent glyph on a neutral plate, and the dark fill uses the theme's accent fill token. Static screenshots do not establish animation performance or live provider compatibility. Settings switches are not part of this fixture.

- `compact-panel-dark-wide.png`: SHA256 `4f187e3fe8a3e5d1a9b026efc06098e4711bb581a9907cd4dcc7abccc83ab31e`
- `compact-panel-light-wide.png`: SHA256 `58578e510421ce0397af5e7eb42a93eb9aaacf28d9d4ab4f4ff849340c8b28fb`
- `compact-fast-keyboard.png`: SHA256 `8e3ba46eb463f8199a29dbfa0c28feece1f63cb33ef4646a3b360c0a8027228d`
- `compact-model-list-light-narrow.png`: SHA256 `3ae45a842492448d02177b4a0a9d5d3a38ff5c1e56d2debd5f74fca8ebb63d31`
