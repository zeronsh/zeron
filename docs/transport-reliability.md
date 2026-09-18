# Transport reliability: measured Cloudflare results

## What changed

High round-trip latency exposed unnecessary retransmission: every new edit sent
all previously unacknowledged chat batches again. The actor now tracks batches
in flight **per WebSocket session**. New edits send only new batches; reconnects
still replay the durable SQLite outbox with the same deduplication IDs. Explicit
quota retries retain their existing head-of-queue policy.

The shared Rust socket pump drives reads and writes independently. Its watchdogs
measure **byte inactivity**, including incomplete incoming WebSocket frames,
instead of imposing an absolute duration on an otherwise progressing transfer.
A full consumer queue remains bounded. Closing the consumer cancels both halves.

Messages larger than 16 KiB use 4 KiB WebSocket fragments with empty control pings
between fragments. Cloudflare answers those pings before the application message
is complete. This matters when the whole message fits in the local TCP buffer:
local writes can finish immediately while the remote uplink takes over a minute.
The application still receives one ordered message, with identical bytes. Small
messages and idle heartbeat cadence are unchanged. Large-message fragmentation
adds about 0.4% framing/control traffic; it is not a compression feature.

The document heartbeat remains 15 seconds with a 45-second inactivity lease;
relay transport retains its 10-second heartbeat and 25-second lease. The dialer
bounds DNS, TCP racing, TLS and upgrade together to 20 seconds. An interrupted
send closes the session because resuming a partial frame is unsafe. Documents
recover with their batch IDs and cursors; uncertain RPC mutations are not replayed.

## Live comparison against main

On 2026-09-15, the production Rust `ChatClient`, Loro document, SQLite outbox and
actual Cloudflare Worker/SQLite Durable Object handled synthetic test rooms.
The worker used unchanged edge source from **main `67c960f4`**, isolated bindings,
dev authentication and no production routes. Worker version:
`c2c7587e-b5f7-49ae-aefe-d9b54fc95039`.

A local proxy established verified TLS to Cloudflare and limited aggregate traffic
across both clients to **2,048 bytes/second per direction**, with **600 ms added
one-way delay**. Each run queued 20 edits at 100 ms intervals: 4,900 application
text bytes / 6,756 Loro update bytes. Every run used a fresh room and proxy.
Main and fixed versions ran alternately, three times each.

| Metric | Main, median (range) | Fixed, median (range) |
|---|---:|---:|
| Successful push attempts for 20 unique batches | 30 (30–61) | 20 (20–20) |
| Outbound HTTP/WebSocket bytes | 12,648 (12,648–24,338) | 8,912 (8,912–8,912) |
| Inbound HTTP/WebSocket bytes | 16,373 (15,542–16,397) | 11,922 (11,920–13,526) |
| Completion including final stats verification | 9.84 s (9.29–13.87) | 7.28 s (7.25–7.62) |
| Delivered rows / pending batches | 20 / 0, every run | 20 / 0, every run |
| Sender and receiver disconnects | 0, every run | 0, every run |

Median outbound bytes fell **29.5%**, inbound bytes **27.2%**, and completion time
**26.0%**. An earlier single comparison showed larger gains; the table deliberately
uses the repeated experiment. Three runs establish this workload's behavior,
not a fleet-wide performance distribution. These are decrypted HTTP/WebSocket
counts, **not physical network bytes**; TLS/TCP/IP overhead is excluded.

## Additional live fault and slow-link results

| Profile | Actual result |
|---|---|
| 1 KiB/s each direction, 1.2 s added one-way delay | 20/20 rows, 20 pushes, pending=0, no disconnects; completion 19.14 s |
| 30-second blackout, active connections reset, then 2 KiB/s + 600 ms restored | 20/20 rows, pending=0, 20 pushes; completion 41.19 s including outage |
| Every WebSocket upgrade rejected; first successful HTTP POST response discarded | 20/20 unique rows, 21 pushes, pending=0; completion 48.66 s; 18 upgrades blocked, 1 ACK lost |
| 128 KiB fresh-reader catchup, 2 KiB/s + 600 ms, WebSocket only | Main fails with `chat2 handshake failed`; fixed completes in 70.22 s, cursor=1, zero disconnects |
| 128 KiB upload and delivery to a second client, 2 KiB/s + 600 ms, WebSocket only | Exact document equality, pending=0, both clients stay connected; 139.80 s including setup |

The catchup update is 131,167 bytes, seeded directly before starting the impaired
reader. The upload sends that update through the impaired uplink. Neither large
transfer uses HTTP fallback. The HTTP fault is injected **after Cloudflare returns
success**, proving accepted-but-unacknowledged replay against actual server dedupe.
The live test uses persistent SQLite storage but does not restart the process;
a separate regression reopens the SQLite outbox before replay.

