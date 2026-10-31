//! Localhost IPC bearer — who may drive the engine over `ws://127.0.0.1:{port}`.
//!
//! The Origin check in [`crate::serve_ws_listener`] keeps browser pages out, but
//! any local process can open a TCP socket to the port. Every engine start that
//! binds the port therefore mints a fresh 32-byte token, writes it to
//! `{data_dir}/ipc-token` (owner-only, atomic replace), and the server refuses a
//! WebSocket handshake without `Authorization: Bearer <token>`. Clients read the
//! file at dial time, so they keep working across engine restarts.
//!
//! Client resolution order ([`client_tokens`]): `$ZERON_IPC_TOKEN` (explicit
//! value), then the file named by `$ZERON_IPC_TOKEN_FILE` (what the engine hands
//! the `zeron mcp` servers it injects into agents), then `{data_dir}/ipc-token`.
//! In-process (in-memory) transports never see any of this.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// File name of the token inside the engine's data dir.
pub const TOKEN_FILE_NAME: &str = "ipc-token";
/// Explicit token value for a client (wins over every file).
pub const TOKEN_ENV: &str = "ZERON_IPC_TOKEN";
/// Path of a token file for a client (the engine injects this into `zeron mcp`).
pub const TOKEN_FILE_ENV: &str = "ZERON_IPC_TOKEN_FILE";

/// `{data_dir}/ipc-token`.
pub fn token_path(data_dir: &Path) -> PathBuf {
    data_dir.join(TOKEN_FILE_NAME)
}

/// 32 bytes from the OS CSPRNG, lowercase hex (64 chars).
pub fn generate_token() -> io::Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|e| io::Error::other(format!("no OS randomness for the IPC token: {e}")))?;
    Ok(hex(&bytes))
}

/// Mint a fresh token and publish it at `{data_dir}/ipc-token`. Called by the
/// engine on every start that serves the IPC port; the previous token (and
/// every client still holding it) stops working.
pub fn issue_token(data_dir: &Path) -> io::Result<String> {
    let token = generate_token()?;
    write_token(&token_path(data_dir), &token)?;
    Ok(token)
}

