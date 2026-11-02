# zb-replay v1

`CLAUDE_CODE_EXECUTABLE=/absolute/path/zb-replay` runs the actual Claude adapter
against a deterministic stream-json peer. No provider is used. No `claude`
executable is resolved implicitly. Without `--print` or `-p` it prints
`$ZB_REPLAY_VERSION (zb-replay)` (default `2.1.314`). Replay accepts unknown Claude launch flags; MITM validates the known
control-only launch contract. `--resume ID` and `--resume=ID` share the same state validation.

The environment selects `ZB_REPLAY_MODE=replay` (default), `ballast` (replay plus resident memory), or `mitm` (control-only real child plus synthetic short
turns). `zb:hold` emits `held` and completes; it keeps the CLI alive afterwards.
`zb:echo TEXT` emits TEXT. Other prompts use `ZB_REPLAY_DEFAULT_SCRIPT` when set.

## Script

UTF-8 JSONL, with a required meta line and then one step per line:

```jsonl
{"zb":"meta","v":1,"name":"example","seed":42,"text_bytes":5,"text_sha256":"2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824","frames":3}
{"dt":0,"f":{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}}}
{"dt":35,"f":{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hello"}}}}
{"dt":0,"f":{"type":"stream_event","event":{"type":"content_block_stop","index":0}}}
{"dt":0,"zb":"turn_end","usage":{"input_tokens":0,"output_tokens":0}}
```

Run `zb:play /absolute/path/example.zbr.jsonl speed=1 markers=0 loop=1` as the
engine prompt. Paths cannot contain whitespace. `dt` is milliseconds from the
previous step, divided by speed; speed=0 bursts. `frames` counts raw `f` lines,
not directives. Raw frame fields are preserved, including unknown types,
subagent frames and recorded result boundaries. If there are no raw assistant
frames, block stops synthesize full assistant frames. A final `turn_end` (or
end of file) synthesizes a result unless the recording already supplied results.
`$SESSION_ID`, `$MSG_ID`, `$CWD`, `$TOOL_ID` expand recursively in JSON string
values, without corrupting JSON escaping.

The hash is SHA-256 of concatenated top-level `text_delta.text` UTF-8 bytes.
Thinking, echoed prompts, tool outputs, and subagent text are excluded. Standard
one-loop playback validates both hash and byte count. `markers=N` adds up to N
evenly spaced markers (at most one per delta) and intentionally changes the hash;
loop playback returns the concatenated rendered text and persists its hash.

An explicit `{"dt":0,"zb":"marker"}` marks the next top-level text delta.
It appends ` [[zb:<chat8>:<seq>]]`; sequence starts at 1 per played turn and
continues across loops. The chat prefix is `ZB_REPLAY_CHAT`, then `ZERON_CHAT_ID`,
then `00000000`, truncated to eight characters. Explicit-marker scripts must put
the hash/size of their **rendered** text in meta, for a fixed chat prefix. After
the containing stdout line is flushed, a `mark` datagram records CLOCK_MONOTONIC
ns. A missing/full probe socket never blocks playback.

`{"dt":0,"zb":"exec","argv":["/usr/bin/true"],"cwd":"$CWD","tool":"Bash"}`
executes an argv without shell interpretation, inheriting the replay environment,
and emits tool_use/tool_result plus an `exec` probe. Shell semantics require an
explicit shell argv. These are local workload instructions, not provider tools.

## Memory, state, and observation

- `ZB_REPLAY_BALLAST_MIB`: private anonymous pages, touched off the control loop.
- `ZB_REPLAY_CHILDREN=N:MiB`: owned `--ballast-child MiB` processes idle in pause().
- `ZB_REPLAY_STARTUP_MS`: delay before the first session init, never initialize.
- `ZB_REPLAY_RESUME_MS_PER_MB`: added resume delay based on state file bytes.
- `ZB_REPLAY_STATE_DIR`: `<id>.json`, atomically replaced at turn end (including failed/interrupted turns).
  Defaults to `$CLAUDE_CONFIG_DIR/zb-replay-state` when available. Without either
  directory setting, state is disabled and resume fails. State includes session
  identity, turns, transcript, text_bytes and text_sha256. Resume validates all.
- `ZB_REPLAY_LOG_DIR`: argv/cwd/env names in argv-PID.json; env values are recorded
  only for MAKEFLAGS, CARGO_MAKEFLAGS, and names in `ZB_ENV_CAPTURE` (comma/space/
  colon separated). Prompts sent on stdin are never logged here.
- `ZB_PROBE_SOCK`: Unix datagram destination; `ProbeSock::bind(path)` owns a
  receiver and refuses to replace an existing socket. `ProbeEvent` is serde
  tagged by `k`: spawn, init, mark, exec, mcp_ready. Times are monotonic ns.

MITM requires an explicit `ZB_REPLAY_REAL_CLAUDE`. It starts that executable with
the same launch argv/environment/cwd. At the **child stdin boundary**, it rebuilds
only recognized control requests from whitelisted fields. User/image frames,
control responses (which may carry user tool input), unknown envelopes, and extra
prompt fields are never forwarded. Positional and system-prompt argv forms are
rejected before spawning the child. One private initialize response is cached for
engine discovery. Private mcp_status polls run at 250 ms until all servers are
connected/failed. Unsupported/missing status falls back to an unchanged Linux
process tree for five seconds; `fallback:true` and an empty server list identify
this weaker readiness measurement. Private responses are hidden from the engine.
`--tap` forwarding is deliberately unavailable in this unit: no real CLI was run
or authenticated to produce new recordings.

Immediate (`priority:now`) steering cancels a script; queued (`next`) steering
runs as a subsequent turn. There is no model step scheduler or real tool approval
execution. Recorded control requests can be played, but synthetic playback does
not wait for an approval reply. Live fixtures pin existing wire behavior; they do
not establish a new live 2.1.314 tap comparison.
