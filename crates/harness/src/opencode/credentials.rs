//! Logins a Cloud machine hands OpenCode ([`CREDENTIALS_ENV`]).
//!
//! OpenCode 2.x keeps its logins in its own database, not a file, so a Cloud
//! machine's credential broker can't write them where the CLI reads them.
//! Instead it puts them in the run's environment under [`CREDENTIALS_ENV`]
//! — a JSON list of `{integrationID, value}`, `value` OpenCode's own
//! `Credential.Value` without its `methodID` — and the harness, once the
//! server it started is up, installs them through the server's credential
//! API before anything else asks it a question:
//!
//! - `GET /api/credential` lists what the database holds. Ours carry the
//!   label [`LABEL`]; nothing else is ever changed or removed.
//! - A login of the machine's own for the same integration (any other label)
//!   stays and is used: ours is not added beside it.
//! - The API changes a credential's label only, never its value, so a new
//!   token replaces ours: `DELETE /api/credential/{id}`, then `POST
//!   /api/credential {integrationID, label, value, activate: true}`. One
//!   whose access token is already ours is left as it is.
//! - An OAuth value needs the integration's sign-in method:
//!   `GET /api/integration/{id}` lists them, the first `oauth` one is taken.
//! - Ours for an integration the list no longer names are removed (the vault
//!   stopped granting it).
//!
//! The variable never reaches the server's process. A 1.x server, which has
//! no credential API and reads `auth.json` on every request instead, gets
//! the same logins as `auth.json` entries
//! (`{type: "oauth", refresh, access, expires, accountId?}`).

use serde::Deserialize;
use serde_json::{Value, json};

use super::{Protocol, Server};
use crate::HarnessError;

/// The run-environment variable a Cloud machine's broker hands logins in.
pub const CREDENTIALS_ENV: &str = "ZERON_OPENCODE_CREDENTIALS";

/// The label of every credential the harness installs.
pub const LABEL: &str = "Zeron Cloud";

/// One login to install.
#[derive(Debug, Clone, PartialEq, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Handoff {
    #[serde(rename = "integrationID")]
    pub integration_id: String,
    /// OpenCode's `Credential.Value`, without `methodID` (filled here).
    pub value: Value,
}

/// Parse [`CREDENTIALS_ENV`]'s value. Malformed = nothing to install, said
/// as an error (the broker wrote it, so it's a bug worth surfacing).
pub fn parse(raw: &str) -> Result<Vec<Handoff>, HarnessError> {
    serde_json::from_str(raw).map_err(|e| {
        HarnessError::Protocol(format!("Cloud's OpenCode sign-ins are malformed: {e}"))
    })
}

