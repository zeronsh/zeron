//! vendor code-signature checks for archives that zeron installs without a
//! digest pinned in its own source (a registry release newer than the pin).
//!
//! every extracted file must carry a valid signature chaining to the vendor's
//! platform identity: google's apple developer id team on macOS, and an
//! authenticode leaf certificate issued to google llc on windows. linux builds are unsigned,
//! so there the pinned digest remains the only accepted proof.

use std::path::{Path, PathBuf};

use crate::HarnessError;

#[cfg(any(target_os = "macos", windows))]
const VERIFY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);

/// google llc's apple developer id team.
#[cfg(target_os = "macos")]
const GOOGLE_APPLE_TEAM: &str = "EQHXZ8M8AV";

/// whether this platform can prove a download's origin without a pinned digest.
pub const SUPPORTED: bool = cfg!(any(target_os = "macos", windows));

/// verify that every file under `dir` is signed by google.
pub async fn verify_google_signed(dir: &Path) -> Result<(), HarnessError> {
    let files = regular_files(dir)?;
    if files.is_empty() {
        return Err(unsigned("the archive contains no files"));
    }
    for file in files {
        verify_file(&file).await?;
    }
    Ok(())
}

fn regular_files(dir: &Path) -> Result<Vec<PathBuf>, HarnessError> {
    let mut files = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        for entry in std::fs::read_dir(&next)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                files.push(entry.path());
            } else {
                return Err(unsigned(&format!(
                    "{} is not a regular file",
                    entry.path().display()
                )));
            }
        }
    }
    files.sort();
    Ok(files)
}

fn unsigned(detail: &str) -> HarnessError {
    HarnessError::Install(format!("code signature verification failed: {detail}"))
}

#[cfg(target_os = "macos")]
async fn verify_file(file: &Path) -> Result<(), HarnessError> {
    // developer id leaf and intermediate, issued by apple, to google's team.
    let requirement = format!(
        "=anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] exists \
         and certificate leaf[field.1.2.840.113635.100.6.1.13] exists \
         and certificate leaf[subject.OU] = \"{GOOGLE_APPLE_TEAM}\""
    );
    let mut command = crate::process::Command::new("/usr/bin/codesign");
    command
        .args(["--verify", "--strict", "-R"])
        .arg(requirement)
        .arg(file);
    let output = run(command, file).await?;
    if output.status.success() {
        return Ok(());
    }
    Err(unsigned(&format!(
        "{} is not signed by Google: {}",
        file_name(file),
        String::from_utf8_lossy(&output.stderr).trim()
    )))
}

#[cfg(windows)]
async fn verify_file(file: &Path) -> Result<(), HarnessError> {
    let owned = file.to_path_buf();
    let signer = tokio::time::timeout(
        VERIFY_TIMEOUT,
        tokio::task::spawn_blocking(move || authenticode::signer(&owned)),
    )
    .await
    .map_err(|_| {
        unsigned(&format!(
            "checking {} took longer than {}s",
            file_name(file),
            VERIFY_TIMEOUT.as_secs()
        ))
    })?
    .map_err(|error| unsigned(&format!("the verifier failed: {error}")))?;
    match signer {
        Ok(signer) if signer.is_google() => Ok(()),
        Ok(signer) => Err(unsigned(&format!(
            "{} is signed by {signer}, not Google",
            file_name(file)
        ))),
        Err(detail) => Err(unsigned(&format!("{}: {detail}", file_name(file)))),
    }
}

#[cfg(any(windows, test))]
#[derive(Debug, PartialEq, Eq)]
struct Signer {
    common_name: String,
    organization: String,
}

#[cfg(any(windows, test))]
impl Signer {
    fn is_google(&self) -> bool {
        self.common_name == "Google LLC" && self.organization == "Google LLC"
    }
}

#[cfg(any(windows, test))]
impl std::fmt::Display for Signer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CN={}, O={}", self.common_name, self.organization)
    }
}

