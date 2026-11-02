//! Auto-resume from the phone: a computer's device setting (whether turns
//! its usage limits stop resume by themselves once the limit resets) and one
//! stop's toggle on its error card. Both go over the device relay to the
//! engine that owns them; the stop's state itself rides the chat's session
//! row ([`crate::workspace::SessionRow::auto_resume`]).

use std::sync::atomic::{AtomicBool, Ordering};

use zeron_proto::{AutoResume, AutoResumeSettings};

use crate::Client;
use crate::client::Backend;
use crate::error::{ClientError, Result};

/// Demo mode's device setting, so the switch responds offline.
static DEMO_ENABLED: AtomicBool = AtomicBool::new(true);

impl Client {
    /// `GetAutoResume` on `device_id`.
    pub async fn auto_resume_settings(&self, device_id: &str) -> Result<AutoResumeSettings> {
        match self.inner.backend() {
            Backend::Demo(_) => Ok(AutoResumeSettings {
                enabled: DEMO_ENABLED.load(Ordering::Relaxed),
            }),
            Backend::Live(live) => {
                let reply = live
                    .relay
                    .call(
                        device_id,
                        zeron_rpc::methods::GET_AUTO_RESUME,
                        serde_json::json!({}),
                    )
                    .await?;
                serde_json::from_value(reply).map_err(|e| ClientError::HostError(e.to_string()))
            }
        }
    }

    /// `SetAutoResume` on `device_id`: whether its limit stops resume by
    /// themselves, from now on and for the stops waiting now.
    pub async fn set_auto_resume(
        &self,
        device_id: &str,
        enabled: bool,
    ) -> Result<AutoResumeSettings> {
        match self.inner.backend() {
            Backend::Demo(_) => {
                DEMO_ENABLED.store(enabled, Ordering::Relaxed);
                Ok(AutoResumeSettings { enabled })
            }
            Backend::Live(live) => {
                let reply = live
                    .relay
                    .call(
                        device_id,
                        zeron_rpc::methods::SET_AUTO_RESUME,
                        serde_json::json!({ "enabled": enabled }),
                    )
                    .await?;
                serde_json::from_value(reply).map_err(|e| ClientError::HostError(e.to_string()))
            }
        }
    }

    /// `SetChatAutoResume` on the chat's host `device_id`: resume this chat's
    /// limit stop by itself or not. Replies with the stop's new state (`None`
    /// when the chat has none — it moved on meanwhile).
    pub async fn set_chat_auto_resume(
        &self,
        device_id: &str,
        chat_id: &str,
        enabled: bool,
    ) -> Result<Option<AutoResume>> {
        match self.inner.backend() {
            Backend::Demo(_) => Ok(None),
            Backend::Live(live) => {
                let reply = live
                    .relay
                    .call(
                        device_id,
                        zeron_rpc::methods::SET_CHAT_AUTO_RESUME,
                        serde_json::json!({ "chatId": chat_id, "enabled": enabled }),
                    )
                    .await?;
                serde_json::from_value(reply).map_err(|e| ClientError::HostError(e.to_string()))
            }
        }
    }
}