/// Atomically replace `path` with `token`: a uniquely named sibling created
/// exclusively (never follows a planted symlink) with mode 0600, fsynced, then
/// renamed over the target. A reader sees the old token or the new one, never
/// a torn write.
///
/// Windows has no mode bits: the file inherits the ACL of the data dir
/// (`%LOCALAPPDATA%\Zeron` — owner, SYSTEM and Administrators only by
/// default), the same protection `session.json` relies on there.
pub fn write_token(path: &Path, token: &str) -> io::Result<()> {
    let dir = path
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let suffix = generate_token()?;
    let tmp = dir.join(format!(".{TOKEN_FILE_NAME}.{}.tmp", &suffix[..16]));
    let written = write_exclusive(&tmp, token).and_then(|()| std::fs::rename(&tmp, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

/// Create `path` (which must not exist) owner-only and durably write `token`.
fn write_exclusive(path: &Path, token: &str) -> io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    #[cfg(unix)]
    {
        // umask can only clear bits, but be explicit about the contract.
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(token.as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()
}

/// The token stored at `path`; `None` when the file is missing or empty.
pub fn read_token(path: &Path) -> io::Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(raw) => Ok(Some(raw.trim().to_owned()).filter(|t| !t.is_empty())),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

/// Every token a client should try, best first, deduplicated: the
/// `$ZERON_IPC_TOKEN` value, the `$ZERON_IPC_TOKEN_FILE` file, then
/// `{data_dir}/ipc-token`. Read at call time — after an engine restart the
/// file already holds the new token. Empty when nothing is configured (an
/// older engine that predates IPC auth still accepts a bare dial).
pub fn client_tokens(data_dir: Option<&Path>) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    let mut push = |token: Option<String>| {
        if let Some(token) = token.map(|t| t.trim().to_owned())
            && !token.is_empty()
            && !tokens.contains(&token)
        {
            tokens.push(token);
        }
    };
    push(std::env::var(TOKEN_ENV).ok());
    if let Some(file) = std::env::var_os(TOKEN_FILE_ENV).filter(|f| !f.is_empty()) {
        push(read_logged(Path::new(&file)));
    }
    if let Some(dir) = data_dir {
        push(read_logged(&token_path(dir)));
    }
    tokens
}

fn read_logged(path: &Path) -> Option<String> {
    read_token(path).unwrap_or_else(|err| {
        tracing::warn!(path = %path.display(), error = %err, "rpc: cannot read the IPC token");
        None
    })
}

/// Does an `Authorization` header value carry exactly `expected` as its bearer?
/// Constant-time over the token bytes (the length is not secret: every token
/// is 64 hex chars).
pub(crate) fn bearer_matches(expected: &str, header: Option<&[u8]>) -> bool {
    let Some(header) = header else {
        return false;
    };
    let Some(presented) = strip_bearer(header) else {
        return false;
    };
    constant_time_eq(presented, expected.as_bytes())
}

fn strip_bearer(header: &[u8]) -> Option<&[u8]> {
    const SCHEME: &[u8] = b"bearer ";
    if header.len() < SCHEME.len() || !header[..SCHEME.len()].eq_ignore_ascii_case(SCHEME) {
        return None;
    }
    Some(header[SCHEME.len()..].trim_ascii())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let diff = a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y));
    std::hint::black_box(diff) == 0
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[usize::from(byte >> 4)] as char);
        out.push(DIGITS[usize::from(byte & 0x0f)] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_fresh_32_byte_hex() {
        let a = generate_token().unwrap();
        let b = generate_token().unwrap();
        assert_eq!(a.len(), 64);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn issue_replaces_the_file_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let first = issue_token(dir.path()).unwrap();
        assert_eq!(
            read_token(&token_path(dir.path())).unwrap(),
            Some(first.clone())
        );
        let second = issue_token(dir.path()).unwrap();
        assert_ne!(first, second, "every engine start rotates the token");
        assert_eq!(read_token(&token_path(dir.path())).unwrap(), Some(second));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(token_path(dir.path()))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "token file must be owner-only");
        }
        // No temp files left behind.
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, vec![TOKEN_FILE_NAME.to_string()]);
    }

    #[cfg(unix)]
    #[test]
    fn issue_replaces_a_planted_symlink_instead_of_writing_through_it() {
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("victim");
        std::fs::write(&victim, "untouched").unwrap();
        std::os::unix::fs::symlink(&victim, token_path(dir.path())).unwrap();
        issue_token(dir.path()).unwrap();
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "untouched");
        assert!(
            !std::fs::symlink_metadata(token_path(dir.path()))
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn missing_or_empty_token_reads_as_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_token(&token_path(dir.path())).unwrap(), None);
        std::fs::write(token_path(dir.path()), "\n").unwrap();
        assert_eq!(read_token(&token_path(dir.path())).unwrap(), None);
    }

    #[test]
    fn bearer_matching_is_exact() {
        let token = "ab".repeat(32);
        let header = format!("Bearer {token}");
        assert!(bearer_matches(&token, Some(header.as_bytes())));
        assert!(bearer_matches(
            &token,
            Some(format!("bearer  {token} ").as_bytes())
        ));
        assert!(!bearer_matches(&token, None));
        assert!(!bearer_matches(&token, Some(token.as_bytes())));
        assert!(!bearer_matches(&token, Some(b"Bearer ")));
        assert!(!bearer_matches(
            &token,
            Some(format!("Bearer {}", "ab".repeat(31)).as_bytes())
        ));
        assert!(!bearer_matches(
            &token,
            Some(format!("Bearer {}cd", "ab".repeat(31)).as_bytes())
        ));
        assert!(!bearer_matches(
            &token,
            Some(format!("Basic {token}").as_bytes())
        ));
    }
}
