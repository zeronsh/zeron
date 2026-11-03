# Compact-mode subagent fold

`before.png` / `after.png`: a live turn that spawned four subagents, compact transcript, work fold open — before and after `render_tool_group` budgets spawn cards at `CHIP_HEIGHT`.

Captured from the fixture (production Shell over an isolated in-process engine, synthetic transcript, no agent runs), cropped to the transcript:

```bash
cargo run -p zeron-ui --features appshots-fixture,resource-profile --example subagent-fold-fixture -- [hold-secs] [app-id]
```

It prints `READY` once the fold is open and keeps the window up for capture (`grim -T` here).
