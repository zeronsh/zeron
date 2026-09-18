//! Uploads — attachment staging on the chat's host device
//! (feature-inventory §3.7 "Uploads"; port of zeron's `uploads.ts`).
//!
//! The UI streams a file as base64 chunks (~60KB, sized for the relay when the
//! target device is remote); chunks stage on disk under `{uploads_root}/tmp/
//! {uploadId}/{seq}.b64` (surviving an engine restart mid-upload, unlike zeron's
//! in-memory buffers), and `commit` assembles them into
//! `{uploads_root}/{id8}-{name}` and returns the absolute path, which the
//! composer appends to the prompt so the agent can read the file from disk.
//! Attachments live only on the host device — every read proxies through the
//! owning device via `ReadAttachmentChunk`; nothing is mirrored to the edge.
//!
//! `read_chunk` serves transcript images back in 45KB base64 chunks. Path jail:
//! only files under the uploads dir or a workspace-known chat cwd are readable
//! (the RPC layer supplies the cwd roots) — and only supported image types, as
//! in zeron.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::Serialize;

use crate::EngineError;

/// A pending upload must finish within this window (covers slow mesh links).
const STAGING_TTL: Duration = Duration::from_secs(10 * 60);

/// Queued-attachment ref scheme (2026-08-19 incident: attachment staging was
/// a blocking pre-step in front of QueueCommand, so a send died with the peer
/// link instead of queueing). A ref names bytes by identity instead of by a
/// device-local absolute path: `pending://{uploadId}/{fileName}`. The sender
/// commits the bytes to its OWN uploads dir first (fast, offline-safe), the
/// command queues immediately carrying refs, and the bytes chase it over the
/// peer link. Any device resolves a ref against its own uploads dir — the
/// deterministic committed name `{id8}-{sanitize(fileName)}` makes sender and
/// host land the same file at the same relative path.
pub const PENDING_REF_PREFIX: &str = "pending://";

pub fn is_pending_ref(path: &str) -> bool {
    path.starts_with(PENDING_REF_PREFIX)
}

/// `pending://{uploadId}/{fileName}` (fileName is the ORIGINAL name; sanitize
/// happens at resolution so both ends agree by construction).
pub fn pending_ref(upload_id: &str, file_name: &str) -> String {
    format!("{PENDING_REF_PREFIX}{upload_id}/{file_name}")
}

/// Split a pending ref into `(upload_id, file_name)`.
pub fn parse_pending_ref(path: &str) -> Option<(&str, &str)> {
    let rest = path.strip_prefix(PENDING_REF_PREFIX)?;
    let (id, name) = rest.split_once('/')?;
    (!id.is_empty() && !name.is_empty()).then_some((id, name))
}

/// One queued attachment a `QueueCommand` asks the engine to deliver: the
/// bytes are already committed to THIS device's uploads dir under
/// `{id8}-{sanitize(file_name)}`; the transfer pushes them to the chat's
/// host device by upload identity (never by arbitrary path).
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentTransfer {
    pub upload_id: String,
    pub file_name: String,
}

/// Pending refs listed on a text's attachment-ref lines (`- pending://…`).
/// Line-wise so file names with spaces survive intact.
pub fn pending_refs_in(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let path = line.trim_start().strip_prefix("- ")?.trim();
            is_pending_ref(path).then(|| path.to_string())
        })
        .collect()
}
/// Hard cap on an assembled file.
const MAX_BYTES: u64 = 32 * 1024 * 1024;
/// Multiple of 3 so independent base64 chunks concatenate losslessly.
const READ_CHUNK_BYTES: u64 = 45_000;

/// `ReadAttachmentChunk` reply.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentChunk {
    pub name: String,
    pub mime_type: String,
    /// Base64 of this chunk's byte range.
    pub data: String,
    pub next_offset: u64,
    pub done: bool,
}

struct UploadsInner {
    /// Profile-scoped durable home for new committed attachments.
    dir: PathBuf,
    /// Chunk staging (`{uploads_root}/tmp/{uploadId}/`).
    tmp: PathBuf,
    /// Historical roots accepted for reads only. Writes and staging never use
    /// them. RwLock: a local-profile import adds its source root at runtime so
    /// imported transcripts resolve without an engine restart.
    read_only_roots: std::sync::RwLock<Vec<PathBuf>>,
}

