# Oh My Pi RPC contract used by Zeron

Zeron drives `omp --mode rpc-ui --allow-home` (Oh My Pi ≥ 18.11.0) over JSONL
stdio. OMP's RPC is a superset of Pi's, so the transport, normalizer, dialogs and
MCP extension are shared with `../pi`. The lifecycle is OMP's own; this file lists
only where it differs from `../pi/PROTOCOL.md`. Upstream reference:
`docs/rpc.md` in `can1357/oh-my-pi`.

## Launch

- `--no-themes` is not an OMP flag (unknown flags exit), so it is not passed.
- `--mode rpc-ui` enables the tool UI context, so the agent's `ask` tool reaches
  Zeron as `select` / `editor` dialogs. `set_ask_dialog` is not enabled; one
  question per dialog keeps the shared Pi dialog mapping.
- `--allow-home` stops OMP from moving a chat started in `$HOME` to a temp dir.
- `PI_CODING_AGENT_DIR` (OMP reads Pi's variable name) is set only for an
  explicit agent directory, and then `OMP_PROFILE`/`PI_PROFILE` are removed:
  a named profile makes OMP ignore the variable. Otherwise OMP resolves its
  own, and Zeron's reads follow `utils/src/dirs.ts`: a named profile
  (`OMP_PROFILE`, or `PI_PROFILE` when `OMP_PROFILE` is unset; empty or
  `default` means none) at `~/<PI_CONFIG_DIR or .omp>/profiles/<name>/agent`,
  then `PI_CODING_AGENT_DIR`, then `~/<PI_CONFIG_DIR or .omp>/agent`. Not
  mirrored: `XDG_*_HOME/omp` after `omp config init-xdg`, and a
  `PI_CODING_AGENT_DIR` set from an agent `.env` file.
- Auth lives in `<agent>/agent.db`. The model catalog keys on its credential
  listing (provider, type, enabled), never on token rows, plus `config.yml`,
  `models.yml` and `.env`.
- RPC modes already disable title generation (`PI_NO_TITLE=1`).

## Handshake

OMP writes `{"type":"ready","supportedProtocolVersions":[1,2],...}` first. When
v2 is advertised, Zeron sends `negotiate_protocol {protocolVersion:2}`. In v2
an oversized stdout object arrives as consecutive `rpc_chunk` frames
(`chunkId`, `index`, `count`, `byteLength`, base64 `data`), which the shared
transport reassembles and validates (64 MiB cap). In v1 a response over 1 MiB
would fail, and `get_state` carries the system prompt and tool dump.

## Turn end

There is no `agent_settled`. Every accepted `prompt` completes exactly once:

- its response carries `data.agentInvoked: false` (a builtin slash command
  finished locally; no `prompt_result` follows), or
- a later `{"type":"prompt_result","id",status,error?,sessionSettled}`.

`status` is `completed`, `aborted` or `error` (`error.message`). A prompt that
fails before reaching the agent gets an error response first; the turn does not
wait for its `prompt_result`, which is ignored if it follows. An `agent_end`
with `yielded: false` (retry, compaction) completes no prompt. When every prompt of
the turn has completed, Zeron sends one `get_state` to refresh session identity
(a slash command can switch sessions), then emits `Done`.

`sessionSettled: false` means background work (async task, backgrounded bash)
can still wake the agent; the turn stays open until `session_settled`. A run a
background job starts after `Done` (`agent_start` while idle) is a new turn and
ends at `session_settled`.

## Steering and queue

- Inputs are `prompt` with `streamingBehavior: "steer"` (the first prompt uses
  `"followUp"`, which is ignored when idle). The next input is sent only after
  the previous one is admitted (its response), which keeps Zeron's order.
- `Steered` is emitted when the input's user `message_start` arrives, or when it
  completes locally.
- `set_steering_mode all` is always sent: in OMP it is session-scoped and never
  persisted to `config.yml`.
- Interrupt sends `abort_and_restore_queue` (OMP has no `clear_queue`) and
  discards the withdrawn queue.

## Commands

`get_commands` is not served. Zeron uses `get_available_commands` and
`available_commands_update` (`name`, `description`, `input.hint`). Builtins such
as `/compact` are sent as ordinary prompts and run by OMP; their text arrives in
`command_output` frames.

## Models

`omp models --json` lists the same models as RPC `get_available_models`, with
per-model `thinking` levels, so the catalog needs no `set_model` round trips.
Model ids are `provider/id`.

## Sessions

Files live under `<agent>/sessions/<project>/<timestamp>_<id>.jsonl`. Line 1 is a
padded `{"type":"title"}` slot; line 2 is `{"type":"session","id"}`. Zeron keeps
`<agent>/zeron-sessions/<sha256(id)>.json` and resumes with `--resume <file>`.
OMP creates the file lazily at the first turn, so a session with no turn cannot
be resumed; Zeron then starts fresh without a lost-context notice.
