# Live updates

Status: implemented on Linux and macOS (macOS not run on hardware — see
[Platforms](#platforms)); Windows keeps a silent install-on-quit. Sections below
are the design; [Implementation notes](#implementation-notes) records where the
built version deliberately differs.

## Goal

A new release installs itself without asking, and nothing that is running is
disturbed: agent turns in flight, parked questions, terminals, dev servers.
Where a component cannot be replaced in place, it is replaced part by part,
never by killing work.

Non-goals: Windows engine handoff (see [Platforms](#platforms)); rolling back
across more than one version; updating a harness CLI (that is
`harness_updates.rs`, unchanged).

## Where we start

- The daemon (`zeron headless`) already stages an update and restarts itself,
  but only when `sessions.any_active()` and `terminals.any_open()` are both
  false. A long turn or an open terminal defers the update indefinitely.
- The headed app embeds the engine in-process. "Restart to update" checks only
  for unsaved files, then quits; quit runs `sessions.shutdown()` and
  `terminals.shutdown()`, which interrupt every run and kill every PTY.
- Agent children are owned by the engine process (`kill_on_drop`, piped stdio).
  Protocol state (JSON-RPC ids, parked questions, turn routers, normalizer
  state) lives only in engine memory.
- `RemoteEngine` never redials: a dropped socket leaves the UI "Ready" but
  dead. There is no protocol version between UI and engine, only capability
  flags.

## Architecture

Three independent layers, shipped in this order. Each one is useful alone.

```
UI process  ──ws──▶  engine host process  ──stdio/pty──▶  agents, shells
 (replaced by         (replaced by exec-in-place;
  relaunch)            same PID, same children)
```

### 1. Engine host: exec-in-place handoff

The engine replaces itself with `execve` of the newly installed binary. The PID
does not change, so:

- agents and shells remain **children of the same PID**: `waitpid` and exit
  codes keep working, no pidfd, no fd passing over sockets, no subreaper;
- systemd's main PID and launchd's job are unchanged, so no `KillMode` or
  `AbandonProcessGroup` change is needed;
- Cursor's shim, which exits when its parent pid changes, is unaffected;
- there is never a window with two engines, so no double writer on SQLite and
  no double use of the single-use WorkOS refresh token.

Only file descriptors survive exec. The handoff keeps exactly these, by
clearing `FD_CLOEXEC`: the IPC listener (clients queue in the backlog during
the gap instead of getting `ECONNREFUSED`), the `engine.lock` fd (flock is per
open file description, so the lock is never released), every PTY master, and
every agent stdio pipe. Everything else closes.

The old engine writes a **handoff manifest** to an unlinked anonymous file and
passes its fd in `ZERON_HANDOFF_FD`. The manifest is versioned, additive JSON:

```
{ version, from_exe, listener_fd, lock_fd,
  terminals: [{ id, cwd, shell, pid, master_fd, seq, replay[], exited, script }],
  runs:      [{ chat_id, harness, request, pid, fds{stdin,stdout,stderr},
                stdout_leftover, stderr_tail, engine_state, harness_state }] }
```

It holds secrets (the OpenCode server password, agent session state), so it is
never a named file.

#### Sequence

1. **Trigger.** The updater has staged and verified the new binary, and the
   coordinator finds a safe moment (below). `ZERON_AUTO_UPDATE` defaults on for
   engines that can hand themselves over in place (explicit `1|true|yes` still
   opts any engine in, `0|false|no` keeps report-only); no prompt.
2. **Preflight (old image).** Run `<new> handoff-preflight` (does the binary
   run and read our manifest version). It deliberately does not open the new
   binary's store: a newer build could migrate the schema under the running
   engine. Any failure aborts before anything is touched.
3. **Veto check.** Refuse (and retry later, never kill) while any of these is
   true: an agent-account login is in flight; a harness update holds a lease;
   a token refresh is in flight; a run is interrupting, in its setup phase, or
   in a non-freezable state (see run handoff).
4. **Quiesce.** Stop the token refresh loop; pause doc queues; stop PTY readers
   at a read boundary and flush the pump's batch into the replay window; freeze
   each run (below); flush all doc snapshots. Everything not deliberately
   carried (the edge relay, sockets, tasks, short-lived helpers such as git)
   simply ends with the old image; the relay needs no teardown because the
   successor's join supersedes it.
5. **Two-phase.** Every freeze is reversible until exec. If any component
   fails to freeze, all thaw and the run continues in the old image; the
   handoff retries later.
6. **Exec.** `execve(<app_root>/current/zeron, same argv)` with
   `ZERON_HANDOFF_FD` set. If `execve` itself fails, thaw as in step 5.
7. **Adopt boot (new image).** Read the manifest read-only. Adopt the lock and
   listener instead of acquiring/binding. Register adopted `RunHandle`s and
   terminals **before** `recover_stale()` (it already skips chats with a live
   run). Start everything else normally. When assembly is complete, **commit**:
   close the manifest fd, rewrite the pid stamp, resume doc queues.
8. **Rollback.** If the new image fails before commit, it re-execs
   `from_exe` with the same manifest. The old image adopts it the same way, so
   rollback is just another adopt boot. A crash after commit takes the normal
   `Restart=on-failure` plus stale-run recovery path (`--resume`) that exists
   today.

The first update **from** a release without handoff can never hand off; that
one boundary uses today's behavior (defer until quiescent, restart).

#### Terminals

portable-pty 0.8.1 cannot build a master from a raw fd, and its writer writes
`\n` + EOT on drop, which would exit the shell during handoff. Unix terminals
therefore keep portable-pty only for spawn, then `dup` the master into a small
`PtyMaster` (libc: winsize, dup, EIO-as-EOF read, no EOT on drop), with a
`PidChild` implementing `Child`/`ChildKiller` via `waitpid`. Readers become
interruptible (poll plus a wake fd) so they can stop at a read boundary; unread
bytes stay in the kernel pty buffer. `seq` continues monotonically so a
subscriber's `afterSeq` resume works. `NamedTempFile` becomes `TempPath` and is
never dropped by the old image. `any_open()` counts only live shells.

#### Agent runs

Harness children get `kill_on_drop(false)` (a forgotten handle must not kill
them) and the ACP group-kill `Drop` is disarmed on export. Exit is observed
with `waitpid` in a blocking waiter, as before, because the new image is still
the parent.

Common seam, defaulting to "unsupported" so harnesses opt in one at a time:

- `RunControls.freeze`: a channel of `FreezeRequest{ reply }`. The run loop
  answers `Busy` unless at a safe point, else returns a `FrozenRun` whose
  `commit` sender means "a same-process successor took over" and whose drop
  means "thaw" (see [Implementation notes](#implementation-notes)).
- `HarnessHandoff { harness, state_version, pid, stdin_fd, stdout_fd,
  stderr_fd, extra_fds, stdout_leftover, stderr_tail, state: Value,
  undrained_steers }`.
- `Harness::adopt(handoff, controls, request)` rebuilds the same run loop from
  it, and `supports_adoption()` lets the engine fall back per harness.
- A `ChildHandle` (`Owned` | `Adopted`) replaces direct `Child` use.

Buffering is the crux. `BufReader` + `Lines` hold an unexportable read-ahead and
partial line, and the stdin writer issues two non-atomic `write_all` calls. So:
an owned `LineReader` (exposes leftover bytes, cancel-safe stop) replaces them
in Claude, `jsonrpc.rs` (Codex, ACP) and Cursor; the writer accepts a `Pause`
message, so queued lines finish and are never torn and the writer parks with
its queue; stderr fds are passed and
kept drained so the agent never gets `EPIPE`. On Linux stdout is raised to a
1 MiB pipe before freezing so a chatty agent does not block during the window.

Protocol state is **serialized, not replayed**. Raw lines are not retained;
Codex's `turn/start` response is swallowed by the RPC client; Claude question
and message ids are random; Codex and ACP subagent state spans turns. All of it
is small plain data (`serde` on `Normalizer`, `TurnRouter`, `Subagents`,
`SubagentTracker`, ...), versioned by `state_version`.

Safe points:

- **Best:** a session parked between turns (`idle_since` set, nothing pending).
- **Fine:** mid-turn at a line boundary; the child owns its tools.
- **Fine, with rebinding:** a parked `AskUserQuestion` /
  `requestUserInput`. `request_input` gains an adopt variant that rebinds the
  resolver under the **same** engine request id without emitting a second
  `InputRequested` (a duplicate broke answering before).
- **Never:** interrupting or escalating, Claude's 5-second `held_done` window,
  Codex setup (`initialize` through the first `turn/start` response), an
  in-flight inline JSON-RPC request (Codex; ACP instead carries its
  `session/prompt` request across by id), an OpenCode native `/command` in
  flight. These answer `Busy`; the coordinator retries.

Engine side: `drive_run` gains a `Frozen` end state that skips `Done`
publishing, the `aborted` stamp, subagent "failed" stamping, orphan
re-dispatch, and `remove_run`. `steer_rx` contents are exported back into the
`routed_steers` ledger so nothing is re-dispatched as a fresh turn.
`runtime_config` is exported so the first same-config message does not look
like a config change and interrupt the adopted child. The adopter re-takes the
harness `execution_lease`. The in-memory fold (`folded`, `seen_tools`, subagent
sinks) is exported alongside; rebuilding it from the journal is the fallback.

Per harness:

| Harness | Notes |
|---|---|
| Claude | State: normalizer (`session_id`, `last_model`, `agent_tasks`, `agent_spawn_tools`, `assistant_message_id`), `pending_steers`, `open_tools`, parked question (control-request id, raw input, question ids). |
| Codex | State: `thread_id`, `TurnRouter`, reasoning streams, `queued_steers`, `Subagents`, parked server requests, `RpcClient.next_id`. The reader is stopped and the loop's not-yet-handled messages are exported ahead of the raw leftover bytes (a marker in the message stream can deadlock, see notes). |
| ACP | State: `session_id`, `next_id`, pending ids, steering/preempt/prompt bookkeeping, subagent trackers **with tail offsets**, scratch dir path (ownership moves). |
| OpenCode | Server is loopback HTTP and already outlives stdio. Adopt reconnects the bus, then **re-reads REST state before resuming** (messages, `/permission`, `/question`, `/session/status`, children): the v1 bus has no replay, so a missed question would block the turn forever. Idle settling runs only after reconcile. The server's stdin/stdout are `/dev/null`: the handoff names them `NO_PIPE` (-1) and carries only stderr; the credential rides in `harness_state`. A thaw whose bus dropped reconciles the same way; 2.x runs freeze only between turns (their message shape is not reconciled). |
| Cursor | Engine state is tiny; turn state lives in the shim, which survives. The store lease fd is inherited (OFD lock stays held). `.zeron-owner.json` `parentPid` is unchanged (same pid). |

Pi (native RPC since #630) and any other harness not listed have not been
analyzed; they ship with `supports_adoption() == false` and adopt later.

A harness without adoption is not killed: its idle parked sessions are stopped
and resumed transparently on next dispatch (`--resume` / `thread/resume` /
`session/load`, existing behavior); a session mid-turn simply defers the
handoff.

#### What does not survive, by design

Live UI subscriptions end and resubscribe with `afterSeq` (the redialing client
reconnects by itself); a call in flight at the moment of the exec fails with
`Closed` and the UI's ordinary error path applies (there is no generic retry);
new connections during the ~1 s gap queue in the listener's backlog instead of
being refused; `diff_sync` turn
snapshots for turns in progress; preview tunnels and login callback forwarders
(veto while active).

### 2. Headed app: UI and engine host as separate processes

exec cannot swap an embedded engine without also tearing down the window, and
exec of a GUI process is not reliable on macOS. So the headed app always
attaches to an **engine host** and runs no engine of its own:

- `EngineHandle::bootstrap` gains spawn-or-attach: attach if the port answers;
  else start the host (`systemctl --user start zeron.service` if the unit is
  installed, `launchctl kickstart` if the plist exists, else a detached
  `setsid` `zeron headless`, out of the app's scope/cgroup) and attach.
- The deferred-onboarding logic (`DeferredEngineRpc`) moves from `zeron-ui`
  into the engine crate so a spawned host can wait for in-UI sign-in instead of
  failing headless.
- The UI gets a **redialing client**: `RemoteEngine` redials with backoff and
  re-runs `EngineInfo` after each dial (capabilities go stale across an
  upgrade). This also fixes today's "Ready but dead" state, and is what lets
  the UI ride out an engine handoff.
- **Quit keeps today's meaning.** An engine host the app started (a detached
  process, or an installed service it brought up from stopped) stops when the
  user quits; a daemon or service that was already running keeps running (as
  now). Only the update path detaches without stopping it. A window that
  embeds its engine (`ZERON_EMBED_ENGINE=1`, source builds, or when no host
  can be started) never swaps silently, since its engine would die with it.
- The host inherits the login-shell environment the embedded engine has today
  (PATH, `ZERON_*`, display/dbus/ssh-agent vars where available), because
  shells and agents inherit it.

### 3. UI swap

The UI cannot be patched in place, so it is replaced by a fast relaunch with
its state carried across:

1. Persist a **UI snapshot** to `ui-state.json`: selected chat, composer
   drafts and staged attachments, per-chat scroll, open right-pane surfaces,
   window geometry. It is written continuously (debounced), so it is also
   correct after a crash.
2. The new UI starts, attaches to the running engine host, restores the
   snapshot, then the old window closes. On Linux the `current` symlink is
   swapped; on macOS the bundle is swapped, both as today.
3. Timing: no prompt. The swap waits for the window to be unfocused or idle,
   and for the existing unsaved-file gate (`prepare_exit`) to pass; it never
   interrupts an active IME composition.

## Update policy (replaces the prompt)

Download in the background as today. Then: engine handoff at the next safe
moment; UI swap at the next idle/unfocused moment. The sidebar strip becomes
informational ("Updated to vX") with an opt-out setting; `ZERON_AUTO_UPDATE=0`
keeps report-only. Unmanaged installs (source builds) stay advisory.

## Platforms

- **Linux, macOS:** full design. The engine host is a headless process, so
  exec is safe on macOS too.
- **Windows:** no engine handoff in v1 (synchronous pipe reads can swallow
  bytes across a handoff, the escrow parent locks the exe, ConPTY handles are
  not transferable). Windows keeps today's flow, made silent: install on quit.
- **macOS verification:** development hardware is Linux. macOS is covered by
  unit and integration tests that run on any Unix, but **has not been run on a
  Mac**; the PR says so.

## Compatibility

The manifest and `ui-state.json` are versioned, additive JSON. An adopter that
does not recognize `version` refuses and the old image thaws. UI/engine skew
remains capability-gated; this work adds a `handoff-v1` capability and an
`EngineInfo` build version for diagnostics only (no hard gate).

## Testing

- Unit: manifest round-trip and version refusal; `PtyMaster` (winsize, EIO as
  EOF, no EOT on drop); `LineReader` leftover export; writer freeze never tears
  a line; per-harness state round-trips; `Busy` at every unsafe point.
- Integration (Unix, real processes): start `zeron headless` with a fake agent
  (scripted stream-json / JSON-RPC child) and a real shell; run a turn and a
  parked question; trigger handoff to a second copy of the binary;
  assert the agent and shell **PIDs are unchanged**, streaming continues
  without a gap or duplicate transcript entry, the parked question still
  answers, terminal `afterSeq` resume works, and the IPC port never refused a
  connection.
- Failure injection: `execve` fails (thaw), new image dies before commit
  (rollback re-exec), veto conditions retry rather than kill.
- Redial client: server restart under a live client.

## Delivery

One PR, reviewable as ordered commits: (1) redialing client, `EngineInfo`
version, UI snapshot; (2) engine host spawn-or-attach; (3) terminal handoff and
handoff core with manifest, lock/listener inheritance, veto, rollback;
(4) run-handoff seam, then harnesses one commit each (Claude, Codex, ACP,
OpenCode, Cursor); (5) update policy and UI swap. Each commit builds and passes
tests on its own.

## Implementation notes

Where the built version is more specific than, or differs from, the design
above. The spec is the intent; this is what the code does.

**Freeze is a suspension, not a departure.** The engine holds every frozen
component (`FrozenTerminals`, `FrozenRuns`) *through* the `execve`; exec runs no
destructors, so nothing has to be given up beforehand, and if `execve` fails the
engine drops them and everything resumes where it was. `commit()` exists only for
a same-process successor (tests). Consequences that shaped the code:

- The stdin writer **pauses** (`WriteMsg::Pause`): queued lines finish, then it
  parks holding the pipe and its queue; dropping the guard resumes it. There is
  no `Freeze` sentinel and no new `AgentEvent`; for a same-process commit the
  run's stream simply ends, which the engine reads as "frozen".
- A child that exits inside the freeze window must stay a zombie for the next
  image: waiters use `waitid(WNOWAIT)` and a `ReapGate` (shared by terminals and
  agent runs) holds the actual reap while a freeze is pending. Owned (tokio)
  children cannot be gated; one that exits in the window falls back to crash
  recovery.
- Steers drained from a run's mailbox at freeze time are kept and re-read first
  after a thaw, and exported as `undrained_steers` for the adopter.
- Answers the user gave just before a freeze are written to the agent *before*
  the pause, and `respond_input` takes its lock in the same order as the freeze
  barrier, so an accepted answer is never exported as still parked.

**Adoption works on duplicates.** The successor duplicates every inherited
descriptor (`F_DUPFD_CLOEXEC`); the originals stay open, inheritable and
untouched until the adoption commits, so a failed boot hands back to the old
binary with nothing missing. The engine also duplicates each run's descriptors
when the run is registered, because a harness may adopt later (behind an
execution lease) than the commit that closes the originals. The adopted
`InstanceLock` and `Terminals` are "unarmed" until commit: dropping them earlier
neither unlocks the file nor hangs up a shell.

**Rollback and the loop guard.** A successor that cannot finish booting re-execs
`from_exe` with the same manifest and tells it which version failed
(`ZERON_HANDOFF_ROLLED_BACK_FROM`); that image never hands off to the same
version again. `ZERON_HANDOFF_ATTEMPT` bounds consecutive handoff/rollback
execs. On macOS the running binary is copied aside at engine start so the
rollback target survives the bundle swap. Known limitation: an image running
from that copy no longer looks like a managed install and stops watching for
newer installs until restarted.

**Preflight** checks that the new binary runs and reads our manifest version. It
deliberately does not open the new binary's store, which a newer build could
migrate under the running engine.

**Updater.** `Busy` handoffs are retried every poll and never become a restart;
a `Failed` one backs off ten minutes (per installed version) and only then does
the installed service, at a quiet moment and once per version, fall back to a
restart. `handoff-v1` is advertised only by an engine that serves
`HandoffEngine` (the headless IPC owner). `ZERON_AUTO_UPDATE` unset means on only
for such an engine; the desktop app's in-process engine stays opt-in.

**Engine host.** `zeron headless --host` is the engine of a headed app. A captured
cloud profile that still needs its organization is onboarded by the attached
window: the host serves identity and sign-in first and keeps that same listener
for the assembled service. The app starts a host with `systemctl --user start
zeron.service` when that unit exists, `launchctl kickstart` for the agent, else
a detached process in its own session (and, where available, its own systemd
scope). The host reports `app-hosted` (from `ZERON_ENGINE_HOST=app`, which is
removed from its environment so agents never inherit it); only such a host is
stopped when its window quits, and only an update swap leaves it running. A
supervisor restarts an app-hosted host that vanishes (three times at most, never
after a deliberate stop). `ZERON_EMBED_ENGINE=1` forces the old embedded engine.

**UI swap.** The update is applied only at swap time. The new window starts in
its own session with `ZERON_UI_SWAP` set (it attaches to the running host and
never embeds), announces itself with `ui-ready-<pid>`, and only then does the old
window leave; if it does not come up within 20 s, or the user started something
in the meantime, the old window stays and the swap is retried later without
downloading again. The swap waits for an unfocused or 60 s-idle window, no IME
composition and no unsaved file edits. The new window shows "Updated to vX".

**Per harness** (`supports_adoption`): Claude, Codex, ACP (Grok, Devin, Hermes,
Antigravity), Cursor and OpenCode are implemented with fake-agent tests (real
CLIs were not run). ACP carries an in-flight `session/prompt` across by request
id instead of refusing; Cursor's store lease rides in
`HarnessHandoff.extra_fds`; OpenCode's server has no pipes (`NO_PIPE`) and
reconciles what happened in the gap from REST (2.x refuses mid-turn). Pi has no
adoption and defers a handoff while mid-turn.

**Who may become the engine.** `HandoffEngine` is reachable by any local process
on the loopback socket and the target replaces the process, so the engine only
execs this install's own binary (the managed `app_root`, the macOS bundle, or
for an unmanaged build the path it runs from), owned by its user and not
writable by others; `ZERON_HANDOFF_ANY_EXE` in the engine's own environment
lifts the location rule (tests). The preflight also reports the harness state
versions the successor can read, and the old image refuses (never kills) a run
the successor could not adopt: when a harness's state schema changes, add the
new version to `adoptable_state_versions` and keep reading the old ones.

**Known gaps.** Not exercised on macOS or Windows hardware; the GUI swap has not
been driven end to end; the swap gate does not see question-panel answers,
side-chat drafts or scroll position; the Appshots global hotkey may be unusable
after a swap until restart; harnesses without adoption defer a handoff while
mid-turn (never killed).
