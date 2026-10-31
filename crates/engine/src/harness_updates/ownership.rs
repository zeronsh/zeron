//! Who owns an installed agent CLI, judged from where its launcher resolves
//! and where it really lives — never from what its own updater would guess.
//!
//! A CLI shipped as an npm package can be installed by npm, pnpm, bun, yarn
//! or Volta, pinned by mise, or sit inside some project's `node_modules`.
//! Its self-updater (`claude update`, `pi update --self`, …) reinstalls with
//! whatever npm it finds, which may be another Node's, writing a second copy
//! the launcher never runs. So an install is updated by the package manager
//! that owns it, into the place it lives; one zeron can't prove is left to
//! the user, with the reason.

use std::path::{Path, PathBuf};

use zeron_proto::HarnessId;

/// A version-manager wrapper script is a few lines; anything larger is a
/// real launcher.
const MISE_WRAPPER_MAX_BYTES: u64 = 16 * 1024;

/// The npm package an agent CLI ships as, when it ships as one. OpenCode's
/// package follows the installed generation (see `opencode_release_package`).
pub(super) fn npm_package(harness: HarnessId, installed: Option<&str>) -> Option<&'static str> {
    match harness {
        HarnessId::ClaudeCode => Some("@anthropic-ai/claude-code"),
        HarnessId::Codex => Some("@openai/codex"),
        HarnessId::Pi => Some("@earendil-works/pi-coding-agent"),
        HarnessId::Opencode => Some(super::opencode_release_package(installed.unwrap_or(""))),
        _ => None,
    }
}

/// The package manager owning an npm-packaged CLI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PackageOwner {
    /// `npm install -g` into this prefix (the one the package really lives in).
    Npm {
        prefix: PathBuf,
    },
    Pnpm,
    Bun,
    Yarn,
    /// Volta's shim, with `volta_home` holding the package's image.
    Volta {
        volta_home: PathBuf,
    },
    /// Inside some project's `node_modules`: that project owns it.
    Project,
}

/// How an install must be updated, when zeron can't do it itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Manual {
    /// A mise-managed tool: its version is pinned in mise's config.
    Mise,
    /// A copy inside a project's `node_modules`.
    Project,
}

impl Manual {
    pub(super) fn instruction(&self) -> &'static str {
        match self {
            Manual::Mise => {
                "Installed with mise: update it with `mise upgrade` (or change the version mise pins)"
            }
            Manual::Project => {
                "Installed inside a project's node_modules: update it with that project's package \
                 manager"
            }
        }
    }
}

/// An update zeron can run for a package-managed install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PackageUpdate {
    pub program: PathBuf,
    pub args: Vec<String>,
    /// The installation owner's lock key: one updater per owner at a time.
    pub owner: String,
}

/// `/`-separated and ASCII-lowercased: byte offsets stay valid against the
/// original, so a match can slice the original-case path.
fn slash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn real(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Whether mise owns this launcher: a mise shim or install path, mise's own
/// binary behind a shim (also when Homebrew installed mise), or a wrapper
/// script that hands over to `mise exec`.
pub(super) fn mise_managed(resolved: &Path) -> bool {
    let real = real(resolved);
    let paths = [slash(resolved), slash(&real)].map(|p| p.to_ascii_lowercase());
    if paths.iter().any(|p| {
        // Globals under mise's own Node belong to that Node's npm.
        (p.contains("/mise/installs/") && !p.contains("/mise/installs/node/"))
            || p.contains("/mise/shims/")
    }) {
        return true;
    }
    if real
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.trim_end_matches(".exe") == "mise")
    {
        return true;
    }
    is_mise_wrapper(&real)
}

fn is_mise_wrapper(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() || metadata.len() > MISE_WRAPPER_MAX_BYTES {
        return false;
    }
    let Ok(script) = std::fs::read_to_string(path) else {
        return false;
    };
    script.starts_with("#!")
        && script
            .split_whitespace()
            .collect::<Vec<_>>()
            .windows(2)
            .any(|pair| {
                pair[0].rsplit('/').next() == Some("mise") && matches!(pair[1], "x" | "exec")
            })
}