/// Install `logins` on `server` (see the module docs).
pub(super) async fn install(server: &Server, logins: &[Handoff]) -> Result<(), HarnessError> {
    match server.protocol().await {
        Protocol::V2 => install_v2(server, logins).await,
        Protocol::V1 => write_v1_auth(&v1_auth_file(), logins),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    id: String,
    #[serde(rename = "integrationID")]
    integration_id: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    value: Value,
}

async fn install_v2(server: &Server, logins: &[Handoff]) -> Result<(), HarnessError> {
    let listed = super::unwrap_data(server.get_json("/api/credential", None).await?);
    let entries: Vec<Entry> = serde_json::from_value(listed).map_err(|e| {
        HarnessError::Protocol(format!("opencode GET /api/credential: unexpected reply: {e}"))
    })?;
    // Ours for an integration no longer handed over: the vault stopped
    // granting it.
    for entry in entries.iter().filter(|e| e.label == LABEL) {
        if !logins.iter().any(|l| l.integration_id == entry.integration_id) {
            delete(server, &entry.id).await?;
        }
    }
    for login in logins {
        let mine: Vec<&Entry> = entries
            .iter()
            .filter(|e| e.integration_id == login.integration_id)
            .collect();
        // The machine's own login for it stays, and is used.
        if mine.iter().any(|e| e.label != LABEL) {
            continue;
        }
        let ours: Vec<&&Entry> = mine.iter().filter(|e| e.label == LABEL).collect();
        if ours.len() == 1 && same_token(&ours[0].value, &login.value) {
            continue;
        }
        let value = with_method(server, login).await?;
        for entry in ours {
            delete(server, &entry.id).await?;
        }
        let body = json!({
            "integrationID": login.integration_id,
            "label": LABEL,
            "value": value,
            "activate": true,
        });
        server
            .post_json("/api/credential", None, &body)
            .await
            .map_err(|e| {
                HarnessError::Protocol(format!(
                    "OpenCode refused Cloud's {} sign-in: {e}",
                    login.integration_id
                ))
            })?;
    }
    Ok(())
}

/// The same login already installed: same kind, same access token.
fn same_token(installed: &Value, wanted: &Value) -> bool {
    installed["type"] == wanted["type"]
        && installed["access"] == wanted["access"]
        && installed["key"] == wanted["key"]
        && installed["expires"] == wanted["expires"]
}

/// `login.value`, with the integration's OAuth sign-in method when it is
/// an OAuth login (OpenCode files every OAuth credential under one).
async fn with_method(server: &Server, login: &Handoff) -> Result<Value, HarnessError> {
    let mut value = login.value.clone();
    if value["type"] != "oauth" || value.get("methodID").is_some_and(Value::is_string) {
        return Ok(value);
    }
    let path = format!(
        "/api/integration/{}",
        super::percent_encode_path(&login.integration_id)
    );
    let info = super::unwrap_data(server.get_json(&path, None).await?);
    let method = oauth_method(&info).ok_or_else(|| {
        HarnessError::Protocol(format!(
            "OpenCode offers no sign-in for {}, so Cloud's can't be used",
            login.integration_id
        ))
    })?;
    value["methodID"] = json!(method);
    Ok(value)
}

/// The first OAuth sign-in method an `Integration.Info` lists.
pub(super) fn oauth_method(info: &Value) -> Option<String> {
    info["methods"]
        .as_array()?
        .iter()
        .find(|m| m["type"] == "oauth")
        .and_then(|m| m["id"].as_str())
        .map(str::to_owned)
}

async fn delete(server: &Server, id: &str) -> Result<(), HarnessError> {
    let path = format!("/api/credential/{}", super::percent_encode_path(id));
    let response = server
        .request(reqwest::Method::DELETE, &path)
        .timeout(super::CALL_TIMEOUT)
        .send()
        .await
        .map_err(|e| HarnessError::Protocol(format!("opencode DELETE {path}: {e}")))?;
    let status = response.status();
    if status.is_success() || status == reqwest::StatusCode::NOT_FOUND {
        return Ok(());
    }
    let body = response.text().await.unwrap_or_default();
    Err(HarnessError::Protocol(format!(
        "opencode DELETE {path}: {status} {}",
        super::truncate_body(&body)
    )))
}

/// 1.x's credential file: `$XDG_DATA_HOME/opencode/auth.json`.
fn v1_auth_file() -> std::path::PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| crate::executable::home_dir().map(|h| h.join(".local").join("share")))
        .unwrap_or_default()
        .join("opencode")
        .join("auth.json")
}

