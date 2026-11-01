# h5-claude-shim

Branch `perf/h5-claude-shim`. H7 v1 channel policy (`a8a9334d7`) merged
in `9fd4f469e`. No push, production engine/GUI, real Claude profile, credentials,
external model service, or real MCP fleet is used by this validation.

## Launch and fallback

The Claude driver keeps its existing MCP arguments/config unchanged. With a
plan, lazy enabled, a fresh H4 snapshot for the effective run config/project/CLI
fingerprint, and a sibling small helper supporting handoff version 1 via `--shim-version`, it writes
a private `run/mcp/<run>/shim-<uuid>/launch.json` and launcher symlinks. The
manifest is 0600; the directory is 0700. It contains matching/lifecycle facts,
not environment values. Only eligible bare heavy-runtime commands in that
snapshot become symlinks, derived through H7 classification. Absolute/relative
commands containing a slash stay eager.

The shim uses Linux `AT_EXECFN` to select its multicall name independently of
argv0, and checks `/proc/<ppid>/exe` plus executable identity. A direct Claude
child must match one unambiguous snapshot command, resolved executable and all
arguments, and have a lazy mode. Explicit channel rows stay eager regardless
of their requested mode. Shell children, altered argv0, stale/missing/corrupt
handoffs, unknown modes, mismatches and setup errors exec the next real command
on PATH. Resolution excludes the shim directory and all symlink/hardlink aliases
of the helper. Pass-through preserves literal argv0, non-UTF8 argv, environment,
file descriptors and exit status. The one environment difference: the exec'd
program's PATH lacks the shim directory (every other slot byte-identical), so a
tool that searches PATH for its own argv0, such as a venv or standalone Python,
finds itself instead of the shim. It starts no Tokio runtime or manifest IO on
the ordinary shell path. `which node`/`which npx` in an agent shell can show the
shim path.

Effective inherited env supplies both the real server and identity hash. Entry
env and Claude's plugin root/data variables are neither reconstructed nor put
in argv. Transient Zeron/Claude session markers, PWD/SHLVL/underscore are excluded
from identity; PATH identity excludes the changing shim directory while the
server inherits the actual PATH. Non-UTF8 matching/identity failures stay eager.
Both classification and catalog identities always include the actual startup
getcwd, even when --cwd was not configured. The catalog additionally includes
client/protocol/capability identity.

`mcp-cache/class-v1/<launch-identity>.json` stores only a digest and discovery
classification. The shared proxy core (used by Claude and Codex) execs a known channel or failed
discovery eagerly before creating Tokio or reading stdin. On the first identity, proxy discovery reads the server's own valid
initialize capabilities. A channel immediately becomes live forwarding, with
its original initialize result and notifications, without stopping/restarting
that server. Positive channel evidence cannot be overwritten by a concurrent
negative result. A non-channel completes ordinary capture and stops; its next
session answers from the catalog without starting the real process. Invalid
capabilities or failed discovery restart eagerly and replay the original
initialize before the client has received a response. Corrupt/unreadable
classification files fail through eagerly. No channel leases or channel drops
are used.

The runtime owns a shim guard. Spawn failures, resume-without-conversation
relaunches, ordinary teardown and cancelled tasks remove its directory. Normal
teardown awaits H1's numeric process-group/watchdog marker sweep after the
Claude group shutdown. SIGKILL still relies on H1's Linux parent-death signal
and retained run markers; engine boot sweeping belongs to H9a.

`ZERON_MCP_LAZY=0`, a missing plan (including the preferences toggle), title
runs, unavailable/old helpers, and snapshot misses omit the shim entirely.
Modes other than lazy remain legacy/eager under interception; this mechanism
does not remove configuration rows.

## Runtime reuse

`087a78dbb` moves the H7 stamp from drive_run to dispatch before both routing
and RuntimeConfig capture. Incoming client requests therefore compare their
current host preferences with the actual launch plan. Run directory alone
is not a policy change; modes, enabled state, idle settings and role changes
are. Claude also refuses in-place reconfiguration across a plan change.
`68445b09b` strengthens the offline engine regression to drive actual dispatch:
unchanged prefs retain
the runtime id, while a server-mode change and disabling lazy each replace
the runtime and deliver their new plans.

## Native fixture and measurements

The native integration test uses the production command builder and teardown
through a test-only helper. Its fresh allowlisted HOME/CLAUDE_CONFIG_DIR/XDG
roots, Git boundary, invalid API key, disabled hooks/nonessential traffic and
loopback-only deterministic Messages fixture follow H4 V-5. No credentials are
copied. The fixture never forwards requests and reports zero tokens/cost.

