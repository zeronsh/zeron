# Transport reliability under interrupted connectivity

## Recovery contract

A connection must make progress or release its resources so the protocol owner
can recover. A heartbeat deadline inside a loop does not provide that guarantee
when the loop can suspend indefinitely inside a write or a channel send.

The shared Rust WebSocket pump now polls reads and writes independently. It
bounds each write and inbound-queue delivery, closes on transport silence, and
releases both halves when the session's receiver is dropped. The document
protocols retain their existing 15-second ping / 45-second lease; device relay
sessions retain 10-second ping / 25-second lease. The shared dialer bounds DNS,
TCP address racing, TLS and HTTP upgrade together to 20 seconds; callers can
still enforce a shorter deadline.

A canceled send may already have written part of a frame. The pump therefore
closes the entire socket. Document owners recover through their durable batch
IDs and cursors. RPC owners fail pending calls; a failed call is not proof that
the remote side did not execute it, so generic mutation replay is unsafe.

## Reproduce the failure and fix

The regression commit `94cc6753` extracts the existing document pump without
fixing its blocking behavior and adds negative controls. In a separate worktree
at that commit, run:

```sh
cargo test -p zeron-sync --lib socket::tests -- --nocapture
cargo test -p zeron-sync --lib dial::tests::stalled_upgrade -- --nocapture
```

Expected baseline: five socket tests fail; the slow healthy connection passes;
the unanswered-upgrade test fails. The byte tests run the real WebSocket codec
over a 64-byte duplex pipe, with Tokio's virtual clock. This deterministically
fills the send buffer without depending on OS TCP buffer sizes.

At the fixed revision:

```sh
cargo test -p zeron-sync -p zeron-rpc --features zeron-sync/mock-server -- --nocapture
cargo clippy -p zeron-sync -p zeron-rpc --all-targets --features zeron-sync/mock-server
```

| Fault or control | Required outcome |
|---|---|
| Peer stops accepting an outbound frame | Pump terminates within its lease; inbound processing remains independent |
| Inbound application queue remains full | Session terminates within its queue-delivery lease |
| Consumer is dropped during a blocked write | Socket releases without waiting for the lease |
| Peer continues sending pongs while uplink is blocked | Write deadline still terminates the session |
| Slow healthy connection, mixed text/binary messages | Ordered delivery continues beyond one lease |
| TCP accepted but HTTP upgrade never answered | Dial returns a timeout within 20 seconds |
| Relay upload blocked | Link closes and pending RPC returns `Closed` |
| All WebSocket dials refused; first HTTP acknowledgment lost | Reopened SQLite outbox replays the same batch ID and drains |
| Delayed TCP forwarding, 30-second blackout, connection reset | All 20 queued registry rows converge and pending writes clear |

The TCP impairment test uses loopback listeners only. Each direction delays
every forwarded chunk by 600 ms plus its serialization time at 150,000 bytes/s.
This is a conservative store-and-forward simulation with a 1.2 Mbit/s ceiling,
not an exact 1.2 Mbit/s link or a packet-loss simulator. It pauses both directions
for 30 seconds, resets existing connections, then restores forwarding. Its
runtime is approximately 40 seconds.

The HTTP test injects failures at the `ChatTransport` seam, uses actual SQLite
outbox persistence and the production client actor, and advances virtual time
by 1.2 seconds per request. It does not exercise HTTPS, DNS, a captive portal,
or the production HTTP server. Real-edge tests remain separate.

## Measured results (2026-09-15)

- Rust suite: **79 passed; 3 live-edge tests ignored**.
- TCP impairment: **20/20 rows, pending=0**, six accepted proxy connections;
  convergence **35.188 seconds** after starting the 30-second outage.
- HTTP seam: **2 attempts, 1 unique stored batch, cursor=2, pending=0**;
  convergence **4.96 seconds virtual time**, including the lost acknowledgment.
- The six original negative controls fail at `94cc6753` and pass with the fix.

## Remaining limits and next validation

These results establish bounded recovery for the tested faults, not that every
feature works on any flight's Wi-Fi.

1. **WebSocket blocking:** chat and registry have HTTP pull/push fallback.
   Arbitrary device RPC and attachment transfer still require WebSocket. Add
   operation-specific HTTP transport with stable request IDs and durable
   receipts before retrying uncertain mutations.
2. **Preview connectivity:** preview bytes use WebRTC DataChannel with STUN,
   without an edge byte relay. An HTTPS-compatible relay is needed for networks
   that block peer connectivity.
3. **Mobile parity:** Swift clients have separate lifecycle/timer code. Verify
   stalled sends, stale callbacks, background suspension and path changes on
   iOS. This Linux validation does not cover iOS execution.
4. **Slow transfers:** these are absolute frame and queue deadlines. A frame
   taking longer than its lease can force reconnection even if some bytes are
   moving. Test smaller chunks and progress-aware deadlines at lower bandwidths
   before claiming support below the tested conditions.
5. **Checkpoint identity:** sequence numbers alone do not identify content when
   a checkpoint is replaced at the same covered sequence. Range resume needs a
   strong content validator and replacement/truncation tests.
6. **Retry ownership:** classify authorization failures separately, isolate stalled
   RPC stream consumers, and give HTTP fallback a lifecycle independent of WS
   backoff timing. The shared socket pump does not make these operation-level
   guarantees.
7. **Network matrix:** add real TLS/proxy rejection, packet loss/retransmission,
   DNS/IPv6 failure, address changes and platform sleep/wake tests. No real
   airline or captive-portal session was used here.
