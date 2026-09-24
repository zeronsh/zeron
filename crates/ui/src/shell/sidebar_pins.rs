//! Optimistic pin/section writes are an overlay, never the authoritative watch state.
//! Serialize drops per attachment; old replies cannot undo a newer drop/profile.

use super::*;
use crate::state::EngineHandle;
use std::collections::VecDeque;
use zeron_proto::{SidebarPinChange, SidebarPreferencesState};

pub(super) struct PendingSidebarPins {
    pub id: u64,
    pub profile_key: String,
    pub engine: EngineHandle,
    pub queue: VecDeque<SidebarPinChange>,
    pub unconfirmed: bool,
}

/// A pin write failure. Copy this crate authors stays a key until the notice is
/// stored, and an engine or transport payload is shown as it arrived.
pub(super) enum PinWriteFailure {
    Copy(MessageId),
    Detail(String),
}

impl PinWriteFailure {
    fn text(self, locale: Locale) -> SharedString {
        match self {
            Self::Copy(id) => SharedString::from(i18n::translate(id, locale)),
            Self::Detail(detail) => SharedString::from(detail),
        }
    }
}

pub(super) fn preferences_reply(
    value: serde_json::Value,
) -> Result<SidebarPreferencesState, PinWriteFailure> {
    serde_json::from_value(value.get("sidebarPreferences").cloned().unwrap_or_default())
        .map_err(|_| PinWriteFailure::Copy(MessageId::SidebarPinsNotConfirmed))
}

impl Shell {
    fn set_pin_write_notice(&mut self, message: SharedString) {
        self.sidebar_pin_write_notice = Some(message.clone());
        self.sidebar_notice = Some(message);
    }

    fn clear_pin_write_notice(&mut self) {
        if self.sidebar_notice == self.sidebar_pin_write_notice {
            self.sidebar_notice = None;
        }
        self.sidebar_pin_write_notice = None;
    }

    fn pin_write_is_current(&self, pending: &PendingSidebarPins, cx: &App) -> bool {
        self.active_sidebar_pin_profile_key(cx).as_ref() == Some(&pending.profile_key)
            && self
                .state
                .read(cx)
                .engine()
                .is_some_and(|engine| engine.same_connection(&pending.engine))
    }

    pub(super) fn optimistic_sidebar_pins(&self, cx: &App) -> Option<Vec<String>> {
        let pending = self
            .sidebar_pin_write
            .as_ref()
            .filter(|pending| !pending.unconfirmed && self.pin_write_is_current(pending, cx))?;
        let mut pins = self
            .state
            .read(cx)
            .sidebar_preferences
            .pinned_session_ids
            .clone();
        for change in &pending.queue {
            change.project(&mut pins);
        }
        Some(pins)
    }

    pub(super) fn discard_stale_sidebar_pin_writes(&mut self, cx: &App) {
        if self
            .sidebar_pin_write
            .as_ref()
            .is_some_and(|pending| !self.pin_write_is_current(pending, cx))
        {
            self.sidebar_pin_write = None;
        }
    }

