//! A chat reference as a compact inline chip: harness brand mark, live title,
//! live status glyph. Resolution always happens at render time (rows are
//! prepared off the UI thread), so a rename or a status flip is a repaint,
//! not a rebuild.

use std::hash::{Hash, Hasher};

use chrono::Utc;
use gpui::{AnyElement, EntityId, SharedString, div, prelude::*, px};

use zeron_proto::{ChatIndicator, HarnessId};

use crate::icons::{self, icon};
use crate::state::AppState;
use crate::theme::Theme;

/// A chat resolved for display. `known == false` means the id has no registry
/// row — the chip renders the placeholder rather than guessing.
#[derive(Debug, Clone)]
pub struct ChatRef {
    pub chat_id: String,
    pub title: SharedString,
    pub harness: Option<HarnessId>,
    pub model: Option<SharedString>,
    pub indicator: ChatIndicator,
    pub known: bool,
}

impl ChatRef {
    /// Resolve by full chat id only. Unknown ids degrade to a muted
    /// "Unavailable chat" chip that never opens anything.
    pub(crate) fn resolve(state: &AppState, chat_id: &str) -> ChatRef {
        match state.chats.iter().find(|chat| chat.id == chat_id) {
            Some(chat) => ChatRef {
                chat_id: chat.id.clone(),
                title: chat_title(chat),
                harness: chat.config.as_ref().map(|config| config.harness),
                model: chat
                    .config
                    .as_ref()
                    .and_then(|config| config.model.clone())
                    .map(SharedString::from),
                indicator: state.display_status_for(chat, Utc::now()),
                known: true,
            },
            None => Self::unavailable(chat_id),
        }
    }

    /// The unknown-chat chip: muted "Unavailable chat", never opens anything.
    /// Split from [`Self::resolve`] so snapshot resolvers can degrade ids that
    /// were never fingerprinted (e.g. a pill id absent from `chat_refs`).
    pub(crate) fn unavailable(chat_id: &str) -> ChatRef {
        ChatRef {
            chat_id: chat_id.to_owned(),
            title: "Unavailable chat".into(),
            harness: None,
            model: None,
            indicator: ChatIndicator::Idle,
            known: false,
        }
    }

    /// Resolve a CLI-style reference — full id, unique id prefix, or exact
    /// title — the same inputs `zeron chat` accepts. `None` when nothing
    /// matches, so callers can fall back to the raw text.
    pub(crate) fn resolve_target(state: &AppState, target: &str) -> Option<ChatRef> {
        let target = target.trim();
        if target.is_empty() {
            return None;
        }
        if let Some(chat) = state.chats.iter().find(|chat| chat.id == target) {
            return Some(Self::resolve(state, &chat.id));
        }
        let mut prefix = state
            .chats
            .iter()
            .filter(|chat| chat.id.starts_with(target));
        // A unique prefix resolves; zero or several matches fall through to
        // the exact-title check.
        if let (Some(chat), None) = (prefix.next(), prefix.next()) {
            return Some(Self::resolve(state, &chat.id));
        }
        state
            .chats
            .iter()
            .find(|chat| chat.title.as_deref() == Some(target))
            .map(|chat| Self::resolve(state, &chat.id))
    }
}

/// The harness names a chat's title may repeat in parentheses. A pill shows
/// the harness mark right beside the title, so a trailing "(Devin)" beside
/// the Devin mark is noise; the comparison is case-insensitive because the
/// title is free text. Claude Code is also authored as "Claude".
fn harness_title_names(harness: HarnessId) -> &'static [&'static str] {
    use HarnessId::*;
    match harness {
        ClaudeCode => &["Claude Code", "Claude"],
        Codex => &["Codex"],
        Cursor => &["Cursor"],
        Devin => &["Devin"],
        Grok => &["Grok"],
        Hermes => &["Hermes"],
        Pi => &["Pi"],
        Opencode => &["OpenCode"],
        Antigravity => &["Antigravity"],
        Mock => &["Mock"],
    }
}

/// The harness name a tooltip or menu shows the user.
pub(crate) fn harness_display_name(harness: HarnessId) -> &'static str {
    harness_title_names(harness)[0]
}

