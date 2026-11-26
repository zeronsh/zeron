//! Credential bytes are hashed locally and never included in diagnostics or disk catalogs.
use crate::{HarnessError, ModelContext};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use zeron_proto::HarnessId;

pub(crate) fn root(variable: &str, fallback: PathBuf) -> PathBuf {
    std::env::var_os(variable)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or(fallback)
}

pub(crate) fn context(
    id: HarnessId,
    binary: &Path,
    extra: &[PathBuf],
) -> Result<ModelContext, HarnessError> {
    let home = crate::executable::home_or_current_dir();
    let files = match id {
        HarnessId::Codex => vec![root("CODEX_HOME", home.join(".codex")).join("auth.json")],
        HarnessId::ClaudeCode => {
            let root = root("CLAUDE_CONFIG_DIR", home.join(".claude"));
            vec![root.join("settings.json"), root.join(".credentials.json")]
        }
        HarnessId::Cursor => vec![home.join(".cursor/sdk/auth.json")],
        HarnessId::Fx => vec![
            home.join(".fx/auth.json"),
            home.join(".fx/settings.json"),
            home.join(".fx/api-key"),
            home.join(".fx/chatgpt-auth.json"),
            home.join(".fx/grok-auth.json"),
        ],
        HarnessId::Opencode => {
            let data = root("XDG_DATA_HOME", home.join(".local/share"));
            let config = root("XDG_CONFIG_HOME", home.join(".config"));
            let cwd = std::env::current_dir()?;
            let mut files = vec![
                data.join("opencode/auth.json"),
                config.join("opencode/opencode.json"),
                config.join("opencode/opencode.jsonc"),
            ];
            for dir in cwd.ancestors() {
                for name in [
                    "opencode.json",
                    "opencode.jsonc",
                    ".opencode/opencode.json",
                    ".opencode/opencode.jsonc",
                ] {
                    files.push(dir.join(name));
                }
            }
            if let Some(path) = std::env::var_os("OPENCODE_CONFIG") {
                files.push(path.into());
            }
            files
        }
        HarnessId::Grok => {
            let root = root("GROK_HOME", home.join(".grok"));
            vec![root.join("auth.json"), root.join("config.toml")]
        }
        HarnessId::Hermes => {
            let root = root("HERMES_HOME", home.join(".hermes"));
            vec![
                root.join("auth.json"),
                root.join(".env"),
                root.join("config.yaml"),
            ]
        }
        HarnessId::Pi => {
            vec![root("PI_CODING_AGENT_DIR", home.join(".pi/agent")).join("auth.json")]
        }
        // Its file login (one Zeron signed it in to) and the method its
        // settings choose: a sign-in is another account's catalog. (A
        // Keychain login has no file to tell by.) Consumer OAuth token files
        // (`antigravity-oauth-token` / `acp_token.json`) are hashed by stable
        // account identity below so legacy→unified migration or in-place
        // access_token refresh during model discovery never invalidates the
        // in-flight discovery for the same Google account.
        HarnessId::Antigravity => match crate::acp::antigravity_home() {
            Ok(root) => {
                let acp = root.join("antigravity-acp");
                vec![
                    acp.join("acp_business_token.json"),
                    acp.join("settings.json"),
                ]
            }
            Err(_) => vec![],
        },
        HarnessId::Devin => {
            let data = root("XDG_DATA_HOME", home.join(".local/share"));
            let mut files = vec![data.join("devin/credentials.toml")];
            if cfg!(target_os = "macos") {
                files.push(home.join("Library/Application Support/devin/credentials.toml"));
            }
            if cfg!(windows) {
                files.push(
                    root("APPDATA", home.join("AppData/Roaming")).join("devin/credentials.toml"),
                );
            }
            files
        }
        _ => vec![],
    };
    let binary = binary
        .canonicalize()
        .unwrap_or_else(|_| binary.to_path_buf());
    let version = crate::executable::binary_version(&binary).map(|v| v.to_string());
    let mut hash = Sha256::new();
    field(&mut hash, binary.as_os_str().as_encoded_bytes());
    field(
        &mut hash,
        version.as_deref().unwrap_or("unknown").as_bytes(),
    );
    // Unknown-version executables must still invalidate on replacement.
    if let Ok(metadata) = binary.metadata() {
        field(
            &mut hash,
            format!("{:?}:{}", metadata.modified().ok(), metadata.len()).as_bytes(),
        );
    }
    hash_files(&mut hash, files.iter().chain(extra))?;
    if id == HarnessId::Opencode {
        let data = root("XDG_DATA_HOME", home.join(".local/share"));
        hash_opencode_credentials(&mut hash, &data.join("opencode/opencode.db"));
    }
    if id == HarnessId::Antigravity
        && let Ok(root) = crate::acp::antigravity_home()
    {
        hash_antigravity_oauth(&mut hash, &root.join("antigravity-acp"))?;
    }
    let prefixes: &[&str] = match id {
        HarnessId::Codex => &["CODEX_", "OPENAI_"],
        HarnessId::ClaudeCode => &["CLAUDE_", "ANTHROPIC_", "AWS_"],
        HarnessId::Opencode => &["OPENCODE_", "OPENAI_", "ANTHROPIC_", "GOOGLE_"],
        HarnessId::Grok => &["GROK_", "XAI_"],
        HarnessId::Hermes => &["HERMES_", "OPENAI_", "ANTHROPIC_"],
        HarnessId::Pi => &["PI_", "OPENAI_", "ANTHROPIC_"],
        HarnessId::Devin => &["DEVIN_"],
        HarnessId::Antigravity => &["GEMINI_", "GOOGLE_"],
        HarnessId::Cursor => &["CURSOR_"],
        HarnessId::Fx => &["FX_", "AI_GATEWAY_", "OPENAI_", "ANTHROPIC_"],
        _ => &[],
    };
    let mut env: Vec<_> = std::env::vars_os()
        .filter(|(key, _)| {
            prefixes
                .iter()
                .any(|prefix| key.to_string_lossy().starts_with(prefix))
        })
        .collect();
    env.sort();
    for (key, value) in env {
        field(&mut hash, key.as_encoded_bytes());
        field(&mut hash, value.as_encoded_bytes());
    }
    // A Cloud machine's credential keys for this agent (none elsewhere).
    crate::catalog_env::hash_into(&mut hash, id);
    Ok(ModelContext {
        hash: format!("{:x}", hash.finalize()),
        binary_path: binary,
        binary_version: version,
    })
}

