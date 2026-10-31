//! Repair a global npm install of Codex that lacks its native binary.
//!
//! `@openai/codex` ships the binary as an optional dependency
//! (`@openai/codex-linux-x64` and friends). An install made without optional
//! dependencies — some machine images — leaves a `codex` whose launcher
//! throws "Missing optional dependency" on every start, so no Codex run can
//! begin. Reinstalling that same version with optional dependencies fixes it.

use std::path::{Path, PathBuf};

use crate::process::{Command, Stdio};

const MISSING: &str = "Missing optional dependency";
const PACKAGE: &str = "@openai/codex";
/// A `--version` (or `sudo -n true`) that takes longer than this is hung:
/// the repair runs before a Codex spawn, which must not wait on it forever.
const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// What [`repair`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Repair {
    /// Codex starts (or isn't installed, or fails some other way): nothing
    /// for a reinstall to fix.
    NotNeeded,
    /// Codex was missing its binary and starts now.
    Repaired { version: String },
}

/// Check the installed Codex and reinstall its npm package when its binary
/// is missing. `Err` when the reinstall ran and Codex still can't start.
pub async fn repair() -> Result<Repair, String> {
    let Some(codex) = super::resolve_codex_executable() else {
        return Ok(Repair::NotNeeded);
    };
    let Some(npm) = crate::adapter_install::find_npm() else {
        return Ok(Repair::NotNeeded);
    };
    // A root-owned global prefix (a machine image's /usr/local) needs sudo;
    // only without a password prompt.
    let sudo = (std::env::consts::OS != "windows")
        .then(|| crate::executable::find_on_paths("sudo", Vec::new()))
        .flatten();
    let sudo = match sudo {
        Some(sudo) if succeeds(&sudo, &["-n", "true"]).await => Some(sudo),
        _ => None,
    };
    repair_with(&codex, &npm, sudo.as_deref()).await
}

pub(crate) async fn repair_with(
    codex: &Path,
    npm: &Path,
    sudo: Option<&Path>,
) -> Result<Repair, String> {
    match probe(codex).await {
        Probe::Starts | Probe::Fails(_) => return Ok(Repair::NotNeeded),
        Probe::MissingBinary => {}
    }
    let version = installed_version(codex).unwrap_or_else(|| "latest".into());
    let spec = format!("{PACKAGE}@{version}");
    // Reinstall into the prefix that owns this Codex, whichever npm runs it:
    // another Node's npm would otherwise add a second copy elsewhere.
    let prefix = global_prefix(codex).map(|prefix| prefix.to_string_lossy().into_owned());
    let mut args = vec!["install", "-g"];
    if let Some(prefix) = &prefix {
        args.extend(["--prefix", prefix.as_str()]);
    }
    args.extend([
        "--include=optional",
        "--no-audit",
        "--no-fund",
        spec.as_str(),
    ]);
    let (program, lead) = npm_launch(npm);
    let args: Vec<OsArg> = args
        .iter()
        .map(|arg| OsArg::Str((*arg).to_string()))
        .collect();
    let mut output = run_args(&program, &lead, &args).await?;
    if !output.status.success()
        && let Some(sudo) = sudo
        && permission_denied(&output)
    {
        let mut with_npm = vec![OsArg::Path(program.clone())];
        with_npm.extend(lead.iter().cloned());
        with_npm.extend(args.iter().cloned());
        output = run_args(sudo, &[OsArg::Str("-n".into())], &with_npm).await?;
    }
    if !output.status.success() {
        return Err(format!(
            "reinstalling {spec} failed: {}",
            tail(&String::from_utf8_lossy(&output.stderr))
        ));
    }
    match probe(codex).await {
        Probe::Starts => Ok(Repair::Repaired { version }),
        Probe::MissingBinary => Err(format!("{spec} still has no native binary")),
        Probe::Fails(error) => Err(format!("Codex still can't start: {error}")),
    }
}

enum Probe {
    Starts,
    MissingBinary,
    Fails(String),
}

