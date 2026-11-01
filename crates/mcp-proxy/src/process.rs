use crate::{cache, config::Config};
use std::{
    collections::BTreeSet,
    fs, io,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{Child, ChildStdin, Command},
    task::JoinHandle,
};

pub struct Process {
    pub child: Child,
    pub stdin: Option<ChildStdin>,
    pub tasks: Vec<JoinHandle<()>>,
    pub pgid: u32,
    pub groups: BTreeSet<u32>,
    pub run_dir: Option<PathBuf>,
    pub server: String,
    #[cfg(windows)]
    job: windows::Job,
}
impl Process {
    pub fn spawn(
        c: &Config,
        command: &str,
        args: &[String],
        path: Option<&std::ffi::OsStr>,
    ) -> io::Result<Self> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(cwd) = &c.cwd {
            cmd.current_dir(cwd);
        }
        if let Some(path) = path {
            cmd.env("PATH", path);
        }
        #[cfg(unix)]
        {
            if command == c.command
                && let Some(arg0) = &c.child_argv0
            {
                use std::os::unix::process::CommandExt;
                cmd.as_std_mut().arg0(arg0);
            }
            cmd.process_group(0);
            #[cfg(target_os = "linux")]
            {
                let parent = std::process::id();
                unsafe {
                    cmd.pre_exec(move || {
                        if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                            return Err(io::Error::last_os_error());
                        }
                        if libc::getppid() != parent as i32 {
                            return Err(io::Error::other("proxy exited before spawn"));
                        }
                        Ok(())
                    });
                }
            }
        }
        #[cfg(windows)]
        let job = windows::Job::prepare(&mut cmd)?;
        let mut child = cmd.spawn()?;
        let pgid = child
            .id()
            .ok_or_else(|| io::Error::other("child has no pid"))?;
        #[cfg(windows)]
        if let Err(e) = job.assign_resume(&child) {
            let _ = child.start_kill();
            return Err(e);
        }
        let stdin = child.stdin.take();
        let mut process = Self {
            child,
            stdin,
            tasks: Vec::new(),
            pgid,
            groups: BTreeSet::from([pgid]),
            run_dir: c.run_dir.clone(),
            server: c.server.clone(),
            #[cfg(windows)]
            job,
        };
        if let Err(e) = process.mark(pgid) {
            signal_group(pgid, true);
            let _ = process.child.start_kill();
            return Err(e);
        }
        Ok(process)
    }
    fn mark(&self, pgid: u32) -> io::Result<()> {
        if let Some(dir) = &self.run_dir {
            record_group_marker(dir, pgid, &self.server)?;
        }
        Ok(())
    }
    pub fn discover_watchdogs(&mut self) {
        for group in watchdog_groups(&self.groups, true) {
            if self.groups.insert(group) {
                let _ = self.mark(group);
            }
        }
    }
    pub async fn stop(mut self) {
        self.discover_watchdogs();
        if let Some(mut stdin) = self.stdin.take() {
            let _ = stdin.shutdown().await;
        }
        for group in &self.groups {
            signal_group(*group, false);
        }
        #[cfg(windows)]
        self.job.terminate();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            let _ = self.child.try_wait();
            if !self.groups.iter().any(|g| group_alive(*g))
                || tokio::time::Instant::now() >= deadline
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        for group in &self.groups {
            signal_group(*group, true);
        }
        let _ = self.child.wait().await;
        #[cfg(target_os = "linux")]
        {
            // This dedicated proxy is a subreaper. Reap only our owned groups,
            // after Tokio has reaped the direct child; never steal another wait.
            let deadline = tokio::time::Instant::now() + Duration::from_millis(200);
            loop {
                let mut pending = false;
                for group in &self.groups {
                    loop {
                        let status = unsafe {
                            libc::waitpid(-(*group as i32), std::ptr::null_mut(), libc::WNOHANG)
                        };
                        if status > 0 {
                            continue;
                        }
                        if status == 0 {
                            pending = true;
                        }
                        break;
                    }
                }
                if !pending || tokio::time::Instant::now() >= deadline {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }

        for task in self.tasks {
            task.abort();
        }
        if let Some(dir) = self.run_dir {
            for group in self.groups {
                let _ = fs::remove_file(dir.join(group.to_string()));
            }
        }
    }
    pub fn stderr_task(&mut self, tail: std::sync::Arc<std::sync::Mutex<Vec<u8>>>) {
        let mut stderr = self.child.stderr.take().unwrap();
        self.tasks.push(tokio::spawn(async move {
            let mut buf = [0; 4096];
            let mut output = tokio::io::stderr();
            while let Ok(n) = stderr.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                {
                    let mut tail = tail.lock().unwrap();
                    tail.extend_from_slice(&buf[..n]);
                    let excess = tail.len().saturating_sub(8192);
                    tail.drain(..excess);
                }
                let _ = output.write_all(&buf[..n]).await;
            }
        }));
    }
}

