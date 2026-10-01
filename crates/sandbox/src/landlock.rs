//! Linux Landlock (+ seccomp for the network), applied by the helper mode of
//! the Zeron binary to itself just before it `execve`s the agent.
//!
//! Landlock is allow-list only: a rule on a directory grants rights on its
//! whole subtree and nothing can be denied below it. So "read everywhere
//! except `hidden`" is built by *splitting*: walk from `/` down towards each
//! hidden path and grant every sibling on the way instead of the parent
//! (`grant(/home/u)` becomes `grant(/home/u/proj)`, `grant(/home/u/.config)`,
//! … but not `/home/u/.ssh`). Writable roots are split the same way, so a
//! hidden path inside the workspace stays hidden. Directory *listing* is
//! granted on `/` (names under a hidden folder are visible; contents are
//! not). Symlinks are skipped while splitting: Landlock checks the resolved
//! path, which the split already covers at its real location.
//!
//! Network off: seccomp refuses non-Unix/netlink sockets (and io_uring, which
//! creates sockets without `socket(2)`). When loopback ports must stay
//! reachable, TCP sockets are allowed and Landlock ABI v4 restricts
//! `connect` to those ports and `bind` to nothing; UDP stays refused.
//! Seccomp adapted from Codex's `codex-rs/linux-sandbox/src/landlock.rs`
//! (Apache-2.0).

use std::path::{Path, PathBuf};

use crate::{Backend, Enforcement, Resolved};

/// The running kernel's Landlock ABI version (0: none).
pub(crate) fn kernel_abi() -> u32 {
    #[cfg(target_os = "linux")]
    {
        const LANDLOCK_CREATE_RULESET_VERSION: libc::c_ulong = 1;
        // SAFETY: the version query takes no attribute pointer and size 0.
        let v = unsafe {
            libc::syscall(
                libc::SYS_landlock_create_ruleset,
                std::ptr::null::<libc::c_void>(),
                0usize,
                LANDLOCK_CREATE_RULESET_VERSION,
            )
        };
        if v > 0 { v as u32 } else { 0 }
    }
    #[cfg(not(target_os = "linux"))]
    {
        0
    }
}

/// Device nodes and device folders granted read/write (and ioctl).
pub(crate) const DEVICES: &[&str] = &[
    "/dev/null",
    "/dev/zero",
    "/dev/full",
    "/dev/random",
    "/dev/urandom",
    "/dev/tty",
    "/dev/ptmx",
    "/dev/pts",
    "/dev/shm",
];

/// What to grant, as paths. Pure: the directory listing is passed in, so the
/// split is unit-tested on any OS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Plan {
    /// Read + execute.
    pub read: Vec<PathBuf>,
    /// Every handled right.
    pub write: Vec<PathBuf>,
}

pub(crate) struct Entry {
    pub path: PathBuf,
    pub symlink: bool,
}

pub(crate) fn plan(r: &Resolved, list: &dyn Fn(&Path) -> Vec<Entry>) -> Plan {
    let mut read = Vec::new();
    split(Path::new("/"), &r.hidden, list, &mut read);
    let mut write = Vec::new();
    for root in &r.write {
        split(root, &r.hidden, list, &mut write);
    }
    // Only the existing file can be granted; siblings (atomic-write temp
    // files) can't be without granting the whole parent.
    for prefix in &r.write_prefixes {
        if !r.hidden.iter().any(|h| prefix.starts_with(h)) {
            write.push(prefix.clone());
        }
    }
    Plan { read, write }
}

fn split(
    root: &Path,
    hidden: &[PathBuf],
    list: &dyn Fn(&Path) -> Vec<Entry>,
    out: &mut Vec<PathBuf>,
) {
    if hidden.iter().any(|h| root.starts_with(h)) {
        return;
    }
    let inner: Vec<PathBuf> = hidden
        .iter()
        .filter(|h| h.starts_with(root))
        .cloned()
        .collect();
    if inner.is_empty() {
        out.push(root.to_path_buf());
        return;
    }
    for entry in list(root) {
        if !entry.symlink {
            split(&entry.path, &inner, list, out);
        }
    }
}

pub(crate) fn host_list(dir: &Path) -> Vec<Entry> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<Entry> = read
        .flatten()
        .map(|e| Entry {
            symlink: e.file_type().map(|t| t.is_symlink()).unwrap_or(false),
            path: e.path(),
        })
        .collect();
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// Whether network off can hold with Landlock `abi`: always without
/// loopback ports (seccomp alone), else only with TCP port rules (v4).
pub(crate) fn network_enforceable(r: &Resolved, abi: u32) -> bool {
    r.network || r.loopback_ports.is_empty() || abi >= 4
}

