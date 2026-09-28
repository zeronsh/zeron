---
name: zeron-cli
description: Use when you need to delegate work to child chats, run agents in parallel, or coordinate with and read other chats in Zeron.
---

# Orchestrating chats with the zeron CLI

## Core loop

1. `zeron chat spawn --prompt "…"` (add `--harness`/`--model`, and `--wait` only
   when you need the result immediately). The output ends with
   `@chat:<full-id>` — keep that id.
2. Let the child work. Zeron sends you a `[Zeron system]` message when it
   completes, fails, is interrupted or needs help. Do not poll.
3. Read the result with `zeron chat output <id>` or `zeron chat log <id>`,
   and act on it.

## Rules

- Only spawn or message chats when the user has explicitly asked for it.
- Never poll with sleeps or repeated `show`/`log` reads — notifications
  arrive on their own.
- Reference a chat as `@chat:<full-id>`, never a URL.
- Limits: depth 4 of nesting, 8 running children per parent. A refused spawn
  exits 5 — batch or wait before spawning more.
- Send follow-ups with `zeron chat tell <id> "…"`; answer a blocked child
  with `zeron chat answer`.

## References

- [commands.md](references/commands.md) — full `zeron chat` command reference.
- [patterns.md](references/patterns.md) — fan-out/fan-in, reviewer loops,
  unblocking and recovering children.
