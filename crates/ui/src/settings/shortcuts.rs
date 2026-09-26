//! Settings → Shortcuts (feature-inventory §1.4): a table of the rebindable
//! bindings — click a combo to record (Esc cancels), live conflict detection,
//! per-row Reset and Restore defaults. Changes emit [`ShortcutsEvent`]; the
//! shell persists them and re-applies the app keymap.

use gpui::{
    Context, Entity, EventEmitter, FocusHandle, Keystroke, SharedString, Window, div, prelude::*,
    px,
};

use crate::appshots::{AppshotCapabilities, AppshotDestination};
use crate::popover::{self, ScrollRailHost};

#[path = "appshots.rs"]
mod appshots_page;
use crate::settings::widgets;
use crate::settings::{
    ComposerSendBehavior, KeymapConfig, ShortcutId, combo_from_keystroke, display_combo,
};
use crate::state::AppState;
use crate::theme::Theme;

/// Outcome of one keystroke while recording. Pure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordOutcome {
    /// Esc — abandon recording, keep the old combo.
    Cancelled,
    /// A bare modifier (or unusable key) — stay recording.
    Ignored,
    /// A full combo landed.
    Set(String),
}

pub fn record_key(key: &str, ctrl: bool, alt: bool, shift: bool, cmd: bool) -> RecordOutcome {
    if key.eq_ignore_ascii_case("escape") {
        return RecordOutcome::Cancelled;
    }
    match combo_from_keystroke(ctrl, alt, shift, cmd, key) {
        Some(combo) => RecordOutcome::Set(combo),
        None => RecordOutcome::Ignored,
    }
}

#[derive(Debug, Clone)]
pub enum ShortcutsEvent {
    /// The keymap changed — persist + re-apply.
    KeymapChanged(KeymapConfig),
    /// The Escape fallback changed — persist it locally.
    EscapeStopsActiveAgentChanged(bool),
    /// The composer send behavior changed — persist + re-apply.
    ComposerSendBehaviorChanged(ComposerSendBehavior),
    AppshotsChanged {
        enabled: bool,
        sound_enabled: bool,
        destination: AppshotDestination,
    },
}

pub struct ShortcutsPage {
    appshots_page: bool,
    general_page: bool,
    appshots_focus_pending: bool,
    scroll: crate::settings::widgets::PageScroll,
    /// Working copy (kept in sync with the shell via change events).
    keymap: KeymapConfig,
    escape_stops_active_agent: bool,
    composer_send_behavior: ComposerSendBehavior,
    recording: Option<ShortcutId>,
    recording_blur: Option<gpui::Subscription>,
    recording_interceptor: Option<gpui::Subscription>,
    /// A rejected record attempt ("{Combo} is already assigned to {label}.") —
    /// conflicts never persist; they're refused at record time, as in zeron.
    conflict_notice: Option<SharedString>,
    focus: FocusHandle,
    appshots_enabled: bool,
    appshot_sound_enabled: bool,
    appshot_destination: AppshotDestination,
    appshot_capabilities: AppshotCapabilities,
    send_select: widgets::SelectState,
    destination_select: widgets::SelectState,
    capture_access_prompted: bool,
    semantic_access_prompted: bool,
    /// Settings → General's thread naming card (its own title-bound picker).
    thread_naming: Entity<crate::settings::thread_naming::ThreadNamingCard>,
    worktrees: Entity<super::worktrees::WorktreeSettingsCard>,
    _state: Entity<AppState>,
}

impl EventEmitter<ShortcutsEvent> for ShortcutsPage {}

impl ShortcutsPage {
    pub fn new(
        state: Entity<AppState>,
        keymap: KeymapConfig,
        escape_stops_active_agent: bool,
        composer_send_behavior: ComposerSendBehavior,
        appshots_enabled: bool,
        appshot_sound_enabled: bool,
        appshot_destination: AppshotDestination,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.on_release(|_, _| crate::appshots::set_recording(false))
            .detach();
        Self {
            appshots_page: false,
            general_page: false,
            appshots_focus_pending: false,
            scroll: crate::settings::widgets::PageScroll::default(),
            keymap,
            escape_stops_active_agent,
            composer_send_behavior,
            recording: None,
            recording_blur: None,
            recording_interceptor: None,
            conflict_notice: None,
            focus: cx.focus_handle(),
            appshots_enabled,
            appshot_sound_enabled,
            appshot_destination,
            appshot_capabilities: crate::appshots::capabilities(),
            send_select: widgets::SelectState::default(),
            destination_select: widgets::SelectState::default(),
            capture_access_prompted: false,
            semantic_access_prompted: false,
            thread_naming: {
                let state = state.clone();
                cx.new(|cx| crate::settings::thread_naming::ThreadNamingCard::new(state, cx))
            },
            worktrees: cx.new(|cx| super::worktrees::WorktreeSettingsCard::new(state.clone(), cx)),
            _state: state,
        }
    }

