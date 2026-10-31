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
voice call, and no open subagent sinks. Claude Code also requires channel-safe
marketplace-plugin evidence.
The final claim marks the runtime retiring under the steering ledger lock.
Accepted prompts racing retirement retain the existing orphan replay guarantee.
An open subagent sink holds the runtime indefinitely; silence is not completion.

Claude's `ScheduleWakeup` and `CronCreate` timers live in the CLI process and are
not in `background_tasks_changed` (2.1.316 lists shells, monitors, agents and
workflows only), so the Claude driver reads them off the tool calls and holds the
runtime through `TurnControl::background_live`: a wakeup until its clamped delay
plus 15 minutes, a recurring job until `CronDelete`, an empty `CronList` or the
CLI's seven-day expiry, a one-shot job until its pinned fire time plus 15
minutes. Durable jobs from a previous process are not re-detected after a resume.
A parked runtime must also have seen no background task start or end for 30
seconds (`BACKGROUND_SETTLE`): a task that just ended is about to wake the agent
with its result, and the pressure and manual paths have no deferred window.

Channel servers remain eager and otherwise unchanged. Only Claude Code has
channel delivery, so Codex and all other harnesses hibernate under the ordinary
idle/work guards without a channel check. For Claude, H4 verification V-8 found
that channel_enable requires a marketplace-plugin server. The snapshot's
source=plugin identifies those rows, including plugins whose scope is dynamic.
User, project, local and dynamic non-plugin sources (including Zeron's own server
and absolute-path native servers such as reins mcp) cannot hold Claude channels.

A fresh Claude snapshot is required. Only plugin-sourced rows need affirmative
per-run NonChannel evidence from the proxy; missing, failed or Channel records
hold the runtime. Snapshot capability claims alone cannot clear a plugin row.
The engine checks this policy again at selection and final claim, so late plugin
channel discovery holds an already parked runtime. Proxy-wide channel-live
markers do not gate hibernation: they can describe non-plugin advertisements.
Evidence stays bound to the launch configuration; a later capture after a config
edit cannot clear an older runtime's channel protection. No channel leases or
ownership changes are introduced.

Memory pressure is sampled every ten seconds. Linux uses MemAvailable below the
larger of 2 GiB and ten percent of MemTotal (never more than a quarter of
MemTotal, so a small machine's normal state is not pressure), or PSI `some avg10 > 10`. Windows
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