/// The title a pill draws. A trailing parenthesised harness name that names
/// the chat's own harness is dropped — the mark already says which harness
/// the chat runs — but a title that would be nothing else is kept whole.
/// Tooltips keep the stored title untouched.
pub(crate) fn display_title(title: &str, harness: Option<HarnessId>) -> &str {
    let Some(harness) = harness else {
        return title;
    };
    let Some(inside) = title.trim_end().strip_suffix(')') else {
        return title;
    };
    let Some(open) = inside.rfind('(') else {
        return title;
    };
    let name = inside[open + 1..].trim();
    if !harness_title_names(harness)
        .iter()
        .any(|known| known.eq_ignore_ascii_case(name))
    {
        return title;
    }
    let stripped = title[..open].trim_end();
    if stripped.is_empty() { title } else { stripped }
}

/// An untitled chat reads like the sidebar's rows ("New session"); the
/// message preview stands in when a title hasn't been generated yet.
fn chat_title(chat: &zeron_proto::Chat) -> SharedString {
    chat.title
        .clone()
        .or_else(|| chat.last_message_preview.clone())
        .unwrap_or_else(|| "New session".into())
        .into()
}

/// Hash the resolved identity of every referenced chat: title, harness, model
/// and display indicator — the same inputs [`crate::chat_activity::fingerprint`]
/// tracks, so a chip repaints exactly when the menu row would. References may
/// be full ids, unique prefixes or exact titles (CLI-style), so resolution
/// goes through [`ChatRef::resolve_target`].
pub(crate) fn chat_refs_fingerprint<'a>(
    state: &AppState,
    ids: impl IntoIterator<Item = &'a str>,
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for id in ids {
        id.hash(&mut hasher);
        match ChatRef::resolve_target(state, id) {
            Some(chat) => {
                chat.chat_id.hash(&mut hasher);
                chat.title.as_ref().hash(&mut hasher);
                chat.harness.map(|h| h as u8).hash(&mut hasher);
                chat.model
                    .as_ref()
                    .map(SharedString::as_str)
                    .hash(&mut hasher);
                (chat.indicator as u8).hash(&mut hasher);
            }
            None => 0u8.hash(&mut hasher),
        }
    }
    hasher.finish()
}

