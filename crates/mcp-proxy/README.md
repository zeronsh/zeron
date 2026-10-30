# Lazy stdio MCP proxy

```
zeron mcp-proxy --server NAME --mode lazy|lazy-keep-alive|keep-alive
  [--idle-min 10] [--identity-env K1,K2] [--plugin-root PATH] [--cwd PATH]
  [--run-dir DATA/run/mcp/RUN] [--no-npx-resolve] -- COMMAND ARGS...
zeron mcp-proxy --selftest
```

`--selftest` is an in-process one-tool server for CLI capability probes.
The standalone `zeron-mcp-proxy` accepts identical arguments without the
`mcp-proxy` word. Both use a current-thread Tokio runtime; process creation
happens on the main thread to keep Linux PDEATHSIG reliable.

Unix packages ship the small executable beside `zeron`, which execs it before
initializing application logging. Linux updates unpack the whole versioned
archive; macOS updates replace the whole app bundle, so both carry the helper.
Source builds without the sibling, and Windows (whose updater ships a single
exe), use the same core inside `zeron`. Windows uses a suspended child assigned
to a kill-on-close Job before resuming it. Windows and macOS runtime behavior
still needs native validation; only Linux has been executed here.

`ZERON_DATA_DIR` selects storage (the normal app data directory otherwise):

- `mcp-cache/v1/<sha256>.json`: original initialization and merged catalogs;
  private 0600 files replaced atomically. `<sha256>.lock` serializes each key's
  freshness check and replacement. Identity env values are hashed, never saved.
- `run/mcp/<run>/<pgid>` when `--run-dir` is supplied: private ownership markers.
  Linux markers include the leader's start ticks to reject PID reuse.
  `cleanup_run_dir()` ignores nonnumeric run files and kills recorded groups.
- `logs/mcp-proxy.jsonl` when `--run-dir` is supplied: append-only bounded JSON
  lifecycle records, also emitted to stderr. Child stderr is copied verbatim;
  failure reporting retains at most 8 KiB and exposes at most 300 characters.

Lazy cache misses start a discovery child and stop it after capture unless a
call/subscription/progress/request holds it. Cache hits initialize without a
child. Every real connection refreshes metadata; list changes invalidate the
persistent catalog and refresh it in the background. Cached metadata remains
available to this client during startup. Cache entries expire at the declared
TTL or 30 days, whichever is sooner. Private and zero-TTL results remain only
in this proxy's memory. Failed discovery marks the key for transparent
passthrough in the next proxy.

Lazy idle timeout is ten minutes, checked every thirty seconds; a zero timeout
disables idle stopping. Lazy-keep-alive never idle-stops. Keep-alive starts the
child before client initialization and transparently forwards the protocol.
All modes clean up on EOF, SIGTERM, SIGHUP, and SIGINT. Stops close stdin, TERM
owned groups, wait up to two seconds, then KILL survivors. Linux also adopts
and reaps descendants. A chrome-devtools telemetry watchdog is identified by
its `telemetry/watchdog/main.js --parent-pid=PID` argv and the exact parent PID
in an owned group; its separate group is recorded and stopped as well.

Test-only millisecond overrides: `ZERON_MCP_PROXY_IDLE_MS`,
`ZERON_MCP_PROXY_CHECK_MS`, `ZERON_MCP_PROXY_BACKOFF_MS`, and
`ZERON_MCP_PROXY_START_TIMEOUT_MS`. Production startup/backoff defaults are
60 seconds. Queued cancellation produces no response; in-flight cancellation
rewrites the target ID. Sampling/elicitation/roots requests retain server IDs.

Run `cargo test -p zeron-mcp-proxy` in the performance worker environment.
The fake server and PSS helper use only target-local scratch storage.

Initial Linux debug measurement (2026-10-30, 7 samples/state, proxy PID only):
2,120 KiB PSS cached idle and 2,400 KiB while forwarding an outstanding fake
call. Reproduce with:

```
python3 crates/mcp-proxy/tests/measure_pss.py \
  "$CARGO_TARGET_DIR/debug/zeron-mcp-proxy" "$CARGO_TARGET_DIR/debug/fake-mcp"
```

The full GUI-linked `zeron` fallback has not been measured; the reported budget
is for the packaged small proxy. Real browser servers and CLI harness rewrites
belong to the subsequent work units, as do engine run-dir sweeps and daily npm
cache refresh.
