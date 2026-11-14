//! Reading a small file that someone else may have put in place: a config
//! file in a cloned repository, a CLI's cache. A path checked first and
//! read after can be a symlink to `/dev/zero` (read forever) or a FIFO
//! (open blocks until a writer comes), or be swapped for one in between, so
//! the checks are made on the open handle and the read itself is bounded.

use std::io::Read;
use std::path::Path;

/// `path`'s bytes when it is a regular file of at most `max` bytes (a
/// symlink to one is followed). A FIFO or device is refused without
/// blocking, and a file over `max` (or one growing past it as it is read)
/// is refused once `max + 1` bytes are in: `InvalidInput` and
/// `FileTooLarge` errors respectively; a missing file is `NotFound`.
pub fn read(path: &Path, max: u64) -> std::io::Result<Vec<u8>> {
    let (file, len) = open(path, max)?;
    let mut bytes = Vec::with_capacity(len as usize);
    file.take(max.saturating_add(1)).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        return Err(too_large());
    }
    Ok(bytes)
}

/// [`read`] as UTF-8 text.
pub fn read_to_string(path: &Path, max: u64) -> std::io::Result<String> {
    String::from_utf8(read(path, max)?)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))
}

/// `path` opened for a streamed read, refused as [`read`] refuses it, with
/// its length when opened. Read it through `take(max + 1)` (or check what
/// came in) if it may grow meanwhile.
pub fn open(path: &Path, max: u64) -> std::io::Result<(std::fs::File, u64)> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // A FIFO opened for reading blocks until a writer appears; a
        // terminal would become the controlling one.
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY);
    }
    let file = options.open(path)?;
    // What was opened, not what the path named a moment before.
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    if meta.len() > max {
        return Err(too_large());
    }
    // (O_NONBLOCK changes nothing for a regular file's reads.)
    Ok((file, meta.len()))
}

fn too_large() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::FileTooLarge, "file too large")
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Runs `read` on another thread, failing the test if it hangs.
    fn read_promptly(path: &Path, max: u64) -> std::io::Result<Vec<u8>> {
        let path = path.to_path_buf();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(read(&path, max));
        });
        rx.recv_timeout(Duration::from_secs(5))
            .expect("the read hung")
    }

    #[test]
    fn a_regular_file_within_the_limit_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.json");
        std::fs::write(&path, b"{}").unwrap();
        assert_eq!(read(&path, 2).unwrap(), b"{}");
        assert_eq!(
            read(&path, 1).unwrap_err().kind(),
            std::io::ErrorKind::FileTooLarge
        );
        // A symlink to a regular file still reads.
        let link = dir.path().join("link.json");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert_eq!(read(&link, 16).unwrap(), b"{}");
        assert_eq!(
            read(&dir.path().join("missing"), 16).unwrap_err().kind(),
            std::io::ErrorKind::NotFound
        );
    }

    #[test]
    fn a_symlink_to_dev_zero_is_refused_promptly() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join(".mcp.json");
        std::os::unix::fs::symlink("/dev/zero", &link).unwrap();
        let started = Instant::now();
        let err = read_promptly(&link, 1024 * 1024).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn a_fifo_is_refused_without_waiting_for_a_writer() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("settings.json");
        let c_path = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: a valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
        let err = read_promptly(&fifo, 1024).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }
}
