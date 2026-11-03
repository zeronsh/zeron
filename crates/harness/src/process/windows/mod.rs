//! Windows-only native agent launcher. A job-contained inert parent supplies
//! creation-time membership and stdio; no post-creation assignment or local
//! inheritable handles can escape into unrelated subprocess launches.
mod command;
mod escrow;
mod stdio;
#[cfg(test)]
mod tests;
pub use command::Command;
pub use stdio::Stdio;
pub type ChildStdout = tokio::fs::File;

/// The parent's end of a child's stdin pipe. Dropping it is the EOF an agent
/// waiting on its input exits on; the [`Child`] learns of that through
/// `_open` (its receiver sees the sender go).
#[derive(Debug)]
pub struct ChildStdin {
    file: tokio::fs::File,
    _open: tokio::sync::watch::Sender<()>,
}

impl tokio::io::AsyncWrite for ChildStdin {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        std::pin::Pin::new(&mut self.get_mut().file).poll_write(cx, buf)
    }
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut self.get_mut().file).poll_flush(cx)
    }
    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut self.get_mut().file).poll_shutdown(cx)
    }
}

/// How long a teardown waits for a stdin its driver handed to a writer task
/// to actually close: the driver closes it just before the teardown, and the
/// task drops it a moment later.
const STDIN_CLOSE_WAIT: std::time::Duration = std::time::Duration::from_millis(250);

use crate::windows_process::Job;
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::os::windows::process::ExitStatusExt;
use std::process::{ExitStatus, Output};
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW,
    EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess, INFINITE, PROCESS_INFORMATION, ResumeThread,
    STARTF_USESTDHANDLES, STARTUPINFOEXW, WaitForSingleObject,
};

#[derive(Debug)]
pub struct Child {
    process: OwnedHandle,
    pid: u32,
    status: Option<ExitStatus>,
    exited: tokio::sync::oneshot::Receiver<()>,
    pub(super) job: Arc<Job>,
    escrow: escrow::Escrow,
    pub stdin: Option<ChildStdin>,
    pub stdout: Option<ChildStdout>,
    pub stderr: Option<tokio::fs::File>,
    /// Open while the stdin pipe's [`ChildStdin`] lives; `None` without a pipe.
    stdin_open: Option<tokio::sync::watch::Receiver<()>>,
    /// The agent does not exit on stdin EOF (see [`Child::ignore_stdin_eof`]).
    ignores_stdin_eof: bool,
}
impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.job.terminate();
    }
}
impl Child {
    pub fn id(&self) -> Option<u32> {
        self.status.is_none().then_some(self.pid)
    }
    /// Mark an agent that does not exit promptly once its stdin closes, so a
    /// teardown terminates it without waiting out a grace for nothing.
    pub fn ignore_stdin_eof(&mut self) {
        self.ignores_stdin_eof = true;
    }
    /// Whether a teardown's wait for this child to exit on stdin EOF can pay
    /// off: it reads a pipe, the pipe is closed (or [`Self::wait`] closes it,
    /// still holding it), and the agent exits on EOF. A stdin handed to a
    /// writer task gets [`STDIN_CLOSE_WAIT`] to be dropped; one still held
    /// elsewhere means no EOF is coming.
    pub(crate) async fn stdin_eof_coming(&mut self) -> bool {
        if self.ignores_stdin_eof {
            return false;
        }
        if self.stdin.is_some() {
            return true;
        }
        let Some(open) = &mut self.stdin_open else {
            return false;
        };
        // Nothing is ever sent: `changed` ends only once the sender is gone.
        matches!(
            tokio::time::timeout(STDIN_CLOSE_WAIT, open.changed()).await,
            Ok(Err(_))
        )
    }
    pub fn start_kill(&mut self) -> io::Result<()> {
        self.job.terminate()
    }
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        if self.status.is_some() {
            return Ok(self.status);
        }
        match unsafe { WaitForSingleObject(self.process.as_raw_handle(), 0) } {
            WAIT_TIMEOUT => Ok(None),
            WAIT_OBJECT_0 => {
                let mut code = 0;
                if unsafe { GetExitCodeProcess(self.process.as_raw_handle(), &mut code) } == 0 {
                    return Err(io::Error::last_os_error());
                }
                self.status = Some(ExitStatus::from_raw(code));
                let _ = self.job.terminate();
                Ok(self.status)
            }
            _ => Err(io::Error::last_os_error()),
        }
    }
    pub async fn wait(&mut self) -> io::Result<ExitStatus> {
        // Closing stdin allows children waiting for EOF to exit, like Tokio.
        drop(self.stdin.take());
        if let Some(status) = self.try_wait()? {
            return Ok(status);
        }
        (&mut self.exited)
            .await
            .map_err(|_| io::Error::other("agent exit monitor stopped"))?;
        self.try_wait()?
            .ok_or_else(|| io::Error::other("agent exit wait failed"))
    }
    pub async fn wait_with_output(mut self) -> io::Result<Output> {
        drop(self.stdin.take());
        let (mut stdout, mut stderr) = (self.stdout.take(), self.stderr.take());
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let (status, _, _) = tokio::try_join!(
            self.wait(),
            async {
                if let Some(pipe) = &mut stdout {
                    pipe.read_to_end(&mut out).await?;
                }
                Ok::<_, io::Error>(())
            },
            async {
                if let Some(pipe) = &mut stderr {
                    pipe.read_to_end(&mut err).await?;
                }
                Ok::<_, io::Error>(())
            }
        )?;
        Ok(Output {
            status,
            stdout: out,
            stderr: err,
        })
    }
}

