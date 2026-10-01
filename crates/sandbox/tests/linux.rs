//! Real-process tests for the Linux backends (Landlock + seccomp through the
//! helper, and bubblewrap). This binary is its own helper: started as
//! `<test> sandbox-exec …` it confines itself and execs the agent, exactly as
//! the `zeron` binary does, so it owns `main` (`harness = false`). Backends
//! the machine lacks are skipped with a message; on other OSes it does
//! nothing.

#[cfg(not(target_os = "linux"))]
fn main() {
    println!("linux sandbox tests: skipped (not Linux)");
}

#[cfg(target_os = "linux")]
fn main() {
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    match args.get(1).and_then(|a| a.to_str()) {
        Some(zeron_sandbox::HELPER_SUBCOMMAND) => zeron_sandbox::helper_main(args[2..].to_vec()),
        Some("probe-connect") => linux::probe_connect(&args[2..]),
        _ => linux::run_all(),
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::ffi::OsString;
    use std::io::Read;
    use std::net::{TcpListener, TcpStream, ToSocketAddrs};
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::Duration;

    use zeron_sandbox::{Backend, Helper, SandboxMode, SandboxSpec, available, wrap_with};

    pub fn probe_connect(args: &[OsString]) -> ! {
        let host = args[0].to_str().unwrap();
        let port: u16 = args[1].to_str().unwrap().parse().unwrap();
        let ok = (host, port)
            .to_socket_addrs()
            .ok()
            .and_then(|mut addrs| addrs.next())
            .is_some_and(|addr| TcpStream::connect_timeout(&addr, Duration::from_secs(5)).is_ok());
        std::process::exit(if ok { 0 } else { 1 });
    }

    struct Fixture {
        _root: tempfile::TempDir,
        workspace: PathBuf,
        outside: PathBuf,
        secret: PathBuf,
    }

    fn fixture() -> Fixture {
        let root = tempfile::Builder::new()
            .prefix("sbx")
            .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
            .unwrap();
        let base = std::fs::canonicalize(root.path()).unwrap();
        let workspace = base.join("workspace");
        let outside = base.join("outside");
        let secret = base.join("secret");
        for d in [&workspace, &outside, &secret] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(outside.join("readable.txt"), "public").unwrap();
        std::fs::write(secret.join("key"), "hunter2").unwrap();
        Fixture {
            _root: root,
            workspace,
            outside,
            secret,
        }
    }

    fn spec(f: &Fixture, mode: SandboxMode, network: bool) -> SandboxSpec {
        let mut s = SandboxSpec::new(mode, network, &f.workspace);
        s.hidden.push(f.secret.clone());
        s
    }

    fn helper() -> Helper {
        Helper {
            program: std::env::current_exe().unwrap(),
            args: vec![zeron_sandbox::HELPER_SUBCOMMAND.into()],
        }
    }

    fn run(backend: Backend, spec: &SandboxSpec, program: &Path, args: &[&str]) -> (bool, String) {
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        let w = wrap_with(backend, spec, program, &args, &helper()).unwrap();
        let out = Command::new(&w.program)
            .args(&w.args)
            .envs(w.env.iter().map(|(k, v)| (k, v)))
            .current_dir(&spec.workspace)
            .output()
            .unwrap();
        (
            out.status.success(),
            format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            ),
        )
    }

    fn sh(backend: Backend, spec: &SandboxSpec, script: &str) -> (bool, String) {
        run(backend, spec, Path::new("/bin/sh"), &["-c", script])
    }

    pub fn run_all() {
        let backends: Vec<Backend> = available()
            .into_iter()
            .filter(|b| matches!(b, Backend::Landlock | Backend::Bubblewrap))
            .collect();
        if backends.is_empty() {
            println!("linux sandbox tests: skipped (no Landlock or bubblewrap here)");
            return;
        }
        for backend in backends {
            println!("== {backend:?}");
            filesystem(backend);
            read_only_mode(backend);
            network(backend);
            println!("ok");
        }
    }

    fn filesystem(backend: Backend) {
        let f = fixture();
        let s = spec(&f, SandboxMode::WorkspaceWrite, true);
        let (ok, out) = sh(
            backend,
            &s,
            &format!("echo hi > {}/inside", f.workspace.display()),
        );
        assert!(ok, "workspace write: {out}");
        let (ok, _) = sh(
            backend,
            &s,
            &format!("echo hi > {}/nope", f.outside.display()),
        );
        assert!(!ok, "write outside the workspace succeeded");
        assert!(!f.outside.join("nope").exists());
        let (ok, out) = sh(
            backend,
            &s,
            &format!("cat {}/readable.txt", f.outside.display()),
        );
        assert!(ok && out.contains("public"), "read elsewhere: {out}");
        let (ok, out) = sh(backend, &s, &format!("cat {}/key", f.secret.display()));
        assert!(
            !ok && !out.contains("hunter2"),
            "hidden file was readable: {out}"
        );
        let (ok, out) = sh(
            backend,
            &s,
            "echo t > \"${TMPDIR:-/tmp}/zeron-sbx-$$\" && rm \"${TMPDIR:-/tmp}/zeron-sbx-$$\"",
        );
        assert!(ok, "temp write: {out}");
    }

    fn read_only_mode(backend: Backend) {
        let f = fixture();
        let s = spec(&f, SandboxMode::ReadOnly, true);
        let (ok, _) = sh(
            backend,
            &s,
            &format!("echo hi > {}/inside", f.workspace.display()),
        );
        assert!(!ok, "ReadOnly allowed a workspace write");
        let (ok, out) = sh(
            backend,
            &s,
            "echo t > /tmp/zeron-sbx-ro-$$ && rm /tmp/zeron-sbx-ro-$$",
        );
        assert!(ok, "temp write in ReadOnly: {out}");
    }

    fn network(backend: Backend) {
        let f = fixture();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let _ = stream.read(&mut [0u8; 1]);
            }
        });
        let me = std::env::current_exe().unwrap();
        let port_s = port.to_string();
        // Resolved outside the sandbox: inside, DNS is off too.
        let external = ("example.com", 80)
            .to_socket_addrs()
            .ok()
            .and_then(|mut a| a.next())
            .filter(|a| TcpStream::connect_timeout(a, Duration::from_secs(5)).is_ok());

        let s = spec(&f, SandboxMode::WorkspaceWrite, false).with_loopback_port(port);
        let (ok, out) = run(backend, &s, &me, &["probe-connect", "127.0.0.1", &port_s]);
        let w = wrap_with(backend, &s, &me, &[], &helper()).unwrap();
        if w.enforcement.network {
            assert!(ok, "loopback port unreachable with network off: {out}");
            if let Some(addr) = external {
                let ip = addr.ip().to_string();
                let (ok, _) = run(backend, &s, &me, &["probe-connect", &ip, "80"]);
                assert!(!ok, "external TCP connect succeeded with network off");
            }
        } else {
            println!(
                "  network off not enforceable here: {:?}",
                w.enforcement.notes
            );
        }

        // No loopback ports: every IP socket is refused.
        let s = spec(&f, SandboxMode::WorkspaceWrite, false);
        let (ok, _) = run(backend, &s, &me, &["probe-connect", "127.0.0.1", &port_s]);
        assert!(
            !ok,
            "host loopback reachable with network off and no loopback ports"
        );
    }
}
