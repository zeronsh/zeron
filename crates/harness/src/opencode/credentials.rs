//! OpenCode 2.x's sign-ins, as its background service keeps them: one or
//! more credentials per integration (a provider), one of them active. 2.x has
//! no `auth.json` for Zeron's account store to read; its own clients list,
//! switch and remove credentials through these routes, and so does Zeron.
//!
//! A credential's secret never leaves the service through here: only its
//! id, integration, label, kind and whether it is the active one.

use serde_json::Value;

use super::{OpencodeHarness, unwrap_data};
use crate::HarnessError;

/// One sign-in, without its secret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeCredential {
    pub id: String,
    /// The integration (provider) it signs into, by id (`openai`, `mistral`).
    pub integration: String,
    /// The integration's own name, when the service lists it.
    pub integration_name: Option<String>,
    pub label: Option<String>,
    /// How it signs in: `key`, `oauth`, `external` (another tool's login).
    pub kind: String,
    /// The one its integration uses now.
    pub active: bool,
}

impl OpencodeHarness {
    /// Every sign-in the service holds, by integration then label.
    pub async fn native_credentials(&self) -> Result<Vec<NativeCredential>, HarnessError> {
        let server = self.require_service().await?;
        let credentials = unwrap_data(server.get_json("/api/credential", None).await?);
        let names: std::collections::HashMap<String, String> = server
            .get_json("/api/integration", None)
            .await
            .map(unwrap_data)
            .ok()
            .and_then(|list| list.as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(|integration| {
                Some((
                    integration.get("id")?.as_str()?.to_owned(),
                    integration.get("name")?.as_str()?.to_owned(),
                ))
            })
            .collect();
        let mut list: Vec<NativeCredential> = credentials
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|credential| native_credential(credential, &names))
            .collect();
        list.sort_by(|a, b| (&a.integration, &a.label).cmp(&(&b.integration, &b.label)));
        Ok(list)
    }

    /// What sign-in `id` holds, as the service keeps it (`{type: "key", key}`,
    /// `{type: "oauth", access, …}`, `{type: "external", methodID}`): for a
    /// usage probe of the vendor behind it, never kept.
    pub async fn native_credential_value(&self, id: &str) -> Result<Option<Value>, HarnessError> {
        let server = self.require_service().await?;
        let credentials = unwrap_data(server.get_json("/api/credential", None).await?);
        Ok(credentials
            .as_array()
            .into_iter()
            .flatten()
            .find(|credential| credential.get("id").and_then(Value::as_str) == Some(id))
            .and_then(|credential| credential.get("value").cloned()))
    }

    /// Store a new sign-in for `integration` (its `value` as the service
    /// takes it: `{type: "oauth", methodID, access, refresh, expires}` or
    /// `{type: "key", key}`), made the integration's active one.
    pub async fn add_native_credential(
        &self,
        integration: &str,
        label: &str,
        value: Value,
    ) -> Result<(), HarnessError> {
        let server = self.require_service().await?;
        server
            .post_json(
                "/api/credential",
                None,
                &serde_json::json!({
                    "integrationID": integration,
                    "label": label,
                    "value": value,
                    "activate": true,
                }),
            )
            .await
            .map(|_| ())
    }

    /// Make `id` the credential its integration signs in with.
    pub async fn activate_native_credential(&self, id: &str) -> Result<(), HarnessError> {
        let server = self.require_service().await?;
        server
            .post_json(
                &format!("/api/credential/{}/activate", super::encode_directory(id)),
                None,
                &serde_json::json!({}),
            )
            .await
            .map(|_| ())
    }

    /// Sign out of `id`: the service forgets it.
    pub async fn remove_native_credential(&self, id: &str) -> Result<(), HarnessError> {
        let server = self.require_service().await?;
        let path = format!("/api/credential/{}", super::encode_directory(id));
        server.delete(&path, None).await
    }

    /// Whether this machine's OpenCode keeps its sign-ins on a background
    /// service (2.x) rather than in `auth.json`.
    pub async fn has_native_credentials(&self) -> bool {
        self.service_server().await.is_some()
    }
}

/// A listed credential, its secret left behind.
fn native_credential(
    credential: &Value,
    names: &std::collections::HashMap<String, String>,
) -> Option<NativeCredential> {
    let id = credential.get("id")?.as_str()?.to_owned();
    let integration = credential.get("integrationID")?.as_str()?.to_owned();
    Some(NativeCredential {
        integration_name: names.get(&integration).cloned(),
        label: credential
            .get("label")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|label| !label.is_empty())
            .map(str::to_owned),
        kind: credential
            .pointer("/value/type")
            .and_then(Value::as_str)
            .unwrap_or("key")
            .to_owned(),
        active: credential
            .get("active")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        id,
        integration,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_credential_reads_without_its_secret() {
        // (Shape captured live from 2.3.0, secret replaced.)
        let listed = json!({
            "id": "crd_1", "integrationID": "mistral", "label": "Work", "active": true,
            "value": {"type": "key", "key": "sk-not-real"}
        });
        let names = [("mistral".to_owned(), "Mistral".to_owned())].into();
        let credential = native_credential(&listed, &names).unwrap();
        assert_eq!(
            credential,
            NativeCredential {
                id: "crd_1".into(),
                integration: "mistral".into(),
                integration_name: Some("Mistral".into()),
                label: Some("Work".into()),
                kind: "key".into(),
                active: true,
            }
        );
        assert!(!format!("{credential:?}").contains("sk-not-real"));
    }
}
