//! Stable conversation links shared by sidebar copy actions and inbound URL routing.

use zeron_notify::links::encode_component;
// The link format itself is shared with the zgx app (crates/notify).
pub use zeron_notify::links::{
    ConversationDeepLink, conversation_link as zeron_conversation_link,
    parse_conversation_link as parse_zeron_conversation_link,
};
use zeron_proto::{AuthState, Chat, HarnessId, WorkspaceScope};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessConversationLink {
    pub label: &'static str,
    pub url: String,
}

/// Opaque locator: enough to reject links for a different local/synced
/// workspace without putting device, user, or organization ids in the URL.
pub fn workspace_locator(
    scope: Option<WorkspaceScope>,
    auth: Option<&AuthState>,
    local_device_id: Option<&str>,
) -> Option<String> {
    let scope = scope?;
    let identity = match scope {
        WorkspaceScope::Synced | WorkspaceScope::Development => {
            let Some(AuthState::SignedIn { user, org_id }) = auth else {
                return None;
            };
            format!(
                "user:{}:org:{}",
                user.id,
                org_id.as_deref().unwrap_or("personal")
            )
        }
        WorkspaceScope::Local => format!("device:{}", local_device_id?),
    };
    Some(zeron_notify::links::workspace_locator_of(
        &format!("{scope:?}"),
        &identity,
    ))
}

/// Only return schemes verified against the harness app. Hermes exposes a
/// candidate scheme, but its contract is not stable enough to put on users'
/// clipboards yet.
pub fn harness_conversation_link(chat: &Chat) -> Option<HarnessConversationLink> {
    let id = chat.harness_session_id.as_deref()?.trim();
    if id.is_empty() || chat.config.as_ref()?.harness != HarnessId::Codex {
        return None;
    }
    Some(HarnessConversationLink {
        label: "Codex conversation link",
        url: format!("codex://threads/{}", encode_component(id)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn harness_chat(harness: HarnessId) -> Chat {
        Chat {
            id: "chat".into(),
            device_id: "device".into(),
            title: None,
            archived: false,
            cwd: None,
            branch: None,
            checkout_id: None,
            source_context: None,
            config: Some(zeron_proto::ChatConfig {
                harness,
                model: None,
                reasoning: None,
                model_options: Default::default(),
                sandbox: zeron_proto::SandboxLevel::WorkspaceWrite,
                access: Default::default(),
                plan: false,
            }),
            last_message_preview: None,
            last_message_at: None,
            created_at: chrono::DateTime::UNIX_EPOCH,
            harness_session_id: Some("thread/one".into()),
            harness_session_cwd: None,
            harness_session_harness: None,
            parent_chat_id: None,
            child_kind: None,
            space_id: None,
            last_seen_at: None,
            room_gen: None,
        }
    }

    #[test]
    fn local_workspace_locator_waits_for_device_identity() {
        assert_eq!(
            workspace_locator(Some(WorkspaceScope::Local), None, None),
            None
        );
        let first = workspace_locator(Some(WorkspaceScope::Local), None, Some("device-a"));
        let second = workspace_locator(Some(WorkspaceScope::Local), None, Some("device-b"));
        assert!(first.is_some());
        assert_ne!(first, second);
    }

    #[test]
    fn synced_workspace_locator_waits_for_signed_in_identity() {
        let scope = Some(WorkspaceScope::Synced);
        assert_eq!(workspace_locator(scope, None, Some("device-a")), None);
        assert_eq!(
            workspace_locator(scope, Some(&AuthState::SignedOut), Some("device-a")),
            None
        );
        assert_eq!(
            workspace_locator(
                scope,
                Some(&AuthState::NeedsOrganization {
                    user: zeron_proto::UserProfile {
                        id: "user-a".into(),
                        email: "user@example.com".into(),
                        name: None,
                    },
                }),
                Some("device-a"),
            ),
            None
        );
        assert!(
            workspace_locator(
                scope,
                Some(&AuthState::SignedIn {
                    user: zeron_proto::UserProfile {
                        id: "user-a".into(),
                        email: "user@example.com".into(),
                        name: None,
                    },
                    org_id: Some("org-a".into()),
                }),
                None,
            )
            .is_some()
        );
    }

    #[test]
    fn codex_link_is_exact_and_unverified_harnesses_are_omitted() {
        assert_eq!(
            harness_conversation_link(&harness_chat(HarnessId::Codex))
                .unwrap()
                .url,
            "codex://threads/thread%2Fone"
        );
        assert!(harness_conversation_link(&harness_chat(HarnessId::Hermes)).is_none());
    }
}
