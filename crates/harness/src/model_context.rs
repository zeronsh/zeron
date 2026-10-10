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
        HarnessId::Opencode => {
            let paths = crate::opencode::paths::Paths::detect();
            let cwd = &paths.home;
            let mut files = vec![
                paths.auth_file(),
                paths.config.join("opencode.json"),
                paths.config.join("opencode.jsonc"),
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
        // Read through SQLite so uncheckpointed WAL credentials count too.
        // Hash only credentials: new chat rows must not invalidate the picker.
        let db = crate::opencode::paths::Paths::detect().database();
        hash_opencode_credentials(&mut hash, &db)?;
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

fn hash_opencode_credentials(hash: &mut Sha256, path: &Path) -> Result<(), HarnessError> {
    field(hash, path.as_os_str().as_encoded_bytes());
    if !path.exists() {
        field(hash, b"missing");
        return Ok(());
    }
    let error = |error: rusqlite::Error| {
        HarnessError::Protocol(format!("OpenCode credential fingerprint: {error}"))
    };
    let db =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(error)?;
    db.busy_timeout(std::time::Duration::from_secs(2))
        .map_err(error)?;
    let exists: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='credential')",
            [],
            |row| row.get(0),
        )
        .map_err(error)?;
    if !exists {
        field(hash, b"legacy");
        return Ok(());
    }
    let mut query = db
        .prepare("SELECT id,integration_id,value,active FROM credential ORDER BY id")
        .map_err(error)?;
    let rows = query
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<i64>>(3)?,
            ))
        })
        .map_err(error)?;
    for row in rows {
        let (id, provider, value, active) = row.map_err(error)?;
        for bytes in [
            id.as_bytes(),
            provider.as_deref().unwrap_or("").as_bytes(),
            value.as_bytes(),
        ] {
            field(hash, bytes);
        }
        field(hash, format!("{active:?}").as_bytes());
    }
    Ok(())
}
fn hash_files<'a>(
    hash: &mut Sha256,
    files: impl Iterator<Item = &'a PathBuf>,
) -> Result<(), HarnessError> {
    for path in files {
        field(hash, path.as_os_str().as_encoded_bytes());
        match std::fs::read(path) {
            Ok(bytes) => {
                hash.update([1]);
                field(hash, &bytes);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => hash.update([0]),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

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
    fn sqlite_credentials_in_wal_invalidate_but_chat_rows_do_not() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("opencode.db");
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; CREATE TABLE credential(id TEXT, integration_id TEXT, value TEXT, active INTEGER); CREATE TABLE chat(id INTEGER); INSERT INTO credential VALUES('cred','openai','secret-one',1);").unwrap();
        let fingerprint = || {
            let mut hash = Sha256::new();
            hash_opencode_credentials(&mut hash, &path).unwrap();
            format!("{:x}", hash.finalize())
        };
        let first = fingerprint();
        db.execute("UPDATE credential SET value='secret-two'", [])
            .unwrap();
        let changed = fingerprint();
        assert_ne!(first, changed);
        db.execute("INSERT INTO chat VALUES(1)", []).unwrap();
        assert_eq!(fingerprint(), changed);
        assert!(!changed.contains("secret"));
    }
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
}
