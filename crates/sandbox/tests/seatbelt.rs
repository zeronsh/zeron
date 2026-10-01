//! Real-process tests for the Seatbelt backend: every assertion spawns a
//! process under the generated profile through `/usr/bin/sandbox-exec`.
//! Fixtures live under the target dir — not in a temp folder, which the
//! profile always leaves writable.
#![cfg(target_os = "macos")]

use std::ffi::OsString;
use std::io::Read;
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use zeron_proto::{AgentPolicy, HarnessId};
use zeron_sandbox::{
    Backend, Helper, SandboxMode, SandboxSpec, available, default_agent_paths, wrap, wrap_with,
};

struct Fixture {
    _root: tempfile::TempDir,
    base: PathBuf,
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
        base,
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

struct Out {
    ok: bool,
    text: String,
}

fn run_env(spec: &SandboxSpec, program: &Path, args: &[&str], env: &[(OsString, OsString)]) -> Out {
    let args: Vec<OsString> = args.iter().map(OsString::from).collect();
    let w = wrap(spec, program, &args).unwrap();
    assert_eq!(w.enforcement.backend, Backend::Seatbelt);
    assert!(w.enforcement.is_complete(), "{:?}", w.enforcement);
    let out = Command::new(&w.program)
        .args(&w.args)
        .envs(w.env.iter().chain(env).map(|(k, v)| (k, v)))
        .current_dir(&spec.workspace)
        .output()
        .unwrap();
    Out {
        ok: out.status.success(),
        text: format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    }
}

fn run(spec: &SandboxSpec, program: &Path, args: &[&str]) -> Out {
    run_env(spec, program, args, &[])
}

fn sh(spec: &SandboxSpec, script: &str) -> Out {
    run(spec, Path::new("/bin/sh"), &["-c", script])
}

fn q(p: &Path) -> String {
    format!("'{}'", p.display().to_string().replace('\'', r"'\''"))
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .chain(
            ["/opt/homebrew/bin", "/usr/local/bin"]
                .iter()
                .map(PathBuf::from),
        )
        .chain(std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/bin")))
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

#[test]
fn seatbelt_is_the_best_backend_here() {
    assert_eq!(available(), vec![Backend::Seatbelt]);
}

#[test]
fn workspace_write_confines_writes_and_hides_secrets() {
    let f = fixture();
    let s = spec(&f, SandboxMode::WorkspaceWrite, true);

    let out = sh(
        &s,
        &format!("echo hi > {} && cat {0}", q(&f.workspace.join("inside"))),
    );
    assert!(
        out.ok && out.text.contains("hi"),
        "workspace write: {}",
        out.text
    );

    let out = sh(&s, &format!("echo hi > {}", q(&f.outside.join("nope"))));
    assert!(!out.ok, "write outside the workspace succeeded");
    assert!(out.text.contains("Operation not permitted"), "{}", out.text);
    assert!(!f.outside.join("nope").exists());

    let out = sh(&s, &format!("cat {}", q(&f.outside.join("readable.txt"))));
    assert!(
        out.ok && out.text.contains("public"),
        "read elsewhere: {}",
        out.text
    );

    let out = sh(&s, &format!("cat {}", q(&f.secret.join("key"))));
    assert!(
        !out.ok && !out.text.contains("hunter2"),
        "hidden file read: {}",
        out.text
    );
    let out = sh(&s, &format!("ls {}", q(&f.secret)));
    assert!(
        !out.ok && !out.text.contains("key"),
        "hidden dir listed: {}",
        out.text
    );
    let out = sh(&s, &format!("echo x > {}", q(&f.secret.join("planted"))));
    assert!(!out.ok, "hidden dir written");

    // $TMPDIR (per-user /private/var/folders/…/T) and /tmp.
    let out = sh(
        &s,
        "f=\"$TMPDIR/zeron-sbx-$$\" && echo t > \"$f\" && rm \"$f\" && \
         echo t > /tmp/zeron-sbx-$$ && rm /tmp/zeron-sbx-$$ && echo temp-ok",
    );
    assert!(
        out.ok && out.text.contains("temp-ok"),
        "temp write: {}",
        out.text
    );

    let out = sh(&s, "echo x > /dev/null && echo devnull-ok");
    assert!(out.ok && out.text.contains("devnull-ok"), "{}", out.text);
}

#[test]
fn read_only_mode_forbids_workspace_writes_but_not_temp() {
    let f = fixture();
    let s = spec(&f, SandboxMode::ReadOnly, true);
    let out = sh(&s, &format!("echo hi > {}", q(&f.workspace.join("inside"))));
    assert!(!out.ok, "ReadOnly allowed a workspace write");
    let out = sh(&s, &format!("cat {}", q(&f.outside.join("readable.txt"))));
    assert!(out.ok, "{}", out.text);
    let out = sh(
        &s,
        "echo t > \"$TMPDIR/zeron-sbx-ro-$$\" && rm \"$TMPDIR/zeron-sbx-ro-$$\"",
    );
    assert!(out.ok, "temp write in ReadOnly: {}", out.text);
}

#[test]
fn git_hooks_and_config_stay_read_only_and_cant_be_renamed_away() {
    let f = fixture();
    let git = Command::new("git")
        .args(["init", "-q"])
        .current_dir(&f.workspace)
        .status()
        .unwrap();
    assert!(git.success());
    let policy = AgentPolicy {
        sandbox: SandboxMode::WorkspaceWrite,
        ..AgentPolicy::default()
    };
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    let s = SandboxSpec::for_agent(HarnessId::Mock, &policy, &f.workspace, &home);
    let git_dir = f.workspace.join(".git");

    let out = sh(
        &s,
        &format!("echo evil > {}", q(&git_dir.join("hooks/pre-commit"))),
    );
    assert!(!out.ok, "planted a git hook");
    let out = sh(
        &s,
        &format!(
            "echo '[core] fsmonitor = evil' >> {}",
            q(&git_dir.join("config"))
        ),
    );
    assert!(!out.ok, "rewrote .git/config");
    let out = sh(
        &s,
        &format!("mv {} {}", q(&git_dir), q(&f.workspace.join("moved"))),
    );
    assert!(!out.ok, "renamed .git away from its protection");
    assert!(git_dir.join("hooks").exists());

    // Ordinary git work still functions.
    let out = sh(
        &s,
        "echo a > a.txt && git add a.txt && \
         git -c user.name=t -c user.email=t@t commit -qm init && git log --oneline",
    );
    assert!(
        out.ok && out.text.contains("init"),
        "commit failed: {}",
        out.text
    );
}

#[test]
fn hidden_paths_inside_the_workspace_win() {
    let f = fixture();
    std::fs::write(f.workspace.join(".env"), "TOKEN=hunter2").unwrap();
    let mut s = spec(&f, SandboxMode::WorkspaceWrite, true);
    s.hidden.push(f.workspace.join(".env"));
    let out = sh(&s, "cat .env");
    assert!(!out.ok && !out.text.contains("hunter2"), "{}", out.text);
    let out = sh(&s, "echo x > .env");
    assert!(!out.ok);
    let out = sh(&s, "echo x > other && cat other");
    assert!(out.ok, "{}", out.text);
}

#[test]
fn writable_prefixes_cover_atomic_replace_siblings_only() {
    let f = fixture();
    let state = f.outside.join("state.json");
    std::fs::write(&state, "{}").unwrap();
    let mut s = spec(&f, SandboxMode::WorkspaceWrite, true);
    s.writable_prefixes.push(state.clone());
    let tmp = f.outside.join("state.json.tmp.123.abc");
    let out = sh(
        &s,
        &format!("echo '{{\"a\":1}}' > {} && mv {0} {}", q(&tmp), q(&state)),
    );
    assert!(out.ok, "atomic replace: {}", out.text);
    assert_eq!(std::fs::read_to_string(&state).unwrap().trim(), "{\"a\":1}");
    let out = sh(
        &s,
        &format!("echo x > {}", q(&f.outside.join("other.json"))),
    );
    assert!(!out.ok, "a non-sibling was writable");
}

#[test]
fn paths_with_quotes_and_backslashes_are_escaped() {
    let f = fixture();
    let weird = f.base.join("we\"ird\\ dir) (allow default");
    std::fs::create_dir_all(&weird).unwrap();
    let mut s = spec(&f, SandboxMode::ReadOnly, true);
    s.writable.push(weird.clone());
    let out = sh(
        &s,
        &format!("echo ok > {} && echo written", q(&weird.join("f"))),
    );
    assert!(out.ok && out.text.contains("written"), "{}", out.text);
    // The name didn't inject `(allow default)`.
    let out = sh(&s, &format!("echo x > {}", q(&f.outside.join("nope"))));
    assert!(!out.ok);
}

fn loopback_listener() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let _ = stream.read(&mut [0u8; 1]);
        }
    });
    port
}

