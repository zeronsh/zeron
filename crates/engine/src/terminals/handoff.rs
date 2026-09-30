//! Terminal handoff — carry live shells across an in-place engine replacement.
//!
//! The engine hands itself to its successor with `execve`, which keeps the PID:
//! every shell stays a child of the same process, and only file descriptors
//! survive. So a handoff is:
//!
//! 1. [`Terminals::freeze`] — for every live terminal, stop its reader at a
//!    read boundary (unread bytes stay in the kernel's pty buffer), publish
//!    what the reader already gathered as a `Data` event, hold the shell's
//!    exit status back from the waiter and snapshot the replay window.
//!    Everything is reversible.
//! 2. [`FrozenTerminals::make_inheritable`] — as late as possible, clear
//!    `FD_CLOEXEC` on the masters so they survive the `execve`; the engine's
//!    handoff then execs while still holding the frozen state and
//!    [`FrozenTerminals::thaw`]s (also what dropping does) if the exec fails.
//!    [`FrozenTerminals::commit`] is the one-way alternative for a successor
//!    that is not an exec.
//! 3. [`Terminals::adopt`] — in the successor: re-wrap the inherited masters,
//!    restore replay and `seq`, and start reading, writing and waiting again.
//!
//! A shell that exits while frozen or in the gap stays a zombie (the
//! [`pty_unix::ReapGate`] keeps the old image from reaping it), so the
//! successor reports its real exit code.

use std::os::fd::RawFd;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::*;

/// One terminal in the handoff manifest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalHandoff {
    pub id: String,
    pub cwd: String,
    pub shell: String,
    pub pid: i32,
    /// The inherited master fd; `-1` for a terminal whose shell already exited
    /// (an inert replay buffer awaiting its TTL).
    pub master_fd: i32,
    /// Last sequence number issued; adoption continues from here, so a
    /// subscriber's `afterSeq` resume works across the handoff.
    pub seq: u64,
    pub replay: Vec<TerminalEvent>,
    pub exited: bool,
    /// Milliseconds since the terminal was last active (an `Instant` cannot
    /// be serialized; the exited-session reaper needs it).
    pub idle_ms: u64,
    /// The private action script, when one is still needed.
    pub script: Option<PathBuf>,
}

/// One live terminal while frozen — what a thaw needs to undo the freeze.
struct FrozenTerminal {
    id: String,
    session: Arc<Mutex<LiveTerminal>>,
    /// The raw-output sender the stopped reader handed back; `None` when the
    /// reader had already hit EOF (the shell's output is over).
    raw_tx: Option<mpsc::UnboundedSender<Vec<u8>>>,
    master_fd: RawFd,
    gate: Arc<pty_unix::ReapGate>,
    pump: mpsc::UnboundedSender<PumpCmd>,
}

/// Every terminal, frozen. Dropping this without [`Self::commit`] thaws.
pub struct FrozenTerminals {
    inner: Arc<TerminalsInner>,
    live: Vec<FrozenTerminal>,
    handoffs: Vec<TerminalHandoff>,
    finished: bool,
}

fn snapshot(id: &str, session: &LiveTerminal, master_fd: RawFd) -> TerminalHandoff {
    TerminalHandoff {
        id: id.to_string(),
        cwd: session.cwd.clone(),
        shell: session.shell.clone(),
        pid: session.pid,
        master_fd,
        seq: session.seq,
        replay: session.replay.iter().cloned().collect(),
        exited: session.exited,
        idle_ms: u64::try_from(session.last_active_at.elapsed().as_millis()).unwrap_or(u64::MAX),
        script: session
            .initial_script
            .as_ref()
            .map(|path| path.to_path_buf()),
    }
}

fn exiting(id: &str) -> EngineError {
    EngineError::Other(format!("terminal {id} is exiting"))
}

