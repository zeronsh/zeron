//! Devin's ACP session starts with a bundled catalog and refreshes it later.
//! `models list` waits for the account catalog before writing JSON, so use it
//! instead of racing `session/new` against `config_option_update` notifications.

use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use tokio::io::AsyncReadExt;
use tokio::sync::Mutex;
use tokio::time::Instant;

use zeron_proto::Model;

use crate::HarnessError;
use crate::jsonrpc::{Incoming, RpcClient};
use crate::process::{Command, Stdio};

/// A freshly discovered variant may also arrive after `session/new` in the
/// process that runs the prompt. Wait for that exact id; the generic ACP
/// family fallback could otherwise silently select a different GPT model.
pub(super) async fn wait_for_model(
    client: &RpcClient,
    incoming: &mut tokio::sync::mpsc::Receiver<Incoming>,
    session_id: &str,
    response: &mut serde_json::Value,
    model: &str,
) -> Result<(), HarnessError> {
    let wait = async {
        loop {
            if super::models_from_session(response, &[])
                .iter()
                .any(|m| m.id == model)
            {
                return Ok(());
            }
            match incoming.recv().await {
                Some(Incoming::Notification { method, params })
                    if method == "session/update"
                        && params.get("sessionId").and_then(serde_json::Value::as_str)
                            == Some(session_id)
                        && params["update"]["sessionUpdate"] == "config_option_update" =>
                {
                    if params["update"]["configOptions"].is_array() {
                        response["configOptions"] = params["update"]["configOptions"].clone();
                    }
                }
                Some(Incoming::Request { id, method, params }) => {
                    super::handle_server_request(client, id, &method, &params);
                }
                Some(_) => {}
                None => {
                    return Err(HarnessError::Protocol(
                        "Devin exited while refreshing models".into(),
                    ));
                }
            }
        }
    };
    tokio::time::timeout(super::DEFAULT_MODEL_DISCOVERY_TIMEOUT, wait)
        .await
        .map_err(|_| {
            HarnessError::Protocol(format!(
                "Devin did not advertise requested model {model} after refreshing"
            ))
        })?
}

#[derive(Default)]
pub(super) struct Catalog {
    // Only overlapping callers share a result. A later picker open always
    // probes again, including after errors, login changes, or model rollouts.
    latest: Mutex<Option<(Instant, Vec<Model>)>>,
}

impl Catalog {
    pub(super) async fn refresh(
        &self,
        exe: &Path,
        timeout: Duration,
    ) -> Result<Vec<Model>, HarnessError> {
        let requested_at = Instant::now();
        let mut latest = self.latest.lock().await;
        if let Some((completed_at, models)) = &*latest
            && *completed_at >= requested_at
        {
            return Ok(models.clone());
        }
        let mut cmd = Command::new(exe);
        cmd.args(["models", "list", "--format", "json"]);
        crate::compose_child_path(&mut cmd, exe);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = cmd.spawn()?;
        let mut stdout = child.stdout.take().expect("piped stdout");
        let mut stderr = child.stderr.take().expect("piped stderr");
        let mut out = Vec::new();
        let mut err = Vec::new();
        let result = tokio::time::timeout(timeout, async {
            let (status, _, _) = tokio::try_join!(
                child.wait(),
                stdout.read_to_end(&mut out),
                stderr.read_to_end(&mut err)
            )?;
            Ok::<_, std::io::Error>(status)
        })
        .await;
        // In particular, timeout must reap the probe before its registry
        // execution lease can be released to an installer.
        crate::shutdown_child(&mut child, Duration::from_millis(250)).await;
        let status = result
            .map_err(|_| HarnessError::Protocol("Devin model discovery timed out".into()))??;
        if !status.success() {
            return Err(HarnessError::Protocol(format!(
                "Devin models list failed ({}): {}",
                status,
                String::from_utf8_lossy(&err).trim()
            )));
        }
        let models = parse_catalog(&out)?;
        *latest = Some((Instant::now(), models.clone()));
        Ok(models)
    }
}

#[derive(Deserialize)]
struct ModelList {
    families: Vec<Family>,
}

#[derive(Deserialize)]
struct Family {
    variants: Vec<Variant>,
}

#[derive(Deserialize)]
struct Variant {
    model_uid: String,
    label: String,
    cost_summary: Option<String>,
}

fn parse_catalog(bytes: &[u8]) -> Result<Vec<Model>, HarnessError> {
    let catalog: ModelList = serde_json::from_slice(bytes)
        .map_err(|error| HarnessError::Protocol(format!("invalid Devin model catalog: {error}")))?;
    let mut models = Vec::new();
    for variant in catalog.families.into_iter().flat_map(|f| f.variants) {
        if variant.model_uid.trim().is_empty() || variant.label.trim().is_empty() {
            return Err(HarnessError::Protocol(
                "invalid empty Devin model id or label".into(),
            ));
        }
        if models.iter().any(|m: &Model| m.id == variant.model_uid) {
            continue;
        }
        models.push(Model {
            id: variant.model_uid,
            label: variant.label,
            description: variant.cost_summary,
            // Devin encodes effort and fast mode in the exact variant id.
            reasoning_levels: Vec::new(),
            options: Vec::new(),
        });
    }
    if models.is_empty() {
        return Err(HarnessError::Protocol(
            "Devin returned an empty model catalog".into(),
        ));
    }
    Ok(models)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_exact_variants_without_turning_family_aliases_into_models() {
        let models = parse_catalog(br#"{"families":[{
            "family_uid":"gpt-6-astra", "aliases":["astra"], "variants":[
                {"model_uid":"gpt-6-astra-medium","label":"GPT-6 Astra Medium Thinking","cost_summary":"Account pricing"},
                {"model_uid":"gpt-6-astra-high","label":"GPT-6 Astra High Thinking"},
                {"model_uid":"gpt-6-astra-high","label":"Duplicate"}
            ]
        }]}"#).unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "gpt-6-astra-medium");
        assert_eq!(models[0].label, "GPT-6 Astra Medium Thinking");
        assert_eq!(models[0].description.as_deref(), Some("Account pricing"));
        assert_eq!(models[1].id, "gpt-6-astra-high");
        assert!(
            models
                .iter()
                .all(|m| m.reasoning_levels.is_empty() && m.options.is_empty())
        );
    }

    #[test]
    fn invalid_or_empty_catalogs_are_retryable_errors() {
        for bytes in [
            "not json",
            "{}",
            r#"{"families":[]}"#,
            r#"{"families":[{"variants":[{"label":"Missing id"}]}]}"#,
            r#"{"families":[{"variants":[{"model_uid":"","label":"Empty id"}]}]}"#,
        ] {
            assert!(parse_catalog(bytes.as_bytes()).is_err(), "{bytes}");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timed_out_catalog_probe_is_reaped_before_returning() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("devin");
        std::fs::write(
            &executable,
            "#!/bin/sh\ntrap '' INT TERM\necho $$ > \"$(dirname \"$0\")/pid\"\nexec sleep 30\n",
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let result = super::Catalog::default()
            .refresh(&executable, std::time::Duration::from_millis(500))
            .await;
        assert!(result.unwrap_err().to_string().contains("timed out"));
        let pid: i32 = std::fs::read_to_string(temp.path().join("pid"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            -1,
            "probe still alive after timeout returned"
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }
}