#[cfg(windows)]
mod authenticode {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Security::Cryptography::{
        CERT_CONTEXT, CERT_NAME_ATTR_TYPE, CertGetNameStringW, szOID_COMMON_NAME,
        szOID_ORGANIZATION_NAME,
    };
    use windows_sys::Win32::Security::WinTrust::{
        WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0, WINTRUST_FILE_INFO,
        WTD_CACHE_ONLY_URL_RETRIEVAL, WTD_CHOICE_FILE, WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE,
        WTD_STATEACTION_VERIFY, WTD_UI_NONE, WTHelperGetProvSignerFromChain,
        WTHelperProvDataFromStateData, WinVerifyTrust,
    };

    use super::Signer;

    /// the embedded authenticode signer of `file`, once windows has verified
    /// the signature and its chain to a trusted root.
    pub(super) fn signer(file: &Path) -> Result<Signer, String> {
        let path: Vec<u16> = file.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut file_info = WINTRUST_FILE_INFO {
            cbStruct: size_of::<WINTRUST_FILE_INFO>() as u32,
            pcwszFilePath: path.as_ptr(),
            ..Default::default()
        };
        // revocation stays offline, as for get-authenticodesignature: a
        // network stall must not hang an update, and the archive itself
        // arrived over tls from google.
        let mut data = WINTRUST_DATA {
            cbStruct: size_of::<WINTRUST_DATA>() as u32,
            dwUIChoice: WTD_UI_NONE,
            fdwRevocationChecks: WTD_REVOKE_NONE,
            dwUnionChoice: WTD_CHOICE_FILE,
            Anonymous: WINTRUST_DATA_0 {
                pFile: &mut file_info,
            },
            dwStateAction: WTD_STATEACTION_VERIFY,
            dwProvFlags: WTD_CACHE_ONLY_URL_RETRIEVAL,
            ..Default::default()
        };
        let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
        // SAFETY: `data` and `file_info` outlive both calls; the state is
        // released by the close action below whatever the verdict.
        let status =
            unsafe { WinVerifyTrust(INVALID_HANDLE_VALUE, &mut action, (&raw mut data).cast()) };
        let signer = if status == 0 {
            // SAFETY: a successful verify leaves provider state describing
            // the primary signer, whose chain starts with its leaf.
            unsafe { leaf_signer(&data) }
        } else {
            Err(format!(
                "the signature is invalid (WinVerifyTrust 0x{status:08x})"
            ))
        };
        data.dwStateAction = WTD_STATEACTION_CLOSE;
        // SAFETY: closes the state opened by the verify call above.
        unsafe { WinVerifyTrust(INVALID_HANDLE_VALUE, &mut action, (&raw mut data).cast()) };
        signer
    }

    unsafe fn leaf_signer(data: &WINTRUST_DATA) -> Result<Signer, String> {
        let missing = || "the signature has no signer certificate".to_owned();
        let provider = unsafe { WTHelperProvDataFromStateData(data.hWVTStateData) };
        if provider.is_null() {
            return Err(missing());
        }
        let signer = unsafe { WTHelperGetProvSignerFromChain(provider, 0, 0, 0) };
        if signer.is_null() || unsafe { (*signer).csCertChain } == 0 {
            return Err(missing());
        }
        let leaf = unsafe { (*(*signer).pasCertChain).pCert };
        if leaf.is_null() {
            return Err(missing());
        }
        Ok(Signer {
            common_name: unsafe { subject_attribute(leaf, szOID_COMMON_NAME) },
            organization: unsafe { subject_attribute(leaf, szOID_ORGANIZATION_NAME) },
        })
    }

