# Cursor model and conversation stability

Validation: Linux, Node v22.22.1, pinned `@cursor/sdk@1.0.28`, Cursor CLI
`2026.09.15-d2fe57e` (the updater confirmed it was current on 2026-09-18).
Machine-readable evidence: [results.json](results.json).

## Problems reproduced

1. A failed model refresh still returned Auto + Composer 2.5 as a successful
   catalog. This overwrote a previously valid picker list. Cursor's live model
   endpoint returned an explicit **30 requests/minute** rate limit during the
   initial stress run. Refreshing on every picker open can trigger that limit.
2. Different Zeron builds shared a mutable shim filename inside the SDK install.
   The local shared file had reverted to the older immediate-exit implementation.
   Its first 30 stress responses were truncated JSON; the next 20 returned rate
   limits. This observation is consistent with an old engine/build still being
   involved, and does not establish which remote version produced the user's UI.
3. A dead SDK process can leave the agent `running` with an `activeRunId` in its
   durable store. Every subsequent send then fails with “already has active run.”
   The new fault test fails against the previous production shim immediately
   after the first killed run. The repaired shim passes the same test 100 times.
4. A follow-up turn's crash could be hidden by the first turn's completed flag.
   Cancellation racing a fatal response could also produce two terminal events.

## Changes

- Model catalogs are cached for 60 seconds and scoped to a hash of credential
  contents, expiry state, and Cursor environment settings. An account change
  invalidates the cache, including when credentials change during discovery.
- Failed refreshes retain the same account's last successful catalog. A cold
  failure is an error, never a fabricated two-row success. Transient probes get
  at most three attempts under a 20-second total deadline. Rate limits/auth
  failures do not receive immediate retries; cooldowns coalesce failed callers.
- The shim transmits only catalog fields used by the picker, including the
  default variant, and still drains stdout before exiting.
- Shim filenames include their source digest and are atomically published.
  Concurrent/older Zeron builds cannot overwrite the chosen implementation.
- An OS lease serializes use of each Zeron-managed conversation store. A live
  owner's PID marker prevents recovery from modifying an active process's store.
  Shutdown cancels/closes the SDK on EOF and signals; a parent-death watchdog
  also handles engine crashes when another process keeps the stdin pipe open.
- Before resuming an exclusively owned abandoned store, recovery marks the old
  run cancelled, clears `activeRunId`, and retains its newest conversation
  checkpoint and SDK metadata. It does not create a replacement conversation,
  delete history, or resend the interrupted prompt.
- Failed turns stop the shim session. Follow-up turns reset their terminal
  bookkeeping, and cancellation/fatal races emit one terminal event.

## Evidence

| Check | Result |
| --- | --- |
| Full harness regression suite | 297 passed, 0 failed, 10 opt-in tests ignored; includes other harnesses |
| Engine restart/publication regressions | 11 passed, 0 failed, 3 opt-in tests ignored |
| Live catalog burst | 10,001 requests; all 39 rows preserved; zero failures |
| Live catalog + injected rate-limit outage | Warm real catalog, let its TTL expire, inject outage, issue 10,000 requests, then recover against real service; all 39 rows preserved; only 3 measured shim starts |
| Deterministic cache stress | 10,000 overlapping requests through 100 outages; 200 probes total; zero catalog collapses |
| Cold outage burst | 1,000 callers share one failure; no fabricated catalog; later recovery succeeds |
| Real subprocess catalog faults | 41 probes; 256-model payload over 1 MiB; 10 truncated/empty/nonzero-exit refreshes retain the complete catalog |
| Concurrent shim publication | 800 publications across 8 simultaneous source versions, with old-filename rewrites; zero corrupt/replaced selected shims |
| Production shim fault stress | 100 kill/EOF/cancel/send-error/wait-error/auth-error/hung-cancellation cases; 100 same-session recoveries; retained history/metadata; exactly 201 requested sends, no recovery replays |
| Real harness resume soak | 20 recall rounds in one conversation, including 4 kills, 3 cancellations, 3 dropped streams; zero lost session IDs or checkpoint tokens |
| Real harness parked session | 20 turns in one live shim, then close and resume; all token recalls pass |
| Real tool-side-effect recovery | Kill/cancel/drop only after a shell append executed; recover and recall the token; exactly 3 appends, no duplicated side effects |
| Native Cursor CLI | Initial turn plus 12 resumed follow-ups in a real CLI session; all token recalls pass |
| Lifecycle regressions | Live-owner exclusion, engine death with a leaked stdin writer, follow-up crash finality, cancellation/fatal races, bounded model timeout, cancelled cache refresh, login changes |