fn field(hash: &mut Sha256, bytes: &[u8]) {
    hash.update((bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
}
fn hash_files<'a>(
    hash: &mut Sha256,
    files: impl Iterator<Item = &'a PathBuf>,
) -> Result<(), HarnessError> {
    for path in files {
        field(hash, path.as_os_str().as_encoded_bytes());
        // Some of these sit in the project (`opencode.json` up its tree), a
        // repository anyone may have written: one that is a FIFO or a
        // symlink to `/dev/zero` is refused, never read.
        match crate::bounded_file::read(path, MAX_HASHED_FILE) {
            Ok(bytes) => {
                hash.update([1]);
                field(hash, &bytes);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => hash.update([0]),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::InvalidInput | std::io::ErrorKind::FileTooLarge
                ) =>
            {
                hash.update([2]);
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

/// Hash the credential rows that decide OpenCode 2.x's connected catalog.
/// The database itself is large and mostly sessions/messages, so hashing its
/// bytes would be expensive and would invalidate on unrelated writes. Only row
/// identity, provider and active state are mixed in: token refreshes must not
/// invalidate a catalog, and secrets never leave OpenCode's database.
fn hash_opencode_credentials(hash: &mut Sha256, db: &Path) {
    field(hash, b"opencode-2-credentials");
    if !db.exists() {
        hash.update([0]);
        return;
    }
    let marker = |hash: &mut Sha256, marker: u8| hash.update([marker]);
    let Ok(conn) = rusqlite::Connection::open_with_flags(
        db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) else {
        marker(hash, 2);
        return;
    };
    let Ok(mut statement) = conn.prepare(
        "SELECT id, COALESCE(integration_id, ''), COALESCE(active, 0) \
         FROM credential ORDER BY id",
    ) else {
        marker(hash, 2);
        return;
    };
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
        ))
    });
    let Ok(rows) = rows else {
        marker(hash, 2);
        return;
    };
    let mut count = 0_u64;
    for row in rows.filter_map(Result::ok) {
        count += 1;
        field(hash, row.0.as_bytes());
        field(hash, row.1.as_bytes());
        field(hash, row.2.to_string().as_bytes());
    }
    field(hash, count.to_string().as_bytes());
}

/// Hash Antigravity's consumer OAuth login by stable account identity
/// (`refresh_token`) rather than volatile access_token/expiry bytes so
/// legacy→unified migration and in-place token refreshes during model
/// discovery never falsely invalidate an in-flight catalog refresh.
fn hash_antigravity_oauth(hash: &mut Sha256, acp_dir: &Path) -> Result<(), HarnessError> {
    field(hash, b"antigravity-oauth-identity");
    let candidates = [
        acp_dir.join("antigravity-oauth-token"),
        acp_dir.join("acp_token.json"),
    ];
    let mut fallback_raw: Option<Vec<u8>> = None;
    for path in &candidates {
        match crate::bounded_file::read(path, MAX_HASHED_FILE) {
            Ok(bytes) => {
                if let Ok(doc) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                    let refresh = doc
                        .pointer("/token/refresh_token")
                        .or_else(|| doc.get("refresh_token"))
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty());
                    if let Some(rt) = refresh {
                        hash.update([1]);
                        field(hash, b"rt");
                        field(hash, rt.as_bytes());
                        return Ok(());
                    }
                    let access = doc
                        .pointer("/token/access_token")
                        .or_else(|| doc.get("access_token"))
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty());
                    if let Some(at) = access {
                        hash.update([2]);
                        field(hash, b"at");
                        field(hash, at.as_bytes());
                        return Ok(());
                    }
                }
                if fallback_raw.is_none() {
                    fallback_raw = Some(bytes);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::InvalidInput | std::io::ErrorKind::FileTooLarge
                ) =>
            {
                hash.update([4]);
                return Ok(());
            }
            Err(e) => return Err(e.into()),
        }
    }
    if let Some(raw) = fallback_raw {
        hash.update([3]);
        field(hash, &raw);
    } else {
        hash.update([0]);
    }
    Ok(())
}

