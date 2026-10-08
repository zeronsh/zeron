//! The voice orchestrator's chrome: the sidebar footer trigger and the
//! full-window stage. A voice session follows no chat selection, so neither
//! surface lives inside the conversation column — the user moves between
//! threads freely while the orchestrator keeps listening.

use super::*;
use zeron_proto::voice::{VoicePhase, VoiceWork};

/// The stage paints the hero preset magnified to fill the canvas.
pub(super) const VOICE_STAGE_ORB_SCALE: f32 = 2.25;
/// Stage orb footprint, including breathing room around the artwork.
const STAGE_ORB_BOX: f32 = 128.0 * VOICE_STAGE_ORB_SCALE + 112.0;
const STAGE_ENTER_MS: f32 = 460.0;
const STAGE_EXIT_MS: f32 = 220.0;
/// Captions show the tail of a long utterance rather than wrapping off-stage.
pub(super) const STAGE_CAPTION_CHARS: usize = 160;
const FOOTER_HOVER: &str = "voice-footer-orb";
/// Call-bar geometry: round controls inside a floating pill.
const BAR_CONTROL: f32 = 44.0;
const BAR_PAD: f32 = 8.0;

/// Stage visibility for a transition that flipped `elapsed_ms` ago.
pub(super) fn stage_reveal(open: bool, elapsed_ms: f32, reduced: bool) -> f32 {
    match (open, reduced) {
        (true, true) => 1.0,
        (false, true) => 0.0,
        (true, false) => motion::EASE_OUT_EXPO.eval(elapsed_ms / STAGE_ENTER_MS),
        (false, false) => 1.0 - motion::EASE.eval(elapsed_ms / STAGE_EXIT_MS),
    }
}

impl Shell {
    /// The footer microphone: a new orchestrator in a projectless Codex chat
    /// on this device. The chat's model follows the composer when it already
    /// targets Codex; otherwise Codex's own default model runs the delegations.
    pub(super) fn start_voice(&mut self, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let (Some(engine), Some(device)) = (state.engine().cloned(), state.local_device_id.clone())
        else {
            return;
        };
        let config = self
            .composer
            .read(cx)
            .pickers()
            .read(cx)
            .resolved(cx)
            .chat_config()
            .filter(|config| config.harness == HarnessId::Codex)
            .unwrap_or_else(|| ChatConfig {
                harness: HarnessId::Codex,
                model: None,
                reasoning: None,
                model_options: Default::default(),
                sandbox: zeron_proto::SandboxLevel::WorkspaceWrite,
            });
        let saved = settings::current(cx);
        let device = saved.codex_voice_device.unwrap_or(device);
        let voice = saved.codex_voice;
        let host_name = state
            .device_name(&device)
            .unwrap_or("This device")
            .to_owned();
        self.voice.update(cx, |controller, cx| {
            controller.start(engine, device, config, voice, cx);
            controller.host_name = Some(host_name);
            controller.set_stage_open(true, cx);
        });
    }

    /// Read whether this device's Codex can take a call; `force` re-reads it
    /// after Settings → Agents changes or on coming back to a window that
    /// hides voice (Codex may have been installed meanwhile).
    pub(super) fn check_voice_codex(&mut self, force: bool, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        self.voice
            .update(cx, |voice, cx| voice.check_codex(&engine, force, cx));
    }

    pub(super) fn end_voice(&mut self, cx: &mut Context<Self>) {
        self.voice.update(cx, |voice, cx| voice.cancel(cx));
    }

