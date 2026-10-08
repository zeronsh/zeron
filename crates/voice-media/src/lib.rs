//! Codex's packaged WebRTC helper protocol (Apache-2.0, OpenAI rust-v0.159.0).
//! Audio, AEC, interruption and encrypted media stay inside that native process.
//! Only bounded SDP signaling, controls and level meters cross this pipe.
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{ChildStdin, ChildStdout, Command},
};
use zeron_proto::voice::VoiceRejection;
const MAX_FRAME: usize = 128 * 1024;

pub struct NativeHost {
    stop: tokio_util::sync::CancellationToken,
    input: ChildStdin,
    output: ChildStdout,
    device_selection: bool,
}
impl Drop for NativeHost {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}
impl NativeHost {
    fn command(path: &Path) -> Command {
        let mut cmd = Command::new(path);
        // Fixed allowlist mirrors Codex. Never inherit API keys or dynamic loader/plugin overrides.
        cmd.env_clear();
        for (k, v) in std::env::vars_os() {
            if matches!(
                k.to_string_lossy().to_ascii_uppercase().as_str(),
                "SYSTEMROOT"
                    | "WINDIR"
                    | "HOME"
                    | "USERPROFILE"
                    | "LOCALAPPDATA"
                    | "APPDATA"
                    | "TEMP"
                    | "TMP"
                    | "TMPDIR"
                    | "XDG_RUNTIME_DIR"
                    | "PULSE_SERVER"
                    | "PULSE_COOKIE"
                    | "PIPEWIRE_REMOTE"
                    | "DBUS_SESSION_BUS_ADDRESS"
                    | "HTTP_PROXY"
                    | "HTTPS_PROXY"
                    | "ALL_PROXY"
                    | "NO_PROXY"
                    | "SSL_CERT_FILE"
                    | "SSL_CERT_DIR"
                    | "REQUESTS_CA_BUNDLE"
                    | "CURL_CA_BUNDLE"
            ) {
                cmd.env(k, v);
            }
        }
        for k in [
            "GST_PLUGIN_PATH",
            "GST_PLUGIN_PATH_1_0",
            "GST_PLUGIN_SYSTEM_PATH",
            "GST_PLUGIN_SYSTEM_PATH_1_0",
        ] {
            cmd.env(k, "");
        }
        cmd.env(
            "GST_REGISTRY",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env("GST_REGISTRY_UPDATE", "no")
        .env("GST_REGISTRY_FORK", "no");
        #[cfg(target_os = "linux")]
        for directory in [
            "/usr/lib/x86_64-linux-gnu/alsa-lib",
            "/usr/lib/aarch64-linux-gnu/alsa-lib",
            "/usr/lib64/alsa-lib",
            "/usr/lib/alsa-lib",
        ] {
            if Path::new(directory).is_dir() {
                cmd.env("ALSA_PLUGIN_DIR", directory);
                break;
            }
        }
        cmd.current_dir(path.parent().unwrap())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        cmd
    }
    pub async fn open(
        path: &Path,
        stop: tokio_util::sync::CancellationToken,
    ) -> Result<Self, VoiceRejection> {
        let device_selection = helper_device_selection(path)?;
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            Self::command(path).arg("--build-commit").output(),
        )
        .await
        .map_err(|_| VoiceRejection::NativeRuntimeUnavailable)?
        .map_err(|_| VoiceRejection::NativeRuntimeUnavailable)?;
        let commit = std::str::from_utf8(&result.stdout)
            .ok()
            .map(str::trim)
            .filter(|s| !s.is_empty() && s.len() <= 128)
            .filter(|_| result.status.success())
            .ok_or(VoiceRejection::NativeRuntimeUnavailable)?;
        let mut child = Self::command(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|_| VoiceRejection::NativeRuntimeUnavailable)?;
        let input = child.stdin.take().ok_or(VoiceRejection::Protocol)?;
        let output = child.stdout.take().ok_or(VoiceRejection::Protocol)?;
        let reaper_stop = stop.clone();
        tokio::spawn(async move {
            tokio::select! {biased;
                _=reaper_stop.cancelled()=>{let _=child.start_kill();let _=child.wait().await;},
                _=child.wait()=>{},
            }
        });
        let mut host = Self {
            stop: stop.clone(),
            input,
            output,
            device_selection,
        };
        // A helper that cannot start its runtime (e.g. a library that no longer
        // matches its runtime.json) exits here; that is the runtime, not the call.
        let unavailable = |reason| match reason {
            VoiceRejection::Protocol => VoiceRejection::NativeRuntimeUnavailable,
            reason => reason,
        };
        host.expect(
            json!({"type":"hello","protocol":1,"buildCommit":commit}),
            "ready",
            30,
        )
        .await
        .map_err(unavailable)?;
        host.expect(json!({"type":"initializeRuntime"}), "runtimeReady", 30)
            .await
            .map_err(unavailable)?;
        Ok(host)
    }
    pub async fn exchange(
        &mut self,
        message: Value,
        seconds: u64,
    ) -> Result<Value, VoiceRejection> {
        if self.stop.is_cancelled() {
            return Err(VoiceRejection::Protocol);
        }
        struct ExchangeGuard(Option<tokio_util::sync::CancellationToken>);
        impl Drop for ExchangeGuard {
            fn drop(&mut self) {
                if let Some(stop) = &self.0 {
                    stop.cancel();
                }
            }
        }
        // A cancelled partial read invalidates the pipe, including when the
        // caller drops this future without dropping the media endpoint yet.
        let mut guard = ExchangeGuard(Some(self.stop.clone()));
        let result = tokio::time::timeout(Duration::from_secs(seconds), async {
            let payload = serde_json::to_vec(&message).map_err(|_| VoiceRejection::Protocol)?;
            if payload.len() > MAX_FRAME {
                return Err(VoiceRejection::Overflow);
            }
            self.input
                .write_u32(payload.len() as u32)
                .await
                .map_err(|_| VoiceRejection::Protocol)?;
            self.input
                .write_all(&payload)
                .await
                .map_err(|_| VoiceRejection::Protocol)?;
            self.input
                .flush()
                .await
                .map_err(|_| VoiceRejection::Protocol)?;
            let length = self
                .output
                .read_u32()
                .await
                .map_err(|_| VoiceRejection::Protocol)? as usize;
            if length == 0 || length > MAX_FRAME {
                return Err(VoiceRejection::Overflow);
            }
            let mut payload = vec![0; length];
            self.output
                .read_exact(&mut payload)
                .await
                .map_err(|_| VoiceRejection::Protocol)?;
            serde_json::from_slice(&payload).map_err(|_| VoiceRejection::Protocol)
        })
        .await
        .map_err(|_| VoiceRejection::Protocol)?;
        if result.is_ok() {
            guard.0 = None;
        }
        result
    }
    pub async fn expect(
        &mut self,
        message: Value,
        expected: &str,
        seconds: u64,
    ) -> Result<(), VoiceRejection> {
        if self.exchange(message, seconds).await?["type"] != expected {
            self.stop.cancel();
            return Err(VoiceRejection::Protocol);
        }
        Ok(())
    }
    pub async fn controls(&mut self, muted: bool) -> Result<(), VoiceRejection> {
        self.expect(json!({"type":"setAudioControls","controls":{"microphoneMuted":muted,"speakerSuppressed":false}}),"audioControlsApplied",5).await
    }
    pub async fn open_devices(&mut self) -> Result<(), VoiceRejection> {
        let mut message = json!({"type":"openDevices"});
        // Codex 0.161 requires selection even for the default devices. Older
        // helpers reject that field, despite sharing the same protocol version.
        if self.device_selection {
            message["selection"] = json!({});
        }
        self.expect(message, "devicesOpened", 5).await
    }
    /// Synchronous local shutdown, independent of pending RPC or helper I/O.
    pub fn close(&self) {
        self.stop.cancel();
    }
    pub async fn levels(&mut self) -> Result<(u16, u16), VoiceRejection> {
        let v = self.exchange(json!({"type":"inspectAudio"}), 5).await?;
        if v["type"] != "audioState" {
            return Err(VoiceRejection::Protocol);
        }
        let mic = v["state"]["microphonePeak"]
            .as_u64()
            .and_then(|n| u16::try_from(n).ok())
            .ok_or(VoiceRejection::Protocol)?;
        let speaker = v["state"]["speakerPeak"]
            .as_u64()
            .and_then(|n| u16::try_from(n).ok())
            .ok_or(VoiceRejection::Protocol)?;
        Ok((mic, speaker))
    }
}