/// The most of one config or credentials file hashed.
const MAX_HASHED_FILE: u64 = 16 * 1024 * 1024;

impl ModelContext {
    pub(crate) fn key(&self) -> [u8; 32] {
        Sha256::digest(self.hash.as_bytes()).into()
    }
    pub(crate) fn log(&self) {
        tracing::info!(binary_path = %self.binary_path.display(), binary_version = self.binary_version.as_deref().unwrap_or("unknown"), "Model discovery binary");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn content_changes_and_missing_files_invalidate_without_exposing_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("auth.json");
        let key = || {
            let mut hash = Sha256::new();
            hash_files(&mut hash, std::iter::once(&file)).unwrap();
            format!("{:x}", hash.finalize())
        };
        let missing = key();
        std::fs::write(&file, "account-one").unwrap();
        let first = key();
        std::fs::write(&file, "account-two").unwrap();
        assert_ne!(first, key());
        assert_ne!(missing, first);
        assert!(!first.contains("account"));
        std::fs::remove_file(&file).unwrap();
        assert_eq!(key(), missing);
    }

    #[test]
    fn antigravity_oauth_hash_is_stable_across_migration_and_access_refresh() {
        let dir = tempfile::tempdir().unwrap();
        let key = || {
            let mut hash = Sha256::new();
            hash_antigravity_oauth(&mut hash, dir.path()).unwrap();
            format!("{:x}", hash.finalize())
        };
        let signed_out = key();

        // Legacy acp_token.json only.
        std::fs::write(
            dir.path().join("acp_token.json"),
            r#"{"refresh_token":"1//acct-a","access_token":"ya29.old","client_id":"cid"}"#,
        )
        .unwrap();
        let legacy_a = key();
        assert_ne!(legacy_a, signed_out);

        // Migrated antigravity-oauth-token written alongside legacy file.
        std::fs::write(
            dir.path().join("antigravity-oauth-token"),
            r#"{"token":{"refresh_token":"1//acct-a","access_token":"expired","expiry":"2020-01-01T00:00:00Z"},"auth_method":"consumer"}"#,
        )
        .unwrap();
        assert_eq!(key(), legacy_a, "migration must keep same account hash");

        // In-place access_token refresh by unified CLI keeps the same hash.
        std::fs::write(
            dir.path().join("antigravity-oauth-token"),
            r#"{"token":{"refresh_token":"1//acct-a","access_token":"ya29.fresh","expiry":"2099-01-01T00:00:00Z"},"auth_method":"consumer"}"#,
        )
        .unwrap();
        assert_eq!(key(), legacy_a, "access_token refresh must keep same hash");

        // Switching to another Google account invalidates immediately.
        std::fs::write(
            dir.path().join("antigravity-oauth-token"),
            r#"{"token":{"refresh_token":"1//acct-b","access_token":"ya29.b","expiry":"2099-01-01T00:00:00Z"},"auth_method":"consumer"}"#,
        )
        .unwrap();
        assert_ne!(key(), legacy_a, "different refresh_token must invalidate");

        // Signing out (both files removed) returns to signed_out hash.
        std::fs::remove_file(dir.path().join("antigravity-oauth-token")).unwrap();
        std::fs::remove_file(dir.path().join("acp_token.json")).unwrap();
        assert_eq!(key(), signed_out);
    }

    /// A project's `opencode.json` that is a symlink to `/dev/zero` or a
    /// FIFO is hashed as unreadable at once, never read.
    #[cfg(unix)]
    #[test]
    fn a_config_that_is_a_device_or_a_fifo_is_not_read() {
        let dir = tempfile::tempdir().unwrap();
        let zero = dir.path().join("opencode.json");
        std::os::unix::fs::symlink("/dev/zero", &zero).unwrap();
        let fifo = dir.path().join("opencode.jsonc");
        let c_path = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: a valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut hash = Sha256::new();
            let _ = tx.send(hash_files(&mut hash, [zero, fifo].iter()).is_ok());
        });
        assert!(
            rx.recv_timeout(std::time::Duration::from_secs(5))
                .expect("hashing the project's config hung")
        );
    }
}
