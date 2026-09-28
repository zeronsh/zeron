# JSON output and exit codes

Every `zeron` command accepts `--json` and then prints one JSON value on
stdout (logs stay on stderr). The shape is per command — run the command once
with `--json` to see its fields rather than guessing names.

## Exit codes

| Code | Meaning |
|------|---------|
| 0    | ok / turn completed |
| 1    | error (the message on stderr says what) — also usage errors: unknown command, bad flag |
| 2    | the chat is awaiting input — use `zeron chat answer` |
| 3    | the turn errored — read `zeron chat log` or `show` for the failure |
| 4    | the turn was interrupted |
| 5    | a limit was reached (too many running children, depth cap) |
| 124  | timed out — the `--timeout` duration elapsed |

Timeouts accept `90s`, `20m`, `1h`, or bare seconds.

Branch on the code, not the wording: `0` means the turn completed, `124`
means your deadline elapsed with the chat still working — worth a retry;
`2` and `3` need a different next step, and `4` means a human may have
stopped it on purpose.
