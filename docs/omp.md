# Oh My Pi native RPC

Zeron launches `omp --mode rpc-ui --allow-home` directly. Oh My Pi 18.11.0 or
newer is required; 18.11.1 is covered by a real-process test with a local mock
provider. OMP's RPC is a superset of Pi's, so Zeron reuses the Pi transport,
event normalizer, dialogs and MCP bridge, and drives OMP's own turn lifecycle.
Protocol details are in [PROTOCOL.md](../crates/harness/src/omp/PROTOCOL.md);
see [Pi native RPC](pi.md) for the shared behavior.

Install with `curl -fsSL https://omp.sh/install | sh` (Windows:
`irm https://omp.sh/install.ps1 | iex`) and sign in with `omp login`. Zeron
does not manage OMP credentials, so OMP has no Accounts section. `OMP_EXECUTABLE`
selects an explicit executable; otherwise Zeron uses its usual PATH, login-shell
and install-dir discovery (`~/.local/bin`, `~/.bun/bin`). The agent directory
follows OMP: a named profile (`OMP_PROFILE`, or `PI_PROFILE`), then
`PI_CODING_AGENT_DIR`, then `~/.omp/agent`.

Sessions retain their native id and JSONL history. Zeron records the id to
absolute file mapping in `<agent>/zeron-sessions` and reopens the exact file
with `--resume` after validating its header. OMP writes the file only at the
first turn, so a chat that never ran a turn starts fresh without a notice. A
session file that cannot be found starts a new conversation with a visible
notice. Slash commands that switch sessions (`/new`, extension commands)
refresh the id and file mapping before the turn completes.

Models and thinking levels come from `omp models --json`, which needs no model
requests. The Thinking option can disable reasoning; unsupported saved effort
levels are clamped to a supported lower level. Model switches do not persist
global defaults. Commands come from `get_available_commands` and
`available_commands_update`, including extension commands and `skill:*`
entries; OMP builtins such as `/compact` run natively and their output appears
in the transcript.

Steering queues at a model step boundary and starts immediately when idle.
Zeron always selects OMP's `all` steering mode, which OMP keeps per session, so
messages queued before the next model call enter that call together. Each input
completes once, with its `prompt_result` or a local completion; the turn ends
when every input has completed and background work has settled
(`session_settled`). Work that a background job starts after the turn ends
appears as a new turn. Interrupt sends `abort_and_restore_queue`, discards the
withdrawn queue, and terminates the owned process tree after a grace period.

Extension and `ask` dialogs (`select`, `confirm`, `input`, `editor`) use Zeron
questions; timeouts, OMP cancellations and interruption remove pending
questions. Notifications and OMP error notices appear in the transcript.
Terminal-only extension UI is not rendered.

Zeron's delegation uses the Pi MCP bridge loaded with `--extension`. Its tools
are registered as essential, because OMP hides discoverable extension tools
from the model until searched.

Validation:

```sh
cargo test -p zeron-harness --features native-fixture --test omp_rpc
cargo test -p zeron-engine --test omp_resume
cargo test -p zeron-harness --test omp_live -- --ignored --nocapture
cargo run -p zeron-harness --example omp_models_probe
```

The ignored OMP tests require an installed CLI. They use an isolated home and
agent directory with a local provider, and make no model API requests.