#[derive(Clone)]
pub struct Uploads {
    inner: Arc<UploadsInner>,
}

impl Uploads {
    /// Use the historical device-global uploads directory.
    pub fn new(data_dir: &Path) -> Self {
        Self::from_root(&data_dir.join("uploads"))
    }

    /// Use an already-resolved profile uploads directory.
    pub fn from_root(dir: &Path) -> Self {
        Self::from_root_with_fallback(dir, None)
    }

    /// Use a profile root for all writes and an optional legacy read-only root.
    pub fn from_root_with_fallback(dir: &Path, legacy_read_root: Option<&Path>) -> Self {
        Self {
            inner: Arc::new(UploadsInner {
                tmp: dir.join("tmp"),
                dir: dir.to_path_buf(),
                read_only_roots: std::sync::RwLock::new(
                    legacy_read_root
                        .into_iter()
                        .map(Path::to_path_buf)
                        .collect(),
                ),
            }),
        }
    }

    /// The durable uploads dir (a path-jail root).
    pub fn dir(&self) -> &Path {
        &self.inner.dir
    }

    /// Accept `root` for reads from now on (idempotent). Profile import calls
    /// this so transcripts that embed absolute paths under the local profile's
    /// uploads root keep resolving after the switch to a synced profile.
    pub fn add_read_only_root(&self, root: &Path) {
        let mut roots = self
            .inner
            .read_only_roots
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !roots.iter().any(|r| r == root) {
            roots.push(root.to_path_buf());
        }
    }

    /// Stage one base64 chunk. Positional (`seq`) writes are IDEMPOTENT: a client
    /// retrying a chunk whose ack was lost overwrites the same slot instead of
    /// double-appending. Callers without `seq` get append-only behavior.
    pub fn append(&self, upload_id: &str, data: &str, seq: Option<u64>) -> Result<(), EngineError> {
        let dir = self.staging_dir(upload_id)?;
        self.sweep();
        std::fs::create_dir_all(&dir)?;
        let at = match seq {
            Some(seq) => seq,
            None => next_free_seq(&dir)?,
        };
        if at > 1_000_000 {
            return Err(EngineError::Other("Invalid chunk index".into()));
        }
        // Base64 inflates by ~4/3; bound the staged payload against the file cap.
        let staged: u64 = chunk_files(&dir)?
            .iter()
            .filter(|(seq, _)| *seq != at)
            .map(|(_, path)| std::fs::metadata(path).map(|m| m.len()).unwrap_or(0))
            .sum();
        if (staged + data.len() as u64) * 3 / 4 > MAX_BYTES {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(EngineError::Other("Upload too large".into()));
        }
        std::fs::write(dir.join(format!("{at:06}.b64")), data)?;
        Ok(())
    }

    /// Assemble the staged chunks into a durable file and return its absolute
    /// path.
    pub fn commit(&self, upload_id: &str, file_name: &str) -> Result<String, EngineError> {
        let dir = self.staging_dir(upload_id)?;
        let mut parts = chunk_files(&dir)?;
        if parts.is_empty() {
            return Err(EngineError::Other("Unknown or expired upload".into()));
        }
        parts.sort_by_key(|(seq, _)| *seq);
        // Positional appends may leave holes if a chunk never arrived — joining
        // around them would silently corrupt the file.
        let mut joined = String::new();
        for (i, (seq, path)) in parts.iter().enumerate() {
            if *seq != i as u64 {
                return Err(EngineError::Other("Upload is missing a chunk".into()));
            }
            joined.push_str(std::fs::read_to_string(path)?.trim());
        }
        let bytes = BASE64
            .decode(joined.as_bytes())
            .map_err(|e| EngineError::Other(format!("upload is not valid base64: {e}")))?;
        if bytes.len() as u64 > MAX_BYTES {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(EngineError::Other("Upload too large".into()));
        }
        std::fs::create_dir_all(&self.inner.dir)?;
        let name = sanitize(file_name);
        let id8: String = upload_id.chars().take(8).collect();
        let path = self.inner.dir.join(format!("{id8}-{name}"));
        std::fs::write(&path, &bytes)?;
        let _ = std::fs::remove_dir_all(&dir);
        Ok(path.to_string_lossy().to_string())
    }

