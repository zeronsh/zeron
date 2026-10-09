use std::path::{Path, PathBuf};

use anyhow::{Context as _, anyhow, ensure};
use zeron_ui::LaunchRequest;

pub fn request(target: Option<&str>, from_terminal: bool) -> anyhow::Result<LaunchRequest> {
    match target {
        Some(url) if url.starts_with("zeron://") => Ok(LaunchRequest::OpenUrl {
            url: url.to_owned(),
        }),
        Some(path) => Ok(LaunchRequest::OpenProject {
            path: project_path(Path::new(path))?,
        }),
        None if from_terminal => {
            let cwd = std::env::current_dir()
                .context("reading the current directory")
                .and_then(|cwd| project_path(&cwd));
            match cwd {
                Ok(path) => Ok(LaunchRequest::OpenProject { path }),
                Err(error) => {
                    eprintln!("zeron: opening without a project: {error:#}");
                    Ok(LaunchRequest::Activate)
                }
            }
        }
        None => Ok(LaunchRequest::Activate),
    }
}

fn project_path(raw: &Path) -> anyhow::Result<String> {
    let absolute =
        std::path::absolute(raw).with_context(|| format!("resolving {}", raw.display()))?;
    let metadata = std::fs::metadata(&absolute).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => anyhow!("{} does not exist", raw.display()),
        _ => anyhow!("{}: {error}", raw.display()),
    })?;
    ensure!(metadata.is_dir(), "{} is not a directory", raw.display());
    let real =
        std::fs::canonicalize(&absolute).with_context(|| format!("resolving {}", raw.display()))?;
    let real = without_verbatim_prefix(real);
    ensure!(
        real.parent().is_some(),
        "{} is a filesystem root, not a project",
        real.display()
    );
    real.into_os_string()
        .into_string()
        .map_err(|path| anyhow!("{} is not valid UTF-8", path.to_string_lossy()))
}

#[cfg(windows)]
fn without_verbatim_prefix(path: PathBuf) -> PathBuf {
    use std::path::{Component, Prefix};
    let mut components = path.components();
    let Some(Component::Prefix(prefix)) = components.next() else {
        return path;
    };
    let root = match prefix.kind() {
        Prefix::VerbatimDisk(letter) => format!("{}:\\", letter as char),
        Prefix::VerbatimUNC(server, share) => format!(
            "\\\\{}\\{}\\",
            server.to_string_lossy(),
            share.to_string_lossy()
        ),
        _ => return path,
    };
    let mut plain = PathBuf::from(root);
    plain.extend(components.filter(|component| !matches!(component, Component::RootDir)));
    plain
}

#[cfg(not(windows))]
fn without_verbatim_prefix(path: PathBuf) -> PathBuf {
    path
}

#[cfg(unix)]
pub fn controlling_terminal() -> bool {
    let fd = unsafe { libc::open(c"/dev/tty".as_ptr(), libc::O_RDWR | libc::O_NOCTTY) };
    if fd < 0 {
        return false;
    }
    unsafe { libc::close(fd) };
    true
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn verbatim_prefixes_are_stripped() {
        assert_eq!(
            without_verbatim_prefix(PathBuf::from(r"\\?\C:\Users\x\proj")),
            PathBuf::from(r"C:\Users\x\proj")
        );
        assert_eq!(
            without_verbatim_prefix(PathBuf::from(r"\\?\UNC\server\share\proj")),
            PathBuf::from(r"\\server\share\proj")
        );
        assert_eq!(
            without_verbatim_prefix(PathBuf::from(r"C:\Users\x")),
            PathBuf::from(r"C:\Users\x")
        );
        assert!(
            without_verbatim_prefix(PathBuf::from(r"\\?\C:\"))
                .parent()
                .is_none()
        );
    }
}
