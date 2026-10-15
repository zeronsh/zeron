//! The voice orchestrator's chrome: the sidebar footer trigger and the
//! full-window stage. A voice session follows no chat selection, so neither
//! surface lives inside the conversation column — the user moves between
//! threads freely while the orchestrator keeps listening.

use super::*;
use crate::voice::glyph::{self, Grid, Outline};
use zeron_proto::voice::{VoicePhase, VoiceRole, VoiceWork};

/// Small, the glyph is the call's beam in miniature: a strip the width of
/// the icons beside it, its dots fine enough for a voice to read as a wave
/// along it, as the dictation track reads. Its dots, width and height.
const FOOTER_BEAM: (f32, f32, f32) = (1.4, 22.0, 11.0);
/// The stage's field: dots across and their size. It fills its disc, its
/// rim easing into the material, and a voice pushes out from its heart. Its
/// dots are the dictation beam's, about 0.02 in across as the phone's are
/// (`VoiceStageViewController`): one resolution for every voice glyph.
const STAGE_FIELD: (usize, f32) = (44, 3.0);
/// How far above the middle the orb settles, leaving the words room below.
const STAGE_ORB_LIFT: f32 = 120.0;
/// The transcript's column.
const STAGE_TRANSCRIPT_WIDTH: f32 = 560.0;
/// The veil behind the orb: how much of the page's colour it lays over the
/// picture, then how far it holds past the orb and how long its blur runs,
/// as shares of the orb's width.
const STAGE_ORB_VEIL: (f32, f32, f32) = (0.97, 0.5, 0.75);
/// How far down the stage the hero artwork reaches, as a share of its
/// height, and at most: it stays behind the orb, clear of the words.
const STAGE_HERO: (f32, f32) = (0.58, 620.0);
/// The tray over the orchestrator's composer: the height its beam shows
/// above the composer, and how far it dissolves into the tray's controls.
const COMPOSER_BEAM: (f32, f32) = (22.0, 20.0);
/// The tray's mute and hang-up.
const TRAY_CONTROL: f32 = 24.0;
const STAGE_ENTER_MS: f32 = 620.0;
const STAGE_EXIT_MS: f32 = 300.0;
/// The newest turn veils in whole, as the phone's does.
pub(super) const STAGE_CAPTION_CHARS: usize = 4000;
const FOOTER_HOVER: &str = "voice-footer-orb";
/// Call-bar geometry: round controls inside a floating pill.
const BAR_CONTROL: f32 = 36.0;
const BAR_PAD: f32 = 6.0;

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
    /// Hears the call into its glyph, once a frame however many places draw
    /// it, and keeps frames coming while it moves. Under reduced motion the
    /// glyph holds still.
    fn voice_glyph(&mut self, cx: &mut Context<Self>) -> (glyph::Mode, glyph::Inks) {
        let voice = self.voice.read(cx);
        let (heard, mode, live) = (voice.heard(), voice.glyph_mode(), voice.is_live());
        if live && !self.reduced_motion {
            let now = std::time::Instant::now();
            self.voice_field.hear(heard, now);
            self.voice_stage_field.hear(heard, now);
            self.voice_beam.hear(heard, now);
            // A change of state passes from one pattern to the next.
            self.voice_field.shift(mode);
            self.voice_stage_field.shift(mode);
            self.voice_beam.shift(mode);
            if self.voice_field.moving(mode) {
                self.motion_active.set(true);
            }
        }
        (mode, glyph::inks(Theme::of(cx)))
    }

    /// The call's beam in miniature: the same beam the tray and the track
    /// show, in a pill the size of an icon.
    fn voice_mini(&self, mode: glyph::Mode, inks: glyph::Inks) -> AnyElement {
        let (dot, width, height) = FOOTER_BEAM;
        glyph::beam(
            &self.voice_beam,
            Grid::dense(dot),
            mode,
            inks,
            Outline::Stadium,
        )
        .w(px(width))
        .h(px(height))
        .flex_none()
        .into_any_element()
    }

    /// The pointer's touch on the stage's orb, from the orb's corner, as it
    /// stands now: its light holds, then fades, after the pointer moves on.
    /// Frames keep coming while it fades.
    fn voice_orb_touch(&self) -> Option<glyph::Touch> {
        let (x, y, at) = self.voice_touch?;
        let strength = glyph::Touch::strength_after(at.elapsed().as_secs_f32());
        if strength <= 0.0 {
            return None;
        }
        self.motion_active.set(true);
        Some(glyph::Touch { x, y, strength })
    }

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
        let (mode, inks) = self.voice_glyph(cx);

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
                .child(self.voice_mini(mode, inks))
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
                icon(icons::VOICE_WAVE)
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

    /// The live call above the composer of every chat while it runs, so the
    /// call is never out of reach; pressing the orb returns to the stage.
    pub(super) fn render_voice_composer_orb(
        &mut self,
        composer_width: f32,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let voice = self.voice.read(cx);
        // Every open conversation, never the new-thread canvas: its
        // selectors sit where the strip would.
        let shown = matches!(self.route, Route::Chat)
            && voice.is_live()
            && self.state.read(cx).selected_chat.is_some();
        if !shown {
            return None;
        }
        // A tray growing out of the composer, as the queue's does: tucked
        // under its top edge and as wide as its straight top, ending where
        // its corners begin to round. The call's beam rises in it; pressing
        // the beam returns to the stage. Mute and hang-up sit at its end.
        let (mode, inks) = self.voice_glyph(cx);
        let (height, fade) = COMPOSER_BEAM;
        let theme = Theme::of(cx).clone();
        let muted = self.voice.read(cx).muted();
        let overlap = crate::composer::QUEUE_COMPOSER_OVERLAP;
        let pill = composer_width - 2.0 * Theme::SPACE_LG;
        let width = (pill - 2.0 * crate::composer::COMPOSER_RADIUS).max(0.0);
        let small = |id: &'static str, active: bool| {
            round_control(id, &theme, active).size(px(TRAY_CONTROL))
        };
        let mic = small("voice-tray-mic", muted)
            .aria_label(if muted {
                "Unmute microphone"
            } else {
                "Mute microphone"
            })
            .tooltip(stage_tooltip(if muted { "Unmute" } else { "Mute" }))
            .on_click(cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.voice.update(cx, |voice, cx| voice.toggle_mute(cx))
            }))
            .child(
                icon(if muted {
                    icons::MICROPHONE_OFF
                } else {
                    icons::MICROPHONE
                })
                .size(px(13.0))
                .text_color(if muted { theme.on_solid } else { theme.text }),
            );
        let end = small("voice-tray-end", false)
            .aria_label("End voice")
            .tooltip(stage_tooltip("End voice"))
            .bg(motion::hover_blend(
                "voice-tray-end",
                theme.danger,
                theme.danger.blend(gpui::black().opacity(0.14)),
            ))
            .on_click(cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.end_voice(cx)
            }))
            .child(
                icon(icons::PHONE_HANG_UP)
                    .size(px(13.0))
                    .text_color(gpui::white()),
            );
        let beam = div()
            .id("voice-composer-orb")
            .debug_selector(|| "voice-composer-orb".into())
            .role(gpui::Role::Button)
            .aria_label("Open voice")
            .tab_index(0)
            .flex_1()
            .min_w_0()
            .h_full()
            .cursor_pointer()
            .tooltip(stage_tooltip("Open voice"))
            .on_click(cx.listener(|this, _, _, cx| this.set_voice_stage_open(true, cx)))
            // From the tray's padding, as its controls sit at the other end;
            // it only dissolves where it meets them.
            .child(
                crate::edge_fade::edge_faded(
                    fade,
                    false,
                    false,
                    glyph::beam(
                        &self.voice_beam,
                        Grid::dense(glyph::BEAM_DOT),
                        mode,
                        inks,
                        // No frame to follow: the beam's own dots thin out
                        // at its ends, as the orb's do at its rim.
                        glyph::Outline::Square,
                    )
                    .size_full(),
                )
                .fade_right(true),
            );
        // The queue's own surface, tucked under the composer's top edge: the
        // call sits in the conversation as a queued message does.
        let tray = crate::queue::queue_panel_surface(&theme, false)
            .w(px(width))
            .mx_auto()
            .mb(px(-overlap))
            .child(
                div()
                    .h(px(height))
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .child(beam)
                    .child(mic)
                    .child(end),
            );
        Some(motion::fade_quick("voice-composer-orb-in", tray).into_any_element())
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
        let reveal = self.voice_stage_reveal(cx);
        if reveal > 0.0 && reveal < 1.0 {
            self.motion_active.set(true);
        }
        let (mode, inks) = self.voice_glyph(cx);
        let voice = self.voice.read(cx);
        // The call's recent turns, oldest first, each with its speaker once
        // known: the transcript reads as the conversation does.
        let lines: Vec<(Option<String>, String, Option<VoiceRole>)> = voice
            .caption
            .lines()
            .filter(|line| !line.text.trim().is_empty())
            .map(|line| {
                (
                    line.item.map(str::to_owned),
                    line.text.trim().to_owned(),
                    line.speaker,
                )
            })
            .collect();
        let snapshot = voice.snapshot.clone();
        let chat_id = voice.chat_id.clone();
        let awaiting = snapshot
            .as_ref()
            .is_some_and(|s| s.work == VoiceWork::AwaitingInput);
        if reveal <= 0.001 {
            return None;
        }

        // The newest turn's words veil in like transcript text (zeron-veil,
        // shared with mobile); every turn keeps one size throughout.
        let newest = lines.last().map(|(item, text, _)| {
            self.voice_caption.advance(
                item.as_deref(),
                text,
                std::time::Instant::now(),
                self.reduced_motion,
            )
        });
        if self.voice_caption.is_animating() {
            self.motion_active.set(true);
        }

        let theme = Theme::of(cx).clone();
        let viewport = window.viewport_size();
        let height = f32::from(viewport.height);
        // The sidebar stays usable beside the stage; collapsed, the stage
        // spans the whole window. Settings always shows its own sidebar.
        let left = if matches!(self.route, Route::Settings(_)) {
            self.settings.sidebar_width
        } else {
            self.sidebar_now()
        };
        let width = (f32::from(viewport.width) - left).max(0.0);

        // The orb arrives once, settling above the middle with the words'
        // room below it: it grows the last bit of the way as the stage
        // comes, and never moves after.
        let (dots, dot) = STAGE_FIELD;
        let grid = Grid::dense(dot * (0.84 + 0.16 * reveal));
        let (room, _) = grid.extent(dots, dots);
        let orb_top = (height / 2.0 - STAGE_ORB_LIFT - room / 2.0)
            .max(Theme::TITLEBAR_HEIGHT + 20.0)
            + 28.0 * (1.0 - reveal);
        let orb_left = (width - room) / 2.0;
        let bar_top = 28.0 + BAR_CONTROL + 2.0 * BAR_PAD;
        // The words live in the band between the orb and the call bar,
        // centred in it while they are few and filling it once they are
        // many, so the space above and below them always balances.
        let transcript_bottom = bar_top + 20.0;
        let transcript_height = (height - transcript_bottom - (orb_top + room + 24.0)).max(0.0);

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
        // A round veil of the page's own colour behind the orb, its edge a
        // long blur: the picture recedes evenly all around it, whatever the
        // picture holds there, so the orb always stands clear of it.
        let (veil, veil_spread, veil_blur) = STAGE_ORB_VEIL;
        let halo = div()
            .absolute()
            .top(px(orb_top))
            .left(px(orb_left))
            .size(px(room))
            .rounded_full()
            .shadow(vec![gpui::BoxShadow {
                // The page's colour at the veil's own strength: on frost
                // the page is see-through, so its alpha is set, not scaled.
                color: gpui::Hsla {
                    a: veil,
                    ..theme.bg
                },
                offset: gpui::point(px(0.0), px(0.0)),
                blur_radius: px(room * veil_blur),
                spread_radius: px(room * veil_spread),
                inset: false,
            }]);

        // The new-thread hero's artwork with only its soft bottom fade: no
        // cutout around the orb, so no light channel splits the picture.
        let hero = artwork.map(|artwork| {
            div()
                .absolute()
                .top_0()
                .left_0()
                .w(px(width))
                .h(px((height * STAGE_HERO.0).min(STAGE_HERO.1)))
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
        // The orb: the composer's own material (its frost, tint and rim) in
        // a disc, and the field inside it running to its edge. Its dots and
        // their light never leave the disc.
        let disc = div()
            .size(px(room))
            .rounded_full()
            .border_1()
            .border_color(theme.composer_surface_border())
            .bg(theme.composer_surface_bg())
            .when(!theme.is_frost(), |el| el.shadow_lg())
            .flex()
            .items_center()
            .justify_center()
            .child(glyph::field(
                &self.voice_stage_field,
                dots,
                dots,
                grid,
                mode,
                &inks,
                Outline::Disc,
                self.voice_orb_touch(),
            ));
        // The orb's place in the window, for the pointer over it.
        let orb_at = (left + orb_left, orb_top);
        let orb = div()
            .id("voice-stage-orb")
            .absolute()
            .top(px(orb_top))
            .left(px(orb_left))
            .size(px(room))
            // The pointer over the orb lights the dots under it; the light
            // stays a moment where it left them.
            .on_mouse_move(
                cx.listener(move |this, event: &gpui::MouseMoveEvent, _, cx| {
                    let (x, y) = (
                        f32::from(event.position.x) - orb_at.0,
                        f32::from(event.position.y) - orb_at.1,
                    );
                    let inside = (x - room / 2.0).hypot(y - room / 2.0) <= room / 2.0;
                    if inside && !this.reduced_motion {
                        this.voice_touch = Some((x, y, std::time::Instant::now()));
                        cx.notify();
                    }
                }),
            )
            // Painted plain, never frosted: frost can neither fade nor
            // scale, and a disc that pops into blur once it has arrived is
            // worse than one that keeps the page's own surface throughout.
            .child(disc.opacity(reveal));

        // The transcript, as the conversation reads: the user's turns in
        // their bubble on the right, Codex's (or one not yet attributed) on
        // the left in the page's text. A short exchange sits on the call bar;
        // a longer one fades out under the top edge.
        let column = STAGE_TRANSCRIPT_WIDTH.min(width - 48.0).max(200.0);
        let count = lines.len();
        let newest_key = lines
            .last()
            .and_then(|(item, _, _)| item.clone())
            .unwrap_or_else(|| format!("turns-{count}"));
        // New words bring the newest turn into view; until then a reader
        // scrolled back stays where they are.
        let said: usize = lines.iter().map(|(_, text, _)| text.len()).sum();
        if self.voice_transcript_seen != (newest_key.clone(), said) {
            self.voice_transcript_seen = (newest_key.clone(), said);
            self.voice_transcript_scroll.scroll_to_bottom();
        }
        let scrolled = f32::from(self.voice_transcript_scroll.offset().y) < -1.0;
        let mut previous: Option<Option<VoiceRole>> = None;
        let rows: Vec<AnyElement> = lines
            .into_iter()
            .enumerate()
            .map(|(ix, (item, text, speaker))| {
                let user = speaker == Some(VoiceRole::User);
                let gap = match previous {
                    None => 0.0,
                    Some(before) if (before == Some(VoiceRole::User)) == user => 12.0,
                    Some(_) => 20.0,
                };
                previous = Some(speaker);
                let words: AnyElement = match (&newest, ix + 1 == count) {
                    (Some(veil), true) => {
                        veiled_caption(veil.text.clone(), &veil.spans, &theme, window)
                            .into_any_element()
                    }
                    _ => SharedString::from(text).into_any_element(),
                };
                let body = div()
                    .min_w_0()
                    .text_size(crate::typography::ui_rems(16.0))
                    .line_height(px(24.0))
                    .text_color(theme.text)
                    .child(words);
                // Codex's turns read on the page, yours in your bubble: two
                // voices told apart by their place, with no glyph but the orb.
                let row = div()
                    .w_full()
                    .flex()
                    .items_start()
                    .mt(px(gap))
                    .when(user, |row| row.justify_end())
                    .child(if user {
                        body.max_w(px(column * 0.8))
                            .bg(crate::theme::user_bubble_bg())
                            .rounded(px(Theme::BUBBLE_RADIUS))
                            .px(px(16.0))
                            .py(px(10.0))
                    } else {
                        body.max_w(px(column * 0.86))
                    });
                // Each turn arrives as the conversation's rows do.
                let id = item.unwrap_or_else(|| format!("turn-{ix}"));
                motion::fade_in(format!("voice-turn-{id}"), row).into_any_element()
            })
            .collect();
        // The words sit centred in the band while they fit, the block
        // re-centring as turns come (keyed by the newest, it settles into
        // place with the rows' own entrance). Once they outgrow the band
        // they fill it and scroll, the newest in view and the rest a scroll
        // away, fading out under the top edge.
        let transcript = div()
            .absolute()
            .left(px((width - column) / 2.0))
            .w(px(column))
            .bottom(px(transcript_bottom))
            .h(px(transcript_height))
            .flex()
            .flex_col()
            .justify_center()
            .child(motion::fade_in(
                format!("voice-stage-transcript-{newest_key}"),
                div()
                    .w_full()
                    .max_h(px(transcript_height))
                    .child(crate::edge_fade::edge_faded(
                        32.0,
                        scrolled,
                        false,
                        div()
                            .id("voice-stage-transcript")
                            .w_full()
                            .max_h(px(transcript_height))
                            .overflow_y_scroll()
                            .track_scroll(&self.voice_transcript_scroll)
                            .flex()
                            .flex_col()
                            .children(rows),
                    )),
            ));
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
                .child(halo)
                .child(orb)
                .child(transcript)
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom(px(28.0 - 12.0 * (1.0 - reveal)))
                        .flex()
                        .justify_center()
                        .child(controls),
                )
                .child(titlebar)
                .into_any_element(),
        )
    }

    /// The call's controls in a floating pill, led by its glyph: the session
    /// clock, the microphone, the transcript, a red hang-up and the way back.
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

        // Who the call is with and for how long: the orb above is the
        // call's one glyph here.
        let session = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(
                div()
                    .font_family(popup.font_mono.clone())
                    .text_size(crate::typography::ui_rems(12.0))
                    .text_color(popup.text_muted)
                    .child(SharedString::from(elapsed.unwrap_or_else(|| "–:––".into()))),
            )
            .children(voice.host_name.clone().map(|name| {
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
                .size(px(16.0))
                .text_color(if muted { popup.on_solid } else { popup.text }),
            );

        // The hang-up: the one saturated control.
        let end = div()
            .id("voice-bar-end")
            .role(gpui::Role::Button)
            .aria_label("End voice")
            .tab_index(0)
            .h(px(BAR_CONTROL))
            .px(px(14.0))
            .rounded_full()
            .flex()
            .items_center()
            .gap(px(6.0))
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
                    .size(px(16.0))
                    .text_color(gpui::white()),
            )
            .child(
                div()
                    .text_size(crate::typography::ui_rems(12.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(gpui::white())
                    .child(SharedString::from("End")),
            );

        // The way back: to the call's own conversation, where Codex's
        // question (if it asked one) waits; without one, to the chats.
        let back = round_control("voice-bar-back", &popup, false)
            .aria_label(if awaiting {
                "Answer Codex in the conversation"
            } else {
                "Back to the conversation"
            })
            .tooltip(stage_tooltip(if awaiting {
                "Codex is asking you something"
            } else {
                "Back to the conversation · Esc"
            }))
            .on_click(cx.listener(move |this, _, _, cx| match chat_id.clone() {
                Some(chat_id) => this.open_chat(chat_id, cx),
                None => this.set_voice_stage_open(false, cx),
            }))
            .child(
                icon(icons::ALT_ARROW_DOWN)
                    .size(px(16.0))
                    .text_color(if awaiting {
                        popup.warning
                    } else {
                        popup.text_muted
                    }),
            )
            .when(awaiting, |el| {
                el.child(
                    div()
                        .absolute()
                        .top(px(6.0))
                        .right(px(6.0))
                        .size(px(7.0))
                        .rounded_full()
                        .border_2()
                        .border_color(popover::surface_bg(&popup))
                        .bg(popup.warning),
                )
            });

        // Concentric: the bar's radius is its controls' plus its padding.
        let radius = BAR_CONTROL / 2.0 + BAR_PAD;
        let bar = div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .p(px(BAR_PAD))
            .pl(px(BAR_PAD + 4.0))
            .rounded(px(radius))
            .border_1()
            .border_color(popup.border.opacity(0.7))
            .bg(popover::surface_bg(&popup))
            .when(!popup.is_frost(), |el| el.shadow_lg())
            .text_color(popup.text)
            .child(session.mr(px(10.0)))
            .child(mic)
            .child(back)
            .child(end);
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
    run.color = theme.text;
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
    fn composer_orb_shows_on_every_chat_and_returns_to_the_call(cx: &mut gpui::TestAppContext) {
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

        // The call stays in reach from every chat, not only the orchestrator's.
        select("another-chat", cx);
        assert!(cx.debug_bounds("voice-composer-orb").is_some());

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
