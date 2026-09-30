//! Unix pty primitives for terminals that can outlive an engine image.
//!
//! portable-pty is kept only to open the pty pair and spawn the shell. Its
//! master cannot be rebuilt from a raw fd, its writer sends `\n` + EOT when
//! dropped (which would exit a shell during a live handoff), and its reader
//! cannot be interrupted. These types cover exactly those gaps:
//!
//! - [`PtyMaster`] owns a dup of the master fd, so it can be inherited across
//!   `execve` and re-wrapped by the next image;
//! - [`PtyReader`] polls the master together with a self-pipe, so a freeze can
//!   stop it at a read boundary without consuming or losing a byte (unread
//!   bytes stay in the kernel's pty buffer for the next reader). The master is
//!   set `O_NONBLOCK` and polled with a bounded timeout, so the reader never
//!   depends on `poll(2)` on a pty master being reliable (it is not on macOS):
//!   worst case it re-checks the stop pipe and retries the read every
//!   [`POLL_INTERVAL_MS`];
//! - [`PtyWriter`] closes without writing anything;
//! - [`wait_pid_blocking`] blocks with `waitid(WNOWAIT)` and reaps only
//!   afterwards, so a waiter that dies with the old process image leaves the
//!   zombie — and its exit status — for the new image (same parent PID).

use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

use portable_pty::PtySize;

/// How long the reader waits in `poll` before re-checking the stop pipe and
/// retrying a (non-blocking) read regardless of what `poll` reported.
const POLL_INTERVAL_MS: libc::c_int = 100;
/// How long a write may wait for the shell to consume input before giving up.
const WRITE_STALL_LIMIT: std::time::Duration = std::time::Duration::from_secs(5);
/// Backoff when `poll` says the master is ready but a read finds nothing, so
/// a misreporting `poll` degrades to a slow loop instead of a hot spin.
const MISREPORT_BACKOFF: std::time::Duration = std::time::Duration::from_millis(2);

/// Whether `fd` is an open character device that is a terminal (a pty
/// master is): a cheap guard against adopting a stale or garbled descriptor.
pub(crate) fn is_pty_master(fd: RawFd) -> bool {
    if fd < 3 {
        return false;
    }
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    let statted = unsafe { libc::fstat(fd, &mut stat) } == 0;
    let is_tty = unsafe { libc::isatty(fd) } == 1;
    statted && (stat.st_mode & libc::S_IFMT) == libc::S_IFCHR && is_tty
}

fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    let flags = cvt(unsafe { libc::fcntl(fd, libc::F_GETFL) })?;
    cvt(unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) })?;
    Ok(())
}

fn cvt(rc: libc::c_int) -> io::Result<libc::c_int> {
    if rc < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(rc)
    }
}

fn set_cloexec(fd: RawFd) -> io::Result<()> {
    set_inheritable(fd, false)
}

/// Whether `fd` survives an `execve` (clears or sets `FD_CLOEXEC`).
pub(crate) fn set_inheritable(fd: RawFd, inheritable: bool) -> io::Result<()> {
    let flags = cvt(unsafe { libc::fcntl(fd, libc::F_GETFD) })?;
    let flags = if inheritable {
        flags & !libc::FD_CLOEXEC
    } else {
        flags | libc::FD_CLOEXEC
    };
    cvt(unsafe { libc::fcntl(fd, libc::F_SETFD, flags) })?;
    Ok(())
}

/// A dup of a pty master fd. `FD_CLOEXEC` is set; a handoff clears it on the
/// fds it means to keep.
pub(crate) struct PtyMaster {
    fd: OwnedFd,
}

