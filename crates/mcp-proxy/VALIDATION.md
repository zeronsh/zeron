# h1-mcp-proxy validation, 2026-10-30

Branch: `perf/h1-mcp-proxy`; base: `55ffd4142`. WU3 → WU1 → WU2,
one commit per work unit; no push or PR. No real MCP fleet, GUI, engine
instance, real Zeron data directory, or workspace-wide test was launched.

Every Cargo command used:

```sh
export CARGO_TARGET_DIR=$HOME/Documents/dev/zeron-wt/targets/perf-h1-mcp-proxy
export CARGO_MAKEFLAGS="-j12 --jobserver-auth=fifo:$HOME/Documents/dev/tmp/zeron-perf/jobserver/fifo"
export PATH=$(echo "$PATH" | tr ':' '\n' | grep -v swiftly | paste -sd:)
export XDG_CONFIG_HOME=$HOME/Documents/dev/tmp/zeron-perf/scratch/h1-mcp-proxy/xdg
export GIT_CONFIG_GLOBAL=/dev/null
mkdir -p "$XDG_CONFIG_HOME"
```

Subsequent commands also set `TMPDIR` to
`$HOME/Documents/dev/tmp/zeron-perf/scratch/h1-mcp-proxy/tmp`.
The reusable environment is recorded at
`/home/user/Documents/dev/tmp/zeron-perf/scratch/h1-mcp-proxy/env.sh`.
No Cargo jobs argument was passed.

Exact build/test commands and results:

| Command | Result |
|---|---|
| `cargo test -p zeron-mcp-proxy --lib` | Initial compile failed with E0502 in pagination; fixed before the successful package runs. No tests executed in this attempt. |
| `cargo test -p zeron-mcp-proxy` | Initial WU1 run: 5 unit + 10 E2E passed, 0 failed. Final run: **12 unit + 13 E2E passed, 0 failed, 0 ignored**; binary/doc suites each had 0 tests. |
| `cargo check -p zeron` | Passed, exit 0, including both app binaries and the final proxy source. Existing warnings in other crates; no proxy warnings. Cold dependency check took 16m 46s. |
| `bash -n scripts/package-linux.sh scripts/package-macos.sh` | Both syntax checks passed, exit 0. |
| `git diff --check` | Passed, exit 0. |

The final package test included lock waiting behind the app check and took
12m 05s overall; its E2E suite itself took 2.05s. Tests only use the fake
server and target-local temporary directories. The 18-case npx parser table
is one of the 12 unit tests, not 18 independently counted test cases.

Coverage includes cache identity/order/freshness/permissions/privacy/TTL,
failed-discovery passthrough, pagination, cold first call, cached initialization
without a child, eager startup, idle stop/restart, startup timeout, crash
backoff and retry, sampling, progress, queued and in-flight cancellation,
logging/roots replay, in-flight/subscription/server-request idle holds, tool
catalog changes, EOF/SIGTERM cleanup, descendant and detached-watchdog cleanup,
SIGKILL with persistent run markers, and the in-process selftest. Resolver
fixtures cover pins and npm version spellings, newest-cache fallback,
top-level-spec preference over transitive copies, symlink realpaths, JS versus
native bins, composed PATH/first-node selection, cache expiry/corruption/missing
bins, runtime-argument privacy, direct launch, unsupported flags, and the
`--no-npx-resolve` escape hatch.

Proxy-only Linux PSS was measured with:

```sh
python3 crates/mcp-proxy/tests/measure_pss.py \
  "$CARGO_TARGET_DIR/debug/zeron-mcp-proxy" "$CARGO_TARGET_DIR/debug/fake-mcp"
```

Final binary, seven `/proc/<proxy-pid>/smaps_rollup` samples per state:

| State | Median PSS | Min–max |
|---|---:|---:|
| Cached idle, verified no server spawn | **2,279 KiB (2.23 MiB)** | 2,279–2,279 KiB |
| Outstanding forwarded fake call | **2,564 KiB (2.50 MiB)** | 2,556–2,568 KiB |

These numbers exclude the fake server. This is a debug binary, not a release
build or the full GUI-linked `zeron` fallback. Earlier WU1 binary samples were
2,120/2,400 KiB and 2,104/2,389 KiB idle/forwarding; final numbers above include
the resolver and reviewed lifecycle code.

Plan adjustments and remaining platform/integration checks:

- The small binary is included proactively to meet the budget without linking
  the app image into each proxy. Linux/macOS packaging and macOS signing were
  updated. Their existing updater paths replace the complete archive/bundle.
  Native packaging/signing was not executed. Windows retains the in-process
  core because its updater currently delivers one executable; its PSS has not
  been measured. The app subcommand was type-checked, not launched as a full
  GUI-linked executable.
- Added `--cwd` so harness integrations can explicitly distinguish configured
  cwd from the session's inherited cwd in cache identity. Later drivers should
  pass it only for an explicit server cwd.
- Per-key advisory lock files supplement atomic renames to guarantee newer
  writers win across the read/rename race. Linux additionally uses subreaper
  ownership to reap descendants and identify adopted telemetry watchdogs.
- A cold npm cache falls back to the original npx/npm command, which performs
  its ordinary install. The proxy does not add pi's preliminary force-install
  subprocess. Floating specs re-scan npm installs on the next proxy so daily
  engine refreshes can take effect within the resolution-cache TTL. The daily
  nice/pressure-aware refresh is outside WU1/WU2 and remains engine work.
- Tests use actual fake-process stdio instead of a second scripted-transport
  abstraction. Millisecond knobs exercise the same production timeout logic;
  no ten-minute wall-clock idle wait was performed.
- macOS process-group/ps behavior and Windows suspended-child/Job cleanup are
  implemented but require native testing. Windows npx resolution is deferred
  per plan. `bunx` remains passthrough: the cited parser and requested resolver
  cover npx/npm exec; the plan's broader bunx sentence needs separate handling.
- Real Chrome watchdog V-P1 and Claude/Codex live harness verification belong
  to WU4 and later. Engine teardown/boot must call the exported
  `cleanup_run_dir()`; that helper's real group cleanup was exercised here.

Files changed:

- Workspace `Cargo.toml`, `Cargo.lock`; `THIRD_PARTY_NOTICES.md`.
- `apps/zeron/Cargo.toml`, `apps/zeron/src/main.rs`, `apps/zeron/src/lib.rs`.
- `crates/mcp-proxy/Cargo.toml`, `README.md`, `VALIDATION.md`.
- `crates/mcp-proxy/src/{lib,main,config,rpc,cache,process,proxy,npx}.rs`.
- `crates/mcp-proxy/tests/e2e.rs`, `tests/measure_pss.py`,
  `tests/support/fake_mcp.rs`, `tests/fixtures/{README.md,tools.json}`.
- `scripts/package-linux.sh`, `scripts/package-macos.sh`.

## Review pass, 2026-10-30

Adversarial review of the proxy, cache, lifecycle, cleanup and npx resolver
against the fake server, the installed libnpmexec source and a real npm 12
`_npx` cache. Each fix is its own commit with a test that failed before it.

Fixed:

- A stray non-JSON line on the server's stdout killed the server (clients skip
  it); invalid UTF-8 ended the stream.
- Bounded relay queues and `try_send` ended the proxy when a peer was slow
  (notification floods).
- A server that negotiates an older protocol never hit the cache.
- A declared-but-unimplemented `resources/templates/list` (-32601) failed
  discovery and pinned the key to passthrough.
- A cursor-bearing list request was answered from the merged cache, duplicating
  entries for a client resuming a live server's page.
- After a miss, a first call queued behind discovery caused a stop and a second
  start; the first call also waited behind the refresh's listing requests.
- npx resolution: install selection did not match npm (see the commit); a
  second `--package` or a dependency's bin ran a different executable; project
  and global installs that npm prefers were ignored; floating specs were never
  refreshed.
- Failure text returned to the model and logged could echo credentials.
- The discovery-failed marker never expired; script servers' cache keys ignored
  the script; log growth; 0755 directories; SIGKILL of a group number that may
  have been reused (and unverified group numbers in macOS sweeps).
- `zeron mcp-proxy` failed outright when the packaged helper could not be
  executed instead of running in-process.

Overhead measured (release build, fake server, sequential `tools/call`, warm
proxy, 2,000 calls): +36 µs median per small call. It grows with payload because
messages are parsed and re-serialized: +0.3 ms at 100 KB, +5 ms at 2 MB.

Not changed, by design or out of scope here: server-to-client requests during
a cache-miss discovery are forwarded before the client's `initialize` reply
(holding them could deadlock servers that need the answer to list tools);
floating npx specs are up to 24 h behind npx; chrome's own process group (not
the telemetry watchdog) is not tracked; Windows and macOS runtime behavior and
Windows compilation (no windows std in this toolchain) were not exercised.
