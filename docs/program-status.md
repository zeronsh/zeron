# Agent and terminal status

Zeron's native harnesses report structured lifecycle events. `InputRequested`
and `InputResolved` drive needs-input/working; `Done` records completed,
interrupted, or errored independently of whether a persistent harness process
remains alive. Interrupted turns do not produce successful completion receipts.

Managed PTYs additionally receive the [Program Status Protocol, OSC 7501](https://www.superlogical.com/rex/docs/build/program-status)
(revision 0.4). The engine parses reports and answers capability probes, including
when no terminal viewport is attached. Raw terminal output is preserved. Programs
that require detection, including compatible interactive Claude Code versions,
can discover support without an environment override.

## One display projection

`zeron_proto::view::display_status` is shared by desktop, client/mobile views, and
MCP. Native execution state and terminal-reported activity are independent:

| Evidence | Display |
| --- | --- |
| A native or terminal input request | `awaitingInput` / Needs input |
| Native work or a working terminal record | `working` |
| Unseen native failure or terminal error | `errored` |
| Unseen completed response or terminal `done` | `completed` / Done |
| Acknowledged results or interrupted native turn | `idle` |

Needs input takes priority over working, and working over settled results.
An idle/done root record cannot hide a working or blocked child. Native status
and `last_completed_turn` are never overwritten by OSC reports. A terminal
report is display evidence, not authorization to approve a permission request
or proof that a native turn completed.
Composer Stop/steer controls continue to use native execution state; a terminal
report does not create a native run to interrupt.

The engine leases live terminal activity into the ordinary session watch and
synced registry every ten seconds. Programs do not have to heartbeat. A live
session lease older than 45 seconds is stale: it does not display Working,
Needs input, or a false successful completion. Settled results remain until seen
or cleared. Terminal completions use the chat's existing synced seen marker,
including when there is no new assistant message.

## MCP contract

`list_chats`, `get_chat`, and `wait_for_turn` return the same `status` as the
sidebar (`working`, `awaitingInput`, `completed`, `errored`, `idle`). Additional
fields distinguish the sources:

- `sessionStatus`: native execution state, including idle while a terminal works.
- `statusStale`: whether live evidence expired; expiry is not completion.
- `lastTurnOutcome`: latest native status and `finishedAt`, when present. A
  repeated interruption is distinct even if its Working update was coalesced.
- `terminalActivity`: terminal id, selected program record, and actual report time.
  The record includes its id, state, optional input kind, app, title, message,
  and progress. This timestamp is not rewritten by heartbeat refreshes.

Native `pendingInput` is answered with `respond_to_input`. A terminal-only
permission/question/auth request is handled in its terminal; it does not create
a native input resolver. `wait_for_turn` reports `interrupted` for interrupted
native work and waits for fresh evidence or its deadline when a host is stale.

## Protocol lifecycle and limits

Records are tracked at the PTY owner and survive viewport detach/reattach.
`clear` removes the addressed subtree; a root clear and RIS remove all records.
OSC 133 A (a new shell prompt) and PTY exit remove live records; done/error
records survive those events. Closing the terminal removes its records.
Alternate-screen changes and soft reset do not clear status. Programs should
report done/error before exiting; a terminal cannot infer the state of an
unreported foreground child from silence.

The receiver bounds a sequence to 4096 bytes and each terminal to 256 records,
validates ids, UTF-8/base64 text and protocol field limits before applying a
report, and disarms bidi overrides in displayed messages. It handles sequences
split across arbitrary PTY chunks and ignores OSC-shaped data inside other
terminal control strings. Only one bounded summary per chat is replicated.

## Verification

`cargo test -p zeron-engine --test osc7501` exercises a real Unix PTY through the
engine, registry, shared sidebar derivation, and MCP. It also runs a capability
probe without a terminal emulator. Parser unit tests cover fragmentation,
malformed/oversized reports, hierarchical records, replacement, and lifecycle
clearing. Registry tests verify replication, persistence, and clearing additive
fields. Native interrupt integration coverage checks that sidebar/MCP do not
report a cancelled turn as successful completion.

For native visual evidence, run the isolated production Shell fixture:

```sh
cargo run -p zeron-ui --example program-status-fixture \
  --features appshots-fixture,resource-profile -- /tmp/zeron-program-status-captures
```

It sends real OSC reports through three PTYs and captures working, needs input,
and done in dark/light themes, then acknowledges the done result. The transcript
table is generated from the shared status projection. Captures must be outside
the checkout. Linux capture uses Hyprland and `grim`; macOS uses native window
rendering. No agent or external provider is invoked.