fn internet() -> bool {
    ("example.com", 443)
        .to_socket_addrs()
        .ok()
        .and_then(|mut a| a.next())
        .is_some_and(|a| TcpStream::connect_timeout(&a, Duration::from_secs(5)).is_ok())
}

#[test]
fn network_off_blocks_the_internet_but_not_loopback() {
    let f = fixture();
    let port = loopback_listener();
    let s = spec(&f, SandboxMode::WorkspaceWrite, false).with_loopback_port(port);

    let out = run(
        &s,
        Path::new("/usr/bin/nc"),
        &["-z", "-w", "3", "127.0.0.1", &port.to_string()],
    );
    assert!(
        out.ok,
        "loopback unreachable with network off: {}",
        out.text
    );
    let out = run(
        &s,
        Path::new("/usr/bin/nc"),
        &["-z", "-w", "3", "localhost", &port.to_string()],
    );
    assert!(
        out.ok,
        "localhost unreachable with network off: {}",
        out.text
    );

    // A server the agent starts on loopback, and a client to it.
    let out = sh(
        &s,
        "/usr/bin/python3 -c 'import socket\n\
s=socket.socket(); s.bind((\"127.0.0.1\",0)); s.listen(1)\n\
c=socket.create_connection(s.getsockname()); print(\"self-loopback-ok\")'",
    );
    assert!(
        out.ok && out.text.contains("self-loopback-ok"),
        "{}",
        out.text
    );

    let out = run(
        &s,
        Path::new("/usr/bin/curl"),
        &[
            "-sS",
            "--max-time",
            "10",
            "-o",
            "/dev/null",
            "https://example.com",
        ],
    );
    assert!(!out.ok, "reached the internet with network off");
    if internet() {
        // By IP too, so this isn't just DNS failing.
        let ip = ("example.com", 443)
            .to_socket_addrs()
            .unwrap()
            .next()
            .unwrap()
            .ip()
            .to_string();
        let out = run(&s, Path::new("/usr/bin/nc"), &["-z", "-w", "5", &ip, "443"]);
        assert!(!out.ok, "TCP to {ip} succeeded with network off");
        let out = sh(
            &s,
            "/usr/bin/python3 -c 'import socket\n\
s=socket.socket(socket.AF_INET, socket.SOCK_DGRAM); s.sendto(b\"x\", (\"1.1.1.1\", 53))'",
        );
        assert!(!out.ok, "UDP left the machine with network off");
    }
}

