# `zeron chat` command reference

`<chat>` resolves by full id, unique id prefix, exact title, or `self`.
Every command accepts `--json`. Durations accept `90s`, `20m`, `1h` or bare
seconds.

## spawn

```
zeron chat spawn (--prompt <text> | --prompt-file <path|->)
    [--harness h] [--model m] [--reasoning r] [--title t]
    [--project p] [--device d]
    [--worktree [--base <ref>] | --same-checkout | --cwd <path>]
    [--parent <chat> | --no-parent]
    [--wait [--timeout 20m]]
```

Harness, model, reasoning and sandbox inherit the parent; sandbox can only be
lowered. `--model` takes a full id or a unique shorthand (`haiku` → `claude-haiku-4-5`); ambiguous or unknown shorthands error listing candidates.
Environment defaults to a new worktree of the parent's repo.
`--parent` nests under another chat, `--no-parent` spawns top-level. Human
output ends with `@chat:<full-id>`.

## tell

```
zeron chat tell <chat> (<text> | --message-file <path|->)
    [--mode auto|steer|queue] [--wait [--timeout]]
```

`auto` starts a turn when idle and steers a live one; `steer` delivers into a
running turn; `queue` holds until the turn ends.

## wait

```
zeron chat wait <chat>... [--any] [--timeout 20m]
```

Blocks until the turn(s) settle; `--any` returns at the first. Exit code is
the outcome (see the json chapter). Inside the parent chat, `wait` and
`output` ack the updates already delivered there.

## output

```
zeron chat output <chat>
```

Prints the last assistant reply of the latest settled turn.

## show

```
zeron chat show <chat>
```

Summary: status, harness and model, cwd and branch, pending input, children.

## log

```
zeron chat log <chat> [--limit N] [--tools] [--reasoning]
```

The transcript; `--tools` includes tool calls, `--reasoning` includes
thinking.

## list

```
zeron chat list [--children [<chat>]] [--project p] [--archived]
```

## manage

```
zeron chat interrupt <chat>
zeron chat answer <chat> <answer>... [--request <id>]
zeron chat archive <chat> [--unarchive]
zeron chat fork <chat> [--prompt ...]
```

## misc

```
zeron harness list
zeron model list <harness>
zeron guide [chapter]
```

## Exit codes

0 ok/completed · 1 error — also usage errors (unknown command, bad flag) ·
2 awaiting input · 3 errored turn · 4 interrupted · 5 limit reached ·
124 timed out.
