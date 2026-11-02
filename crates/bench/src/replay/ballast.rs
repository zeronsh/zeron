//! Touched anonymous memory and owned idle children, without periodic wakeups.
use anyhow::{Context, Result, bail};
use std::process::{Child, Command, Stdio};

pub struct Ballast {
    _bytes: Vec<u8>,
}
impl Ballast {
    pub fn new(mib: usize) -> Result<Self> {
        let len = mib
            .checked_mul(1024 * 1024)
            .context("ballast size overflow")?;
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(len).context("reserve ballast")?;
        bytes.resize(len, 0);
        // Volatile stores force private physical pages even with optimizations.
        for at in (0..len).step_by(4096) {
            unsafe {
                std::ptr::write_volatile(bytes.as_mut_ptr().add(at), 1);
            }
        }
        Ok(Self { _bytes: bytes })
    }
}

pub struct Children(Vec<Child>);
impl Children {
    pub fn from_env() -> Result<Self> {
        let mut children = Self(Vec::new());
        if let Ok(spec) = std::env::var("ZB_REPLAY_CHILDREN") {
            let (n, mib) = spec.split_once(':').context("children must be n:MiB")?;
            let n: usize = n.parse()?;
            let mib: usize = mib.parse()?;
            if n > 4096 {
                bail!("too many ballast children");
            }
            for _ in 0..n {
                let mut cmd = Command::new(std::env::current_exe()?);
                cmd.args(["--ballast-child", &mib.to_string()])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null());
                #[cfg(target_os = "linux")]
                {
                    use std::os::unix::process::CommandExt;
                    let parent = std::process::id();
                    unsafe {
                        cmd.pre_exec(move || {
                            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
                            if libc::getppid() as u32 != parent {
                                libc::_exit(1);
                            }
                            Ok(())
                        });
                    }
                }
                children.0.push(cmd.spawn()?);
            }
        }
        Ok(children)
    }
}
impl Drop for Children {
    fn drop(&mut self) {
        for child in &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub fn child(mib: usize) -> Result<()> {
    let _ballast = Ballast::new(mib)?;
    loop {
        unsafe {
            libc::pause();
        }
    }
}