    /// The committed absolute path a pending ref resolves to on THIS device
    /// (`{uploads_dir}/{id8}-{sanitize(name)}`), whether or not it exists yet.
    pub fn pending_target(&self, upload_id: &str, file_name: &str) -> PathBuf {
        let id8: String = upload_id.chars().take(8).collect();
        // The id8 fragment becomes part of a file name — jail its charset the
        // same way staging does (a hostile ref must not traverse).
        let id8 = sanitize(&id8);
        self.inner
            .dir
            .join(format!("{id8}-{}", sanitize(file_name)))
    }

    /// Resolve a `pending://` ref against this device's uploads dir.
    /// `Some(absolute)` iff the bytes have landed here.
    pub fn resolve_pending(&self, path: &str) -> Option<String> {
        let (upload_id, file_name) = parse_pending_ref(path)?;
        let target = self.pending_target(upload_id, file_name);
        target
            .is_file()
            .then(|| target.to_string_lossy().to_string())
    }

    /// Read one 45KB chunk of an attachment. `extra_roots` are the workspace's
    /// known chat cwds — together with the uploads dir they form the path jail.
    pub fn read_chunk(
        &self,
        path: &str,
        offset: u64,
        extra_roots: &[PathBuf],
    ) -> Result<AttachmentChunk, EngineError> {
        use std::io::{Read, Seek};
        let file = self.inspect(path, extra_roots)?;
        let size = file.size;
        let start = offset.min(size);
        let next_offset = (start + READ_CHUNK_BYTES).min(size);
        // Read ONLY this chunk's byte range — never the whole file per chunk.
        let mut buf = vec![0u8; (next_offset - start) as usize];
        let mut handle = std::fs::File::open(&file.resolved)?;
        handle.seek(std::io::SeekFrom::Start(start))?;
        let mut read = 0usize;
        while read < buf.len() {
            let n = handle.read(&mut buf[read..])?;
            if n == 0 {
                break;
            }
            read += n;
        }
        buf.truncate(read);
        Ok(AttachmentChunk {
            name: file.name,
            mime_type: file.mime_type,
            data: BASE64.encode(&buf),
            next_offset,
            done: next_offset >= size,
        })
    }

    // ── internals ───────────────────────────────────────────────────────────

    fn staging_dir(&self, upload_id: &str) -> Result<PathBuf, EngineError> {
        // The id becomes a directory name — jail it to a safe charset.
        let ok = !upload_id.is_empty()
            && upload_id.len() <= 64
            && upload_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'));
        if !ok {
            return Err(EngineError::Other("Invalid upload id".into()));
        }
        Ok(self.inner.tmp.join(upload_id))
    }

    /// Reclaim staging dirs whose newest chunk is older than the TTL (an upload
    /// abandoned mid-stream must not hold up to 32MB forever).
    fn sweep(&self) {
        let Ok(entries) = std::fs::read_dir(&self.inner.tmp) else {
            return;
        };
        for entry in entries.flatten() {
            let newest = std::fs::read_dir(entry.path())
                .ok()
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|f| f.metadata().ok()?.modified().ok())
                .max();
            // An empty dir is NOT free to reclaim: `append` creates the dir
            // before writing the first chunk, and parallel chunk uploads run
            // 3-wide — a sibling's sweep landing in that window deleted the
            // dir out from under the first write (v0.2.12 "Couldn't stage the
            // attachment locally"). Judge an empty dir by its own age.
            let newest = newest.or_else(|| entry.metadata().ok()?.modified().ok());
            let expired = match newest {
                Some(at) => at.elapsed().map(|age| age > STAGING_TTL).unwrap_or(false),
                None => false,
            };
            if expired {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }

    fn inspect(&self, path: &str, extra_roots: &[PathBuf]) -> Result<InspectedFile, EngineError> {
        let outside = || EngineError::Other("Attachment is outside the upload cache".into());
        // Queued-attachment refs resolve against this device's uploads dir
        // (present only once the transfer landed) — then jail as usual.
        let resolved_ref;
        let path = if is_pending_ref(path) {
            resolved_ref = self.resolve_pending(path).ok_or_else(outside)?;
            resolved_ref.as_str()
        } else {
            path
        };
        // Canonicalize BOTH sides so `..` segments and symlinks can't escape.
        let resolved = std::fs::canonicalize(path).map_err(|_| outside())?;
        let read_roots = self
            .inner
            .read_only_roots
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let allowed = std::iter::once(&self.inner.dir)
            .chain(read_roots.iter())
            .chain(extra_roots.iter())
            .filter_map(|root| std::fs::canonicalize(root).ok())
            .any(|root| resolved.starts_with(&root) && resolved != root);
        if !allowed {
            return Err(outside());
        }
        let meta = std::fs::metadata(&resolved)?;
        if !meta.is_file() {
            return Err(EngineError::Other("Attachment is not a file".into()));
        }
        if meta.len() > MAX_BYTES {
            return Err(EngineError::Other("Attachment is too large".into()));
        }
        let mime_type = mime_by_ext(&resolved)
            .ok_or_else(|| EngineError::Other("Attachment is not a supported image".into()))?;
        Ok(InspectedFile {
            name: resolved
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "attachment".into()),
            mime_type: mime_type.to_string(),
            size: meta.len(),
            resolved,
        })
    }
}

