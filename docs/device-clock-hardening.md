# Device clocks and causal time

Zeron must not require synchronized device clocks or matching timezones to
rename a device, converge metadata, deliver a command, or expire a live status.
This audit covers registry replication, engine runtime timing, desktop watches,
the shared mobile client, iOS countdowns, and edge presence/deletion retention.
It does not claim to prove the absence of every timing bug.

## Contracts

- Runtime waits use monotonic elapsed time. `zeron_proto::time` supplies an
  epoch-shaped process clock so existing millisecond deadline APIs can keep
  their wire format without following changes to the OS civil clock. It is
  anchored once per process, handles pre-epoch and extreme initial dates, and
  uses saturating arithmetic. It is not trusted UTC or a cross-device clock.
- Ordering of observed registry writes uses HLCs. All state delivery and restore
  paths observe clocks before producing local writes, including pending ops and
  field-clock overrides. OS timestamps are clamped inside the 13-digit format
  with logical headroom. Logical ordering survives clock regression and restart.
- Freshness needs receipt evidence. A peer's timestamp is neither proof that it
  is online nor a receiver-side deadline. Engine and UI are different clock
  domains, even on the same machine after a clock correction.
- Timezones affect date formatting, not causal ordering, TTLs, or liveness.

## Findings and handling

| Path | Failure under clock skew | Handling |
| --- | --- | --- |
| Device rename and other registry mutations | Linux's write loses to an already observed Windows row whose clock is hours ahead | Observe remote HLC time and counter on full/delta state, broadcasts, migration enqueue, and restore; next write is causally newer |
| Legacy pending clocks | Extreme dates on older builds persisted invalid-width HLCs that the edge rejects indefinitely | Restore re-stamps malformed pending clocks after observing valid clocks, preserving their authored intent |
| Device boot metadata | A full-row boot write re-stamps a stale device name and can overwrite an unseen remote rename | Upserts retain clocks for unchanged fields; explicit edits still represent new intent |
| Chat activity, creation, tabs, spaces, search ties | A future-dated old row remains above later events; a backward step changes ordering | Optional registry activity/creation clocks drive ordering; deterministic legacy timestamp fallback remains |
| Unread state | A slow reader cannot clear unread; a fast reader hides later activity | Store the activity clock the reader actually observed; later activity remains unread regardless of either date |
| Registry and chat presence | Sender clocks create false online/offline states | Edge stamps receipt; client stores local receipt and monotonic expiry; snapshots carry a server time anchor |
| Presence replay and dial gate | Replayed snapshots refresh old sightings; boot dates stand in for liveness | Repeated snapshot sightings do not renew leases; dark-peer decisions use local silence with a join warmup |
| Live session indicators | Fast hosts show eternal Working; slow hosts appear immediately stale | Project changed rows into receipt time at engine/mobile/UI boundaries, expire unchanged rows after 45 seconds, and preserve local run-start projection |
| Engine-to-UI presence | Engine and UI started on opposite sides of a civil clock correction disagree about online state | UI rebases changed engine sightings into its own process clock without renewing identical rows |
| Command TTL and attachment waits | Sender dates expire fresh commands or keep old commands eligible indefinitely | Bound sender TTL duration, durably stamp first host receipt, dedupe replay without renewing, and expire an observed lease after host generation change |
| Interrupt supersession and retry attempts | Incorrect issuedAt selects the wrong attempt | Use append/document order |
| Mobile pending send presentation | Cached sender dates distort delivery grace and local expiry | Start delivery grace on local observation; host outcomes decide durable command expiry |
| Queue editing | Clock steps or old persisted epochs hold editing locks forever | Current-process leases follow the runtime clock; reopening converts persisted Editing to ReviewRequired, preserving the draft |
| Upload staging GC | Wrong filesystem mtimes delete active uploads or keep abandoned uploads forever | Track local monotonic activity; unknown directories get one local grace period after restart |
| Runtime idle, heartbeat, duration, retry and UI timing | Wall-clock steps disable timeouts or create absurd countdowns | Shared monotonic process clock; iOS calls the same Rust clock; pull-to-refresh uses uptime |
| Mobile authentication | A slow clock repeatedly reuses an expired cached JWT | Refresh cached tokens on startup; fresh responses receive a bounded elapsed-time lease, single-flight rotation remains intact |
| HTTP Retry-After dates | Wrong local UTC erases or stretches provider backoff | Use the response Date header as the date anchor when available; existing bounded fallback remains |
| Edge tombstone GC | A past deletion HLC gets purged immediately, allowing stale resurrection; a future HLC never ages out | Persist server receipt per tombstone, conservatively backfill legacy tombstones, retain 30 days, and keep daily GC alarms scheduled |
| Retry jitter and worktree names | Wall-clock randomness collapses when the OS date is stalled or pre-epoch | Use UUID randomness without civil time |
| Temporary git indexes | Timestamp-derived names collide when clocks stall or regress | Use random UUIDs |

## Compatibility and boundaries

Optional creation/activity fields are additive to the existing RPC entities;
older clients ignore them. `seenActivityHlc` is an ordinary registry field.
Legacy seen markers continue using historical dates until a current reader
marks the chat seen. The presence anchor is additive; when talking to an older
edge without it, a snapshot cannot establish freshness, so live beats/probes
establish it instead. Deploy the edge changes with the client rollout to retain
immediate snapshot-based presence.

Historical timestamps cannot be reconstructed into true UTC when their source
clock was wrong. They remain display metadata. Migration preserves available
source context and room generation, with deterministic historical clocks.
HLCs establish a total order consistent with observed causality, not a provable
physical order of concurrent offline writes. The finite wire clock still has a
representational ceiling; hostile or exhausted wire clocks are not a trusted
clock service.

A command first arriving after a long offline delay has no trustworthy original
age: its bounded lease starts on first host receipt. A previously observed
pending command fails closed after host restart because its elapsed-time lease
cannot be reconstructed. The user can explicitly retry it. Persisted edit
leases require review, rather than treating expiration as permission to send.

Monotonic clocks can exclude system suspend. The existing wake/reconnect event
bus now also handles backward civil-clock steps; server silence leases and
freshness continue to converge after wake. Authentication keeps elapsed-time
and wall-elapsed checks for conservative refresh across suspend. Relative labels
and historical durations are approximate if the original dates were wrong.

Server civil time is assumed trustworthy for edge retention and snapshot age.
TLS certificate validation and third-party authentication policies still depend
on their own trusted time checks. This change does not disable those checks or
promise connectivity with an arbitrarily incorrect OS clock. iOS native build
and device execution need Apple tooling; the Rust core and generated bindings
can be verified on Linux.

## Regression coverage

Tests cover an ahead-of-Linux remote rename across all delivery paths and old
snapshot restore; extreme/pre-epoch HLC input; unchanged boot metadata merging;
causal unread, activity and creation ordering; wrong-clock session/device
receipts and replay; bounded command durations and durable restart receipts;
queue edit recovery; filesystem-mtime-independent staging cleanup; both clock
step directions; cached future-dated JWT refresh; provider-relative backoff;
and real workerd SQLite tombstone retention/alarm scheduling and presence.
Existing desktop state, sync, workspace, queue, attachment and mobile demo
workflow suites exercise the integration paths.