Two synthetic MCP servers (a user npx launcher and an installed marketplace
plugin node launcher) run the H1 Rust fake. Each touches 150 MiB of ballast and
starts a child plus detached watchdog. Seven Linux smaps_rollup PSS samples sum
only the Claude MCP subtree; they exclude Claude and the fixture driver. The
comparison is eager versus warm-cache idle lazy, before any tool call.

Final run: Claude **2.1.314**, Linux, debug binaries. Every native assertion
passed, including first and later channel calls, unchanged names/catalogs,
zero real servers on warm initialization, exactly one first-call server,
plugin root/data + entry env, allow/deny rules, Bash node/npx/python behavior,
and child/detached-watchdog teardown. First non-channel discovery stops its
server; first channel discovery retains the same server; later channel startup
has no proxy ancestor. `ZERON_MCP_LAZY=0` leaves PATH unmodified. The prefs
escape hatch and actual runtime replacement/reuse are covered in the engine
test. The fixture refuses machine-wide managed config so it cannot launch a
managed real fleet.

| MCP subtree state | Median PSS | Range (7 samples) | Processes in final sample |
|---|---:|---:|---:|
| Eager | **402,346 KiB (392.92 MiB)** | 390,098–402,357 KiB | 9 |
| Warm idle lazy | **3,020 KiB (2.95 MiB)** | 3,016–3,020 KiB | 2 |
| Saved | **399,326 KiB (389.97 MiB)** | | |

The sampler identifies the two MCP roots from fake-server PID marks and proxy
executables, then sums their descendant trees. It does not depend on npm's
mutable process title. A post-run check of all **53 recorded PIDs** found no
remaining /proc entries, including zombies. All numeric run markers and shim
directories were gone. The session fixture reported zero tokens/cost.

Final 300-pair warmed pass-through measurement (alternating run order):
baseline median **0.322 ms**, shim median **0.840 ms**, median paired added
latency **0.520 ms**. This uses /usr/bin/true, not a model or server.

### Exact commands and final results

All Cargo commands sourced this worker environment (no Cargo jobs argument):

```sh
source /home/user/Documents/dev/tmp/zeron-perf/scratch/h5-claude-shim/env.sh
```

That file sets the required isolated CARGO_TARGET_DIR, shared FIFO
CARGO_MAKEFLAGS, Swiftly-free PATH, TMPDIR, XDG_CONFIG_HOME, and /dev/null
GIT_CONFIG_GLOBAL from worker-rules.md.

| Command | Result |
|---|---|
| `cargo test -p zeron-harness --lib claude::` | **98 passed, 0 failed**. Its capture subprocess runs one of those cases separately; do not double-count that child result. |
| `cargo test -p zeron-mcp-proxy` | **19 unit + 16 process + 5 shim = 40 passed, 0 failed** after the review fixes below (35 before); bin/doc targets have 0 tests. Includes inherited-cwd catalog/classification partition and known-channel pre-initialize exec. |
| `cargo test -p zeron-engine --lib sessions::tests::mcp_plan_prefs_change_replaces_runtime_and_unchanged_plan_reuses_it` | **1 passed, 0 failed**, 649 filtered out. Actual offline dispatch, not a comparison-only mock. |
| `cargo test -p zeron-harness --features native-fixture --test claude_mcp_shim -- --ignored --nocapture` | **1 passed, 0 failed**, 11.92 s. Env exports below. |
| `git diff --check` | Passed. |

Native test exports (binary selection only; credentials/config are newly
synthesized inside the test):

```sh
export ZERON_TEST_CLAUDE=/home/user/.local/share/mise/installs/claude/2.1.314/claude
export ZERON_TEST_MCP_PROXY="$CARGO_TARGET_DIR/debug/zeron-mcp-proxy"
```

Latency command (choose a new scratch suffix to repeat):

```sh
python3 crates/mcp-proxy/tests/measure_shim.py \
  "$CARGO_TARGET_DIR/debug/zeron-mcp-proxy" "$TMPDIR/shim-latency-final"
```

The same zero-token Python runner also passed directly with:

```sh
python3 crates/harness/tests/fixtures/claude_mcp_shim.py \
  "$TMPDIR/native-complete" \
  /home/user/.local/share/mise/installs/claude/2.1.314/claude \
  "$CARGO_TARGET_DIR/debug/zeron-mcp-proxy" \
  "$CARGO_TARGET_DIR/debug/harness-claude-shim-fixture"
```

Earlier `cargo check -p zeron-harness` and `cargo check -p zeron-engine` passed;
the final test builds compile the subsequent shared-core changes. Python syntax
compilation passed for both new scripts. Existing unrelated dead-code/doc
warnings remain.