impl Terminals {
    /// Freeze every terminal for a handoff. Fails (and leaves everything
    /// running) if any shell is exiting right now or a reader/pump does not
    /// answer; the caller retries later ("exiting" is transient — but a
    /// background job that keeps a dead shell's pty open can keep a terminal
    /// in that state until the job ends, so callers retry with a limit and
    /// report what is blocking). While frozen, terminals can be neither opened
    /// nor closed. Do not cancel this future partway (for instance with a
    /// timeout): a reader thread's output sender would be lost and that
    /// terminal would stop producing output. Bound it from the inside instead.
    pub async fn freeze(&self) -> Result<FrozenTerminals, EngineError> {
        // The flag and the session list are taken under one lock: `open` and
        // `close` check the flag under the same lock, so every terminal is
        // either in this list or refused.
        let sessions: Vec<_> = {
            let sessions = lock(&self.inner.sessions);
            if self.inner.frozen.swap(true, Ordering::SeqCst) {
                return Err(EngineError::Other("terminals are already frozen".into()));
            }
            sessions
                .iter()
                .map(|(id, session)| (id.clone(), session.clone()))
                .collect()
        };
        // From here `frozen` owns the flag: dropping it (on any error below)
        // thaws what was frozen so far and reopens the terminals.
        let mut frozen = FrozenTerminals {
            inner: self.inner.clone(),
            live: Vec::new(),
            handoffs: Vec::new(),
            finished: false,
        };
        for (id, session) in sessions {
            frozen.freeze_one(&id, &session).await?;
        }
        Ok(frozen)
    }

    /// Rebuild terminals from a predecessor's manifest entries. Must run
    /// inside a tokio runtime, before anything else can open a terminal. A
    /// terminal whose master fd did not arrive is dropped (and logged): its
    /// shell is unreachable.
    pub fn adopt(handoffs: Vec<TerminalHandoff>) -> Result<Self, EngineError> {
        let mut sessions = HashMap::new();
        for handoff in handoffs {
            match adopt_one(&handoff) {
                Ok(session) => {
                    sessions.insert(handoff.id.clone(), session);
                }
                Err(err) => {
                    tracing::error!(terminal = %handoff.id, error = %err, "could not adopt terminal");
                    // Nobody will hold this shell's master once the inherited
                    // originals close (at the adoption's commit), so the
                    // kernel hangs it up. Reap it then, so it does not linger
                    // as a zombie. (A rollback exec replaces this image before
                    // the wait returns, and the wait does not reap on its
                    // own — see `wait_pid_gated` — so the predecessor still
                    // finds the shell.)
                    if !handoff.exited && handoff.pid > 1 {
                        let pid = handoff.pid;
                        tokio::task::spawn_blocking(move || pty_unix::wait_pid_blocking(pid));
                    }
                }
            }
        }
        let terminals = Self::from_sessions(sessions);
        terminals.inner.unarmed.store(true, Ordering::SeqCst);
        Ok(terminals)
    }

    /// The adoption committed: from here these terminals belong to this
    /// engine, and dropping or shutting them down hangs the shells up like any
    /// other. Until then dropping them leaves the shells running (see
    /// [`TerminalsInner`]'s `unarmed`).
    pub fn arm(&self) {
        self.inner.unarmed.store(false, Ordering::SeqCst);
    }
}

impl FrozenTerminals {
    /// The manifest entries as they stand (every terminal, live or exited).
    pub fn handoffs(&self) -> &[TerminalHandoff] {
        &self.handoffs
    }

    async fn freeze_one(
        &mut self,
        id: &str,
        session: &Arc<Mutex<LiveTerminal>>,
    ) -> Result<(), EngineError> {
        let (gate, reader, pump) = {
            let mut terminal = lock(session);
            if terminal.exited {
                // An inert replay buffer: nothing to stop, only to carry.
                self.handoffs.push(snapshot(id, &terminal, -1));
                return Ok(());
            }
            // The output pump is mid-exit (it took the master) or the waiter
            // already reaped the shell: this terminal is dying. Not a state
            // to hand over; the caller retries once it has settled.
            if terminal.master.is_none() || !terminal.reap_gate.hold() {
                return Err(exiting(id));
            }
            (
                terminal.reap_gate.clone(),
                terminal.reader.take(),
                terminal.pump.clone(),
            )
        };
        // The gate is held: register for thaw before anything below can fail.
        self.live.push(FrozenTerminal {
            id: id.to_string(),
            session: session.clone(),
            raw_tx: None,
            master_fd: -1,
            gate,
            pump: pump.clone(),
        });

        if let Some(reader) = reader {
            reader.stop.stop();
            let joined = tokio::task::spawn_blocking(move || reader.thread.join())
                .await
                .map_err(|e| EngineError::Other(format!("pty reader join: {e}")))?;
            let raw_tx =
                joined.map_err(|_| EngineError::Other(format!("pty reader for {id} panicked")))?;
            if let Some(last) = self.live.last_mut() {
                last.raw_tx = raw_tx;
            }
        }

        // Everything the reader gathered becomes a `Data` event (with a seq)
        // before the snapshot, and the pump parks so nothing else is emitted.
        let (ack_tx, ack_rx) = oneshot::channel();
        pump.send(PumpCmd::FlushAndPause(ack_tx))
            .map_err(|_| exiting(id))?;
        tokio::time::timeout(Duration::from_secs(3), ack_rx)
            .await
            .map_err(|_| EngineError::Other(format!("terminal {id} did not pause its output")))?
            .map_err(|_| exiting(id))?;

        let handoff = {
            let terminal = lock(session);
            let master = terminal.master.as_ref().ok_or_else(|| exiting(id))?;
            let fd = master.as_raw_fd();
            if let Some(last) = self.live.last_mut() {
                last.master_fd = fd;
            }
            snapshot(id, &terminal, fd)
        };
        self.handoffs.push(handoff);
        Ok(())
    }