    unsafe fn subject_attribute(cert: *const CERT_CONTEXT, oid: *const u8) -> String {
        let mut name = [0u16; 512];
        let written = unsafe {
            CertGetNameStringW(
                cert,
                CERT_NAME_ATTR_TYPE,
                0,
                oid.cast(),
                name.as_mut_ptr(),
                name.len() as u32,
            )
        } as usize;
        String::from_utf16_lossy(&name[..written.saturating_sub(1).min(name.len())])
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
async fn verify_file(file: &Path) -> Result<(), HarnessError> {
    Err(unsigned(&format!(
        "{} cannot be verified on this platform",
        file_name(file)
    )))
}

#[cfg(target_os = "macos")]
async fn run(
    mut command: crate::process::Command,
    file: &Path,
) -> Result<std::process::Output, HarnessError> {
    command
        .stdin(crate::process::Stdio::null())
        .stdout(crate::process::Stdio::piped())
        .stderr(crate::process::Stdio::piped())
        .kill_on_drop(true);
    tokio::time::timeout(VERIFY_TIMEOUT, command.output())
        .await
        .map_err(|_| {
            unsigned(&format!(
                "checking {} took longer than {}s",
                file_name(file),
                VERIFY_TIMEOUT.as_secs()
            ))
        })?
        .map_err(|error| unsigned(&format!("could not run the verifier: {error}")))
}

fn file_name(file: &Path) -> String {
    file.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| file.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_google_llc_is_accepted_as_the_windows_signer() {
        let signer = |common_name: &str, organization: &str| Signer {
            common_name: common_name.into(),
            organization: organization.into(),
        };
        assert!(signer("Google LLC", "Google LLC").is_google());
        assert!(!signer("Google LLC", "Evil Corp").is_google());
        assert!(!signer("Google LLC Impostor", "Google LLC").is_google());
        assert!(!signer("Microsoft Corporation", "Microsoft Corporation").is_google());
        assert!(!signer("", "").is_google());
    }

    #[tokio::test]
    async fn unsigned_files_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("server"), "#!/bin/sh\nexit 0\n").unwrap();
        let error = verify_google_signed(dir.path()).await.unwrap_err();
        assert!(error.to_string().contains("code signature"), "{error}");
    }

    #[tokio::test]
    async fn empty_archives_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        assert!(verify_google_signed(dir.path()).await.is_err());
    }

    /// apple's own binaries are signed, but not by google's team.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn binaries_from_another_team_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::copy("/bin/ls", dir.path().join("ls")).unwrap();
        let error = verify_google_signed(dir.path()).await.unwrap_err();
        assert!(
            error.to_string().contains("not signed by Google"),
            "{error}"
        );
    }

    /// windows ships its own binaries signed by microsoft, not google.
    #[cfg(windows)]
    #[tokio::test]
    async fn binaries_from_another_publisher_are_rejected() {
        let system = std::env::var_os("SystemRoot").unwrap();
        let dir = tempfile::tempdir().unwrap();
        // embedded-signed, unlike catalog-signed tools such as notepad
        std::fs::copy(
            Path::new(&system).join("System32").join("ntoskrnl.exe"),
            dir.path().join("ntoskrnl.exe"),
        )
        .unwrap();
        let error = verify_google_signed(dir.path()).await.unwrap_err();
        assert!(error.to_string().contains("code signature"), "{error}");
    }

    /// github's windows runners ship chrome, which google signs.
    #[cfg(windows)]
    #[tokio::test]
    async fn google_signed_binary_is_accepted() {
        let chrome = Path::new(r"C:\Program Files\Google\Chrome\Application\chrome.exe");
        if !chrome.is_file() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        std::fs::copy(chrome, dir.path().join("chrome.exe")).unwrap();
        verify_google_signed(dir.path()).await.unwrap();
    }

    /// set `ZERON_TEST_GOOGLE_SIGNED_DIR` to an extracted antigravity archive.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn google_signed_archive_is_accepted() {
        let Some(dir) = std::env::var_os("ZERON_TEST_GOOGLE_SIGNED_DIR") else {
            return;
        };
        verify_google_signed(Path::new(&dir)).await.unwrap();
    }
}