/// Who owns `resolved` (the launcher as found) as an install of `package`,
/// `None` when it isn't package-managed at all (a vendor's native install,
/// a system package) and the caller's other rules apply.
pub(super) fn package_owner(resolved: &Path, package: &str) -> Option<PackageOwner> {
    let real = real(resolved);
    let paths = [slash(resolved), slash(&real)];
    let lower: Vec<String> = paths.iter().map(|p| p.to_ascii_lowercase()).collect();
    let any = |needle: &str| lower.iter().any(|p| p.contains(needle));
    let package = package.to_ascii_lowercase();

    if any("/.bun/bin/") || any("/.bun/install/global/node_modules/") {
        return Some(PackageOwner::Bun);
    }
    if [
        "/.local/share/pnpm/",
        "/library/pnpm/",
        "/local/share/pnpm/",
        "/appdata/local/pnpm/",
        "/pnpm/global/",
    ]
    .iter()
    .any(|needle| any(needle))
    {
        return Some(PackageOwner::Pnpm);
    }
    if any("/yarn/global/node_modules/") || any("/yarn/data/global/node_modules/") {
        return Some(PackageOwner::Yarn);
    }
    if let Some(volta_home) = volta_home(resolved, &real, &package) {
        return Some(PackageOwner::Volta { volta_home });
    }
    if let Some(prefix) = npm_prefix(&slash(&real), &package) {
        return Some(PackageOwner::Npm { prefix });
    }
    if any("/node_modules/") {
        return Some(PackageOwner::Project);
    }
    None
}

/// Volta links `<VOLTA_HOME>/bin/<cmd>` to its `volta-shim`, which picks the
/// package at run time; the package's image directory proves Volta installed
/// this one.
fn volta_home(resolved: &Path, real: &Path, package: &str) -> Option<PathBuf> {
    let shim = real.file_name()?.to_str()?.trim_end_matches(".exe");
    if shim != "volta-shim" {
        return None;
    }
    let home = resolved.parent()?.parent()?.to_path_buf();
    let mut image = home.join("tools").join("image").join("packages");
    for part in package.split('/') {
        image.push(part);
    }
    image.is_dir().then_some(home)
}

/// The global prefix `package` was installed into, read off its real path:
/// `<prefix>/lib/node_modules/<package>/…` (Unix) or
/// `<prefix>/node_modules/<package>/…` (Windows). A prefix that is itself
/// inside `node_modules` is a nested (project) install, not a global one; so
/// is a mise tool that isn't Node (mise's npm backend mimics the layout).
fn npm_prefix(real: &str, package: &str) -> Option<PathBuf> {
    let lower = real.to_ascii_lowercase();
    let segment = if cfg!(windows) {
        format!("/node_modules/{package}/")
    } else {
        format!("/lib/node_modules/{package}/")
    };
    let at = lower.rfind(&segment)?;
    let before = &lower[..at];
    if before.contains("/node_modules/") {
        return None;
    }
    if before.contains("/mise/installs/") && !before.contains("/mise/installs/node/") {
        return None;
    }
    Some(PathBuf::from(if at == 0 { "/" } else { &real[..at] }))
}

/// The update command for a package-managed install, installing
/// `package@tag`. `pnpm_version` is pnpm's own version when the owner is
/// pnpm (10.4+ needs the build of the package approved on the command line,
/// and older pnpm rejects the flag).
pub(super) fn package_update(
    owner: &PackageOwner,
    resolved: &Path,
    package: &str,
    tag: &str,
    pnpm_version: Option<&str>,
) -> Result<PackageUpdate, String> {
    let spec = format!("{package}@{tag}");
    let found = |name: &str, near: &[PathBuf]| {
        find_program(name, near).ok_or_else(|| format!("`{name}` was not found to update with"))
    };
    let launcher_dir = resolved
        .parent()
        .map(Path::to_path_buf)
        .into_iter()
        .collect::<Vec<_>>();
    Ok(match owner {
        PackageOwner::Npm { prefix } => {
            // The prefix's own npm first: another Node's npm would install
            // with its own engine and defaults.
            let near = if cfg!(windows) {
                vec![prefix.clone()]
            } else {
                vec![prefix.join("bin")]
            };
            PackageUpdate {
                program: found("npm", &near)?,
                args: vec![
                    "install".into(),
                    "-g".into(),
                    "--prefix".into(),
                    prefix.to_string_lossy().into_owned(),
                    // npm 12 skips install scripts unless allowed (exiting 0
                    // with the package's postinstall never run); older npm
                    // only warns about the unknown option.
                    format!("--allow-scripts={package}"),
                    "--include=optional".into(),
                    "--no-audit".into(),
                    "--no-fund".into(),
                    spec,
                ],
                owner: format!("npm:{}", slash(prefix).to_ascii_lowercase()),
            }
        }
        PackageOwner::Pnpm => {
            let mut args = vec!["add".to_string(), "-g".to_string()];
            if pnpm_version.is_some_and(|version| version_at_least(version, &[10, 4])) {
                args.push(format!("--allow-build={package}"));
            }
            args.push(spec);
            PackageUpdate {
                program: found("pnpm", &launcher_dir)?,
                args,
                owner: "pnpm-global".into(),
            }
        }
        PackageOwner::Bun => PackageUpdate {
            program: found("bun", &launcher_dir)?,
            args: vec!["add".into(), "-g".into(), spec],
            owner: "bun-global".into(),
        },
        PackageOwner::Yarn => PackageUpdate {
            program: found("yarn", &[])?,
            args: vec!["global".into(), "add".into(), spec],
            owner: "yarn-global".into(),
        },
        PackageOwner::Volta { volta_home } => PackageUpdate {
            program: found("volta", &[volta_home.join("bin")])?,
            args: vec!["install".into(), spec],
            owner: format!("volta:{}", slash(volta_home).to_ascii_lowercase()),
        },
        PackageOwner::Project => return Err(Manual::Project.instruction().into()),
    })
}

