# h4-claude-capture: installed Claude MCP verification

Recorded 2026-10-30 (America/Los_Angeles; UTC date 2026-10-31).
Claude **2.1.314**, Linux. No model service called, no real login, no real
credentials, no real MCP fleet, no Zeron engine/GUI, no push.

## Reproduction and isolation

The harness resolver checks its override, PATH/login-shell PATH and known
install locations, choosing the newest version. The PATH executable here is
`/home/user/.local/share/mise/command-wrappers/bin/claude` (a mise wrapper).
`mise which claude` returned
`/home/user/.local/share/mise/installs/claude/latest/claude`; its resolved
2.1.314 native binary was used directly so mise cannot depend on the isolated
HOME. `--version` in the isolated environment returned `2.1.314 (Claude Code)`.

Run from this worktree:

```sh
source /home/user/Documents/dev/tmp/zeron-perf/scratch/h4-claude-capture/env.sh
python3 crates/harness/tests/fixtures/claude_mcp_verify.py \
  /home/user/Documents/dev/tmp/zeron-perf/scratch/h4-claude-capture/verification-complete-2 \
  /home/user/.local/share/mise/installs/claude/2.1.314/claude \
  /home/user/Documents/dev/zeron-wt/targets/perf-h1-mcp-proxy/debug/zeron-mcp-proxy
```

Use a **new** scratch directory on reruns (the runner creates a Git worktree).
The checked-in runner builds synthetic user/project/local, marketplace plugin,
inline plugin, HTTP and channel fixtures. It copies **nothing** from the real
profile. Its subprocess environment is an allowlist: synthetic HOME,
CLAUDE_CONFIG_DIR, XDG directories, TMPDIR; `/dev/null` global Git config;
`ANTHROPIC_API_KEY=invalid-zeron-zero-token-verification`;
`ANTHROPIC_BASE_URL=http://127.0.0.1:1`; nonessential traffic/IDE attachment off;
and synthetic `CAPTURE_PARENT`/`CAPTURE_EXPAND` values. No credential file,
auth token, keychain value, inherited provider variable or key is copied.
The HTTP 401 case subsequently caused Claude to create its own synthetic
`.credentials.json` discovery cache (`mcpOAuth` with an empty access token and
no refresh token). It contains no usable credential. That file was never
copied; the Rust check used a fresh config-only profile.
Hooks are disabled. Git fixtures create their own repository boundary so
ancestor project settings cannot participate. Early exploratory runs without
that boundary were discarded; they inherited unrelated ancestor settings.
No model input was sent in those runs either.

The status/caps probes send only JSON control requests. V-5 additionally sends
a synthetic user message **only to a deterministic loopback HTTP fixture**,
which returns a fixed tool-use response. The fixture has no model, network
forwarder or usable key, and both the CLI and fixture report zero tokens/cost. Initialize reports
`account.tokenSource: "none"`, `apiKeySource: "ANTHROPIC_API_KEY"`.
`mcp_call` calls only our synthetic tool, with `result_not_for_model:true`.
The channel notification is synthetic; the invalid key and refused loopback
API endpoint also prevent a channel wake from reaching a model. HTTP fixtures
bind only loopback. No real OAuth tokens are used. All stdout/stderr/requests
and commands are retained under the scratch directory in `<case>.json`.
Each child is deliberately group-terminated after its answers; **143 is that
SIGTERM teardown, not a claimed successful CLI exit**.

In the commands/output below `$CLI`, `$PROXY`, `$F` and `$P` stand for the
native binary, WU1 standalone proxy, new scratch fixture root, and synthetic
plugin root (`$F/plugin`). `$COMMON` expands to:

```sh
-p --input-format stream-json --output-format stream-json --verbose \
--settings '{"disableAllHooks":true}'
```

All probes first send `initialize`, then `mcp_status`, polling `pending` rows
with further `mcp_status` controls until terminal. The recorded status shape is:

