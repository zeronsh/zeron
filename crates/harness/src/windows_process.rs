//! Windows process-tree ownership. Job handles are never inherited by children.
use std::io;
use std::os::windows::io::{AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle};
use windows_sys::Win32::System::Console::HPCON;
use windows_sys::Win32::System::JobObjects::{
    CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectExtendedLimitInformation, SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Threading::{
    DeleteProcThreadAttributeList, InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
    PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROC_THREAD_ATTRIBUTE_JOB_LIST,
    PROC_THREAD_ATTRIBUTE_PARENT_PROCESS, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE,
    UpdateProcThreadAttribute,
};

/// Owns a session's entire process tree, including descendants of exited parents.
/// Closing the owner process also closes this handle and kills the job.
#[derive(Debug)]
pub struct Job(OwnedHandle);

/// Creation-time job and inherited-handle lists. Values are borrowed until
/// CreateProcessW returns; the opaque attribute buffer is word-aligned and owned.
pub struct Attributes<'a> {
    buffer: Vec<usize>,
    initialized: bool,
    _values: std::marker::PhantomData<&'a [std::os::windows::io::RawHandle]>,
}
impl<'a> Attributes<'a> {
    pub(crate) fn for_job(jobs: &'a [std::os::windows::io::RawHandle]) -> io::Result<Self> {
        let mut attrs = Self::new(1)?;
        attrs.add(
            PROC_THREAD_ATTRIBUTE_JOB_LIST,
            jobs.as_ptr().cast(),
            std::mem::size_of_val(jobs),
        )?;
        Ok(attrs)
    }
    pub(crate) fn for_parent(
        parent: &'a [std::os::windows::io::RawHandle; 1],
        handles: &'a [std::os::windows::io::RawHandle],
    ) -> io::Result<Self> {
        let mut attrs = Self::new(2)?;
        attrs.add(
            PROC_THREAD_ATTRIBUTE_PARENT_PROCESS,
            parent.as_ptr().cast(),
            std::mem::size_of_val(parent),
        )?;
        attrs.add(
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
            handles.as_ptr().cast(),
            std::mem::size_of_val(handles),
        )?;
        Ok(attrs)
    }

    /// Borrow a live pseudoconsole and job list through process creation.
    pub fn for_console(
        console: &'a HPCON,
        jobs: &'a [std::os::windows::io::RawHandle],
    ) -> io::Result<Self> {
        let mut attrs = Self::new(2)?;
        // PSEUDOCONSOLE takes the handle value, unlike the pointer-to-list attributes.
        attrs.add(
            PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE,
            *console as *const _,
            std::mem::size_of::<HPCON>(),
        )?;
        attrs.add(
            PROC_THREAD_ATTRIBUTE_JOB_LIST,
            jobs.as_ptr().cast(),
            std::mem::size_of_val(jobs),
        )?;
        Ok(attrs)
    }

    fn new(count: u32) -> io::Result<Self> {
        let mut bytes = 0;
        unsafe { InitializeProcThreadAttributeList(std::ptr::null_mut(), count, 0, &mut bytes) };
        if bytes == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut attrs = Self {
            buffer: vec![0; bytes.div_ceil(std::mem::size_of::<usize>())],
            initialized: false,
            _values: std::marker::PhantomData,
        };
        if unsafe { InitializeProcThreadAttributeList(attrs.ptr(), count, 0, &mut bytes) } == 0 {
            return Err(io::Error::last_os_error());
        }
        attrs.initialized = true;
        Ok(attrs)
    }

