/*
MIT License

Copyright (c) 2026 Nico Bailon

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
*/
// Rust adaptation of pi-mcp-adapter npx-resolver.ts (3fc01da222891e7588a4450034a7e8a0d2bf3018).
// Adds exact top-level-spec preference, child PATH composition, and per-proxy discovery.
use crate::cache;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha512};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{OsStr, OsString},
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

const CACHE_VERSION: u64 = 2;
const CACHE_TTL_MS: u64 = 24 * 60 * 60 * 1000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invocation {
    pub package_spec: String,
    pub bin_name: Option<String>,
    pub extra_args: Vec<String>,
}

/// Preserve npx's argument separator; npm exec's first separator belongs to npm.
pub fn parse(command: &str, args: &[String]) -> Option<Invocation> {
    let command = Path::new(command).file_name()?.to_str()?;
    match command {
        "npx" => parse_npx(args),
        "npm" if args.first()?.as_str() == "exec" => parse_npm_exec(&args[1..]),
        _ => None,
    }
}
fn parse_npx(args: &[String]) -> Option<Invocation> {
    let separator = args.iter().position(|s| s == "--");
    let before = &args[..separator.unwrap_or(args.len())];
    let after = separator.map(|i| &args[i + 1..]).unwrap_or(&[]);
    let mut positionals = Vec::new();
    let mut spec = None;
    let mut package_flag = false;
    let mut i = 0;
    while i < before.len() {
        let arg = &before[i];
        if !positionals.is_empty() {
            positionals.push(arg.clone());
            i += 1;
            continue;
        }
        match arg.as_str() {
            "-y" | "--yes" => (),
            "-p" | "--package" => {
                let value = before
                    .get(i + 1)
                    .filter(|s| !s.is_empty() && !s.starts_with('-'))?;
                // npm keys several packages' install directory by an ICU-collated sort
                // of all specs; only the single-package layout is reproduced here.
                if spec.replace(value.clone()).is_some() {
                    return None;
                }
                package_flag = true;
                i += 1;
            }
            s if s.starts_with("--package=") => {
                let value = s.strip_prefix("--package=")?.to_owned();
                if value.is_empty() || spec.replace(value).is_some() {
                    return None;
                }
                package_flag = true;
            }
            s if s.starts_with('-') => return None,
            _ => positionals.push(arg.clone()),
        }
        i += 1;
    }
    let first = positionals.first()?.clone();
    let mut extra = positionals[1..].to_vec();
    if !after.is_empty() {
        extra.push("--".into());
        extra.extend_from_slice(after);
    }
    Some(Invocation {
        package_spec: if package_flag { spec? } else { first.clone() },
        bin_name: package_flag.then_some(first),
        extra_args: extra,
    })
}
fn parse_npm_exec(args: &[String]) -> Option<Invocation> {
    let separator = args.iter().position(|s| s == "--")?;
    let before = &args[..separator];
    let after = &args[separator + 1..];
    let mut spec = None;
    let mut i = 0;
    while i < before.len() {
        match before[i].as_str() {
            "-y" | "--yes" => (),
            "--package" => {
                let value = before
                    .get(i + 1)
                    .filter(|s| !s.is_empty() && !s.starts_with('-'))?;
                if spec.replace(value.clone()).is_some() {
                    return None;
                }
                i += 1;
            }
            s if s.starts_with("--package=") => {
                let value = s.strip_prefix("--package=")?.to_owned();
                if value.is_empty() || spec.replace(value).is_some() {
                    return None;
                }
            }
            s if s.starts_with('-') => return None,
            _ => (),
        }
        i += 1;
    }
    Some(Invocation {
        package_spec: spec?,
        bin_name: Some(after.first()?.clone()),
        extra_args: after[1..].to_vec(),
    })
}

