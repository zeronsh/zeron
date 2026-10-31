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
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, SystemTime},
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
                spec.get_or_insert_with(|| value.clone());
                package_flag = true;
                i += 1;
            }
            s if s.starts_with("--package=") => {
                let value = s.strip_prefix("--package=")?.to_owned();
                if value.is_empty() {
                    return None;
                }
                spec.get_or_insert(value);
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
                spec.get_or_insert_with(|| value.clone());
                i += 1;
            }
            s if s.starts_with("--package=") => {
                let value = s.strip_prefix("--package=")?.to_owned();
                if value.is_empty() {
                    return None;
                }
                spec.get_or_insert(value);
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
    requested: Option<String>,
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
        requested: version.map(str::to_owned),
        exact: normalized.filter(|s| exact_version(s)).map(str::to_owned),
    })
}
fn bin_candidates(name: &str, explicit: Option<&str>) -> Vec<String> {
    let mut bins = Vec::new();
    if let Some(explicit) = explicit {
        bins.push(explicit.to_owned());
    }
    if let Some(scoped) = name.strip_prefix('@') {
        if let Some((scope, name)) = scoped.split_once('/') {
            bins.push(name.into());
            bins.push(format!("{scope}-{name}"));
        }
    } else {
        bins.push(name.into());
    }
    bins.dedup();
    bins
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
fn from_npm_cache(npm_cache: &Path, invocation: &Invocation) -> Option<CacheEntry> {
    let spec = package_spec(&invocation.package_spec)?;
    let entries = fs::read_dir(npm_cache.join("_npx")).ok()?;
    let mut installs = Vec::new();
    for entry in entries.flatten() {
        let root = entry.path();
        if !root.is_dir() {
            continue;
        }
        let modules = root.join("node_modules");
        let package = modules.join(&spec.name);
        let manifest = package.join("package.json");
        let Some(pkg) = json_file(&manifest) else {
            continue;
        };
        let Some(version) = pkg["version"].as_str() else {
            continue;
        };
        if spec.exact.as_ref().is_some_and(|v| v != version) {
            continue;
        }
        let direct = json_file(&root.join("package.json")).is_some_and(|p| {
            p["dependencies"][&spec.name]
                .as_str()
                .is_some_and(|requested| {
                    requested == spec.requested.as_deref().unwrap_or("*")
                        || requested == invocation.package_spec
                })
        });
        let mtime = root
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        installs.push((direct, mtime, root, pkg, package, manifest));
    }
    // Exact top-level request first, newest installation second, stable tie break.
    installs.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| b.1.cmp(&a.1))
            .then_with(|| a.2.cmp(&b.2))
    });
    for (_, _, root, pkg, package, manifest) in installs {
        let bins = &pkg["bin"];
        let selected = if let Some(relative) = bins.as_str() {
            Some((
                spec.name.rsplit('/').next().unwrap().to_owned(),
                relative.to_owned(),
            ))
        } else if let Some(bins) = bins.as_object() {
            bin_candidates(&spec.name, invocation.bin_name.as_deref())
                .into_iter()
                .find_map(|name| {
                    bins.get(&name)
                        .and_then(Value::as_str)
                        .map(|p| (name, p.to_owned()))
                })
                .or_else(|| {
                    bins.iter().find_map(|(name, path)| {
                        path.as_str().map(|p| (name.clone(), p.to_owned()))
                    })
                })
        } else {
            None
        };
        let Some((name, relative)) = selected else {
            continue;
        };
        // Explicit bin names are resolved inside .bin, never as filesystem paths.
        if name.contains(['/', '\\']) || name == "." || name == ".." {
            continue;
        }
        let bin_dir = root.join("node_modules/.bin");
        let bin = fs::canonicalize(bin_dir.join(name))
            .or_else(|_| fs::canonicalize(package.join(relative)));
        let Ok(bin) = bin else { continue };
        if !bin.is_file() {
            continue;
        }
        return Some(CacheEntry {
            is_js: detect_js(&bin),
            bin,
            version: pkg["version"].as_str()?.into(),
            resolved_at: cache::now_ms(),
            bin_dir,
            package_json: manifest,
        });
    }
    None
}