    // Private: constructors supply valid values borrowed for the list's lifetime.
    fn add(
        &mut self,
        attribute: u32,
        value: *const std::ffi::c_void,
        bytes: usize,
    ) -> io::Result<()> {
        if unsafe {
            UpdateProcThreadAttribute(
                self.ptr(),
                0,
                attribute as usize,
                value,
                bytes,
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    pub fn ptr(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.buffer.as_mut_ptr().cast()
    }
}
impl Drop for Attributes<'_> {
    fn drop(&mut self) {
        if self.initialized {
            unsafe { DeleteProcThreadAttributeList(self.ptr()) };
        }
    }
}

impl Job {
    /// Borrow the non-inheritable job for a creation-time JOB_LIST attribute.
    pub fn as_handle(&self) -> BorrowedHandle<'_> {
        use std::os::windows::io::AsHandle;
        self.0.as_handle()
    }

    pub fn new() -> io::Result<Self> {
        // SAFETY: null security attributes create a non-inheritable, unnamed job.
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: CreateJobObjectW returned an owned, valid handle.
        let job = Self(unsafe { OwnedHandle::from_raw_handle(handle) });
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: the job and fixed-size information buffer remain valid for the call.
        if unsafe {
            SetInformationJobObject(
                job.0.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&limits) as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }

    pub fn terminate(&self) -> io::Result<()> {
        // SAFETY: this handle owns only processes assigned to this session.
        if unsafe { TerminateJobObject(self.0.as_raw_handle(), 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

const MACHINE_ENVIRONMENT: &str = r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment";

/// The persisted PATH a new login session would get (machine entries, then
/// the user's), read now rather than inherited at our start: a Node or CLI
/// installed while zeron runs updates the registry, not our environment.
/// `%VAR%` references resolve against our environment first, then the
/// persisted user and machine variables (nvm-windows' `%NVM_SYMLINK%` is
/// defined by the same install that adds it to PATH).
pub(crate) fn registry_path() -> Option<std::ffi::OsString> {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    let machine = read_registry_string(HKEY_LOCAL_MACHINE, MACHINE_ENVIRONMENT, "Path");
    let user = read_registry_string(HKEY_CURRENT_USER, "Environment", "Path");
    let lookup = |name: &str| {
        std::env::var(name)
            .ok()
            .or_else(|| read_registry_string(HKEY_CURRENT_USER, "Environment", name))
            .or_else(|| read_registry_string(HKEY_LOCAL_MACHINE, MACHINE_ENVIRONMENT, name))
    };
    let entries: Vec<String> = [machine, user]
        .into_iter()
        .flatten()
        .flat_map(|path| {
            path.split(';')
                .map(|entry| crate::executable::expand_windows_vars(entry.trim(), &lookup))
                .collect::<Vec<_>>()
        })
        // An entry naming a variable nobody defines is not a directory.
        .filter(|entry| !entry.is_empty() && !entry.contains('%'))
        .collect();
    (!entries.is_empty()).then(|| entries.join(";").into())
}

/// A REG_SZ / REG_EXPAND_SZ value, unexpanded.
fn read_registry_string(
    root: windows_sys::Win32::System::Registry::HKEY,
    subkey: &str,
    name: &str,
) -> Option<String> {
    use windows_sys::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        RRF_NOEXPAND, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ, RegGetValueW,
    };
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let (subkey, name) = (wide(subkey), wide(name));
    // RRF_RT_REG_EXPAND_SZ is only valid together with RRF_NOEXPAND.
    let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND;
    let mut buffer: Vec<u16> = vec![0; 1024];
    for _ in 0..4 {
        let mut bytes = (buffer.len() * 2) as u32;
        // SAFETY: the key/value names are NUL-terminated and `bytes` is the
        // buffer's true size; the call writes at most that many bytes.
        let status = unsafe {
            RegGetValueW(
                root,
                subkey.as_ptr(),
                name.as_ptr(),
                flags,
                std::ptr::null_mut(),
                buffer.as_mut_ptr().cast(),
                &mut bytes,
            )
        };
        match status {
            ERROR_SUCCESS => {
                buffer.truncate(bytes as usize / 2);
                while buffer.last() == Some(&0) {
                    buffer.pop();
                }
                return Some(String::from_utf16_lossy(&buffer));
            }
            // A concurrent writer can grow the value between calls.
            ERROR_MORE_DATA => buffer.resize((bytes as usize).div_ceil(2) + 1, 0),
            _ => return None,
        }
    }
    None
}