#[test]
fn network_on_reaches_the_internet() {
    if !internet() {
        eprintln!("skipped: no internet outside the sandbox");
        return;
    }
    let f = fixture();
    let s = spec(&f, SandboxMode::WorkspaceWrite, true);
    let out = run(
        &s,
        Path::new("/usr/bin/curl"),
        &[
            "-sS",
            "--max-time",
            "20",
            "-o",
            "/dev/null",
            "-w",
            "%{http_code}",
            "https://example.com",
        ],
    );
    assert!(out.ok && out.text.contains("200"), "curl: {}", out.text);
}

#[test]
fn node_starts_and_does_tls_under_the_profile() {
    let Some(node) = which("node") else {
        eprintln!("skipped: node not installed");
        return;
    };
    let f = fixture();
    let s = spec(&f, SandboxMode::WorkspaceWrite, true);
    let script = "const fs=require('fs'),os=require('os'),cp=require('child_process');\
        fs.writeFileSync('n.txt','x');\
        fs.writeFileSync(os.tmpdir()+'/zeron-sbx-node-'+process.pid,'x');\
        console.log('cpus',os.cpus().length,require('crypto').randomUUID().length,\
          cp.execSync('echo child').toString().trim(), os.userInfo().username.length>0);";
    let out = run(&s, &node, &["-e", script]);
    assert!(out.ok && out.text.contains("child"), "node: {}", out.text);
    assert!(f.workspace.join("n.txt").exists());
    if internet() {
        let out = run(
            &s,
            &node,
            &[
                "-e",
                "fetch('https://example.com').then(r=>console.log('status',r.status))",
            ],
        );
        assert!(
            out.ok && out.text.contains("status 200"),
            "node fetch: {}",
            out.text
        );
    }
}