```json
{"type":"control_response","response":{"subtype":"success","request_id":"capture-2","response":{"mcpServers":[{"name":"plugin:playwright:playwright","status":"connected","serverInfo":{"name":"capture-fixture","version":"1.0.0"},"config":{"type":"stdio","command":"/usr/bin/python3","args":["/fixture/claude_mcp_fake.py","expanded-value","/fixture/plugin"]},"transport":"stdio","scope":"dynamic","source":"plugin","tools":[{"name":"browser_click","annotations":{}}]}]}}}
```

Paths are redacted to `/fixture` in the golden fixtures. No secret replacement
was necessary: all fixture values were synthetic. The complete final status
frames (including initial pending rows) are checked in at
[`../../tests/fixtures/claude_mcp_status.json`](../../tests/fixtures/claude_mcp_status.json).
Missing `env`, tool descriptions/input schemas, plugin provenance paths and
server capabilities in the actual status reply are **not invented**.

## Results

| Check | Verdict | Observed result / consequence |
|---|---|---|
| V-1 | PASS | `mcp_status` exists; strict retained only `zeron_caps`, dropping user/project/local/plugin fixtures. |
| V-2 | FAIL for lossless rewrite | Config has command/args, **omits env**. Expanded args and plugin environment were observed in the fake child. Snapshot alone cannot reproduce configured env. |
| V-3 | PASS | Child received both entry env and parent CLI env. |
| V-4 | NEEDS-USER | No authenticated claude.ai connector baseline in this credential-free profile. Absence is not evidence of strict preservation. |
| V-4b | PARTIAL / NEEDS-USER | Internal type with URL and ID is accepted; failed with `No claude.ai OAuth token found`. Successful redeclaration needs a real connector/login. |
| V-5 | PASS, deterministic local response | `system/init` lists normalized name. Existing allow rule runs tool without a permission request; removing it produces `can_use_tool`, and denial prevents execution. No model called. |
| V-6 | PASS | Dynamic same-name config overrides user scope, with replacement args. |
| V-7 | PASS with caveat | Disabling installed marketplace plugin + `--plugin-dir-no-mcp` retains its command/skill and omits its MCP. Identity/data path changes to `@inline` / `playwright-inline`; no claim of compatibility for arbitrary hooks/assets. |
| V-8 | NOT PROVEN / dynamic enable rejected | Fake dynamic channel connects; no channel delivery/wake is exposed in the control stream. `channel_enable` rejects a non-plugin server. Keep channel owners legacy. |
| V-9 | NEEDS-USER | Synthetic HTTP redeclaration works structurally and attempts connection, but no stored OAuth exists to test reuse. |
| V-10 | PASS | Worktree cwd **and nested cwd use the primary checkout path** for `projects[...]` local MCP. |
| V-P1 | PASS, local telemetry endpoint | Real 1.11.0 watchdog had a distinct pgid and exited in **71.1 ms** after server group SIGKILL. No browser launched. External telemetry latency not measured. |

### V-1: strict scope filtering and status existence

Commands (`baseline.json`, `strict.json`):

```sh
$CLI $COMMON --plugin-dir "$P"
$CLI $COMMON --plugin-dir "$P" --strict-mcp-config --mcp-config \
  '{"mcpServers":{"zeron_caps":{"command":"<PROXY>","args":["--data-dir","/fixture/data","--selftest"]}}}'
```

Baseline output (all connected):
`plugin:playwright:playwright (source=plugin, scope=dynamic)`,
`user_fixture (user/user)`, `project_fixture (project/project)`,
`local_fixture (local/local)`. Strict output: only
`zeron_caps (dynamic/dynamic)`, connected, tool `zeron_proxy_selftest`.
The plugin command and skill remain available even under strict; strict only
filters its MCP servers. Verdict: PASS.

### V-2 / V-3: config fidelity and child environment

Commands: V-1 baseline plus the normalized dynamic command below. The fixture
sets `CAPTURE_ENTRY=${CAPTURE_EXPAND}` in env and args,
`CAPTURE_ROOT=${CLAUDE_PLUGIN_ROOT}` (also passed in args), and `FAKE_DUMP_ENV=<fixture file>`.
The recorded plugin status has `args:["/fixture/claude_mcp_fake.py","expanded-value","/fixture/plugin"]`
and **no `env` key** for plugin, user, project, local or dynamic rows.
The child reports, using only allowlisted synthetic environment names:

```json
{"CAPTURE_ENTRY":"expanded-value","CAPTURE_PARENT":"parent-value","CAPTURE_EXPAND":"expanded-value","CAPTURE_ROOT":"/fixture/plugin","CLAUDE_PLUGIN_ROOT":"/fixture/plugin","CLAUDE_PLUGIN_DATA":"/fixture/home/.claude/plugins/data/playwright-fixture"}
```

The dynamic entry gets the first three values; CAPTURE_ROOT and both plugin variables are
null. The inline plugin has `CLAUDE_PLUGIN_DATA=.../playwright-inline`.
Claude expands variables, supplies plugin ROOT/DATA, and merges entry env into
its inherited environment. Rewriting to a dynamic entry would have to preserve
ROOT and DATA explicitly. V-2 fails the required complete capture; V-3 passes. A separate `proxy-env.json` probe uses the **real WU1 proxy**:

```sh
$CLI $COMMON --strict-mcp-config --mcp-config \
  '{"mcpServers":{"proxy_fixture":{"command":"<PROXY>","args":["--data-dir","/fixture/data","--server","proxy_fixture","--mode","keep-alive","--run-dir","/fixture/proxy-run","--","/usr/bin/python3","/fixture/claude_mcp_fake.py","proxy-env"],"env":{"CAPTURE_ENTRY":"${CAPTURE_EXPAND}","FAKE_DUMP_ENV":"/fixture/proxy-child-env.json"}}}}'
```

The proxied fake child again reports `CAPTURE_ENTRY=expanded-value`,
`CAPTURE_PARENT=parent-value`, `CAPTURE_EXPAND=expanded-value` (other allowlisted
plugin values null). `proxy_fixture` is connected with its tool, proving the
entry/parent merge also survives CLI → proxy → server.

The runtime caps probe additionally inserts a sentinel env entry: omitting it
makes `status_preserves_env=false` and `permits_rewrite()` false.

### V-4 / V-4b: claude.ai connectors

V-1 strict output has no claude.ai rows; neither does its baseline, because
there is no login. V-4: NEEDS-USER (requires an authenticated connector baseline).

Command (`connector-redeclare.json`):

```sh
$CLI $COMMON --strict-mcp-config --mcp-config \
  '{"mcpServers":{"connector_fixture":{"type":"claudeai-proxy","url":"http://127.0.0.1:1/mcp","id":"fixture-id","displayName":"Fixture","scope":"claudeai"}}}'
```

Output:

```json
{"name":"connector_fixture","status":"failed","error":"No claude.ai OAuth token found","config":{"type":"claudeai-proxy","url":"http://127.0.0.1:1/mcp","id":"fixture-id"},"transport":"claudeai-proxy","sessionProxied":true,"scope":"dynamic","source":"dynamic"}
```

The public config parser accepts the type when its required ID is supplied.
An earlier synthetic config without ID produced an empty list; that is not a
valid redeclaration test. Successful transport/auth/tool behavior needs a real
connector status row and login: V-4b PARTIAL / NEEDS-USER. Runtime caps represent
strict connector preservation as `null` when there was no baseline; WU5 must
not convert that to `true`.

### V-5: normalized tool names and permission rules

Command (`normalized.json`):

```sh
$CLI $COMMON --strict-mcp-config --mcp-config \
  '{"mcpServers":{"plugin_playwright_playwright":{"type":"stdio","command":"/usr/bin/python3","args":["/fixture/claude_mcp_fake.py","${CAPTURE_EXPAND}"],"env":{"CAPTURE_ENTRY":"${CAPTURE_EXPAND}","FAKE_DUMP_ENV":"/fixture/dynamic-env.json"}}}}'
```

Additional controls:

```json
{"subtype":"list_permission_rules"}
{"subtype":"get_settings"}
{"subtype":"mcp_call","tool":"mcp__plugin_playwright_playwright__browser_click","arguments":{},"result_not_for_model":true}
```

Outputs:

```json
{"behavior":"allow","source":"userSettings","rule":"mcp__plugin_playwright_playwright__browser_click","editability":"persistent"}
{"subtype":"success","request_id":"capture-5","response":{"content":[{"type":"text","text":"fixture ok"}]}}
```

`mcp_status` retains the exact dynamic name and tool basename. The controls-only
probe emits no `system/init`; host `mcp_call` establishes name lookup but does
not test the assistant-tool permission path. A second **zero-model** probe
closes that gap using a deterministic local HTTP API fixture:

```sh
python3 crates/harness/tests/fixtures/claude_mcp_permission_probe.py \
  /home/user/Documents/dev/tmp/zeron-perf/scratch/h4-claude-capture/permission-probe-final \
  /home/user/.local/share/mise/installs/claude/2.1.314/claude
```

It creates two fresh credential-free profiles. Its actual CLI command in each
profile is `$CLI $COMMON --permission-mode default --permission-prompt-tool stdio
--model haiku --strict-mcp-config --mcp-config <the normalized fake server>`.
`ANTHROPIC_BASE_URL` is a loopback HTTP server implemented by the probe itself;
`ANTHROPIC_API_KEY` is the deliberately invalid fixture key. The HTTP server
returns fixed SSE tool-use/text JSON with zero usage. There is **no model** or
network forwarding implementation. A synthetic user message reaches only that
local fixture. It drives the installed CLI's ordinary assistant-tool execution
and permission checking, while all model token/cost counters stay zero.

Output (`allowed/result.json` and `no-rule/result.json` under that scratch root):

```json
{"case":"existing allow rule","system_init_tool_present":true,"permission_requests":[],"calls":["browser_click"],"input_tokens":0,"output_tokens":0,"total_cost_usd":0,"is_error":false}
{"case":"allow rule removed, host denies","system_init_tool_present":true,"permission_requests":["mcp__plugin_playwright_playwright__browser_click"],"calls":[],"input_tokens":0,"output_tokens":0,"total_cost_usd":0,"is_error":false}
```

Both `system/init.tools` lists contain the **exact** normalized fully qualified
name. The fake MCP's call log and negative control demonstrate that the existing
allow rule matches, rather than a permissive mode bypassing it. Stderr empty;
runner exit **0**. Verdict: PASS for normalization and permission matching in the
actual CLI with a deterministic API response; no real model/real login used.

### V-6: dynamic override

Command (`override.json`):

```sh
$CLI $COMMON --mcp-config \
  '{"mcpServers":{"user_fixture":{"type":"stdio","command":"/usr/bin/python3","args":["/fixture/claude_mcp_fake.py","override"],"env":{"CAPTURE_ENTRY":"override"}}}}'
```

Output: `user_fixture`, connected, `scope=dynamic`, `source=dynamic`, final
argument `override` (baseline was `user`). Project/local rows remain.
Verdict: PASS.

### V-7: plugin without its MCP

Commands (`installed-plugin.json`, `no-mcp-plugin.json`):

```sh
$CLI $COMMON
$CLI $COMMON --settings \
  '{"disableAllHooks":true,"enabledPlugins":{"playwright@fixture":false}}' \
  --plugin-dir-no-mcp "$P"
```

The profile has a synthetic local `fixture` marketplace, a v2 installed plugin
record and `enabledPlugins.playwright@fixture=true`. In both launches,
initialize contains `playwright:capture-command` and
`playwright:capture-skill` (alias `capture-skill`). The second launch has only
user/project/local MCP rows; the plugin MCP is absent. Verdict: PASS for
command/skill preservation. The data directory changes from `playwright-fixture`
to `playwright-inline`. Disabling `playwright@inline` itself also hides the inline
commands: WU5 must disable the **original marketplace ID**, not the inline ID.

### V-8: fake channel

Commands (`channel.json`, `channel-enabled.json`):

```sh
$CLI $COMMON --strict-mcp-config --mcp-config '<dynamic channel_fixture JSON>'
$CLI $COMMON --strict-mcp-config \
  --dangerously-load-development-channels server:channel_fixture \
  --mcp-config '<dynamic channel_fixture JSON>'
```

