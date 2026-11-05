//! Exercise the actual creation interval, not just cleanup after successful spawn.
use super::*;
use std::io::BufRead;
use std::os::windows::io::AsHandle;
use std::os::windows::process::CommandExt;
use std::time::{Duration, Instant};
use windows_sys::Win32::System::JobObjects::IsProcessInJob;
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, TerminateProcess,
};

// Safety net for failed assertions: kill only the fixture process whose owned
// handle we acquired while it was alive, never by a recycled PID.
struct FixtureProcess(OwnedHandle);
impl Drop for FixtureProcess {
    fn drop(&mut self) {
        unsafe { TerminateProcess(self.0.as_raw_handle(), 99) };
    }
}
impl FixtureProcess {
    fn assert_exited(&self) {
        assert_eq!(
            unsafe { WaitForSingleObject(self.0.as_raw_handle(), 5000) },
            WAIT_OBJECT_0
        );
    }
}
fn parked_command() -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "process::windows::tests::parked_child_helper"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

#[test]
fn parked_child_helper() {
    if let Ok(handle) = std::env::var("ZERON_TEST_INHERITED_EVENT") {
        // Signal only if the parent's sentinel handle accidentally survived the
        // launch allow-list. Handle reuse in this child cannot signal that event.
        unsafe {
            windows_sys::Win32::System::Threading::SetEvent(handle.parse::<usize>().unwrap() as _)
        };
    }
    if std::env::var_os("ZERON_TEST_PARKED_CHILD").is_some() {
        std::thread::sleep(Duration::from_secs(15));
    }
}

#[test]
fn stdin_eof_helper() {
    if let Some(marker) = std::env::var_os("ZERON_TEST_STDIN_EOF_MARKER") {
        let _ = std::io::copy(&mut std::io::stdin(), &mut std::io::sink());
        std::fs::write(marker, "eof").unwrap();
    }
}
fn eof_command(marker: &std::path::Path) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "process::windows::tests::stdin_eof_helper"])
        .env("ZERON_TEST_STDIN_EOF_MARKER", marker)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

/// The teardown's grace is for an agent about to exit on stdin EOF. A child
/// with no stdin pipe, or whose stdin is still held, is terminated at once
/// (`opencode serve` reads no stdin and would otherwise cost every stop the
/// whole grace); one whose handed-off stdin closes gets to exit on its own.
#[tokio::test]
async fn teardown_waits_on_stdin_eof_only_when_one_is_coming() {
    let grace = Duration::from_secs(20);
    let mut parked = parked_command();
    parked.env("ZERON_TEST_PARKED_CHILD", "1");
    let mut child = parked.spawn().unwrap();
    let started = Instant::now();
    crate::shutdown_child(&mut child, grace).await;
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "no stdin pipe: no wait"
    );

    let dir = tempfile::tempdir().unwrap();
    let held_marker = dir.path().join("held");
    let mut child = eof_command(&held_marker).spawn().unwrap();
    let held = child.stdin.take();
    let started = Instant::now();
    crate::shutdown_child(&mut child, grace).await;
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "stdin still held: no wait"
    );
    assert!(!held_marker.exists());
    drop(held);

    let closed_marker = dir.path().join("closed");
    let mut child = eof_command(&closed_marker).spawn().unwrap();
    let stdin = child.stdin.take().unwrap();
    // A writer task drops it just after the teardown starts, as drivers do.
    let writer = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        drop(stdin);
    });
    crate::shutdown_child(&mut child, grace).await;
    writer.await.unwrap();
    assert!(
        closed_marker.exists(),
        "the child exits on its own after EOF"
    );

    let ignored_marker = dir.path().join("ignored");
    let mut child = eof_command(&ignored_marker).spawn().unwrap();
    child.ignore_stdin_eof();
    let started = Instant::now();
    crate::shutdown_child(&mut child, grace).await;
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "ignores EOF: no wait"
    );
}

#[tokio::test]
async fn unrelated_inheritable_handles_are_not_passed_to_agents() {
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::System::Threading::CreateEventW;
    let security = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: std::ptr::null_mut(),
        bInheritHandle: 1,
    };
    let event = unsafe { CreateEventW(&security, 1, 0, std::ptr::null()) };
    assert!(!event.is_null());
    let event = unsafe { OwnedHandle::from_raw_handle(event) };
    let mut command = parked_command();
    command.env(
        "ZERON_TEST_INHERITED_EVENT",
        (event.as_raw_handle() as usize).to_string(),
    );
    let mut child = command.spawn().unwrap();
    assert!(child.wait().await.unwrap().success());
    assert_eq!(
        unsafe { WaitForSingleObject(event.as_raw_handle(), 0) },
        WAIT_TIMEOUT
    );
}