/// The real agent CLIs installed here, under the spec a spawn site would
/// build for them: start, and (read-only) confirm the login is still visible
/// — Claude Code's lives in the Keychain, Codex's in ~/.codex/auth.json.
#[test]
fn real_agent_clis_start_under_workspace_write() {
    let f = fixture();
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    let policy = AgentPolicy {
        sandbox: SandboxMode::WorkspaceWrite,
        ..AgentPolicy::default()
    };
    let cases: &[(HarnessId, &str, &[&[&str]])] = &[
        (
            HarnessId::ClaudeCode,
            "claude",
            &[&["--version"], &["auth", "status"]],
        ),
        (
            HarnessId::Codex,
            "codex",
            &[&["--version"], &["login", "status"]],
        ),
        (HarnessId::Pi, "pi", &[&["--version"]]),
    ];
    let mut ran = 0;
    for (harness, bin, invocations) in cases {
        let Some(exe) = which(bin) else {
            eprintln!("skipped {bin}: not installed");
            continue;
        };
        let s = SandboxSpec::for_agent(*harness, &policy, &f.workspace, &home);
        let env = default_agent_paths(*harness, &home).env;
        // Is the CLI logged in at all, outside the sandbox?
        let logged_in_outside = invocations.len() > 1 && {
            let out = Command::new(&exe).args(invocations[1]).output().unwrap();
            let text = String::from_utf8_lossy(&out.stdout).to_string()
                + &String::from_utf8_lossy(&out.stderr);
            text.contains("\"loggedIn\": true") || text.contains("Logged in")
        };
        for args in *invocations {
            let out = run_env(&s, &exe, args, &env);
            eprintln!("{bin} {}: {}", args.join(" "), out.text.trim());
            assert!(
                out.ok,
                "{bin} {args:?} failed under the sandbox: {}",
                out.text
            );
            if args[0] != "--version" && logged_in_outside {
                assert!(
                    out.text.contains("\"loggedIn\": true") || out.text.contains("Logged in"),
                    "{bin} lost its login inside the sandbox: {}",
                    out.text
                );
            }
        }
        ran += 1;
    }
    eprintln!("{ran} agent CLIs checked");
}

/// macOS refuses to apply a *different* Seatbelt profile inside a sandboxed
/// process (`sandbox_apply: Operation not permitted`; only an identical
/// profile re-applies). So a harness's own tool sandbox (Codex
/// `workspace-write`, Claude Code `sandbox.enabled`) can't run inside Zeron's:
/// with Zeron's sandbox on, the harness must run its tools unsandboxed
/// (Codex `danger-full-access`) and rely on the outer profile, which its
/// tool processes inherit.
#[test]
fn a_nested_seatbelt_profile_is_refused() {
    let f = fixture();
    let s = spec(&f, SandboxMode::WorkspaceWrite, true);
    let out = run(
        &s,
        Path::new("/usr/bin/sandbox-exec"),
        &["-p", "(version 1)(allow default)", "/bin/echo", "nested-ok"],
    );
    assert!(
        !out.ok && out.text.contains("sandbox_apply"),
        "{}",
        out.text
    );
}

#[test]
fn landlock_and_bubblewrap_are_unavailable_on_macos() {
    let f = fixture();
    let s = spec(&f, SandboxMode::WorkspaceWrite, true);
    let helper = Helper::current_exe().unwrap();
    for backend in [Backend::Landlock, Backend::Bubblewrap] {
        assert!(wrap_with(backend, &s, Path::new("/bin/true"), &[], &helper).is_err());
    }
}