The JSON uses the same fake Python stdio server with `env:{"FAKE_CHANNEL":"1"}`.
It declares `capabilities.experimental["claude/channel"]={}` and emits
`notifications/claude/channel` after `notifications/initialized`. Both status
outputs report connected with its tool. There is no channel notification/wake
in the CLI output. Extra control `{"subtype":"channel_enable","serverName":"channel_fixture"}`
returns:

```text
server channel_fixture is not plugin-sourced; channel_enable requires a marketplace plugin
```

Verdict: delivery NOT PROVEN; dynamic enable rejected. Channel delivery/auth
eligibility requires further testing in a user-authorized channel profile.
Do not proxy channel owners in v1. No Telegram plugin is used.

### V-9: strict HTTP OAuth identity

Command (`http-strict.json`; fixture port was 33537 in the earlier v4 run,
final run has its own recorded loopback port):

```sh
$CLI $COMMON --strict-mcp-config --mcp-config \
  '{"mcpServers":{"oauth_fixture":{"type":"http","url":"http://127.0.0.1:<port>/mcp"}}}'
```

The synthetic endpoint responds `401` with `WWW-Authenticate: Bearer realm="fixture"`.
Output: `oauth_fixture`, `status=failed`, `config.type=http`, same URL,
`transport=http`, `scope=dynamic`, `source=dynamic`. Config redeclaration attempts
the transport. It cannot prove OAuth cache reuse without a stored credential.
Verdict: NEEDS-USER; do not copy or use real OAuth tokens to finish this check.

### V-10: project key in worktrees

Commands (`worktree-key.json`, `nested-key.json`):

```sh
cd "$F/worktree" && $CLI $COMMON
cd "$F/worktree/nested" && $CLI $COMMON
```

The runner creates a primary repository at `$F/project`, a real linked worktree,
and distinct `projects[...]` MCP entries for primary/worktree/nested paths:
`main_key`, `worktree_key`, `nested_key`. **Both launches return `main_key` with
scope/source local**, not either other entry. Verdict: PASS. Fingerprinting
follows the worktree `.git` marker and `commondir` to include the primary key;
it also covers cwd/worktree keys conservatively. Files in the worktree remain
fingerprint inputs independently of this local MCP key.

### V-P1: real Chrome watchdog

Command:

```sh
python3 crates/harness/tests/fixtures/claude_mcp_watchdog.py \
  /home/user/Documents/dev/tmp/zeron-perf/scratch/h4-claude-capture/watchdog \
  /home/user/.npm/_npx/053a863a8198098a/node_modules/chrome-devtools-mcp
```

The runner launches installed `chrome-devtools-mcp` **1.11.0** with `/usr/bin/node`,
a new empty HOME/XDG config and its telemetry endpoint overridden to a local
HTTP fixture. It sends only MCP `initialize`; no Chrome/browser/tool operation.
Output:

```json
{"server_pid":1480487,"server_pgid":1480487,"watchdog_pid":1480606,"watchdog_pgid":1480606,"watchdog_exited":true,"elapsed_ms":71.1,"separate_group":true}
```

Initialize reports `serverInfo.name=chrome_devtools`, `version=1.11.0`.
The runner sends SIGKILL only to `-server_pgid`, polls watchdog state for five
seconds, and treats absent/zombie as exited. Stderr is the standard Chrome MCP
privacy/telemetry notice plus Node's localStorage experimental warning. Verdict:
PASS for process ownership/death detection; no claim about external telemetry
shutdown latency. The runner cleans up its own processes even on failure.

## WU5 contract and mandatory fallbacks

- `FingerprintInputs::key(cli)` + `SnapshotStore::load(key)` is the freshness
  check. A missing/corrupt/version-mismatched/stale snapshot means eager legacy
  discovery. Capture uses the existing eager initialize child in the background,
  then rechecks fingerprint/binary identity before publishing. Errors invalidate
  the matching key and never turn into an empty successful fleet.
- **Installed 2.1.314 cannot safely rewrite stdio configs from this snapshot:**
  `env` is omitted. Caps insert a known sentinel and record its omission.
  `ClaudeMcpCaps::permits_rewrite()` is false for this CLI, even though strict
  filtering and `config.command` work. Do not rebuild env from config files or
  silently substitute `{}`. Lazy MCP can remain enabled as a preference while
  this unsupported CLI takes automatic legacy fallback.