struct InspectedFile {
    resolved: PathBuf,
    name: String,
    mime_type: String,
    size: u64,
}

fn chunk_files(dir: &Path) -> Result<Vec<(u64, PathBuf)>, EngineError> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(Vec::new());
    };
    let mut files = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let seq = path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.parse::<u64>().ok());
        if let Some(seq) = seq
            && path.extension().and_then(|e| e.to_str()) == Some("b64")
        {
            files.push((seq, path));
        }
    }
    Ok(files)
}

fn next_free_seq(dir: &Path) -> Result<u64, EngineError> {
    Ok(chunk_files(dir)?
        .iter()
        .map(|(seq, _)| seq + 1)
        .max()
        .unwrap_or(0))
}

fn sanitize(file_name: &str) -> String {
    let base = Path::new(file_name)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let cleaned: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let tail: String = cleaned
        .chars()
        .rev()
        .take(80)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if tail.is_empty() {
        "upload".into()
    } else {
        tail
    }
}

fn mime_by_ext(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "svg" => Some("image/svg+xml"),
        "bmp" => Some("image/bmp"),
        "tif" | "tiff" => Some("image/tiff"),
        "avif" => Some("image/avif"),
        "heic" => Some("image/heic"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_names() {
        assert_eq!(sanitize("../../etc/passwd"), "passwd");
        assert_eq!(sanitize("my photo (1).png"), "my_photo__1_.png");
        assert_eq!(sanitize(""), "upload");
    }

    #[test]
    fn sweep_spares_a_fresh_empty_staging_dir() {
        // The parallel-chunk race: uploader A has created its staging dir but
        // not yet written chunk 0 when uploader B's sweep runs. The empty dir
        // must survive; only an ABANDONED empty dir (older than the TTL) goes.
        let dir = tempfile::tempdir().unwrap();
        let uploads = Uploads::from_root(dir.path());
        let racing = dir.path().join("tmp").join("upload-racing");
        std::fs::create_dir_all(&racing).unwrap();

        uploads.append("upload-other", "aGk=", Some(0)).unwrap();
        assert!(racing.exists(), "fresh empty staging dir was reclaimed");

        let stale = std::time::SystemTime::now() - (STAGING_TTL + Duration::from_secs(60));
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // A directory handle needs backup semantics; setting its timestamp
            // needs FILE_WRITE_ATTRIBUTES rather than write access to its data.
            options.custom_flags(0x02000000).access_mode(0x100);
        }
        options.open(&racing).unwrap().set_modified(stale).unwrap();
        uploads.append("upload-other", "aGk=", Some(0)).unwrap();
        assert!(
            !racing.exists(),
            "abandoned empty staging dir must be swept"
        );
    }

    #[test]
    fn commit_assembles_chunks_into_a_durable_file() {
        let dir = tempfile::tempdir().unwrap();
        let uploads = Uploads::from_root(dir.path());
        uploads
            .append("upload-1", &BASE64.encode(b"local"), Some(0))
            .unwrap();

        let path = uploads.commit("upload-1", "image.png").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"local");
        assert!(!dir.path().join("tmp").join("upload-1").exists());
    }

    #[test]
    fn pending_refs_round_trip_and_resolve_after_commit() {
        assert_eq!(
            pending_ref("abcd1234-rest", "my photo (1).png"),
            "pending://abcd1234-rest/my photo (1).png"
        );
        assert_eq!(
            parse_pending_ref("pending://abcd1234-rest/my photo (1).png"),
            Some(("abcd1234-rest", "my photo (1).png"))
        );
        assert_eq!(parse_pending_ref("pending://no-slash"), None);
        assert_eq!(parse_pending_ref("/tmp/plain.png"), None);

        let dir = tempfile::tempdir().unwrap();
        let uploads = Uploads::from_root(dir.path());
        let r = pending_ref("abcd1234-rest", "my photo (1).png");
        // Not landed yet.
        assert_eq!(uploads.resolve_pending(&r), None);
        // Commit under the SAME identity → the ref resolves to the committed
        // path (the invariant sender and host both rely on).
        uploads
            .append("abcd1234-rest", &BASE64.encode(b"img"), Some(0))
            .unwrap();
        let committed = uploads.commit("abcd1234-rest", "my photo (1).png").unwrap();
        assert_eq!(uploads.resolve_pending(&r), Some(committed));
    }

    #[test]
    fn pending_refs_in_scans_ref_lines_only() {
        let text = "See the attached image(s).\n\nAttached images (local files — open them to view):\n- pending://u1/a b.png\n- /abs/other.png\n- pending://u2/c.png";
        assert_eq!(
            pending_refs_in(text),
            vec![
                "pending://u1/a b.png".to_string(),
                "pending://u2/c.png".to_string()
            ]
        );
        assert!(pending_refs_in("mentions pending://u1/x.png inline only").is_empty());
    }

    #[test]
    fn pending_ref_traversal_cannot_escape_the_jail() {
        let dir = tempfile::tempdir().unwrap();
        let uploads = Uploads::from_root(dir.path());
        // Hostile names sanitize into the jail rather than traversing out.
        let target = uploads.pending_target("../../../etc", "../../passwd");
        assert!(target.starts_with(dir.path()));
        assert!(!target.to_string_lossy().contains(".."));
    }
}