fn helper_device_selection(path: &Path) -> Result<bool, VoiceRejection> {
    let root = path
        .parent()
        .and_then(Path::parent)
        .ok_or(VoiceRejection::NativeRuntimeUnavailable)?;
    let bytes = match std::fs::read(root.join("manifest.json")) {
        Ok(bytes) => bytes,
        // The pinned development runtime and older fixtures may omit the
        // upstream manifest; they use the original openDevices format.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(_) => return Err(VoiceRejection::NativeRuntimeUnavailable),
    };
    let manifest: Value =
        serde_json::from_slice(&bytes).map_err(|_| VoiceRejection::NativeRuntimeUnavailable)?;
    let version = manifest["appVersion"]
        .as_str()
        .and_then(|v| semver::Version::parse(v).ok())
        .ok_or(VoiceRejection::Unsupported)?;
    Ok(version >= semver::Version::new(0, 161, 0))
}

fn runtime_target() -> Result<&'static str, VoiceRejection> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        ("macos", "x86_64") => Ok("x86_64-apple-darwin"),
        ("linux", "x86_64") => Ok("x86_64-unknown-linux-gnu"),
        ("linux", "aarch64") => Ok("aarch64-unknown-linux-gnu"),
        ("windows", "x86_64") => Ok("x86_64-pc-windows-msvc"),
        ("windows", "aarch64") => Ok("aarch64-pc-windows-msvc"),
        _ => Err(VoiceRejection::Unsupported),
    }
}