fn version_at_least(version: &str, minimum: &[u64]) -> bool {
    let numbers: Vec<u64> = version
        .trim()
        .trim_start_matches('v')
        .split(['.', '-', '+'])
        .map_while(|part| part.parse().ok())
        .collect();
    numbers.as_slice() >= minimum
}

/// `name` in `near` first, then on the engine's PATH, then on the login
/// shell's PATH (an app launched from the Dock has a minimal PATH).
fn find_program(name: &str, near: &[PathBuf]) -> Option<PathBuf> {
    let names: Vec<String> = if cfg!(windows) {
        ["cmd", "exe", "ps1"]
            .iter()
            .map(|ext| format!("{name}.{ext}"))
            .chain(std::iter::once(name.to_string()))
            .collect()
    } else {
        vec![name.to_string()]
    };
    let mut dirs: Vec<PathBuf> = near.to_vec();
    for path in [
        std::env::var_os("PATH"),
        zeron_harness::shell_env::login_shell_path().map(|p| p.to_os_string()),
    ]
    .into_iter()
    .flatten()
    {
        dirs.extend(std::env::split_paths(&path));
    }
    dirs.iter()
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLAUDE: &str = "@anthropic-ai/claude-code";

    #[cfg(unix)]
    #[test]
    fn package_managers_are_told_apart_by_where_the_cli_lives() {
        let owner = |path: &str| package_owner(Path::new(path), CLAUDE);
        assert_eq!(
            owner("/opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/cli.js"),
            Some(PackageOwner::Npm {
                prefix: "/opt/homebrew".into()
            })
        );
        // Homebrew's Node keeps its globals in its keg: that's npm's install.
        assert_eq!(
            owner(
                "/opt/homebrew/Cellar/node/24.1.0/lib/node_modules/@anthropic-ai/claude-code/cli.js"
            ),
            Some(PackageOwner::Npm {
                prefix: "/opt/homebrew/Cellar/node/24.1.0".into()
            })
        );
        // Globals under mise's own Node are that npm's.
        assert_eq!(
            owner(
                "/home/u/.local/share/mise/installs/node/22.4.0/lib/node_modules/@anthropic-ai/claude-code/cli.js"
            ),
            Some(PackageOwner::Npm {
                prefix: "/home/u/.local/share/mise/installs/node/22.4.0".into()
            })
        );
        assert_eq!(owner("/home/u/.bun/bin/claude"), Some(PackageOwner::Bun));
        assert_eq!(
            owner("/home/u/.local/share/pnpm/claude"),
            Some(PackageOwner::Pnpm)
        );
        assert_eq!(
            owner("/Users/u/Library/pnpm/claude"),
            Some(PackageOwner::Pnpm)
        );
        assert_eq!(
            owner("/home/u/.config/yarn/global/node_modules/@anthropic-ai/claude-code/cli.js"),
            Some(PackageOwner::Yarn)
        );
        // Some other package's dependency, or a project's copy.
        assert_eq!(
            owner(
                "/opt/homebrew/lib/node_modules/some-tool/node_modules/@anthropic-ai/claude-code/cli.js"
            ),
            Some(PackageOwner::Project)
        );
        assert_eq!(
            owner("/work/app/node_modules/.bin/claude"),
            Some(PackageOwner::Project)
        );
        // A vendor's native install isn't package-managed.
        assert_eq!(owner("/home/u/.local/bin/claude"), None);
        assert_eq!(owner("/usr/local/bin/codex"), None);
    }

    #[cfg(unix)]
    #[test]
    fn mise_is_recognised_by_path_binary_and_wrapper() {
        assert!(mise_managed(Path::new(
            "/home/u/.local/share/mise/installs/npm-anthropic-ai-claude-code/2.1.0/bin/claude"
        )));
        assert!(mise_managed(Path::new(
            "/home/u/.local/share/mise/shims/claude"
        )));
        assert!(!mise_managed(Path::new(
            "/home/u/.local/share/mise/installs/node/22.4.0/lib/node_modules/@anthropic-ai/claude-code/cli.js"
        )));
        assert!(!mise_managed(Path::new("/home/u/.local/bin/claude")));

        let dir = tempfile::tempdir().unwrap();
        let wrapper = dir.path().join("claude");
        std::fs::write(
            &wrapper,
            "#!/bin/sh\nexec /usr/local/bin/mise x -- claude \"$@\"\n",
        )
        .unwrap();
        assert!(mise_managed(&wrapper));
        let plain = dir.path().join("codex");
        std::fs::write(
            &plain,
            "#!/bin/sh\nexec node /opt/codex/bin/codex.js \"$@\"\n",
        )
        .unwrap();
        assert!(!mise_managed(&plain));
        // mise itself behind a shim (here: installed by Homebrew).
        let keg = dir.path().join("Cellar/mise/2025.1.0/bin");
        std::fs::create_dir_all(&keg).unwrap();
        std::fs::write(keg.join("mise"), vec![0u8; 64 * 1024]).unwrap();
        let shim = dir.path().join("claude-shim");
        std::os::unix::fs::symlink(keg.join("mise"), &shim).unwrap();
        assert!(mise_managed(&shim));
    }

    #[cfg(unix)]
    #[test]
    fn volta_is_proven_by_the_packages_image() {
        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("volta-shim"), "").unwrap();
        std::os::unix::fs::symlink(bin.join("volta-shim"), bin.join("claude")).unwrap();
        assert_eq!(package_owner(&bin.join("claude"), CLAUDE), None);
        std::fs::create_dir_all(
            home.path()
                .join("tools/image/packages/@anthropic-ai/claude-code"),
        )
        .unwrap();
        assert_eq!(
            package_owner(&bin.join("claude"), CLAUDE),
            Some(PackageOwner::Volta {
                volta_home: home.path().to_path_buf()
            })
        );
    }

    #[cfg(unix)]
    #[test]
    fn updates_run_the_owners_own_tool_into_its_own_place() {
        use std::os::unix::fs::PermissionsExt;
        let prefix = tempfile::tempdir().unwrap();
        let npm = prefix.path().join("bin/npm");
        std::fs::create_dir_all(npm.parent().unwrap()).unwrap();
        std::fs::write(&npm, "").unwrap();
        std::fs::set_permissions(&npm, std::fs::Permissions::from_mode(0o755)).unwrap();
        let owner = PackageOwner::Npm {
            prefix: prefix.path().to_path_buf(),
        };
        let update =
            package_update(&owner, Path::new("/x/claude"), CLAUDE, "stable", None).unwrap();
        assert_eq!(update.program, npm);
        assert_eq!(
            update.args,
            [
                "install",
                "-g",
                "--prefix",
                prefix.path().to_str().unwrap(),
                "--allow-scripts=@anthropic-ai/claude-code",
                "--include=optional",
                "--no-audit",
                "--no-fund",
                "@anthropic-ai/claude-code@stable",
            ]
        );
        assert!(update.owner.starts_with("npm:"));

        let bun_home = tempfile::tempdir().unwrap();
        let bun = bun_home.path().join("bun");
        std::fs::write(&bun, "").unwrap();
        let update = package_update(
            &PackageOwner::Bun,
            &bun_home.path().join("claude"),
            CLAUDE,
            "latest",
            None,
        )
        .unwrap();
        assert_eq!(update.program, bun);
        assert_eq!(
            update.args,
            ["add", "-g", "@anthropic-ai/claude-code@latest"]
        );

        let pnpm_home = tempfile::tempdir().unwrap();
        std::fs::write(pnpm_home.path().join("pnpm"), "").unwrap();
        let launcher = pnpm_home.path().join("claude");
        let args = |version| {
            package_update(&PackageOwner::Pnpm, &launcher, CLAUDE, "latest", version)
                .unwrap()
                .args
        };
        assert_eq!(
            args(Some("10.4.1")),
            [
                "add",
                "-g",
                "--allow-build=@anthropic-ai/claude-code",
                "@anthropic-ai/claude-code@latest"
            ]
        );
        assert_eq!(
            args(Some("9.15.0")),
            ["add", "-g", "@anthropic-ai/claude-code@latest"]
        );

        assert!(package_update(&PackageOwner::Project, &launcher, CLAUDE, "latest", None).is_err());
    }
}
