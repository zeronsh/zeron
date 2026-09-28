# Harnesses and models

A harness is the agent runtime a chat runs on (for example Claude Code or
Codex); a model and reasoning level ride on top.

```
zeron harness list
zeron model list claude-code
```

`harness list` shows installed and enabled harnesses. `model list <harness>`
shows the models that harness offers, with their reasoning levels.

`--model` accepts a full id or a shorthand that matches exactly one model
(`haiku` → `claude-haiku-4-5`). When a shorthand matches several, or none,
the error names candidate ids — run `model list` and pick one.

## Inheritance

A spawned chat inherits its parent's harness, model, reasoning level and
sandbox unless you pass `--harness`, `--model`, `--reasoning` on
`zeron chat spawn`. Model, reasoning and model options inherit only when
the harness stays the same — with `--harness` they come from the new
harness's defaults. Sandbox can only be lowered from the parent's, never
raised. Chats spawned outside a parent default to the workspace default
harness.

Pick the cheapest harness and model that can do the job: a cheap reviewer or
researcher costs the same supervision as an expensive one.