- WU5 must enforce the caps gate, per-row complete config, supported source and
  transport, complete tools/status, managed MCP and managed `serverCommand`
  policy constraints. Preserve native-small and remote transports only with
  verified lossless config/auth identity. Missing data is a legacy condition.
- When the CLI eventually provides lossless config, M1 needs normalized plugin
  names, original plugin ROOT/DATA, private per-run files, cleanup and runtime
  identity changes. V-5's normalization and permission path are covered by the zero-model fixture.
- Connector behavior is unknown without a login. Treat `strict_keeps_claudeai:
  null` as unknown. V-4/V-4b/V-9 remain merge gates. Use M2 only after connector
  and OAuth behavior is verified; V-6/V-7 establish its basic override/skill
  mechanics but arbitrary plugin data/hooks/assets still require care.
- Keep generic channel owners legacy; omit channel servers for nonowners only
  under the separate policy/lease contract. The status row does not expose the
  fake server's channel capability; absence of that field must not classify a
  potentially channel-enabled server as safe to proxy.
- No launch rewriting, policy toggle, engine integration or live-session capture
  was added here. Those belong to subsequent units. WU4 changes in `mod.rs` are
  confined to `probe_initialize`, plus the necessary module declaration.

## Validation

All Cargo commands use the worker's isolated target directory, shared FIFO
jobserver environment, filtered PATH, scratch TMPDIR/XDG, `/dev/null` global
Git config and isolated Claude config. No Cargo jobs flag is passed.
The exact build/test environment was:

```sh
export CARGO_TARGET_DIR=/home/user/Documents/dev/zeron-wt/targets/perf-h4-claude-capture
export CARGO_MAKEFLAGS="-j12 --jobserver-auth=fifo:/home/user/Documents/dev/tmp/zeron-perf/jobserver/fifo"
export PATH=$(echo "$PATH" | tr ':' '\n' | grep -v swiftly | paste -sd:)
export XDG_CONFIG_HOME=/home/user/Documents/dev/tmp/zeron-perf/scratch/h4-claude-capture/xdg
export GIT_CONFIG_GLOBAL=/dev/null
export TMPDIR=/home/user/Documents/dev/tmp/zeron-perf/scratch/h4-claude-capture/tmp
export ZERON_DATA_DIR=/home/user/Documents/dev/tmp/zeron-perf/scratch/h4-claude-capture/data
export CLAUDE_CONFIG_DIR=/home/user/Documents/dev/tmp/zeron-perf/scratch/h4-claude-capture/config
mkdir -p "$XDG_CONFIG_HOME" "$TMPDIR" "$CLAUDE_CONFIG_DIR"
```

| Exact command | Result |
|---|---|
| `cargo test -p zeron-harness --lib claude::mcp_capture` | **17 passed, 0 failed, 0 ignored**. Includes an isolated subprocess lifecycle test; its inner single-test result is not double-counted. |
| `cargo test -p zeron-harness --lib claude::discovery` | **2 passed, 0 failed**. |
| `cargo test -p zeron-harness --lib claude::tests::probe_timeout` | **1 passed, 0 failed**. |
| `cargo test -p zeron-harness --test claude_models` | **2 passed, 0 failed**. |
| `cargo test -p zeron-harness --test claude commands_come_from_the_initialize_control_request` | **1 passed, 0 failed**. |
| `cargo test -p zeron-harness --test claude discovery` | **2 passed, 0 failed, 1 ignored** (the existing real-login live test). |
| `cargo test -p zeron-harness --test claude_refresh_lock` | **2 passed, 0 failed**. Fixture updated to answer the new control while retaining its refresh-lock assertions. |
| `cargo build -p zeron-harness --example claude_mcp_capture_probe` | Exit **0**. |
| `git diff --check` | Exit **0**. |

