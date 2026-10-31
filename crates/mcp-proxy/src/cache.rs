use crate::config::Config;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// A failed discovery sends later sessions down the uncached passthrough path; a failure
/// caused by something transient (offline, a slow first install) must not do so forever.
pub const DISCOVERY_RETRY_MS: u64 = 6 * 60 * 60 * 1000;
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
pub fn hash(value: &Value) -> String {
    // serde_json may have preserve_order enabled by another workspace dependency.
    fn sorted(v: &Value) -> Value {
        match v {
            Value::Object(m) => {
                let m: BTreeMap<_, _> = m.iter().map(|(k, v)| (k.clone(), sorted(v))).collect();
                json!(m)
            }
            Value::Array(a) => Value::Array(a.iter().map(sorted).collect()),
            _ => v.clone(),
        }
    }
    hex::encode(Sha256::digest(serde_json::to_vec(&sorted(value)).unwrap()))
}
pub fn executable(command: &str, path: Option<&std::ffi::OsStr>) -> PathBuf {
    let candidate =
        if Path::new(command).components().count() > 1 || Path::new(command).is_absolute() {
            PathBuf::from(command)
        } else {
            std::env::split_paths(path.unwrap_or_default())
                .map(|p| p.join(command))
                .find(|p| is_executable(p))
                .unwrap_or_else(|| command.into())
        };
    fs::canonicalize(&candidate).unwrap_or(candidate)
}
fn is_executable(p: &Path) -> bool {
    let Ok(meta) = p.metadata() else { return false };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}
pub fn binary_id(path: &Path) -> String {
    let meta = path.metadata().ok();
    format!(
        "{}:{}:{}",
        path.display(),
        meta.as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|t| t.as_nanos())
            .unwrap_or(0),
        meta.map(|m| m.len()).unwrap_or(0)
    )
}
/// Interpreter servers (`node server.js`, `python app.py`) change when their script does,
/// not when the interpreter does: fold in the identity of every argument that is a file.
fn arg_files(c: &Config) -> Value {
    let base = c.cwd.clone().or_else(|| std::env::current_dir().ok());
    json!(
        c.args
            .iter()
            .take(32)
            .filter_map(|arg| {
                let path = base
                    .as_ref()
                    .map_or_else(|| PathBuf::from(arg), |b| b.join(arg));
                path.is_file().then(|| (arg, binary_id(&path)))
            })
            .collect::<Vec<_>>()
    )
}
pub fn key(c: &Config, initialize: &Value, command: &Path, bin: &Path, npx: Value) -> String {
    let env: BTreeMap<_, _> = c
        .identity_env
        .iter()
        .map(|k| {
            (
                k.clone(),
                std::env::var_os(k).map(|v| v.to_string_lossy().into_owned()),
            )
        })
        .collect();
    key_fields(
        command,
        &c.args,
        &json!(env),
        c.plugin_root.as_deref(),
        c.cwd.as_deref(),
        bin,
        npx,
        arg_files(c),
        initialize,
    )
}
#[allow(clippy::too_many_arguments)]
fn key_fields(
    command: &Path,
    args: &[String],
    env: &Value,
    plugin_root: Option<&Path>,
    cwd: Option<&Path>,
    bin: &Path,
    npx: Value,
    arg_files: Value,
    init: &Value,
) -> String {
    hash(
        &json!({"v":1,"command":command,"args":args,"identity_env":env,"plugin_root":plugin_root,"cwd_if_configured":cwd,
        "npx":npx,"bin_id":binary_id(bin),"arg_files":arg_files,"client":{"name":init["clientInfo"]["name"],
        "version_major":init["clientInfo"]["version"].as_str().unwrap_or("").split('.').next().unwrap_or(""),
        "protocolVersion":init["protocolVersion"],"capabilities_hash":hash(&init["capabilities"])}}),
    )
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    pub key: String,
    pub name: String,
    pub cached_at: u64,
    pub initialize: Value,
    pub tools: Vec<Value>,
    pub prompts: Option<Vec<Value>>,
    pub resources: Option<Vec<Value>>,
    pub resource_templates: Option<Vec<Value>>,
    pub ttl_ms: Option<u64>,
    pub cache_scope: Option<String>,
    #[serde(default)]
    pub discovery_failed: bool,
}
impl Entry {
    pub fn new(key: String, name: String, initialize: Value) -> Self {
        Self {
            key,
            name,
            initialize,
            cached_at: now_ms(),
            tools: Vec::new(),
            prompts: None,
            resources: None,
            resource_templates: None,
            ttl_ms: None,
            cache_scope: None,
            discovery_failed: false,
        }
    }
    /// The key already covers the protocol version the client asked for. The version
    /// stored in `initialize` is what the server negotiated, which may legitimately
    /// differ (an older server answers with its own), so it is not compared here.
    pub fn valid(&self, key: &str) -> bool {
        let age = now_ms().saturating_sub(self.cached_at);
        self.key == key
            && !self.discovery_failed
            && self.cache_scope.as_deref() != Some("private")
            && age
                < self
                    .ttl_ms
                    .unwrap_or(u64::MAX)
                    .min(30 * 24 * 60 * 60 * 1000)
    }
    /// A recent failed discovery: serve this key transparently instead of retrying yet.
    pub fn failed_recently(&self) -> bool {
        self.discovery_failed && now_ms().saturating_sub(self.cached_at) < DISCOVERY_RETRY_MS
    }
    pub fn listing(&self, method: &str) -> Option<Value> {
        match method {
            "tools/list" => Some(json!({"tools":self.tools})),
            "prompts/list" => Some(json!({"prompts":self.prompts.as_deref().unwrap_or(&[])})),
            "resources/list" => Some(json!({"resources":self.resources.as_deref().unwrap_or(&[])})),
            "resources/templates/list" => {
                Some(json!({"resourceTemplates":self.resource_templates.as_deref().unwrap_or(&[])}))
            }
            _ => None,
        }
    }
}
pub fn advertise(mut init: Value) -> Value {
    if !init["capabilities"].is_object() {
        init["capabilities"] = json!({});
    }
    for cap in ["tools", "prompts", "resources"] {
        if cap == "tools" || init["capabilities"].get(cap).is_some() {
            if !init["capabilities"][cap].is_object() {
                init["capabilities"][cap] = json!({});
            }
            init["capabilities"][cap]["listChanged"] = json!(true);
        }
    }
    init
}
pub fn path(data_dir: &Path, key: &str) -> PathBuf {
    data_dir.join("mcp-cache/v1").join(format!("{key}.json"))
}
pub fn read(data_dir: &Path, key: &str) -> Option<Entry> {
    let entry: Entry = serde_json::from_slice(&fs::read(path(data_dir, key)).ok()?).ok()?;
    (entry.key == key).then_some(entry)
}

