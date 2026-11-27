//! Linux: a detached process's own systemd scope, out of its starter's unit.
//!
//! Under a systemd user manager an app runs in a unit of its own: uwsm and
//! other launchers start each app as a service (`app-…@….service`), GNOME
//! and KDE as a scope. A process the app starts stays in that unit's cgroup
//! whatever its session or process group, and when the unit stops (a
//! service's, as soon as the app exits) systemd kills everything in it. The
//! engine, which is to outlive the app's window (crates/engine's
//! managed.rs), went with it, and every session it ran.
//!
//! So a process started detached from within a user unit is started through
//! `systemd-run --user --scope`, which puts it in a transient scope of its
//! own, then becomes it: the same process, the same environment, in the
//! app's slice, so it still ends with the graphical session as the app does.
//! It's handed back once it's in its scope, so an app that exits straight
//! after doesn't take it down. Where that can't be done (no user manager to
//! reach, no `systemd-run` or one too old for these options, a sandbox), it's
//! started as before.

use std::process::{Child, Command};
use std::time::{Duration, Instant};

/// `command`, to be started in a scope of its own, where this process is in
/// a systemd user unit: start it, then [`handed_over`].
pub(crate) fn own_scope(command: &Command) -> Option<Command> {
    let cgroup = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    Some(scoped(command, user_unit_slice(&cgroup)?))
}