    /// Let the masters survive an `execve` (clear `FD_CLOEXEC`). Deliberately
    /// separate from the freeze and called as late as possible: while a master
    /// is inheritable, any child the engine spawns would inherit it too.
    /// [`Self::thaw`] makes them close-on-exec again.
    pub fn make_inheritable(&self) -> std::io::Result<()> {
        for frozen in &self.live {
            pty_unix::set_inheritable(frozen.master_fd, true)?;
        }
        Ok(())
    }

    /// Undo the freeze: restart readers, resume the pumps, let waiters reap
    /// again and reopen the terminals. Idempotent.
    pub fn thaw(mut self) {
        self.thaw_inner();
    }

    fn thaw_inner(&mut self) {
        if std::mem::replace(&mut self.finished, true) {
            return;
        }
        for frozen in self.live.drain(..) {
            {
                let mut terminal = lock(&frozen.session);
                if frozen.master_fd >= 0
                    && let Err(err) = pty_unix::set_inheritable(frozen.master_fd, false)
                {
                    tracing::error!(
                        terminal = %frozen.id,
                        error = %err,
                        "could not make a terminal master close-on-exec again"
                    );
                }
                if let (Some(tx), Some(master)) = (frozen.raw_tx, terminal.master.as_ref()) {
                    let restarted = master
                        .reader()
                        .and_then(|(reader, stop)| spawn_reader(&frozen.id, reader, stop, tx));
                    match restarted {
                        Ok(handle) => terminal.reader = Some(handle),
                        Err(err) => tracing::error!(
                            terminal = %frozen.id,
                            error = %err,
                            "could not restart the pty reader after a thaw"
                        ),
                    }
                }
            }
            let _ = frozen.pump.send(PumpCmd::Resume);
            frozen.gate.release();
        }
        self.inner.frozen.store(false, Ordering::SeqCst);
    }

    /// Give the terminals to a successor that lives in this same process
    /// image's stead. ONE-WAY: the masters are forgotten (never closed here),
    /// the action scripts are kept, and each shell's [`ReapGate`] stays held
    /// forever, so this image can no longer reap a shell or report its exit,
    /// and the terminals stay frozen (no open/close). An `execve` does NOT
    /// need this: exec runs no destructors, so the engine's handoff keeps the
    /// `FrozenTerminals`, calls [`Self::make_inheritable`], and — if the exec
    /// fails — simply [`Self::thaw`]s. Use `commit` only when the old image
    /// keeps running without its terminals (tests standing in for an exec).
    pub fn commit(mut self) -> Vec<TerminalHandoff> {
        self.finished = true;
        for frozen in self.live.drain(..) {
            let mut terminal = lock(&frozen.session);
            if let Some(master) = terminal.master.take() {
                std::mem::forget(master);
            }
            drop(terminal.writer.take());
            if let Some(script) = terminal.initial_script.take() {
                let _ = script.keep();
            }
        }
        std::mem::take(&mut self.handoffs)
    }
}

impl Drop for FrozenTerminals {
    fn drop(&mut self) {
        self.thaw_inner();
    }
}