#[test]
fn creation_is_already_in_job_and_setup_failure_rolls_back() {
    let mut processes = Vec::new();
    let result = spawn_prepared(&parked_command(), |child| {
        processes.push(FixtureProcess(child.process.try_clone()?));
        processes.push(FixtureProcess(child.escrow.process.try_clone()?));
        let mut member = 0;
        assert_ne!(
            unsafe {
                IsProcessInJob(
                    child.process.as_raw_handle(),
                    child.job.as_handle().as_raw_handle(),
                    &mut member,
                )
            },
            0
        );
        assert_ne!(member, 0, "child was created outside its job");
        Err(io::Error::other("injected setup failure before resume"))
    });
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("injected setup failure")
    );
    for process in processes {
        process.assert_exited();
    }
}

#[test]
fn ordinary_subprocess_cannot_keep_agent_pipes_open() {
    use std::io::Read;
    let mut command = parked_command();
    command.stdout(Stdio::piped());
    let mut outsider = None;
    let mut reader = None;
    let result = spawn_prepared(&command, |child| {
        // A raw std launch deliberately does not participate in our allow-list.
        let foreign = std::process::Command::new(std::env::current_exe()?)
            .args(["--exact", "process::windows::tests::parked_child_helper"])
            .env("ZERON_TEST_PARKED_CHILD", "1")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()?;
        outsider = Some(foreign);
        let raw = child.stdout.as_ref().unwrap().as_raw_handle();
        reader = Some(std::fs::File::from(
            unsafe { std::os::windows::io::BorrowedHandle::borrow_raw(raw) }
                .try_clone_to_owned()?,
        ));
        Err(io::Error::other("injected setup failure"))
    });
    assert!(result.is_err());
    let mut outsider = outsider.unwrap();
    let guard = FixtureProcess(outsider.as_handle().try_clone_to_owned().unwrap());
    let (done, rx) = std::sync::mpsc::channel();
    let read = std::thread::spawn(move || {
        let _ = done.send(reader.unwrap().read_to_end(&mut Vec::new()));
    });
    let eof = rx.recv_timeout(Duration::from_secs(2));
    let still_running = outsider.try_wait().unwrap().is_none();
    outsider.kill().unwrap();
    outsider.wait().unwrap();
    read.join().unwrap();
    drop(guard);
    assert!(
        still_running,
        "the unrelated fixture exited before checking isolation"
    );
    assert!(
        eof.is_ok(),
        "an unrelated subprocess inherited an agent pipe"
    );
    assert_eq!(eof.unwrap().unwrap(), 0);
}

#[test]
fn crash_interval_owner_helper() {
    let Some(pid_file) = std::env::var_os("ZERON_TEST_CRASH_PID_FILE") else {
        return;
    };
    let mut command = parked_command();
    command.env("ZERON_TEST_PARKED_CHILD", "1");
    let _ = spawn_prepared(&command, |child| {
        std::fs::write(pid_file, format!("{},{}", child.pid, child.escrow.pid))?;
        // Tell the parent that CreateProcessW returned, without starting the
        // monitor or resuming the child. Parent kills this owner at this point.
        let mut gate = String::new();
        let _ = std::io::stdin().lock().read_line(&mut gate);
        std::process::exit(77);
    });
    panic!("crash owner must not finish launch");
}

#[test]
fn owner_death_immediately_after_creation_kills_suspended_child() {
    let temp = tempfile::tempdir().unwrap();
    let pid_file = temp.path().join("pid");
    let mut owner = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "process::windows::tests::crash_interval_owner_helper",
        ])
        .env("ZERON_TEST_CRASH_PID_FILE", &pid_file)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .unwrap();
    let owner_guard = FixtureProcess(owner.as_handle().try_clone_to_owned().unwrap());
    let deadline = Instant::now() + Duration::from_secs(10);
    let pids = loop {
        if let Ok(text) = std::fs::read_to_string(&pid_file) {
            if let Ok(pids) = text
                .split(',')
                .map(str::parse::<u32>)
                .collect::<Result<Vec<_>, _>>()
            {
                if pids.len() == 2 {
                    break pids;
                }
            }
        }
        assert!(
            Instant::now() < deadline,
            "owner never reached creation interval"
        );
        assert!(
            owner.try_wait().unwrap().is_none(),
            "owner exited before creation"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    let children: Vec<_> = pids
        .into_iter()
        .map(|pid| {
            let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_TERMINATE, 0, pid) };
            assert!(!handle.is_null());
            FixtureProcess(unsafe { OwnedHandle::from_raw_handle(handle) })
        })
        .collect();
    owner.kill().unwrap(); // hard exit: none of the owner's Rust destructors run
    owner.wait().unwrap();
    for child in children {
        child.assert_exited();
    }
    drop(owner_guard);
}

