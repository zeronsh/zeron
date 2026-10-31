//! Private, read-only snapshot of the live profile.
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OpenFlags, backup::Backup};
use std::{
    collections::BTreeMap,
    ffi::CStr,
    fs,
    io::{Read, Seek, SeekFrom},
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

pub fn snapshot() -> Result<PathBuf> {
    let started = std::time::Instant::now();
    let root = super::safe_root()?;
    let entry = unsafe { libc::getpwuid(libc::getuid()) };
    ensure!(!entry.is_null(), "cannot resolve user home");
    let home = unsafe { CStr::from_ptr((*entry).pw_dir) };
    let source = fs::canonicalize(
        PathBuf::from(std::ffi::OsStr::from_bytes(home.to_bytes())).join(".zeron"),
    )?;
    ensure!(
        source.file_name().is_some_and(|s| s == ".zeron"),
        "unexpected profile path"
    );
    let id = format!("snapshot-{}", crate::mono_now());
    let output = super::fixture_dir(&root, &["fixtures", "private", "P-real", &id])?;
    fs::set_permissions(&output, fs::Permissions::from_mode(0o700))?;
    let mount = output.join("source-ro");
    fs::create_dir(&mount)?;
    let bin = std::env::current_exe()?;
    let status = Command::new("unshare").args(["--user", "--map-root-user", "--mount", "--propagation", "private", "--", "/bin/sh", "-c", "mount --bind -- \"$1\" \"$2\" && mount -o remount,bind,ro -- \"$2\" && exec \"$3\" fixture snapshot-worker \"$2\" \"$4\"", "snapshot"])
        .env("ZB_ROOT", &root).arg(&source).arg(&mount).arg(&bin).arg(&output).status().context("run read-only snapshot namespace")?;
    ensure!(
        status.success(),
        "profile snapshot failed; inspect {}",
        output.display()
    );
    fs::remove_dir(mount)?;
    let manifest_path = output.join("manifest.json");
    let mut manifest: serde_json::Value = serde_json::from_slice(&fs::read(&manifest_path)?)?;
    manifest["generation_ms"] = serde_json::json!(started.elapsed().as_millis());
    fs::write(manifest_path, serde_json::to_vec_pretty(&manifest)?)?;
    Ok(output)
}

type Listing = BTreeMap<String, (i64, i64, bool)>;
fn listing(base: &Path, dir: &Path, rows: &mut Listing) -> Result<()> {
    let meta = fs::symlink_metadata(dir)?;
    rows.insert(
        dir.strip_prefix(base)?.to_string_lossy().into_owned(),
        (meta.mtime(), meta.mtime_nsec(), meta.is_dir()),
    );
    if meta.is_dir() {
        for item in fs::read_dir(dir)? {
            let path = item?.path();
            match listing(base, &path, rows) {
                Ok(()) => (),
                Err(e) if !path.exists() => {
                    let _ = e;
                }
                Err(e) => return Err(e),
            }
        }
    }
    Ok(())
}
fn live_owned(path: &str) -> bool {
    path.ends_with("/previews.json")
        || path == "previews.json"
        || path.starts_with("agent-accounts/")
        || path == "agent-accounts"
        || path.contains("journals/")
        || path.starts_with("logs/")
        || path.contains("docs.sqlite3")
        || path.starts_with("turn-snapshots/")
        || path.starts_with("model-catalogs/")
        || path == "."
        || path.is_empty()
}
fn excluded(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if ["api-key", "api_key", "credential", "secret", "token"]
        .iter()
        .any(|part| lower.contains(part))
        || lower == ".env"
    {
        return true;
    }
    matches!(
        name,
        "agent-accounts"
            | "session.json"
            | "ipc-token"
            | "device-id"
            | "engine.lock"
            | ".credentials.json"
            | "browser"
    )
}

pub fn copy_mounted(source: &Path, output: &Path) -> Result<()> {
    let root = super::safe_root()?;
    let output = fs::canonicalize(output)?;
    ensure!(
        output.starts_with(root.join("fixtures/private/P-real")),
        "unsafe snapshot destination"
    );
    ensure!(
        source == output.join("source-ro"),
        "unexpected snapshot mount"
    );
    let output = output.as_path();
    // This entry point is only valid for the mount installed by snapshot().
    let mounts = fs::read_to_string("/proc/self/mountinfo")?;
    let source_text = source.to_str().context("mount path is not UTF-8")?;
    ensure!(
        mounts
            .lines()
            .any(|line| line.split_whitespace().nth(4) == Some(source_text)
                && line
                    .split_whitespace()
                    .nth(5)
                    .is_some_and(|opts| opts.split(',').any(|v| v == "ro"))),
        "source is not a read-only mount"
    );
    let mut before = Listing::new();
    listing(source, source, &mut before)?;
    fs::write(
        output.join("mtime-before.json"),
        serde_json::to_vec_pretty(&before)?,
    )?;
    let data = output.join("data");
    fs::create_dir(&data)?;
    copy_tree(source, source, &data)?;
    let mut after = Listing::new();
    listing(source, source, &mut after)?;
    fs::write(
        output.join("mtime-after.json"),
        serde_json::to_vec_pretty(&after)?,
    )?;
    let changed: Vec<_> = before
        .keys()
        .chain(after.keys())
        .filter(|p| {
            !live_owned(p)
                && !before
                    .get(*p)
                    .or_else(|| after.get(*p))
                    .is_some_and(|v| v.2)
                && before.get(*p) != after.get(*p)
        })
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut hashes = BTreeMap::new();
    super::profile::hash_tree(output, &data, &mut hashes)?;
    let (journal_bytes, sqlite_bytes) = super::profile::data_sizes(&data)?;
    let manifest = serde_json::json!({"files":hashes,"journal_bytes":journal_bytes,"sqlite_bytes":sqlite_bytes,"name":"P-real","source_mount_read_only":true,"before_listing":"mtime-before.json","after_listing":"mtime-after.json","non_live_owned_mtime_changes":changed,"excluded":["session.json","ipc-token","device-id","engine.lock","agent-accounts","browser",".credentials.json","names containing api-key, api_key, credential, secret or token",".env"]});
    fs::write(
        output.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    Ok(())
}
fn copy_tree(base: &Path, dir: &Path, output: &Path) -> Result<()> {
    for item in fs::read_dir(dir)? {
        let entry = item?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if excluded(&name)
            || name.ends_with("-wal")
            || name.ends_with("-shm")
            || name.ends_with("-journal")
        {
            continue;
        }
        let meta = fs::symlink_metadata(&path)?;
        let dest = output.join(path.strip_prefix(base)?);
        if meta.is_dir() {
            fs::create_dir(&dest)?;
            copy_tree(base, &path, output)?;
        } else if meta.is_file() {
            if name.ends_with(".sqlite3") || name.ends_with(".sqlite") || name.ends_with(".db") {
                let mut wal_name = path.as_os_str().to_os_string();
                wal_name.push("-wal");
                let wal = PathBuf::from(wal_name);
                let has_wal = wal.exists();
                let mut encoded = String::from("file:");
                for byte in path.as_os_str().as_bytes() {
                    if byte.is_ascii_alphanumeric() || b"/-._~".contains(byte) {
                        encoded.push(*byte as char);
                    } else {
                        encoded.push_str(&format!("%{byte:02X}"));
                    }
                }
                let uri = format!(
                    "{encoded}?mode=ro{}",
                    if has_wal { "" } else { "&immutable=1" }
                );
                let src = Connection::open_with_flags(
                    &uri,
                    OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
                )?;
                let mut dst = Connection::open(&dest)?;
                Backup::new(&src, &mut dst)?.run_to_completion(
                    128,
                    Duration::from_millis(10),
                    None,
                )?;
                if !has_wal {
                    let now = fs::metadata(&path)?;
                    ensure!(
                        !wal.exists()
                            && meta.mtime() == now.mtime()
                            && meta.mtime_nsec() == now.mtime_nsec()
                            && meta.len() == now.len(),
                        "database changed during backup: {}",
                        path.display()
                    );
                }
            } else {
                ensure!(
                    Command::new("/usr/bin/cp")
                        .args(["--reflink=auto", "--preserve=mode,timestamps", "--"])
                        .arg(&path)
                        .arg(&dest)
                        .status()?
                        .success(),
                    "copy {} failed",
                    path.display()
                );
                if name.ends_with(".jsonl")
                    && path.components().any(|c| c.as_os_str() == "journals")
                {
                    truncate_line(&dest)?;
                }
            }
        }
    }
    Ok(())
}
fn truncate_line(path: &Path) -> Result<()> {
    let mut file = fs::OpenOptions::new().read(true).write(true).open(path)?;
    let len = file.metadata()?.len();
    let mut pos = len;
    let mut buf = [0u8; 8192];
    while pos > 0 {
        let take = pos.min(buf.len() as u64) as usize;
        pos -= take as u64;
        file.seek(SeekFrom::Start(pos))?;
        file.read_exact(&mut buf[..take])?;
        if let Some(i) = buf[..take].iter().rposition(|b| *b == b'\n') {
            file.set_len(pos + i as u64 + 1)?;
            return Ok(());
        }
    }
    file.set_len(0)?;
    Ok(())
}

/// Boot a reflinked snapshot in an isolated trial and return its chat count.
pub async fn verify(path: &Path, engine: &Path) -> Result<usize> {
    let root = super::safe_root()?;
    let fixture = fs::canonicalize(path)?;
    ensure!(
        fixture.starts_with(root.join("fixtures/private/P-real")),
        "invalid real fixture"
    );
    let orgs = fixture.join("data/orgs");
    let mut profiles = Vec::new();
    if orgs.is_dir() {
        for org in fs::read_dir(orgs)? {
            let org = org?;
            if org.file_type()?.is_dir() {
                for user in fs::read_dir(org.path())? {
                    let user = user?;
                    if user.file_type()?.is_dir() && user.path().join("docs.sqlite3").is_file() {
                        profiles.push((org.file_name(), user.file_name()));
                    }
                }
            }
        }
    }
    ensure!(
        profiles.len() == 1,
        "expected one account profile, found {}",
        profiles.len()
    );
    let mut trial = crate::Trial::create(&root, "real-verify")?;
    // A copied live journal can contain interrupted turns. Never start any
    // installed provider while checking that the snapshot boots.
    trial.replay_bin = PathBuf::from("/usr/bin/false");
    for key in [
        "CODEX_EXECUTABLE",
        "PI_EXECUTABLE",
        "MUSE_EXECUTABLE",
        "OPENCODE_EXECUTABLE",
        "CURSOR_SDK_SHIM_EXECUTABLE",
        "GROK_EXECUTABLE",
        "DEVIN_EXECUTABLE",
        "FX_EXECUTABLE",
        "HERMES_EXECUTABLE",
        "ANTIGRAVITY_ACP_EXECUTABLE",
    ] {
        trial.extra.push((key.into(), "/usr/bin/false".into()));
    }
    trial
        .extra
        .push(("ZERON_ORG_ID".into(), profiles[0].0.clone()));
    trial
        .extra
        .push(("ZERON_USER_ID".into(), profiles[0].1.clone()));
    trial.allocate_port()?;
    trial.prepare()?;
    ensure!(
        Command::new("cp")
            .args(["-a", "--reflink=auto"])
            .arg(fixture.join("data/."))
            .arg(&trial.data)
            .status()?
            .success(),
        "copy real fixture failed"
    );
    let org_dir = trial.data.join("orgs").join(&profiles[0].0);
    let copied_user = org_dir.join(&profiles[0].1);
    let dev_user = org_dir.join("dev-user");
    if copied_user != dev_user {
        ensure!(!dev_user.exists(), "development profile already exists");
        fs::rename(copied_user, dev_user)?;
    }
    let launched = crate::launch(engine, &trial).await?;
    let result = async {
        let client = crate::rpc::Client::connect(trial.port, &trial.data).await?;
        let mut stream = client
            .subscribe("WatchChats", serde_json::json!({}))
            .await?;
        let item = stream.recv().await.context("chat stream ended")??;
        Ok::<usize, anyhow::Error>(
            item.value
                .as_array()
                .context("chat list is not an array")?
                .len(),
        )
    }
    .await;
    let stop = launched.stop().await;
    let count = result?;
    stop?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn journal_truncates_only_unterminated_tail() {
        let path = std::env::temp_dir().join(format!("zb-journal-{}", crate::mono_now()));
        fs::write(&path, b"one\ntwo\npartial").unwrap();
        truncate_line(&path).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"one\ntwo\n");
        fs::remove_file(path).unwrap();
    }
    #[test]
    fn snapshot_excludes_secrets_and_classifies_engine_writes() {
        assert!(excluded("jev-api-key"));
        assert!(excluded("agent-accounts"));
        assert!(excluded("session.json"));
        assert!(live_owned("orgs/a/b/journals/chat.jsonl"));
        assert!(live_owned("orgs/a/b/previews.json"));
        assert!(!live_owned("user-created.txt"));
    }
}