fn adopt_one(handoff: &TerminalHandoff) -> Result<Arc<Mutex<LiveTerminal>>, EngineError> {
    let replay: VecDeque<TerminalEvent> = handoff.replay.iter().cloned().collect();
    let replay_bytes = replay.iter().map(event_bytes).sum();
    let last_active_at = std::time::Instant::now()
        .checked_sub(Duration::from_millis(handoff.idle_ms))
        .unwrap_or_else(std::time::Instant::now);
    let initial_script = handoff
        .script
        .clone()
        .and_then(|path| tempfile::TempPath::try_from_path(path).ok());
    let (pump_tx, pump_rx) = mpsc::unbounded_channel();
    let reap_gate = pty_unix::ReapGate::new();

    // `exited` is authoritative: a live terminal with no usable master is
    // invalid (rejected below), not an exited one.
    if handoff.exited {
        return Ok(Arc::new(Mutex::new(LiveTerminal {
            initial_script,
            master: None,
            writer: None,
            killer: Box::new(pty_unix::PidKiller(handoff.pid)),
            cwd: handoff.cwd.clone(),
            shell: handoff.shell.clone(),
            pump: pump_tx,
            pid: handoff.pid,
            reap_gate,
            reader: None,
            subscribers: Vec::new(),
            replay,
            replay_bytes,
            seq: handoff.seq,
            last_active_at,
            exited: true,
        })));
    }

    // Work on a dup (close-on-exec) and leave the inherited number alone: a
    // failed adoption boot hands the same manifest back to the predecessor,
    // whose masters must still be open, and a stale manifest must never make
    // us close a descriptor we do not own. The originals are closed once the
    // adoption commits (`Adoption::commit`).
    if !pty_unix::is_pty_master(handoff.master_fd) {
        return Err(EngineError::Other(format!(
            "inherited descriptor {} is not a pty master",
            handoff.master_fd
        )));
    }
    let master = pty_unix::PtyMaster::from_raw_dup(handoff.master_fd).map_err(|e| {
        EngineError::Other(format!("inherited master fd {}: {e}", handoff.master_fd))
    })?;
    let (reader, stop) = master
        .reader()
        .map_err(|e| EngineError::Other(format!("pty reader: {e}")))?;
    let writer = master
        .writer()
        .map_err(|e| EngineError::Other(format!("pty writer: {e}")))?;

    let session = Arc::new(Mutex::new(LiveTerminal {
        initial_script,
        master: Some(master),
        writer: Some(writer),
        killer: Box::new(pty_unix::PidKiller(handoff.pid)),
        cwd: handoff.cwd.clone(),
        shell: handoff.shell.clone(),
        pump: pump_tx,
        pid: handoff.pid,
        reap_gate: reap_gate.clone(),
        reader: None,
        subscribers: Vec::new(),
        replay,
        replay_bytes,
        seq: handoff.seq,
        last_active_at,
        exited: false,
    }));
    let (raw_tx, raw_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let handle = spawn_reader(&handoff.id, reader, stop, raw_tx)
        .map_err(|e| EngineError::Other(format!("pty reader thread: {e}")))?;
    lock(&session).reader = Some(handle);
    let pid = handoff.pid;
    // We are still the shell's parent, so a shell that exited in the gap is a
    // zombie we can reap for its real exit code.
    let wait = tokio::task::spawn_blocking(move || pty_unix::wait_pid_gated(pid, &reap_gate));
    tokio::spawn(pump_output(Arc::downgrade(&session), raw_rx, wait, pump_rx));
    Ok(session)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seq_of(event: &TerminalEvent) -> u64 {
        match event {
            TerminalEvent::Data { seq, .. } | TerminalEvent::Exit { seq, .. } => *seq,
        }
    }

    fn output(events: &[TerminalEvent]) -> Vec<u8> {
        events
            .iter()
            .filter_map(|event| match event {
                TerminalEvent::Data { data, .. } => Some(BASE64.decode(data).unwrap()),
                TerminalEvent::Exit { .. } => None,
            })
            .flatten()
            .collect()
    }

    fn open_sh(terminals: &Terminals) -> TerminalSession {
        terminals
            .open_with_shell(
                &std::env::temp_dir().to_string_lossy(),
                80,
                24,
                Some("/bin/sh"),
            )
            .expect("open sh")
    }

    /// Everything the terminal has produced so far, once `done` matches its
    /// output. Terminals echo their input, so tests wait for text the shell
    /// COMPUTES (`$((1+1))`), which the echoed command line does not contain.
    async fn output_until(
        terminals: &Terminals,
        id: &str,
        done: impl Fn(&str) -> bool,
    ) -> Vec<TerminalEvent> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        let mut rx = terminals.subscribe(id, None).expect("subscribe");
        let mut events = Vec::new();
        loop {
            if done(&String::from_utf8_lossy(&output(&events))) {
                return events;
            }
            let event = tokio::time::timeout_at(deadline, rx.recv())
                .await
                .unwrap_or_else(|_| {
                    panic!(
                        "no matching output before the timeout; got {:?}",
                        String::from_utf8_lossy(&output(&events))
                    )
                })
                .expect("terminal stream alive");
            events.push(event);
        }
    }

    /// Everything up to and including the `Exit` event, and its exit code.
    async fn until_exit(terminals: &Terminals, id: &str) -> (Vec<TerminalEvent>, i32) {
        let mut rx = terminals.subscribe(id, None).expect("subscribe");
        let mut events = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            let event = tokio::time::timeout_at(deadline, rx.recv())
                .await
                .expect("exit before timeout")
                .expect("stream ends only after Exit");
            let exit = match &event {
                TerminalEvent::Exit { exit_code, .. } => Some(*exit_code),
                TerminalEvent::Data { .. } => None,
            };
            events.push(event);
            if let Some(code) = exit {
                return (events, code);
            }
        }
    }

    fn assert_contiguous(events: &[TerminalEvent]) {
        let seqs: Vec<u64> = events.iter().map(seq_of).collect();
        assert!(
            seqs.windows(2).all(|pair| pair[1] == pair[0] + 1),
            "seq must be contiguous: {seqs:?}"
        );
    }

    fn alive(pid: i32) -> bool {
        unsafe { libc::kill(pid, 0) == 0 }
    }

    fn cloexec(fd: i32) -> bool {
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        flags >= 0 && flags & libc::FD_CLOEXEC != 0
    }

    fn fd_open(fd: i32) -> bool {
        unsafe { libc::fcntl(fd, libc::F_GETFD) >= 0 }
    }

    /// Wait until `pid` has exited but not been reaped (a zombie).
    async fn wait_for_zombie(pid: i32) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            let rc = unsafe {
                libc::waitid(
                    libc::P_PID,
                    pid as libc::id_t,
                    &mut info,
                    libc::WEXITED | libc::WNOWAIT | libc::WNOHANG,
                )
            };
            if rc == 0 && unsafe { info.si_pid() } == pid {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "pid {pid} never became a zombie"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Stand-in for the exec: the old image never touches its terminals again
    /// (dropping them would kill the shells the successor now owns). Returns
    /// the old waiters' gates so a test can let those parked threads finish
    /// at the end; a real exec destroys them instead.
    fn retire(old: Terminals) -> OldImage {
        let gates = lock(&old.inner.sessions)
            .values()
            .map(|session| lock(session).reap_gate.clone())
            .collect();
        std::mem::forget(old);
        OldImage(gates)
    }

    /// Releases the retired image's parked waiters when the test ends —
    /// including when an assertion fails; left parked they would hang the
    /// runtime's shutdown instead of letting the failure be reported.
    struct OldImage(Vec<Arc<pty_unix::ReapGate>>);

    impl Drop for OldImage {
        fn drop(&mut self) {
            for gate in &self.0 {
                gate.release();
            }
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn freeze_and_thaw_cycles_lose_and_duplicate_no_output() {
        let terminals = Terminals::new();
        let session = open_sh(&terminals);
        terminals
            .write_bytes(
                &session.id,
                b"i=0; while [ $i -lt 400 ]; do echo line-$i; i=$((i+1)); done; echo DONE-$((1+1))\n",
            )
            .unwrap();
        // Freeze at moments spread across the burst, including mid-batch.
        for delay_ms in [0, 1, 3, 2, 5, 1] {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            let frozen = terminals.freeze().await.expect("freeze");
            assert_eq!(frozen.handoffs().len(), 1);
            assert!(
                alive(frozen.handoffs()[0].pid),
                "the shell survives a freeze"
            );
            frozen.thaw();
        }
        let events =
            output_until(&terminals, &session.id, |text| text.contains("DONE-2\r\n")).await;
        let text = String::from_utf8_lossy(&output(&events)).to_string();
        for i in 0..400 {
            assert_eq!(
                text.matches(&format!("line-{i}\r\n")).count(),
                1,
                "line-{i} exactly once"
            );
        }
        assert_contiguous(&events);
        terminals.shutdown();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_snapshot_carries_replay_seq_and_binary_output() {
        let terminals = Terminals::new();
        let session = open_sh(&terminals);
        terminals
            .write_bytes(&session.id, b"printf 'a\\377b\\n'; echo READY-$((1+1))\n")
            .unwrap();
        output_until(&terminals, &session.id, |text| text.contains("READY-2\r\n")).await;
        let frozen = terminals.freeze().await.unwrap();
        let handoff = frozen.handoffs()[0].clone();
        let bytes = output(&handoff.replay);
        assert!(
            bytes.windows(3).any(|window| window == [b'a', 0xff, b'b']),
            "a non-UTF-8 byte survives: {bytes:?}"
        );
        assert_contiguous(&handoff.replay);
        assert_eq!(handoff.replay.last().map(seq_of), Some(handoff.seq));
        assert!(handoff.master_fd >= 3 && !handoff.exited);
        frozen.thaw();
        terminals
            .write_bytes(&session.id, b"echo still-$((1+1))\n")
            .unwrap();
        output_until(&terminals, &session.id, |text| text.contains("still-2\r\n")).await;
        terminals.shutdown();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn terminals_cannot_be_opened_or_closed_while_frozen() {
        let terminals = Terminals::new();
        let session = open_sh(&terminals);
        let frozen = terminals.freeze().await.unwrap();
        assert!(
            terminals
                .open_with_shell(
                    &std::env::temp_dir().to_string_lossy(),
                    80,
                    24,
                    Some("/bin/sh")
                )
                .is_err()
        );
        assert!(terminals.close(&session.id).is_err());
        assert!(terminals.freeze().await.is_err(), "no second freeze");
        drop(frozen); // dropping thaws
        let second = open_sh(&terminals);
        terminals.close(&second.id).unwrap();
        terminals.close(&session.id).unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn adopt_continues_seq_and_the_same_shell() {
        let old = Terminals::new();
        let session = open_sh(&old);
        old.write_bytes(&session.id, b"echo before-$((1+1))\n")
            .unwrap();
        output_until(&old, &session.id, |text| text.contains("before-2\r\n")).await;
        let frozen = old.freeze().await.unwrap();
        let handoffs = frozen.commit();
        let handoff = handoffs[0].clone();
        let _old_image = retire(old);

        let adopted = Terminals::adopt(handoffs).unwrap();
        assert!(adopted.any_live());
        adopted
            .write_bytes(&session.id, b"echo after-$((1+1))\n")
            .unwrap();
        let events = output_until(&adopted, &session.id, |text| text.contains("after-2\r\n")).await;
        let text = String::from_utf8_lossy(&output(&events)).to_string();
        assert!(
            text.contains("before-2\r\n"),
            "the replay window came across"
        );
        assert_contiguous(&events);
        assert!(
            events.iter().any(|event| seq_of(event) > handoff.seq),
            "adoption continues past the predecessor's last seq"
        );
        // Same process, same shell.
        adopted.write_bytes(&session.id, b"echo $$\n").unwrap();
        // The echoed command line holds "$$", never the number; only the
        // shell's output ends in "<pid>\r\n".
        output_until(&adopted, &session.id, |text| {
            text.contains(&format!("{}\r\n", handoff.pid))
        })
        .await;
        assert!(
            fd_open(handoff.master_fd),
            "the inherited original stays open until the adoption commits"
        );
        adopted.arm(); // as `commit_adoption` does before an engine can shut down
        adopted.shutdown();
        unsafe { libc::close(handoff.master_fd) };
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_shell_that_exits_during_the_gap_reports_its_real_exit_code() {
        let old = Terminals::new();
        let session = open_sh(&old);
        let frozen = old.freeze().await.unwrap();
        let handoffs = frozen.commit();
        let master_fd = handoffs[0].master_fd;
        // The gap: the shell exits while no engine is reading. Type the
        // command through the inherited master, as a user's keystroke would
        // have reached it.
        let command = b"exit 5\n";
        let written = unsafe { libc::write(master_fd, command.as_ptr().cast(), command.len()) };
        assert_eq!(written, command.len() as isize);
        tokio::time::sleep(Duration::from_millis(500)).await; // the shell is a zombie now
        let _old_image = retire(old);

        let adopted = Terminals::adopt(handoffs).unwrap();
        let (_events, exit) = until_exit(&adopted, &session.id).await;
        assert_eq!(exit, 5, "the successor reaped the real status");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn exited_terminals_are_carried_as_inert_replay_buffers() {
        let old = Terminals::new();
        let session = open_sh(&old);
        old.write_bytes(&session.id, b"echo bye-$((1+1)); exit 3\n")
            .unwrap();
        let (_events, code) = until_exit(&old, &session.id).await;
        assert_eq!(code, 3);
        assert!(!old.any_live());
        assert!(old.any_open(), "an exited session still awaits its TTL");
        let frozen = old.freeze().await.unwrap();
        assert!(frozen.handoffs()[0].exited);
        assert_eq!(frozen.handoffs()[0].master_fd, -1);
        let handoffs = frozen.commit();
        let _old_image = retire(old);

        let adopted = Terminals::adopt(handoffs).unwrap();
        assert!(!adopted.any_live());
        let (events, code) = until_exit(&adopted, &session.id).await;
        assert_eq!(code, 3);
        assert!(String::from_utf8_lossy(&output(&events)).contains("bye-2\r\n"));
        assert!(adopted.write_bytes(&session.id, b"x").is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_terminal_whose_master_never_arrived_is_dropped_not_fatal() {
        let old = Terminals::new();
        let good = open_sh(&old);
        let mut handoffs = old.freeze().await.unwrap().commit();
        let _old_image = retire(old);
        let mut lost = handoffs[0].clone();
        lost.id = "lost".into();
        lost.master_fd = 9999;
        handoffs.push(lost);
        let adopted = Terminals::adopt(handoffs).unwrap();
        assert!(adopted.subscribe(&good.id, None).is_ok());
        assert!(adopted.subscribe("lost", None).is_err());
        adopted.arm(); // as `commit_adoption` does before an engine can shut down
        adopted.shutdown();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_dying_terminal_makes_the_freeze_retryable_not_fatal() {
        let terminals = Terminals::new();
        let session = open_sh(&terminals);
        terminals.write_bytes(&session.id, b"exit\n").unwrap();
        // Whichever side of the exit the freeze lands on, it must either
        // freeze cleanly or refuse; and it must leave the terminals usable.
        for _ in 0..20 {
            match terminals.freeze().await {
                Ok(frozen) => frozen.thaw(),
                Err(err) => assert!(err.to_string().contains("exiting"), "{err}"),
            }
        }
        let (_events, code) = until_exit(&terminals, &session.id).await;
        assert_eq!(code, 0);
        assert!(!terminals.any_live());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn opens_racing_a_freeze_are_frozen_or_refused_never_missed() {
        let terminals = Terminals::new();
        // Open (and close) continuously for the whole test, so every freeze
        // races an open that is in flight. A fixed count would be used up
        // during the first freeze, leaving the later freezes unraced.
        let stop = Arc::new(AtomicBool::new(false));
        let opener = {
            let terminals = terminals.clone();
            let stop = stop.clone();
            tokio::spawn(async move {
                while !stop.load(Ordering::SeqCst) {
                    if let Ok(session) = terminals.open_with_shell(
                        &std::env::temp_dir().to_string_lossy(),
                        80,
                        24,
                        Some("/bin/sh"),
                    ) {
                        // Closing is refused while frozen too; either outcome is fine.
                        let _ = terminals.close(&session.id);
                    }
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            })
        };
        for _ in 0..15 {
            match terminals.freeze().await {
                Ok(frozen) => {
                    let manifest: std::collections::BTreeSet<_> =
                        frozen.handoffs().iter().map(|h| h.id.clone()).collect();
                    // Give a racing open every chance to slip in behind the snapshot.
                    tokio::time::sleep(Duration::from_millis(15)).await;
                    let now: std::collections::BTreeSet<_> =
                        lock(&terminals.inner.sessions).keys().cloned().collect();
                    assert_eq!(
                        now, manifest,
                        "no terminal appeared or vanished behind the snapshot"
                    );
                    frozen.thaw();
                }
                Err(err) => assert!(err.to_string().contains("exiting"), "{err}"),
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        stop.store(true, Ordering::SeqCst);
        opener.await.unwrap();
        terminals.shutdown();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn adopting_a_garbled_manifest_never_closes_descriptors_it_does_not_own() {
        let old = Terminals::new();
        let good = open_sh(&old);
        let mut handoffs = old.freeze().await.unwrap().commit();
        let _old_image = retire(old);
        let good_master = handoffs[0].master_fd;
        // A regular file the process owns: open, not a pty master.
        let file = std::fs::File::open("/etc/hostname")
            .or_else(|_| std::fs::File::open("/dev/null"))
            .unwrap();
        let raw = std::os::fd::AsRawFd::as_raw_fd(&file);
        for (name, fd) in [("stdin", 0), ("file", raw), ("closed", 9999), ("neg", -1)] {
            let mut bad = handoffs[0].clone();
            bad.id = format!("bad-{name}");
            bad.master_fd = fd;
            handoffs.push(bad);
        }
        let adopted = Terminals::adopt(handoffs).unwrap();
        assert!(adopted.subscribe(&good.id, None).is_ok());
        for name in ["stdin", "file", "closed", "neg"] {
            assert!(adopted.subscribe(&format!("bad-{name}"), None).is_err());
        }
        assert!(fd_open(0), "stdin was not closed");
        assert!(fd_open(raw), "the unrelated file descriptor was not closed");
        assert!(
            fd_open(good_master),
            "the inherited original is left for the commit to close"
        );
        adopted.arm(); // as `commit_adoption` does before an engine can shut down
        adopted.shutdown();
        unsafe { libc::close(good_master) };
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_shell_that_exits_while_frozen_is_not_reaped_until_thawed() {
        let terminals = Terminals::new();
        let session = open_sh(&terminals);
        let frozen = terminals.freeze().await.unwrap();
        let (pid, fd) = (frozen.handoffs()[0].pid, frozen.handoffs()[0].master_fd);
        let command = b"exit 7\n";
        assert_eq!(
            unsafe { libc::write(fd, command.as_ptr().cast(), command.len()) },
            command.len() as isize
        );
        wait_for_zombie(pid).await;
        // Still a zombie a moment later: the old image's waiter is parked at the gate.
        tokio::time::sleep(Duration::from_millis(200)).await;
        wait_for_zombie(pid).await;
        frozen.thaw();
        let (_events, code) = until_exit(&terminals, &session.id).await;
        assert_eq!(code, 7, "the real status survived the freeze");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn only_make_inheritable_lets_the_masters_survive_an_exec() {
        let terminals = Terminals::new();
        open_sh(&terminals);
        let frozen = terminals.freeze().await.unwrap();
        let fd = frozen.handoffs()[0].master_fd;
        assert!(cloexec(fd), "closed by an exec until asked");
        frozen.make_inheritable().unwrap();
        assert!(!cloexec(fd));
        frozen.thaw();
        assert!(cloexec(fd), "a thaw makes it close-on-exec again");
        terminals.shutdown();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_unarmed_adoption_leaves_shells_and_scripts_alone_when_dropped() {
        let old = Terminals::new();
        old.open_with_command(
            "/tmp",
            80,
            24,
            &std::collections::HashMap::new(),
            "echo scripted",
        )
        .unwrap();
        let frozen = old.freeze().await.unwrap();
        let handoffs = frozen.commit();
        let handoff = handoffs[0].clone();
        let _old_image = retire(old);
        let script = handoff
            .script
            .clone()
            .expect("a command terminal has a script");
        assert!(script.exists());

        // A boot that fails after adopting: the half-built engine graph is
        // dropped, and the predecessor is about to take the shells back. Its
        // waiter stays parked at the reap gate (the exec would end that
        // thread); the guard lets it finish when this test ends.
        let first = Terminals::adopt(handoffs.clone()).unwrap();
        let _first_graph = OldImage(
            lock(&first.inner.sessions)
                .values()
                .map(|session| lock(session).reap_gate.clone())
                .collect(),
        );
        drop(first);
        tokio::time::sleep(Duration::from_millis(500)).await; // a hang-up would have landed by now
        assert!(
            alive(handoff.pid),
            "the shell survives an adoption that never committed"
        );
        assert!(
            script.exists(),
            "so does the action script the predecessor still needs"
        );
        assert!(fd_open(handoff.master_fd), "and the inherited master");

        // Once committed the terminals are this engine's: dropping them hangs
        // the shells up like any other engine's would.
        let adopted = Terminals::adopt(handoffs).unwrap();
        adopted.arm();
        drop(adopted);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while alive(handoff.pid) {
            assert!(
                std::time::Instant::now() < deadline,
                "an armed drop hangs the shell up"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        unsafe { libc::close(handoff.master_fd) };
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn any_live_ignores_exited_sessions() {
        let terminals = Terminals::new();
        assert!(!terminals.any_live());
        let session = open_sh(&terminals);
        assert!(terminals.any_live());
        terminals.write_bytes(&session.id, b"exit\n").unwrap();
        until_exit(&terminals, &session.id).await;
        assert!(!terminals.any_live());
        assert!(terminals.any_open());
    }
}