The native CLI and SDK have separate session stores. The native CLI check
validates the updated CLI and its real session behavior; the harness tests
exercise Zeron's actual SDK driver, not a substitution with CLI print mode.
All live prompts use disposable workspaces and synthetic tokens. The fault
prompts deliberately write only a disposable side-effect counter and sleep.

## Steering burst follow-up

Rapid steering is tested with a bounded channel of eight entries, deliberately
forcing backpressure. Ten deterministic bursts of 200 prompts check exact order,
unique assistant message IDs, one completion per turn, and clean closure when
the sender disconnects. Twenty cancellation bursts of 100 prompts verify one
interrupted completion and no queued turn promotion. Cancellation now takes
priority over simultaneously ready output/steering events.

Against the real SDK, 24 immediately queued prompts completed in order with
context retained, followed by successful same-session resume. A separate burst
of 100 queued tool prompts was cancelled during a running tool: none were
promoted, no queued file write occurred, and the established conversation
checkpoint was recalled after resume.

An exploratory cancellation of the very first turn exposed a provider limit:
that unfinished turn had no resumable context checkpoint. The resumed session
was usable but could not recall its interrupted first prompt. The established-
conversation test therefore seeds a completed checkpoint first. This is not
evidence that every partial, uncheckpointed turn survives interruption.

## Reproduce

The automated suite requires Node; subprocess catalog fixtures also use a POSIX
shell. It does not need Cursor credentials or consume provider quota:

```sh
cargo test -p zeron-harness -- --nocapture
```

The following live probes require an authenticated SDK and consume Cursor quota:

```sh
cargo run -p zeron-harness --example cursor_stability_probe -- models 10000
ZERON_CURSOR_STATE_DIR=$(mktemp -d) cargo run -p zeron-harness --example cursor_stability_probe -- sessions 20
ZERON_CURSOR_STATE_DIR=$(mktemp -d) cargo run -p zeron-harness --example cursor_stability_probe -- parked 20
```

Rapid live steering and cancellation (each consumes provider quota):

```sh
ZERON_CURSOR_STATE_DIR=$(mktemp -d) cargo run -p zeron-harness --example cursor_stability_probe -- burst 24
ZERON_CURSOR_STATE_DIR=$(mktemp -d) cargo run -p zeron-harness --example cursor_stability_probe -- cancel-burst 100
```

To measure the live-catalog outage test (about two minutes, including real TTLs):

```sh
cursor_stress_dir=$(mktemp -d)
cargo run -p zeron-harness --example cursor_stability_probe -- resolve > "$cursor_stress_dir/launch.json"
CURSOR_SDK_SHIM_EXECUTABLE="$PWD/crates/harness/tests/fixtures/cursor-stress-proxy.py" \
ZERON_CURSOR_STRESS_LAUNCH="$cursor_stress_dir/launch.json" \
ZERON_CURSOR_STRESS_COUNTER="$cursor_stress_dir/probes.log" \
ZERON_CURSOR_STRESS_OUTAGE_FLAG="$cursor_stress_dir/outage" \
cargo run -p zeron-harness --example cursor_stability_probe -- outage 10000
wc -l "$cursor_stress_dir/probes.log"  # 3: live warm-up, injected failure, live recovery
```

To demonstrate the old persistent active-run failure with the same fixture:

```sh
git show 4368e926:crates/harness/src/cursor/shim.mjs > /tmp/cursor-before-stability.mjs
ZERON_CURSOR_TEST_SHIM=/tmp/cursor-before-stability.mjs \
cargo test -p zeron-harness --test cursor_shim stress_100_interrupted_sessions -- --nocapture
# Expected failure on the first recovery. Without the override, all 100 pass.
```

## Scope and limits

The engine that owns the conversation must contain this fix. Updating a local
viewer alone cannot repair an older remote engine. Last-good catalogs survive
refresh failures in that engine process; after an engine restart without network
access, discovery reports an error until a live catalog can be obtained.

Recovery applies to Zeron-managed stores, including existing per-agent stores
with stale active runs. It does not rewrite unrelated native CLI or legacy
SDK-default stores. A genuinely live owner is not cancelled by a competing
resume. Missing/unreadable storage is reported instead of silently replacing a
conversation. A recovered interrupted turn is not represented as completed.

These are finite Linux live tests plus deterministic crash/error injection, not
a six-hour provider soak or a guarantee against future Cursor service failures.
The provider's original `ERROR_NOT_LOGGED_IN` incident was not reproduced live;
that error with a stale active run is covered by the fault fixture. The verified
invariant is that the tested failures do not permanently wedge the managed
conversation or replace a good catalog with synthetic defaults.
