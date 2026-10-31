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
            cache::private_dir_all(dir)?;
            let value = serde_json::json!({"pgid":pgid,"start":start_id(pgid)});
            cache::atomic_write(&dir.join(pgid.to_string()), value.to_string().as_bytes())?;
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
        // Only still-populated groups: a finished group's number may already belong to
        // an unrelated process group.
        for group in &self.groups {
            if group_alive(*group) {
                signal_group(*group, true);
            }
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
        ps_start(pid)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        String::new()
    }
}
/// Without /proc a recorded group is only trusted if the leader's start time still matches;
/// a bare pgid in a marker left over from before a reboot could name any process group.
#[cfg(unix)]
#[cfg_attr(target_os = "linux", allow(dead_code))]
fn ps_start(pid: u32) -> String {
    std::process::Command::new("ps")
        .args(["-o", "lstart=", "-p", &pid.to_string()])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_default()
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
        let current = start_id(group);
        if !current.is_empty()
            && value["start"]
                .as_str()
                .is_some_and(|s| !s.is_empty() && s != current)
        {
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
        if group_alive(group) {
            signal_group(group, true);
        }
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

#[cfg(all(test, unix))]
mod tests {
    #[test]
    fn ps_start_identifies_a_process_by_its_start_time() {
        let own = super::ps_start(std::process::id());
        assert!(!own.is_empty());
        assert_eq!(own, super::ps_start(std::process::id()));
        assert!(super::ps_start(u32::MAX - 1).is_empty());
    }
}