    pub(super) fn queue_sidebar_pin_write(
        &mut self,
        profile_key: String,
        change: SidebarPinChange,
        cx: &mut Context<Self>,
    ) -> bool {
        self.discard_stale_sidebar_pin_writes(cx);
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.set_pin_write_notice(SharedString::from(i18n::translate(
                MessageId::SidebarPinsEngineOffline,
                i18n::locale(cx),
            )));
            cx.notify();
            return false;
        };
        if let Some(pending) = &mut self.sidebar_pin_write {
            if pending.unconfirmed {
                self.set_pin_write_notice(SharedString::from(i18n::translate(
                    MessageId::SidebarPinsAwaitingConfirmation,
                    i18n::locale(cx),
                )));
                cx.notify();
                return false;
            }
            pending.queue.push_back(change);
            cx.notify();
            return true;
        }
        self.sidebar_pin_write_generation += 1;
        let id = self.sidebar_pin_write_generation;
        self.sidebar_pin_write = Some(PendingSidebarPins {
            id,
            profile_key,
            engine: engine.clone(),
            queue: VecDeque::from([change.clone()]),
            unconfirmed: false,
        });
        // Detached from generic sidebar mutations: rename/archive must not
        // cancel a pin write, and rapid drops must reach the engine in order.
        cx.spawn(async move |this, cx| {
            let mut next = change;
            loop {
                let request = engine.client().call(
                    methods::MUTATE,
                    serde_json::json!({
                        "op": "changeSidebarPin", "change": next,
                    }),
                );
                let deadline = cx.background_executor().timer(Duration::from_secs(20));
                let result =
                    match futures::future::select(Box::pin(request), Box::pin(deadline)).await {
                        futures::future::Either::Left((result, _)) => result
                            .map_err(|error| PinWriteFailure::Detail(error.to_string()))
                            .and_then(preferences_reply),
                        futures::future::Either::Right((_, request)) => {
                            // The old request may still run. Do not send a later
                            // intent whose execution order is uncertain.
                            this.update(cx, |shell, cx| shell.mark_pin_write_unconfirmed(id, cx))
                                .ok();
                            // Keep observing the original request. No later user
                            // drop may overtake it until it resolves or disconnects.
                            request
                                .await
                                .map_err(|error| PinWriteFailure::Detail(error.to_string()))
                                .and_then(preferences_reply)
                        }
                    };
                let queued = this
                    .update(cx, |shell, cx| {
                        shell.finish_sidebar_pin_write(id, result, cx)
                    })
                    .ok()
                    .flatten();
                let Some(queued) = queued else { break };
                next = queued;
            }
        })
        .detach();
        cx.notify();
        true
    }

    pub(super) fn finish_sidebar_pin_write(
        &mut self,
        id: u64,
        result: Result<SidebarPreferencesState, PinWriteFailure>,
        cx: &mut Context<Self>,
    ) -> Option<SidebarPinChange> {
        self.discard_stale_sidebar_pin_writes(cx);
        if self
            .sidebar_pin_write
            .as_ref()
            .is_none_or(|pending| pending.id != id)
        {
            return None;
        }
        let imported_profile = if result.is_ok() {
            self.sidebar_pin_write.as_ref().and_then(|pending| {
                matches!(
                    pending.queue.front(),
                    Some(SidebarPinChange::Section {
                        change: zeron_proto::SidebarSectionChange::Import { .. }
                    })
                )
                .then(|| pending.profile_key.clone())
            })
        } else {
            None
        };
        match result {
            Ok(value) => {
                self.clear_pin_write_notice();
                self.state.update(cx, |state, cx| {
                    if state.apply_sidebar_preferences(value) {
                        cx.notify();
                    }
                });
            }
            Err(failure) => {
                let locale = i18n::locale(cx);
                self.set_pin_write_notice(
                    i18n::fill(
                        MessageId::SidebarPinsSaveFailed,
                        "{error}",
                        &failure.text(locale),
                        locale,
                    )
                    .into(),
                );
            }
        }
        if let Some(profile) = imported_profile {
            // The engine durably owns these rows (including its offline outbox).
            self.settings.sidebar_sections_by_profile.remove(&profile);
            self.schedule_save(cx);
        }
        let pending = self.sidebar_pin_write.as_mut().unwrap();
        pending.queue.pop_front();
        let next = pending.queue.front().cloned();
        if next.is_none() {
            // Removing the overlay reveals the latest watch/ack, not a stale
            // pre-drag backup that could erase a concurrent remote update.
            self.sidebar_pin_write = None;
        }
        cx.notify();
        next
    }

    pub(super) fn mark_pin_write_unconfirmed(&mut self, id: u64, cx: &mut Context<Self>) {
        self.discard_stale_sidebar_pin_writes(cx);
        if self
            .sidebar_pin_write
            .as_ref()
            .is_some_and(|pending| pending.id == id)
        {
            let pending = self.sidebar_pin_write.as_mut().unwrap();
            pending.queue.clear();
            pending.unconfirmed = true;
            self.set_pin_write_notice(SharedString::from(i18n::translate(
                MessageId::SidebarPinsUnconfirmed,
                i18n::locale(cx),
            )));
            cx.notify();
        }
    }
}