/// Per-key advisory lock closes the re-read/rename race between simultaneous proxies.
pub fn save(data_dir: &Path, entry: &Entry) -> io::Result<()> {
    let path = path(data_dir, &entry.key);
    private_dir_all(path.parent().unwrap())?;
    let _guard = FileLock::new(&path.with_extension("lock"))?;
    if let Some(disk) = read(data_dir, &entry.key) {
        if disk.cached_at > entry.cached_at
            || (entry.discovery_failed
                && !disk.discovery_failed
                && disk.cached_at >= entry.cached_at)
        {
            return Ok(());
        }
    }
    if !entry.discovery_failed
        && (entry.cache_scope.as_deref() == Some("private") || entry.ttl_ms == Some(0))
    {
        let _ = fs::remove_file(&path);
        return Ok(());
    }
    atomic_write(&path, &serde_json::to_vec(entry)?)
}
pub fn invalidate(data_dir: &Path, key: &str) -> io::Result<()> {
    let p = path(data_dir, key);
    let _guard = FileLock::new(&p.with_extension("lock"))?;
    match fs::remove_file(p) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}
pub struct FileLock {
    file: fs::File,
}
impl FileLock {
    pub fn new(path: &Path) -> io::Result<Self> {
        let file = private_options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        #[cfg(unix)]
        if unsafe { libc::flock(std::os::fd::AsRawFd::as_raw_fd(&file), libc::LOCK_EX) } != 0 {
            return Err(io::Error::last_os_error());
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::Storage::FileSystem::{LOCKFILE_EXCLUSIVE_LOCK, LockFileEx};
            let mut overlapped = unsafe { std::mem::zeroed() };
            if unsafe {
                LockFileEx(
                    file.as_raw_handle(),
                    LOCKFILE_EXCLUSIVE_LOCK,
                    0,
                    u32::MAX,
                    u32::MAX,
                    &mut overlapped,
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(Self { file })
    }
}
impl Drop for FileLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            libc::flock(std::os::fd::AsRawFd::as_raw_fd(&self.file), libc::LOCK_UN);
        }
    }
}
/// Cached metadata, logs and ownership markers are per-user; so are their directories.
pub fn private_dir_all(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}
pub fn private_options() -> fs::OpenOptions {
    let mut options = fs::OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let tmp = path.with_extension(format!(
        "{}.{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut file = private_options().write(true).create_new(true).open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn dir() -> tempfile::TempDir {
        let root =
            PathBuf::from(std::env::var_os("CARGO_TARGET_DIR").expect("isolated target required"))
                .join("mcp-proxy-tests");
        fs::create_dir_all(&root).unwrap();
        tempfile::tempdir_in(root).unwrap()
    }
    #[test]
    fn identity_is_stable_and_partitions_catalogs() {
        let init = json!({"clientInfo":{"name":"client","version":"2.1"},"protocolVersion":"2025-03-26","capabilities":{"roots":{},"sampling":{}}});
        let path = Path::new("/fake/bin");
        let compute = |cmd, args: &[String], env: Value, npx: Value, init: &Value| {
            key_fields(cmd, args, &env, None, None, path, npx, json!([]), init)
        };
        let original = compute(path, &[], json!({"A":"secret","B":"2"}), Value::Null, &init);
        assert_eq!(
            original,
            compute(path, &[], json!({"B":"2","A":"secret"}), Value::Null, &init)
        );
        let mut changed = init.clone();
        changed["protocolVersion"] = json!("other");
        for other in [
            compute(
                Path::new("/other"),
                &[],
                json!({"A":"secret","B":"2"}),
                Value::Null,
                &init,
            ),
            compute(
                path,
                &["arg".into()],
                json!({"A":"secret","B":"2"}),
                Value::Null,
                &init,
            ),
            compute(
                path,
                &[],
                json!({"A":"different","B":"2"}),
                Value::Null,
                &init,
            ),
            compute(
                path,
                &[],
                json!({"A":"secret","B":"2"}),
                json!({"package":"a","version":"1"}),
                &init,
            ),
            compute(
                path,
                &[],
                json!({"A":"secret","B":"2"}),
                Value::Null,
                &changed,
            ),
        ] {
            assert_ne!(original, other);
        }
        assert_eq!(
            hash(&json!({"z":{"b":1,"a":2},"a":0})),
            hash(&json!({"a":0,"z":{"a":2,"b":1}}))
        );
    }
    #[test]
    fn runtime_options_and_client_minor_do_not_change_key() {
        let root = dir();
        let a = Config::parse(
            ["--server", "demo", "--identity-env", "A,B", "--", "/fake"].map(str::to_owned),
            root.path().into(),
        )
        .unwrap();
        let mut b = a.clone();
        b.mode = crate::Mode::KeepAlive;
        b.idle = std::time::Duration::ZERO;
        b.identity_env.reverse();
        let init = json!({"clientInfo":{"name":"demo","version":"2.1"},"protocolVersion":"v","capabilities":{}});
        let mut other = init.clone();
        other["clientInfo"]["version"] = json!("2.9");
        assert_eq!(
            key(
                &a,
                &init,
                Path::new("/fake"),
                Path::new("/fake"),
                Value::Null
            ),
            key(
                &b,
                &other,
                Path::new("/fake"),
                Path::new("/fake"),
                Value::Null
            )
        );
    }
    #[test]
    fn concurrent_writers_keep_newest_and_private_is_never_persisted() {
        let dir = dir();
        let mut old = Entry::new("key".into(), "demo".into(), json!({"protocolVersion":"v"}));
        old.cached_at = 100;
        let mut new = old.clone();
        new.cached_at = 200;
        std::thread::scope(|s| {
            for e in [&old, &new] {
                let path = dir.path();
                s.spawn(move || save(path, e).unwrap());
            }
        });
        assert_eq!(read(dir.path(), "key").unwrap().cached_at, 200);
        save(dir.path(), &old).unwrap();
        assert_eq!(read(dir.path(), "key").unwrap().cached_at, 200);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(path(dir.path(), "key"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        new.cache_scope = Some("private".into());
        save(dir.path(), &new).unwrap();
        assert!(read(dir.path(), "key").is_none());
        new.cache_scope = None;
        new.ttl_ms = Some(0);
        save(dir.path(), &new).unwrap();
        assert!(read(dir.path(), "key").is_none());
    }
    #[test]
    fn script_arguments_are_part_of_the_identity() {
        let root = dir();
        let script = root.path().join("server.js");
        fs::write(&script, "one").unwrap();
        let config = Config::parse(
            [
                "--server".into(),
                "demo".into(),
                "--".into(),
                "/fake".into(),
                script.to_string_lossy().into_owned(),
                "--flag".into(),
            ],
            root.path().into(),
        )
        .unwrap();
        let init = json!({"clientInfo":{"name":"c","version":"1"},"protocolVersion":"v","capabilities":{}});
        let compute = || {
            key(
                &config,
                &init,
                Path::new("/fake"),
                Path::new("/fake"),
                Value::Null,
            )
        };
        let before = compute();
        assert_eq!(before, compute());
        fs::write(&script, "longer script").unwrap();
        assert_ne!(before, compute());
    }
    #[test]
    fn failed_discovery_is_retried_after_a_while() {
        let mut entry = Entry::new("key".into(), "demo".into(), Value::Null);
        entry.discovery_failed = true;
        assert!(entry.failed_recently());
        entry.cached_at = now_ms() - DISCOVERY_RETRY_MS - 1;
        assert!(!entry.failed_recently());
        entry.discovery_failed = false;
        entry.cached_at = now_ms();
        assert!(!entry.failed_recently());
    }
    #[test]
    fn validity_ttl_protocol_and_discovery_marker() {
        let dir = dir();
        let mut entry = Entry::new("key".into(), "demo".into(), json!({"protocolVersion":"v"}));
        assert!(entry.valid("key"));
        assert!(!entry.valid("other"));
        entry.ttl_ms = Some(0);
        assert!(!entry.valid("key"));
        entry.ttl_ms = None;
        entry.cached_at = now_ms() - 31 * 24 * 60 * 60 * 1000;
        assert!(!entry.valid("key"));
        entry.cached_at = now_ms();
        entry.discovery_failed = true;
        save(dir.path(), &entry).unwrap();
        let disk = read(dir.path(), "key").unwrap();
        assert!(disk.discovery_failed);
        assert!(!disk.valid("key"));
    }
}