#[cfg(target_os = "linux")]
fn processes() -> Vec<(u32, u32, u32, String, String)> {
    let Ok(entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .filter_map(|e| {
            let e = e.ok()?;
            let pid = e.file_name().to_str()?.parse().ok()?;
            let stat = fs::read_to_string(e.path().join("stat")).ok()?;
            let fields: Vec<_> = stat.rsplit_once(')')?.1.split_whitespace().collect();
            let group = fields.get(2)?.parse().ok()?;
            let parent = fields.get(1)?.parse().ok()?;
            let state = fields.first()?.to_string();
            let cmd =
                String::from_utf8_lossy(&fs::read(e.path().join("cmdline")).unwrap_or_default())
                    .replace('\0', " ");
            Some((pid, group, parent, state, cmd))
        })
        .collect()
}
#[cfg(all(unix, not(target_os = "linux")))]
fn processes() -> Vec<(u32, u32, u32, String, String)> {
    let Ok(output) = std::process::Command::new("ps")
        .args(["-axo", "pid=,pgid=,ppid=,stat=,command="])
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            Some((
                words.next()?.parse().ok()?,
                words.next()?.parse().ok()?,
                words.next()?.parse().ok()?,
                words.next()?.into(),
                words.collect::<Vec<_>>().join(" "),
            ))
        })
        .collect()
}
#[cfg(unix)]
fn watchdog_groups(groups: &BTreeSet<u32>, adopted: bool) -> BTreeSet<u32> {
    let procs = processes();
    let parents: BTreeSet<_> = procs
        .iter()
        .filter(|(_, g, _, _, _)| groups.contains(g))
        .map(|(pid, _, _, _, _)| *pid)
        .chain(groups.iter().copied())
        .collect();
    procs
        .iter()
        .filter(|(_, _, parent, _, cmd)| {
            cmd.split_whitespace()
                .any(|s| s.contains("telemetry/watchdog/main.js"))
                && ((adopted && cfg!(target_os = "linux") && *parent == std::process::id())
                    || cmd.split_whitespace().any(|s| {
                        s.strip_prefix("--parent-pid=")
                            .and_then(|s| s.parse::<u32>().ok())
                            .is_some_and(|p| parents.contains(&p))
                    }))
        })
        .map(|(_, group, _, _, _)| *group)
        .filter(|g| *g > 1 && *g != unsafe { libc::getpgrp() } as u32)
        .collect()
}
#[cfg(not(unix))]
fn watchdog_groups(_: &BTreeSet<u32>, _: bool) -> BTreeSet<u32> {
    BTreeSet::new()
}
#[cfg(unix)]
fn group_alive(group: u32) -> bool {
    processes()
        .iter()
        .any(|(_, g, _, state, _)| *g == group && !state.starts_with('Z'))
}
#[cfg(not(unix))]
fn group_alive(_: u32) -> bool {
    false
}
/// What distinguishes this process from a later one that reuses its pid: the
/// start tick on Linux, the start time `ps` prints elsewhere. Empty when it
/// cannot be read (no such process, or no way to read it on this platform).
fn start_id(pid: u32) -> String {
    #[cfg(target_os = "linux")]
    {
        return fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|s| {
                s.rsplit_once(')')
                    .and_then(|(_, f)| f.split_whitespace().nth(19))
                    .map(str::to_owned)
            })
            .unwrap_or_default();
    }
    #[cfg(all(unix, not(target_os = "linux")))]
    {
        return std::process::Command::new("ps")
            .args(["-o", "lstart=", "-p", &pid.to_string()])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
            .unwrap_or_default();
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        String::new()
    }
}
/// The kernel's id of this boot. A pid and its start tick can repeat across a
/// reboot; this cannot. Empty where the platform has none (`start_id` already
/// carries an absolute time there).
fn boot_id() -> String {
    #[cfg(target_os = "linux")]
    {
        return fs::read_to_string("/proc/sys/kernel/random/boot_id")
            .map(|s| s.trim().to_owned())
            .unwrap_or_default();
    }
    #[cfg(not(target_os = "linux"))]
    String::new()
}
fn signal_group(group: u32, force: bool) {
    #[cfg(unix)]
    if group > 1 && group != unsafe { libc::getpgrp() } as u32 {
        unsafe {
            libc::kill(
                -(group as i32),
                if force { libc::SIGKILL } else { libc::SIGTERM },
            );
        }
    }
    #[cfg(not(unix))]
    let _ = (group, force);
}