const HELPER: &str = if cfg!(windows) {
    "bin/codex-voice-host.exe"
} else {
    "bin/codex-voice-host"
};

/// The helper shipped inside the device's standalone Codex installation (layout
/// 1, version 0.159 or later), resolved from its `codex` executable. Zeron never
/// redistributes this runtime; npm installations do not contain it. The helper
/// only initializes from a `codex-resources/voice` directory.
pub fn installed_helper(codex_executable: &Path) -> Result<PathBuf, VoiceRejection> {
    let binary = codex_executable
        .canonicalize()
        .map_err(|_| VoiceRejection::NativeRuntimeUnavailable)?;
    let root = binary
        .parent()
        .and_then(Path::parent)
        .ok_or(VoiceRejection::NativeRuntimeUnavailable)?;
    let manifest: Value = std::fs::read(root.join("codex-package.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .ok_or(VoiceRejection::NativeRuntimeUnavailable)?;
    let version = manifest["version"]
        .as_str()
        .and_then(|v| semver::Version::parse(v).ok())
        .ok_or(VoiceRejection::Unsupported)?;
    if manifest["layoutVersion"] != 1 || version < semver::Version::new(0, 159, 0) {
        return Err(VoiceRejection::Unsupported);
    }
    let path = root.join("codex-resources/voice").join(HELPER);
    if path.canonicalize().ok().as_ref() != Some(&path) || !path.is_file() {
        return Err(VoiceRejection::NativeRuntimeUnavailable);
    }
    Ok(path)
}

/// Client media for a remote call: an explicit development runtime
/// (`ZERON_VOICE_MEDIA_DIR`, see `scripts/package-voice-runtime.py`), otherwise
/// this device's standalone Codex installation. Never reads Codex credentials.
pub fn media_helper(codex_executable: Option<&Path>) -> Result<PathBuf, VoiceRejection> {
    if let Some(path) = std::env::var_os("ZERON_VOICE_MEDIA_DIR") {
        return verify_runtime(Path::new(&path));
    }
    installed_helper(codex_executable.ok_or(VoiceRejection::NativeRuntimeUnavailable)?)
}

pub const BUILD_COMMIT: &str = "a956835d020762cb2b570053af06f643a11c0ecc";

pub fn verify_runtime(root: &Path) -> Result<std::path::PathBuf, VoiceRejection> {
    use sha2::{Digest, Sha256};
    let reject = || VoiceRejection::NativeRuntimeUnavailable;
    // Fail here, clearly, rather than when the helper refuses to start.
    if root.file_name() != Some("voice".as_ref())
        || root.parent().and_then(Path::file_name) != Some("codex-resources".as_ref())
    {
        return Err(reject());
    }
    let manifest: Value = serde_json::from_slice(
        &std::fs::read(root.join("zeron-runtime.json")).map_err(|_| reject())?,
    )
    .map_err(|_| reject())?;
    if manifest["buildCommit"] != BUILD_COMMIT
        || manifest["protocol"] != 1
        || manifest["target"] != runtime_target()?
    {
        return Err(VoiceRejection::Unsupported);
    }
    let files = manifest["sha256"].as_object().ok_or_else(reject)?;
    if !files.contains_key(HELPER)
        || !files.contains_key("runtime.json")
        || !files.contains_key("NOTICE.md")
    {
        return Err(reject());
    }
    for (name, digest) in files {
        let path = Path::new(name);
        if path
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return Err(reject());
        }
        let bytes = std::fs::read(root.join(path)).map_err(|_| reject())?;
        if digest.as_str() != Some(format!("{:x}", Sha256::digest(&bytes)).as_str()) {
            return Err(reject());
        }
    }
    Ok(root.join(HELPER))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn codex_package(root: &Path, version: &str, helper: bool) -> PathBuf {
        let codex = root.join(if cfg!(windows) {
            "bin/codex.exe"
        } else {
            "bin/codex"
        });
        std::fs::create_dir_all(codex.parent().unwrap()).unwrap();
        std::fs::write(&codex, "").unwrap();
        std::fs::write(
            root.join("codex-package.json"),
            json!({"layoutVersion":1,"version":version}).to_string(),
        )
        .unwrap();
        if helper {
            let path = root.join("codex-resources/voice").join(HELPER);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
        }
        codex
    }

    #[test]
    fn helper_is_resolved_from_the_installed_codex_package() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap().join("release");
        let codex = codex_package(&root, "0.160.0", true);
        assert_eq!(
            installed_helper(&codex).unwrap(),
            root.join("codex-resources/voice").join(HELPER)
        );
        // Older layouts and npm-style installs without resources are unavailable.
        codex_package(&root, "0.158.9", true);
        assert_eq!(
            installed_helper(&codex).unwrap_err(),
            VoiceRejection::Unsupported
        );
        let npm = temp.path().join("npm");
        let codex = codex_package(&npm, "0.160.0", false);
        assert_eq!(
            installed_helper(&codex).unwrap_err(),
            VoiceRejection::NativeRuntimeUnavailable
        );
        std::fs::remove_file(npm.join("codex-package.json")).unwrap();
        assert_eq!(
            installed_helper(&codex).unwrap_err(),
            VoiceRejection::NativeRuntimeUnavailable
        );
    }

    /// An opt-in runtime check: no provider connection or audio devices.
    #[tokio::test]
    #[ignore = "requires a projected runtime in ZERON_VOICE_MEDIA_DIR"]
    async fn packaged_native_runtime_initializes_without_audio_devices() {
        let path = media_helper(None).unwrap();
        let host = NativeHost::open(&path, Default::default()).await.unwrap();
        host.close();
    }

    #[test]
    fn runtime_rejects_missing_tampered_misplaced_and_incompatible_resources() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("Resources/codex-resources/voice");
        std::fs::create_dir_all(&dir).unwrap();
        assert!(verify_runtime(&dir).is_err());
        let mut hashes = serde_json::Map::new();
        for name in [HELPER, "runtime.json", "NOTICE.md"] {
            let path = dir.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, name).unwrap();
            hashes.insert(
                name.into(),
                json!(format!("{:x}", Sha256::digest(name.as_bytes()))),
            );
        }
        let mut manifest = json!({"protocol":1,"buildCommit":BUILD_COMMIT,"target":runtime_target().unwrap(),"sha256":hashes});
        let save = |m: &Value| {
            std::fs::write(
                dir.join("zeron-runtime.json"),
                serde_json::to_vec(m).unwrap(),
            )
            .unwrap()
        };
        save(&manifest);
        assert_eq!(verify_runtime(&dir).unwrap(), dir.join(HELPER));
        // The helper refuses to start outside `codex-resources/voice`.
        let elsewhere = temp.path().join("Resources/voice");
        std::fs::rename(temp.path().join("Resources/codex-resources"), &elsewhere).unwrap();
        assert!(verify_runtime(&elsewhere.join("voice")).is_err());
        std::fs::rename(&elsewhere, temp.path().join("Resources/codex-resources")).unwrap();
        std::fs::write(dir.join("runtime.json"), "tampered").unwrap();
        assert!(verify_runtime(&dir).is_err());
        std::fs::write(dir.join("runtime.json"), "runtime.json").unwrap();
        manifest["protocol"] = json!(2);
        save(&manifest);
        assert_eq!(
            verify_runtime(&dir).unwrap_err(),
            VoiceRejection::Unsupported
        );
        manifest["protocol"] = json!(1);
        manifest["target"] = json!("incompatible-platform");
        save(&manifest);
        assert_eq!(
            verify_runtime(&dir).unwrap_err(),
            VoiceRejection::Unsupported
        );
        manifest["target"] = json!(runtime_target().unwrap());
        manifest["sha256"].as_object_mut().unwrap().remove(HELPER);
        save(&manifest);
        assert!(verify_runtime(&dir).is_err());
        manifest["sha256"][HELPER] = json!(format!("{:x}", Sha256::digest(HELPER.as_bytes())));
        manifest["sha256"]["../escape"] = json!("digest");
        save(&manifest);
        assert!(verify_runtime(&dir).is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn desktop_media_opens_default_devices_with_old_and_new_helpers() {
        use std::os::unix::fs::PermissionsExt;
        use zeron_voice_session::VoiceMediaEndpoint;

        for version in [None, Some("0.159.0"), Some("0.160.0"), Some("0.161.0")] {
            let dir = tempfile::tempdir().unwrap();
            let codex = codex_package(dir.path(), version.unwrap_or("0.160.0"), true);
            let helper = installed_helper(&codex).unwrap();
            std::fs::write(
                &helper,
                include_bytes!("../../harness/tests/fixtures/fake-codex-voice-host.py"),
            )
            .unwrap();
            std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
            if let Some(version) = version {
                std::fs::write(
                    helper
                        .parent()
                        .unwrap()
                        .parent()
                        .unwrap()
                        .join("manifest.json"),
                    json!({"appVersion":version}).to_string(),
                )
                .unwrap();
            }
            let media = DesktopMedia {
                path: helper,
                host: Default::default(),
                stop: Default::default(),
            };
            media.prepare().await.unwrap();
            media.offer().await.unwrap();
            media
                .apply_answer(
                    zeron_proto::voice::remote::Sdp::new("fixture-answer".into()).unwrap(),
                )
                .await
                .unwrap();
            media.set_muted(true).await.unwrap();
            assert_eq!(media.levels().await.unwrap(), (1024, 0));
            media.stop.cancel();
        }
    }

    #[test]
    fn malformed_helper_metadata_is_not_treated_as_a_legacy_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let codex = codex_package(dir.path(), "0.161.0", true);
        let helper = installed_helper(&codex).unwrap();
        let manifest = helper
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("manifest.json");
        for contents in ["{", "{}", r#"{"appVersion":"invalid"}"#] {
            std::fs::write(&manifest, contents).unwrap();
            assert!(helper_device_selection(&helper).is_err());
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn helper_that_cannot_start_its_runtime_is_a_runtime_failure() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let helper = dir.path().join("codex-voice-host");
        // Answers hello, then exits on initializeRuntime like a helper whose
        // libraries no longer match its runtime.json (exit 23).
        std::fs::write(
            &helper,
            "#!/usr/bin/env python3\nimport json,struct,sys\n\
             if sys.argv[1:]==['--build-commit']: print('commit'); sys.exit(0)\n\
             n=struct.unpack('>I',sys.stdin.buffer.read(4))[0]; sys.stdin.buffer.read(n)\n\
             b=json.dumps({'type':'ready'}).encode()\n\
             sys.stdout.buffer.write(struct.pack('>I',len(b))+b); sys.stdout.flush()\n\
             sys.stdin.buffer.read(4); sys.exit(23)\n",
        )
        .unwrap();
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
        let result = NativeHost::open(&helper, Default::default()).await;
        assert_eq!(result.err(), Some(VoiceRejection::NativeRuntimeUnavailable));
    }
}

