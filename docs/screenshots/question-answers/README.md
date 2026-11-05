# Question answers

A resolved question's chip shows what it received, and the question panel
closes unanswered. These frames are the production `Shell` drawn offscreen by
`crates/ui/examples/question-answers-fixture.rs`: every chip state seeded into
a real engine chat (synthetic transcript, mock harness, no agent), then Escape
through the window's real key dispatch closes the open question, and the
fixture checks the doc recorded the decline before it prints PASS.

```bash
cargo run -p zeron-ui --features appshots-fixture \
  --example question-answers-fixture -- <output-dir>
```

- `after-chips.png`: an old session's question, which recorded nothing, keeps
  its header; one answer; three questions (two options picked, an answer
  typed, one skipped); and one closed unanswered.
- `after-panel.png`: the open question and its close.
- `after-closed.png`: after Escape, the chip reads `Ship → Not answered` and
  the composer is back.
- `before-chips.png`, `before-panel.png`: `main` at 77a3c0ff, from the same
  fixture minus the `answers` field `main` doesn't have (uncommitted). Every
  chip shows its first header only, and the panel, which `main` can't close,
  stays up over the transcript.

Frames are cropped from the 1200×1000 window (2× scale).