/// Resolution runs once per proxy. A missing cache falls back to the original
/// invocation, which performs npm's ordinary installation itself. Daily cache
/// population/refresh belongs to the engine, never to a foreground tool call.
pub async fn resolve(command: &str, args: &[String], data_dir: &Path) -> Option<Resolution> {
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
    )
}
fn resolve_in(
    npm_cache: &Path,
    invocation: &Invocation,
    data_dir: &Path,
    inherited_path: Option<&OsStr>,
) -> Option<Resolution> {
    let spec = package_spec(&invocation.package_spec)?;
    let key = json!([
        invocation.package_spec,
        invocation.bin_name.as_deref().unwrap_or("")
    ])
    .to_string();
    let file = data_dir.join("mcp-cache/npx.json");
    let disk = json_file(&file);
    let cached = disk
        .as_ref()
        .filter(|d| d["version"] == CACHE_VERSION)
        .and_then(|d| serde_json::from_value::<CacheEntry>(d["entries"][&key].clone()).ok());
    let cached = cached.filter(|c| {
        cache::now_ms().saturating_sub(c.resolved_at) < CACHE_TTL_MS
            && c.package_json.starts_with(npm_cache.join("_npx"))
            && c.bin.is_file()
            && c.bin_dir.is_dir()
            && spec.exact.as_ref().is_none_or(|v| v == &c.version)
            && json_file(&c.package_json).is_some_and(|p| p["version"] == c.version)
    });
    // Floating specs must see a newly refreshed npm install on the next session,
    // even when the persisted resolution is less than 24 hours old.
    let entry = if spec.exact.is_none() {
        from_npm_cache(npm_cache, invocation)
    } else {
        cached.or_else(|| from_npm_cache(npm_cache, invocation))
    }?;
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
    // Cache failures must not prevent the equivalent direct executable path.
    let _ = (|| -> std::io::Result<()> {
        fs::create_dir_all(file.parent().unwrap())?;
        let _lock = cache::FileLock::new(&file.with_extension("lock"))?;
        let mut entries: BTreeMap<String, Value> = json_file(&file)
            .filter(|d| d["version"] == CACHE_VERSION)
            .and_then(|d| serde_json::from_value(d["entries"].clone()).ok())
            .unwrap_or_default();
        entries.insert(key, serde_json::to_value(entry)?);
        cache::atomic_write(
            &file,
            &serde_json::to_vec(&json!({"version":CACHE_VERSION,"entries":entries}))?,
        )
    })();
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
    fn package(
        cache: &Path,
        install: &str,
        name: &str,
        version: &str,
        requested: Option<&str>,
        bin: &str,
        contents: &str,
    ) -> PathBuf {
        let root = cache.join("_npx").join(install);
        let pkg = root.join("node_modules").join(name);
        fs::create_dir_all(&pkg).unwrap();
        fs::create_dir_all(root.join("node_modules/.bin")).unwrap();
        fs::write(
            pkg.join("package.json"),
            json!({"name":name,"version":version,"bin":bin}).to_string(),
        )
        .unwrap();
        fs::write(pkg.join(bin), contents).unwrap();
        if let Some(requested) = requested {
            fs::write(
                root.join("package.json"),
                json!({"dependencies":{name:requested}}).to_string(),
            )
            .unwrap();
        }
        pkg.join(bin)
    }
    fn mtime(dir: &Path, offset: u64) {
        let file = fs::File::open(dir).unwrap();
        file.set_times(
            fs::FileTimes::new()
                .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1700000000 + offset)),
        )
        .unwrap();
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
            (
                "npx",
                &["--package=demo", "--package", "other", "demo"],
                Some(("demo", Some("demo"), &[])),
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
    #[test]
    fn exact_pin_wins_over_newer_wrong_scoped_or_unscoped_versions() {
        let dir = temp();
        for name in ["plain", "@scope/pkg"] {
            let right = package(
                dir.path(),
                "correct",
                name,
                "2.0.0",
                None,
                "cli.js",
                "// js",
            );
            package(dir.path(), "newer", name, "1.0.0", None, "cli.js", "// js");
            mtime(&dir.path().join("_npx/correct"), 1);
            mtime(&dir.path().join("_npx/newer"), 10);
            for suffix in ["2.0.0", "v2.0.0", "=2.0.0"] {
                assert_eq!(
                    from_npm_cache(dir.path(), &invocation(&format!("{name}@{suffix}")))
                        .unwrap()
                        .bin,
                    right
                );
            }
        }
    }
    #[test]
    fn latest_prefers_exact_top_level_request_over_transitive_copy() {
        let dir = temp();
        let right = package(
            dir.path(),
            "direct",
            "plain",
            "1.0.0",
            Some("latest"),
            "cli.js",
            "// js",
        );
        package(
            dir.path(),
            "transitive",
            "plain",
            "9.0.0",
            None,
            "cli.js",
            "// js",
        );
        mtime(&dir.path().join("_npx/direct"), 1);
        mtime(&dir.path().join("_npx/transitive"), 10);
        assert_eq!(
            from_npm_cache(dir.path(), &invocation("plain@latest"))
                .unwrap()
                .bin,
            right
        );
        fs::remove_file(dir.path().join("_npx/direct/package.json")).unwrap();
        mtime(&dir.path().join("_npx/direct"), 1);
        assert_eq!(
            from_npm_cache(dir.path(), &invocation("plain@latest"))
                .unwrap()
                .version,
            "9.0.0"
        );
    }
    #[cfg(unix)]
    #[test]
    fn bin_candidates_symlink_realpath_js_and_native() {
        let dir = temp();
        let source = package(
            dir.path(),
            "one",
            "@scope/pkg",
            "1.0.0",
            None,
            "cli",
            "#!/usr/bin/env node\n",
        );
        let pkg = source.parent().unwrap();
        fs::write(
            pkg.join("package.json"),
            json!({"version":"1.0.0","bin":{"scope-pkg":"wrong","explicit":"cli"}}).to_string(),
        )
        .unwrap();
        let target = dir.path().join("linked-native");
        fs::write(&target, b"\x7fELF").unwrap();
        let link = dir.path().join("_npx/one/node_modules/.bin/explicit");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let mut i = invocation("@scope/pkg");
        i.bin_name = Some("explicit".into());
        let result = from_npm_cache(dir.path(), &i).unwrap();
        assert_eq!(result.bin, target);
        assert!(!result.is_js);
        fs::remove_file(link).unwrap();
        assert!(from_npm_cache(dir.path(), &i).unwrap().is_js);
        assert_eq!(bin_candidates("@scope/pkg", None), vec!["pkg", "scope-pkg"]);
    }
    #[cfg(unix)]
    #[test]
    fn resolver_composes_path_uses_first_node_and_never_caches_extra_args() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp();
        let npm = dir.path().join("npm");
        let data = dir.path().join("data");
        let bin = package(&npm, "one", "plain", "1.0.0", Some("*"), "cli.js", "// js");
        let bin_dir = npm.join("_npx/one/node_modules/.bin");
        let node = bin_dir.join("node");
        fs::write(&node, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&node, fs::Permissions::from_mode(0o755)).unwrap();
        let resolved = resolve_in(
            &npm,
            &invocation("plain"),
            &data,
            Some(OsStr::new("/other")),
        )
        .unwrap();
        assert_eq!(resolved.command, node);
        assert_eq!(resolved.args[0], bin.to_str().unwrap());
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
    fn persisted_cache_ttl_missing_binary_wrong_pin_and_malformed_entries() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp();
        let npm = dir.path().join("npm");
        let data = dir.path().join("data");
        let right = package(&npm, "one", "plain", "2.0.0", None, "cli", "#!/bin/sh\n");
        fs::set_permissions(&right, fs::Permissions::from_mode(0o755)).unwrap();
        let i = invocation("plain@2.0.0");
        let key = json!([i.package_spec, ""]).to_string();
        assert_eq!(resolve_in(&npm, &i, &data, None).unwrap().bin, right);
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
            assert_eq!(resolve_in(&npm, &i, &data, None).unwrap().bin, right);
        }
        let mut payload = json_file(&file).unwrap();
        payload["entries"][&key]["version"] = json!("1.0.0");
        fs::write(&file, payload.to_string()).unwrap();
        assert_eq!(resolve_in(&npm, &i, &data, None).unwrap().version, "2.0.0");
        let mut payload = json_file(&file).unwrap();
        payload["entries"][&key]["resolved_at"] = json!(1);
        fs::write(&file, payload.to_string()).unwrap();
        assert!(resolve_in(&npm, &i, &data, None).unwrap().version == "2.0.0");
        fs::remove_file(right).unwrap();
        assert!(resolve_in(&npm, &i, &data, None).is_none());
    }
}