/// Imported generated media: only these sanitized fields may leave the engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedImage {
    pub path: String,
    pub name: String,
    pub mime_type: String,
    pub size: u64,
}

const MAX_GENERATED_IMAGE_BYTES: u64 = 24 * 1024 * 1024;

fn raster_signature(bytes: &[u8]) -> Option<(&'static str, &'static str)> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(("png", "image/png"))
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some(("jpg", "image/jpeg"))
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some(("webp", "image/webp"))
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(("gif", "image/gif"))
    } else {
        None
    }
}

fn same_generated_file(a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if a.dev() != b.dev()
            || a.ino() != b.ino()
            || a.ctime() != b.ctime()
            || a.ctime_nsec() != b.ctime_nsec()
        {
            return false;
        }
    }
    a.is_file() && b.is_file() && a.len() == b.len() && a.modified().ok() == b.modified().ok()
}

/// Walk canonical descendants relative to a pinned directory descriptor. A
/// concurrent directory/symlink replacement cannot redirect the source open.
#[cfg(unix)]
fn open_generated_file(
    root: &Path,
    relative: &Path,
) -> std::io::Result<(std::fs::File, Vec<std::fs::File>)> {
    use std::os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, fs::OpenOptionsExt},
    };
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(root)?;
    let components: Vec<_> = relative.components().collect();
    for (i, component) in components.iter().enumerate() {
        let std::path::Component::Normal(name) = component else {
            return Err(std::io::Error::other("Invalid generated image path"));
        };
        let name = std::ffi::CString::new(name.as_bytes())?;
        let flags = libc::O_RDONLY
            | libc::O_CLOEXEC
            | libc::O_NOFOLLOW
            | libc::O_NONBLOCK
            | if i + 1 < components.len() {
                libc::O_DIRECTORY
            } else {
                0
            };
        // SAFETY: a live directory fd and a NUL-terminated component; ownership
        // of the returned fd transfers exactly once into File.
        let fd = unsafe { libc::openat(file.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        file = unsafe { std::fs::File::from_raw_fd(fd) };
    }
    Ok((file, Vec::new()))
}

#[cfg(not(any(unix, windows)))]
fn open_generated_file(
    root: &Path,
    relative: &Path,
) -> std::io::Result<(std::fs::File, Vec<std::fs::File>)> {
    Ok((std::fs::File::open(root.join(relative))?, Vec::new()))
}

// Windows has no openat equivalent in std. Hold every traversed directory
// without delete sharing, reject reparse points, and deny write/delete sharing
// on the source until publication. This prevents path replacement and writes
// even when NTFS timestamps and file lengths would compare equal.
#[cfg(windows)]
fn open_generated_file(
    root: &Path,
    relative: &Path,
) -> std::io::Result<(std::fs::File, Vec<std::fs::File>)> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    const FILE_SHARE_READ: u32 = 1;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x02000000;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x00200000;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    let invalid = || std::io::Error::other("Invalid generated image path");
    let open_directory = |path: &Path| -> std::io::Result<std::fs::File> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(invalid());
        }
        Ok(file)
    };
    let mut parents = vec![open_directory(root)?];
    let mut path = root.to_path_buf();
    let components: Vec<_> = relative.components().collect();
    if components.is_empty() {
        return Err(invalid());
    }
    for (index, component) in components.iter().enumerate() {
        let std::path::Component::Normal(name) = component else {
            return Err(invalid());
        };
        path.push(name);
        if index + 1 < components.len() {
            parents.push(open_directory(&path)?);
        }
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(&path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(invalid());
    }
    Ok((file, parents))
}

