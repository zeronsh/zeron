//! Linux bubblewrap: a private mount namespace with `/` read-only, writable
//! roots bound back read-write, read-only carve-outs bound read-only over
//! them, and hidden paths covered (an empty read-only tmpfs over folders,
//! `/dev/null` over files). Unlike Landlock it can deny *below* a grant, so
//! every part of the filesystem spec holds exactly (for paths that exist —
//! bind mounts need an existing target).
//!
//! Network off: `--unshare-net` gives the agent a network namespace with
//! only its own loopback, which also cuts it off from the engine's IPC port
//! on the host loopback. So when `loopback_ports` are needed the namespace is
//! shared and the helper (Landlock TCP port rules + seccomp) confines the
//! network inside bwrap instead; see [`NetworkPlan`].
//!
//! Flag choices follow Codex's `codex-rs/linux-sandbox/src/bwrap.rs`
//! (Apache-2.0): `--new-session` (no TIOCSTI into the parent terminal),
//! `--die-with-parent`, fresh pid/ipc namespaces with their own `/proc`, a
//! minimal `/dev` plus the host's `/dev/shm`, all capabilities dropped.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::landlock::{network_enforceable, network_notes};
use crate::{Backend, Enforcement, Resolved};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NetworkPlan {
    /// Network on: share the host's network namespace.
    Shared,
    /// Network off, nothing on loopback needed: `--unshare-net`.
    Unshared,
    /// Network off but loopback ports needed: share the namespace and run
    /// the agent through the helper with `--net-only`.
    Helper,
}

pub(crate) fn network_plan(r: &Resolved, _landlock_abi: u32) -> NetworkPlan {
    if r.network {
        NetworkPlan::Shared
    } else if r.loopback_ports.is_empty() {
        NetworkPlan::Unshared
    } else {
        NetworkPlan::Helper
    }
}

pub(crate) fn program() -> Option<PathBuf> {
    static FOUND: OnceLock<Option<PathBuf>> = OnceLock::new();
    FOUND
        .get_or_init(|| {
            let path = std::env::var_os("PATH").unwrap_or_default();
            std::env::split_paths(&path)
                .chain(["/usr/bin".into(), "/usr/local/bin".into()])
                .map(|dir| dir.join("bwrap"))
                .find(|p| p.is_file())
        })
        .clone()
}

