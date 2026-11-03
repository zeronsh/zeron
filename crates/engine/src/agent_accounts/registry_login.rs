//! Signing in to an agent added from the ACP registry, the same for every
//! one: its own sign-in ([`zeron_harness::AcpHarness::sign_in_registry`]),
//! run here on the device it's added to, its browser captured. The page it
//! asks for is reported by the login's polls and opened by the app on the
//! device the person is at; a `localhost` redirect from that browser reaches
//! the agent through the login's callback route (the same tunnel Codex's and
//! Claude's sign-ins take), registered once the page is known. A device code
//! it prints is said with the page. A sign-in that stops for typed input
//! ends saying so, with the command to run in a terminal on this device.
//!
//! Registry agents keep their logins themselves: there is no account of
//! Zeron's to save, list or switch.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use zeron_proto::{AgentLoginMode, AgentLoginStart, HarnessId, RegistryAgent};

use super::{AgentAccounts, EngineError, LoginFlow, TaskLoginState, lock, new_id};

/// What a terminal sign-in's failure says before its command.
pub(crate) const TERMINAL_PREFIX: &str = "This sign-in asks for something only a terminal can answer. Run it there: ";

impl AgentAccounts {
    pub(super) fn start_registry_login(
        &self,
        agent: RegistryAgent,
        method: Option<&str>,
        requester: Option<&str>,
    ) -> Result<AgentLoginStart, EngineError> {
        let harness = HarnessId::Registry(agent);
        self.reap_spawned_flows(harness);
        let record = zeron_harness::acp_registry::record(agent)
            .ok_or_else(|| EngineError::Other(format!("{} isn't added on this device", agent.registry_id())))?;
        let acp = match lock(&self.inner.cli_overrides).get(&harness) {
            Some(path) => zeron_harness::AcpHarness::registry(agent, &record.name).with_executable(path.clone()),
            None => zeron_harness::AcpHarness::registry(agent, &record.name),
        };
        let login_id = new_id();
        let home = self.login_home(&login_id)?;
        let url_file = home.join("browser-url");
        std::fs::write(&url_file, "").map_err(|e| EngineError::Other(e.to_string()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&url_file, std::fs::Permissions::from_mode(0o600));
        }
        #[cfg(unix)]
        let browser = super::ensure_recording_browser(&self.inner.config.root_dir());
        #[cfg(not(unix))]
        let browser = None;
        let options = zeron_harness::acp::SignInOptions {
            browser,
            method: method.map(str::to_owned),
            env: vec![("ZERON_LOGIN_URL_FILE".into(), url_file.clone().into())],
            url_filter: None,
        };
        let state = Arc::new(Mutex::new(TaskLoginState {
            requester: requester.map(str::to_string),
            ..Default::default()
        }));
        let task_state = state.clone();
        let task_home = home.clone();
        tracing::info!(agent = agent.registry_id(), login_id = %login_id, "registry agent sign-in started");
        let log_id = login_id.clone();
        let handle = tokio::spawn(async move {
            // The page the browser was asked for, once it's asked.
            let watch_state = task_state.clone();
            let watch_file = url_file.clone();
            let watcher = tokio::spawn(async move {
                loop {
                    if lock(&watch_state).url.is_some() {
                        break;
                    }
                    if let Some(url) = std::fs::read_to_string(&watch_file).ok().and_then(|text| {
                        text.lines()
                            .map(str::trim)
                            .find(|l| l.starts_with("https://") || l.starts_with("http://"))
                            .map(str::to_owned)
                    }) {
                        lock(&watch_state).url = Some(url);
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            });
            let progress_state = task_state.clone();
            let printed = Arc::new(Mutex::new(String::new()));
            let outcome = acp
                .sign_in_registry(options, move |progress| match progress {
                    zeron_harness::acp::SignInProgress::OpenBrowser(url) => {
                        let mut state = lock(&progress_state);
                        if state.url.is_none() {
                            state.url = Some(url);
                        }
                    }
                    zeron_harness::acp::SignInProgress::Output(line) => {
                        let mut text = lock(&printed);
                        if text.len() < 64 * 1024 {
                            text.push_str(&line);
                            text.push('\n');
                        }
                        if let Some(code) = super::scan_device_code(&text) {
                            let mut state = lock(&progress_state);
                            state.message =
                                Some(format!("Your code is {code}. Check it matches on the page that opened, then confirm."));
                            state.code = Some(code);
                        }
                    }
                })
                .await
                .map_err(|error| {
                    let text = error.to_string();
                    match text.find(zeron_harness::acp::NEEDS_TERMINAL) {
                        Some(at) => format!(
                            "{TERMINAL_PREFIX}{}",
                            &text[at + zeron_harness::acp::NEEDS_TERMINAL.len()..]
                        ),
                        None => text,
                    }
                });
            watcher.abort();
            {
                let state = lock(&task_state);
                match &outcome {
                    Ok(()) => tracing::info!(
                        agent = agent.registry_id(),
                        login_id = %log_id,
                        browser = state.url.is_some(),
                        code = state.code.is_some(),
                        "registry agent sign-in done"
                    ),
                    Err(error) => tracing::warn!(
                        agent = agent.registry_id(),
                        login_id = %log_id,
                        browser = state.url.is_some(),
                        error = %error.lines().next().unwrap_or_default().chars().take(200).collect::<String>(),
                        "registry agent sign-in failed"
                    ),
                }
            }
            crate::acp_registry::forget_sign_in(agent);
            let _ = std::fs::remove_dir_all(&task_home);
            lock(&task_state).outcome = Some(outcome);
        });
        lock(&self.inner.flows).insert(
            login_id.clone(),
            LoginFlow::Task {
                harness,
                started_at: Instant::now(),
                state,
                handle,
                home: Some(home),
                port: None,
            },
        );
        Ok(AgentLoginStart {
            login_id,
            url: String::new(),
            mode: AgentLoginMode::Browser,
            callback_port: None,
        })
    }
}
