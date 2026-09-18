# Model discovery reliability

Initial validation on Linux, 2026-09-17, after merging origin/main at `15f6ec31`.

The Cursor fallback/cache policy described here is superseded by the
[2026-09-18 stability follow-up](cursor-stability/README.md), which includes
rate-limit handling, credential-scoped last-good catalogs, session recovery,
and substantially expanded live and fault-injection evidence.

## Cursor failure and repair

The production shim called `process.exit()` immediately after writing its
catalog. Node does not guarantee that asynchronous pipe writes have drained
when exiting. The live failure returned exit status 0 and exactly 65,536 bytes
of truncated JSON. The Rust harness consequently returned only Auto and
Composer 2.5.

Every shim exit now waits for the stdout write callback, including model,
login, fatal-error, and session-close paths. SDK background handles can still
be terminated promptly after the output drains. The Rust probe also rejects
unsuccessful subprocess exits instead of accepting an accompanying catalog.

The pinned `@cursor/sdk@1.0.28` already resolves credentials in this order:
explicit key, `CURSOR_API_KEY`, saved SDK login (with expiry/backend checks).
The earlier "public, no auth" code comment was incorrect. No separate auth
file parser or credential injection is needed. Three live probes with
`CURSOR_API_KEY` removed returned complete 73,027-byte catalogs using the
saved SDK login: 40 entries, mapped to 39 picker rows after removing the
`default` alias. Parameters and default variants remain intact.

## Other harnesses

| Harness | Discovery and coverage |
| --- | --- |
| Cursor | SDK JSONL; drain before exit; refresh on subsequent calls; overlapping successful requests share a result. |
| ACP (Grok, Hermes, Pi, Antigravity) | Read complete JSON-RPC responses before shutdown; replace permanent model caching with refresh and overlapping-request sharing. |
| Devin | Already refreshes its native JSON catalog command and shares overlapping requests. Tests cover refresh, errors, and timeouts. |
| OpenCode | Read the complete HTTP provider catalog; replace permanent model caching with refresh and overlapping-request sharing. Commands can still share the discovery server. |
| Codex | Already reads complete JSON-RPC responses and all model-list pages on each discovery. Pagination tests pass. |
| Claude | Static model catalog; no model-discovery subprocess output or account catalog cache. |

Failed/empty discovery never permanently caches a fallback. Cursor and ACP
keep their existing per-request fallback behavior; OpenCode and Devin surface
errors so callers can retry. Refreshing may cost another subprocess startup
on a later picker open; overlapping successful probes remain coalesced.

## Validation

- `cursor-agent update`: upgraded from `2026.08.11-e8db854` to
  `2026.09.15-d2fe57e`; `cursor-agent --list-models` succeeds.
- `cargo run -p zeron-harness --example cursor_models_probe`: 39 picker models
  through the real managed SDK/shim path, previously only 2 fallback rows.
- `cargo run -p zeron-harness --example opencode_models_probe`: 8 live models.
- Local Codex app-server `model/list`: 6 models; automated pagination coverage
  also passes.
- `cargo test -p zeron-harness`: 282 passed, 0 failed, 10 opt-in tests ignored.
- `git diff --check`: clean.

New tests execute the production JavaScript shim with a synthetic SDK and
require Node. They exercise a catalog over 4 MiB, a 1 MiB fatal frame, and
large login output. Subprocess/HTTP fixtures additionally cover large model
responses, account changes, concurrent probes, malformed output, unsuccessful
exit status, provider disconnects, and recovery. Live checks were not performed
for every authenticated ACP provider; those paths have fixture coverage.