    pub fn show_appshots(&mut self, appshots: bool) {
        self.show_section(appshots, false);
    }

    pub fn show_section(&mut self, appshots: bool, general: bool) {
        if self.appshots_page != appshots || self.general_page != general {
            self.stop_recording();
            self.conflict_notice = None;
            self.appshots_page = appshots;
            self.general_page = general;
            self.appshots_focus_pending = appshots;
            // One scroll state serves these pages — rewind it so each opens
            // at the top instead of where the other was left.
            self.scroll.reset();
            if appshots {
                self.appshot_capabilities = crate::appshots::capabilities();
            }
        }
    }

    fn start_recording(&mut self, id: ShortcutId, window: &mut Window, cx: &mut Context<Self>) {
        self.recording = Some(id);
        crate::appshots::set_recording(true);
        let page = cx.entity().downgrade();
        // Bound actions run before Div key listeners. Intercept first so a
        // conflicting chord is recorded/refused instead of running its action.
        self.recording_interceptor = Some(cx.intercept_keystrokes(move |event, window, cx| {
            let _ = page.update(cx, |page, cx| {
                if page.focus.is_focused(window) {
                    page.record_keystroke(&event.keystroke, cx);
                }
            });
        }));
        self.recording_blur = Some(cx.on_blur(&self.focus, window, |this, _, cx| {
            this.stop_recording();
            cx.notify();
        }));
        self.conflict_notice = None;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn stop_recording(&mut self) {
        self.recording = None;
        self.recording_blur = None;
        self.recording_interceptor = None;
        crate::appshots::set_recording(false);
    }

    fn commit(&mut self, cx: &mut Context<Self>) {
        cx.emit(ShortcutsEvent::KeymapChanged(self.keymap.clone()));
        cx.notify();
    }

    fn set_escape_stops_active_agent(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.escape_stops_active_agent != enabled {
            self.escape_stops_active_agent = enabled;
            cx.emit(ShortcutsEvent::EscapeStopsActiveAgentChanged(enabled));
            cx.notify();
        }
    }

    fn set_composer_send_behavior(
        &mut self,
        behavior: ComposerSendBehavior,
        cx: &mut Context<Self>,
    ) {
        if self.composer_send_behavior != behavior {
            self.composer_send_behavior = behavior;
            self.conflict_notice = None;
            cx.emit(ShortcutsEvent::ComposerSendBehaviorChanged(behavior));
            cx.notify();
        }
    }

    fn commit_appshots(&self, cx: &mut Context<Self>) {
        cx.emit(ShortcutsEvent::AppshotsChanged {
            enabled: self.appshots_enabled,
            sound_enabled: self.appshot_sound_enabled,
            destination: self.appshot_destination,
        });
    }

    fn record_keystroke(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) {
        let Some(recording) = self.recording else {
            return;
        };
        let mods = &keystroke.modifiers;
        match record_key(
            &keystroke.key,
            mods.control,
            mods.alt,
            mods.shift,
            mods.platform,
        ) {
            RecordOutcome::Cancelled => {
                self.stop_recording();
                cx.notify();
            }
            RecordOutcome::Ignored => {}
            RecordOutcome::Set(combo) => {
                if recording == ShortcutId::CaptureAppshot
                    && crate::appshots::validate_shortcut(&combo).is_err()
                {
                    self.conflict_notice = Some(
                        format!(
                            "Use {} with a letter, number, function key or navigation key.",
                            if cfg!(target_os = "macos") {
                                "Control, Option or Command"
                            } else {
                                "Control or Alt"
                            }
                        )
                        .into(),
                    );
                    self.stop_recording();
                    cx.notify();
                    cx.stop_propagation();
                    return;
                }
                if send_combo_is_reserved(self.composer_send_behavior, &combo) {
                    self.conflict_notice = Some(
                        format!("{} is reserved for the composer.", display_combo(&combo)).into(),
                    );
                    self.stop_recording();
                    cx.notify();
                    cx.stop_propagation();
                    return;
                }
                // A combo already bound elsewhere is REFUSED, naming the owner
                // (zeron settings.shortcuts.tsx: "… is already assigned to …").
                if let Some(owner) = conflict_owner(&self.keymap, recording, &combo) {
                    self.conflict_notice = Some(
                        format!(
                            "{} is already assigned to {}.",
                            display_combo(&combo),
                            owner.label()
                        )
                        .into(),
                    );
                    self.stop_recording();
                    cx.notify();
                } else {
                    self.keymap.set(recording, combo);
                    self.stop_recording();
                    self.conflict_notice = None;
                    self.commit(cx);
                }
            }
        }
        cx.stop_propagation();
    }

    /// One shortcut row: label left, Reset when customized, and
    /// the click-to-record combo chip (recording inverts it to
    /// white-on-black). `ix` is the id's position in [`ShortcutId::ALL`]
    /// (unique element ids across the group cards); `gx` is the row's place
    /// in its own card (separator rule).
    fn render_row(
        &self,
        id: ShortcutId,
        ix: usize,
        gx: usize,
        recording: Option<ShortcutId>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        widgets::card_row(theme, gx == 0)
            .child(
                div()
                    .flex_1()
                    .min_w(px(160.0))
                    .flex()
                    .flex_col()
                    .child(widgets::row_title(theme, id.label())),
            )
            .child(self.render_binding_control(id, ix, recording, theme, cx))
    }

    fn render_binding_control(
        &self,
        id: ShortcutId,
        ix: usize,
        recording: Option<ShortcutId>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let accent = theme.accent;
        let combo = self.keymap.get(id).to_string();
        let is_recording = recording == Some(id);
        let non_default = combo != id.default_combo();
        let chip_text: SharedString = if is_recording {
            "Press keys…".into()
        } else {
            display_combo(&combo).into()
        };
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(20.0))
            .when(non_default && !is_recording, |el| {
                el.child(
                    div()
                        .id(("shortcut-reset", ix))
                        .role(gpui::Role::Button)
                        .aria_label(format!("Reset {} shortcut", id.label()))
                        .min_h(px(24.0))
                        .tab_index(0)
                        .focus_visible(move |style| style.border_2().border_color(accent))
                        .text_size(crate::typography::ui_rems(11.0))
                        .text_color(theme.text_muted)
                        .cursor_pointer()
                        .hover(|s| s.text_color(theme.text))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.keymap.reset(id);
                            this.stop_recording();
                            this.commit(cx);
                        }))
                        .child(SharedString::from("Reset")),
                )
            })
            .child(
                div()
                    .id(("shortcut-combo", ix))
                    .role(gpui::Role::Button)
                    .aria_label(format!(
                        "Change {} shortcut: {}",
                        id.label(),
                        display_combo(&combo)
                    ))
                    .tab_index(0)
                    .min_w(px(96.0))
                    .h(px(widgets::SELECT_HEIGHT))
                    .px(px(12.0))
                    .rounded(px(8.0))
                    // Same glass wash as the settings dropdowns; the
                    // transparent edge is held for the focus ring.
                    .border_1()
                    .border_color(gpui::transparent_black())
                    .focus_visible(move |style| style.border_color(accent))
                    .flex()
                    .items_center()
                    .justify_center()
                    .font_family(theme.font_mono.clone())
                    .text_size(crate::typography::ui_rems(12.0))
                    .cursor_pointer()
                    .map(|el| {
                        if is_recording {
                            el.bg(accent.opacity(0.16))
                                .border_color(accent.opacity(0.55))
                                .text_color(theme.text)
                        } else {
                            let hover_key = format!("shortcut-combo-{ix}-hover");
                            el.bg(crate::motion::hover_blend(
                                &hover_key,
                                widgets::select_fill(theme, false),
                                widgets::select_fill(theme, true),
                            ))
                            .on_hover(crate::motion::hover_listener(hover_key))
                            .text_color(theme.text)
                        }
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.start_recording(id, window, cx);
                    }))
                    .child(chip_text),
            )
    }

    fn on_scroll_hovered(&mut self, hovered: &bool, _: &mut Window, cx: &mut Context<Self>) {
        if self.scroll.set_list_hovered(*hovered) {
            cx.notify();
        }
    }

    // Kept for the appshots half of this page (appshots.rs), whose host still
    // carries the drag-move listener itself.
    fn on_bar_drag_move(
        &mut self,
        event: &gpui::DragMoveEvent<popover::MenuScrollbarDrag>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.rail_drag_to(event.event.position.y) {
            cx.notify();
        }
    }

    /// The shared rail under the id of whichever page is showing. Kept as a
    /// method because the appshots half of this page renders through it.
    fn render_scrollbar(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let id = if self.appshots_page {
            "appshots-settings-page-scrollbar"
        } else if self.general_page {
            "general-settings-page-scrollbar"
        } else {
            "shortcuts-page-scrollbar"
        };
        popover::rail(self, id, theme, cx)
    }
}

