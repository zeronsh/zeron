`tools.json` is the fake server's default catalog. `tests/support/fake_mcp.rs`
is registered as the `fake-mcp` test binary by the proxy crate. Controls:

- `FAKE_TOOLS`, `FAKE_CHANGED_TOOLS`: JSON tool arrays.
- `FAKE_SPAWN_MARK`, `FAKE_CHILD_MARK`: append process IDs.
- `FAKE_INIT_DELAY_MS`, `FAKE_CALL_DELAY_MS`, `FAKE_INIT_ERROR`.
- `FAKE_CRASH_ON`, `FAKE_SAMPLING_ON`, `FAKE_LIST_CHANGED_AFTER`: tool names.
- `FAKE_PROGRESS`: number of progress notifications (original token preserved).
- `FAKE_DUMP_ENV`, `FAKE_REQUEST_LOG`: environment snapshot and inbound JSONL.
- `FAKE_GRANDCHILD=1`, `FAKE_WATCHDOG=1`: sleeping descendant and separate-group
  telemetry watchdog; both inherit stdio to exercise cleanup of held-open pipes.
- `FAKE_PAGED=1`, `FAKE_TTL_MS`, `FAKE_CACHE_SCOPE`, `FAKE_INSTRUCTIONS`.
- `FAKE_STDERR_BYTES`: diagnostic noise size.

All paths are supplied by the caller; tests use directories under the isolated
Cargo target directory, never system temporary directories or real Zeron data.
