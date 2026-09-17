use std::path::{Path, PathBuf};

use crate::HarnessError;

pub(super) fn home() -> Result<PathBuf, HarnessError> {
    let cwd = std::env::current_dir()?;
    // sign-in starts in home; anchor every launch there so project cwd changes
    // cannot switch the settings and credentials used after authentication.
    let child_cwd = std::env::var_os("HOME")
        .map(|home| cwd.join(home))
        .unwrap_or(cwd);
    let gemini_home = std::env::var_os("GEMINI_HOME");
    let home = user_home()?;
    resolve_home(
        gemini_home.as_deref().map(Path::new),
        home.as_deref(),
        &child_cwd,
    )
}

pub(super) fn resolve_home(
    gemini_home: Option<&Path>,
    home: Option<&Path>,
    child_cwd: &Path,
) -> Result<PathBuf, HarnessError> {
    let path = match gemini_home {
        Some(path) if path.as_os_str().is_empty() => {
            return Err(HarnessError::Protocol(
                "GEMINI_HOME is empty; unset it or set an absolute path before signing in".into(),
            ));
        }
        Some(path) => expand_user(path, home)?,
        None => expand_user(Path::new("~/.gemini"), home)?,
    };
    let resolved = child_cwd.join(path);
    if !resolved.is_absolute() {
        return Err(HarnessError::Protocol(
            "cannot resolve GEMINI_HOME to an absolute directory; set an absolute path".into(),
        ));
    }
    Ok(resolved)
}

#[cfg(unix)]
fn user_home() -> Result<Option<PathBuf>, HarnessError> {
    match std::env::var_os("HOME") {
        Some(home) => Ok(Some(PathBuf::from(home))),
        None => passwd_entry(None).map(|(_, home)| Some(home)),
    }
}

#[cfg(windows)]
fn user_home() -> Result<Option<PathBuf>, HarnessError> {
    Ok(std::env::var_os("USERPROFILE")
        .or_else(|| {
            let path = std::env::var_os("HOMEPATH")?;
            let mut drive = std::env::var_os("HOMEDRIVE").unwrap_or_default();
            drive.push(path);
            Some(drive)
        })
        .map(PathBuf::from))
}

pub(super) fn expand_user(path: &Path, home: Option<&Path>) -> Result<PathBuf, HarnessError> {
    let mut components = path.components();
    let Some(first) = components.next() else {
        return Ok(path.to_path_buf());
    };
    let first = first.as_os_str();
    if !first.as_encoded_bytes().starts_with(b"~") {
        return Ok(path.to_path_buf());
    }
    let name = first.to_str().ok_or_else(|| {
        HarnessError::Protocol("cannot resolve a non-Unicode home name in GEMINI_HOME".into())
    })?;
    let expanded = if name == "~" {
        home.filter(|home| !home.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .ok_or_else(|| {
                HarnessError::Protocol(
                    "cannot resolve ~ in GEMINI_HOME; set an absolute path".into(),
                )
            })?
    } else {
        named_home(&name[1..])?
    };
    Ok(expanded.join(components.as_path()))
}

#[cfg(unix)]
fn named_home(name: &str) -> Result<PathBuf, HarnessError> {
    passwd_entry(Some(name)).map(|(_, home)| home)
}

#[cfg(windows)]
fn named_home(name: &str) -> Result<PathBuf, HarnessError> {
    let home = user_home()?.ok_or_else(|| {
        HarnessError::Protocol(
            "cannot resolve a named home in GEMINI_HOME; set an absolute path".into(),
        )
    })?;
    let username = std::env::var_os("USERNAME");
    if username.as_deref() == Some(std::ffi::OsStr::new(name)) {
        return Ok(home);
    }
    if username.is_some() && home.file_name() == username.as_deref() {
        if let Some(parent) = home.parent() {
            return Ok(parent.join(name));
        }
    }
    Err(HarnessError::Protocol(
        "cannot resolve a named home in GEMINI_HOME; set an absolute path".into(),
    ))
}

#[cfg(unix)]
pub(super) fn passwd_entry(name: Option<&str>) -> Result<(String, PathBuf), HarnessError> {
    use std::ffi::{CStr, CString, OsStr};
    use std::os::unix::ffi::OsStrExt;

    let name = name
        .map(CString::new)
        .transpose()
        .map_err(|_| HarnessError::Protocol("invalid home name in GEMINI_HOME".into()))?;
    let mut buffer = vec![0_u8; 4096];
    loop {
        let mut entry = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut result = std::ptr::null_mut();
        // reentrant lookups keep other auth tasks from overwriting the returned strings.
        let status = unsafe {
            match &name {
                Some(name) => libc::getpwnam_r(
                    name.as_ptr(),
                    entry.as_mut_ptr(),
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                    &mut result,
                ),
                None => libc::getpwuid_r(
                    libc::getuid(),
                    entry.as_mut_ptr(),
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                    &mut result,
                ),
            }
        };
        if status == libc::ERANGE && buffer.len() < 1024 * 1024 {
            buffer.resize(buffer.len() * 2, 0);
            continue;
        }
        if status != 0 || result.is_null() {
            return Err(HarnessError::Protocol(
                "cannot resolve GEMINI_HOME through the user database; set an absolute path".into(),
            ));
        }
        // a successful lookup initializes entry; its strings live in buffer until copied.
        let (username, home) = unsafe {
            let entry = entry.assume_init();
            if entry.pw_name.is_null() || entry.pw_dir.is_null() {
                return Err(HarnessError::Protocol(
                    "user database returned an incomplete home entry".into(),
                ));
            }
            (
                CStr::from_ptr(entry.pw_name).to_string_lossy().into_owned(),
                PathBuf::from(OsStr::from_bytes(CStr::from_ptr(entry.pw_dir).to_bytes())),
            )
        };
        if !home.is_absolute() {
            return Err(HarnessError::Protocol(
                "user database returned a non-absolute home directory".into(),
            ));
        }
        return Ok((username, home));
    }
}