/// The handed-over logins as 1.x `auth.json` entries, every other entry
/// left as it is (one of the machine's own for the same provider stays).
pub(super) fn write_v1_auth(file: &std::path::Path, logins: &[Handoff]) -> Result<(), HarnessError> {
    let mut map = match std::fs::read(file) {
        Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
            Ok(Value::Object(map)) => map,
            _ => return Ok(()),
        },
        Err(_) => serde_json::Map::new(),
    };
    let mut changed = false;
    for login in logins {
        let value = &login.value;
        let mut entry = json!({
            "type": value["type"],
            "refresh": value["refresh"],
            "access": value["access"],
            "expires": value["expires"],
        });
        if value["type"] == "key" {
            entry = json!({ "type": "api", "key": value["key"] });
        } else if let Some(account) = value["metadata"]["accountID"].as_str() {
            entry["accountId"] = json!(account);
        }
        // An entry with a real refresh token of its own (not ours: ours is
        // empty, or Copilot's GitHub token, the same every time).
        let own = map.get(&login.integration_id).is_some_and(|e| {
            e["refresh"].as_str().is_some_and(|r| !r.is_empty()) && e["refresh"] != value["refresh"]
        });
        if own || map.get(&login.integration_id) == Some(&entry) {
            continue;
        }
        map.insert(login.integration_id.clone(), entry);
        changed = true;
    }
    if !changed {
        return Ok(());
    }
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let bytes = serde_json::to_vec_pretty(&Value::Object(map))
        .map_err(|e| HarnessError::Protocol(e.to_string()))?;
    let tmp = file.with_extension("json.zeron-tmp");
    {
        let mut options = std::fs::OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        use std::io::Write as _;
        let mut out = options.open(&tmp)?;
        out.write_all(&bytes)?;
        out.sync_all()?;
    }
    std::fs::rename(&tmp, file)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handoffs_parse_and_the_oauth_method_is_the_first_listed() {
        let parsed = parse(
            r#"[{"integrationID":"openai","value":{"type":"oauth","refresh":"","access":"a","expires":5}}]"#,
        )
        .unwrap();
        assert_eq!(parsed[0].integration_id, "openai");
        assert!(parse("{").is_err());
        let info = json!({"methods": [
            {"type": "key", "label": "API key"},
            {"id": "chatgpt", "type": "oauth", "label": "ChatGPT"},
            {"id": "other", "type": "oauth", "label": "Other"},
        ]});
        assert_eq!(oauth_method(&info).as_deref(), Some("chatgpt"));
        assert_eq!(oauth_method(&json!({"methods": []})), None);
    }

    /// A 2.x server's credential API, recording every call.
    async fn mock_v2(calls: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else { return };
                let calls = calls.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 4096];
                    let (head, mut body) = loop {
                        let n = socket.read(&mut chunk).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                        if let Some(at) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            break (
                                String::from_utf8_lossy(&buf[..at]).to_string(),
                                buf[at + 4..].to_vec(),
                            );
                        }
                    };
                    let length = head
                        .lines()
                        .find_map(|l| {
                            l.split_once(':')
                                .filter(|(n, _)| n.eq_ignore_ascii_case("content-length"))
                                .and_then(|(_, v)| v.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    while body.len() < length {
                        let n = socket.read(&mut chunk).await.unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        body.extend_from_slice(&chunk[..n]);
                    }
                    let mut line = head.lines().next().unwrap_or("").split_whitespace();
                    let call = format!("{} {}", line.next().unwrap_or(""), line.next().unwrap_or(""));
                    calls
                        .lock()
                        .unwrap()
                        .push((call.clone(), String::from_utf8_lossy(&body).to_string()));
                    let (status, reply) = match call.as_str() {
                        "GET /api/info" => ("200 OK", json!({"version": "2.3.2"})),
                        "GET /api/credential" => (
                            "200 OK",
                            json!({"data": [
                                {"id": "c-old", "integrationID": "openai", "label": LABEL, "active": true,
                                 "value": {"type": "oauth", "methodID": "chatgpt", "refresh": "", "access": "old", "expires": 1}},
                                {"id": "c-gone", "integrationID": "xai", "label": LABEL, "active": true,
                                 "value": {"type": "key", "key": "k"}},
                                {"id": "c-own", "integrationID": "github-copilot", "label": "My Copilot", "active": true,
                                 "value": {"type": "oauth", "methodID": "device", "refresh": "gho_own", "access": "x", "expires": 0}},
                            ]}),
                        ),
                        "GET /api/integration/openai" => (
                            "200 OK",
                            json!({"data": {"id": "openai", "name": "OpenAI", "methods": [
                                {"type": "key"}, {"id": "chatgpt", "type": "oauth", "label": "ChatGPT"}
                            ], "connections": []}}),
                        ),
                        c if c.starts_with("DELETE ") => ("204 No Content", Value::Null),
                        "POST /api/credential" => ("200 OK", json!({"data": {"id": "c-new"}})),
                        _ => ("404 Not Found", json!({"error": "no route"})),
                    };
                    let reply = if reply.is_null() { String::new() } else { reply.to_string() };
                    let _ = socket
                        .write_all(
                            format!(
                                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
                                reply.len()
                            )
                            .as_bytes(),
                        )
                        .await;
                });
            }
        });
        base
    }

    #[tokio::test]
    async fn a_2x_server_gets_cloud_logins_through_its_credential_api() {
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let base = mock_v2(calls.clone()).await;
        let harness = super::super::OpencodeHarness::new().with_base_url(base);
        let logins = json!([
            {"integrationID": "openai", "value": {"type": "oauth", "refresh": "", "access": "fresh",
              "expires": 99, "metadata": {"accountID": "acct-1"}}},
            {"integrationID": "github-copilot", "value": {"type": "oauth", "refresh": "gho_cloud",
              "access": "gho_cloud", "expires": 0}},
        ]);
        let env = std::collections::BTreeMap::from([
            (CREDENTIALS_ENV.to_string(), logins.to_string()),
            ("OPENAI_API_KEY".to_string(), "unrelated".to_string()),
        ]);
        let mut server = harness
            .server_with_env(None, None, &[], &env, zeron_proto::AccessMode::FullAccess)
            .await
            .unwrap();
        server.shutdown(std::time::Duration::ZERO).await;
        let calls = calls.lock().unwrap().clone();
        let names: Vec<&str> = calls.iter().map(|(c, _)| c.as_str()).collect();
        // Ours for an integration no longer handed over goes; our stale
        // ChatGPT one is replaced; the machine's own Copilot login stays.
        assert!(names.contains(&"DELETE /api/credential/c-gone"), "{names:?}");
        assert!(names.contains(&"DELETE /api/credential/c-old"), "{names:?}");
        assert!(!names.iter().any(|c| c.contains("c-own")), "{names:?}");
        let posts: Vec<Value> = calls
            .iter()
            .filter(|(c, _)| c == "POST /api/credential")
            .map(|(_, body)| serde_json::from_str(body).unwrap())
            .collect();
        assert_eq!(
            posts,
            vec![json!({
                "integrationID": "openai",
                "label": LABEL,
                "activate": true,
                "value": {"type": "oauth", "methodID": "chatgpt", "refresh": "", "access": "fresh",
                          "expires": 99, "metadata": {"accountID": "acct-1"}},
            })]
        );
        // The handoff never reaches the server as an environment variable.
        assert!(calls.iter().all(|(_, body)| !body.contains(CREDENTIALS_ENV)));
    }

    #[tokio::test]
    async fn model_discovery_boots_with_the_same_logins() {
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let base = mock_v2(calls.clone()).await;
        let harness = super::super::OpencodeHarness::new().with_base_url(base);
        let logins = json!([{"integrationID": "openai", "value": {"type": "oauth", "refresh": "",
            "access": "discovery", "expires": 5}}]);
        crate::catalog_env::set(
            zeron_proto::HarnessId::Opencode,
            std::collections::BTreeMap::from([(CREDENTIALS_ENV.to_string(), logins.to_string())]),
        );
        let mut server = harness.server(None, None).await.unwrap();
        server.shutdown(std::time::Duration::ZERO).await;
        crate::catalog_env::set(zeron_proto::HarnessId::Opencode, Default::default());
        let calls = calls.lock().unwrap();
        let post = calls
            .iter()
            .find(|(c, _)| c == "POST /api/credential")
            .expect("discovery's server gets the logins");
        assert!(post.1.contains("\"access\":\"discovery\""), "{post:?}");
    }

    #[tokio::test]
    async fn the_same_token_already_installed_is_left_alone() {
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let base = mock_v2(calls.clone()).await;
        let harness = super::super::OpencodeHarness::new().with_base_url(base);
        let logins = json!([
            {"integrationID": "openai", "value": {"type": "oauth", "refresh": "", "access": "old", "expires": 1}},
            {"integrationID": "xai", "value": {"type": "key", "key": "k"}},
        ]);
        let env = std::collections::BTreeMap::from([(CREDENTIALS_ENV.to_string(), logins.to_string())]);
        let mut server = harness
            .server_with_env(None, None, &[], &env, zeron_proto::AccessMode::FullAccess)
            .await
            .unwrap();
        server.shutdown(std::time::Duration::ZERO).await;
        let calls = calls.lock().unwrap();
        assert!(
            calls.iter().all(|(c, _)| c.starts_with("GET ")),
            "{calls:?}"
        );
    }

    #[test]
    fn a_1x_server_gets_auth_json_entries_beside_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("auth.json");
        std::fs::write(
            &file,
            r#"{"anthropic":{"type":"api","key":"sk-own"},"github-copilot":{"type":"oauth","refresh":"gho_own","access":"x","expires":0}}"#,
        )
        .unwrap();
        let logins = vec![
            Handoff {
                integration_id: "openai".into(),
                value: json!({"type":"oauth","refresh":"","access":"a1","expires":9,"metadata":{"accountID":"acct"}}),
            },
            Handoff {
                integration_id: "github-copilot".into(),
                value: json!({"type":"oauth","refresh":"gho_cloud","access":"gho_cloud","expires":0}),
            },
        ];
        write_v1_auth(&file, &logins).unwrap();
        let written: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(
            written["openai"],
            json!({"type":"oauth","refresh":"","access":"a1","expires":9,"accountId":"acct"})
        );
        assert_eq!(written["anthropic"]["key"], "sk-own");
        // The machine's own Copilot login (a real token) stays.
        assert_eq!(written["github-copilot"]["refresh"], "gho_own");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&file).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
