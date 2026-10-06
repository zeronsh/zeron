use crate::{
    popover,
    settings::{self, widgets},
    theme::Theme,
};
use gpui::{App, Context, Entity, Global, Task, Window, div, prelude::*, px};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
struct VoiceGlobal {
    card: Entity<VoiceCard>,
    directory: PathBuf,
}
impl Global for VoiceGlobal {}
pub(crate) fn init(root: PathBuf, cx: &mut App) {
    let directory = root.join("models/parakeet-tdt-0.6b-v3-int8");
    let card = cx.new(|_| VoiceCard {
        scroll: widgets::PageScroll::default(),
        ready: zeron_voice::installed(&directory),
        cache_present: directory.exists(),
        directory: directory.clone(),
        cancel: None,
        task: None,
        progress: 0,
        error: None,
        inputs: Inputs::default(),
        input_select: widgets::SelectState::default(),
        shortcut: None,
    });
    cx.set_global(VoiceGlobal { card, directory });
}
pub(crate) fn directory(cx: &App) -> PathBuf {
    cx.global::<VoiceGlobal>().directory.clone()
}
pub(crate) fn enabled(cx: &App) -> bool {
    settings::current(cx).dictation_enabled
        && cx
            .try_global::<VoiceGlobal>()
            .is_some_and(|g| g.card.read(cx).ready)
}
pub(crate) fn card(cx: &mut App) -> Entity<VoiceCard> {
    if !cx.has_global::<VoiceGlobal>() {
        init(std::env::temp_dir().join("zeron-voice-test"), cx);
    }
    cx.global::<VoiceGlobal>().card.clone()
}
pub(crate) struct VoiceCard {
    scroll: widgets::PageScroll,
    directory: PathBuf,
    ready: bool,
    cache_present: bool,
    cancel: Option<Arc<AtomicBool>>,
    task: Option<Task<()>>,
    progress: u64,
    error: Option<String>,
    inputs: Inputs,
    input_select: widgets::SelectState,
    /// Created on first render (it needs this card's context).
    shortcut: Option<Entity<settings::shortcuts::ShortcutField>>,
}
/// Microphones as last enumerated, refreshed off the UI thread while the page
/// is visible so newly connected devices appear.
#[derive(Default)]
struct Inputs {
    devices: Vec<zeron_voice::InputDevice>,
    default: Option<String>,
    checked: Option<Instant>,
    task: Option<Task<()>>,
}
const INPUT_REFRESH: Duration = Duration::from_secs(3);
impl VoiceCard {
    fn set_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        settings::update(settings::SavePolicy::Immediate, cx, |s| {
            s.dictation_enabled = enabled
        });
        cx.refresh_windows();
        cx.notify();
    }
    fn primary(&mut self, cx: &mut Context<Self>) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Release);
        } else if self.ready {
            self.set_enabled(!settings::current(cx).dictation_enabled, cx);
        } else {
            self.download(cx);
        }
        cx.notify();
    }
    fn download(&mut self, cx: &mut Context<Self>) {
        if self.cancel.is_some() {
            return;
        }
        self.error = None;
        self.progress = 0;
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        let progress = Arc::new(AtomicU64::new(0));
        let counter = progress.clone();
        let dir = self.directory.clone();
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let r = zeron_voice::download(&dir, &cancel, |n| counter.store(n, Ordering::Relaxed))
                .map_err(|_| {
                if cancel.load(Ordering::Acquire) { "Download cancelled.".to_owned() }
                else { "Couldn’t download or verify the model. Check your connection and free storage, then retry.".to_owned() }
            });
            let _ = tx.send(r);
        });
        self.task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                let result = rx.try_recv();
                let done = !matches!(result, Err(std::sync::mpsc::TryRecvError::Empty));
                if this
                    .update(cx, |this, cx| {
                        this.progress = progress.load(Ordering::Relaxed);
                        if done {
                            this.cache_present = this.directory.exists();
                        }
                        match result {
                            Ok(Ok(())) => {
                                this.ready = true;
                                let cancelled = this
                                    .cancel
                                    .as_ref()
                                    .is_some_and(|c| c.load(Ordering::Acquire));
                                this.cancel = None;
                                this.set_enabled(!cancelled, cx);
                            }
                            Ok(Err(e)) => {
                                this.cancel = None;
                                this.error = Some(e);
                            }
                            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                                this.cancel = None;
                                this.error = Some("Download interrupted. Try again.".into());
                            }
                            _ => {}
                        }
                        cx.notify();
                    })
                    .is_err()
                    || done
                {
                    break;
                }
            }
        }));
        cx.notify();
    }
    fn remove(&mut self, cx: &mut Context<Self>) {
        if zeron_voice::busy() {
            self.error = Some("Stop dictation before removing the model.".into());
            cx.notify();
            return;
        }
        self.set_enabled(false, cx);
        zeron_voice::unload();
        match std::fs::remove_dir_all(&self.directory) {
            Ok(()) => {
                self.cache_present = false;
                self.ready = false;
                self.progress = 0;
                self.error = None
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.cache_present = false;
                self.ready = false;
                self.progress = 0;
                self.error = None;
            }
            Err(_) => {
                self.error =
                    Some("Could not remove the model. Check folder permissions and retry.".into())
            }
        }
        cx.notify();
    }
}
impl popover::ScrollRailHost for VoiceCard {
    fn rail_bar(&mut self) -> &mut popover::MenuScrollbarState {
        self.scroll.rail_bar()
    }
    fn rail_scroll(&self) -> Option<gpui::ScrollHandle> {
        self.scroll.rail_scroll()
    }
}
impl VoiceCard {
    fn refresh_inputs(&mut self, cx: &mut Context<Self>) {
        if self.inputs.task.is_some()
            || self
                .inputs
                .checked
                .is_some_and(|at| at.elapsed() < INPUT_REFRESH)
        {
            return;
        }
        let scan = cx.background_executor().spawn(async {
            (
                zeron_voice::input_devices(),
                zeron_voice::default_input_device(),
            )
        });
        self.inputs.task = Some(cx.spawn(async move |this, cx| {
            let (devices, default) = scan.await;
            this.update(cx, |this, cx| {
                let changed = this.inputs.devices != devices || this.inputs.default != default;
                this.inputs.devices = devices;
                this.inputs.default = default;
                this.inputs.checked = Some(Instant::now());
                if changed {
                    cx.notify();
                }
            })
            .ok();
            // A static settings page otherwise has no reason to render again,
            // so checking the age only in render misses hot-plugged devices.
            // Invalidate once when the scan expires. Only a visible card starts
            // the next scan, so leaving Settings does not keep polling devices.
            cx.background_executor().timer(INPUT_REFRESH).await;
            this.update(cx, |this, cx| {
                this.inputs.task = None;
                cx.notify();
            })
            .ok();
        }));
    }

