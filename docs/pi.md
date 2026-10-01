# Pi native RPC

Zeron launches `pi --mode rpc` directly. Pi 0.85.1 or newer is required; 0.85.1
is covered by a real-process test with a local mock provider. Protocol details
and the acceptance barrier are in [PROTOCOL.md](../crates/harness/src/pi/PROTOCOL.md).

Install and authenticate Pi through its CLI. `PI_EXECUTABLE` selects an explicit
Pi executable; otherwise Zeron uses its usual PATH, login-shell and install-dir
discovery. `PI_CODING_AGENT_DIR` continues to control Pi settings and credentials.
`PI_ACP_EXECUTABLE` and `PI_ACP_PI_COMMAND` are no longer used. Zeron neither
installs nor launches `pi-acp`, and existing adapter files need not be deleted.

Sessions retain their native UUID and JSONL history. Zeron records the UUID to
absolute file mapping in `PI_CODING_AGENT_DIR/zeron-sessions` (normally
`~/.pi/agent/zeron-sessions`). For older chats it also reads
`~/.pi/pi-acp/session-map.json`, then searches native session directories,
including configured `sessionDir` locations. It validates the session header
before reopening the exact file. A session that cannot be found starts a new
conversation with a visible notice, as other harnesses do. A session that Pi has not yet
written can retain its UUID through `--session-id` only after ordered native
queries prove it has no conversation or extension entries. The proof is revoked
before another input is sent. Unsaved custom extension state cannot be recreated:
Pi only materializes it after its first assistant response, so if that file
never existed the chat continues in a new session with the same notice. Session changes made by extensions
refresh the UUID and file mapping before turn completion.

Models and supported thinking levels come from native RPC discovery. The
Thinking option can disable reasoning. Unsupported saved effort levels are
clamped to a supported lower level. Discovery uses a temporary no-session
process; model switches do not persist global defaults. Commands and skills
are discovered in the workspace. Pi controls project-extension trust.

Steering queues at a model step boundary and starts immediately when idle.
When no steering mode is configured, Zeron selects Pi's `all` mode, so messages
queued before the next model call enter that call together. Pi persists this
mode in its global settings. A mode already set in Pi's global or project
settings (including one chosen with `/steering`) is never overwritten.
Each input is sent as soon as the preceding preflight and ordered state query
finish, without waiting for earlier queued inputs to be consumed or adding a
batching delay. Inputs arriving after a model call starts belong to a later step.
Zeron confirms each original message only when Pi consumes it. Extension commands
and inputs handled without a model run retain serialized delivery.
Interrupt clears queues, aborts
the run and terminates the owned process tree after a grace period. A completed
model iteration (`agent_end`) alone does not close the turn: retries,
compaction and handled extension commands follow the native lifecycle.

Extension `select`, `confirm`, `input` and `editor` dialogs use Zeron questions.
Editor content preserves whitespace and prefill; desktop supports Shift+Enter
and iOS uses a multiline editor. Timeouts and interruption remove pending
questions. Notifications appear in the transcript. Terminal-only extension UI
such as widgets and custom TUI components is not rendered.

Zeron's existing delegation uses a temporary MCP bridge loaded with
`--extension`. Tools and chat identity retain the existing engine contract;
no Pi subagent extension is installed. Image attachments are sent as native
image blocks; image-only tool results do not yet render inline in Zeron.

Validation:

```sh
cargo test -p zeron-harness --features native-fixture
cargo test -p zeron-engine --lib --test pi_resume --test acp_lifecycle --test message_queue --test e2e
cargo test -p zeron-harness --test pi_live -- --ignored --nocapture
cargo check -p zeron-ui --tests
cargo build -p zeron-mobile --features bindgen
```

The ignored Pi test requires an installed CLI and uses isolated settings and a
local provider; it makes no model API requests. Native iOS UI compilation still
requires Xcode on macOS.
