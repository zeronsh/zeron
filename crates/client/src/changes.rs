//! Read the selected session's checkout on its execution host. Patches stay
//! in memory; they are never added to the registry or viewer's disk cache.

use crate::{Client, ClientError, error::Result};
use zeron_proto::CheckoutDiff;

impl Client {
    pub async fn checkout_changes(&self, chat_id: &str) -> Result<CheckoutDiff> {
        let chat = self
            .inner
            .workspace
            .chat(chat_id)
            .ok_or_else(|| ClientError::NotFound(chat_id.to_owned()))?;
        let cwd = chat
            .cwd
            .clone()
            .filter(|p| !p.is_empty())
            .or_else(|| {
                let workspace = self.workspace();
                chat.space_id
                    .as_deref()
                    .and_then(|id| workspace.project(id))
                    .map(|p| p.path.clone())
            })
            .ok_or_else(|| ClientError::NotFound("Session folder unavailable".into()))?;
        let device_id = self.chat_host(chat_id)?;
        let reply = self
            .inner
            .host_rpc(
                &device_id,
                crate::rpc::methods::GET_CHECKOUT_DIFF,
                serde_json::json!({ "chatId": chat_id, "cwd": cwd, "mode": "working" }),
            )
            .await?;
        serde_json::from_value(reply).map_err(|e| ClientError::HostError(e.to_string()))
    }
}