impl popover::ScrollRailHost for ShortcutsPage {
    fn rail_bar(&mut self) -> &mut popover::MenuScrollbarState {
        self.scroll.rail_bar()
    }

    fn rail_scroll(&self) -> Option<gpui::ScrollHandle> {
        self.scroll.rail_scroll()
    }
}

/// The shortcut (other than `id`) already bound to `combo`, if any. Pure.
pub fn conflict_owner(keymap: &KeymapConfig, id: ShortcutId, combo: &str) -> Option<ShortcutId> {
    ShortcutId::ALL
        .into_iter()
        .find(|&other| other.available() && other != id && keymap.get(other) == combo)
}

pub fn send_combo_is_reserved(_behavior: ComposerSendBehavior, combo: &str) -> bool {
    combo == "mod-enter"
}

pub fn modifier_send_label(is_macos: bool) -> &'static str {
    if is_macos { "⌘ Enter" } else { "Ctrl Enter" }
}

/// The page's sections, in display order. [`group`] is a total match, so every
/// [`ShortcutId::ALL`] entry lands in exactly one — a shortcut added later
/// extends the match and appears on the page by construction
/// (`every_shortcut_lands_in_a_rendered_group` holds the other half: its group
/// name must be listed here).
const GROUP_ORDER: [&str; 7] = [
    "Files",
    "Browser",
    "Panels",
    "Sessions",
    "Projects",
    "Jump to session",
    "Appshots",
];