    /// Options for the microphone select and the index of the current one. A
    /// saved device that is unplugged stays listed so the choice is visible;
    /// recording falls back to the system default meanwhile.
    fn input_options(&self, saved: Option<&str>) -> (Vec<widgets::SelectOption>, usize) {
        let default_name = self.inputs.default.as_ref().and_then(|id| {
            self.inputs
                .devices
                .iter()
                .find(|d| &d.id == id)
                .map(|d| d.name.clone())
        });
        let mut options = vec![match default_name {
            Some(name) => widgets::SelectOption::new("System default").detail(name),
            None => widgets::SelectOption::new("System default"),
        }];
        options.extend(
            self.inputs
                .devices
                .iter()
                .map(|d| widgets::SelectOption::new(d.name.clone())),
        );
        let selected = match saved {
            None => 0,
            Some(id) => match self.inputs.devices.iter().position(|d| d.id == id) {
                Some(ix) => ix + 1,
                None => {
                    options.push(widgets::SelectOption::new("Disconnected microphone"));
                    options.len() - 1
                }
            },
        };
        (options, selected)
    }
}
impl Render for VoiceCard {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).for_settings_surface();
        let enabled = settings::current(cx).dictation_enabled;
        let downloading = self.cancel.is_some();
        let on = downloading || (enabled && self.ready);
        let size_mb = zeron_voice::download_size() as f64 / 1e6;
        let meta = if downloading {
            Some(format!(
                "Downloading · {:.0} of {size_mb:.0} MB",
                self.progress as f64 / 1e6
            ))
        } else if !self.ready {
            Some(format!("{size_mb:.0} MB download"))
        } else {
            None
        };
        let weak = cx.entity().downgrade();
        let switch = widgets::toggle_switch(&theme, on, "voice-dictation")
            .id("voice-enable")
            .cursor_pointer()
            .tab_index(0)
            .role(gpui::Role::Switch)
            .aria_label("Dictation")
            .aria_toggled(if on {
                gpui::Toggled::True
            } else {
                gpui::Toggled::False
            })
            .focus_visible(|s| s.border_2().border_color(theme.accent).opacity(1.0))
            .on_click(cx.listener(|this, _, _, cx| this.primary(cx)))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    cx.stop_propagation();
                    this.primary(cx);
                }
            }))
            .on_a11y_action(gpui::AccessibleAction::Click, move |_, _, cx| {
                weak.update(cx, |this, cx| this.primary(cx)).ok();
            });
        let progress = downloading.then(|| {
            div()
                .mt(px(8.0))
                .w_full()
                .h(px(3.0))
                .rounded_full()
                .bg(theme.border)
                .child(
                    div()
                        .h_full()
                        .rounded_full()
                        .w(gpui::relative(
                            (self.progress as f32 / zeron_voice::download_size() as f32)
                                .clamp(0.0, 1.0),
                        ))
                        .bg(theme.accent),
                )
        });
        let dictation_row = widgets::card_row(&theme, true)
            .child(
                div()
                    .flex_1()
                    .min_w(px(160.0))
                    .flex()
                    .flex_col()
                    .child(widgets::row_title(&theme, "Dictation"))
                    .children(meta.map(|meta| {
                        div()
                            .id("voice-status")
                            .role(gpui::Role::Status)
                            .aria_label(meta.clone())
                            .child(widgets::meta_line(&theme, vec![meta.into_any_element()]))
                    }))
                    .children(progress),
            )
            .child(switch);
        let microphone_row = (enabled && self.ready).then(|| {
            self.refresh_inputs(cx);
            let saved = settings::current(cx).dictation_input.clone();
            let (options, selected) = self.input_options(saved.as_deref());
            let ids: Vec<Option<String>> = std::iter::once(None)
                .chain(self.inputs.devices.iter().map(|d| Some(d.id.clone())))
                .chain(std::iter::once(saved))
                .collect();
            let control = widgets::select(
                "voice-microphone",
                "Microphone",
                &theme,
                |card: &mut Self| &mut card.input_select,
            )
            .options(options, selected)
            .width(200.0)
            .on_select(move |_, ix, _, cx| {
                let input = ids.get(ix).cloned().flatten();
                settings::update(settings::SavePolicy::Immediate, cx, |s| {
                    s.dictation_input = input
                });
                cx.notify();
            })
            .render(&self.input_select, cx);
            widgets::card_row(&theme, false)
                .child(
                    div()
                        .flex_1()
                        .min_w(px(160.0))
                        .child(widgets::row_title(&theme, "Microphone")),
                )
                .child(control)
        });
        // Always visible, so ⌘D can be rebound (and freed) before the model
        // is downloaded.
        let shortcut_row = {
            let field = self
                .shortcut
                .get_or_insert_with(|| {
                    cx.new(|cx| {
                        settings::shortcuts::ShortcutField::new(
                            settings::ShortcutId::ToggleDictation,
                            cx,
                        )
                    })
                })
                .clone();
            widgets::card_row(&theme, false)
                .child(
                    div()
                        .flex_1()
                        .min_w(px(160.0))
                        .flex()
                        .flex_col()
                        .child(widgets::row_title(&theme, "Shortcut"))
                        .child(widgets::meta_line(
                            &theme,
                            vec!["Hold to talk, release to transcribe".into_any_element()],
                        )),
                )
                .child(field)
        };
        let model_card = (self.cache_present && !downloading).then(|| {
            let remove_weak = cx.entity().downgrade();
            widgets::section_card(&theme).child(
                widgets::card_row(&theme, true)
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(160.0))
                            .flex()
                            .flex_col()
                            .child(widgets::row_title(&theme, "Speech model"))
                            .child(widgets::meta_line(
                                &theme,
                                vec![
                                    "Parakeet v3".into_any_element(),
                                    format!("{size_mb:.0} MB").into_any_element(),
                                ],
                            )),
                    )
                    .child(
                        widgets::action_button(&theme, widgets::ActionTone::Quiet)
                            .id("voice-remove")
                            .role(gpui::Role::Button)
                            .aria_label("Remove speech model")
                            .tab_index(0)
                            .child("Remove")
                            .focus_visible(|s| s.border_2().border_color(theme.accent))
                            .on_click(cx.listener(|this, _, _, cx| this.remove(cx)))
                            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                    cx.stop_propagation();
                                    this.remove(cx);
                                }
                            }))
                            .on_a11y_action(gpui::AccessibleAction::Click, move |_, _, cx| {
                                remove_weak.update(cx, |this, cx| this.remove(cx)).ok();
                            }),
                    ),
            )
        });
        let error = self.error.as_ref().map(|e| {
            div()
                .id("voice-error")
                .role(gpui::Role::Status)
                .aria_label(e.clone())
                .child(widgets::error_strip(&theme, e.clone()))
        });
        let scrollbar = popover::rail(self, "voice-page-scrollbar", &theme, cx);
        div()
            .id("voice-page-host")
            .relative()
            .size_full()
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if this.scroll.set_list_hovered(*hovered) {
                    cx.notify();
                }
            }))
            .child(
                crate::edge_fade::edge_faded(
                    16.0,
                    true,
                    true,
                    div()
                        .id("voice-page")
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll.scroll)
                        .child(
                            widgets::page_column()
                                .child(widgets::page_header(&theme, "Voice", None))
                                .child(widgets::page_subtitle(
                                    &theme,
                                    "Transcribed on this device. Audio is never saved.",
                                ))
                                .child(
                                    widgets::section_card(&theme)
                                        .child(dictation_row)
                                        .children(microphone_row)
                                        .child(shortcut_row),
                                )
                                .children(error)
                                .children(model_card),
                        ),
                )
                .fade_overflow_y(&self.scroll.scroll),
            )
            .children(scrollbar)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[gpui::test]
    fn dictation_partial_cache_is_removable_after_restart(cx: &mut gpui::TestAppContext) {
        let root = tempfile::tempdir().unwrap();
        let cache = root.path().join("models/parakeet-tdt-0.6b-v3-int8");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join("encoder-model.int8.onnx.part"), b"partial").unwrap();
        cx.update(|cx| settings::init(settings::UiSettings::default(), root.path(), cx));
        let card = cx.update(card);
        card.update(cx, |card, cx| {
            assert!(!card.ready);
            assert_eq!(card.progress, 0);
            assert!(card.cache_present);
            card.remove(cx);
            assert!(!card.cache_present);
            assert!(!card.ready);
        });
        assert!(!cache.exists());
    }
    #[gpui::test]
    fn microphone_choice_survives_disconnection(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| settings::init(settings::UiSettings::default(), dir.path(), cx));
        let card = cx.update(card);
        card.update(cx, |card, _| {
            card.inputs.devices = vec![
                zeron_voice::InputDevice {
                    id: "coreaudio:built-in".into(),
                    name: "MacBook Pro Microphone".into(),
                },
                zeron_voice::InputDevice {
                    id: "coreaudio:usb".into(),
                    name: "USB Microphone".into(),
                },
            ];
            let (options, selected) = card.input_options(None);
            assert_eq!((options.len(), selected), (3, 0));
            assert_eq!(card.input_options(Some("coreaudio:usb")).1, 2);
            let (options, selected) = card.input_options(Some("coreaudio:gone"));
            assert_eq!((options.len(), selected), (4, 3));
        });
    }
    #[gpui::test]
    fn dictation_is_opt_in_and_disabling_preserves_download(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| settings::init(settings::UiSettings::default(), dir.path(), cx));
        cx.update(|cx| assert!(!enabled(cx)));
        let card = cx.update(card);
        card.update(cx, |card, cx| {
            card.ready = true;
            assert!(!settings::current(cx).dictation_enabled);
            card.primary(cx);
        });
        cx.update(|cx| assert!(enabled(cx)));
        card.update(cx, |card, cx| {
            card.primary(cx);
            assert!(card.ready);
        });
        cx.update(|cx| assert!(!enabled(cx)));
    }
}
