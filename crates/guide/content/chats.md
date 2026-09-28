# Chats

A chat is one agent conversation. `zeron chat` spawns, messages, waits on and
reads chats on any device in the workspace.

Everywhere a command takes `<chat>`, you may pass a full chat id, a unique id
prefix, an exact title, or `self` for your own chat (the `ZERON_CHAT_ID` env).
Every command accepts `--json` for machine-readable output; human output goes
to stdout, logs to stderr. Inside a chat, `zeron` always runs the binary of
the Zeron that started your chat (`ZERON_CLI`), whatever PATH resolves.

## Spawn

```
zeron chat spawn --prompt "Summarize the diff" --title "Diff summary"
zeron chat spawn --prompt-file - --harness claude-code --model haiku --reasoning high
zeron chat spawn --prompt "Fix the flaky test" --wait --timeout 20m
```

`--prompt` and `--prompt-file` are exclusive; `--prompt-file -` reads stdin.
`--harness`, `--model`, `--reasoning` and the project/environment inherit the
parent chat when omitted (see `zeron guide environments`). Human output ends
with a line `@chat:<full-id>` — that line is how the UI links the chip.

`--parent <chat>` attaches the child under another chat; `--no-parent` spawns
a top-level chat. With `--wait`, spawn returns after the child's first turn
settles.

## Talk to a chat

```
zeron chat tell @chat:3f6b2a18-… "Use serde_json, not manual parsing"
zeron chat tell 3f6b2a18 --message-file ./instructions.md --mode queue
zeron chat tell 3f6b2a18 "ping" --wait
```

Modes: `auto` (default — start a turn when idle, steer a live turn at the
next input boundary), `steer` (deliver into a running turn), `queue` (hold
until the current turn ends). A chat waiting for your answer rejects `tell`;
use `answer` instead.

## Wait and read

```
zeron chat wait 3f6b2a18 9c4d2b77 --timeout 20m
zeron chat wait --any 3f6b2a18 9c4d2b77
zeron chat output 3f6b2a18
zeron chat log 3f6b2a18 --limit 20 --tools --reasoning
zeron chat show 3f6b2a18
```

`wait` blocks until a chat's turn settles (or `--any` of them) and returns the
outcome. `output` prints the last assistant reply of the latest settled turn.
`log` prints the transcript; `--tools` and `--reasoning` widen it. `show`
prints the summary: status, harness and model, cwd and branch, pending input
and children.

Waiting inside your own chat also acknowledges the child updates that chat
already received, so they are not delivered twice.

## Manage

```
zeron chat interrupt 3f6b2a18
zeron chat answer 3f6b2a18 "Option B" --request req-9
zeron chat archive 3f6b2a18 --unarchive
zeron chat fork 3f6b2a18 --prompt "Continue from here"
zeron chat list --children --project comet
```

`interrupt` stops the live turn. `answer` replies to a pending input request.
`archive` hides the chat and its descendants; `--unarchive` restores it.
`fork` copies a chat's settled history into a new chat. `list` shows chats;
`--children` limits to one parent's children.

## Exit codes

`0` ok or completed, `1` error, `2` awaiting input, `3` errored turn,
`4` interrupted, `5` limit reached, `124` timed out. Durations accept `90s`,
`20m`, `1h`, or bare seconds.