impl Uploads {
    /// Copy a Codex-owned raster into the active profile without exposing the
    /// original path to the journal, document, RPC readers, or relay.
    pub fn import_generated_image(
        &self,
        source: &Path,
        allowed_root: &Path,
        stable_key: &str,
    ) -> Result<ImportedImage, EngineError> {
        self.import_generated_image_after_copy(source, allowed_root, stable_key, || {})
    }

    fn import_generated_image_after_copy(
        &self,
        source: &Path,
        allowed_root: &Path,
        stable_key: &str,
        after_copy: impl FnOnce(),
    ) -> Result<ImportedImage, EngineError> {
        use sha2::{Digest, Sha256};
        use std::io::{Read, Write};
        let invalid = || EngineError::Other("Generated image source is invalid or changed".into());
        if !source.is_absolute() {
            return Err(invalid());
        }
        let root = allowed_root.canonicalize()?;
        let canonical = source.canonicalize()?;
        let relative = canonical.strip_prefix(&root).map_err(|_| invalid())?;
        if relative.as_os_str().is_empty() {
            return Err(invalid());
        }
        let (mut input, _pinned_directories) = open_generated_file(&root, relative)?;
        let before = input.metadata()?;
        if !before.is_file() || before.len() > MAX_GENERATED_IMAGE_BYTES {
            return Err(invalid());
        }
        let mut header = [0u8; 12];
        let count = input.read(&mut header)?;
        let (ext, mime) = raster_signature(&header[..count]).ok_or_else(invalid)?;
        std::fs::create_dir_all(self.dir())?;
        let uploads_root = self.dir().canonicalize()?;
        let name = format!("generated.{ext}");
        let hash = format!("{:x}", Sha256::digest(stable_key.as_bytes()));
        let destination = uploads_root.join(format!("{hash}-{name}"));
        let mut temporary = tempfile::NamedTempFile::new_in(&uploads_root)?;
        temporary.write_all(&header[..count])?;
        let copied = std::io::copy(
            &mut (&mut input).take(MAX_GENERATED_IMAGE_BYTES + 1 - count as u64),
            &mut temporary,
        )? + count as u64;
        after_copy();
        // Check both the open inode and the name: replacement, growth, and
        // same-size writes all invalidate this attempt before publication.
        if copied != before.len()
            || copied > MAX_GENERATED_IMAGE_BYTES
            || !same_generated_file(&before, &input.metadata()?)
            || source.canonicalize()? != canonical
            || !same_generated_file(&before, &std::fs::metadata(&canonical)?)
        {
            return Err(invalid());
        }
        temporary.flush()?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(&destination)
            .map_err(|e| EngineError::Io(e.error))?;
        Ok(ImportedImage {
            path: destination.to_string_lossy().into_owned(),
            name,
            mime_type: mime.into(),
            size: copied,
        })
    }
}

#[cfg(test)]
mod generated_image_tests {
    use super::*;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nfixture";