/// Sends one request line to `exe args…` under `spec` and waits (30s) for a
/// stdout line containing `expect`; the child is killed afterwards.
fn handshake(
    spec: &SandboxSpec,
    exe: &Path,
    args: &[&str],
    env: &[(OsString, OsString)],
    request: &str,
    expect: &str,
) -> Result<String, String> {
    use std::io::{BufRead, Write};
    let args: Vec<OsString> = args.iter().map(OsString::from).collect();
    let w = wrap(spec, exe, &args).unwrap();
    let mut child = Command::new(&w.program)
        .args(&w.args)
        .envs(w.env.iter().chain(env).map(|(k, v)| (k, v)))
        .current_dir(&spec.workspace)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, "{request}").unwrap();
    stdin.flush().unwrap();
    let stdout = child.stdout.take().unwrap();
    let expect = expect.to_owned();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout)
            .lines()
            .map_while(Result::ok)
        {
            if line.contains(&expect) {
                let _ = tx.send(line);
                return;
            }
        }
    });
    let got = rx.recv_timeout(Duration::from_secs(30));
    drop(stdin);
    let _ = child.kill();
    let output = child.wait_with_output().unwrap();
    got.map_err(|_| {
        format!(
            "no answer; stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

/// The CLIs in the mode Zeron actually drives them (Claude stream-json,
/// Codex app-server) answer their initialize handshake under the policy a
/// chat would run with: workspace-write, network off but the engine's
/// loopback port reachable. No turn is sent, so no model is called.
#[test]
fn real_agent_protocols_initialize_with_network_off() {
    let f = fixture();
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    let policy = AgentPolicy {
        sandbox: SandboxMode::WorkspaceWrite,
        network: false,
        ..AgentPolicy::default()
    };
    if let Some(claude) = which("claude") {
        let s = SandboxSpec::for_agent(HarnessId::ClaudeCode, &policy, &f.workspace, &home)
            .with_loopback_port(27654);
        let env = default_agent_paths(HarnessId::ClaudeCode, &home).env;
        let answer = handshake(
            &s,
            &claude,
            &["--print", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose"],
            &env,
            r#"{"type":"control_request","request_id":"zeron-sbx","request":{"subtype":"initialize"}}"#,
            "zeron-sbx",
        )
        .unwrap_or_else(|e| panic!("claude initialize under the sandbox: {e}"));
        assert!(
            answer.contains("control_response") && !answer.contains("\"error\""),
            "{answer}"
        );
    }
    if let Some(codex) = which("codex") {
        let s = SandboxSpec::for_agent(HarnessId::Codex, &policy, &f.workspace, &home)
            .with_loopback_port(27654);
        let answer = handshake(
            &s,
            &codex,
            &["app-server"],
            &[],
            r#"{"id":1,"method":"initialize","params":{"clientInfo":{"name":"zeron-sbx-test","title":"Zeron","version":"0"},"capabilities":{"experimentalApi":true}}}"#,
            "\"id\":1",
        )
        .unwrap_or_else(|e| panic!("codex app-server initialize under the sandbox: {e}"));
        assert!(answer.contains("\"result\""), "{answer}");
    }
}

/// launchd runs jobs outside any sandbox; the profile must not let the agent
/// submit one (a classic Seatbelt escape).
#[test]
fn launchd_jobs_cannot_be_submitted() {
    let f = fixture();
    let s = spec(&f, SandboxMode::WorkspaceWrite, true);
    let label = format!("com.zeron.sandbox-test.{}", std::process::id());
    let pwned = f.outside.join("pwned");
    let out = run(
        &s,
        Path::new("/bin/launchctl"),
        &[
            "submit",
            "-l",
            &label,
            "--",
            "/usr/bin/touch",
            pwned.to_str().unwrap(),
        ],
    );
    std::thread::sleep(Duration::from_secs(2));
    let _ = Command::new("/bin/launchctl")
        .args(["remove", &label])
        .status();
    assert!(
        !pwned.exists(),
        "a launchd job escaped the sandbox ({})",
        out.text
    );
}