impl Command {
    /// Launch the command. Creation that fails while the image is locked
    /// (the malware scanner on a just-updated CLI) is retried with a short
    /// backoff.
    pub fn spawn(&mut self) -> io::Result<Child> {
        let mut delays = super::SPAWN_RETRY_DELAYS.into_iter();
        loop {
            let prepared = self.prepare()?;
            let launched = prepared.launched.clone();
            match launch(self, prepared, |_| Ok(())) {
                Err(error) if super::spawn_retryable(&error, modified_ago(&launched)) => {
                    let Some(delay) = delays.next() else {
                        return Err(error);
                    };
                    tracing::debug!(%error, image = %launched.display(), "retrying agent launch");
                    // Rare and short; callers are synchronous by signature.
                    super::sleep_off_runtime(delay);
                }
                result => return result,
            }
        }
    }

    pub async fn output(&mut self) -> io::Result<Output> {
        self.stdout(Stdio::piped()).stderr(Stdio::piped());
        self.spawn()?.wait_with_output().await
    }
}

// A private hook makes the critical interval testable: the process exists but
// is still suspended, and no monitor has been started. Production uses a no-op.
#[cfg(test)]
fn spawn_prepared(
    command: &Command,
    after_create: impl FnOnce(&Child) -> io::Result<()>,
) -> io::Result<Child> {
    launch(command, command.prepare()?, after_create)
}

fn modified_ago(path: &std::path::Path) -> Option<std::time::Duration> {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
}

fn launch(
    command: &Command,
    mut prepared: command::Prepared,
    after_create: impl FnOnce(&Child) -> io::Result<()>,
) -> io::Result<Child> {
    let job = Arc::new(Job::new()?);
    let escrow = escrow::Escrow::new(&job)?;
    let (input, stdin) = command.stdio[0].open(0)?;
    let (output, stdout) = command.stdio[1].open(1)?;
    let (error, stderr) = command.stdio[2].open(2)?;
    let handles = escrow.duplicate(&[input, output, error])?;
    let parent = [escrow.process.as_raw_handle()];
    let mut attrs = crate::windows_process::Attributes::for_parent(&parent, &handles)?;
    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = handles[0];
    startup.StartupInfo.hStdOutput = handles[1];
    startup.StartupInfo.hStdError = handles[2];
    startup.lpAttributeList = attrs.ptr();
    let mut info = PROCESS_INFORMATION::default();
    // SAFETY: buffers/attribute values and all borrowed handles remain alive
    // through creation. The job handle is excluded from inherited handles.
    if unsafe {
        CreateProcessW(
            prepared.executable.as_ptr(),
            prepared.line.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1,
            CREATE_NO_WINDOW
                | CREATE_SUSPENDED
                | CREATE_UNICODE_ENVIRONMENT
                | EXTENDED_STARTUPINFO_PRESENT,
            prepared.environment.as_ptr().cast(),
            prepared
                .cwd
                .as_ref()
                .map_or(std::ptr::null(), |cwd| cwd.as_ptr()),
            &startup.StartupInfo,
            &mut info,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // Child owns the job immediately: every subsequent fallible setup step is
    // protected by Drop. A hard owner crash is protected by kernel job membership.
    let process = unsafe { OwnedHandle::from_raw_handle(info.hProcess) };
    let thread = unsafe { OwnedHandle::from_raw_handle(info.hThread) };
    let (done, exited) = tokio::sync::oneshot::channel();
    let (stdin, stdin_open) = match stdin {
        Some(file) => {
            let (open, watched) = tokio::sync::watch::channel(());
            (Some(ChildStdin { file, _open: open }), Some(watched))
        }
        None => (None, None),
    };
    let child = Child {
        process,
        pid: info.dwProcessId,
        status: None,
        exited,
        job,
        escrow,
        stdin,
        stdout,
        stderr,
        stdin_open,
        ignores_stdin_eof: false,
    };
    child.escrow.release(&handles)?;
    after_create(&child)?;
    let process = child.process.try_clone()?;
    let cleanup_job = child.job.clone();
    // Do not consume Tokio blocking capacity needed by the child's own pipes.
    std::thread::Builder::new()
        .name("agent-exit".into())
        .spawn(move || {
            unsafe { WaitForSingleObject(process.as_raw_handle(), INFINITE) };
            let _ = cleanup_job.terminate();
            let _ = done.send(());
        })?;
    if unsafe { ResumeThread(thread.as_raw_handle()) } == u32::MAX {
        return Err(io::Error::last_os_error());
    }
    Ok(child)
}
