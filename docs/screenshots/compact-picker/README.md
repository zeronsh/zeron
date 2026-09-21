# Compact picker runtime evidence

Captured from clean `codex/compact-picker` source `309d352364f4def96badf19b1f1d839ac8a19926` on 2026-09-19. The fixture uses synthetic catalogs and temporary settings, with no real conversation or IPC server.

Build: `CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/Users/gaelcado/.zeron/worktrees/comet/quiet-onyx/target cargo build --locked -p zeron-ui --example compact-picker-fixture --features compact-picker-fixture`.
Executable: `/Users/gaelcado/.zeron/worktrees/comet/quiet-onyx/target/debug/examples/compact-picker-fixture`; SHA256 `282015cc91837bb5300b4c20da39953bfc738966b7b22aa723c76246e8c8cfa7`; verified PID 98352. Exited successfully with the fixture PASS marker.

All keyboard assertions passed for effort selection, reset to model default, fast tier and favorite identity. Sixteen captures cover panel/list at dark/light and 840/360 logical widths, height 560. The two included captures were visually inspected: controls and list rows remain contained, the back/header and search are visible, and focus/selection remain distinct. Static screenshots do not establish animation performance or live provider compatibility.

- `compact-model-list-light-narrow.png`: SHA256 `9ecc26db208b368a02ecf5ab46c66462f63e5b00bffd39504e16652a686df260`
- `compact-panel-dark-wide.png`: SHA256 `9518418a589dfcb75514eeced89ff35049dc1e8bed46bd5882bde68899da78a4`