// ---------------------------------------------------------------------------
// Batch (`.cmd`/`.bat`) launches through cmd.exe
// ---------------------------------------------------------------------------

/// Runs only when spawned by `batch_scripts_spawn_through_cmd_with_literal_arguments`
/// through a `.cmd` shim: records the CRT-parsed argv the inner program (the
/// role node plays under a real npm shim) actually received.
#[test]
fn batch_inner_helper() {
    let Some(file) = std::env::var_os("ZERON_TEST_BATCH_ARGS_FILE") else {
        return;
    };
    // argv = [test binary, --exact, helper name, forwarded arguments…]
    let forwarded: Vec<String> = std::env::args().skip(3).collect();
    std::fs::write(file, forwarded.join("\n")).unwrap();
}

#[tokio::test]
async fn batch_scripts_spawn_through_cmd_with_literal_arguments() {
    let dir = tempfile::tempdir().unwrap();
    let received = dir.path().join("received.txt");
    // A real shim shape: forward `%*` to an inner program. What that program's
    // C runtime parses is the contract — exactly what node receives under an
    // npm `.cmd` shim launched by zeron.
    let script = dir.path().join("forward-args.cmd");
    std::fs::write(
        &script,
        format!(
            "@echo off\r\n\"{}\" --exact process::windows::tests::batch_inner_helper %*\r\n",
            std::env::current_exe().unwrap().display()
        ),
    )
    .unwrap();
    let arguments = [
        "plain",
        "a b",
        "trailing \\",
        "日本語 😀",
        "meta & | > < ^ ( ) !",
    ];
    let mut command = Command::new(&script);
    command
        .args(arguments)
        .env("ZERON_TEST_BATCH_ARGS_FILE", &received)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = command.spawn().expect("batch spawn");
    let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .expect("batch deadline")
        .unwrap();
    assert!(status.success(), "batch exit: {status}");
    let output = std::fs::read_to_string(&received).expect("arguments were recorded");
    assert_eq!(
        output.lines().collect::<Vec<_>>(),
        arguments,
        "arguments must reach the inner program verbatim (no splitting, no injection)"
    );
    assert!(!dir.path().join("injected.txt").exists());
}

#[tokio::test]
async fn batch_scripts_report_shim_exit_codes_and_kill_their_tree() {
    let dir = tempfile::tempdir().unwrap();
    let failing = dir.path().join("fail.bat");
    std::fs::write(&failing, "@exit /b 7\r\n").unwrap();
    let mut command = Command::new(&failing);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let status = tokio::time::timeout(Duration::from_secs(10), command.output())
        .await
        .expect("batch deadline")
        .unwrap()
        .status;
    assert_eq!(status.code(), Some(7), "the .bat exit code must propagate");

    // A shim that parks itself behind a long-running grandchild: killing the
    // owned cmd.exe must take the whole job down with it.
    let parked = dir.path().join("parked-tree.cmd");
    std::fs::write(
        &parked,
        "@echo off\r\nstart /b \"\" ping -n 30 127.0.0.1 >nul\r\npause >nul\r\n",
    )
    .unwrap();
    let mut command = Command::new(&parked);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = command.spawn().expect("batch spawn");
    child.start_kill().expect("job terminate");
    let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .expect("kill deadline")
        .unwrap();
    assert!(!status.success());
}

#[tokio::test]
async fn bare_batch_program_resolves_from_child_path() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("only-a-shim.CMD");
    std::fs::write(&script, "@exit /b 0\r\n").unwrap();
    // No extension in the program name: resolution must find the shim on the
    // child's PATH through the PATHEXT variants and spawn it through cmd.exe.
    let mut command = Command::new("only-a-shim");
    command
        .env("PATH", dir.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = command.spawn().expect("shim resolves from child PATH");
    let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .expect("shim deadline")
        .unwrap();
    assert!(status.success(), "shim exit: {status}");
}

// ---------------------------------------------------------------------------
// npm cmd-shims launch their target directly
// ---------------------------------------------------------------------------