/// The section a shortcut's row renders under.
fn group(id: ShortcutId) -> &'static str {
    match id {
        ShortcutId::CaptureAppshot => "Appshots",
        ShortcutId::SaveFile => "Files",
        ShortcutId::BrowserReload => "Browser",
        ShortcutId::ToggleSidebar
        | ShortcutId::ToggleChanges
        | ShortcutId::ToggleFiles
        | ShortcutId::ToggleTerminal => "Panels",
        ShortcutId::NewProject => "Projects",
        ShortcutId::OpenModelPicker
        | ShortcutId::NewSession
        | ShortcutId::NextSession
        | ShortcutId::PrevSession
        | ShortcutId::ArchiveSession => "Sessions",
        ShortcutId::JumpSession(_) => "Jump to session",
    }
}

impl Render for ShortcutsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.appshots_page {
            if std::mem::take(&mut self.appshots_focus_pending) {
                window.focus(&self.focus, cx);
            }
            return self.render_appshots(window, cx);
        }
        let theme = Theme::of(cx).for_settings_surface();
        let recording = self.recording;
        let escape_stops_active_agent = self.escape_stops_active_agent;
        let send_behavior = self.composer_send_behavior;
        let compact_mode = crate::settings::transcript_compact_mode(cx);
        let customized = self.keymap != KeymapConfig::default()
            || escape_stops_active_agent
            || send_behavior != ComposerSendBehavior::default();
        let modifier_label = modifier_send_label(cfg!(target_os = "macos"));

        let send_behaviors = [
            (ComposerSendBehavior::Enter, "Enter"),
            (ComposerSendBehavior::ModEnter, modifier_label),
        ];
        let send_behavior_control = widgets::select(
            "composer-send-behavior",
            "Send messages with",
            &theme,
            |page: &mut Self| &mut page.send_select,
        )
        .options(
            send_behaviors
                .iter()
                .map(|(_, label)| widgets::SelectOption::new(*label)),
            send_behaviors
                .iter()
                .position(|(behavior, _)| *behavior == send_behavior)
                .unwrap_or_default(),
        )
        .width(128.0)
        .font_family(theme.font_mono.clone())
        .on_select(move |page, ix, _, cx| page.set_composer_send_behavior(send_behaviors[ix].0, cx))
        .render(&self.send_select, cx);

        let send_behavior_row = widgets::card_row(&theme, true)
            .child(
                div()
                    .flex_1()
                    .min_w(px(160.0))
                    .child(widgets::row_title(&theme, "Send messages with")),
            )
            .child(send_behavior_control);
        let compact_mode_row = widgets::card_row(&theme, false)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(widgets::row_title(&theme, "Compact mode"))
                    .child(widgets::meta_line(
                        &theme,
                        vec![
                            div()
                                .child("Collapse thinking and tools.")
                                .into_any_element(),
                        ],
                    )),
            )
            .child(
                widgets::toggle_switch(&theme, compact_mode, "transcript-compact-mode")
                    .id("transcript-compact-mode-toggle")
                    .tab_index(0)
                    .role(gpui::Role::Switch)
                    .aria_label("Compact mode")
                    .aria_toggled(if compact_mode {
                        gpui::Toggled::True
                    } else {
                        gpui::Toggled::False
                    })
                    .focus_visible(|s| s.border_2().border_color(theme.accent))
                    .cursor_pointer()
                    .on_click(cx.listener(move |_, _, _, cx| {
                        crate::settings::set_transcript_compact_mode(!compact_mode, cx);
                        cx.notify();
                    })),
            );
        let escape_behavior_row = widgets::card_row(&theme, false)
            .child(
                div()
                    .flex_1()
                    .min_w(px(160.0))
                    .child(widgets::row_title(&theme, "Stop agent with Escape"))
                    .child(widgets::meta_line(
                        &theme,
                        vec![
                            div()
                                .child("When no dialog or menu is open.")
                                .into_any_element(),
                        ],
                    )),
            )
            .child(
                widgets::toggle_switch(
                    &theme,
                    escape_stops_active_agent,
                    "escape-stops-active-agent",
                )
                .id("escape-stops-active-agent-toggle")
                .debug_selector(|| "escape-stops-active-agent-toggle".into())
                .tab_index(0)
                .role(gpui::Role::Switch)
                .aria_label("Stop active agent with Escape")
                .aria_toggled(if escape_stops_active_agent {
                    gpui::Toggled::True
                } else {
                    gpui::Toggled::False
                })
                .focus_visible(|s| s.border_2().border_color(theme.accent))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.set_escape_stops_active_agent(!escape_stops_active_agent, cx);
                })),
            );
        if self.general_page {
            let scrollbar = self.render_scrollbar(&theme, cx);
            return div()
                .id("general-settings-page-host")
                .relative()
                .size_full()
                .on_hover(cx.listener(Self::on_scroll_hovered))
                .child(
                    crate::edge_fade::edge_faded(
                        16.0,
                        true,
                        true,
                        div()
                            .id("general-settings-page")
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll.scroll)
                            .child(
                                widgets::page_column()
                                    .child(widgets::page_header(&theme, "General", None))
                                    .child(
                                        widgets::section_card(&theme)
                                            .child(send_behavior_row)
                                            .child(compact_mode_row)
                                            .child(escape_behavior_row),
                                    )
                                    .child(self.thread_naming.clone())
                                    .child(self.worktrees.clone()),
                            ),
                    )
                    .fade_overflow_y(&self.scroll.scroll),
                )
                .children(scrollbar)
                .into_any_element();
        }
        // One block per group, each under its small section label — the flat
        // 16-row table read as one undifferentiated wall. `ix` (the id's
        // position in ALL) keys the interactive elements, so ids stay unique
        // across blocks. The section wrapper owns the spacing, so the block's
        // own top margin is zeroed.
        let mut groups: Vec<gpui::AnyElement> = Vec::new();
        for name in GROUP_ORDER {
            if name == "Appshots" {
                continue;
            }
            let mut card = widgets::section_card(&theme).mt_0();
            let ids = ShortcutId::ALL.into_iter().filter(|&id| group(id) == name);
            for (gx, id) in ids.enumerate() {
                let ix = ShortcutId::ALL.iter().position(|&a| a == id).unwrap_or(0);
                card = card.child(self.render_row(id, ix, gx, recording, &theme, cx));
            }
            groups.push(widgets::section(&theme, name, card).into_any_element());
        }

        // Helper line stays in the muted tone even for a rejected conflict —
        // the message names the specific clash (zeron settings.shortcuts.tsx).
        let helper: SharedString = if recording.is_some() {
            "Press Escape to cancel.".into()
        } else if let Some(notice) = self.conflict_notice.clone() {
            notice
        } else {
            "Shortcuts must be unique.".into()
        };

        let scrollbar = self.render_scrollbar(&theme, cx);
        div()
            .id("shortcuts-page-host")
            .relative()
            .size_full()
            .on_hover(cx.listener(Self::on_scroll_hovered))
            .child(
                crate::edge_fade::edge_faded(16.0, true, true, div()
                    .id("shortcuts-page")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll.scroll)
                    .track_focus(&self.focus)
                    .child(
                        widgets::page_column()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_start()
                                    .flex_wrap()
                                    .justify_between()
                                    .gap(px(24.0))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(200.0))
                                            .flex()
                                            .flex_col()
                                            .child(widgets::page_header(
                                                &theme,
                                                "Keyboard shortcuts",
                                                None,
                                            ))
                                            .child(
                                                widgets::page_subtitle(
                                                    &theme,
                                                    "Click a binding, then press the new key combination.",
                                                )
                                                .max_w(px(512.0))
                                                .line_height(px(20.0)),
                                            ),
                                    )
                                    .child({
                                        // `disabled:opacity-35` when nothing is
                                        // customized or while recording.
                                        let disabled = !customized || recording.is_some();
                                        widgets::ghost_action(&theme)
                                            .id("shortcuts-restore-defaults")
                                            .flex_none()
                                            .when(disabled, |el| el.opacity(0.35))
                                            .when(!disabled, |el| {
                                                el.on_click(
                                                    cx.listener(|this, _, _, cx| {
                                                        this.keymap = KeymapConfig::default();
                                                        this.stop_recording();
                                                        this.conflict_notice = None;
                                                        this.commit(cx);
                                                        this.set_escape_stops_active_agent(
                                                            false, cx,
                                                        );
                                                        this.set_composer_send_behavior(
                                                            ComposerSendBehavior::Enter,
                                                            cx,
                                                        );
                                                    }),
                                                )
                                            })
                                            .child(
                                                crate::icons::icon(crate::icons::RESTART)
                                                    .size(px(14.0))
                                                    .text_color(theme.text_muted),
                                            )
                                            .child(SharedString::from("Restore defaults"))
                                    }),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .children(groups),
                            )
                            .child(
                                div()
                                    .mt(px(12.0))
                                    .px(px(4.0))
                                    .min_h(px(20.0))
                                    .flex()
                                    .justify_center()
                                    .text_size(crate::typography::ui_rems(12.0))
                                    .text_color(theme.text_muted)
                                    .child(helper),
                            )
                            ,
                    )).fade_overflow_y(&self.scroll.scroll),
            )
            .children(scrollbar)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn conversation_controls_work_after_moving_out_of_shortcuts(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
        });
        let (page, cx) = cx.add_window_view(|_, cx| {
            let state = cx.new(|_| AppState::new());
            let mut page = ShortcutsPage::new(
                state,
                KeymapConfig::default(),
                false,
                ComposerSendBehavior::Enter,
                false,
                false,
                AppshotDestination::Automatic,
                cx,
            );
            page.show_section(false, true);
            page
        });
        cx.update(|window, cx| window.draw(cx).clear());
        let send = cx.debug_bounds("composer-send-behavior").unwrap();
        cx.simulate_click(send.center(), gpui::Modifiers::default());
        cx.update(|window, cx| window.draw(cx).clear());
        let option = cx.debug_bounds("composer-send-behavior-option-1").unwrap();
        cx.simulate_click(option.center(), gpui::Modifiers::default());
        page.update(cx, |page, _| assert!(!page.send_select.is_open()));
        // Let the menu's exit animation finish before clicking beneath it:
        // the reap compares wall-clock instants, so real time must pass too.
        std::thread::sleep(
            crate::motion::MENU_OUT
                .total()
                .mul_f32(crate::motion::speed_scale()),
        );
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(1));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear());
        assert!(cx.debug_bounds("composer-send-behavior-option-1").is_none());
        let escape = cx.debug_bounds("escape-stops-active-agent-toggle").unwrap();
        cx.simulate_click(escape.center(), gpui::Modifiers::default());
        page.update(cx, |page, _| {
            assert_eq!(page.composer_send_behavior, ComposerSendBehavior::ModEnter);
            assert!(page.escape_stops_active_agent);
            page.show_section(false, false);
        });
        cx.update(|window, cx| window.draw(cx).clear());
        assert!(cx.debug_bounds("composer-send-behavior").is_none());
        assert!(
            cx.debug_bounds("escape-stops-active-agent-toggle")
                .is_none()
        );
    }

    #[gpui::test]
    fn appshots_setup_can_be_enabled_and_configured_by_keyboard(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
        });
        let window = cx.add_window(|_, cx| {
            let state = cx.new(|_| AppState::new());
            let mut page = ShortcutsPage::new(
                state,
                KeymapConfig::default(),
                false,
                ComposerSendBehavior::default(),
                false,
                false,
                AppshotDestination::Automatic,
                cx,
            );
            page.show_appshots(true);
            page
        });
        window
            .update(cx, |page, w, cx| w.focus(&page.focus, cx))
            .unwrap();
        cx.update_window(window.into(), |_, w, cx| {
            w.draw(cx).clear();
        })
        .unwrap();
        let press = |cx: &mut gpui::TestAppContext, key: &str| {
            cx.update_window(window.into(), |_, w, cx| {
                w.draw(cx).clear();
                let keystroke = gpui::Keystroke::parse(key).unwrap();
                w.dispatch_event(
                    gpui::PlatformInput::KeyDown(gpui::KeyDownEvent {
                        keystroke: keystroke.clone(),
                        is_held: false,
                        prefer_character_input: false,
                    }),
                    cx,
                );
                w.dispatch_event(
                    gpui::PlatformInput::KeyUp(gpui::KeyUpEvent { keystroke }),
                    cx,
                );
            })
            .unwrap();
        };
        press(cx, "tab");
        press(cx, "space");
        window
            .update(cx, |page, _, _| assert!(page.appshots_enabled))
            .unwrap();
        press(cx, "tab");
        press(cx, "enter");
        window
            .update(cx, |page, _, _| assert!(page.appshot_sound_enabled))
            .unwrap();
        // Skip the shortcut trigger; arrows open the destination dropdown on
        // the current choice and Enter commits the highlighted one.
        for key in ["tab", "tab", "down", "down", "enter"] {
            press(cx, key);
        }
        window
            .update(cx, |page, _, _| {
                assert_eq!(page.appshot_destination, AppshotDestination::LastSession);
                assert!(!page.destination_select.is_open());
            })
            .unwrap();
        for key in ["space", "down", "enter"] {
            press(cx, key);
        }
        window
            .update(cx, |page, _, _| {
                assert_eq!(page.appshot_destination, AppshotDestination::NewSession)
            })
            .unwrap();
        // Escape closes without choosing.
        for key in ["up", "up", "escape"] {
            press(cx, key);
        }
        window
            .update(cx, |page, _, _| {
                assert_eq!(page.appshot_destination, AppshotDestination::NewSession);
                assert!(!page.destination_select.is_open());
            })
            .unwrap();
        for key in ["up", "up", "enter"] {
            press(cx, key);
        }
        window
            .update(cx, |page, _, _| {
                assert_eq!(page.appshot_destination, AppshotDestination::LastSession)
            })
            .unwrap();
    }

    #[gpui::test]
    fn recorder_refuses_bound_actions_before_they_can_run(cx: &mut gpui::TestAppContext) {
        use std::{cell::Cell, rc::Rc};
        let fired = Rc::new(Cell::new(false));
        let observed = fired.clone();
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            cx.bind_keys([gpui::KeyBinding::new(
                &crate::settings::platform_combo("mod-n"),
                crate::shell::NewSession,
                None,
            )]);
            cx.on_action(move |_: &crate::shell::NewSession, _| observed.set(true));
        });
        let window = cx.add_window(|_, cx| {
            let state = cx.new(|_| AppState::new());
            ShortcutsPage::new(
                state,
                KeymapConfig::default(),
                false,
                ComposerSendBehavior::default(),
                false,
                true,
                AppshotDestination::Automatic,
                cx,
            )
        });
        window
            .update(cx, |page, window, cx| {
                page.start_recording(ShortcutId::CaptureAppshot, window, cx)
            })
            .unwrap();
        cx.simulate_keystrokes(window.into(), &crate::settings::platform_combo("mod-n"));
        window
            .update(cx, |page, _, _| {
                assert!(!fired.get(), "the existing action ran while recording");
                assert!(
                    page.conflict_notice
                        .as_deref()
                        .unwrap()
                        .contains("New session")
                );
                assert_eq!(
                    page.keymap.capture_appshot,
                    ShortcutId::CaptureAppshot.default_combo()
                );
                assert!(page.recording.is_none());
                assert!(page.recording_interceptor.is_none());
            })
            .unwrap();
        cx.simulate_keystrokes(window.into(), &crate::settings::platform_combo("mod-n"));
        assert!(
            fired.get(),
            "finishing recording must restore normal actions"
        );
    }

    #[test]
    fn recording_outcomes() {
        assert_eq!(
            record_key("escape", false, false, false, false),
            RecordOutcome::Cancelled
        );
        assert_eq!(
            record_key("Escape", true, false, false, false),
            RecordOutcome::Cancelled
        );
        assert_eq!(
            record_key("s", false, false, false, true),
            RecordOutcome::Set("mod-s".into())
        );
        assert_eq!(
            record_key("k", false, true, true, true),
            RecordOutcome::Set("mod-alt-shift-k".into())
        );
        // macOS-only: elsewhere ctrl IS the primary and records as "mod".
        #[cfg(target_os = "macos")]
        assert_eq!(
            record_key("tab", true, false, true, false),
            RecordOutcome::Set("ctrl-shift-tab".into())
        );
        // Bare modifiers stay recording.
        assert_eq!(
            record_key("shift", false, false, true, false),
            RecordOutcome::Ignored
        );
        assert_eq!(
            record_key("ctrl", true, false, false, false),
            RecordOutcome::Ignored
        );
    }

    #[test]
    fn every_shortcut_lands_in_a_rendered_group() {
        // The page renders GROUP_ORDER's cards and nothing else — a group()
        // arm returning a name missing from GROUP_ORDER would silently drop
        // its rows from Settings.
        for id in ShortcutId::ALL {
            assert!(
                GROUP_ORDER.contains(&group(id)),
                "{:?} is grouped under {:?}, which GROUP_ORDER does not render",
                id,
                group(id)
            );
        }
        // And every named group has at least one row — no empty cards.
        for name in GROUP_ORDER {
            assert!(
                ShortcutId::ALL.into_iter().any(|id| group(id) == name),
                "group {:?} would render an empty card",
                name
            );
        }
    }

    #[test]
    fn conflicting_records_are_refused() {
        // zeron parity: a combo bound elsewhere is refused at record time (the
        // helper names the owner) — conflicts never persist into the keymap.
        let keymap = KeymapConfig::default();
        let RecordOutcome::Set(combo) = record_key("r", false, false, false, true) else {
            panic!("expected Set");
        };
        assert_eq!(
            conflict_owner(&keymap, ShortcutId::ToggleSidebar, &combo),
            Some(ShortcutId::ToggleChanges)
        );
        // Re-recording a shortcut's own combo is not a conflict.
        assert_eq!(
            conflict_owner(&keymap, ShortcutId::ToggleChanges, &combo),
            None
        );
        // A free combo conflicts with nothing.
        assert_eq!(
            conflict_owner(&keymap, ShortcutId::ToggleSidebar, "mod-shift-x"),
            None
        );
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn appshot_binding_participates_in_existing_conflict_checks() {
        let mut keymap = KeymapConfig::default();
        assert_eq!(
            conflict_owner(&keymap, ShortcutId::CaptureAppshot, "mod-n"),
            Some(ShortcutId::NewSession)
        );
        keymap.set(ShortcutId::CaptureAppshot, "mod-alt-k".into());
        assert_eq!(
            conflict_owner(&keymap, ShortcutId::NewSession, "mod-alt-k"),
            Some(ShortcutId::CaptureAppshot)
        );
        assert_eq!(
            conflict_owner(&keymap, ShortcutId::CaptureAppshot, "mod-alt-k"),
            None
        );
    }

    #[test]
    fn modifier_send_labels_are_platform_specific() {
        assert_eq!(modifier_send_label(true), "⌘ Enter");
        assert_eq!(modifier_send_label(false), "Ctrl Enter");
    }

    #[test]
    fn modifier_send_is_always_reserved_for_the_composer() {
        assert!(send_combo_is_reserved(
            ComposerSendBehavior::Enter,
            "mod-enter"
        ));
        assert!(send_combo_is_reserved(
            ComposerSendBehavior::ModEnter,
            "mod-enter"
        ));
        assert!(!send_combo_is_reserved(
            ComposerSendBehavior::ModEnter,
            "mod-shift-enter"
        ));
    }
}