Final selected suite: **27 passed, 0 failed; 1 live test ignored**. The initial
capture test run was **16 passed, 1 failed**: a golden-worktree assertion assumed
row position 1, but the final installed-plugin fixture prepended a plugin row.
It was corrected to assert by server name and scope. Later capture runs also
verify synchronous invalidation on initialize failure and strict rejection of
a plugin-sourced row even if its name happens to be `zeron_caps`. Existing
`zeron-sync::connect_with_tuned` dead-code warning remains; no new harness
warning. Python fixture syntax was checked with `ast.parse`; all four passed. The
permission fixture additionally passed its two CLI allow/deny cases.

### Rust capture/store/caps against the real CLI

The compiled `claude_mcp_capture_probe` example ran with a new
`/home/user/Documents/dev/tmp/zeron-perf/scratch/h4-claude-capture/rust-live/home`
and config root beneath it. Only synthetic `mcpServers`/`projects`, settings,
installed-plugin state and known-marketplace state were copied from the fixture;
no `.credentials.json` or other account state was copied. A guard aborted the
first wrapper invocation before spawning any CLI when it noticed the HTTP
fixture's empty discovery-cache file in the older profile.

With `$R` denoting that `rust-live` directory and `$F` the `verification-complete-2`
fixture, this is the executed environment/command (native example binary):

```sh
env -i PATH=/usr/local/bin:/usr/bin:/bin HOME="$R/home" \
  CLAUDE_CONFIG_DIR="$R/home/.claude" TMPDIR="$R/tmp" \
  XDG_CONFIG_HOME="$R/xdg" XDG_DATA_HOME="$R/data" XDG_CACHE_HOME="$R/cache" \
  GIT_CONFIG_GLOBAL=/dev/null ZERON_DATA_DIR="$R/data" CLAUDE_CODE_EXECUTABLE="$CLI" \
  ANTHROPIC_API_KEY=invalid-zeron-zero-token-verification \
  ANTHROPIC_BASE_URL=http://127.0.0.1:1 CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 \
  CLAUDE_CODE_AUTO_CONNECT_IDE=0 CLAUDE_CODE_IDE_SKIP_AUTO_INSTALL=1 \
  CAPTURE_PARENT=parent-value CAPTURE_EXPAND=expanded-value \
  /home/user/Documents/dev/zeron-wt/targets/perf-h4-claude-capture/debug/examples/claude_mcp_capture_probe \
  "$F/project" "$PROXY"
```

Output (paths/identity timestamp shortened):

```text
cli: <native Claude 2.1.314>
commands: 46; captured servers: 4
snapshot: <R>/data/claude-mcp/c0b4b47efd25817c5aeb9103c6e3f9855479738b62578ae7e308ac8e04bfacdb.json
```

```json
{"version":1,"cli_binary_identity":"<native-path>:<mtime>:298229464","strict_drops_plugins":true,"strict_keeps_claudeai":null,"status_has_config":true,"status_preserves_env":false,"probed_at_ms":1793407798933}
```

Exit **0**; both snapshot and `caps.json` checked as mode **0600**. This exercises
actual `probe_initialize` background capture, fingerprint, store and the Rust
caps subprocess against the installed CLI, without any model message. The
complete output is retained at `<R>/rust-probe.txt`.


## Remaining risks

Authenticated connectors/OAuth and channel notification delivery remain
unverified. V-5 covers the real CLI permission path using a deterministic local
API fixture; no live model service integration was exercised. macOS/Windows permissions and process
behavior were not exercised. V-P1 uses a local telemetry endpoint, so an
unreachable external endpoint could change watchdog exit latency. Snapshot
freshness follows the specified config inputs, not remote connector changes;
WU5 must preserve legacy behavior when account/connector state is unknown or
the launch environment differs from the captured expansion environment.
Binary identity matches the engine's canonical path/mtime/size identity;
stable launcher shims (for example mise) can change their target without a
metadata change. WU5 must use a resolved native CLI identity or fall back for
such shims before enabling rewrite. This verification pins the native 2.1.314
binary. Plugin contents use the specified mtime/size stamps; edits preserving
both cannot be detected. Snapshot write errors, absent proxy selftest support,
and negative caps all require legacy launch. No end-to-end lazy launch or
memory reduction is claimed by this unit.
