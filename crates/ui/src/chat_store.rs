//! A single live RPC feed per conversation. Windows retain the source while
//! displaying it and keep incremental rendering projections of its frames.
//! Weak cache entries do not keep inactive documents pinned in the engine.

use std::collections::HashMap;

use gpui::{App, AppContext, Entity, Global, Subscription, WeakEntity};

use crate::state::{AppState, EngineHandle};

#[derive(Default)]
pub(crate) struct ChatStore {
    sources: HashMap<String, WeakEntity<AppState>>,
}

impl Global for ChatStore {}

pub(crate) struct ConversationBinding {
    pub source: Entity<AppState>,
    pub _subscriptions: Vec<Subscription>,
}

pub(crate) struct ConversationFrame {
    pub frame: zeron_doc::TranscriptFrame,
}

impl gpui::EventEmitter<ConversationFrame> for AppState {}

pub(crate) fn acquire(
    chat_id: String,
    engine: Option<EngineHandle>,
    cx: &mut App,
) -> Entity<AppState> {
    if !cx.has_global::<ChatStore>() {
        cx.set_global(ChatStore::default());
    }
    if let Some(source) = cx
        .global::<ChatStore>()
        .sources
        .get(&chat_id)
        .and_then(WeakEntity::upgrade)
    {
        return source;
    }
    let source = cx.new(|cx| AppState::conversation_source(chat_id.clone(), engine, cx));
    let store = cx.global_mut::<ChatStore>();
    store.sources.retain(|_, source| source.upgrade().is_some());
    store.sources.insert(chat_id, source.downgrade());
    source
}

pub(crate) fn clear(cx: &mut App) {
    if cx.has_global::<ChatStore>() {
        cx.global_mut::<ChatStore>().sources.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn views_share_one_feed_and_release_it_after_the_last_consumer(cx: &mut gpui::TestAppContext) {
        let weak = cx.update(|cx| {
            let a = acquire("chat-a".into(), None, cx);
            let b = acquire("chat-a".into(), None, cx);
            let c = acquire("chat-c".into(), None, cx);
            assert_eq!(a, b);
            assert_ne!(a, c);
            let weak = a.downgrade();
            drop(a);
            assert!(weak.upgrade().is_some());
            drop(b);
            weak
        });
        cx.update(|_| assert!(weak.upgrade().is_none()));
    }
}