impl PtyMaster {
    /// Duplicate `fd` (close-on-exec) without taking ownership of it.
    pub(crate) fn from_raw_dup(fd: RawFd) -> io::Result<Self> {
        let dup = cvt(unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) })?;
        Ok(Self {
            fd: unsafe { OwnedFd::from_raw_fd(dup) },
        })
    }

    pub(crate) fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }

    pub(crate) fn resize(&self, size: PtySize) -> io::Result<()> {
        let ws = libc::winsize {
            ws_row: size.rows,
            ws_col: size.cols,
            ws_xpixel: size.pixel_width,
            ws_ypixel: size.pixel_height,
        };
        cvt(unsafe { libc::ioctl(self.fd.as_raw_fd(), libc::TIOCSWINSZ as _, &ws) })?;
        Ok(())
    }

    /// `(cols, rows)` as the kernel reports them.
    #[cfg(test)]
    pub(crate) fn size(&self) -> io::Result<(u16, u16)> {
        let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
        cvt(unsafe { libc::ioctl(self.fd.as_raw_fd(), libc::TIOCGWINSZ as _, &mut ws) })?;
        Ok((ws.ws_col, ws.ws_row))
    }

    /// A reader on its own dup of the master, and the handle that stops it.
    pub(crate) fn reader(&self) -> io::Result<(PtyReader, ReaderStop)> {
        let mut pipe = [0 as libc::c_int; 2];
        cvt(unsafe { libc::pipe(pipe.as_mut_ptr()) })?;
        let (wake_read, wake_write) =
            unsafe { (OwnedFd::from_raw_fd(pipe[0]), OwnedFd::from_raw_fd(pipe[1])) };
        set_cloexec(wake_read.as_raw_fd())?;
        set_cloexec(wake_write.as_raw_fd())?;
        // `stop()` must never block, even if it is called over and over.
        set_nonblocking(wake_write.as_raw_fd())?;
        let dup = Self::from_raw_dup(self.as_raw_fd())?;
        // O_NONBLOCK lives on the open file description, which every dup of
        // this master shares: reads never block (the reader polls with a
        // timeout) and `PtyWriter` waits out EAGAIN itself.
        set_nonblocking(dup.as_raw_fd())?;
        Ok((
            PtyReader {
                fd: dup.fd,
                wake: wake_read,
            },
            ReaderStop { wake: wake_write },
        ))
    }

    /// A writer on its own dup of the master. Dropping it writes nothing.
    pub(crate) fn writer(&self) -> io::Result<PtyWriter> {
        Ok(PtyWriter(Self::from_raw_dup(self.as_raw_fd())?.fd))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ReadOutcome {
    Data(usize),
    /// The slave side closed (the shell exited).
    Eof,
    /// [`ReaderStop::stop`] was called, or its handle was dropped (closing the
    /// wake pipe reads as a stop); no byte was consumed by this call.
    Stopped,
}

pub(crate) struct PtyReader {
    fd: OwnedFd,
    wake: OwnedFd,
}

impl PtyReader {
    /// Blocks until data, EOF, or a stop request. Once stopped, always
    /// returns [`ReadOutcome::Stopped`]. `buf` must not be empty.
    pub(crate) fn read_chunk(&mut self, buf: &mut [u8]) -> io::Result<ReadOutcome> {
        debug_assert!(!buf.is_empty(), "a zero-length read would report EOF");
        let mut misreported = false;
        loop {
            let mut fds = [
                libc::pollfd {
                    fd: self.wake.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: self.fd.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            let rc = unsafe { libc::poll(fds.as_mut_ptr(), 2, POLL_INTERVAL_MS) };
            if rc < 0 {
                let err = io::Error::last_os_error();
                if err.raw_os_error() == Some(libc::EINTR) {
                    continue;
                }
                return Err(err);
            }
            // The stop request wins over pending data: those bytes stay in the
            // kernel buffer for the next reader.
            if fds[0].revents != 0 {
                return Ok(ReadOutcome::Stopped);
            }
            if misreported {
                std::thread::sleep(MISREPORT_BACKOFF);
            }
            // Whatever `poll` said (or failed to say) about the master, try a
            // non-blocking read: it decides between data, EOF and "nothing
            // yet", and a timed-out poll still gets its retry.
            let n = unsafe { libc::read(self.fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
            if n > 0 {
                return Ok(ReadOutcome::Data(n as usize));
            }
            if n == 0 {
                return Ok(ReadOutcome::Eof);
            }
            let err = io::Error::last_os_error();
            match err.raw_os_error() {
                Some(libc::EINTR) => continue,
                Some(libc::EAGAIN) => {
                    misreported = fds[1].revents != 0;
                    continue;
                }
                // Linux reports a closed slave as EIO.
                Some(libc::EIO) => return Ok(ReadOutcome::Eof),
                _ => return Err(err),
            }
        }
    }
}

/// Wakes a blocked [`PtyReader`] so it returns [`ReadOutcome::Stopped`].
pub(crate) struct ReaderStop {
    wake: OwnedFd,
}

impl ReaderStop {
    pub(crate) fn stop(&self) {
        let byte = 1u8;
        // The write end is non-blocking: a full pipe already holds a pending
        // wake, so there is nothing more to say.
        let _ = unsafe { libc::write(self.wake.as_raw_fd(), (&byte as *const u8).cast(), 1) };
    }
}

/// Writes to the pty master. Unlike portable-pty's writer, dropping it does
/// not send an EOT, so a shell survives the old image letting go of it.
pub(crate) struct PtyWriter(OwnedFd);

impl Write for PtyWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let started = std::time::Instant::now();
        loop {
            let n = unsafe { libc::write(self.0.as_raw_fd(), buf.as_ptr().cast(), buf.len()) };
            if n >= 0 {
                return Ok(n as usize);
            }
            let err = io::Error::last_os_error();
            match err.raw_os_error() {
                Some(libc::EINTR) => continue,
                Some(libc::EAGAIN) => {
                    // A shell that stops reading input (or a frozen terminal
                    // whose output is not being drained) must not wedge the
                    // caller — who holds the terminal's lock — forever.
                    if started.elapsed() > WRITE_STALL_LIMIT {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "terminal input is not being consumed",
                        ));
                    }
                    let mut fd = libc::pollfd {
                        fd: self.0.as_raw_fd(),
                        events: libc::POLLOUT,
                        revents: 0,
                    };
                    // Bounded: poll on a pty master is unreliable on macOS.
                    unsafe { libc::poll(&mut fd, 1, POLL_INTERVAL_MS) };
                }
                _ => return Err(err),
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Signals a shell by pid, the way portable-pty's own killer does (SIGHUP).
#[derive(Debug, Clone, Copy)]
pub(crate) struct PidKiller(pub(crate) libc::pid_t);

impl portable_pty::ChildKiller for PidKiller {
    fn kill(&mut self) -> io::Result<()> {
        // kill(0, ..) and kill(-1, ..) would signal a whole group or everything.
        if self.0 <= 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "refusing to signal a non-process pid",
            ));
        }
        if unsafe { libc::kill(self.0, libc::SIGHUP) } == 0 {
            return Ok(());
        }
        let err = io::Error::last_os_error();
        // Already gone (and reaped) is the outcome the caller wanted.
        if err.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            Err(err)
        }
    }

    fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
        Box::new(*self)
    }
}