/// `child`, an [`own_scope`] command just started, once `systemd-run` has
/// started its scope and become the command (it does so only then: it waits
/// for the scope's job). None where it gave up and exited instead, the
/// command not started. One the manager is slow to answer is handed back
/// after a while as it is, to carry on by itself.
pub(crate) fn handed_over(mut child: Child) -> Option<Child> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        // Exited (left unreaped, to read), then its name: one read after it
        // exited is the name it exited under.
        let exited = has_exited(&child);
        match std::fs::read_to_string(format!("/proc/{}/comm", child.id())) {
            // It became the command (which may have ended since).
            Ok(comm) if comm.trim_end() != "systemd-run" => return Some(child),
            // It gave up without starting it.
            Ok(_) if exited => {
                let _ = child.wait();
                return None;
            }
            Ok(_) => {}
            // Gone, reaped by someone else: there's nothing more to learn.
            Err(_) => return Some(child),
        }
        if Instant::now() >= deadline {
            return Some(child);
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Whether `child` has exited, without reaping it.
fn has_exited(child: &Child) -> bool {
    // SAFETY: waitid on our own child, with WNOWAIT (it stays to be reaped)
    // and WNOHANG (no blocking); `info` is plain data waitid fills.
    unsafe {
        let mut info: libc::siginfo_t = std::mem::zeroed();
        let flags = libc::WEXITED | libc::WNOHANG | libc::WNOWAIT;
        libc::waitid(libc::P_PID, child.id(), &mut info, flags) == 0 && info.si_pid() != 0
    }
}

/// `command` run through `systemd-run` in a transient scope of the user's
/// manager in `slice`, unloaded once it ends: its program, arguments,
/// environment and directory carried over (what this crate's callers set;
/// not its standard streams, which the caller sets).
fn scoped(command: &Command, slice: &str) -> Command {
    let mut scoped = Command::new("systemd-run");
    scoped
        .args(["--user", "--scope", "--quiet", "--collect"])
        .arg(format!("--slice={slice}"))
        .arg("--")
        .arg(command.get_program())
        .args(command.get_args());
    for (key, value) in command.get_envs() {
        match value {
            Some(value) => scoped.env(key, value),
            None => scoped.env_remove(key),
        };
    }
    if let Some(dir) = command.get_current_dir() {
        scoped.current_dir(dir);
    }
    scoped
}

/// The slice holding the systemd user unit a process is in, from its
/// `/proc/<pid>/cgroup`: cgroup v2's `0::` line, a unit (a service or a
/// scope) under the user's manager (`user@<uid>.service`), and the slice
/// it's in. None outside one: a login session's scope, a system service, a
/// container's root, the manager itself.
fn user_unit_slice(cgroup: &str) -> Option<&str> {
    let path = cgroup.lines().find_map(|line| line.strip_prefix("0::"))?;
    let mut parts = path.split('/');
    parts.find(|part| part.starts_with("user@") && part.ends_with(".service"))?;
    let units: Vec<&str> = parts.collect();
    let [.., slice, unit] = units.as_slice() else {
        return None;
    };
    let unit = unit.ends_with(".service") || unit.ends_with(".scope");
    (unit && slice.ends_with(".slice")).then_some(*slice)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    #[test]
    fn a_process_in_a_user_unit_is_in_its_slice() {
        let uwsm = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-graphical.slice/app-Hyprland-zeron@950eb96d.service\n";
        assert_eq!(user_unit_slice(uwsm), Some("app-graphical.slice"));
        let gnome = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-gnome-zeron-4242.scope\n";
        assert_eq!(user_unit_slice(gnome), Some("app.slice"));
        // A hybrid hierarchy lists cgroup v1's controllers too.
        let hybrid = "1:name=systemd:/user.slice/user-1000.slice/user@1000.service/app.slice/kitty-1-0.scope\n0::/user.slice/user-1000.slice/user@1000.service/app.slice/kitty-1-0.scope\n";
        assert_eq!(user_unit_slice(hybrid), Some("app.slice"));
    }

    #[test]
    fn outside_a_user_unit_there_is_no_slice() {
        for cgroup in [
            // A login session, a system service, the manager itself.
            "0::/user.slice/user-1000.slice/session-2.scope\n",
            "0::/system.slice/zeron.service\n",
            "0::/user.slice/user-1000.slice/user@1000.service/init.scope\n",
            // A cgroup a unit delegated, a container's root, cgroup v1.
            "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-x.service/payload\n",
            "0::/\n",
            "1:name=systemd:/user.slice/user-1000.slice/user@1000.service/app.slice/app-x.service\n",
            "",
        ] {
            assert_eq!(user_unit_slice(cgroup), None, "{cgroup}");
        }
    }

    #[test]
    fn a_scoped_command_is_the_command_in_a_scope_of_its_own() {
        let mut command = Command::new("/opt/zeron/zeron-engine");
        command
            .arg("--build-id")
            .env("ZERON_IPC_PORT", "27654")
            .env_remove("ZERON_EDGE_TOKEN")
            .current_dir("/tmp");
        let scoped = scoped(&command, "app-graphical.slice");
        assert_eq!(scoped.get_program(), "systemd-run");
        assert_eq!(
            scoped.get_args().collect::<Vec<_>>(),
            [
                "--user",
                "--scope",
                "--quiet",
                "--collect",
                "--slice=app-graphical.slice",
                "--",
                "/opt/zeron/zeron-engine",
                "--build-id",
            ]
        );
        let envs: Vec<_> = scoped.get_envs().collect();
        assert!(envs.contains(&(OsStr::new("ZERON_IPC_PORT"), Some(OsStr::new("27654")))));
        assert!(envs.contains(&(OsStr::new("ZERON_EDGE_TOKEN"), None)));
        assert_eq!(scoped.get_current_dir(), Some(Path::new("/tmp")));
    }

    /// A stand-in `systemd-run` in `dir` running `script`.
    fn systemd_run(dir: &Path, script: &str) -> Command {
        let path = dir.join("systemd-run");
        std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        Command::new(path)
    }

    #[test]
    fn a_command_is_handed_over_once_systemd_run_has_become_it() {
        let dir = tempfile::tempdir().unwrap();
        // Takes a moment for the scope, then becomes the command.
        let child = systemd_run(dir.path(), "sleep 0.05; exec sleep 30")
            .spawn()
            .unwrap();
        let mut child = handed_over(child).expect("handed over");
        let comm = std::fs::read_to_string(format!("/proc/{}/comm", child.id())).unwrap();
        assert_eq!(comm.trim_end(), "sleep");
        child.kill().unwrap();
        child.wait().unwrap();
        // A command that ends at once is still one that was started.
        let child = systemd_run(dir.path(), "exec true").spawn().unwrap();
        assert!(handed_over(child).is_some());
    }

    #[test]
    fn a_systemd_run_that_gives_up_hands_nothing_over() {
        let dir = tempfile::tempdir().unwrap();
        let child = systemd_run(dir.path(), "exit 1").spawn().unwrap();
        assert!(handed_over(child).is_none());
    }

    /// Where the probe says the pid of the process it started.
    const PROBE_PID_FILE: &str = "ZERON_LAUNCH_PROBE_PID_FILE";

    /// Run by the test below inside a service: starts `sleep` detached and
    /// says its pid, then ends, ending the service.
    #[test]
    #[ignore = "run inside a service by a_detached_process_outlives_the_unit_that_started_it"]
    fn detached_probe() {
        let Some(pid_file) = std::env::var_os(PROBE_PID_FILE) else {
            return;
        };
        let child = crate::spawn_detached(Command::new("sleep").arg("120")).unwrap();
        std::fs::write(pid_file, child.id().to_string()).unwrap();
        crate::reap(child);
    }

    /// Live, under the user's systemd manager: a process started detached
    /// from inside a service outlives it, though the service ends as its main
    /// process exits, as an app's does under uwsm
    /// (`cargo test -p zeron-launch -- --ignored outlives`).
    #[test]
    #[ignore = "requires a systemd user manager"]
    fn a_detached_process_outlives_the_unit_that_started_it() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        let ran = Command::new("systemd-run")
            .args(["--user", "--wait", "--quiet", "--collect"])
            .arg(format!("--setenv={PROBE_PID_FILE}={}", pid_file.display()))
            .arg("--")
            .arg(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "systemd_scope::tests::detached_probe",
                "--ignored",
            ])
            .status()
            .unwrap();
        assert!(ran.success(), "the probe's service: {ran}");
        let pid = std::fs::read_to_string(&pid_file).unwrap();
        let stat = std::fs::read_to_string(Path::new("/proc").join(pid.trim()).join("stat"));
        // Running or sleeping, not gone or a zombie waiting to be reaped.
        let alive = stat.is_ok_and(|stat| {
            let state = stat
                .rsplit_once(") ")
                .and_then(|(_, rest)| rest.chars().next());
            state.is_some_and(|state| state != 'Z' && state != 'X')
        });
        let _ = Command::new("kill").arg(pid.trim()).status();
        assert!(alive, "the detached process went down with the service");
    }
}