    pub(super) fn set_voice_stage_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.voice
            .update(cx, |voice, cx| voice.set_stage_open(open, cx));
    }

    /// Track stage open/close flips for the transition clock and hand focus
    /// over: the stage owns the keyboard while it is up. Navigating to
    /// another thread or to Settings steps the stage aside; voice continues.
    pub(super) fn sync_voice_stage(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selected = self.state.read(cx).selected_chat.clone();
        let navigating_to_settings =
            self.route != self.voice_stage_route && matches!(self.route, Route::Settings(_));
        // Record every render, even with no visibility change: leaving and
        // reentering the same settings page must count as navigation too.
        self.voice_stage_route = self.route;
        if self.voice.read(cx).stage_open
            && self.voice_stage_was_open
            && (navigating_to_settings || selected != self.voice_stage_selection)
        {
            self.set_voice_stage_open(false, cx);
        }
        let open = self.voice.read(cx).stage_open;
        if open == self.voice_stage_was_open {
            return;
        }
        self.voice_stage_was_open = open;
        self.voice_stage_selection = selected;
        self.voice_stage_changed_at = Some(std::time::Instant::now());
        let focus = if open {
            self.voice_stage_focus.clone()
        } else {
            self.composer.focus_handle(cx)
        };
        if window.is_window_active() {
            window.focus(&focus, cx);
        }
    }

    pub(super) fn render_voice_trigger(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        self.check_voice_codex(false, cx);
        let voice = self.voice.read(cx);
        // Without Codex on this device a call could only fail.
        if !voice.offered() {
            return None;
        }
        let live = voice.is_live();
        let stage_open = voice.stage_open;
        let failure = (voice.phase == VoicePhase::Failed)
            .then(|| voice.reason_text())
            .filter(|text| !text.is_empty());
        let (orb_state, microphone, speaker) = (
            voice.orb_state(),
            voice.microphone_level(),
            voice.speaker_level(),
        );
        let reduced = self.reduced_motion;
        self.voice_footer_orb.update(cx, |orb, cx| {
            orb.set_visible(live, cx);
            orb.set_state(orb_state, cx);
            orb.set_audio_levels(microphone, speaker, cx);
            orb.set_reduced_motion(reduced, cx);
        });

        let button = div()
            .id("voice-trigger")
            .debug_selector(|| "voice-trigger".into())
            .role(gpui::Role::Button)
            .tab_index(0)
            .relative()
            .size(px(SIDEBAR_FOOTER_BUTTON_SIZE))
            .flex_none()
            .rounded(px(8.0))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .bg(motion::hover_blend(
                FOOTER_HOVER,
                theme.glass_hover().opacity(0.0),
                theme.glass_hover(),
            ))
            .on_hover(motion::hover_listener(FOOTER_HOVER))
            .focus_visible(|s| s.border_2().border_color(theme.accent));

        if live {
            return button
                .aria_label(if stage_open {
                    "Hide voice"
                } else {
                    "Open voice"
                })
                .tooltip(stage_tooltip(if stage_open {
                    "Hide voice · Esc"
                } else {
                    "Voice is live · open"
                }))
                .on_click(
                    cx.listener(move |this, _, _, cx| this.set_voice_stage_open(!stage_open, cx)),
                )
                .child(self.voice_footer_orb.clone())
                .into_any_element()
                .into();
        }

        let tooltip: SharedString = failure
            .map(SharedString::from)
            .unwrap_or_else(|| "Start voice · Codex orchestrator".into());
        let button = button
            .aria_label("Start voice")
            .tooltip(move |_, cx| {
                let text = tooltip.clone();
                cx.new(|_| SurfaceTabTooltip { text }).into()
            })
            .on_click(cx.listener(|this, _, _, cx| this.start_voice(cx)))
            .child(
                icon(icons::MICROPHONE)
                    .size(px(15.0))
                    .text_color(motion::hover_blend(
                        FOOTER_HOVER,
                        theme.text_muted,
                        theme.text,
                    )),
            );
        let Some(failure) = failure else {
            return Some(button.into_any_element());
        };
        // A failed start explains itself once, anchored to the microphone;
        // the next press retries.
        let popup_theme = theme.for_popup();
        let card = popover::popover_card(&popup_theme)
            .w(px(232.0))
            .flex()
            .flex_row()
            .items_start()
            .gap(px(8.0))
            .child(
                icon(icons::DANGER_TRIANGLE)
                    .mt(px(1.0))
                    .size(px(14.0))
                    .text_color(popup_theme.warning),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(crate::typography::ui_rems(12.0))
                    .text_color(popup_theme.text_muted)
                    .child(SharedString::from(failure)),
            )
            .child(
                div()
                    .id("voice-failure-dismiss")
                    .flex_none()
                    .cursor_pointer()
                    .rounded(px(4.0))
                    .on_click(cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.voice.update(cx, |voice, cx| voice.dismiss_reason(cx));
                    }))
                    .child(
                        icon(icons::CLOSE)
                            .size(px(14.0))
                            .text_color(popup_theme.text_faint),
                    ),
            )
            .into_any_element();
        button
            .child(
                div()
                    .absolute()
                    .top(px(4.0))
                    .right(px(4.0))
                    .size(px(5.0))
                    .rounded_full()
                    .bg(theme.warning),
            )
            .child(popover::anchored_menu_above_end(
                "voice-failure",
                card,
                None,
            ))
            .into_any_element()
            .into()
    }

    /// The live orb above the orchestrator chat's composer. Every other chat
    /// keeps its plain composer; pressing the orb returns to the stage.
    pub(super) fn render_voice_composer_orb(
        &mut self,
        composer_width: f32,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let voice = self.voice.read(cx);
        let shown = matches!(self.route, Route::Chat)
            && voice.is_live()
            && voice.chat_id.is_some()
            && voice.chat_id == self.state.read(cx).selected_chat;
        let (orb_state, microphone, speaker) = (
            voice.orb_state(),
            voice.microphone_level(),
            voice.speaker_level(),
        );
        let reduced = self.reduced_motion;
        self.voice_composer_orb.update(cx, |orb, cx| {
            orb.set_visible(shown, cx);
            orb.set_state(orb_state, cx);
            orb.set_audio_levels(microphone, speaker, cx);
            orb.set_reduced_motion(reduced, cx);
        });
        if !shown {
            return None;
        }
        let orb = div()
            .id("voice-composer-orb")
            .debug_selector(|| "voice-composer-orb".into())
            .role(gpui::Role::Button)
            .aria_label("Open voice")
            .tab_index(0)
            .rounded_full()
            .cursor_pointer()
            .tooltip(stage_tooltip("Open voice"))
            .on_click(cx.listener(|this, _, _, cx| this.set_voice_stage_open(true, cx)))
            .child(self.voice_composer_orb.clone());
        Some(
            motion::fade_in(
                "voice-composer-orb-in",
                div()
                    .w_full()
                    .max_w(px(composer_width))
                    .mx_auto()
                    .pb(px(4.0))
                    .flex()
                    .justify_center()
                    .child(orb),
            )
            .into_any_element(),
        )
    }

    /// How far the stage has arrived: 0 hidden, 1 fully covering the page.
    pub(super) fn voice_stage_reveal(&self, cx: &App) -> f32 {
        let elapsed = self
            .voice_stage_changed_at
            .map_or(f32::INFINITY, |at| at.elapsed().as_secs_f32() * 1000.0);
        stage_reveal(self.voice.read(cx).stage_open, elapsed, self.reduced_motion)
    }

    /// Opacity for the page the stage covers. A glass stage has no fill, so
    /// what it covers fades out and only the window frost shows through.
    pub(super) fn voice_stage_underlay_opacity(&self, cx: &App) -> f32 {
        if Theme::of(cx).is_glass() {
            1.0 - self.voice_stage_reveal(cx)
        } else {
            1.0
        }
    }

    /// Full-window stage: the session's orb over the new-thread hero artwork,
    /// live caption, and the session controls. Escape returns to the chats
    /// without ending voice.
    pub(super) fn render_voice_stage(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let open = self.voice.read(cx).stage_open;
        let reveal = self.voice_stage_reveal(cx);
        if reveal > 0.0 && reveal < 1.0 {
            self.motion_active.set(true);
        }
        let voice = self.voice.read(cx);
        let (orb_state, microphone, speaker) = (
            voice.orb_state(),
            voice.microphone_level(),
            voice.speaker_level(),
        );
        let (caption_item, partial) = (
            voice.caption_item().map(str::to_owned),
            voice.caption.text().to_owned(),
        );
        let snapshot = voice.snapshot.clone();
        let chat_id = voice.chat_id.clone();
        let awaiting = snapshot
            .as_ref()
            .is_some_and(|s| s.work == VoiceWork::AwaitingInput);
        let reduced = self.reduced_motion;
        self.voice_stage_orb.update(cx, |orb, cx| {
            orb.set_visible(open, cx);
            orb.set_state(orb_state, cx);
            orb.set_audio_levels(microphone, speaker, cx);
            orb.set_reduced_motion(reduced, cx);
            // The orb blooms out of the footer as the stage arrives.
            orb.set_scale(VOICE_STAGE_ORB_SCALE * (0.82 + 0.18 * reveal), cx);
        });
        if reveal <= 0.001 {
            return None;
        }

        // Streamed words veil in like transcript text; a new speaker turn
        // fades the previous caption out (zeron-veil, shared with mobile).
        let caption = self.voice_caption.advance(
            caption_item.as_deref(),
            &partial,
            std::time::Instant::now(),
            self.reduced_motion,
        );
        if self.voice_caption.is_animating() {
            self.motion_active.set(true);
        }

        let theme = Theme::of(cx).clone();
        let viewport = window.viewport_size();
        // The sidebar stays usable beside the stage; collapsed, the stage
        // spans the whole window. Settings always shows its own sidebar.
        let left = if matches!(self.route, Route::Settings(_)) {
            self.settings.sidebar_width
        } else {
            self.sidebar_now()
        };
        let width = (f32::from(viewport.width) - left).max(0.0);
        let ui_settings = settings::current(cx);
        let artwork = ui_settings
            .new_thread_composer_background
            .as_ref()
            .and_then(|background| {
                crate::new_thread_background_effects::prepare(
                    ui_settings.new_thread_background_effect,
                    &theme,
                    std::path::Path::new(&background.path),
                    cx,
                )
            });
        let adjustment = ui_settings
            .new_thread_composer_background
            .as_ref()
            .map(|background| background.adjustment)
            .unwrap_or_default();
        // The new-thread hero's artwork with only its soft bottom fade: no
        // cutout around the orb, so no light channel splits the picture.
        let hero = artwork.map(|artwork| {
            div()
                .absolute()
                .top_0()
                .left_0()
                .w(px(width))
                .h(px(new_thread_background_height(f32::from(viewport.height))))
                .opacity(new_thread_background_opacity(theme.is_frost()))
                .child(
                    gpui::canvas(
                        |_, _, _| {},
                        move |bounds, _, window, _| {
                            crate::new_thread_background_mask::paint(
                                artwork.clone(),
                                bounds,
                                bounds,
                                adjustment,
                                false,
                                window,
                            );
                        },
                    )
                    .absolute()
                    .inset_0(),
                )
        });

        let orb_block = div()
            .relative()
            .size(px(STAGE_ORB_BOX))
            .flex()
            .items_center()
            .justify_center()
            .child(self.voice_stage_orb.clone());

        let center = div()
            .relative()
            .top(px(18.0 * (1.0 - reveal)))
            .flex()
            .flex_col()
            .items_center()
            .child(orb_block)
            .child(
                div()
                    .relative()
                    .mt(px(10.0))
                    .w(px(560.0))
                    .max_w(px((width - 48.0).max(200.0)))
                    .h(px(48.0))
                    .overflow_hidden()
                    .text_center()
                    .text_size(crate::typography::ui_rems(14.0))
                    .line_height(px(22.0))
                    .text_color(theme.text_muted)
                    .child(veiled_caption(caption.text, &caption.spans, &theme, window))
                    .children(caption.previous.map(|(previous, opacity)| {
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .right_0()
                            .opacity(opacity)
                            .child(previous)
                    })),
            );

        let controls = self.render_voice_call_bar(&theme, awaiting, chat_id, cx);
        // The stage occludes the conversation titlebar. Restore its native
        // drag and double-click behavior above the stage artwork and controls.
        let titlebar = self.titlebar_drag_region(
            "voice-stage-titlebar",
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .h(px(Theme::TITLEBAR_HEIGHT)),
            cx,
        );

        Some(
            div()
                .id("voice-stage")
                .debug_selector(|| "voice-stage".into())
                .track_focus(&self.voice_stage_focus)
                .absolute()
                .top_0()
                .bottom_0()
                .right_0()
                .left(px(left))
                .occlude()
                .opacity(reveal)
                // On glass the stage shows the window frost like the new-thread
                // page; the page beneath fades out instead (see `render`).
                .when(!theme.is_glass(), |stage| stage.bg(theme.bg))
                .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                    let key = event.keystroke.key.as_str();
                    let plain = !event.keystroke.modifiers.modified();
                    if key == "escape" {
                        this.set_voice_stage_open(false, cx);
                        cx.stop_propagation();
                    } else if key == "m" && plain {
                        this.voice.update(cx, |voice, cx| voice.toggle_mute(cx));
                        cx.stop_propagation();
                    }
                }))
                .child(div().absolute().inset_0().children(hero))
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .pb(px(64.0))
                        .child(center),
                )
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom(px(32.0 - 16.0 * (1.0 - reveal)))
                        .flex()
                        .justify_center()
                        .child(controls),
                )
                .child(titlebar)
                .into_any_element(),
        )
    }

    /// Video-call style bar: session time, the call controls grouped around
    /// a red hang-up, and the way back to the chats.
    fn render_voice_call_bar(
        &mut self,
        theme: &Theme,
        awaiting: bool,
        chat_id: Option<String>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let popup = theme.for_popup();
        let voice = self.voice.read(cx);
        let muted = voice.muted();
        let elapsed = voice
            .active_since
            .map(|since| crate::voice::format_elapsed(since.elapsed().as_secs()));
        let live_tone = if awaiting {
            popup.warning
        } else if muted {
            popup.text_faint
        } else {
            popup.success
        };

        // Session clock, like a meeting's running time.
        let clock = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .pl(px(10.0))
            .pr(px(6.0))
            .child(div().size(px(7.0)).rounded_full().bg(live_tone))
            .child(
                div()
                    .min_w(px(40.0))
                    .font_family(popup.font_mono.clone())
                    .text_size(crate::typography::ui_rems(13.0))
                    .text_color(popup.text_muted)
                    .child(SharedString::from(elapsed.unwrap_or_else(|| "–:––".into()))),
            );

        let clock = clock.children(voice.host_name.clone().map(|name| {
            div()
                .text_size(crate::typography::ui_rems(12.0))
                .text_color(popup.text_faint)
                .child(name)
        }));

        // Microphone: inverted plate while muted.
        let mic = round_control("voice-bar-mic", &popup, muted)
            .aria_label(if muted {
                "Unmute microphone"
            } else {
                "Mute microphone"
            })
            .tooltip(stage_tooltip(if muted {
                "Unmute · M"
            } else {
                "Mute · M"
            }))
            .on_click(cx.listener(|this, _, _, cx| {
                this.voice.update(cx, |voice, cx| voice.toggle_mute(cx))
            }))
            .child(
                icon(if muted {
                    icons::MICROPHONE_OFF
                } else {
                    icons::MICROPHONE
                })
                .size(px(19.0))
                .text_color(if muted { popup.on_solid } else { popup.text }),
            );

        let transcript = chat_id.map(|chat_id| {
            round_control("voice-bar-transcript", &popup, false)
                .aria_label(if awaiting {
                    "Answer Codex in the transcript"
                } else {
                    "Open the voice transcript"
                })
                .tooltip(stage_tooltip(if awaiting {
                    "Codex is asking you something"
                } else {
                    "Transcript"
                }))
                .on_click(cx.listener(move |this, _, _, cx| this.open_chat(chat_id.clone(), cx)))
                .child(
                    icon(icons::CHAT_ROUND_LINE)
                        .size(px(19.0))
                        .text_color(if awaiting { popup.warning } else { popup.text }),
                )
                .when(awaiting, |el| {
                    el.child(
                        div()
                            .absolute()
                            .top(px(9.0))
                            .right(px(9.0))
                            .size(px(8.0))
                            .rounded_full()
                            .border_2()
                            .border_color(popover::surface_bg(&popup))
                            .bg(popup.warning),
                    )
                })
        });

        // The hang-up: the one saturated control, wide enough to never be
        // mistaken for its neighbours.
        let end = div()
            .id("voice-bar-end")
            .role(gpui::Role::Button)
            .aria_label("End voice")
            .tab_index(0)
            .h(px(BAR_CONTROL))
            .px(px(20.0))
            .rounded_full()
            .flex()
            .items_center()
            .gap(px(8.0))
            .cursor_pointer()
            .bg(motion::hover_blend(
                "voice-bar-end",
                popup.danger,
                popup.danger.blend(gpui::black().opacity(0.14)),
            ))
            .on_hover(motion::hover_listener("voice-bar-end"))
            .focus_visible(|s| s.border_2().border_color(popup.accent))
            .tooltip(stage_tooltip("End voice"))
            .on_click(cx.listener(|this, _, _, cx| this.end_voice(cx)))
            .child(
                icon(icons::PHONE_HANG_UP)
                    .size(px(19.0))
                    .text_color(gpui::white()),
            )
            .child(
                div()
                    .text_size(crate::typography::ui_rems(13.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(gpui::white())
                    .child(SharedString::from("End")),
            );

        let back = round_control("voice-bar-back", &popup, false)
            .aria_label("Back to chats")
            .tooltip(stage_tooltip("Back to chats · Esc"))
            .on_click(cx.listener(|this, _, _, cx| this.set_voice_stage_open(false, cx)))
            .child(
                icon(icons::ALT_ARROW_DOWN)
                    .size(px(19.0))
                    .text_color(popup.text_muted),
            );

        let divider = || div().w(px(1.0)).h(px(24.0)).mx(px(4.0)).bg(popup.border);
        let radius = BAR_CONTROL / 2.0 + BAR_PAD;
        let bar = div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .p(px(BAR_PAD))
            .rounded(px(radius))
            .border_1()
            .border_color(popup.border.opacity(0.7))
            .bg(popover::surface_bg(&popup))
            .when(!popup.is_frost(), |el| el.shadow_lg())
            .text_color(popup.text)
            .child(clock)
            .child(divider())
            .child(mic)
            .children(transcript)
            .child(end)
            .child(divider())
            .child(back);
        crate::frost::frosted(radius, 18.0, bar).into_any_element()
    }
}

/// The caption with its fading words recolored, paint-only (`apply_veil`).
fn veiled_caption(
    text: String,
    spans: &[zeron_veil::VeilSpan],
    theme: &Theme,
    window: &Window,
) -> gpui::StyledText {
    let mut run = window.text_style().to_run(text.len());
    run.color = theme.text_muted;
    let runs = crate::markdown::veil::apply_veil(vec![run], spans);
    gpui::StyledText::new(text).with_runs(runs)
}

fn stage_tooltip(text: &'static str) -> impl Fn(&mut Window, &mut App) -> gpui::AnyView + 'static {
    move |_, cx| cx.new(|_| SurfaceTabTooltip { text: text.into() }).into()
}

/// A round call-bar control; `active` inverts it onto the solid plate.
fn round_control(id: &'static str, theme: &Theme, active: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .role(gpui::Role::Button)
        .tab_index(0)
        .relative()
        .size(px(BAR_CONTROL))
        .flex_none()
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .text_color(if active { theme.on_solid } else { theme.text })
        .bg(if active {
            theme.solid
        } else {
            motion::hover_blend(id, theme.glass_hover().opacity(0.5), theme.glass_hover())
        })
        .on_hover(motion::hover_listener(id))
        .focus_visible(|s| s.border_2().border_color(theme.accent))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn voice_stage_topbar_accepts_window_drag_presses(cx: &mut gpui::TestAppContext) {
        struct StageHost(Entity<Shell>);
        impl Render for StageHost {
            fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                self.0.update(cx, |shell, cx| {
                    div()
                        .size_full()
                        .relative()
                        .children(shell.render_voice_stage(window, cx))
                })
            }
        }

        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
            settings::init(settings::UiSettings::default(), dir.path(), cx);
        });
        let (host, cx) = cx.add_window_view(|_, cx| {
            StageHost(cx.new(|cx| {
                let state = cx.new(|_| AppState::new());
                let mut shell = Shell::new(
                    state,
                    EngineBootConfig {
                        data_dir: dir.path().into(),
                        ipc_port: 0,
                        edge_url: "http://127.0.0.1:1".into(),
                        edge_token: None,
                        org_id: None,
                        workos_client_id: None,
                        default_harness: HarnessId::Mock,
                    },
                    cx,
                );
                shell.reduced_motion = true;
                shell.voice.update(cx, |voice, _| {
                    voice.phase = VoicePhase::Active;
                    voice.stage_open = true;
                });
                shell
            }))
        });
        let shell = host.read_with(cx, |host, _| host.0.clone());
        cx.update(|window, cx| window.draw(cx).clear());
        let bounds = cx.debug_bounds("voice-stage").unwrap();
        let topbar = gpui::point(
            bounds.center().x,
            bounds.top() + px(Theme::TITLEBAR_HEIGHT / 2.0),
        );
        cx.simulate_mouse_down(topbar, MouseButton::Left, gpui::Modifiers::default());
        shell.read_with(cx, |shell, _| assert!(shell.titlebar_should_move));
        cx.simulate_mouse_up(topbar, MouseButton::Left, gpui::Modifiers::default());
        shell.read_with(cx, |shell, _| assert!(!shell.titlebar_should_move));

        let body = gpui::point(
            bounds.center().x,
            bounds.top() + px(Theme::TITLEBAR_HEIGHT + 32.0),
        );
        cx.simulate_mouse_down(body, MouseButton::Left, gpui::Modifiers::default());
        shell.read_with(cx, |shell, _| assert!(!shell.titlebar_should_move));
        cx.simulate_mouse_up(body, MouseButton::Left, gpui::Modifiers::default());
    }

    #[gpui::test]
    fn voice_stage_can_start_and_reopen_in_settings_until_navigation(
        cx: &mut gpui::TestAppContext,
    ) {
        struct StageHost(Entity<Shell>);
        impl Render for StageHost {
            fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                self.0.update(cx, |shell, cx| {
                    shell.sync_voice_stage(window, cx);
                    div()
                        .size_full()
                        .children(shell.render_voice_stage(window, cx))
                })
            }
        }

        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
            settings::init(settings::UiSettings::default(), dir.path(), cx);
        });
        let (host, cx) = cx.add_window_view(|_, cx| {
            StageHost(cx.new(|cx| {
                let state = cx.new(|_| AppState::new());
                let mut shell = Shell::new(
                    state,
                    EngineBootConfig {
                        data_dir: dir.path().into(),
                        ipc_port: 0,
                        edge_url: "http://127.0.0.1:1".into(),
                        edge_token: None,
                        org_id: None,
                        workos_client_id: None,
                        default_harness: HarnessId::Mock,
                    },
                    cx,
                );
                shell.reduced_motion = true;
                shell.route = Route::Settings(SettingsSection::Voice);
                // The initial stage opens while native voice is still preparing.
                shell.voice.update(cx, |voice, cx| {
                    voice.phase = VoicePhase::Checking;
                    voice.set_stage_open(true, cx);
                });
                shell
            }))
        });
        let shell = host.read_with(cx, |host, _| host.0.clone());
        let stays_open = |cx: &mut gpui::VisualTestContext| {
            for _ in 0..3 {
                cx.update(|window, cx| window.draw(cx).clear());
                shell.read_with(cx, |shell, cx| assert!(shell.voice.read(cx).stage_open));
                assert!(cx.debug_bounds("voice-stage").is_some());
            }
        };
        stays_open(cx);

        shell.update(cx, |shell, cx| {
            shell
                .voice
                .update(cx, |voice, _| voice.phase = VoicePhase::Active);
            shell.set_voice_stage_open(false, cx);
        });
        cx.update(|window, cx| window.draw(cx).clear());
        shell.update(cx, |shell, cx| shell.set_voice_stage_open(true, cx));
        stays_open(cx);

        // Choosing another settings page hides the stage without ending voice.
        shell.update(cx, |shell, cx| {
            shell.open_settings(SettingsSection::General, cx)
        });
        cx.update(|window, cx| window.draw(cx).clear());
        shell.read_with(cx, |shell, cx| {
            assert!(!shell.voice.read(cx).stage_open);
            assert!(shell.voice.read(cx).is_live());
        });
        shell.update(cx, |shell, cx| shell.set_voice_stage_open(true, cx));
        stays_open(cx);

        // Leaving and reentering the same settings page is navigation too.
        shell.update(cx, |shell, _| shell.route = Route::Chat);
        cx.update(|window, cx| window.draw(cx).clear());
        shell.update(cx, |shell, cx| {
            shell.open_settings(SettingsSection::General, cx)
        });
        cx.update(|window, cx| window.draw(cx).clear());
        shell.read_with(cx, |shell, cx| {
            assert!(!shell.voice.read(cx).stage_open);
            assert!(shell.voice.read(cx).is_live());
        });

        shell.update(cx, |shell, cx| shell.set_voice_stage_open(true, cx));
        stays_open(cx);
        shell.update(cx, |shell, cx| {
            shell.state.update(cx, |state, _| {
                state.selected_chat = Some("another-chat".into())
            });
        });
        cx.update(|window, cx| window.draw(cx).clear());
        shell.read_with(cx, |shell, cx| {
            assert!(!shell.voice.read(cx).stage_open);
            assert!(shell.voice.read(cx).is_live());
        });
    }

    #[gpui::test]
    fn opening_any_session_steps_the_stage_aside_without_ending_voice(
        cx: &mut gpui::TestAppContext,
    ) {
        struct StageHost(Entity<Shell>);
        impl Render for StageHost {
            fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                self.0.update(cx, |shell, cx| {
                    shell.sync_voice_stage(window, cx);
                    div()
                        .size_full()
                        .children(shell.render_voice_stage(window, cx))
                })
            }
        }

        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
            settings::init(settings::UiSettings::default(), dir.path(), cx);
        });
        let (host, cx) = cx.add_window_view(|_, cx| {
            StageHost(cx.new(|cx| {
                let state = cx.new(|_| AppState::new());
                let mut shell = Shell::new(
                    state,
                    EngineBootConfig {
                        data_dir: dir.path().into(),
                        ipc_port: 0,
                        edge_url: "http://127.0.0.1:1".into(),
                        edge_token: None,
                        org_id: None,
                        workos_client_id: None,
                        default_harness: HarnessId::Mock,
                    },
                    cx,
                );
                shell.reduced_motion = true;
                shell
                    .state
                    .update(cx, |state, _| state.selected_chat = Some("current".into()));
                shell.voice.update(cx, |voice, cx| {
                    voice.phase = VoicePhase::Active;
                    voice.set_stage_open(true, cx);
                });
                shell
            }))
        });
        let shell = host.read_with(cx, |host, _| host.0.clone());
        let open_stage = |cx: &mut gpui::VisualTestContext| {
            shell.update(cx, |shell, cx| shell.set_voice_stage_open(true, cx));
            cx.update(|window, cx| window.draw(cx).clear());
            shell.read_with(cx, |shell, cx| assert!(shell.voice.read(cx).stage_open));
        };
        let stepped_aside = |cx: &mut gpui::VisualTestContext, selected: &str| {
            cx.update(|window, cx| window.draw(cx).clear());
            shell.read_with(cx, |shell, cx| {
                assert!(!shell.voice.read(cx).stage_open);
                assert!(shell.voice.read(cx).is_live(), "the call keeps running");
                assert_eq!(shell.route, Route::Chat);
                assert_eq!(
                    shell.state.read(cx).selected_chat.as_deref(),
                    Some(selected)
                );
            });
        };
        cx.update(|window, cx| window.draw(cx).clear());

        // The session already under the stage: the selection doesn't change,
        // so only an explicit step-aside shows it.
        shell.update(cx, |shell, cx| shell.open_chat("current".into(), cx));
        stepped_aside(cx, "current");

        // From Settings back to the session the stage was opened over.
        shell.update(cx, |shell, _| {
            shell.route = Route::Settings(SettingsSection::Voice)
        });
        open_stage(cx);
        shell.update(cx, |shell, cx| shell.open_chat("current".into(), cx));
        stepped_aside(cx, "current");

        // Another session.
        open_stage(cx);
        shell.update(cx, |shell, cx| shell.open_chat("other".into(), cx));
        stepped_aside(cx, "other");
    }

    #[gpui::test]
    fn composer_orb_shows_only_on_the_orchestrator_chat_and_returns_to_the_call(
        cx: &mut gpui::TestAppContext,
    ) {
        struct OrbHost(Entity<Shell>);
        impl Render for OrbHost {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                self.0.update(cx, |shell, cx| {
                    div()
                        .size_full()
                        .children(shell.render_voice_composer_orb(600.0, cx))
                })
            }
        }

        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
            settings::init(settings::UiSettings::default(), dir.path(), cx);
        });
        let orchestrator = format!("{}1", zeron_proto::voice::ORCHESTRATOR_CHAT_PREFIX);
        let (host, cx) = cx.add_window_view(|_, cx| {
            OrbHost(cx.new(|cx| {
                let state = cx.new(|_| AppState::new());
                let shell = Shell::new(
                    state,
                    EngineBootConfig {
                        data_dir: dir.path().into(),
                        ipc_port: 0,
                        edge_url: "http://127.0.0.1:1".into(),
                        edge_token: None,
                        org_id: None,
                        workos_client_id: None,
                        default_harness: HarnessId::Mock,
                    },
                    cx,
                );
                shell.voice.update(cx, |voice, _| {
                    voice.phase = VoicePhase::Active;
                    voice.chat_id = Some(orchestrator.clone());
                });
                shell
            }))
        });
        let shell = host.read_with(cx, |host, _| host.0.clone());
        let select = |chat: &str, cx: &mut gpui::VisualTestContext| {
            let chat = chat.to_owned();
            shell.update(cx, |shell, cx| {
                shell
                    .state
                    .update(cx, |state, _| state.selected_chat = Some(chat));
            });
            cx.update(|window, cx| window.draw(cx).clear());
        };

        select("another-chat", cx);
        assert!(cx.debug_bounds("voice-composer-orb").is_none());

        select(&orchestrator, cx);
        let orb = cx.debug_bounds("voice-composer-orb").unwrap();
        cx.simulate_click(orb.center(), gpui::Modifiers::default());
        shell.read_with(cx, |shell, cx| assert!(shell.voice.read(cx).stage_open));
    }

    #[test]
    fn stage_reveal_eases_in_and_out_and_snaps_for_reduced_motion() {
        assert_eq!(stage_reveal(true, 0.0, false), 0.0);
        assert_eq!(stage_reveal(true, f32::INFINITY, false), 1.0);
        assert!(stage_reveal(true, STAGE_ENTER_MS / 4.0, false) > 0.5);
        assert_eq!(stage_reveal(false, 0.0, false), 1.0);
        assert_eq!(stage_reveal(false, f32::INFINITY, false), 0.0);
        assert_eq!(stage_reveal(true, 0.0, true), 1.0);
        assert_eq!(stage_reveal(false, 0.0, true), 0.0);
    }
}