/// Blocks until `pid` has exited, WITHOUT reaping it.
fn wait_exited_no_reap(pid: libc::pid_t) -> io::Result<()> {
    loop {
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let rc = unsafe {
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOWAIT,
            )
        };
        if rc == 0 {
            return Ok(());
        }
        let err = io::Error::last_os_error();
        if err.raw_os_error() != Some(libc::EINTR) {
            return Err(err);
        }
    }
}

/// Who may reap a shell's exit status: see [`ReapGate`], shared with the
/// agent-run handoff so both freeze windows behave the same way.
pub(crate) use zeron_harness::handoff::ReapGate;

fn reap(pid: libc::pid_t) -> io::Result<portable_pty::ExitStatus> {
    let mut status = 0;
    loop {
        let rc = unsafe { libc::waitpid(pid, &mut status, 0) };
        if rc == pid {
            break;
        }
        let err = io::Error::last_os_error();
        if rc < 0 && err.raw_os_error() != Some(libc::EINTR) {
            return Err(err);
        }
    }
    let code = if libc::WIFEXITED(status) {
        libc::WEXITSTATUS(status) as u32
    } else if libc::WIFSIGNALED(status) {
        128 + libc::WTERMSIG(status) as u32
    } else {
        1
    };
    Ok(portable_pty::ExitStatus::with_exit_code(code))
}

/// Wait for `pid` and reap it, unless `gate` is held. The blocking phase
/// leaves the zombie in place (see the module docs), so a waiter that dies
/// with the process image at an exec handoff never steals the exit status
/// from the next image; the gate closes the window between a freeze and that
/// exec. The one window this cannot close: a waiter that dies between
/// `waitpid` reaping the child and the status being delivered to the caller
/// loses that status.
pub(crate) fn wait_pid_gated(
    pid: libc::pid_t,
    gate: &ReapGate,
) -> io::Result<portable_pty::ExitStatus> {
    wait_exited_no_reap(pid)?;
    gate.begin_reap();
    reap(pid)
}

/// [`wait_pid_gated`] with a gate nobody holds.
pub(crate) fn wait_pid_blocking(pid: libc::pid_t) -> io::Result<portable_pty::ExitStatus> {
    wait_pid_gated(pid, &ReapGate::new())
}