async fn probe(codex: &Path) -> Probe {
    let output = tokio::time::timeout(PROBE_TIMEOUT, command(codex).arg("--version").output())
        .await
        .unwrap_or_else(|_| Err(std::io::Error::other("`codex --version` timed out")));
    match output {
        Ok(out) if out.status.success() => Probe::Starts,
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            if stderr.contains(MISSING) {
                Probe::MissingBinary
            } else {
                Probe::Fails(tail(&stderr))
            }
        }
        Err(err) => Probe::Fails(err.to_string()),
    }
}

/// The version of the npm package behind `codex`: on Unix the bin entry is a
/// symlink into `…/node_modules/@openai/codex/bin/codex.js`; on Windows it
/// is a `codex.cmd` shim beside a `node_modules` folder holding the package
/// (`%APPDATA%\npm\node_modules\@openai\codex`).
fn installed_version(codex: &Path) -> Option<String> {
    let linked = std::fs::canonicalize(codex)
        .ok()
        .and_then(|script| Some(script.parent()?.parent()?.join("package.json")));
    let beside = codex.parent().map(|dir| {
        dir.join("node_modules")
            .join("@openai")
            .join("codex")
            .join("package.json")
    });
    linked
        .into_iter()
        .chain(beside)
        .find_map(|manifest| package_version(&manifest))
}

fn package_version(manifest: &Path) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(manifest).ok()?).ok()?;
    if value["name"] != PACKAGE {
        return None;
    }
    let version = value["version"].as_str()?;
    version
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'))
        .then(|| version.to_string())
}

/// The npm prefix the package behind `codex` was installed into globally:
/// `<prefix>/lib/node_modules/@openai/codex` (Unix) or
/// `<prefix>/node_modules/@openai/codex` (Windows), in its plain form: npm
/// does not take the `\\?\` verbatim paths Windows canonicalizes to.
fn global_prefix(codex: &Path) -> Option<PathBuf> {
    let script = super::plain_executable(std::fs::canonicalize(codex).ok()?);
    if let Some(node_modules) = script
        .ancestors()
        .find(|dir| dir.file_name().is_some_and(|name| name == "node_modules"))
    {
        let parent = node_modules.parent()?;
        return Some(match parent.file_name() {
            Some(name) if name == "lib" && !cfg!(windows) => parent.parent()?.to_path_buf(),
            _ => parent.to_path_buf(),
        });
    }
    // Windows: `<prefix>\codex.cmd` is a shim file, not a link into the
    // package, which sits beside it in `<prefix>\node_modules\@openai\codex`.
    let dir = script.parent()?;
    dir.join("node_modules")
        .join("@openai")
        .join("codex")
        .is_dir()
        .then(|| dir.to_path_buf())
}

fn permission_denied(output: &std::process::Output) -> bool {
    let stderr = String::from_utf8_lossy(&output.stderr);
    stderr.contains("EACCES") || stderr.contains("EPERM") || stderr.contains("permission denied")
}

#[derive(Clone)]
enum OsArg {
    Str(String),
    Path(PathBuf),
}

/// How to launch `npm`. On Windows the discovered npm is its `npm-cli.js`
/// (`<node root>\node_modules\npm\bin\npm-cli.js`, see
/// `adapter_install::find_npm`), which only runs through the node beside it.
fn npm_launch(npm: &Path) -> (PathBuf, Vec<OsArg>) {
    let script = npm
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("js"));
    if !script {
        return (npm.to_path_buf(), Vec::new());
    }
    let node_name = if cfg!(windows) { "node.exe" } else { "node" };
    let node = npm
        .ancestors()
        .nth(4)
        .map(|root| root.join(node_name))
        .filter(|node| node.is_file())
        .or_else(|| crate::executable::find_on_paths("node", Vec::new()))
        .unwrap_or_else(|| PathBuf::from(node_name));
    (node, vec![OsArg::Path(npm.to_path_buf())])
}

async fn run_args(
    program: &Path,
    lead: &[OsArg],
    args: &[OsArg],
) -> Result<std::process::Output, String> {
    let mut cmd = command(program);
    for arg in lead.iter().chain(args) {
        match arg {
            OsArg::Str(s) => cmd.arg(s),
            OsArg::Path(p) => cmd.arg(p),
        };
    }
    tokio::time::timeout(std::time::Duration::from_secs(300), cmd.output())
        .await
        .map_err(|_| format!("{} timed out", program.display()))?
        .map_err(|err| format!("{}: {err}", program.display()))
}