pub(crate) fn network_notes(r: &Resolved, abi: u32, notes: &mut Vec<String>) {
    if r.network {
        return;
    }
    if r.loopback_ports.is_empty() {
        notes.push("network off: IP sockets are refused (seccomp)".into());
    } else if abi >= 4 {
        notes.push(format!(
            "network off: TCP connect only to ports {:?} — Landlock filters by port, not \
             address, so those ports on other hosts are reachable too; UDP is refused; the \
             agent can't listen on TCP",
            r.loopback_ports
        ));
    } else {
        notes.push(format!(
            "network off is NOT enforced: loopback ports {:?} must stay reachable and this \
             kernel's Landlock (ABI {abi}) can't filter TCP (needs Linux 6.7+); UDP is still \
             refused",
            r.loopback_ports
        ));
    }
}

pub(crate) fn enforcement(r: &Resolved, abi: u32) -> Enforcement {
    let mut notes = Vec::new();
    notes.push(
        "hidden paths: contents are unreadable, names can be listed; entries created after \
         the agent started next to a hidden path (e.g. new files directly in $HOME) are \
         unreadable until the next start; a hard link to a hidden file is not hidden"
            .into(),
    );
    if !r.read_only.is_empty() {
        notes.push(format!(
            "read-only carve-outs are not enforced (Landlock can't deny below a grant): {}",
            r.read_only
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !r.write_prefixes.is_empty() {
        notes.push(
            "atomic-replace files (e.g. ~/.claude.json) are writable in place only; \
             writing a temp sibling and renaming it over the file is refused"
                .into(),
        );
    }
    if abi < 3 {
        notes.push(format!(
            "Landlock ABI {abi}: truncating files outside the writable areas is not restricted \
             (needs ABI 3, Linux 6.2)"
        ));
    }
    if abi < 2 {
        notes.push(
            "Landlock ABI 1: renames and links across folders are refused even inside the \
             workspace (needs ABI 2, Linux 5.19)"
                .into(),
        );
    }
    network_notes(r, abi, &mut notes);
    Enforcement {
        backend: Backend::Landlock,
        writes: abi >= 1,
        hidden: abi >= 1,
        read_only: r.read_only.is_empty(),
        network: network_enforceable(r, abi),
        notes,
    }
}

/// Confine the calling thread (inherited across `execve`): Landlock for the
/// filesystem when `filesystem`, Landlock TCP rules + seccomp for the network
/// when it is off. Fails closed: any error means "don't run the agent".
#[cfg(target_os = "linux")]
pub(crate) fn confine_current_thread(r: &Resolved, filesystem: bool) -> Result<(), String> {
    use landlock::{
        ABI, Access, AccessFs, AccessNet, BitFlags, CompatLevel, Compatible, NetPort, PathBeneath,
        PathFd, Ruleset, RulesetAttr, RulesetCreatedAttr, RulesetStatus, Scope,
    };

    // SAFETY: plain prctl; required before seccomp without CAP_SYS_ADMIN.
    if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
        return Err(format!(
            "PR_SET_NO_NEW_PRIVS: {}",
            std::io::Error::last_os_error()
        ));
    }

    let abi = kernel_abi();
    let net_rules = !r.network && !r.loopback_ports.is_empty() && abi >= 4;
    if filesystem || net_rules {
        // ABI 9 adds connecting to pathname Unix sockets; only handle it when
        // the network is off (with it on, ssh-agent and friends must work).
        let fs_handled = if r.network {
            AccessFs::from_all(ABI::V8)
        } else {
            AccessFs::from_all(ABI::V9)
        };
        let read = AccessFs::from_read(ABI::V9);
        let mut ruleset = Ruleset::default().set_compatibility(CompatLevel::BestEffort);
        if filesystem {
            ruleset = ruleset
                .handle_access(fs_handled)
                .map_err(|e| e.to_string())?;
        }
        if net_rules {
            ruleset = ruleset
                .handle_access(AccessNet::BindTcp | AccessNet::ConnectTcp)
                .map_err(|e| e.to_string())?;
        }
        let mut scopes: BitFlags<Scope> = Scope::Signal.into();
        if !r.network {
            scopes |= Scope::AbstractUnixSocket;
        }
        ruleset = ruleset.scope(scopes).map_err(|e| e.to_string())?;
        let mut created = ruleset.create().map_err(|e| e.to_string())?;
        if filesystem {
            let plan = plan(r, &host_list);
            let mut grants: Vec<(PathBuf, BitFlags<AccessFs>)> =
                vec![(PathBuf::from("/"), AccessFs::ReadDir.into())];
            grants.extend(plan.read.into_iter().map(|p| (p, read)));
            grants.extend(plan.write.into_iter().map(|p| (p, fs_handled)));
            grants.extend(DEVICES.iter().map(|d| (PathBuf::from(d), fs_handled)));
            for (path, access) in grants {
                // A path that vanished since planning has nothing to grant.
                let Ok(fd) = PathFd::new(&path) else {
                    continue;
                };
                created = created
                    .add_rule(PathBeneath::new(fd, access))
                    .map_err(|e| format!("{}: {e}", path.display()))?;
            }
        }
        if net_rules {
            for port in &r.loopback_ports {
                created = created
                    .add_rule(NetPort::new(*port, AccessNet::ConnectTcp))
                    .map_err(|e| e.to_string())?;
            }
        }
        let status = created.restrict_self().map_err(|e| e.to_string())?;
        if filesystem && status.ruleset == RulesetStatus::NotEnforced {
            return Err("Landlock is not enforced by this kernel".into());
        }
    }

    if !r.network {
        seccomp::deny_network(!r.loopback_ports.is_empty())?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
mod seccomp {
    use std::collections::BTreeMap;

    use seccompiler::{
        BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter,
        SeccompRule, TargetArch,
    };

    /// Refuse sockets other than Unix and netlink, and io_uring. With
    /// `allow_tcp`, IPv4/IPv6 *stream* sockets are allowed too (Landlock
    /// decides which ports they may connect to).
    pub(super) fn deny_network(allow_tcp: bool) -> Result<(), String> {
        let arch = if cfg!(target_arch = "x86_64") {
            TargetArch::x86_64
        } else if cfg!(target_arch = "aarch64") {
            TargetArch::aarch64
        } else if cfg!(target_arch = "riscv64") {
            TargetArch::riscv64
        } else {
            return Err("network off: no seccomp filter for this CPU architecture".into());
        };
        let prog = filter(allow_tcp, arch)?;
        seccompiler::apply_filter(&prog).map_err(|e| e.to_string())
    }

    pub(super) fn filter(allow_tcp: bool, arch: TargetArch) -> Result<BpfProgram, String> {
        let arg = |index, op, value: libc::c_int| {
            SeccompCondition::new(index, SeccompCmpArgLen::Dword, op, value as u64)
                .map_err(|e| e.to_string())
        };
        let mut rules: BTreeMap<i64, Vec<SeccompRule>> = BTreeMap::new();
        for nr in [
            libc::SYS_io_uring_setup,
            libc::SYS_io_uring_enter,
            libc::SYS_io_uring_register,
        ] {
            rules.insert(nr, Vec::new());
        }
        let mut socket = Vec::new();
        if allow_tcp {
            socket.push(
                SeccompRule::new(vec![
                    arg(0, SeccompCmpOp::Ne, libc::AF_UNIX)?,
                    arg(0, SeccompCmpOp::Ne, libc::AF_NETLINK)?,
                    arg(0, SeccompCmpOp::Ne, libc::AF_INET)?,
                    arg(0, SeccompCmpOp::Ne, libc::AF_INET6)?,
                ])
                .map_err(|e| e.to_string())?,
            );
            // SOCK_DGRAM, RAW, RDM, SEQPACKET, DCCP, PACKET (type & 0xf;
            // the high bits are SOCK_NONBLOCK/SOCK_CLOEXEC).
            for family in [libc::AF_INET, libc::AF_INET6] {
                for ty in [2, 3, 4, 5, 6, 10] {
                    socket.push(
                        SeccompRule::new(vec![
                            arg(0, SeccompCmpOp::Eq, family)?,
                            arg(1, SeccompCmpOp::MaskedEq(0xf), ty)?,
                        ])
                        .map_err(|e| e.to_string())?,
                    );
                }
            }
        } else {
            socket.push(
                SeccompRule::new(vec![
                    arg(0, SeccompCmpOp::Ne, libc::AF_UNIX)?,
                    arg(0, SeccompCmpOp::Ne, libc::AF_NETLINK)?,
                ])
                .map_err(|e| e.to_string())?,
            );
        }
        rules.insert(libc::SYS_socket, socket);
        rules.insert(
            libc::SYS_socketpair,
            vec![
                SeccompRule::new(vec![arg(0, SeccompCmpOp::Ne, libc::AF_UNIX)?])
                    .map_err(|e| e.to_string())?,
            ],
        );
        SeccompFilter::new(
            rules,
            SeccompAction::Allow,
            SeccompAction::Errno(libc::EPERM as u32),
            arch,
        )
        .map_err(|e| e.to_string())?
        .try_into()
        .map_err(|e: seccompiler::BackendError| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SandboxMode, SandboxSpec};
    use std::collections::BTreeMap;

    /// A fake tree: `/`, `/etc`, `/home/u/{proj,.ssh,.bashrc,.netrc,link}`,
    /// `/home/u/proj/{src,.env}`, `/usr`.
    fn fake_list(dir: &Path) -> Vec<Entry> {
        let tree: BTreeMap<&str, Vec<(&str, bool)>> = BTreeMap::from([
            (
                "/",
                vec![
                    ("etc", false),
                    ("home", false),
                    ("usr", false),
                    ("bin", true),
                ],
            ),
            ("/home", vec![("u", false)]),
            (
                "/home/u",
                vec![
                    (".bashrc", false),
                    (".netrc", false),
                    (".ssh", false),
                    ("link", true),
                    ("proj", false),
                ],
            ),
            ("/home/u/proj", vec![(".env", false), ("src", false)]),
        ]);
        tree.get(dir.to_str().unwrap())
            .map(|names| {
                names
                    .iter()
                    .map(|(n, symlink)| Entry {
                        path: dir.join(n),
                        symlink: *symlink,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn resolved(network: bool, ports: Vec<u16>) -> Resolved {
        let spec = SandboxSpec {
            mode: SandboxMode::WorkspaceWrite,
            network,
            workspace: "/home/u/proj".into(),
            writable: vec![],
            writable_prefixes: vec!["/home/u/.claude.json".into()],
            read_only: vec!["/home/u/proj/.git/hooks".into()],
            hidden: vec![
                "/home/u/.ssh".into(),
                "/home/u/.netrc".into(),
                "/home/u/proj/.env".into(),
            ],
            loopback_ports: ports,
        };
        // Built by hand: these paths must not be canonicalised against the
        // host running the test.
        let mut r = Resolved::new(&spec, vec![]).unwrap();
        r.workspace = "/home/u/proj".into();
        r.write = vec!["/home/u/proj".into(), "/tmp".into()];
        r.write_prefixes = vec!["/home/u/.claude.json".into()];
        r.hidden = spec.hidden.clone();
        r.read_only = spec.read_only.clone();
        r
    }

    #[test]
    fn split_grants_siblings_of_hidden_paths_and_skips_symlinks() {
        let p = plan(&resolved(false, vec![]), &fake_list);
        assert_eq!(
            p.read,
            vec![
                PathBuf::from("/etc"),
                "/home/u/.bashrc".into(),
                "/home/u/proj/src".into(),
                "/usr".into(),
            ]
        );
        assert_eq!(
            p.write,
            vec![
                PathBuf::from("/home/u/proj/src"),
                "/tmp".into(),
                "/home/u/.claude.json".into(),
            ]
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn seccomp_filters_compile_for_every_architecture() {
        use seccompiler::TargetArch;
        for arch in [TargetArch::x86_64, TargetArch::aarch64, TargetArch::riscv64] {
            for allow_tcp in [false, true] {
                assert!(seccomp::filter(allow_tcp, arch).is_ok());
            }
        }
    }

    #[test]
    fn network_enforcement_depends_on_ports_and_abi() {
        assert!(network_enforceable(&resolved(true, vec![1]), 1));
        assert!(network_enforceable(&resolved(false, vec![]), 1));
        assert!(!network_enforceable(&resolved(false, vec![27654]), 3));
        assert!(network_enforceable(&resolved(false, vec![27654]), 4));
        let e = enforcement(&resolved(false, vec![27654]), 3);
        assert!(!e.network && !e.read_only && e.writes && e.hidden);
        assert!(e.notes.iter().any(|n| n.contains("NOT enforced")));
        assert!(!e.is_complete());
    }
}