/// The status slot every chat row ends in: spinner while working, check when
/// finished-unseen, a plain status dot otherwise. Shared by the activity
/// menu and the transcript's chat rows.
pub(crate) fn status_glyph(
    key: String,
    status: ChatIndicator,
    view: EntityId,
    theme: &Theme,
    cx: &mut gpui::App,
) -> AnyElement {
    let color = crate::shell::spaces::status_dot_color(status, theme);
    let glyph: AnyElement = match status {
        ChatIndicator::Completed => icon(icons::CHECK)
            .size(px(11.0))
            .flex_none()
            .text_color(color)
            .into_any_element(),
        ChatIndicator::Working => {
            crate::loaders::mini_glyph_spinner(format!("{key}-working"), 2.0, theme.glyph, view, cx)
                .into_any_element()
        }
        _ => div()
            .size(px(6.0))
            .flex_none()
            .rounded_full()
            .bg(color)
            .into_any_element(),
    };
    div()
        .flex_none()
        .size(px(13.0))
        .flex()
        .items_center()
        .justify_center()
        .child(glyph)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_proto::Chat;

    fn chat(id: &str, title: Option<&str>) -> Chat {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "deviceId": "dev",
            "archived": false,
            "createdAt": Utc::now(),
            "title": title,
            "config": {
                "harness": "claude-code",
                "model": "opus",
                "sandbox": "workspace-write"
            },
        }))
        .unwrap()
    }

    #[test]
    fn resolve_marks_unknown_and_untitled_chats() {
        let mut state = AppState::new();
        state.chats.push(chat("a1b2c3", None));
        let missing = ChatRef::resolve(&state, "nope");
        assert!(!missing.known);
        assert_eq!(missing.title.as_ref(), "Unavailable chat");
        let untitled = ChatRef::resolve(&state, "a1b2c3");
        assert!(untitled.known);
        assert_eq!(untitled.title.as_ref(), "New session");
        assert_eq!(untitled.harness, Some(HarnessId::ClaudeCode));
        assert_eq!(untitled.model.as_deref(), Some("opus"));
    }

    #[test]
    fn resolve_target_accepts_full_id_unique_prefix_and_exact_title() {
        let mut state = AppState::new();
        let mut named = chat("3f6b2a18-x", None);
        named.id = "3f6b2a18-9c4d".into();
        named.title = Some("Refactor".into());
        state.chats.push(named);
        state.chats.push(chat("3f6b2a18-y", None));
        state.chats.last_mut().unwrap().id = "3f6b2a18-zzzz".into();
        assert_eq!(
            ChatRef::resolve_target(&state, "3f6b2a18-9c4d").map(|r| r.title.to_string()),
            Some("Refactor".into())
        );
        assert_eq!(
            ChatRef::resolve_target(&state, "Refactor").map(|r| r.chat_id),
            Some("3f6b2a18-9c4d".into())
        );
        // An ambiguous prefix resolves nothing (the raw text renders muted).
        assert!(ChatRef::resolve_target(&state, "3f6b2a18").is_none());
        assert!(ChatRef::resolve_target(&state, "nope").is_none());
        assert!(ChatRef::resolve_target(&state, "  ").is_none());
    }

    #[test]
    fn display_title_strips_only_the_chats_own_trailing_harness() {
        let devin = Some(HarnessId::Devin);
        assert_eq!(
            display_title("Editor P2 rutas de sfx y fx (Devin)", devin),
            "Editor P2 rutas de sfx y fx"
        );
        assert_eq!(
            display_title("Audit the sync layer (Codex)", Some(HarnessId::Codex)),
            "Audit the sync layer"
        );
        // Case-insensitive, and Claude Code also authors as "Claude".
        assert_eq!(
            display_title("Fix the queue (codex)", Some(HarnessId::Codex)),
            "Fix the queue"
        );
        assert_eq!(
            display_title("Refactor the parser (claude)", Some(HarnessId::ClaudeCode)),
            "Refactor the parser"
        );
        assert_eq!(
            display_title(
                "Refactor the parser (Claude Code)",
                Some(HarnessId::ClaudeCode)
            ),
            "Refactor the parser"
        );
    }

    #[test]
    fn display_title_keeps_other_harnesses_and_empty_results() {
        // A different harness is a real disambiguator: keep it.
        assert_eq!(
            display_title("Audit the sync layer (Codex)", Some(HarnessId::Devin)),
            "Audit the sync layer (Codex)"
        );
        assert_eq!(
            display_title("Audit the sync layer", Some(HarnessId::Codex)),
            "Audit the sync layer"
        );
        // Parentheses that are not the harness, unknown chats, and titles
        // that are only the harness name all survive intact.
        assert_eq!(
            display_title("Retry backoff (draft)", Some(HarnessId::Codex)),
            "Retry backoff (draft)"
        );
        assert_eq!(
            display_title("Audit the sync layer (Codex)", None),
            "Audit the sync layer (Codex)"
        );
        assert_eq!(display_title("(Devin)", Some(HarnessId::Devin)), "(Devin)");
        assert_eq!(
            display_title(" (Devin)", Some(HarnessId::Devin)),
            " (Devin)"
        );
    }

    #[test]
    fn fingerprint_tracks_title_and_live_status() {
        let mut state = AppState::new();
        state.chats.push(chat("c1", Some("Child")));
        let base = chat_refs_fingerprint(&state, ["c1"]);
        assert_eq!(base, chat_refs_fingerprint(&state, ["c1"]));
        state.chats[0].title = Some("Renamed".into());
        let renamed = chat_refs_fingerprint(&state, ["c1"]);
        assert_ne!(base, renamed);
        // A send in flight reads as Working — the same flip the live chip shows.
        state.begin_pending_send("c1", "m1", Utc::now());
        assert_ne!(renamed, chat_refs_fingerprint(&state, ["c1"]));
        // Membership changes fingerprint too.
        assert_ne!(renamed, chat_refs_fingerprint(&state, ["c1", "other"]));
    }
}