/// One confirmed-live process group recorded under a proxy run directory.
pub struct LiveGroup {
    pub pgid: u32,
    pub server: String,
}

/// Write the marker `cleanup_run_dir` and `live_groups` both understand.
/// `server` is the MCP name. Older markers omit it.
pub fn record_group_marker(dir: &Path, pgid: u32, server: &str) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let value = serde_json::json!({
        "pgid": pgid,
        "start": start_id(pgid),
        "boot": boot_id(),
        "server": server,
    });
    cache::atomic_write(&dir.join(pgid.to_string()), value.to_string().as_bytes())
}

/// Live groups in `dir`. Read-only: nothing is signalled or removed.
///
/// A group counts only when its leader is the process the marker recorded
/// (same boot, same start tick, not a zombie). A gone leader is not counted;
/// cleanup may still reap that marker, because a pid cannot be reused while
/// its group has members.
pub fn live_groups(dir: &Path) -> Vec<LiveGroup> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut groups = Vec::new();
    for entry in entries.flatten() {
        let Some(group) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
            .filter(|group| *group > 1 && *group <= i32::MAX as u32)
        else {
            continue;
        };
        let Ok(raw) = fs::read(entry.path()) else {
            continue;
        };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&raw) else {
            continue;
        };
        if !marker_is_live(group, &value) {
            continue;
        }
        groups.push(LiveGroup {
            pgid: group,
            server: value["server"].as_str().unwrap_or("").to_owned(),
        });
    }
    groups
}

fn marker_is_live(group: u32, value: &serde_json::Value) -> bool {
    if value["pgid"] != group {
        return false;
    }
    let boot = boot_id();
    if value["boot"]
        .as_str()
        .is_some_and(|recorded| !recorded.is_empty() && !boot.is_empty() && recorded != boot)
    {
        return false;
    }
    let current = start_id(group);
    let recorded = value["start"].as_str().unwrap_or_default();
    !current.is_empty() && recorded == current && !leader_is_zombie(group)
}

#[cfg(target_os = "linux")]
fn leader_is_zombie(pgid: u32) -> bool {
    let Ok(stat) = fs::read_to_string(format!("/proc/{pgid}/stat")) else {
        return true;
    };
    match stat
        .rsplit_once(')')
        .and_then(|(_, rest)| rest.split_whitespace().next())
    {
        Some(state) => state.starts_with('Z'),
        None => true,
    }
}

#[cfg(not(target_os = "linux"))]
fn leader_is_zombie(_pgid: u32) -> bool {
    false
}

