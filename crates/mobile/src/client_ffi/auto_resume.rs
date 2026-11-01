//! Auto-resume over FFI (`zeron_client::auto_resume`): a computer's setting
//! for whether turns its usage limits stop pick themselves back up once the
//! limit resets, and one stop's switch on its error card.

use super::{CoreClient, CoreResult, on_runtime};

#[uniffi::export]
impl CoreClient {
    /// Whether turns the usage limit stops on `device_id` resume by
    /// themselves (on unless the user turned it off).
    pub async fn auto_resume_enabled(&self, device_id: String) -> CoreResult<bool> {
        let client = self.client.clone();
        on_runtime(async move { client.auto_resume_settings(&device_id).await })
            .await
            .map(|settings| settings.enabled)
    }

    /// Turn auto-resume on `device_id` on or off, for its later stops and
    /// the ones waiting now; the device's setting after the change.
    pub async fn set_auto_resume_enabled(&self, device_id: String, enabled: bool) -> CoreResult<bool> {
        let client = self.client.clone();
        on_runtime(async move { client.set_auto_resume(&device_id, enabled).await })
            .await
            .map(|settings| settings.enabled)
    }

    /// The error card's switch: resume `chat_id`'s limit stop by itself, or
    /// not. Asks the chat's host; its session row brings the new state.
    pub async fn set_chat_auto_resume(&self, chat_id: String, enabled: bool) -> CoreResult<()> {
        let client = self.client.clone();
        let host = client.workspace().session(&chat_id).map(|row| row.device_id.clone());
        on_runtime(async move {
            let Some(host) = host else {
                return Err(zeron_client::ClientError::NotFound(format!("chat {chat_id}")));
            };
            client.set_chat_auto_resume(&host, &chat_id, enabled).await.map(drop)
        })
        .await
    }
}