async fn succeeds(program: &Path, args: &[&str]) -> bool {
    tokio::time::timeout(PROBE_TIMEOUT, command(program).args(args).output())
        .await
        .is_ok_and(|out| out.is_ok_and(|out| out.status.success()))
}

/// The crate's owned launcher: on Windows it runs `.cmd` shims without a
/// console window flashing up from the GUI, and its job object takes npm's
/// whole process tree down when a timed-out install is dropped.
fn command(program: &Path) -> Command {
    let mut cmd = Command::new(program);
    cmd.stdin(Stdio::null()).kill_on_drop(true);
    cmd
}

fn tail(text: &str) -> String {
    let text = text.trim();
    let start = text.char_indices().rev().nth(399).map_or(0, |(i, _)| i);
    text[start..].to_string()
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    /// Windows npm: `%APPDATA%\npm\codex.cmd` is a shim file (not a link);
    /// the package lives in the `node_modules` folder beside it.
    #[test]
    fn version_is_read_beside_a_windows_shim() {
        let dir = tempfile::tempdir().unwrap();
        let package = dir.path().join("node_modules/@openai/codex");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(
            package.join("package.json"),
            r#"{"name":"@openai/codex","version":"0.164.0"}"#,
        )
        .unwrap();
        let shim = dir.path().join("codex.cmd");
        std::fs::write(&shim, "@ECHO off\r\nnode codex.js %*\r\n").unwrap();
        assert_eq!(installed_version(&shim).as_deref(), Some("0.164.0"));
        // Another package's manifest is never taken for Codex's.
        std::fs::write(
            package.join("package.json"),
            r#"{"name":"evil","version":"1.0.0"}"#,
        )
        .unwrap();
        assert_eq!(installed_version(&shim), None);
    }

    /// The same Windows layout: the shim's own folder is the global prefix
    /// npm reinstalls into, never whatever prefix the found npm defaults to.
    #[test]
    fn a_windows_shim_folder_is_its_global_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let shim = dir.path().join("codex.cmd");
        std::fs::write(&shim, "@ECHO off\r\nnode codex.js %*\r\n").unwrap();
        // A lone shim names no package: no prefix to pin.
        assert_eq!(global_prefix(&shim), None);
        std::fs::create_dir_all(dir.path().join("node_modules/@openai/codex")).unwrap();
        let prefix = super::super::plain_executable(dir.path().canonicalize().unwrap());
        assert_eq!(global_prefix(&shim), Some(prefix.clone()));
        // Plain form: npm takes no `\\?\` verbatim path.
        assert!(!prefix.to_string_lossy().starts_with(r"\\?\"));
    }

    #[test]
    fn a_js_npm_cli_runs_through_the_node_beside_it() {
        let dir = tempfile::tempdir().unwrap();
        let cli = dir.path().join("node_modules/npm/bin/npm-cli.js");
        std::fs::create_dir_all(cli.parent().unwrap()).unwrap();
        std::fs::write(&cli, "").unwrap();
        let node = dir
            .path()
            .join(if cfg!(windows) { "node.exe" } else { "node" });
        std::fs::write(&node, "").unwrap();
        let (program, lead) = npm_launch(&cli);
        assert_eq!(program, node);
        assert!(matches!(lead.as_slice(), [OsArg::Path(p)] if *p == cli));
        let plain = dir.path().join("npm");
        let (program, lead) = npm_launch(&plain);
        assert_eq!(program, plain);
        assert!(lead.is_empty());
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn script(path: &Path, body: &str) {
        std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// A global install laid out like npm's: `bin/codex` → the package's
    /// launcher, which fails until the platform package "arrives".
    fn install(dir: &Path) -> PathBuf {
        let package = dir.join("lib/node_modules/@openai/codex");
        std::fs::create_dir_all(package.join("bin")).unwrap();
        std::fs::write(
            package.join("package.json"),
            r#"{"name":"@openai/codex","version":"0.163.0"}"#,
        )
        .unwrap();
        let fixed = dir.join("fixed");
        script(
            &package.join("bin/codex.js"),
            &format!(
                "if [ -f '{}' ]; then echo 'codex-cli 0.163.0'; exit 0; fi\n\
                 echo 'Error: Missing optional dependency @openai/codex-linux-x64. Reinstall Codex' >&2; exit 1",
                fixed.display()
            ),
        );
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        let codex = dir.join("bin/codex");
        std::os::unix::fs::symlink(package.join("bin/codex.js"), &codex).unwrap();
        codex
    }

    #[tokio::test]
    async fn reinstalls_the_same_version_with_optional_dependencies() {
        let dir = tempfile::tempdir().unwrap();
        let codex = install(dir.path());
        let npm = dir.path().join("npm");
        let calls = dir.path().join("calls");
        script(
            &npm,
            &format!(
                "echo \"$@\" >> '{}'; touch '{}'",
                calls.display(),
                dir.path().join("fixed").display()
            ),
        );
        let outcome = repair_with(&codex, &npm, None).await;
        assert_eq!(
            outcome,
            Ok(Repair::Repaired {
                version: "0.163.0".into()
            })
        );
        let calls = std::fs::read_to_string(calls).unwrap();
        let prefix = dir.path().canonicalize().unwrap();
        assert_eq!(
            calls.trim(),
            format!(
                "install -g --prefix {} --include=optional --no-audit --no-fund @openai/codex@0.163.0",
                prefix.display()
            )
        );
        // Healthy now: nothing more to do.
        assert_eq!(repair_with(&codex, &npm, None).await, Ok(Repair::NotNeeded));
    }

    /// A shim-style install (a launcher file beside `node_modules`, as npm
    /// lays out Windows' global prefix) reinstalls into the shim's folder.
    #[tokio::test]
    async fn a_shim_install_reinstalls_into_its_own_folder() {
        let dir = tempfile::tempdir().unwrap();
        let package = dir.path().join("node_modules/@openai/codex");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(
            package.join("package.json"),
            r#"{"name":"@openai/codex","version":"0.162.0"}"#,
        )
        .unwrap();
        let fixed = dir.path().join("fixed");
        let codex = dir.path().join("codex");
        script(
            &codex,
            &format!(
                "if [ -f '{}' ]; then echo 'codex-cli 0.162.0'; exit 0; fi\n\
                 echo 'Error: Missing optional dependency @openai/codex-win32-x64' >&2; exit 1",
                fixed.display()
            ),
        );
        let npm = dir.path().join("npm");
        let calls = dir.path().join("calls");
        script(
            &npm,
            &format!(
                "echo \"$@\" >> '{}'; touch '{}'",
                calls.display(),
                fixed.display()
            ),
        );
        let outcome = repair_with(&codex, &npm, None).await;
        assert!(
            matches!(outcome, Ok(Repair::Repaired { .. })),
            "{outcome:?}"
        );
        let calls = std::fs::read_to_string(calls).unwrap();
        assert_eq!(
            calls.trim(),
            format!(
                "install -g --prefix {} --include=optional --no-audit --no-fund @openai/codex@0.162.0",
                dir.path().canonicalize().unwrap().display()
            )
        );
    }

    #[tokio::test]
    async fn a_root_owned_prefix_retries_through_sudo() {
        let dir = tempfile::tempdir().unwrap();
        let codex = install(dir.path());
        let npm = dir.path().join("npm");
        let fixed = dir.path().join("fixed");
        // Plain npm is refused; through "sudo" (which marks itself) it works.
        script(
            &npm,
            &format!(
                "if [ -n \"$AS_ROOT\" ]; then touch '{}'; exit 0; fi\n\
                 echo 'npm error code EACCES' >&2; exit 243",
                fixed.display()
            ),
        );
        let sudo = dir.path().join("sudo");
        script(
            &sudo,
            "[ \"$1\" = -n ] || exit 9; shift; AS_ROOT=1 exec \"$@\"",
        );
        let outcome = repair_with(&codex, &npm, Some(&sudo)).await;
        assert!(
            matches!(outcome, Ok(Repair::Repaired { .. })),
            "{outcome:?}"
        );
    }

    #[tokio::test]
    async fn other_failures_are_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let codex = dir.path().join("codex");
        script(&codex, "echo 'some other crash' >&2; exit 1");
        let npm = dir.path().join("npm");
        script(&npm, "echo called > /dev/null; exit 1");
        assert_eq!(repair_with(&codex, &npm, None).await, Ok(Repair::NotNeeded));
    }
}