/// Records every argument after argv[0] when spawned by the shim tests.
#[test]
fn shim_target_helper() {
    let Some(file) = std::env::var_os("ZERON_TEST_SHIM_ARGS_FILE") else {
        return;
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::fs::write(file, args.join("\n")).unwrap();
}

/// This test binary under another name, without copying it when possible.
fn link_test_binary(to: &std::path::Path) {
    std::fs::create_dir_all(to.parent().unwrap()).unwrap();
    let exe = std::env::current_exe().unwrap();
    if std::fs::hard_link(&exe, to).is_err() {
        std::fs::copy(&exe, to).unwrap();
    }
}

const SHIM_HEAD: &str = "@ECHO off\r\nGOTO start\r\n:find_dp0\r\nSET dp0=%~dp0\r\nEXIT /b\r\n:start\r\nSETLOCAL\r\nCALL :find_dp0\r\n";

async fn run_recording(mut command: Command, received: &std::path::Path) -> Vec<String> {
    command
        .env("ZERON_TEST_SHIM_ARGS_FILE", received)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = command.spawn().expect("shim spawn");
    let status = tokio::time::timeout(Duration::from_secs(20), child.wait())
        .await
        .expect("shim deadline")
        .unwrap();
    assert!(status.success(), "shim exit: {status}");
    std::fs::read_to_string(received)
        .expect("arguments were recorded")
        .split('\n')
        .map(str::to_owned)
        .collect()
}

#[tokio::test]
async fn npm_native_shim_runs_its_exe_without_cmd_limits() {
    let dir = tempfile::tempdir().unwrap();
    link_test_binary(&dir.path().join(r"node_modules\@scope\cli\bin\native.exe"));
    let shim = dir.path().join("native.cmd");
    std::fs::write(
        &shim,
        format!("{SHIM_HEAD}\"%dp0%\\node_modules\\@scope\\cli\\bin\\native.exe\"   %*\r\n"),
    )
    .unwrap();
    // A line break (cmd.exe rejects it) and a command line past cmd.exe's
    // 8191 characters both reach the binary verbatim.
    let long = "x".repeat(9000);
    let mut command = Command::new(&shim);
    command.args([
        "--exact",
        "process::windows::tests::shim_target_helper",
        "line\nbreak",
        "100% & more",
        &long,
    ]);
    let received = dir.path().join("received.txt");
    assert_eq!(
        run_recording(command, &received).await,
        [
            "--exact",
            "process::windows::tests::shim_target_helper",
            "line",
            "break",
            "100% & more",
            long.as_str(),
        ]
    );
}

#[tokio::test]
async fn npm_node_shim_runs_its_script_with_the_node_beside_it() {
    let dir = tempfile::tempdir().unwrap();
    // The role node.exe plays: this test binary, which records its argv.
    link_test_binary(&dir.path().join("node.exe"));
    let script = dir.path().join(r"node_modules\@scope\cli\cli.js");
    std::fs::create_dir_all(script.parent().unwrap()).unwrap();
    std::fs::write(&script, "#!/usr/bin/env node\r\n").unwrap();
    let shim = dir.path().join("claude.cmd");
    std::fs::write(
        &shim,
        format!(
            "{SHIM_HEAD}\r\nIF EXIST \"%dp0%\\node.exe\" (\r\n  SET \"_prog=%dp0%\\node.exe\"\r\n) ELSE (\r\n  SET \"_prog=node\"\r\n  SET PATHEXT=%PATHEXT:;.JS;=;%\r\n)\r\n\r\nendLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & \"%_prog%\"  \"%dp0%\\node_modules\\@scope\\cli\\cli.js\" %*\r\n"
        ),
    )
    .unwrap();
    let mut command = Command::new(&shim);
    command.args(["--exact", "process::windows::tests::shim_target_helper"]);
    let received = dir.path().join("received.txt");
    let args = run_recording(command, &received).await;
    assert_eq!(
        std::path::PathBuf::from(&args[0]),
        std::path::absolute(&script).unwrap()
    );
    assert_eq!(
        &args[1..],
        ["--exact", "process::windows::tests::shim_target_helper"]
    );
}

#[test]
fn cmd_route_rejects_command_lines_past_its_limit() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("plain.cmd");
    std::fs::write(&script, "@exit /b 0\r\n").unwrap();
    let mut command = Command::new(&script);
    command.arg("x".repeat(9000));
    let error = command.spawn().expect_err("over cmd.exe's limit");
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    assert!(error.to_string().contains("8191"), "{error}");
}

#[test]
fn missing_working_directory_is_not_a_missing_program() {
    let dir = tempfile::tempdir().unwrap();
    let mut command = parked_command();
    command.current_dir(dir.path().join("gone"));
    let error = command.spawn().expect_err("cwd does not exist");
    assert_eq!(error.kind(), io::ErrorKind::NotADirectory);
    assert!(error.to_string().contains("working directory"), "{error}");
}

#[tokio::test]
async fn extensionless_program_in_a_dotted_directory_uses_pathext() {
    let dir = tempfile::tempdir().unwrap();
    let profile = dir.path().join("first.last");
    std::fs::create_dir_all(&profile).unwrap();
    std::fs::write(profile.join("tool.cmd"), "@exit /b 0\r\n").unwrap();
    let mut command = Command::new(profile.join("tool"));
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let status = tokio::time::timeout(Duration::from_secs(10), command.spawn().unwrap().wait())
        .await
        .expect("tool deadline")
        .unwrap();
    assert!(status.success());
}
