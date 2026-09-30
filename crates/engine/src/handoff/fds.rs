//! File-descriptor helpers for a handoff: which fds survive an `execve`.

use std::io;
use std::os::fd::{FromRawFd, OwnedFd, RawFd};

/// Whether `fd` survives an `execve` (clears or sets `FD_CLOEXEC`). A handoff
/// makes exactly the descriptors it carries inheritable, as late as possible.
pub fn set_inheritable(fd: RawFd, inheritable: bool) -> io::Result<()> {
    crate::terminals::pty_unix::set_inheritable(fd, inheritable)
}

/// Whether `fd` is close-on-exec.
pub fn is_cloexec(fd: RawFd) -> bool {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    flags >= 0 && flags & libc::FD_CLOEXEC != 0
}

/// Take ownership of an inherited fd: it must be open, and it is made
/// close-on-exec again so later child processes do not inherit it.
///
/// The caller must be the sole owner of `fd` (it came from the predecessor).
pub fn adopt_fd(fd: RawFd) -> io::Result<OwnedFd> {
    if fd < 3 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("refusing to adopt standard descriptor {fd}"),
        ));
    }
    if unsafe { libc::fcntl(fd, libc::F_GETFD) } < 0 {
        return Err(io::Error::last_os_error());
    }
    set_inheritable(fd, false)?;
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Whether `fd` is an open socket in the listening state.
pub(crate) fn is_listening_socket(fd: RawFd) -> bool {
    if fd < 3 {
        return false;
    }
    let mut listening: libc::c_int = 0;
    let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
    let rc = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_ACCEPTCONN,
            (&mut listening as *mut libc::c_int).cast(),
            &mut len,
        )
    };
    rc == 0 && listening != 0
}

/// Whether `fd` is an open pipe or socket (an agent's stdio).
pub(crate) fn is_pipe_like(fd: RawFd) -> bool {
    if fd < 3 {
        return false;
    }
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    let statted = unsafe { libc::fstat(fd, &mut stat) } == 0;
    let kind = stat.st_mode & libc::S_IFMT;
    statted && (kind == libc::S_IFIFO || kind == libc::S_IFSOCK)
}

/// Whether `fd` is an open regular file (the lock file, the manifest).
pub(crate) fn is_regular_file(fd: RawFd) -> bool {
    if fd < 3 {
        return false;
    }
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    let statted = unsafe { libc::fstat(fd, &mut stat) } == 0;
    statted && (stat.st_mode & libc::S_IFMT) == libc::S_IFREG
}

/// A tokio listener on a dup of the predecessor's listening socket. The
/// inherited descriptor is left open: a failed adoption hands it back.
///
/// Must run inside a tokio runtime.
pub fn listener_from_inherited(fd: RawFd) -> io::Result<tokio::net::TcpListener> {
    if fd < 3 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("inherited listener descriptor {fd} is not valid"),
        ));
    }
    if !is_listening_socket(fd) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("inherited descriptor {fd} is not a listening socket"),
        ));
    }
    let dup = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
    if dup < 0 {
        return Err(io::Error::last_os_error());
    }
    let listener = unsafe { std::net::TcpListener::from_raw_fd(dup) };
    listener.set_nonblocking(true)?;
    tokio::net::TcpListener::from_std(listener)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::{AsRawFd, IntoRawFd};

    #[tokio::test]
    async fn an_inherited_listener_keeps_accepting_and_the_original_stays_open() {
        let original = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = original.local_addr().unwrap().port();
        let raw = original.as_raw_fd();
        let adopted = listener_from_inherited(raw).unwrap();
        assert_eq!(adopted.local_addr().unwrap().port(), port);
        assert!(
            is_cloexec(adopted.as_raw_fd()),
            "the dup does not leak to children"
        );
        // A client that connects while nobody is accepting is queued, then served.
        let client = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        let (accepted, _) = adopted.accept().await.unwrap();
        drop((client, accepted));
        assert!(
            unsafe { libc::fcntl(raw, libc::F_GETFD) } >= 0,
            "the original is untouched"
        );
    }

    #[tokio::test]
    async fn only_a_listening_socket_is_accepted_as_the_ipc_listener() {
        let (a, _b) = std::os::unix::net::UnixStream::pair().unwrap();
        assert!(
            listener_from_inherited(a.as_raw_fd()).is_err(),
            "connected, not listening"
        );
        let plain = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        drop(plain);
        for fd in [0, 1, 2, -1, 9999] {
            assert!(listener_from_inherited(fd).is_err(), "fd {fd}");
        }
    }

    #[test]
    fn adopt_fd_restores_cloexec_and_rejects_closed_and_standard_fds() {
        let (a, _b) = std::os::unix::net::UnixStream::pair().unwrap();
        set_inheritable(a.as_raw_fd(), true).unwrap();
        assert!(!is_cloexec(a.as_raw_fd()));
        let raw = a.into_raw_fd();
        let owned = adopt_fd(raw).unwrap();
        assert!(is_cloexec(owned.as_raw_fd()));
        assert!(adopt_fd(9999).is_err(), "not open");
        for fd in [0, 1, 2, -1] {
            assert!(adopt_fd(fd).is_err(), "fd {fd}");
        }
    }
}