/// Desktop implementation shared by UIs; permission belongs to the viewport.
/// Instantiate one per call. Closing cannot wait behind a locked helper pipe.
pub struct DesktopMedia {
    path: std::path::PathBuf,
    host: tokio::sync::Mutex<Option<NativeHost>>,
    stop: tokio_util::sync::CancellationToken,
}
impl DesktopMedia {
    /// See [`media_helper`]: the local standalone Codex unless a development
    /// runtime is set explicitly.
    pub fn for_codex(codex_executable: Option<&Path>) -> Result<Self, VoiceRejection> {
        Ok(Self {
            path: media_helper(codex_executable)?,
            host: Default::default(),
            stop: Default::default(),
        })
    }
}
#[async_trait::async_trait]
impl zeron_voice_session::VoiceMediaEndpoint for DesktopMedia {
    async fn prepare(&self) -> Result<(), VoiceRejection> {
        let host = NativeHost::open(&self.path, self.stop.clone()).await?;
        if self.stop.is_cancelled() {
            return Err(VoiceRejection::InvalidLease);
        }
        *self.host.lock().await = Some(host);
        Ok(())
    }
    async fn offer(&self) -> Result<zeron_proto::voice::remote::Sdp, VoiceRejection> {
        let mut state = self.host.lock().await;
        let host = state.as_mut().ok_or(VoiceRejection::InvalidLease)?;
        let offer = host.exchange(json!({"type":"startTransport"}), 20).await?;
        if offer["type"] != "offer" {
            return Err(VoiceRejection::Protocol);
        }
        zeron_proto::voice::remote::Sdp::new(
            offer["sdp"]
                .as_str()
                .ok_or(VoiceRejection::Protocol)?
                .into(),
        )
    }
    async fn apply_answer(
        &self,
        answer: zeron_proto::voice::remote::Sdp,
    ) -> Result<(), VoiceRejection> {
        let mut state = self.host.lock().await;
        let host = state.as_mut().ok_or(VoiceRejection::InvalidLease)?;
        host.expect(
            json!({"type":"applyAnswer","sdp":answer.expose()}),
            "transportReady",
            20,
        )
        .await?;
        host.open_devices().await?;
        host.expect(json!({"type":"setAudioControls","controls":{"microphoneMuted":true,"speakerSuppressed":true}}),"audioControlsApplied",5).await
    }
    async fn set_muted(&self, muted: bool) -> Result<(), VoiceRejection> {
        self.host
            .lock()
            .await
            .as_mut()
            .ok_or(VoiceRejection::InvalidLease)?
            .controls(muted)
            .await
    }
    async fn levels(&self) -> Result<(u16, u16), VoiceRejection> {
        self.host
            .lock()
            .await
            .as_mut()
            .ok_or(VoiceRejection::InvalidLease)?
            .levels()
            .await
    }
    fn close(&self) {
        self.stop.cancel();
    }
}
impl Drop for DesktopMedia {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}