// These tests reap their children by pid (the code under test), not through
// `std::process::Child`, so clippy's zombie check does not apply.
#[cfg(test)]
#[allow(clippy::zombie_processes)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn pty_pair() -> (PtyMaster, OwnedFd) {
        let (mut master, mut slave) = (0, 0);
        let rc = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(rc, 0, "openpty: {}", io::Error::last_os_error());
        unsafe {
            (
                PtyMaster {
                    fd: OwnedFd::from_raw_fd(master),
                },
                OwnedFd::from_raw_fd(slave),
            )
        }
    }

    fn write_fd(fd: &OwnedFd, bytes: &[u8]) {
        let n = unsafe { libc::write(fd.as_raw_fd(), bytes.as_ptr().cast(), bytes.len()) };
        assert_eq!(n, bytes.len() as isize);
    }

    fn set_raw(fd: &OwnedFd) {
        let mut t: libc::termios = unsafe { std::mem::zeroed() };
        assert_eq!(unsafe { libc::tcgetattr(fd.as_raw_fd(), &mut t) }, 0);
        unsafe { libc::cfmakeraw(&mut t) };
        assert_eq!(
            unsafe { libc::tcsetattr(fd.as_raw_fd(), libc::TCSANOW, &t) },
            0
        );
    }

    fn set_nonblocking(fd: &OwnedFd) {
        let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) };
        assert!(flags >= 0);
        assert_eq!(
            unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) },
            0
        );
    }

    #[test]
    fn reader_returns_data_then_eof_when_the_slave_closes() {
        let (master, slave) = pty_pair();
        let (mut reader, _stop) = master.reader().unwrap();
        write_fd(&slave, b"hi\n");
        let mut buf = [0u8; 64];
        assert!(matches!(
            reader.read_chunk(&mut buf).unwrap(),
            ReadOutcome::Data(n) if n >= 2
        ));
        drop(slave);
        // EIO after the slave closes is EOF, not an error. The line
        // discipline may still hand out the echoed "\r\n" first.
        loop {
            match reader.read_chunk(&mut buf).unwrap() {
                ReadOutcome::Data(_) => continue,
                ReadOutcome::Eof => break,
                ReadOutcome::Stopped => panic!("nobody stopped this reader"),
            }
        }
    }

    #[test]
    fn stop_wakes_a_blocked_read_without_consuming_bytes() {
        let (master, slave) = pty_pair();
        let (mut reader, stop) = master.reader().unwrap();
        let blocked = std::thread::spawn(move || {
            let mut buf = [0u8; 8];
            reader.read_chunk(&mut buf).unwrap()
        });
        std::thread::sleep(Duration::from_millis(100));
        stop.stop();
        assert!(matches!(blocked.join().unwrap(), ReadOutcome::Stopped));
        // A byte written after the stop is still in the kernel buffer for the
        // next reader — that is what lets a frozen terminal lose nothing.
        write_fd(&slave, b"x");
        let (mut again, _stop) = master.reader().unwrap();
        let mut buf = [0u8; 8];
        assert!(matches!(
            again.read_chunk(&mut buf).unwrap(),
            ReadOutcome::Data(1)
        ));
    }

    #[test]
    fn stop_with_data_already_pending_leaves_the_bytes_for_the_next_reader() {
        // The freeze scenario: output is waiting in the kernel when the
        // reader is told to stop. Stop wins, and nothing is consumed.
        let (master, slave) = pty_pair();
        set_raw(&slave);
        let (mut reader, stop) = master.reader().unwrap();
        write_fd(&slave, b"pending");
        stop.stop();
        let mut buf = [0u8; 16];
        assert_eq!(reader.read_chunk(&mut buf).unwrap(), ReadOutcome::Stopped);
        // Once stopped, always stopped.
        assert_eq!(reader.read_chunk(&mut buf).unwrap(), ReadOutcome::Stopped);
        let (mut next, _keep) = master.reader().unwrap();
        assert_eq!(next.read_chunk(&mut buf).unwrap(), ReadOutcome::Data(7));
        assert_eq!(&buf[..7], b"pending");
    }

    #[test]
    fn dropping_the_stop_handle_stops_the_reader() {
        let (master, _slave) = pty_pair();
        let (mut reader, stop) = master.reader().unwrap();
        drop(stop);
        let mut buf = [0u8; 8];
        assert_eq!(reader.read_chunk(&mut buf).unwrap(), ReadOutcome::Stopped);
    }

    #[test]
    fn repeated_stops_never_block() {
        let (master, _slave) = pty_pair();
        let (_reader, stop) = master.reader().unwrap();
        // Far more than a pipe buffer holds.
        for _ in 0..200_000 {
            stop.stop();
        }
    }

    #[test]
    fn pid_killer_refuses_process_group_and_broadcast_pids() {
        use portable_pty::ChildKiller as _;
        for pid in [0, 1, -1] {
            assert!(PidKiller(pid).kill().is_err(), "pid {pid}");
        }
    }

    #[test]
    fn dropping_the_writer_does_not_send_an_eot() {
        let (master, slave) = pty_pair();
        set_raw(&slave);
        drop(master.writer().unwrap());
        set_nonblocking(&slave);
        let mut buf = [0u8; 8];
        let n = unsafe { libc::read(slave.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
        assert_eq!(n, -1, "nothing was written to the slave");
        assert_eq!(
            io::Error::last_os_error().raw_os_error(),
            Some(libc::EAGAIN)
        );
    }

    #[test]
    fn writer_delivers_bytes_to_the_slave() {
        let (master, slave) = pty_pair();
        set_raw(&slave);
        let mut writer = master.writer().unwrap();
        writer.write_all(b"ls\n").unwrap();
        let mut buf = [0u8; 8];
        let n = unsafe { libc::read(slave.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
        assert_eq!(&buf[..n as usize], b"ls\n");
    }

    #[test]
    fn wait_pid_reports_the_exit_code_of_a_child_that_already_exited() {
        let child = std::process::Command::new("sh")
            .args(["-c", "exit 7"])
            .spawn()
            .unwrap();
        let pid = child.id() as libc::pid_t;
        std::thread::sleep(Duration::from_millis(200)); // it is a zombie now
        assert_eq!(wait_pid_blocking(pid).unwrap().exit_code(), 7);
    }

    #[test]
    fn wait_pid_reports_death_by_signal_as_128_plus_the_signal() {
        let child = std::process::Command::new("sh")
            .args(["-c", "kill -9 $$"])
            .spawn()
            .unwrap();
        let pid = child.id() as libc::pid_t;
        assert_eq!(wait_pid_blocking(pid).unwrap().exit_code(), 128 + 9);
    }

    #[test]
    fn the_blocking_phase_leaves_the_zombie_for_the_next_reaper() {
        let child = std::process::Command::new("sh")
            .args(["-c", "exit 3"])
            .spawn()
            .unwrap();
        let pid = child.id() as libc::pid_t;
        wait_exited_no_reap(pid).unwrap();
        // Not reaped: the status is still there for whoever waits next (the
        // next engine image, after an exec handoff).
        let mut status = 0;
        let rc = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
        assert_eq!(rc, pid);
        assert_eq!(libc::WEXITSTATUS(status), 3);
    }

    #[test]
    fn a_held_gate_keeps_the_zombie_until_released() {
        let child = std::process::Command::new("sh")
            .args(["-c", "exit 4"])
            .spawn()
            .unwrap();
        let pid = child.id() as libc::pid_t;
        let gate = ReapGate::new();
        assert!(gate.hold());
        let waiter = {
            let gate = gate.clone();
            std::thread::spawn(move || wait_pid_gated(pid, &gate))
        };
        // Wait (bounded) until the child has actually exited; only then is
        // "the waiter is still parked" meaningful.
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
            // Still a zombie with its status intact: reapable by a successor.
            if rc == 0 && unsafe { info.si_pid() } == pid {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "child never exited");
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(100));
        assert!(!waiter.is_finished(), "the waiter is parked at the gate");
        gate.release();
        assert_eq!(waiter.join().unwrap().unwrap().exit_code(), 4);
    }

    #[test]
    fn hold_fails_once_the_waiter_has_claimed_the_reap() {
        let child = std::process::Command::new("sh")
            .args(["-c", "exit 0"])
            .spawn()
            .unwrap();
        let gate = ReapGate::new();
        wait_pid_gated(child.id() as libc::pid_t, &gate).unwrap();
        assert!(!gate.hold(), "the shell is gone: nothing left to hold");
    }

    #[test]
    fn set_inheritable_toggles_close_on_exec() {
        let (master, _slave) = pty_pair();
        let fd = master.as_raw_fd();
        set_inheritable(fd, true).unwrap();
        assert_eq!(
            unsafe { libc::fcntl(fd, libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
        set_inheritable(fd, false).unwrap();
        assert_ne!(
            unsafe { libc::fcntl(fd, libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
    }

    #[test]
    fn resize_round_trips() {
        let (master, _slave) = pty_pair();
        master
            .resize(PtySize {
                rows: 33,
                cols: 101,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        assert_eq!(master.size().unwrap(), (101, 33));
    }

    #[test]
    fn duplicated_masters_are_close_on_exec() {
        let (master, _slave) = pty_pair();
        let dup = PtyMaster::from_raw_dup(master.as_raw_fd()).unwrap();
        let flags = unsafe { libc::fcntl(dup.as_raw_fd(), libc::F_GETFD) };
        assert_ne!(flags & libc::FD_CLOEXEC, 0);
    }

    #[test]
    fn pid_killer_treats_a_missing_process_as_success() {
        use portable_pty::ChildKiller as _;
        let child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id() as libc::pid_t;
        let mut child = child;
        child.wait().unwrap();
        assert!(PidKiller(pid).kill().is_ok());
    }
}