    #[test]
    fn generated_image_formats_are_sniffed_and_replays_are_idempotent() {
        let root = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let uploads = Uploads::from_root(store.path());
        for (bytes, ext, mime) in [
            (PNG, "png", "image/png"),
            (b"\xff\xd8\xfffixture".as_slice(), "jpg", "image/jpeg"),
            (b"RIFFxxxxWEBPfixture".as_slice(), "webp", "image/webp"),
            (b"GIF89afixture".as_slice(), "gif", "image/gif"),
        ] {
            let source = root.path().join("wrong.extension");
            std::fs::write(&source, bytes).unwrap();
            let key = format!("chat\0{ext}:image");
            let image = uploads
                .import_generated_image(&source, root.path(), &key)
                .unwrap();
            let again = uploads
                .import_generated_image(&source, root.path(), &key)
                .unwrap();
            assert_eq!(image, again);
            assert_eq!(image.mime_type, mime);
            assert_eq!(image.name, format!("generated.{ext}"));
            assert_eq!(image.size, bytes.len() as u64);
            assert_eq!(std::fs::read(&image.path).unwrap(), bytes);
            assert!(source.exists());
            let reply = uploads.read_chunk(&image.path, 0, &[]).unwrap();
            assert_eq!(BASE64.decode(reply.data).unwrap(), bytes);
            assert_eq!(reply.mime_type, mime);
            assert!(reply.done);
        }
        assert_eq!(std::fs::read_dir(store.path()).unwrap().count(), 4);
    }

    #[test]
    fn generated_image_rejects_untrusted_sources_without_publishing_files() {
        let root = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(outside.path(), PNG).unwrap();
        let uploads = Uploads::from_root(store.path());
        let unknown = root.path().join("bad.png");
        std::fs::write(&unknown, b"not a raster").unwrap();
        let big = root.path().join("big.png");
        let file = std::fs::File::create(&big).unwrap();
        file.set_len(MAX_GENERATED_IMAGE_BYTES + 1).unwrap();
        let mut paths = vec![
            PathBuf::from("relative.png"),
            outside.path().into(),
            root.path().into(),
            unknown,
            big,
        ];
        #[cfg(unix)]
        {
            let link = root.path().join("escape.png");
            std::os::unix::fs::symlink(outside.path(), &link).unwrap();
            paths.push(link);
        }
        for source in paths {
            assert!(
                uploads
                    .import_generated_image(&source, root.path(), "key")
                    .is_err(),
                "accepted {source:?}"
            );
        }
        assert_eq!(std::fs::read_dir(store.path()).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn generated_image_changed_during_copy_leaves_no_partial_final_or_temp() {
        let root = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let uploads = Uploads::from_root(store.path());
        let source = root.path().join("source.png");
        for replace in [false, true] {
            std::fs::write(&source, PNG).unwrap();
            let result =
                uploads.import_generated_image_after_copy(&source, root.path(), "key", || {
                    if replace {
                        let replacement = root.path().join("replacement.png");
                        std::fs::write(&replacement, PNG).unwrap();
                        std::fs::rename(replacement, &source).unwrap();
                    } else {
                        std::fs::write(&source, b"changed").unwrap();
                    }
                });
            assert!(result.is_err());
            assert_eq!(std::fs::read_dir(store.path()).unwrap().count(), 0);
        }
    }
    #[cfg(windows)]
    #[test]
    fn generated_image_copy_locks_source_and_parent_against_mutation() {
        let root = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let parent = root.path().join("nested");
        std::fs::create_dir(&parent).unwrap();
        let source = parent.join("source.png");
        let replacement = root.path().join("replacement.png");
        std::fs::write(&source, PNG).unwrap();
        std::fs::write(&replacement, PNG).unwrap();
        let uploads = Uploads::from_root(store.path());
        let result = uploads
            .import_generated_image_after_copy(&source, root.path(), "key", || {
                assert!(std::fs::write(&source, b"changed").is_err());
                assert!(std::fs::rename(&replacement, &source).is_err());
                assert!(std::fs::rename(&parent, root.path().join("moved")).is_err());
            })
            .unwrap();
        assert_eq!(std::fs::read(&result.path).unwrap(), PNG);
        assert_eq!(std::fs::read_dir(store.path()).unwrap().count(), 1);
        // All locks must be released after the operation.
        std::fs::write(&source, b"changed").unwrap();
        std::fs::rename(&parent, root.path().join("moved")).unwrap();
    }
}