/// `bwrap` is installed and can create its namespaces here (user namespaces
/// may be disabled, or restricted by AppArmor). Probed once.
pub(crate) fn available() -> bool {
    static WORKS: OnceLock<bool> = OnceLock::new();
    *WORKS.get_or_init(|| {
        if !cfg!(target_os = "linux") {
            return false;
        }
        let Some(bwrap) = program() else {
            return false;
        };
        std::process::Command::new(bwrap)
            .args([
                "--new-session",
                "--die-with-parent",
                "--unshare-pid",
                "--unshare-ipc",
                "--ro-bind",
                "/",
                "/",
                "--dev",
                "/dev",
                "--proc",
                "/proc",
                "--",
                "/bin/true",
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Dir,
    File,
}

pub(crate) fn host_kind(path: &Path) -> Option<Kind> {
    let meta = std::fs::metadata(path).ok()?;
    Some(if meta.is_dir() { Kind::Dir } else { Kind::File })
}

/// The full `bwrap` argument list; `inner` is the command run inside.
pub(crate) fn args(
    r: &Resolved,
    kind: &dyn Fn(&Path) -> Option<Kind>,
    net: NetworkPlan,
    inner: Vec<OsString>,
) -> Vec<OsString> {
    let mut a: Vec<OsString> = Vec::new();
    let mut push = |items: &[&str]| a.extend(items.iter().map(OsString::from));
    push(&[
        "--new-session",
        "--die-with-parent",
        "--unshare-pid",
        "--unshare-ipc",
    ]);
    if net == NetworkPlan::Unshared {
        push(&["--unshare-net"]);
    }
    push(&[
        "--ro-bind",
        "/",
        "/",
        "--dev",
        "/dev",
        "--bind-try",
        "/dev/shm",
        "/dev/shm",
        "--proc",
        "/proc",
    ]);
    let hidden_below = |p: &Path| r.hidden.iter().any(|h| p.starts_with(h));
    let pair = |flag: &str, p: &Path| [OsString::from(flag), p.into(), p.into()];
    for root in &r.write {
        // `/dev/shm` is already bound above, after `--dev` replaced `/dev`.
        if root.starts_with("/dev") || hidden_below(root) || kind(root).is_none() {
            continue;
        }
        a.extend(pair("--bind", root));
    }
    for file in &r.write_prefixes {
        if !hidden_below(file) && kind(file).is_some() {
            a.extend(pair("--bind", file));
        }
    }
    for ro in &r.read_only {
        if !hidden_below(ro) && kind(ro).is_some() {
            a.extend(pair("--ro-bind", ro));
        }
    }
    for hidden in &r.hidden {
        match kind(hidden) {
            Some(Kind::Dir) => {
                a.extend(["--perms", "000", "--tmpfs"].map(OsString::from));
                a.push(hidden.into());
                a.push("--remount-ro".into());
                a.push(hidden.into());
            }
            Some(Kind::File) => {
                a.push("--ro-bind".into());
                a.push("/dev/null".into());
                a.push(hidden.into());
            }
            None => {}
        }
    }
    a.push("--cap-drop".into());
    a.push("ALL".into());
    a.push("--".into());
    a.extend(inner);
    a
}

pub(crate) fn enforcement(r: &Resolved, net: NetworkPlan) -> Enforcement {
    let abi = crate::landlock::kernel_abi();
    let mut notes = vec![
        "paths that don't exist when the agent starts can't be hidden or protected (bind \
         mounts need a target); writable folders are created beforehand"
            .into(),
        "the agent runs in its own pid namespace: it sees only its own processes, and \
         stopping bwrap kills it (no graceful SIGTERM)"
            .into(),
    ];
    if !r.write_prefixes.is_empty() {
        notes.push(
            "atomic-replace files (e.g. ~/.claude.json) are writable in place only; renaming \
             a temp file over a bind-mounted file fails"
                .into(),
        );
    }
    let network = match net {
        NetworkPlan::Shared | NetworkPlan::Unshared => true,
        NetworkPlan::Helper => {
            network_notes(r, abi, &mut notes);
            network_enforceable(r, abi)
        }
    };
    if net == NetworkPlan::Unshared {
        notes.push("network off: private network namespace (own loopback only)".into());
    }
    Enforcement {
        backend: Backend::Bubblewrap,
        writes: true,
        hidden: true,
        read_only: true,
        network,
        notes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SandboxMode, SandboxSpec};

    fn resolved(network: bool, ports: Vec<u16>) -> Resolved {
        let spec = SandboxSpec::new(SandboxMode::WorkspaceWrite, network, "/w");
        let mut r = Resolved::new(&spec, vec![]).unwrap();
        r.write = vec![
            "/dev/shm".into(),
            "/h/.claude".into(),
            "/missing".into(),
            "/w".into(),
        ];
        r.write_prefixes = vec!["/h/.claude.json".into()];
        r.read_only = vec!["/w/.git/hooks".into()];
        r.hidden = vec!["/h/.ssh".into(), "/h/.netrc".into(), "/h/.aws".into()];
        r.loopback_ports = ports;
        r
    }

    fn fake_kind(p: &Path) -> Option<Kind> {
        match p.to_str().unwrap() {
            "/h/.claude" | "/w" | "/w/.git/hooks" | "/h/.ssh" => Some(Kind::Dir),
            "/h/.claude.json" | "/h/.netrc" => Some(Kind::File),
            _ => None,
        }
    }

    fn render(args: &[OsString]) -> String {
        args.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn golden_args_network_off() {
        let r = resolved(false, vec![]);
        assert_eq!(network_plan(&r, 0), NetworkPlan::Unshared);
        let a = args(
            &r,
            &fake_kind,
            NetworkPlan::Unshared,
            vec!["/bin/agent".into(), "--x".into()],
        );
        assert_eq!(
            render(&a),
            "--new-session --die-with-parent --unshare-pid --unshare-ipc --unshare-net \
             --ro-bind / / --dev /dev --bind-try /dev/shm /dev/shm --proc /proc \
             --bind /h/.claude /h/.claude --bind /w /w \
             --bind /h/.claude.json /h/.claude.json \
             --ro-bind /w/.git/hooks /w/.git/hooks \
             --perms 000 --tmpfs /h/.ssh --remount-ro /h/.ssh \
             --ro-bind /dev/null /h/.netrc \
             --cap-drop ALL -- /bin/agent --x"
        );
    }

    #[test]
    fn loopback_ports_keep_the_namespace_and_use_the_helper() {
        let r = resolved(false, vec![27654]);
        assert_eq!(network_plan(&r, 6), NetworkPlan::Helper);
        let a = args(&r, &fake_kind, NetworkPlan::Helper, vec!["/zeron".into()]);
        assert!(!a.contains(&OsString::from("--unshare-net")));
        assert_eq!(
            network_plan(&resolved(true, vec![27654]), 6),
            NetworkPlan::Shared
        );
    }
}