Development attempts were corrected before the final runs: the native helper
initially failed to compile due to an unsized Path conversion; the offline
engine fixture initially omitted the required steering_mode method. Native
fixture runs twice reported 0 passed/1 failed: first it assumed Claude would
retry a plugin immediately despite Claude's own 15-minute plugin failure cache;
then the stricter PSS selector assumed npm retained its original process title.
The fixture now checks the preserved skip behavior and traces PID ancestry.
These were fixture/measurement corrections; the final commands above all pass.
One queued native command was terminated before execution to add its explicit
binary env; that termination is not counted as a test pass.

### Evidence and files

Scratch evidence:

- `/home/user/Documents/dev/tmp/zeron-perf/scratch/h5-claude-shim/tmp/claude-shim-e2e-PXvzcN/result.json`
  (final PSS, cases and zero-token/teardown results; per-case frames beside it).
- `.../tmp/native-e2e-final.log`, `proxy-verified.log`, `harness-final.log`,
  `engine-test-final.log`, `shim-latency-final/latency.json`.

H5 files: Cargo.lock; the proto RunRequest stamp documentation; engine sessions.rs;
harness Cargo.toml, claude/mod.rs, mcp_capture.rs, mcp_plan.rs and this report;
harness tests/claude_mcp_shim.rs and fixtures/claude_shim.rs + claude_mcp_shim.py;
proxy README, src/{cache,config,discovery,lib,main,process,proxy,shim}.rs,
tests/{e2e,shim}.rs, tests/support/fake_mcp.rs and tests/measure_shim.py.
Other files in the H7 merge retain that unit's changes.

H5 commits before the final validation commit:

- `087a78dbb` Intercept Claude stdio MCP launches with a per-run lazy proxy shim
- `6fbeeb2b7` Preserve stable cache identity and literal argv0 through shim fallback
- `9fd4f469e` Merge branch 'perf/h7-mcp-policy' into perf/h5-claude-shim
- `a638c64af` Test runtime reuse against effective MCP preference changes
- `51d05d4b9` Learn channel capabilities during discovery and bypass known channel launches
- `68445b09b` Verify dispatch reuse and preserve live channel forwarding with eager fallbacks
- `5ec93458a` Share channel exec and discovery across proxy launchers and key caches by runtime cwd

The engine stamp fix is in `087a78dbb`; actual-dispatch coverage is in
`68445b09b` plus the final fixture trait-method correction. Shared core for H6
is `5ec93458a`; merge H5 through that commit to include its prerequisite Config
changes. No push was performed.

## Review fixes

Found by the release review and fixed in separate commits, each with a test:

- The shim directory stayed on the exec'd program's PATH, so a venv or
  uv/standalone Python (which finds its prefix by searching PATH for argv0)
  resolved the shim and lost its site-packages in every agent shell.
- Teardown left an empty `run/mcp/<run>` directory per launch.
- A plan change replaced a running CLI on macOS and Windows, where nothing is
  intercepted; `same_plan` now reports no change off Linux.
- A proxy setup failure (child-subreaper prctl, signal registration) failed the
  MCP server instead of exec'ing the real command.
- The shim's env identity hashed per-login/session variables (INVOCATION_ID,
  SSH_AUTH_SOCK, `__MISE_DIFF`, TMUX_PANE...), forcing rediscovery and leaving
  uncollected cache files.

## Limits

Claude interception is Linux native only. macOS and Windows deliberately omit interception and
keep their existing eager launches; no native validation was performed there.
The shared proxy core supports known-channel in-place exec on Unix; Windows,
which has no exec, uses its existing keep-alive forwarding/Job cleanup.
Script/wrapper Claude executables whose actual parent image differs also fail
through eagerly. The packaged small helper is required; the GUI-linked app is
never substituted into launcher symlinks.

The helper is a Rust binary, which sets SIGPIPE to ignore at startup and resets
it to default before exec. A caller that deliberately ignored SIGPIPE before
spawning a shimmed tool loses that inherited disposition (the ordinary
default-disposition case is identical). Failed discovery evidence and H1 cache
markers expire after 24 hours; the next launch retries discovery under the
existing startup timeout. Old classification failures without a timestamp retry
on the next launch. Positive channel evidence remains sticky.
After an engine SIGKILL the run and shim directories remain until the H9a boot
sweep.

A shell deliberately execing the exact MCP argv with an identical environment
and normal argv0 becomes indistinguishable from a direct MCP spawn. The
orchestrator accepted this three-condition boundary: it receives the same
transparent MCP protocol, with lazy startup. Ordinary shell commands and
custom argv0 are checked separately. Existing OAuth/connector behavior is
preserved structurally by not changing config; authenticated OAuth/connector
flows were not exercised. PSS is a synthetic debug-binary Linux measurement,
not a claim about the user's fleet or release binaries.