/// Sweep only numeric proxy-owned marker files. Non-marker run files are untouched.
/// The engine calls this after harness teardown and at boot (WU9a).
pub async fn cleanup_run_dir(dir: &Path) -> io::Result<()> {
    let mut groups = BTreeSet::new();
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    for entry in entries {
        let entry = entry?;
        let Some(group) = entry
            .file_name()
            .to_str()
            .and_then(|v| v.parse::<u32>().ok())
            .filter(|v| *v > 1 && *v <= i32::MAX as u32)
        else {
            continue;
        };
        let Ok(raw) = fs::read(entry.path()) else {
            continue;
        };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&raw) else {
            continue;
        };
        if value["pgid"] != group {
            continue;
        }
        // The marker is only a claim that this pgid was ours. A group is
        // signalled when its leader is provably the process we recorded, or
        // when the leader is gone (a pid cannot be reused while its group has
        // members, so whatever remains is that group). Anything that cannot be
        // told apart from a stranger's process is dropped unsignalled.
        let boot = boot_id();
        if value["boot"]
            .as_str()
            .is_some_and(|b| !b.is_empty() && !boot.is_empty() && b != boot)
        {
            fs::remove_file(entry.path())?;
            continue;
        }
        let current = start_id(group);
        let recorded = value["start"].as_str().unwrap_or_default();
        if !current.is_empty() && recorded != current {
            fs::remove_file(entry.path())?;
            continue;
        }
        groups.insert(group);
    }
    groups.extend(watchdog_groups(&groups, false));
    for group in &groups {
        signal_group(*group, false);
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while groups.iter().any(|g| group_alive(*g)) && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    for group in groups {
        signal_group(group, true);
        let _ = fs::remove_file(dir.join(group.to_string()));
    }
    Ok(())
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::{
        Foundation::*,
        System::{Diagnostics::ToolHelp::*, JobObjects::*, Threading::*},
    };
    pub struct Job(OwnedHandle);
    impl Job {
        pub fn prepare(command: &mut Command) -> io::Result<Self> {
            let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            let job = Self(unsafe { OwnedHandle::from_raw_handle(handle) });
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if unsafe {
                SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    std::mem::size_of_val(&limits) as u32,
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            // Suspend until assigned, preventing a descendant escaping before Job ownership.
            command.creation_flags(CREATE_SUSPENDED);
            Ok(job)
        }
        pub fn assign_resume(&self, child: &Child) -> io::Result<()> {
            if unsafe {
                AssignProcessToJobObject(self.0.as_raw_handle(), child.raw_handle().unwrap())
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
            if snapshot == INVALID_HANDLE_VALUE {
                return Err(io::Error::last_os_error());
            }
            let _snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot) };
            let mut entry = THREADENTRY32 {
                dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
                ..Default::default()
            };
            let mut found = unsafe { Thread32First(snapshot, &mut entry) };
            while found != 0 {
                if entry.th32OwnerProcessID == child.id().unwrap() {
                    let thread =
                        unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
                    if thread.is_null() {
                        return Err(io::Error::last_os_error());
                    }
                    let _thread = unsafe { OwnedHandle::from_raw_handle(thread) };
                    if unsafe { ResumeThread(thread) } == u32::MAX {
                        return Err(io::Error::last_os_error());
                    }
                    return Ok(());
                }
                found = unsafe { Thread32Next(snapshot, &mut entry) };
            }
            Err(io::Error::other("suspended child thread not found"))
        }
        pub fn terminate(&self) {
            unsafe {
                TerminateJobObject(self.0.as_raw_handle(), 1);
            }
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    fn leader(dir: &Path) -> (std::process::Child, PathBuf) {
        use std::os::unix::process::CommandExt;
        let child = std::process::Command::new("sleep")
            .arg("60")
            .process_group(0)
            .spawn()
            .unwrap();
        let marker = dir.join(child.id().to_string());
        (child, marker)
    }
    fn mark(marker: &Path, pgid: u32, start: &str, boot: Option<&str>) {
        let mut value = serde_json::json!({"pgid":pgid,"start":start});
        if let Some(boot) = boot {
            value["boot"] = boot.into();
        }
        fs::write(marker, value.to_string()).unwrap();
    }
    fn alive(child: &mut std::process::Child) -> bool {
        child.try_wait().unwrap().is_none()
    }

    #[tokio::test]
    async fn sweep_kills_the_group_it_recorded() {
        let dir = tempfile::tempdir().unwrap();
        let (mut child, marker) = leader(dir.path());
        mark(&marker, child.id(), &start_id(child.id()), Some(&boot_id()));
        cleanup_run_dir(dir.path()).await.unwrap();
        assert!(!child.wait().unwrap().success());
        assert!(!marker.exists());
    }

    #[tokio::test]
    async fn sweep_spares_a_process_that_reused_the_pid() {
        let dir = tempfile::tempdir().unwrap();
        let (mut child, marker) = leader(dir.path());
        mark(&marker, child.id(), "1", Some(&boot_id()));
        cleanup_run_dir(dir.path()).await.unwrap();
        assert!(alive(&mut child), "a different start tick is a stranger");
        assert!(!marker.exists());
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[tokio::test]
    async fn sweep_spares_a_marker_from_another_boot() {
        // Same pid, same start tick: only the boot id tells the machines apart.
        let dir = tempfile::tempdir().unwrap();
        let (mut child, marker) = leader(dir.path());
        mark(
            &marker,
            child.id(),
            &start_id(child.id()),
            Some("00000000-0000-0000-0000-000000000000"),
        );
        cleanup_run_dir(dir.path()).await.unwrap();
        assert!(alive(&mut child));
        assert!(!marker.exists());
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[tokio::test]
    async fn sweep_spares_a_live_leader_the_marker_cannot_identify() {
        // A marker with no start id (written where none can be read, or when
        // the process had just exited) proves nothing about a live process.
        let dir = tempfile::tempdir().unwrap();
        let (mut child, marker) = leader(dir.path());
        mark(&marker, child.id(), "", Some(&boot_id()));
        cleanup_run_dir(dir.path()).await.unwrap();
        assert!(alive(&mut child));
        assert!(!marker.exists());
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[tokio::test]
    async fn sweep_still_reaps_markers_from_before_boot_ids() {
        let dir = tempfile::tempdir().unwrap();
        let (mut child, marker) = leader(dir.path());
        mark(&marker, child.id(), &start_id(child.id()), None);
        cleanup_run_dir(dir.path()).await.unwrap();
        assert!(!child.wait().unwrap().success());
    }

    #[tokio::test]
    async fn sweep_ignores_unrelated_files_and_malformed_markers() {
        let dir = tempfile::tempdir().unwrap();
        let (mut child, _) = leader(dir.path());
        // A numeric name whose body claims another group, one that is not
        // JSON, and a plain file.
        mark(&dir.path().join(child.id().to_string()), 1, "x", None);
        fs::write(dir.path().join("4242"), "garbage").unwrap();
        fs::write(dir.path().join("hibernate-capture"), "digest").unwrap();
        cleanup_run_dir(dir.path()).await.unwrap();
        assert!(alive(&mut child));
        assert!(dir.path().join("hibernate-capture").exists());
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn live_groups_reports_a_live_marker_and_skips_a_reused_pid() {
        use std::os::unix::process::CommandExt;
        let dir = tempfile::tempdir().unwrap();
        let child = std::process::Command::new("sleep")
            .arg("60")
            .process_group(0)
            .spawn()
            .unwrap();
        struct Reap(Option<std::process::Child>);
        impl Drop for Reap {
            fn drop(&mut self) {
                if let Some(child) = self.0.as_mut() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
        }
        let child = Reap(Some(child));
        let pgid = child.0.as_ref().unwrap().id();
        record_group_marker(dir.path(), pgid, "playwright").unwrap();
        let live = live_groups(dir.path());
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].pgid, pgid);
        assert_eq!(live[0].server, "playwright");
        fs::write(
            dir.path().join("99"),
            serde_json::json!({"pgid": 99, "start": "1", "boot": "nope", "server": "other"})
                .to_string(),
        )
        .unwrap();
        let live = live_groups(dir.path());
        assert_eq!(live.len(), 1);
        assert!(live.iter().all(|group| group.server != "other"));
        drop(child);
    }
}