#[derive(Debug)]
struct Spec {
    name: String,
    exact: Option<String>,
}
fn exact_version(s: &str) -> bool {
    let base = s.split(['-', '+']).next().unwrap_or("");
    let parts: Vec<_> = base.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        && s[base.len()..]
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-.+".contains(&c))
        && !s.ends_with(['-', '+'])
}
fn package_spec(spec: &str) -> Option<Spec> {
    let spec = spec.trim();
    if spec.is_empty()
        || spec.contains([':', '\\', '#'])
        || spec.starts_with(['.', '/', '~'])
        || spec.chars().any(char::is_whitespace)
    {
        return None;
    }
    let split = if spec.starts_with('@') {
        let slash = spec.find('/')?;
        spec.rfind('@').filter(|i| *i > slash)
    } else {
        spec.find('@')
    };
    let (name, version) = split
        .map(|i| (&spec[..i], Some(&spec[i + 1..])))
        .unwrap_or((spec, None));
    let valid_segment = |s: &str| {
        !s.is_empty()
            && s != "."
            && s != ".."
            && s.bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
    };
    if let Some(scoped) = name.strip_prefix('@') {
        let (scope, name) = scoped.split_once('/')?;
        if !valid_segment(scope) || !valid_segment(name) {
            return None;
        }
    } else if !valid_segment(name) {
        return None;
    }
    if version.is_some_and(|v| v.contains('/') || v.is_empty()) {
        return None;
    }
    let normalized = version
        .map(|v| v.strip_prefix('=').unwrap_or(v))
        .map(|v| v.strip_prefix(['v', 'V']).unwrap_or(v));
    Some(Spec {
        name: name.into(),
        exact: normalized.filter(|s| exact_version(s)).map(str::to_owned),
    })
}
/// npm exec installs a request into `<cache>/_npx/<hash>`, where the hash is the first
/// 16 hex digits of the SHA-512 of the requested specs. For one spec that is the raw
/// spec text exactly as given on the command line (`pkg`, `pkg@latest`, `pkg@1.2.3`).
pub fn install_hash(spec: &str) -> String {
    hex::encode(Sha512::digest(spec.as_bytes()))[..16].to_owned()
}
/// libnpmexec's `getBinFromManifest`: the only bin, or all bins aliasing one file,
/// else the bin named like the unscoped package; anything else is ambiguous (npx
/// fails there, so the proxy must not guess).
fn default_bin(pkg: &Value, name: &str) -> Option<String> {
    let unscoped = name.rsplit('/').next()?;
    match &pkg["bin"] {
        Value::String(_) => Some(unscoped.to_owned()),
        Value::Object(bins) => {
            let targets: BTreeSet<_> = bins.values().filter_map(Value::as_str).collect();
            if !bins.is_empty() && targets.len() == 1 {
                bins.keys().next().cloned()
            } else if bins.contains_key(unscoped) {
                Some(unscoped.to_owned())
            } else {
                None
            }
        }
        _ => None,
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct CacheEntry {
    bin: PathBuf,
    version: String,
    is_js: bool,
    resolved_at: u64,
    // A .bin symlink may point outside this install; retain the original PATH dir.
    bin_dir: PathBuf,
    package_json: PathBuf,
}
#[derive(Clone, Debug)]
pub struct Resolution {
    pub command: PathBuf,
    pub args: Vec<String>,
    pub path: OsString,
    pub bin: PathBuf,
    pub package: String,
    pub version: String,
}
impl Resolution {
    pub fn identity(&self) -> Value {
        json!({"package":self.package,"version":self.version})
    }
}
fn json_file(path: &Path) -> Option<Value> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}
fn detect_js(path: &Path) -> bool {
    if path
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(|s| ["js", "mjs", "cjs"].contains(&s.to_ascii_lowercase().as_str()))
    {
        return true;
    }
    let mut bytes = [0; 256];
    fs::File::open(path)
        .and_then(|mut f| f.read(&mut bytes))
        .ok()
        .is_some_and(|n| {
            let line = String::from_utf8_lossy(&bytes[..n]);
            line.lines()
                .next()
                .is_some_and(|s| s.starts_with("#!") && s.contains("node"))
        })
}
/// The install `npx <spec>` itself would run, and nothing else: the directory named by
/// the hash of the spec, whose top-level request is this package. Another install that
/// merely contains a copy (a different version or tag of the same package, or a
/// dependency hoisted by an unrelated command) is never a substitute.
fn from_npm_cache(npm_cache: &Path, invocation: &Invocation) -> Option<CacheEntry> {
    let spec = package_spec(&invocation.package_spec)?;
    let root = npm_cache
        .join("_npx")
        .join(install_hash(&invocation.package_spec));
    let root_json = json_file(&root.join("package.json"))?;
    if let Some(recorded) = root_json["_npx"]["packages"].as_array() {
        if recorded.len() != 1 || recorded[0] != invocation.package_spec.as_str() {
            return None;
        }
    }
    root_json["dependencies"].get(&spec.name)?;
    let manifest = root
        .join("node_modules")
        .join(&spec.name)
        .join("package.json");
    let pkg = json_file(&manifest)?;
    let version = pkg["version"].as_str()?;
    if spec.exact.as_ref().is_some_and(|v| v != version) {
        return None;
    }
    let name = match &invocation.bin_name {
        Some(name) => name.clone(),
        None => default_bin(&pkg, &spec.name)?,
    };
    // Bins are names inside .bin, never filesystem paths.
    if name.is_empty() || name.contains(['/', '\\']) || name == "." || name == ".." {
        return None;
    }
    let bin_dir = root.join("node_modules/.bin");
    let bin = fs::canonicalize(bin_dir.join(&name)).ok()?;
    if !bin.is_file() {
        return None;
    }
    Some(CacheEntry {
        is_js: detect_js(&bin),
        bin,
        version: version.into(),
        resolved_at: cache::now_ms(),
        bin_dir,
        package_json: manifest,
    })
}

/// npm exec prefers an installation it finds before it looks at its cache: the project
/// tree (the current directory and its ancestors) and, for a bare command, a global
/// install. If any exists npx would run that one, so the proxy leaves the decision to npx.
fn preempted(invocation: &Invocation, name: &str, cwd: &Path, path: Option<&OsStr>) -> bool {
    let bare = invocation.bin_name.is_none();
    let raw = &invocation.package_spec;
    for dir in cwd.ancestors() {
        let modules = dir.join("node_modules");
        if modules.join(name).join("package.json").exists()
            || (bare && modules.join(".bin").join(raw).symlink_metadata().is_ok())
        {
            return true;
        }
        if bare {
            // The project's own bins count as local packages (`hasPkgBin`).
            if let Some(own) = json_file(&dir.join("package.json")) {
                if own["bin"].get(raw.as_str()).is_some()
                    || (own["bin"].is_string()
                        && own["name"].as_str().and_then(|n| n.rsplit('/').next())
                            == Some(raw.as_str()))
                {
                    return true;
                }
            }
        }
    }
    bare && std::env::split_paths(path.unwrap_or_default()).any(|bin| {
        bin.parent().is_some_and(|prefix| {
            prefix
                .join("lib/node_modules")
                .join(name)
                .join("package.json")
                .exists()
        })
    })
}

/// Resolution runs once per proxy. A missing cache falls back to the original
/// invocation, which performs npm's ordinary installation itself. Daily cache
/// population/refresh belongs to the engine, never to a foreground tool call.
pub async fn resolve(
    command: &str,
    args: &[String],
    data_dir: &Path,
    cwd: &Path,
) -> Option<Resolution> {
    if cfg!(windows) {
        return None;
    }
    let invocation = parse(command, args)?;
    package_spec(&invocation.package_spec)?;
    let npm_cache = if let Some(cache) = std::env::var_os("ZERON_NPM_CACHE")
        .or_else(|| std::env::var_os("NPM_CONFIG_CACHE"))
        .or_else(|| std::env::var_os("npm_config_cache"))
    {
        PathBuf::from(cache)
    } else {
        let mut cmd = tokio::process::Command::new("npm");
        cmd.args(["config", "get", "cache"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(unix)]
        cmd.process_group(0);
        #[cfg(target_os = "linux")]
        unsafe {
            let parent = std::process::id();
            cmd.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::getppid() != parent as i32 {
                    return Err(std::io::Error::other("proxy exited before npm config"));
                }
                Ok(())
            });
        }
        let output = tokio::time::timeout(Duration::from_secs(2), cmd.output())
            .await
            .ok()?
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let path = String::from_utf8(output.stdout).ok()?.trim().to_owned();
        if path.is_empty() {
            return None;
        }
        path.into()
    };
    resolve_in(
        &npm_cache,
        &invocation,
        data_dir,
        std::env::var_os("PATH").as_deref(),
        cwd,
    )
}
fn store(file: &Path, key: &str, entry: &CacheEntry) {
    // Cache failures must not prevent the equivalent direct executable path.
    let _ = (|| -> std::io::Result<()> {
        cache::private_dir_all(file.parent().unwrap())?;
        let _lock = cache::FileLock::new(&file.with_extension("lock"))?;
        let mut entries: BTreeMap<String, Value> = json_file(file)
            .filter(|d| d["version"] == CACHE_VERSION)
            .and_then(|d| serde_json::from_value(d["entries"].clone()).ok())
            .unwrap_or_default();
        entries.insert(key.to_owned(), serde_json::to_value(entry)?);
        cache::atomic_write(
            file,
            &serde_json::to_vec(&json!({"version":CACHE_VERSION,"entries":entries}))?,
        )
    })();
}
fn resolve_in(
    npm_cache: &Path,
    invocation: &Invocation,
    data_dir: &Path,
    inherited_path: Option<&OsStr>,
    cwd: &Path,
) -> Option<Resolution> {
    let spec = package_spec(&invocation.package_spec)?;
    if preempted(invocation, &spec.name, cwd, inherited_path) {
        return None;
    }
    let key = json!([
        invocation.package_spec,
        invocation.bin_name.as_deref().unwrap_or("")
    ])
    .to_string();
    let file = data_dir.join("mcp-cache/npx.json");
    let stored = json_file(&file)
        .filter(|d| d["version"] == CACHE_VERSION)
        .and_then(|d| serde_json::from_value::<CacheEntry>(d["entries"][&key].clone()).ok());
    let expired = |c: &CacheEntry| cache::now_ms().saturating_sub(c.resolved_at) >= CACHE_TTL_MS;
    let mut entry = match &stored {
        // A pinned version cannot go stale; reuse it while the files are still as recorded.
        Some(c)
            if spec.exact.is_some()
                && !expired(c)
                && c.package_json.starts_with(npm_cache.join("_npx"))
                && c.bin.is_file()
                && c.bin_dir.is_dir()
                && spec.exact.as_ref().is_some_and(|v| v == &c.version)
                && json_file(&c.package_json).is_some_and(|p| p["version"] == c.version) =>
        {
            c.clone()
        }
        _ => from_npm_cache(npm_cache, invocation)?,
    };
    if spec.exact.is_none() {
        // npx revalidates a tag or range against the registry on every run; the proxy
        // never talks to it. So a floating resolution is served for at most one TTL per
        // version. Once it expires the original invocation runs (npx refreshes its own
        // cache), and the clock restarts, so servers still follow `@latest` daily.
        if let Some(previous) = stored
            .as_ref()
            .filter(|c| c.version == entry.version && c.bin == entry.bin)
        {
            if expired(previous) {
                store(&file, &key, &entry);
                return None;
            }
            entry.resolved_at = previous.resolved_at;
        }
    }
    let path = std::env::join_paths(
        std::iter::once(entry.bin_dir.clone())
            .chain(std::env::split_paths(inherited_path.unwrap_or_default())),
    )
    .ok()?;
    let command = if entry.is_js {
        cache::executable("node", Some(&path))
    } else {
        entry.bin.clone()
    };
    if !command.is_file() {
        return None;
    }
    let mut args = Vec::new();
    if entry.is_js {
        args.push(entry.bin.to_string_lossy().into_owned());
    }
    args.extend(invocation.extra_args.clone());
    let result = Resolution {
        command,
        args,
        path,
        bin: entry.bin.clone(),
        package: invocation.package_spec.clone(),
        version: entry.version.clone(),
    };
    store(&file, &key, &entry);
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn strings(s: &[&str]) -> Vec<String> {
        s.iter().map(|s| (*s).into()).collect()
    }
    fn invocation(s: &str) -> Invocation {
        Invocation {
            package_spec: s.into(),
            bin_name: None,
            extra_args: vec!["--token=secret-value".into()],
        }
    }
    fn temp() -> tempfile::TempDir {
        let root =
            PathBuf::from(std::env::var_os("CARGO_TARGET_DIR").expect("isolated target required"))
                .join("mcp-proxy-tests");
        fs::create_dir_all(&root).unwrap();
        tempfile::tempdir_in(root).unwrap()
    }
    #[test]
    fn parse_table_preserves_pi_npx_and_npm_exec_semantics() {
        let rows: &[(&str, &[&str], Option<(&str, Option<&str>, &[&str])>)] = &[
            (
                "npx",
                &["-y", "demo", "--stdio"],
                Some(("demo", None, &["--stdio"])),
            ),
            (
                "npx",
                &[
                    "--yes",
                    "dotenv-cli",
                    "--",
                    "npx",
                    "--yes",
                    "@upstash/context7-mcp",
                ],
                Some((
                    "dotenv-cli",
                    None,
                    &["--", "npx", "--yes", "@upstash/context7-mcp"],
                )),
            ),
            (
                "npx",
                &["dotenv-cli", "github-mcp-server", "stdio"],
                Some(("dotenv-cli", None, &["github-mcp-server", "stdio"])),
            ),
            (
                "npx",
                &["-p", "@scope/pkg@1.2.3", "pkg-bin", "--x"],
                Some(("@scope/pkg@1.2.3", Some("pkg-bin"), &["--x"])),
            ),
            // Several packages share an install directory keyed by a collated sort.
            (
                "npx",
                &["--package=demo", "--package", "other", "demo"],
                None,
            ),
            ("npx", &["-p", "demo", "-p", "demo", "demo"], None),
            (
                "npm",
                &["exec", "--package", "a", "--package=b", "--", "a"],
                None,
            ),
            ("npx", &["demo", "--"], Some(("demo", None, &[]))),
            ("npx", &["--package", "demo", "--", "demo"], None),
            ("npx", &["--quiet", "demo"], None),
            ("npx", &["--package="], None),
            ("npx", &["-p", "--yes"], None),
            ("npx", &[], None),
            (
                "npm",
                &[
                    "exec",
                    "--yes",
                    "--package",
                    "demo",
                    "--",
                    "demo-bin",
                    "arg",
                    "--",
                    "tail",
                ],
                Some(("demo", Some("demo-bin"), &["arg", "--", "tail"])),
            ),
            (
                "npm",
                &["exec", "--package=demo", "--", "demo"],
                Some(("demo", Some("demo"), &[])),
            ),
            ("npm", &["exec", "--package", "demo", "demo"], None),
            (
                "npm",
                &["exec", "--offline", "--package", "demo", "--", "demo"],
                None,
            ),
            ("npm", &["exec", "-p", "demo", "--", "demo"], None),
            ("npm", &["run", "demo"], None),
            ("node", &["demo"], None),
        ];
        for (command, args, expected) in rows {
            let expected = expected.map(|(package, bin, extra)| Invocation {
                package_spec: package.into(),
                bin_name: bin.map(str::to_owned),
                extra_args: strings(extra),
            });
            assert_eq!(
                parse(command, &strings(args)),
                expected,
                "{command} {args:?}"
            );
        }
    }
    #[test]
    fn package_specs_reject_local_git_and_parse_exact_spelling() {
        for bad in [
            "",
            "./local",
            "/local",
            "../pkg",
            "file:foo",
            "git+https://git/a",
            "github:a/b",
            "a/b",
            "@scope/../bad",
            "@scope/pkg@github:a/b",
        ] {
            assert!(package_spec(bad).is_none(), "{bad}");
        }
        for spec in [
            "plain@2.0.0",
            "plain@v2.0.0",
            "plain@=2.0.0",
            "@scope/pkg@=v2.0.0",
        ] {
            assert_eq!(package_spec(spec).unwrap().exact.as_deref(), Some("2.0.0"));
        }
        assert_eq!(
            package_spec("plain@1.2.3-pre.2+build")
                .unwrap()
                .exact
                .as_deref(),
            Some("1.2.3-pre.2+build")
        );
        assert!(package_spec("plain@latest").unwrap().exact.is_none());
    }
    /// What `npx <spec>` leaves behind: `_npx/<hash of spec>` with the package requested at
    /// the top level and one `.bin` link per bin, as npm creates them.
    fn install(cache: &Path, spec: &str, name: &str, version: &str, bin: Value) -> PathBuf {
        let root = cache.join("_npx").join(install_hash(spec));
        let pkg = root.join("node_modules").join(name);
        let bins = root.join("node_modules/.bin");
        fs::create_dir_all(&pkg).unwrap();
        fs::create_dir_all(&bins).unwrap();
        fs::write(
            root.join("package.json"),
            json!({"dependencies":{name:format!("^{version}")},"_npx":{"packages":[spec]}})
                .to_string(),
        )
        .unwrap();
        fs::write(
            pkg.join("package.json"),
            json!({"name":name,"version":version,"bin":bin}).to_string(),
        )
        .unwrap();
        let links: Vec<(String, String)> = match &bin {
            Value::String(path) => vec![(name.rsplit('/').next().unwrap().into(), path.clone())],
            Value::Object(m) => m
                .iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap().into()))
                .collect(),
            _ => vec![],
        };
        for (link, path) in links {
            fs::write(pkg.join(&path), "// js").unwrap();
            #[cfg(unix)]
            let _ =
                std::os::unix::fs::symlink(Path::new("..").join(name).join(&path), bins.join(link));
        }
        root
    }
    /// A PATH directory whose `node` is an executable stand-in, so JS bins resolve anywhere.
    #[cfg(unix)]
    fn node_dir(dir: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let bin = dir.join("fake-node-bin");
        fs::create_dir_all(&bin).unwrap();
        fs::write(bin.join("node"), "#!/bin/sh\n").unwrap();
        fs::set_permissions(bin.join("node"), fs::Permissions::from_mode(0o755)).unwrap();
        bin
    }
    #[cfg(unix)]
    fn resolve_here(cache: &Path, i: &Invocation, data: &Path) -> Option<Resolution> {
        let path = std::env::join_paths([node_dir(cache)]).unwrap();
        resolve_in(cache, i, data, Some(&path), cache)
    }

    #[test]
    fn install_hash_matches_what_npm_writes_to_the_cache() {
        // Directory names observed in a real npm 12 `_npx` cache.
        for (spec, dir) in [
            ("chrome-devtools-mcp@1.11.0", "053a863a8198098a"),
            ("@playwright/mcp@latest", "9833c18b2d85bc59"),
            ("prettier", "b388654678d519d9"),
        ] {
            assert_eq!(install_hash(spec), dir);
        }
    }
    #[cfg(unix)]
    #[test]
    fn exact_pin_uses_its_own_install_never_a_newer_copy_or_another_spelling() {
        let dir = temp();
        for name in ["plain", "@scope/pkg"] {
            let pinned = format!("{name}@2.0.0");
            let right = install(dir.path(), &pinned, name, "2.0.0", json!("cli.js"));
            install(
                dir.path(),
                &format!("{name}@3.0.0"),
                name,
                "3.0.0",
                json!("cli.js"),
            );
            let found = from_npm_cache(dir.path(), &invocation(&pinned)).unwrap();
            assert_eq!(found.version, "2.0.0");
            assert!(found.bin.starts_with(fs::canonicalize(right).unwrap()));
            // npm keys the directory by the raw spelling; a different spelling is a
            // different install that npx would create.
            assert!(from_npm_cache(dir.path(), &invocation(&format!("{name}@v2.0.0"))).is_none());
        }
    }
    #[cfg(unix)]
    #[test]
    fn floating_specs_never_borrow_another_requests_install() {
        let dir = temp();
        install(dir.path(), "plain@1.0.0", "plain", "1.0.0", json!("cli.js"));
        // Same package present at the top level of a *different* request's install.
        for spec in ["plain", "plain@latest", "plain@^1", "plain@next"] {
            assert!(
                from_npm_cache(dir.path(), &invocation(spec)).is_none(),
                "{spec}"
            );
        }
        install(
            dir.path(),
            "plain@latest",
            "plain",
            "9.0.0",
            json!("cli.js"),
        );
        assert_eq!(
            from_npm_cache(dir.path(), &invocation("plain@latest"))
                .unwrap()
                .version,
            "9.0.0"
        );
        assert_eq!(
            from_npm_cache(dir.path(), &invocation("plain@1.0.0"))
                .unwrap()
                .version,
            "1.0.0"
        );
        assert!(from_npm_cache(dir.path(), &invocation("plain@^1")).is_none());
    }
    #[cfg(unix)]
    #[test]
    fn hoisted_dependencies_and_foreign_directories_are_not_requests() {
        let dir = temp();
        // `other` installed `plain` as a hoisted dependency.
        let root = install(dir.path(), "other", "other", "1.0.0", json!("cli.js"));
        let hoisted = root.join("node_modules/plain");
        fs::create_dir_all(&hoisted).unwrap();
        fs::write(
            hoisted.join("package.json"),
            r#"{"name":"plain","version":"7.0.0","bin":"cli.js"}"#,
        )
        .unwrap();
        assert!(from_npm_cache(dir.path(), &invocation("plain")).is_none());
        // A directory with the right name that does not request the package at the top.
        let wrong = dir.path().join("_npx").join(install_hash("plain"));
        fs::create_dir_all(wrong.join("node_modules")).unwrap();
        fs::rename(&hoisted, wrong.join("node_modules/plain")).unwrap();
        fs::write(
            wrong.join("package.json"),
            r#"{"dependencies":{"other":"*"}}"#,
        )
        .unwrap();
        assert!(from_npm_cache(dir.path(), &invocation("plain")).is_none());
        // npm's own record of the request disagrees with the directory name.
        install(dir.path(), "plain@4.0.0", "plain", "4.0.0", json!("cli.js"));
        let recorded = dir.path().join("_npx").join(install_hash("plain@4.0.0"));
        fs::write(
            recorded.join("package.json"),
            r#"{"dependencies":{"plain":"^4.0.0"},"_npx":{"packages":["plain@5.0.0"]}}"#,
        )
        .unwrap();
        assert!(from_npm_cache(dir.path(), &invocation("plain@4.0.0")).is_none());
    }
    #[cfg(unix)]
    #[test]
    fn bin_choice_follows_libnpmexec_and_never_guesses() {
        let dir = temp();
        let bin = |spec: &str, bin: Value, explicit: Option<&str>| {
            install(dir.path(), spec, spec, "1.0.0", bin);
            let mut i = invocation(spec);
            i.bin_name = explicit.map(str::to_owned);
            from_npm_cache(dir.path(), &i).map(|e| e.bin.file_name().unwrap().to_owned())
        };
        let file = |name: &str| Some(std::ffi::OsString::from(name));
        assert_eq!(bin("one", json!({"tool":"t.js"}), None), file("t.js"));
        assert_eq!(
            bin("alias", json!({"a":"x.js","b":"x.js"}), None),
            file("x.js")
        );
        assert_eq!(
            bin("many", json!({"many":"m.js","other":"o.js"}), None),
            file("m.js")
        );
        // Several bins, none named like the package: npx fails, the proxy must not pick.
        assert_eq!(bin("ambiguous", json!({"a":"a.js","b":"b.js"}), None), None);
        assert_eq!(bin("string", json!("s.js"), None), file("s.js"));
        // `npx -p pkg cmd` runs `.bin/cmd`, including bins that belong to a dependency.
        assert_eq!(bin("pkg", json!({"pkg":"p.js"}), Some("pkg")), file("p.js"));
        let root = dir.path().join("_npx").join(install_hash("pkg"));
        fs::write(root.join("node_modules/dep.js"), "// js").unwrap();
        std::os::unix::fs::symlink("../dep.js", root.join("node_modules/.bin/dep-cli")).unwrap();
        let mut i = invocation("pkg");
        i.bin_name = Some("dep-cli".into());
        assert_eq!(
            from_npm_cache(dir.path(), &i)
                .unwrap()
                .bin
                .file_name()
                .unwrap(),
            "dep.js"
        );
        for bad in ["missing", "../escape", "a/b", ".."] {
            i.bin_name = Some(bad.into());
            assert!(from_npm_cache(dir.path(), &i).is_none(), "{bad}");
        }
    }
    #[cfg(unix)]
    #[test]
    fn bin_symlink_realpath_js_and_native() {
        let dir = temp();
        let root = install(
            dir.path(),
            "@scope/pkg",
            "@scope/pkg",
            "1.0.0",
            json!({"pkg":"cli"}),
        );
        let target = dir.path().join("linked-native");
        fs::write(&target, b"\x7fELF").unwrap();
        let link = root.join("node_modules/.bin/pkg");
        fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let result = from_npm_cache(dir.path(), &invocation("@scope/pkg")).unwrap();
        assert_eq!(result.bin, target);
        assert!(!result.is_js);
        fs::write(&target, "#!/usr/bin/env node\n").unwrap();
        assert!(
            from_npm_cache(dir.path(), &invocation("@scope/pkg"))
                .unwrap()
                .is_js
        );
    }
    #[cfg(unix)]
    #[test]
    fn project_and_global_installs_are_left_to_npx() {
        let dir = temp();
        let npm = dir.path().join("npm");
        let data = dir.path().join("data");
        install(&npm, "plain", "plain", "1.0.0", json!("cli.js"));
        install(&npm, "plain@1.0.0", "plain", "1.0.0", json!({"x":"cli.js"}));
        let project = dir.path().join("project");
        let sub = project.join("packages/app");
        fs::create_dir_all(&sub).unwrap();
        let bare = invocation("plain");
        let mut with_bin = invocation("plain@1.0.0");
        with_bin.bin_name = Some("x".into());
        let node = node_dir(dir.path());
        let run = |i: &Invocation, cwd: &Path, extra: Option<&OsStr>| {
            let path = std::env::join_paths(
                extra
                    .iter()
                    .flat_map(|p| std::env::split_paths(p))
                    .chain([node.clone()]),
            )
            .unwrap();
            resolve_in(&npm, i, &data, Some(&path), cwd).is_some()
        };
        assert!(run(&bare, &sub, None));
        // A project dependency anywhere above the working directory wins in npx.
        fs::create_dir_all(project.join("node_modules/plain")).unwrap();
        fs::write(project.join("node_modules/plain/package.json"), "{}").unwrap();
        assert!(!run(&bare, &sub, None));
        assert!(!run(&with_bin, &sub, None));
        fs::remove_dir_all(project.join("node_modules")).unwrap();
        // So does a local bin of the command's name, but only for a bare command.
        fs::create_dir_all(project.join("node_modules/.bin")).unwrap();
        fs::write(project.join("node_modules/.bin/plain"), "").unwrap();
        assert!(!run(&bare, &sub, None));
        fs::remove_dir_all(project.join("node_modules")).unwrap();
        // The project's own bin counts as a local package.
        fs::write(
            project.join("package.json"),
            r#"{"name":"me","bin":{"plain":"x.js"}}"#,
        )
        .unwrap();
        assert!(!run(&bare, &sub, None));
        fs::remove_file(project.join("package.json")).unwrap();
        // A global install on PATH (<prefix>/bin next to <prefix>/lib/node_modules).
        let prefix = dir.path().join("global");
        fs::create_dir_all(prefix.join("bin")).unwrap();
        fs::create_dir_all(prefix.join("lib/node_modules/plain")).unwrap();
        fs::write(prefix.join("lib/node_modules/plain/package.json"), "{}").unwrap();
        let path = std::env::join_paths([prefix.join("bin")]).unwrap();
        assert!(!run(&bare, &sub, Some(&path)));
        assert!(run(&with_bin, &sub, Some(&path)));
        assert!(run(&bare, &sub, None));
    }
    #[cfg(unix)]
    #[test]
    fn resolver_composes_path_uses_first_node_and_never_caches_extra_args() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp();
        let npm = dir.path().join("npm");
        let data = dir.path().join("data");
        let root = install(&npm, "plain", "plain", "1.0.0", json!("cli.js"));
        let bin_dir = root.join("node_modules/.bin");
        let node = bin_dir.join("node");
        fs::write(&node, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&node, fs::Permissions::from_mode(0o755)).unwrap();
        let resolved = resolve_in(
            &npm,
            &invocation("plain"),
            &data,
            Some(OsStr::new("/other")),
            dir.path(),
        )
        .unwrap();
        assert_eq!(resolved.command, node);
        assert!(resolved.args[0].ends_with("plain/cli.js"));
        assert_eq!(resolved.args[1], "--token=secret-value");
        assert_eq!(std::env::split_paths(&resolved.path).next(), Some(bin_dir));
        assert!(
            !fs::read_to_string(data.join("mcp-cache/npx.json"))
                .unwrap()
                .contains("secret-value")
        );
    }
    #[cfg(unix)]
    #[test]
    fn persisted_cache_missing_binary_wrong_pin_and_malformed_entries() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp();
        let npm = dir.path().join("npm");
        let data = dir.path().join("data");
        let root = install(&npm, "plain@2.0.0", "plain", "2.0.0", json!("cli.js"));
        let right = fs::canonicalize(root.join("node_modules/plain/cli.js")).unwrap();
        fs::set_permissions(&right, fs::Permissions::from_mode(0o755)).unwrap();
        let i = invocation("plain@2.0.0");
        let key = json!([i.package_spec, ""]).to_string();
        assert_eq!(resolve_here(&npm, &i, &data).unwrap().bin, right);
        let file = data.join("mcp-cache/npx.json");
        for poison in [
            json!({"bin":123}),
            json!({"bin":"/missing","version":"2.0.0","is_js":false,"resolved_at":cache::now_ms(),"bin_dir":"/missing","package_json":"/missing"}),
        ] {
            fs::write(
                &file,
                json!({"version":2,"entries":{&key:poison}}).to_string(),
            )
            .unwrap();
            assert_eq!(resolve_here(&npm, &i, &data).unwrap().bin, right);
        }
        let mut payload = json_file(&file).unwrap();
        payload["entries"][&key]["version"] = json!("1.0.0");
        fs::write(&file, payload.to_string()).unwrap();
        assert_eq!(resolve_here(&npm, &i, &data).unwrap().version, "2.0.0");
        let mut payload = json_file(&file).unwrap();
        payload["entries"][&key]["resolved_at"] = json!(1);
        fs::write(&file, payload.to_string()).unwrap();
        assert!(resolve_here(&npm, &i, &data).unwrap().version == "2.0.0");
        fs::remove_file(&right).unwrap();
        assert!(resolve_here(&npm, &i, &data).is_none());
    }
    #[cfg(unix)]
    #[test]
    fn floating_resolutions_are_handed_back_to_npx_once_a_day() {
        let dir = temp();
        let npm = dir.path().join("npm");
        let data = dir.path().join("data");
        install(&npm, "plain@latest", "plain", "1.0.0", json!("cli.js"));
        let i = invocation("plain@latest");
        let key = json!([i.package_spec, ""]).to_string();
        let file = data.join("mcp-cache/npx.json");
        let stamp = |file: &Path| json_file(file).unwrap()["entries"][&key]["resolved_at"].clone();
        assert!(resolve_here(&npm, &i, &data).is_some());
        let first = stamp(&file);
        std::thread::sleep(Duration::from_millis(5));
        assert!(resolve_here(&npm, &i, &data).is_some());
        assert_eq!(
            stamp(&file),
            first,
            "serving a version must not extend its lifetime"
        );
        let mut payload = json_file(&file).unwrap();
        payload["entries"][&key]["resolved_at"] = json!(cache::now_ms() - CACHE_TTL_MS - 1);
        fs::write(&file, payload.to_string()).unwrap();
        // Expired: npx runs once (and refreshes its own cache), then the clock restarts.
        assert!(resolve_here(&npm, &i, &data).is_none());
        assert!(resolve_here(&npm, &i, &data).is_some());
        // A newer version that appeared meanwhile is served immediately.
        let mut payload = json_file(&file).unwrap();
        payload["entries"][&key]["resolved_at"] = json!(1);
        fs::write(&file, payload.to_string()).unwrap();
        install(&npm, "plain@latest", "plain", "2.0.0", json!("cli.js"));
        assert_eq!(resolve_here(&npm, &i, &data).unwrap().version, "2.0.0");
    }
}