[Raw run outputs and metadata](transport-results/2026-09-15/) are committed beside
this report. The proxy forces HTTP connection close, so its connection counter
is not a production connection-pooling benchmark. Arrival-percentile recording
was refined during testing; p95 fields in raw output are not used for comparisons.

## Regression evidence

**85 Rust tests pass; 3 pre-existing live-edge tests remain ignored.** The live
matrix above is a separate executable. Clippy completes with existing warnings
in document/sync code. The isolated worker configuration also passes Wrangler's
deployment dry run.

- A locally buffered 132,000-byte UTF-8 message traverses a remote 2 KiB/s
  bottleneck; control pongs arrive before message completion, and reassembly
  preserves characters crossing fragment boundaries.
- A 128 KiB frame downloading at 2 KiB/s fails the original pump at 45 seconds;
  the new pump completes at 65 seconds under the same virtual byte transport.
- Healthy idle traffic over 601 virtual seconds remains **400 outbound / 240
  inbound WebSocket bytes**, one connection, in both versions. No extra idle
  traffic was introduced.
- Twenty edits with every acknowledgment withheld produce exactly 20 pushes.
- Blocked writes do not suppress inbound ACKs or cancellation. Inbound queue
  stalls close the session. Incoming pongs cannot keep a blocked writer alive;
  locally buffered unanswered echo/ping traffic cannot keep a dead peer alive.
- An accepted TCP socket that never answers its upgrade times out within 20 s.
- Existing tests cover lost-ACK replay, cursor repair, quota retry, persistence,
  relay supersede, credential rotation and zombie-host recovery.

The original negative controls are reproducible at `94cc6753`. Comparative byte
and idle tests also carry the original pump as a control. The initial PR's
absolute write timeout was insufficient: the slow-transfer tests exposed that
regression, and the final implementation uses byte progress and fragmentation.

## Reproduce

```sh
cargo test -p zeron-sync -p zeron-rpc --features zeron-sync/mock-server
cargo test -p zeron-sync --lib socket:: -- --nocapture
cargo clippy -p zeron-sync -p zeron-rpc --all-targets --features zeron-sync/mock-server
cargo build -p zeron-sync --example transport_live

# Requires authenticated Wrangler and an account with Workers/R2 enabled.
cd edge
npm ci
node_modules/.bin/wrangler r2 bucket create zeron-transport-385-20260915
node_modules/.bin/wrangler deploy --config wrangler.transport-test.jsonc
cd ..

# Use the isolated workers.dev URL printed by deployment.
python3 scripts/run-transport-matrix.py \
  --binary target/debug/examples/transport_live \
  --origin https://zeron-transport-385-20260915.YOUR-SUBDOMAIN.workers.dev \
  --profile stream --output /tmp/transport-stream.json
# Repeat with: very-slow, outage, http, catchup, upload.
```

For the baseline, create a detached worktree at `67c960f4`, copy only
`crates/sync/examples/transport_live.rs` into its examples directory, and build
that same example there. Run the same proxy and streaming/catchup profiles with
its binary. The matrix driver owns and stops each proxy. Large transfers allow
up to 360 seconds for completion; the driver has a 420-second process limit.

After testing, delete only the isolated resources:

```sh
cd edge
node_modules/.bin/wrangler delete --config wrangler.transport-test.jsonc
node_modules/.bin/wrangler r2 bucket delete zeron-transport-385-20260915
```

## Scope and remaining limits

These measurements demonstrate lighter, successful document synchronization on
the stated impaired links. They do not prove a globally most reliable transport
or that every feature works on every aircraft network.

- Generic remote RPC/uploads still require WebSocket; preview uses WebRTC with
  STUN and lacks an HTTPS byte relay. Document HTTP fallback already exists.
- Relay end-to-end echo and room-probe deadlines are separate from transport
  liveness. Large RPC transfers and congested host legs need their own live
  measurements; document results cannot establish those guarantees.
- Swift/iOS lifecycle, sleep/wake, IP changes, authentication expiry, captive
  portals and actual packet loss/retransmission were not exercised here.
- Checkpoint Range resume still needs a strong content validator when content
  changes at the same sequence. HTTP retry ownership and stalled RPC consumer
  isolation remain separate issues.
- A proxy shaping decrypted bytes is not a physical airline network. Live
  requests here use dev auth and a test HTTP adapter with a 180-second timeout;
  production authentication and engine HTTP timeouts need separate coverage.
