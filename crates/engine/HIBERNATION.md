# Idle runtime hibernation (WU9a)

A completed persistent harness turn parks its runtime. Hibernation retires only
that parked runtime; it does not change the transcript, provider session ID,
last completed turn, or native turn outcome. The next prompt uses the existing
provider-native resume path. No Claude `start_idle` or prewarm behavior is added.

The idle window is selected once per run: `ZERON_SESSION_IDLE_MS`, then five
minutes for a voice orchestrator, then `mcp.session_idle_minutes` (ten minutes by
default). Zero disables the idle timer. Manual and memory-pressure requests use
the same safety guards independently of that timer.

`HibernateChat {chatId}` returns `{hibernating: boolean}`. `HibernateIdle {}`
returns `{hibernating: number}`. These report accepted requests, with eligibility
checked again when the run consumes its notification. A request for an older
park is discarded after intervening turn or voice activity. Both accept the existing
`targetDeviceId` routing field.

All hibernation paths require a parked, non-retiring runtime with no pending
steers or input resolvers, no approval/active turn, no background work, no live
voice call, no open subagent sinks, and affirmative channel-free launch evidence.
The final claim marks the runtime retiring under the steering ledger lock.
Accepted prompts racing retirement retain the existing orphan replay guarantee.
An open subagent sink holds the runtime indefinitely; silence is not completion.

Channel servers remain eager and otherwise unchanged. Drivers without complete
channel discovery conservatively stay awake. Claude checks its fresh host-scoped
MCP capture, including absolute-path, eager and remote servers; absent/unknown
capabilities hold the runtime unless the shim/proxy supplies per-run non-channel
evidence. That evidence is refreshed before publishing each park, including
cache-hit launches. Evidence stays bound to the launch configuration; a later
capture after a config edit cannot clear an older runtime's channel protection.
Unknown eager/absolute/remote rows still hold the runtime.
Proxy discovery also writes a sticky run-local `channel-live` hold before forwarding a discovered
channel; known-channel and uncertain legacy exec handoffs write the same hold.
No channel leases or ownership changes are introduced.

Memory pressure is sampled every ten seconds. Linux uses MemAvailable below the
larger of 2 GiB and ten percent of MemTotal, or PSI `some avg10 > 10`. Windows
uses GlobalMemoryStatusEx with the same availability threshold. macOS uses the
read-only `kern.memorystatus_vm_pressure_level` sysctl (warning/critical). An
unavailable probe is a no-op; no system tuning or privileges are required. Two
consecutive pressure samples select one eligible runtime, longest idle first,
with a two-minute minimum. Each subsequent pressured tick selects at most one.

The engine sweeps numeric proxy pgid markers at boot before the first dispatch,
and after harness teardown at run end. It uses the proxy core's identity-checked
TERM/KILL cleanup. Early returns also schedule a fallback sweep. The harness
execution lease is retained through cleanup; the engine waits up to ten seconds
for adapter teardown before the final sweep. On Windows, normal harness/proxy
ownership uses Job Objects; Unix pgid signaling is not available there.

Session rows publish Live/Parked while a handle exists and Hibernated when a
resumable provider session remains after its handle ends. No UI change is in this
unit. `Starting` remains reserved for later prewarm work.

Tracing records `session_hibernated` with harness, reason (idle/pressure/manual),
idle_s, and Linux pss_freed_mib. PSS is the attributed pre-cancel estimate, summed
once per process across groups carrying the engine's per-run env stamp and the
run's MCP group markers; unavailable PSS is omitted. `session_woken` records the
resumed launch's time to first event and `prewarmed=false`. The engine incrementally
reads complete records from logs/mcp-proxy.jsonl into `mcp_proxy` tracing events
with event, server_class and startup_ms. The intake is bounded per tick.

Linux is exercised locally. The macOS and Windows sensor implementations require
platform validation. Live authenticated vendor CLI wake/capability behavior is
not covered by the offline fake harness tests.
