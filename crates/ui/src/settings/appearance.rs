//! Settings → Appearance: system behavior, independent light/dark variants,
//! and the optional interactive accent overlay.

use std::cell::Cell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui::{
    AnyElement, Context, Entity, EventEmitter, FocusHandle, Focusable, Hsla, IntoElement,
    KeyDownEvent, ObjectFit, Render, SharedString, StyledImage as _, Subscription, Window, div,
    img, prelude::*, px,
};
use zeron_theme::vscode::{ImportReport, SourceCompilation};
use zeron_theme::{
    AccentPreset, AccentSelection, CustomThemeEntry, CustomThemeStatus, InstallMode,
    SurfacePreference, SurfaceTreatment, ThemeRegistry, ThemeSelection, ValidationCategory,
    ValidationSeverity,
};

use crate::appearance::{self, AppearanceMode};
use crate::composer::{ComposerInput, ComposerInputEvent};
use crate::i18n::{self, LanguagePreference, Locale, MessageId};
use crate::icons;
use crate::popover::{self, Popup};
use crate::settings::widgets;
use crate::theme::{Appearance, Theme};
use crate::theme_library;
use crate::typography::{self, FontAvailability, UiFontFamily, UiFontSize};

struct ImportDialog {
    input: Entity<ComposerInput>,
    _events: Subscription,
    focus: FocusHandle,
    focus_pending: bool,
    mode: InstallMode,
    compilation: Option<SourceCompilation>,
    selected: HashSet<String>,
    review_variant: Option<String>,
    error: Option<SharedString>,
}

/// The three independently configurable font slots. Interface and code/diff
/// draw from the whole catalog — proportional faces are legal there. The
/// terminal draws from the fixed-width subset only.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FontKind {
    Ui,
    Terminal,
    Code,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AppearanceSettingsEvent {
    CodeFontSizeChanged(f32),
}

impl FontKind {
    const ALL: [Self; 3] = [Self::Ui, Self::Terminal, Self::Code];

    fn slug(self) -> &'static str {
        match self {
            Self::Ui => "interface",
            Self::Terminal => "terminal",
            Self::Code => "code",
        }
    }

    fn label(self, locale: Locale) -> &'static str {
        i18n::translate(
            match self {
                Self::Ui => MessageId::FontKindUiLabel,
                Self::Terminal => MessageId::FontKindTerminalLabel,
                Self::Code => MessageId::FontKindCodeLabel,
            },
            locale,
        )
    }

    fn description(self, locale: Locale) -> &'static str {
        i18n::translate(
            match self {
                Self::Ui => MessageId::FontKindUiDescription,
                Self::Terminal => MessageId::FontKindTerminalDescription,
                Self::Code => MessageId::FontKindCodeDescription,
            },
            locale,
        )
    }

    /// The catalog this slot may pick from. Only the terminal narrows: its
    /// renderer positions cursor, selection, and hit-testing on an `m`-wide
    /// cell grid, which a proportional family silently breaks.
    fn choices_for(self, availability: &FontAvailability) -> &[UiFontFamily] {
        match self {
            Self::Terminal => availability.fixed_width_choices(),
            _ => availability.choices(),
        }
    }

    fn is_available_for(self, availability: &FontAvailability, family: &UiFontFamily) -> bool {
        match self {
            Self::Terminal => availability.is_fixed_width_available(family),
            _ => availability.is_available(family),
        }
    }

    fn requested(self, cx: &gpui::App) -> UiFontFamily {
        match self {
            Self::Ui => typography::requested(cx),
            Self::Terminal => typography::terminal_requested(cx),
            Self::Code => typography::code_requested(cx),
        }
    }

    fn effective(self, cx: &gpui::App) -> UiFontFamily {
        match self {
            Self::Ui => typography::effective(cx),
            Self::Terminal => typography::terminal_effective(cx),
            Self::Code => typography::code_effective(cx),
        }
    }

    fn apply_family(self, family: UiFontFamily, cx: &mut gpui::App) {
        match self {
            Self::Ui => typography::set_family(family, cx),
            Self::Terminal => typography::set_terminal_family(family, cx),
            Self::Code => typography::set_code_family(family, cx),
        };
    }

    fn pixel_size(self, cx: &gpui::App) -> f32 {
        match self {
            Self::Ui => typography::font_size(cx).pixels(),
            Self::Terminal => typography::terminal_font_size(cx),
            Self::Code => typography::code_font_size(cx),
        }
    }

    /// Labels for this kind's size ladder, in ladder order. Both ladders read
    /// as plain pixel values, so all three dropdowns look the same.
    fn size_labels(self) -> Vec<SharedString> {
        match self {
            Self::Ui => UiFontSize::ALL.iter().map(|size| size.label()).collect(),
            _ => MONO_FONT_SIZES
                .iter()
                .map(|size| SharedString::from(format_px(*size)))
                .collect(),
        }
    }

    fn size_count(self) -> usize {
        match self {
            Self::Ui => UiFontSize::ALL.len(),
            _ => MONO_FONT_SIZES.len(),
        }
    }

    /// Ladder position of the committed size. Terminal and code sizes are
    /// stored as free pixels (older settings, hand-edited files), so they snap
    /// to the nearest rung rather than falling off the list.
    fn size_ix(self, cx: &gpui::App) -> usize {
        match self {
            Self::Ui => UiFontSize::ALL
                .iter()
                .position(|size| *size == typography::font_size(cx))
                .unwrap_or_default(),
            _ => nearest_mono_ix(self.pixel_size(cx)),
        }
    }
}

/// Pixel ladder behind the terminal and code size dropdowns. Both defaults
/// (terminal 13, code 12.5) are rungs, so today's rendering is reachable.
const MONO_FONT_SIZES: [f32; 10] = [10.0, 11.0, 12.0, 12.5, 13.0, 14.0, 15.0, 16.0, 18.0, 20.0];

fn nearest_mono_ix(size: f32) -> usize {
    MONO_FONT_SIZES
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| (**a - size).abs().total_cmp(&(**b - size).abs()))
        .map(|(ix, _)| ix)
        .unwrap_or_default()
}

#[derive(Clone)]
struct TranscriptWidthDrag;

impl Render for TranscriptWidthDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

pub struct AppearancePage {
    width_focus: FocusHandle,
    width_hovered: bool,
    width_pressed: bool,
    width_keyboard_active: bool,
    width_bounds: Rc<Cell<Option<gpui::Bounds<gpui::Pixels>>>>,
    pending_width: Option<f32>,
    width_frame_pending: bool,
    scroll: crate::settings::widgets::PageScroll,
    selected_font: UiFontFamily,
    selected_terminal_font: UiFontFamily,
    selected_code_font: UiFontFamily,
    selected_size: UiFontSize,
    selected_terminal_size: f32,
    selected_code_size: f32,
    font_focus: FocusHandle,
    terminal_font_focus: FocusHandle,
    code_font_focus: FocusHandle,
    size_focus: FocusHandle,
    terminal_size_focus: FocusHandle,
    code_size_focus: FocusHandle,
    font_menu: Popup<()>,
    terminal_font_menu: Popup<()>,
    code_font_menu: Popup<()>,
    /// Floating rail for each family dropdown (the menu-scrollbar treatment,
    /// on the menu's own scroll host). One per kind: the menus scroll
    /// independently, so sharing a state would carry one menu's offset and
    /// rail timers into the next one opened.
    font_list: widgets::PageScroll,
    terminal_font_list: widgets::PageScroll,
    code_font_list: widgets::PageScroll,
    size_menu: Popup<()>,
    terminal_size_menu: Popup<()>,
    code_size_menu: Popup<()>,
    font_menu_dismissed_at: Option<std::time::Instant>,
    terminal_font_menu_dismissed_at: Option<std::time::Instant>,
    code_font_menu_dismissed_at: Option<std::time::Instant>,
    /// Filter for whichever family menu is open — a device can carry hundreds
    /// of families, so the list narrows as you type instead of asking you to
    /// scroll. One input serves all three kinds: only one menu is ever open.
    font_search: Entity<ComposerInput>,
    _font_search_events: Subscription,
    size_menu_dismissed_at: Option<std::time::Instant>,
    terminal_size_menu_dismissed_at: Option<std::time::Instant>,
    code_size_menu_dismissed_at: Option<std::time::Instant>,
    language_menu: Popup<()>,
    light_theme_menu: Popup<()>,
    dark_theme_menu: Popup<()>,
    import_dialog: Option<ImportDialog>,
    review_entry: Option<String>,
    library_error: Option<SharedString>,
    background_error: Option<SharedString>,
}

impl AppearancePage {
    fn queue_width(&mut self, width: f32, window: &mut Window, cx: &mut Context<Self>) {
        let width = crate::settings::normalize_transcript_width(width);
        if self
            .pending_width
            .unwrap_or_else(|| crate::settings::transcript_width(cx))
            == width
        {
            return;
        }
        self.pending_width = Some(width);
        // Coalesce pointer events into one layout/settings update per frame.
        if !self.width_frame_pending {
            self.width_frame_pending = true;
            let page = cx.weak_entity();
            window.on_next_frame(move |_, cx| {
                let _ = page.update(cx, |this, cx| {
                    this.width_frame_pending = false;
                    this.apply_pending_width(cx);
                });
            });
        }
        cx.notify();
    }

    fn apply_pending_width(&mut self, cx: &mut Context<Self>) {
        if let Some(width) = self.pending_width.take() {
            crate::settings::set_transcript_width(width, cx);
            cx.notify();
        }
    }

    fn drag_width(&mut self, x: gpui::Pixels, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bounds) = self.width_bounds.get() else {
            return;
        };
        let fraction =
            (f32::from(x - bounds.left()) - 7.0) / (f32::from(bounds.size.width) - 14.0).max(1.0);
        self.queue_width(
            crate::settings::TRANSCRIPT_WIDTH_MIN
                + fraction.clamp(0.0, 1.0)
                    * (crate::settings::TRANSCRIPT_WIDTH_MAX
                        - crate::settings::TRANSCRIPT_WIDTH_MIN),
            window,
            cx,
        );
    }

    fn render_transcript_width(
        &self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use crate::settings::{
            TRANSCRIPT_WIDTH_DEFAULT, TRANSCRIPT_WIDTH_MAX, TRANSCRIPT_WIDTH_MIN,
            TRANSCRIPT_WIDTH_STEP,
        };
        let locale = i18n::locale(cx);
        let width = self
            .pending_width
            .unwrap_or_else(|| crate::settings::transcript_width(cx));
        let fraction =
            (width - TRANSCRIPT_WIDTH_MIN) / (TRANSCRIPT_WIDTH_MAX - TRANSCRIPT_WIDTH_MIN);
        let bounds = self.width_bounds.clone();
        let show_details = self.width_hovered
            || self.width_pressed
            || (self.width_keyboard_active && self.width_focus.is_focused(window));
        let slider = div()
            .id("transcript-width-slider")
            .track_focus(&self.width_focus)
            .relative()
            .w(px(240.0))
            .h(px(28.0))
            .cursor_pointer()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                    window.focus(&this.width_focus, cx);
                    this.width_pressed = true;
                    this.width_keyboard_active = false;
                    cx.notify();
                    cx.stop_propagation();
                    this.drag_width(event.position.x, window, cx);
                }),
            )
            .on_drag(TranscriptWidthDrag, |drag, _, _, cx| {
                cx.stop_propagation();
                cx.new(|_| drag.clone())
            })
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let width = this
                    .pending_width
                    .unwrap_or_else(|| crate::settings::transcript_width(cx));
                let next = match event.keystroke.key.as_str() {
                    "left" | "down" => width - TRANSCRIPT_WIDTH_STEP,
                    "right" | "up" => width + TRANSCRIPT_WIDTH_STEP,
                    "home" => TRANSCRIPT_WIDTH_MIN,
                    "end" => TRANSCRIPT_WIDTH_MAX,
                    _ => return,
                };
                this.width_keyboard_active = true;
                cx.notify();
                cx.stop_propagation();
                window.prevent_default();
                this.queue_width(next, window, cx);
            }))
            .child(
                gpui::canvas(move |rect, _, _| bounds.set(Some(rect)), |_, _, _, _| {})
                    .absolute()
                    .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .left(px(7.0))
                    .right(px(7.0))
                    .top(px(12.0))
                    .h(px(4.0))
                    .rounded_full()
                    .bg(theme.border)
                    .child(
                        div()
                            .h_full()
                            .w(gpui::relative(fraction))
                            .rounded_full()
                            .bg(theme.accent),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(gpui::relative(fraction))
                            .ml(px(-7.0))
                            .top(px(-5.0))
                            .size(px(14.0))
                            .rounded_full()
                            .bg(theme.accent),
                    ),
            );
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(20.0))
            .child(
                div()
                    .flex_1()
                    .min_w(px(200.0))
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(widgets::field_label(
                        theme,
                        i18n::translate(MessageId::AppearanceConversationWidth, locale),
                    ))
                    .child(
                        div()
                            .text_size(typography::ui_rems(12.0))
                            .line_height(px(18.0))
                            .text_color(theme.text_muted)
                            .child(i18n::translate(
                                MessageId::AppearanceConversationWidthHint,
                                locale,
                            )),
                    ),
            )
            .child(
                div()
                    .id("transcript-width-control")
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        this.width_hovered = *hovered;
                        if *hovered {
                            this.width_keyboard_active = false;
                        }
                        cx.notify();
                    }))
                    // Details occupy the surrounding whitespace, so the
                    // slider keeps the same row rhythm as the font controls.
                    .my(px(-12.0))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .when(!show_details, |el| el.invisible())
                            .flex()
                            .justify_between()
                            .text_size(typography::ui_rems(12.0))
                            .line_height(px(16.0))
                            .child(format!("{width:.0} px"))
                            .child(
                                div()
                                    .id("reset-transcript-width")
                                    .cursor_pointer()
                                    .text_color(theme.text_muted)
                                    .hover(|style| style.text_color(theme.text))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.pending_width = None;
                                        crate::settings::set_transcript_width(
                                            TRANSCRIPT_WIDTH_DEFAULT,
                                            cx,
                                        );
                                        cx.notify();
                                    }))
                                    .child(i18n::translate(MessageId::CommonReset, locale)),
                            ),
                    )
                    .child(slider)
                    .child(
                        div()
                            .when(!show_details, |el| el.invisible())
                            .flex()
                            .justify_between()
                            .text_size(typography::ui_rems(11.0))
                            .line_height(px(14.0))
                            .text_color(theme.text_muted)
                            .child("560 px")
                            .child("1,200 px"),
                    ),
            )
            .into_any_element()
    }

    pub fn new(cx: &mut Context<Self>) -> Self {
        // `PaletteSearch` binds text-editing keys only — arrows/Enter/Escape
        // stay unbound and bubble from the input to the menu card's own key
        // handler. `Submitted` never fires here, so Enter has exactly one path.
        let font_search = cx.new(|cx| {
            ComposerInput::with_context(
                i18n::translate(MessageId::AppearanceSearchFonts, i18n::locale(cx)),
                "PaletteSearch",
                cx,
            )
        });
        let font_search_events = cx.subscribe(&font_search, |this: &mut Self, _, event, cx| {
            if matches!(event, ComposerInputEvent::Edited) {
                this.on_font_search_edited(cx);
            }
        });
        Self {
            width_focus: cx.focus_handle(),
            width_hovered: false,
            width_pressed: false,
            width_keyboard_active: false,
            width_bounds: Rc::default(),
            pending_width: None,
            width_frame_pending: false,
            scroll: crate::settings::widgets::PageScroll::default(),
            selected_font: typography::effective(cx),
            selected_terminal_font: typography::terminal_effective(cx),
            selected_code_font: typography::code_effective(cx),
            selected_size: typography::font_size(cx),
            selected_terminal_size: typography::terminal_font_size(cx),
            selected_code_size: typography::code_font_size(cx),
            font_focus: cx.focus_handle(),
            terminal_font_focus: cx.focus_handle(),
            code_font_focus: cx.focus_handle(),
            size_focus: cx.focus_handle(),
            terminal_size_focus: cx.focus_handle(),
            code_size_focus: cx.focus_handle(),
            font_menu: Popup::default(),
            terminal_font_menu: Popup::default(),
            code_font_menu: Popup::default(),
            font_list: widgets::PageScroll::default(),
            terminal_font_list: widgets::PageScroll::default(),
            code_font_list: widgets::PageScroll::default(),
            size_menu: Popup::default(),
            terminal_size_menu: Popup::default(),
            code_size_menu: Popup::default(),
            font_menu_dismissed_at: None,
            terminal_font_menu_dismissed_at: None,
            code_font_menu_dismissed_at: None,
            font_search,
            _font_search_events: font_search_events,
            size_menu_dismissed_at: None,
            terminal_size_menu_dismissed_at: None,
            code_size_menu_dismissed_at: None,
            language_menu: Popup::default(),
            light_theme_menu: Popup::default(),
            dark_theme_menu: Popup::default(),
            import_dialog: None,
            review_entry: None,
            library_error: None,
            background_error: None,
        }
    }

    fn font_menu(&self, kind: FontKind) -> &Popup<()> {
        match kind {
            FontKind::Ui => &self.font_menu,
            FontKind::Terminal => &self.terminal_font_menu,
            FontKind::Code => &self.code_font_menu,
        }
    }

    fn font_menu_mut(&mut self, kind: FontKind) -> &mut Popup<()> {
        match kind {
            FontKind::Ui => &mut self.font_menu,
            FontKind::Terminal => &mut self.terminal_font_menu,
            FontKind::Code => &mut self.code_font_menu,
        }
    }

    fn font_list_mut(&mut self, kind: FontKind) -> &mut widgets::PageScroll {
        match kind {
            FontKind::Ui => &mut self.font_list,
            FontKind::Terminal => &mut self.terminal_font_list,
            FontKind::Code => &mut self.code_font_list,
        }
    }

    fn selected_font(&self, kind: FontKind) -> &UiFontFamily {
        match kind {
            FontKind::Ui => &self.selected_font,
            FontKind::Terminal => &self.selected_terminal_font,
            FontKind::Code => &self.selected_code_font,
        }
    }

    fn set_selected_font(&mut self, kind: FontKind, family: UiFontFamily) {
        match kind {
            FontKind::Ui => self.selected_font = family,
            FontKind::Terminal => self.selected_terminal_font = family,
            FontKind::Code => self.selected_code_font = family,
        }
    }

    fn font_focus(&self, kind: FontKind) -> &FocusHandle {
        match kind {
            FontKind::Ui => &self.font_focus,
            FontKind::Terminal => &self.terminal_font_focus,
            FontKind::Code => &self.code_font_focus,
        }
    }

    fn font_dismissed_at(&mut self, kind: FontKind) -> &mut Option<std::time::Instant> {
        match kind {
            FontKind::Ui => &mut self.font_menu_dismissed_at,
            FontKind::Terminal => &mut self.terminal_font_menu_dismissed_at,
            FontKind::Code => &mut self.code_font_menu_dismissed_at,
        }
    }

    fn commit_font(&mut self, kind: FontKind, cx: &mut Context<Self>) {
        let family = self.selected_font(kind).clone();
        if kind.is_available_for(&typography::availability(cx), &family) {
            kind.apply_family(family, cx);
            let effective = kind.effective(cx);
            self.set_selected_font(kind, effective);
            self.close_font_menu(kind, cx);
            cx.notify();
        }
    }

    fn size_menu(&self, kind: FontKind) -> &Popup<()> {
        match kind {
            FontKind::Ui => &self.size_menu,
            FontKind::Terminal => &self.terminal_size_menu,
            FontKind::Code => &self.code_size_menu,
        }
    }

    fn size_menu_mut(&mut self, kind: FontKind) -> &mut Popup<()> {
        match kind {
            FontKind::Ui => &mut self.size_menu,
            FontKind::Terminal => &mut self.terminal_size_menu,
            FontKind::Code => &mut self.code_size_menu,
        }
    }

    fn size_focus(&self, kind: FontKind) -> &FocusHandle {
        match kind {
            FontKind::Ui => &self.size_focus,
            FontKind::Terminal => &self.terminal_size_focus,
            FontKind::Code => &self.code_size_focus,
        }
    }

    fn size_dismissed_at(&mut self, kind: FontKind) -> &mut Option<std::time::Instant> {
        match kind {
            FontKind::Ui => &mut self.size_menu_dismissed_at,
            FontKind::Terminal => &mut self.terminal_size_menu_dismissed_at,
            FontKind::Code => &mut self.code_size_menu_dismissed_at,
        }
    }

    /// Highlighted rung of this kind's size ladder.
    fn selected_size_ix(&self, kind: FontKind) -> usize {
        match kind {
            FontKind::Ui => UiFontSize::ALL
                .iter()
                .position(|size| *size == self.selected_size)
                .unwrap_or_default(),
            FontKind::Terminal => nearest_mono_ix(self.selected_terminal_size),
            FontKind::Code => nearest_mono_ix(self.selected_code_size),
        }
    }

    fn set_selected_size_ix(&mut self, kind: FontKind, ix: usize) {
        let ix = ix.min(kind.size_count() - 1);
        match kind {
            FontKind::Ui => self.selected_size = UiFontSize::ALL[ix],
            FontKind::Terminal => self.selected_terminal_size = MONO_FONT_SIZES[ix],
            FontKind::Code => self.selected_code_size = MONO_FONT_SIZES[ix],
        }
    }

    fn commit_size(&mut self, kind: FontKind, window: &mut Window, cx: &mut Context<Self>) {
        let ix = self.selected_size_ix(kind);
        match kind {
            FontKind::Ui => {
                typography::set_font_size(UiFontSize::ALL[ix], window, cx);
                self.selected_size = typography::font_size(cx);
            }
            FontKind::Terminal => {
                let next = MONO_FONT_SIZES[ix];
                typography::set_terminal_font_size(next, cx);
                self.selected_terminal_size = next;
            }
            FontKind::Code => {
                let next = MONO_FONT_SIZES[ix];
                typography::set_code_font_size(next, cx);
                self.selected_code_size = next;
                // Open file editors only learn the new size through this event.
                cx.emit(AppearanceSettingsEvent::CodeFontSizeChanged(next));
            }
        }
        self.close_size_menu(kind, cx);
        cx.notify();
    }

    /// This kind's catalog, narrowed and ranked by the typed query.
    fn visible_choices(
        &self,
        kind: FontKind,
        availability: &FontAvailability,
        cx: &gpui::App,
    ) -> Vec<UiFontFamily> {
        filter_families(
            self.font_search.read(cx).text(),
            kind.choices_for(availability),
        )
    }

    /// The kind whose menu is open, if any. Opening one closes the others.
    fn open_font_kind(&self) -> Option<FontKind> {
        FontKind::ALL
            .into_iter()
            .find(|kind| self.font_menu(*kind).is_open())
    }

    fn on_font_search_edited(&mut self, cx: &mut Context<Self>) {
        if let Some(kind) = self.open_font_kind() {
            self.clamp_highlight(kind, cx);
        }
    }

    /// Keep the highlighted row inside the filtered list, so the next Enter
    /// cannot commit a family the query has already filtered out of existence.
    fn clamp_highlight(&mut self, kind: FontKind, cx: &mut Context<Self>) {
        let availability = typography::availability(cx);
        let visible = self.visible_choices(kind, &availability, cx);
        if !visible.contains(self.selected_font(kind)) {
            let next = if visible.is_empty() {
                kind.effective(cx)
            } else {
                first_available(&visible, kind, &availability)
            };
            self.set_selected_font(kind, next);
        }
        cx.notify();
    }

    fn close_font_menu(&mut self, kind: FontKind, cx: &mut Context<Self>) {
        if !self.font_menu_mut(kind).begin_close() {
            return;
        }
        match kind {
            FontKind::Ui => popover::reap_popup(cx, |page| &mut page.font_menu),
            FontKind::Terminal => popover::reap_popup(cx, |page| &mut page.terminal_font_menu),
            FontKind::Code => popover::reap_popup(cx, |page| &mut page.code_font_menu),
        }
    }

    /// Only one of this page's six font dropdowns may be open at a time;
    /// opening any one closes the other five.
    fn close_other_menus(
        &mut self,
        keep_family: Option<FontKind>,
        keep_size: Option<FontKind>,
        cx: &mut Context<Self>,
    ) {
        for kind in FontKind::ALL {
            if Some(kind) != keep_family {
                self.close_font_menu(kind, cx);
            }
            if Some(kind) != keep_size {
                self.close_size_menu(kind, cx);
            }
        }
        self.close_language_menu(cx);
    }

    fn close_size_menu(&mut self, kind: FontKind, cx: &mut Context<Self>) {
        if !self.size_menu_mut(kind).begin_close() {
            return;
        }
        match kind {
            FontKind::Ui => popover::reap_popup(cx, |page| &mut page.size_menu),
            FontKind::Terminal => popover::reap_popup(cx, |page| &mut page.terminal_size_menu),
            FontKind::Code => popover::reap_popup(cx, |page| &mut page.code_size_menu),
        }
    }

    fn close_language_menu(&mut self, cx: &mut Context<Self>) {
        if !self.language_menu.begin_close() {
            return;
        }
        popover::reap_popup(cx, |page| &mut page.language_menu);
    }

    fn toggle_language_menu(&mut self, cx: &mut Context<Self>) {
        let was_open = self.language_menu.is_open();
        self.close_other_menus(None, None, cx);
        if !was_open {
            self.language_menu.open(());
        }
        cx.notify();
    }

    fn dismiss_font_menu(&mut self, kind: FontKind, cx: &mut Context<Self>) {
        *self.font_dismissed_at(kind) = Some(std::time::Instant::now());
        self.close_font_menu(kind, cx);
    }

    fn dismiss_size_menu(&mut self, kind: FontKind, cx: &mut Context<Self>) {
        *self.size_dismissed_at(kind) = Some(std::time::Instant::now());
        self.close_size_menu(kind, cx);
    }

    fn toggle_font_menu(&mut self, kind: FontKind, window: &mut Window, cx: &mut Context<Self>) {
        self.close_other_menus(Some(kind), None, cx);
        let just_dismissed = self
            .font_dismissed_at(kind)
            .take()
            .is_some_and(|at| at.elapsed() < std::time::Duration::from_millis(400));
        if self.font_menu(kind).is_open() {
            self.close_font_menu(kind, cx);
        } else if !just_dismissed {
            // Clear before opening: the resulting `Edited` finds no open menu,
            // so it cannot clobber the highlight we anchor on the next line.
            self.font_search
                .update(cx, |input, cx| input.set_text("", cx));
            let effective = kind.effective(cx);
            self.set_selected_font(kind, effective);
            // Every open starts at the top, and the rail's baseline with it —
            // no reopen flash from the previous session's offset.
            self.font_list_mut(kind).reset();
            self.font_menu_mut(kind).open(());
            window.focus(&self.font_search.read(cx).focus_handle(cx), cx);
        }
        cx.notify();
    }

    fn toggle_size_menu(&mut self, kind: FontKind, cx: &mut Context<Self>) {
        self.close_other_menus(None, Some(kind), cx);
        let just_dismissed = self
            .size_dismissed_at(kind)
            .take()
            .is_some_and(|at| at.elapsed() < std::time::Duration::from_millis(400));
        if self.size_menu(kind).is_open() {
            self.close_size_menu(kind, cx);
        } else if !just_dismissed {
            self.set_selected_size_ix(kind, kind.size_ix(cx));
            self.size_menu_mut(kind).open(());
        }
        cx.notify();
    }

    /// Open on the first navigation key, so ↑↓/Home/End work from the closed
    /// trigger exactly as they do inside the list.
    fn open_size_menu(&mut self, kind: FontKind, cx: &mut Context<Self>) {
        if !self.size_menu(kind).is_open() {
            *self.size_dismissed_at(kind) = None;
            self.toggle_size_menu(kind, cx);
        }
    }

    /// Returns whether the key was consumed. The card and its trigger both
    /// listen, and the trigger's guard re-reads `is_open` — which Enter has
    /// already flipped — so a consumed key must not bubble on.
    fn on_font_key_down(
        &mut self,
        kind: FontKind,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let key = event.keystroke.key.as_str();

        if !self.font_menu(kind).is_open() {
            // Closed: any list key opens the menu, and the filter takes over
            // from there — no stepping through hundreds of rows by hand.
            if matches!(
                key,
                "up" | "down" | "left" | "right" | "home" | "end" | "enter" | "space"
            ) {
                *self.font_dismissed_at(kind) = None;
                self.toggle_font_menu(kind, window, cx);
                return true;
            }
            return false;
        }

        // Open: the filter input owns text and caret keys. Only navigation,
        // commit, and dismiss bubble out to us.
        let availability = typography::availability(cx);
        let choices = self.visible_choices(kind, &availability, cx);
        let modifiers = event.keystroke.modifiers;
        let next = match (
            key,
            popover::classify_key(key, modifiers.platform, modifiers.control),
        ) {
            (_, popover::MenuKey::Up) => {
                step_font(self.selected_font(kind), -1, &choices, kind, &availability)
            }
            (_, popover::MenuKey::Down) => {
                step_font(self.selected_font(kind), 1, &choices, kind, &availability)
            }
            ("home", _) => first_available(&choices, kind, &availability),
            ("end", _) => last_available(&choices, kind, &availability),
            (_, popover::MenuKey::Enter) => {
                self.commit_font(kind, cx);
                return true;
            }
            (_, popover::MenuKey::Escape) => {
                let effective = kind.effective(cx);
                self.set_selected_font(kind, effective);
                self.close_font_menu(kind, cx);
                window.focus(&self.font_focus(kind).clone(), cx);
                cx.notify();
                return true;
            }
            _ => return false,
        };
        self.set_selected_font(kind, next);
        cx.notify();
        true
    }

    fn on_size_key_down(
        &mut self,
        kind: FontKind,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let last = kind.size_count() - 1;
        let current = self.selected_size_ix(kind);
        match event.keystroke.key.as_str() {
            "up" | "left" => {
                self.open_size_menu(kind, cx);
                self.set_selected_size_ix(kind, current.saturating_sub(1));
                cx.notify();
            }
            "down" | "right" => {
                self.open_size_menu(kind, cx);
                self.set_selected_size_ix(kind, current + 1);
                cx.notify();
            }
            "home" => {
                self.open_size_menu(kind, cx);
                self.set_selected_size_ix(kind, 0);
                cx.notify();
            }
            "end" => {
                self.open_size_menu(kind, cx);
                self.set_selected_size_ix(kind, last);
                cx.notify();
            }
            "enter" | "space" => {
                if self.size_menu(kind).is_open() {
                    self.commit_size(kind, window, cx);
                } else {
                    *self.size_dismissed_at(kind) = None;
                    self.toggle_size_menu(kind, cx);
                }
            }
            "escape" => {
                self.set_selected_size_ix(kind, kind.size_ix(cx));
                self.close_size_menu(kind, cx);
                cx.notify();
            }
            _ => {}
        }
    }

    fn open_import(&mut self, cx: &mut Context<Self>) {
        let input = cx.new(|cx| {
            ComposerInput::with_context(
                i18n::translate(MessageId::ThemeImportSourcePlaceholder, i18n::locale(cx)),
                "PaletteSearch",
                cx,
            )
        });
        let events = cx.subscribe(&input, |this: &mut Self, _, event, cx| match event {
            ComposerInputEvent::Edited => {
                let source = this
                    .import_dialog
                    .as_ref()
                    .map(|dialog| PathBuf::from(dialog.input.read(cx).text().trim()));
                if let Some(dialog) = this.import_dialog.as_mut()
                    && dialog
                        .compilation
                        .as_ref()
                        .zip(source.as_ref())
                        .is_some_and(|(compilation, source)| compilation.path != *source)
                {
                    dialog.compilation = None;
                    dialog.selected.clear();
                    dialog.review_variant = None;
                    dialog.error = None;
                    cx.notify();
                }
            }
            ComposerInputEvent::Submitted => {
                if this
                    .import_dialog
                    .as_ref()
                    .is_some_and(|dialog| dialog.compilation.is_some())
                {
                    this.finish_import(cx);
                } else {
                    this.compile_import(cx);
                }
            }
            _ => {}
        });
        self.import_dialog = Some(ImportDialog {
            input,
            _events: events,
            focus: cx.focus_handle(),
            focus_pending: true,
            mode: InstallMode::Snapshot,
            compilation: None,
            selected: HashSet::new(),
            review_variant: None,
            error: None,
        });
        cx.notify();
    }

    fn compile_import(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.import_dialog.as_mut() else {
            return;
        };
        let source = dialog.input.read(cx).text().trim().to_owned();
        if source.is_empty() {
            dialog.error = Some(
                i18n::translate(MessageId::ThemeImportSourceRequired, i18n::locale(cx)).into(),
            );
            cx.notify();
            return;
        }
        let path = PathBuf::from(&source);
        let family_name = source_name(&path);
        let family_id = format!("custom-{}", slug(&family_name));
        match theme_library::compile(&path, &family_id, &family_name) {
            Ok(compilation) => {
                dialog.selected = compilation
                    .family
                    .variants
                    .iter()
                    .map(|variant| variant.id.clone())
                    .collect();
                // Mapping diagnostics are useful, but they are an advanced
                // inspection surface rather than part of the happy path.
                dialog.review_variant = None;
                dialog.compilation = Some(compilation);
                dialog.error = None;
            }
            Err(error) => {
                dialog.error = Some(theme_library::error_text(&error, i18n::locale(cx)).into())
            }
        }
        cx.notify();
    }

    fn choose_import_source(&mut self, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: true,
            multiple: false,
            prompt: Some(
                i18n::translate(MessageId::ThemeImportChooseSource, i18n::locale(cx)).into(),
            ),
        });
        cx.spawn(async move |this, cx| {
            let path = match receiver.await {
                Ok(Ok(Some(mut paths))) => paths.pop(),
                _ => None,
            };
            let Some(path) = path else {
                return;
            };
            let _ = this.update(cx, |page, cx| {
                if let Some(dialog) = page.import_dialog.as_mut() {
                    dialog.input.update(cx, |input, cx| {
                        input.set_text(path.display().to_string(), cx)
                    });
                }
                page.compile_import(cx);
            });
        })
        .detach();
    }

    fn choose_new_thread_background(&mut self, cx: &mut Context<Self>) {
        self.background_error = None;
        let receiver = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(
                i18n::translate(MessageId::ThemeImportChooseBackground, i18n::locale(cx)).into(),
            ),
        });
        cx.spawn(async move |this, cx| {
            let path = match receiver.await {
                Ok(Ok(Some(mut paths))) => paths.pop(),
                _ => None,
            };
            let Some(path) = path else {
                return;
            };
            let _ = this.update(cx, |page, cx| {
                page.background_error =
                    crate::settings::install_new_thread_composer_background(&path, cx)
                        .err()
                        .map(SharedString::from);
                cx.notify();
            });
        })
        .detach();
    }

    fn remove_new_thread_background(&mut self, cx: &mut Context<Self>) {
        self.background_error = crate::settings::remove_new_thread_composer_background(cx)
            .err()
            .map(SharedString::from);
        cx.notify();
    }

    fn finish_import(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.import_dialog.as_mut() else {
            return;
        };
        if dialog.selected.is_empty() {
            dialog.error =
                Some(i18n::translate(MessageId::ThemeImportSelectVariant, i18n::locale(cx)).into());
            cx.notify();
            return;
        }
        let Some(compilation) = dialog.compilation.take() else {
            return;
        };
        let selected = dialog.selected.iter().cloned().collect::<Vec<_>>();
        match theme_library::install(compilation.clone(), &selected, dialog.mode, cx) {
            Ok(_) => self.import_dialog = None,
            Err(error) => {
                dialog.compilation = Some(compilation);
                dialog.error = Some(theme_library::error_text(&error, i18n::locale(cx)).into());
            }
        }
        cx.notify();
    }

    fn on_scroll_hovered(&mut self, hovered: &bool, _: &mut Window, cx: &mut Context<Self>) {
        if self.scroll.set_list_hovered(*hovered) {
            cx.notify();
        }
    }
}

impl popover::ScrollRailHost for AppearancePage {
    // The page's rail; each font dropdown's rail goes through
    // [`widgets::rail`], which can serve further scroll hosts on the same view.
    fn rail_bar(&mut self) -> &mut popover::MenuScrollbarState {
        self.scroll.rail_bar()
    }

    fn rail_scroll(&self) -> Option<gpui::ScrollHandle> {
        self.scroll.rail_scroll()
    }
}

fn source_name(path: &Path) -> String {
    let path = if path.file_name().and_then(|name| name.to_str()) == Some("package.json") {
        path.parent().unwrap_or(path)
    } else {
        path
    };
    path.file_stem()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        // Untranslated on purpose: this name is stored with the imported theme
        // and `family_id` is derived from it, so it identifies data rather than
        // rendering copy.
        .unwrap_or_else(|| "Custom theme".to_owned())
}

fn slug(value: &str) -> String {
    let mut result = String::new();
    let mut separator = false;
    for character in value.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            if separator && !result.is_empty() {
                result.push('-');
            }
            result.push(character);
            separator = false;
        } else {
            separator = true;
        }
    }
    if result.is_empty() {
        "theme".into()
    } else {
        result
    }
}

fn step_font(
    current: &UiFontFamily,
    delta: isize,
    choices: &[UiFontFamily],
    kind: FontKind,
    availability: &FontAvailability,
) -> UiFontFamily {
    if choices.is_empty() {
        return current.clone();
    }
    let current = choices
        .iter()
        .position(|family| family == current)
        .unwrap_or_default() as isize;
    let mut ix = current + delta.signum();
    while (0..choices.len() as isize).contains(&ix) {
        let candidate = &choices[ix as usize];
        if kind.is_available_for(availability, candidate) {
            return candidate.clone();
        }
        ix += delta.signum();
    }
    choices[current as usize].clone()
}

/// Narrow and rank families by a typed query: prefix matches first, then
/// substring matches, catalog order preserved within each rank. An empty query
/// keeps the catalog untouched, so bundled entries still sort first.
fn filter_families(query: &str, choices: &[UiFontFamily]) -> Vec<UiFontFamily> {
    if query.trim().is_empty() {
        return choices.to_vec();
    }
    let labels: Vec<&str> = choices.iter().map(UiFontFamily::label).collect();
    popover::filter_indices(query, &labels)
        .into_iter()
        .map(|ix| choices[ix].clone())
        .collect()
}

fn first_available(
    choices: &[UiFontFamily],
    kind: FontKind,
    availability: &FontAvailability,
) -> UiFontFamily {
    choices
        .iter()
        .find(|family| kind.is_available_for(availability, family))
        .cloned()
        .unwrap_or_else(|| fallback_selection(kind))
}

fn last_available(
    choices: &[UiFontFamily],
    kind: FontKind,
    availability: &FontAvailability,
) -> UiFontFamily {
    choices
        .iter()
        .rev()
        .find(|family| kind.is_available_for(availability, family))
        .cloned()
        .unwrap_or_else(|| fallback_selection(kind))
}

/// Highlight target when a kind's catalog offers nothing: System UI is
/// proportional, so the terminal cannot land there.
fn fallback_selection(kind: FontKind) -> UiFontFamily {
    match kind {
        FontKind::Terminal => UiFontFamily::GeistMono,
        _ => UiFontFamily::System,
    }
}

fn format_px(size: f32) -> String {
    if size.fract().abs() < f32::EPSILON {
        format!("{size:.0} px")
    } else {
        format!("{size:.1} px")
    }
}

fn bar(fraction: f32, tone: Hsla) -> gpui::Div {
    div()
        .h(px(5.0))
        .w(gpui::relative(fraction))
        .rounded(px(3.0))
        .bg(tone)
}

fn accent_helper(accent: AccentSelection, locale: Locale) -> String {
    match accent {
        AccentSelection::ThemeDefault => {
            i18n::translate(MessageId::AccentHelperDefault, locale).to_owned()
        }
        AccentSelection::Preset(preset) => i18n::fill(
            MessageId::AccentHelperPreset,
            "{accent}",
            preset.label(),
            locale,
        ),
    }
}

fn surface_label(surface: SurfacePreference, locale: Locale) -> &'static str {
    i18n::translate(
        match surface {
            SurfacePreference::ThemeDefault => MessageId::SurfaceThemeDefault,
            SurfacePreference::Frosted => MessageId::SurfaceFrosted,
            SurfacePreference::Opaque => MessageId::SurfaceOpaque,
        },
        locale,
    )
}

/// Stable element id for a surface choice. The label is translated, so the id
/// cannot be derived from it.
fn surface_slug(surface: SurfacePreference) -> &'static str {
    match surface {
        SurfacePreference::ThemeDefault => "theme-default",
        SurfacePreference::Frosted => "frosted",
        SurfacePreference::Opaque => "opaque",
    }
}

fn surface_helper(
    surface: SurfacePreference,
    resolved: SurfaceTreatment,
    locale: Locale,
) -> String {
    match surface {
        SurfacePreference::ThemeDefault => i18n::fill(
            MessageId::SurfaceHelperThemeDefault,
            "{treatment}",
            match resolved {
                SurfaceTreatment::Frosted => {
                    i18n::translate(MessageId::SurfaceTreatmentFrosted, locale)
                }
                SurfaceTreatment::Opaque => {
                    i18n::translate(MessageId::SurfaceTreatmentOpaque, locale)
                }
            },
            locale,
        ),
        SurfacePreference::Frosted => {
            i18n::translate(MessageId::SurfaceHelperFrosted, locale).to_owned()
        }
        SurfacePreference::Opaque => {
            i18n::translate(MessageId::SurfaceHelperOpaque, locale).to_owned()
        }
    }
}

/// Message id behind one Appearance mode card.
fn mode_message(mode: AppearanceMode) -> MessageId {
    match mode {
        AppearanceMode::System => MessageId::AppearanceModeSystem,
        AppearanceMode::Light => MessageId::AppearanceModeLight,
        AppearanceMode::Dark => MessageId::AppearanceModeDark,
    }
}

fn language_selector(
    page: &mut AppearancePage,
    theme: &Theme,
    current: LanguagePreference,
    locale: Locale,
    cx: &mut Context<AppearancePage>,
) -> AnyElement {
    let open = page.language_menu.is_open();
    let closing = page.language_menu.closing_since();
    let menu = popover::popover_card(theme)
        .w(px(204.0))
        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
            this.close_language_menu(cx);
            cx.notify();
        }))
        .flex()
        .flex_col()
        .gap(px(2.0))
        .children(LanguagePreference::ALL.into_iter().map(|preference| {
            let active = preference == current;
            popover::menu_row(
                theme,
                active,
                SharedString::from(format!("appearance-language-menu-{}", preference.id())),
            )
            .id(SharedString::from(format!(
                "appearance-language-row-{}",
                preference.id()
            )))
            .on_click(cx.listener(move |this, _, _, cx| {
                i18n::set_preference(preference, cx);
                this.close_language_menu(cx);
                cx.notify();
            }))
            .child(
                div()
                    .flex_1()
                    .child(i18n::preference_label(preference, locale)),
            )
            .when(active, |row| {
                row.child(
                    icons::icon(icons::CHECK)
                        .size(px(14.0))
                        .text_color(theme.accent),
                )
            })
        }))
        .into_any_element();

    div()
        .id("appearance-language-dropdown")
        .relative()
        .w(px(204.0))
        .h(px(36.0))
        .px(px(11.0))
        .rounded(px(9.0))
        .border_1()
        .border_color(if open {
            theme.border_strong
        } else {
            theme.border
        })
        .bg(crate::theme::ink(0.025))
        .flex()
        .items_center()
        .gap(px(8.0))
        .cursor_pointer()
        .on_click(cx.listener(|this, _, _, cx| this.toggle_language_menu(cx)))
        .child(
            div()
                .flex_1()
                .child(i18n::preference_label(current, locale)),
        )
        .child(
            icons::icon(icons::ALT_ARROW_DOWN)
                .size(px(14.0))
                .flex_none()
                .text_color(theme.text_muted),
        )
        .when_some(page.language_menu.get(), |trigger, _| {
            trigger.child(popover::anchored_menu_below(
                "appearance-language-menu",
                menu,
                closing,
            ))
        })
        .into_any_element()
}

fn surface_choice(
    theme: &Theme,
    surface: SurfacePreference,
    locale: Locale,
    selected: bool,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(SharedString::from(format!(
            "appearance-surface-{}",
            surface_slug(surface)
        )))
        .h(px(30.0))
        .px(px(10.0))
        .rounded(px(7.0))
        .border_1()
        .border_color(if selected { theme.accent } else { theme.border })
        .bg(if selected {
            theme.accent_wash
        } else {
            theme.surface_raised.opacity(0.28)
        })
        .text_size(crate::typography::ui_rems(11.5))
        .font_weight(if selected {
            gpui::FontWeight::MEDIUM
        } else {
            gpui::FontWeight::NORMAL
        })
        .text_color(if selected {
            theme.accent
        } else {
            theme.text_muted
        })
        .flex()
        .items_center()
        .cursor_pointer()
        .when(!selected, |control| {
            control.hover(|style| style.bg(theme.surface_raised_hover))
        })
        .child(surface_label(surface, locale))
}

fn background_effect_choice(
    theme: &Theme,
    effect: crate::settings::NewThreadBackgroundEffect,
    selected: bool,
    locale: Locale,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(SharedString::from(format!(
            "new-thread-background-effect-{}",
            effect.slug()
        )))
        .h(px(28.0))
        .px(px(9.0))
        .rounded(px(7.0))
        .border_1()
        .border_color(if selected { theme.accent } else { theme.border })
        .bg(if selected {
            theme.accent_wash
        } else {
            theme.surface_raised.opacity(0.28)
        })
        .text_size(crate::typography::ui_rems(11.0))
        .font_weight(if selected {
            gpui::FontWeight::MEDIUM
        } else {
            gpui::FontWeight::NORMAL
        })
        .text_color(if selected {
            theme.accent
        } else {
            theme.text_muted
        })
        .flex()
        .items_center()
        .cursor_pointer()
        .when(!selected, |control| {
            control.hover(|style| style.bg(theme.surface_raised_hover))
        })
        .child(i18n::translate(effect.label_message(), locale))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Corners {
    All,
    Left,
    Right,
}

fn miniature(theme: &Theme, corners: Corners) -> AnyElement {
    let line = theme.text.opacity(0.22);
    let strong = theme.text.opacity(0.34);
    let r = px(widgets::OPTION_CARD_RADIUS);
    let root = div().size_full().flex().flex_row().bg(theme.surface);
    let root = match corners {
        Corners::All => root.rounded(r),
        Corners::Left => root.rounded_tl(r).rounded_bl(r),
        Corners::Right => root.rounded_tr(r).rounded_br(r),
    };
    root.child(
        div()
            .w(px(44.0))
            .h_full()
            .flex_none()
            .overflow_hidden()
            .flex()
            .flex_col()
            .gap(px(7.0))
            .px(px(8.0))
            .pt(px(14.0))
            .child(bar(0.70, strong))
            .child(bar(1.0, line))
            .child(bar(0.85, line))
            .child(bar(1.0, line)),
    )
    .child(
        div()
            .flex_1()
            .min_w_0()
            .my(px(8.0))
            .mr(px(8.0))
            .rounded(px(6.0))
            .border_1()
            .border_color(theme.border)
            .bg(theme.bg)
            .overflow_hidden()
            .flex()
            .flex_col()
            .gap(px(7.0))
            .p(px(10.0))
            .child(bar(0.62, strong))
            .child(bar(0.88, line))
            .child(bar(0.76, line))
            .child(bar(0.52, line)),
    )
    .into_any_element()
}

fn miniature_split(
    themes: &ThemeSelection,
    accent: AccentSelection,
    surface: SurfacePreference,
) -> AnyElement {
    let light = Theme::for_selection(Appearance::Light, &themes.light, accent, surface);
    let dark = Theme::for_selection(Appearance::Dark, &themes.dark, accent, surface);
    div()
        .size_full()
        .flex()
        .flex_row()
        .child(
            div()
                .w_1_2()
                .h_full()
                .overflow_hidden()
                .child(miniature(&light, Corners::Left)),
        )
        .child(
            div()
                .w_1_2()
                .h_full()
                .overflow_hidden()
                .child(miniature(&dark, Corners::Right)),
        )
        .into_any_element()
}

fn preview(
    mode: AppearanceMode,
    themes: &ThemeSelection,
    accent: AccentSelection,
    surface: SurfacePreference,
) -> AnyElement {
    match mode {
        AppearanceMode::System => miniature_split(themes, accent, surface),
        AppearanceMode::Light => miniature(
            &Theme::for_selection(Appearance::Light, &themes.light, accent, surface),
            Corners::All,
        ),
        AppearanceMode::Dark => miniature(
            &Theme::for_selection(Appearance::Dark, &themes.dark, accent, surface),
            Corners::All,
        ),
    }
}

fn model_appearance(appearance: Appearance) -> zeron_theme::Appearance {
    match appearance {
        Appearance::Dark => zeron_theme::Appearance::Dark,
        Appearance::Light => zeron_theme::Appearance::Light,
    }
}

fn palette_preview(theme: &Theme) -> gpui::Div {
    div()
        .flex_none()
        .w(px(30.0))
        .h(px(18.0))
        .rounded(px(5.0))
        .overflow_hidden()
        .border_1()
        .border_color(theme.border)
        .flex()
        .child(div().w_1_3().h_full().bg(theme.surface))
        .child(div().w_1_3().h_full().bg(theme.bg))
        .child(div().w_1_3().h_full().bg(theme.accent))
}

fn compact_action(
    theme: &Theme,
    label: &str,
    id: impl Into<SharedString>,
) -> gpui::Stateful<gpui::Div> {
    let id = id.into();
    popover::btn_ghost(theme, label, id.clone())
        .id(id)
        .h(px(28.0))
        .px(px(9.0))
        .py(px(0.0))
        .rounded(px(7.0))
        .border_1()
        .border_color(theme.border)
        .bg(theme.surface_raised.opacity(0.34))
        .flex()
        .items_center()
        .text_size(crate::typography::ui_rems(11.5))
}

fn import_scene_preview(variant: &zeron_theme::ThemeVariant) -> AnyElement {
    let theme = Theme::from_variant(
        variant,
        AccentSelection::ThemeDefault,
        SurfacePreference::ThemeDefault,
    );
    div()
        .w_full()
        .h(px(86.0))
        .flex()
        .gap(px(8.0))
        .child(
            div()
                .w(px(152.0))
                .h_full()
                .overflow_hidden()
                .rounded(px(8.0))
                .border_1()
                .border_color(theme.border)
                .child(miniature(&theme, Corners::All)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .h_full()
                .rounded(px(8.0))
                .border_1()
                .border_color(theme.border)
                .bg(theme.bg)
                .p(px(9.0))
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(
                    div()
                        .text_size(crate::typography::ui_rems(10.0))
                        .font_family(theme.font_mono.clone())
                        .child(
                            div()
                                .text_color(theme.syntax.keyword)
                                .child("fn ")
                                .child(div().text_color(theme.syntax.function).child("preview"))
                                .child(div().text_color(theme.syntax.punctuation).child("() {")),
                        ),
                )
                .child(
                    div()
                        .text_size(crate::typography::ui_rems(10.0))
                        .font_family(theme.font_mono.clone())
                        .text_color(theme.syntax.string)
                        .child("  \"Theme mapping\""),
                )
                .child(
                    div()
                        .mt_auto()
                        .h(px(12.0))
                        .flex()
                        .rounded(px(3.0))
                        .overflow_hidden()
                        .children(
                            theme
                                .terminal
                                .ansi
                                .iter()
                                .take(8)
                                .map(|color| div().flex_1().h_full().bg(*color)),
                        ),
                ),
        )
        .child(
            div()
                .w(px(84.0))
                .h_full()
                .rounded(px(8.0))
                .border_1()
                .border_color(theme.border)
                .bg(theme.surface)
                .p(px(8.0))
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(
                    div()
                        .h(px(12.0))
                        .rounded(px(3.0))
                        .bg(theme.diff_add.opacity(0.35)),
                )
                .child(
                    div()
                        .h(px(12.0))
                        .rounded(px(3.0))
                        .bg(theme.diff_del.opacity(0.35)),
                )
                .child(div().h(px(12.0)).rounded(px(3.0)).bg(theme.accent_wash)),
        )
        .into_any_element()
}

fn report_panel(theme: &Theme, report: &ImportReport, locale: Locale) -> gpui::Stateful<gpui::Div> {
    let summary = i18n::fill_many(
        MessageId::ThemeReportSummary,
        &[
            ("{mapped}", &report.mappings.len().to_string()),
            ("{adjusted}", &report.adjustments.len().to_string()),
            ("{inferred}", &report.fallbacks.len().to_string()),
            ("{unsupported}", &report.dropped.len().to_string()),
            ("{warnings}", &report.warnings.len().to_string()),
            ("{validation}", &report.validation.len().to_string()),
        ],
        locale,
    );
    div()
        .id(SharedString::from(format!(
            "theme-report-{}",
            report.source_hash
        )))
        .mt(px(8.0))
        .w_full()
        .max_h(px(168.0))
        .overflow_y_scroll()
        .rounded(px(8.0))
        .border_1()
        .border_color(theme.border)
        .bg(theme.surface_raised.opacity(0.35))
        .p(px(10.0))
        .text_size(crate::typography::ui_rems(11.0))
        .line_height(px(16.0))
        .text_color(theme.text_muted)
        .child(div().text_color(theme.text).child(summary))
        .children(report.adjustments.iter().map(|adjustment| {
            div().mt(px(4.0)).child(SharedString::from(i18n::fill_many(
                MessageId::ThemeReportAdjusted,
                &[
                    ("{role}", &adjustment.zeron_role),
                    ("{original}", &adjustment.original),
                    ("{resolved}", &adjustment.resolved),
                    ("{reason}", &adjustment.reason),
                ],
                locale,
            )))
        }))
        .children(report.fallbacks.iter().map(|message| {
            div().mt(px(4.0)).child(SharedString::from(i18n::fill(
                MessageId::ThemeReportFallback,
                "{message}",
                message,
                locale,
            )))
        }))
        .children(report.warnings.iter().map(|message| {
            div().mt(px(4.0)).child(SharedString::from(i18n::fill(
                MessageId::ThemeReportWarning,
                "{message}",
                message,
                locale,
            )))
        }))
        .children(report.validation.iter().map(|issue| {
            div().mt(px(4.0)).child(SharedString::from(i18n::fill_many(
                MessageId::ThemeReportValidation,
                &[
                    ("{category}", category_label(issue.category, locale)),
                    ("{severity}", severity_label(issue.severity, locale)),
                    ("{message}", &issue.message),
                ],
                locale,
            )))
        }))
        .children(report.dropped.iter().map(|message| {
            div().mt(px(4.0)).child(SharedString::from(i18n::fill(
                MessageId::ThemeReportUnsupported,
                "{message}",
                message,
                locale,
            )))
        }))
        .children(report.mappings.iter().map(|mapping| {
            div().mt(px(4.0)).child(SharedString::from(format!(
                "{} ← {}",
                mapping.zeron_role, mapping.vscode_key
            )))
        }))
}

/// The report's enum labels, named instead of `Debug`-formatted so the panel
/// reads as copy in both locales.
fn category_label(category: ValidationCategory, locale: Locale) -> &'static str {
    i18n::translate(
        match category {
            ValidationCategory::Structural => MessageId::ThemeReportCategoryStructural,
            ValidationCategory::Contrast => MessageId::ThemeReportCategoryContrast,
        },
        locale,
    )
}

fn severity_label(severity: ValidationSeverity, locale: Locale) -> &'static str {
    i18n::translate(
        match severity {
            ValidationSeverity::Warning => MessageId::ThemeReportSeverityWarning,
            ValidationSeverity::Error => MessageId::ThemeReportSeverityError,
        },
        locale,
    )
}

fn accent_swatch(
    page_theme: &Theme,
    selection: AccentSelection,
    selected: bool,
) -> gpui::Stateful<gpui::Div> {
    let swatch_theme = Theme::for_selection(
        page_theme.appearance,
        page_theme.variant_id.as_ref(),
        selection,
        page_theme.surface_preference,
    );
    let sample = match selection {
        AccentSelection::ThemeDefault => div()
            .size_full()
            .rounded(px(6.0))
            .bg(swatch_theme.accent_wash)
            .flex()
            .items_center()
            .justify_center()
            .gap(px(2.0))
            .child(
                div()
                    .w(px(4.0))
                    .h(px(13.0))
                    .rounded(px(2.0))
                    .bg(swatch_theme.glyph.light),
            )
            .child(
                div()
                    .w(px(4.0))
                    .h(px(16.0))
                    .rounded(px(2.0))
                    .bg(swatch_theme.glyph.mid),
            )
            .child(
                div()
                    .w(px(4.0))
                    .h(px(11.0))
                    .rounded(px(2.0))
                    .bg(swatch_theme.glyph.deep),
            ),
        AccentSelection::Preset(_) => div().size_full().rounded(px(6.0)).bg(swatch_theme.accent),
    };
    div()
        .id(SharedString::from(format!("accent-{}", selection.label())))
        .flex_none()
        .w(px(30.0))
        .h(px(34.0))
        .pb(px(4.0))
        .border_b_2()
        .border_color(if selected {
            swatch_theme.accent
        } else {
            gpui::transparent_black()
        })
        .cursor_pointer()
        .child(
            div()
                .size(px(30.0))
                .p(px(2.0))
                .rounded(px(8.0))
                .border_1()
                .border_color(if selected {
                    page_theme.border_strong
                } else {
                    page_theme.border
                })
                .bg(page_theme.surface_raised.opacity(0.42))
                .child(sample),
        )
}

impl AppearancePage {
    fn theme_menu(&self, appearance: Appearance) -> &Popup<()> {
        match appearance {
            Appearance::Light => &self.light_theme_menu,
            Appearance::Dark => &self.dark_theme_menu,
        }
    }

    fn theme_menu_mut(&mut self, appearance: Appearance) -> &mut Popup<()> {
        match appearance {
            Appearance::Light => &mut self.light_theme_menu,
            Appearance::Dark => &mut self.dark_theme_menu,
        }
    }

    fn close_theme_menu(&mut self, appearance: Appearance, cx: &mut Context<Self>) {
        if !self.theme_menu_mut(appearance).begin_close() {
            return;
        }
        match appearance {
            Appearance::Light => popover::reap_popup(cx, |page| &mut page.light_theme_menu),
            Appearance::Dark => popover::reap_popup(cx, |page| &mut page.dark_theme_menu),
        }
    }

    fn render_font_picker(
        &mut self,
        kind: FontKind,
        theme: &Theme,
        availability: &FontAvailability,
        fixed: SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let slug = kind.slug();
        let locale = i18n::locale(cx);
        // The scroll helpers key off `&'static str`, so the ids are spelled
        // out rather than formatted from the slug.
        let (host_id, list_id, rail_id) = match kind {
            FontKind::Ui => (
                "interface-font-host",
                "interface-font-scroll",
                "interface-font-scrollbar",
            ),
            FontKind::Terminal => (
                "terminal-font-host",
                "terminal-font-scroll",
                "terminal-font-scrollbar",
            ),
            FontKind::Code => ("code-font-host", "code-font-scroll", "code-font-scrollbar"),
        };
        let effective = kind.effective(cx);
        let selected = self.selected_font(kind).clone();
        let visible = self.visible_choices(kind, availability, cx);
        let filtered = !self.font_search.read(cx).text().trim().is_empty();
        let rows: Vec<AnyElement> = visible
            .into_iter()
            .enumerate()
            .map(|(ix, family)| {
                let available = kind.is_available_for(availability, &family);
                let active = family == effective;
                let focused = family == selected;
                let label = SharedString::from(family.label().to_owned());
                popover::menu_row_nav(theme, active, focused, format!("{slug}-font-option-{ix}"))
                    .id(SharedString::from(format!("{slug}-font-option-{ix}")))
                    .when(available, |row| {
                        row.on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.set_selected_font(kind, family.clone());
                            this.commit_font(kind, cx);
                        }))
                    })
                    .when(!available, |row| row.opacity(0.45))
                    .child(div().flex_1().min_w_0().truncate().child(label))
                    .child(div().w(px(18.0)).flex_none().when(active, |slot| {
                        slot.child(
                            icons::icon(icons::CHECK)
                                .size(px(14.0))
                                .text_color(theme.accent),
                        )
                    }))
                    .into_any_element()
            })
            .collect();

        let list: AnyElement = if rows.is_empty() {
            div()
                .px(px(8.0))
                .py(px(6.0))
                .text_size(px(12.0))
                .text_color(theme.for_popup().text_faint)
                .child(SharedString::from(if filtered {
                    i18n::translate(MessageId::FontPickerNoMatches, locale)
                } else {
                    i18n::translate(MessageId::FontPickerNoFonts, locale)
                }))
                .into_any_element()
        } else {
            // Card-bleed scroll host (see [`popover::menu_scroll_host`]): the
            // rail mounts as a sibling of the scroller, above its clip. The
            // bleed stays horizontal — the search input sits above the list.
            let rail = widgets::rail(self.font_list_mut(kind), rail_id, theme, cx, move |page| {
                page.font_list_mut(kind)
            });
            let scroll = self.font_list_mut(kind).scroll.clone();
            popover::menu_scroll_host(host_id)
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    if this.font_list_mut(kind).set_list_hovered(*hovered) {
                        cx.notify();
                    }
                }))
                .child(
                    popover::menu_scroll_list(list_id, &scroll)
                        .max_h(px(280.0))
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .children(rows),
                )
                .children(rail)
                .into_any_element()
        };

        // The key handler sits on the card, not just the trigger: focus moves
        // into the filter input, and `PaletteSearch` lets arrows/Enter/Escape
        // bubble to exactly this ancestor.
        let menu = popover::popover_card(theme)
            .w(px(220.0))
            .font_family(fixed)
            .on_mouse_down_out(cx.listener(move |this, _, _, cx| this.dismiss_font_menu(kind, cx)))
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                if this.on_font_key_down(kind, event, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .flex()
            .flex_col()
            .child(popover::search_input_frame(
                theme,
                self.font_search.clone().into_any_element(),
            ))
            .child(list)
            .into_any_element();

        let open = self.font_menu(kind).is_open();
        let closing = self.font_menu(kind).closing_since();
        div()
            .id(SharedString::from(format!("{slug}-font-dropdown")))
            .relative()
            .w(px(220.0))
            .h(px(36.0))
            .px(px(11.0))
            .rounded(px(9.0))
            .border_1()
            .border_color(if open {
                theme.border_strong
            } else {
                theme.border
            })
            .bg(crate::theme::ink(0.025))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .cursor_pointer()
            .track_focus(self.font_focus(kind))
            // Only the closed state: once open, the card above owns these keys
            // and stops their propagation before they reach us.
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                if !this.font_menu(kind).is_open() {
                    this.on_font_key_down(kind, event, window, cx);
                }
            }))
            .on_click(cx.listener(move |this, _, window, cx| {
                window.focus(&this.font_focus(kind).clone(), cx);
                this.toggle_font_menu(kind, window, cx);
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(SharedString::from(effective.label().to_owned())),
            )
            .child(
                icons::icon(icons::ALT_ARROW_DOWN)
                    .size(px(14.0))
                    .flex_none()
                    .text_color(theme.text_muted),
            )
            .when_some(self.font_menu(kind).get(), |trigger, _| {
                trigger.child(popover::anchored_menu_below(
                    SharedString::from(format!("{slug}-font-menu")),
                    menu,
                    closing,
                ))
            })
            .into_any_element()
    }

    /// The size dropdown, identical in shape to the family picker beside it.
    /// Every kind picks from a discrete ladder, so all three rows read alike.
    fn render_size_picker(
        &mut self,
        kind: FontKind,
        theme: &Theme,
        fixed: SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let slug = kind.slug();
        let labels = kind.size_labels();
        let current = kind.size_ix(cx);
        let selected = self.selected_size_ix(kind);
        let rows: Vec<AnyElement> = labels
            .iter()
            .enumerate()
            .map(|(ix, label)| {
                popover::menu_row_nav(
                    theme,
                    ix == current,
                    ix == selected,
                    format!("{slug}-font-size-option-{ix}"),
                )
                .id(SharedString::from(format!("{slug}-font-size-option-{ix}")))
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.set_selected_size_ix(kind, ix);
                    this.commit_size(kind, window, cx);
                }))
                .child(div().flex_1().child(label.clone()))
                .child(div().w(px(18.0)).flex_none().when(ix == current, |slot| {
                    slot.child(
                        icons::icon(icons::CHECK)
                            .size(px(14.0))
                            .text_color(theme.accent),
                    )
                }))
                .into_any_element()
            })
            .collect();

        let menu = popover::popover_card(theme)
            .w(px(128.0))
            .font_family(fixed)
            .on_mouse_down_out(cx.listener(move |this, _, _, cx| this.dismiss_size_menu(kind, cx)))
            .flex()
            .flex_col()
            .gap(px(2.0))
            .children(rows)
            .into_any_element();

        let open = self.size_menu(kind).is_open();
        let closing = self.size_menu(kind).closing_since();
        div()
            .id(SharedString::from(format!("{slug}-font-size-dropdown")))
            .relative()
            .w(px(128.0))
            .h(px(36.0))
            .px(px(11.0))
            .rounded(px(9.0))
            .border_1()
            .border_color(if open {
                theme.border_strong
            } else {
                theme.border
            })
            .bg(crate::theme::ink(0.025))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .cursor_pointer()
            .track_focus(self.size_focus(kind))
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                this.on_size_key_down(kind, event, window, cx)
            }))
            .on_click(cx.listener(move |this, _, window, cx| {
                window.focus(&this.size_focus(kind).clone(), cx);
                this.toggle_size_menu(kind, cx);
            }))
            .child(div().flex_1().child(labels[current].clone()))
            .child(
                icons::icon(icons::ALT_ARROW_DOWN)
                    .size(px(14.0))
                    .flex_none()
                    .text_color(theme.text_muted),
            )
            .when_some(self.size_menu(kind).get(), |trigger, _| {
                trigger.child(popover::anchored_menu_below(
                    SharedString::from(format!("{slug}-font-size-menu")),
                    menu,
                    closing,
                ))
            })
            .into_any_element()
    }

    fn render_theme_selector(
        &mut self,
        appearance_kind: Appearance,
        selections: &ThemeSelection,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let registry = ThemeRegistry::active();
        let selected_id = selections
            .variant_id(model_appearance(appearance_kind))
            .to_owned();
        let selected_variant = registry
            .variant(&selected_id)
            .or_else(|| {
                registry
                    .variants_for(model_appearance(appearance_kind))
                    .next()
            })
            .expect("the built-in registry has both appearances");
        let selected_theme = Theme::for_selection(
            appearance_kind,
            &selected_variant.id,
            AccentSelection::ThemeDefault,
            theme.surface_preference,
        );
        let open = self.theme_menu(appearance_kind).is_open();

        let mut trigger = div()
            .id(SharedString::from(format!(
                "{}-theme-selector",
                if appearance_kind.is_light() {
                    "light"
                } else {
                    "dark"
                }
            )))
            .relative()
            .flex_none()
            .w(px(218.0))
            .h(px(34.0))
            .px(px(10.0))
            .rounded(px(8.0))
            .border_1()
            .border_color(if open {
                theme.border_strong
            } else {
                theme.border
            })
            .bg(theme.surface_raised.opacity(if open { 0.75 } else { 0.42 }))
            .flex()
            .items_center()
            .gap(px(8.0))
            .cursor_pointer()
            .when(!open, |el| {
                el.hover(|style| style.bg(theme.surface_raised_hover))
            })
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, _, _, _| {
                    this.theme_menu_mut(appearance_kind).note_trigger_press();
                }),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                if this.theme_menu_mut(appearance_kind).take_press_was_open() {
                    this.close_theme_menu(appearance_kind, cx);
                } else {
                    let other = if appearance_kind.is_light() {
                        Appearance::Dark
                    } else {
                        Appearance::Light
                    };
                    this.close_theme_menu(other, cx);
                    this.theme_menu_mut(appearance_kind).open(());
                }
                cx.notify();
            }))
            .child(palette_preview(&selected_theme))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(crate::typography::ui_rems(12.5))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(theme.text)
                    .child(SharedString::from(selected_variant.name.clone())),
            )
            .child(
                icons::icon(icons::SORT_VERTICAL)
                    .size(px(14.0))
                    .text_color(theme.text_muted.opacity(if open { 0.9 } else { 0.45 })),
            );

        if self.theme_menu(appearance_kind).get().is_some() {
            let closing = self.theme_menu(appearance_kind).closing_since();
            let heading = i18n::translate(
                if appearance_kind.is_light() {
                    MessageId::ThemeMenuLightHeading
                } else {
                    MessageId::ThemeMenuDarkHeading
                },
                i18n::locale(cx),
            );
            let menu = popover::popover_card(theme)
                .w(px(260.0))
                .on_mouse_down_out(cx.listener(move |this, _, _, cx| {
                    this.close_theme_menu(appearance_kind, cx);
                    cx.notify();
                }))
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(popover::menu_heading(theme, heading))
                .children(
                    registry
                        .variants_for(model_appearance(appearance_kind))
                        .enumerate()
                        .map(|(index, variant)| {
                            let id = variant.id.clone();
                            let name = variant.name.clone();
                            let active = id == selected_id;
                            let sample = Theme::for_selection(
                                appearance_kind,
                                &id,
                                AccentSelection::ThemeDefault,
                                theme.surface_preference,
                            );
                            popover::menu_row(
                                theme,
                                active,
                                SharedString::from(format!(
                                    "appearance-theme-menu-{appearance_kind:?}-{index}"
                                )),
                            )
                            .id(SharedString::from(format!(
                                "appearance-theme-row-{appearance_kind:?}-{index}"
                            )))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                appearance::set_theme(appearance_kind, id.clone(), cx);
                                this.close_theme_menu(appearance_kind, cx);
                                cx.notify();
                            }))
                            .child(palette_preview(&sample))
                            .child(div().flex_1().min_w_0().truncate().child(name))
                            .when(active, |row| {
                                row.child(
                                    icons::icon(icons::CHECK)
                                        .size(px(14.0))
                                        .text_color(theme.accent),
                                )
                            })
                        }),
                )
                .into_any_element();
            trigger = trigger.child(popover::anchored_menu_below(
                SharedString::from(format!(
                    "appearance-{}-theme-menu",
                    if appearance_kind.is_light() {
                        "light"
                    } else {
                        "dark"
                    }
                )),
                menu,
                closing,
            ));
        }

        trigger.into_any_element()
    }

    fn render_import_dialog(
        &mut self,
        viewport: gpui::Size<gpui::Pixels>,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        {
            let dialog = self.import_dialog.as_mut()?;
            if std::mem::take(&mut dialog.focus_pending) {
                let input_focus = dialog.input.focus_handle(cx);
                window.focus(&input_focus, cx);
            }
        }
        let dialog = self.import_dialog.as_ref()?;
        let input = dialog.input.clone();
        let focus = dialog.focus.clone();
        let mode = dialog.mode;
        let compilation = dialog.compilation.clone();
        let selected = dialog.selected.clone();
        let review_variant = dialog.review_variant.clone();
        let error = dialog.error.clone();
        let ready = compilation.is_some() && !selected.is_empty();
        let hairline = crate::theme::hairline(0.08);
        let locale = i18n::locale(cx);

        let mode_control = |label: &'static str,
                            description: &'static str,
                            id_slug: &str,
                            value: InstallMode| {
            let active = mode == value;
            div()
                .id(SharedString::from(format!("theme-import-mode-{id_slug}")))
                .flex_1()
                .min_w_0()
                .p(px(10.0))
                .rounded(px(9.0))
                .border_1()
                .border_color(if active { theme.accent } else { theme.border })
                .bg(if active {
                    theme.accent_wash
                } else {
                    theme.surface_raised.opacity(0.28)
                })
                .cursor_pointer()
                .when(!active, |control| {
                    control.hover(|style| style.bg(theme.surface_raised_hover))
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(dialog) = this.import_dialog.as_mut() {
                        dialog.mode = value;
                    }
                    cx.notify();
                }))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(7.0))
                        .child(
                            div()
                                .size(px(16.0))
                                .rounded_full()
                                .border_1()
                                .border_color(if active {
                                    theme.accent
                                } else {
                                    theme.border_strong
                                })
                                .flex()
                                .items_center()
                                .justify_center()
                                .when(active, |dot| {
                                    dot.child(div().size(px(8.0)).rounded_full().bg(theme.accent))
                                }),
                        )
                        .child(
                            div()
                                .text_size(crate::typography::ui_rems(12.0))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(if active { theme.text } else { theme.text_muted })
                                .child(label),
                        ),
                )
                .child(
                    div()
                        .mt(px(4.0))
                        .ml(px(23.0))
                        .text_size(crate::typography::ui_rems(10.5))
                        .text_color(theme.text_muted)
                        .child(description),
                )
        };

        let section_label = |label: &'static str| {
            div()
                .mb(px(7.0))
                .text_size(crate::typography::ui_rems(11.0))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme.text_muted)
                .child(label)
        };

        let mut main = div()
            .id("theme-import-main")
            .max_h(px(520.0))
            .overflow_y_scroll()
            .px(px(20.0))
            .pb(px(18.0))
            .flex()
            .flex_col()
            .child(section_label(i18n::translate(
                MessageId::ThemeImportSource,
                locale,
            )))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        popover::dialog_field(input.into_any_element())
                            .flex_1()
                            .min_w_0()
                            .h(px(36.0))
                            .py(px(0.0))
                            .flex()
                            .items_center(),
                    )
                    .child(
                        compact_action(
                            theme,
                            i18n::translate(MessageId::CommonBrowse, locale),
                            "theme-import-browse",
                        )
                        .h(px(36.0))
                        .px(px(12.0))
                        .flex_none()
                        .on_click(cx.listener(|this, _, _, cx| this.choose_import_source(cx))),
                    ),
            )
            .child(
                div()
                    .mt(px(16.0))
                    .child(section_label(i18n::translate(
                        MessageId::ThemeImportKeepUpdated,
                        locale,
                    )))
                    .child(
                        div()
                            .flex()
                            .gap(px(8.0))
                            .child(mode_control(
                                i18n::translate(MessageId::ThemeImportModeCopy, locale),
                                i18n::translate(MessageId::ThemeImportModeCopyHint, locale),
                                "import-a-copy",
                                InstallMode::Snapshot,
                            ))
                            .child(mode_control(
                                i18n::translate(MessageId::ThemeImportModeLink, locale),
                                i18n::translate(MessageId::ThemeImportModeLinkHint, locale),
                                "link-to-source",
                                InstallMode::Link,
                            )),
                    ),
            );

        if let Some(ref compilation) = compilation {
            main = main.child(
                div()
                    .mt(px(18.0))
                    .pt(px(16.0))
                    .border_t_1()
                    .border_color(hairline)
                    .flex()
                    .items_baseline()
                    .justify_between()
                    .child(
                        section_label(i18n::translate(MessageId::ThemeImportDetected, locale))
                            .mb(px(0.0)),
                    )
                    .child(
                        div()
                            .text_size(crate::typography::ui_rems(10.5))
                            .text_color(theme.text_muted)
                            .child(SharedString::from(i18n::variants(
                                compilation.family.variants.len(),
                                locale,
                            ))),
                    ),
            );
            for variant in &compilation.family.variants {
                let variant_id = variant.id.clone();
                let selected_now = selected.contains(&variant_id);
                let review_open = review_variant.as_deref() == Some(variant_id.as_str());
                let appearance = i18n::translate(
                    mode_message(if variant.appearance.is_dark() {
                        AppearanceMode::Dark
                    } else {
                        AppearanceMode::Light
                    }),
                    locale,
                );
                let report = compilation.reports.get(&variant.id);
                let sample = Theme::from_variant(
                    variant,
                    AccentSelection::ThemeDefault,
                    SurfacePreference::ThemeDefault,
                );
                main = main.child(
                    div()
                        .id(SharedString::from(format!("theme-import-row-{variant_id}")))
                        .mt(px(8.0))
                        .p(px(11.0))
                        .rounded(px(10.0))
                        .border_1()
                        .border_color(if selected_now {
                            theme.accent.opacity(0.7)
                        } else {
                            theme.border
                        })
                        .bg(if selected_now {
                            theme.accent_wash.opacity(0.42)
                        } else {
                            theme.surface_raised.opacity(0.22)
                        })
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(9.0))
                                .child(
                                    div()
                                        .id(SharedString::from(format!(
                                            "theme-import-select-{variant_id}"
                                        )))
                                        .size(px(18.0))
                                        .rounded(px(5.0))
                                        .border_1()
                                        .border_color(if selected_now {
                                            theme.accent
                                        } else {
                                            theme.border_strong
                                        })
                                        .bg(if selected_now { theme.accent } else { theme.bg })
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor_pointer()
                                        .when(selected_now, |item| {
                                            item.child(
                                                icons::icon(icons::CHECK)
                                                    .size(px(12.0))
                                                    .text_color(theme.on_accent),
                                            )
                                        })
                                        .on_click(cx.listener({
                                            let variant_id = variant_id.clone();
                                            move |this, _, _, cx| {
                                                if let Some(dialog) = this.import_dialog.as_mut() {
                                                    if !dialog.selected.remove(&variant_id) {
                                                        dialog.selected.insert(variant_id.clone());
                                                    }
                                                }
                                                cx.notify();
                                            }
                                        })),
                                )
                                .child(palette_preview(&sample))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .child(
                                            div()
                                                .text_size(crate::typography::ui_rems(12.5))
                                                .font_weight(gpui::FontWeight::MEDIUM)
                                                .text_color(theme.text)
                                                .child(SharedString::from(variant.name.clone())),
                                        )
                                        .child(
                                            div()
                                                .text_size(crate::typography::ui_rems(11.0))
                                                .text_color(theme.text_muted)
                                                .child(appearance),
                                        ),
                                )
                                .child(
                                    compact_action(
                                        theme,
                                        i18n::translate(
                                            if review_open {
                                                MessageId::ThemeImportHideDetails
                                            } else {
                                                MessageId::ThemeImportShowDetails
                                            },
                                            locale,
                                        ),
                                        format!("theme-import-review-{variant_id}"),
                                    )
                                    .on_click(cx.listener({
                                        let variant_id = variant_id.clone();
                                        move |this, _, _, cx| {
                                            if let Some(dialog) = this.import_dialog.as_mut() {
                                                dialog.review_variant =
                                                    if dialog.review_variant.as_deref()
                                                        == Some(variant_id.as_str())
                                                    {
                                                        None
                                                    } else {
                                                        Some(variant_id.clone())
                                                    };
                                            }
                                            cx.notify();
                                        }
                                    })),
                                ),
                        )
                        .when(review_open, |row| {
                            row.child(
                                div()
                                    .mt(px(10.0))
                                    .pt(px(10.0))
                                    .border_t_1()
                                    .border_color(hairline)
                                    .child(import_scene_preview(variant)),
                            )
                            .when_some(report, |row, report| {
                                row.child(report_panel(theme, report, locale))
                            })
                        }),
                );
            }
            for failure in &compilation.failures {
                main = main.child(
                    div()
                        .mt(px(8.0))
                        .p(px(10.0))
                        .rounded(px(8.0))
                        .bg(theme.warning.opacity(0.08))
                        .text_size(crate::typography::ui_rems(11.0))
                        .text_color(theme.warning)
                        .child(SharedString::from(i18n::fill_many(
                            MessageId::ThemeImportCompileFailed,
                            &[("{name}", &failure.name), ("{message}", &failure.message)],
                            locale,
                        ))),
                );
            }
        } else {
            main = main.child(
                div()
                    .mt(px(14.0))
                    .flex()
                    .items_start()
                    .gap(px(7.0))
                    .text_size(crate::typography::ui_rems(11.0))
                    .line_height(px(16.0))
                    .text_color(theme.text_muted)
                    .child(
                        icons::icon(icons::INFO_CIRCLE)
                            .size(px(13.0))
                            .mt(px(1.0))
                            .flex_none(),
                    )
                    .child(i18n::translate(MessageId::ThemeImportAutoVariants, locale)),
            );
        }

        if let Some(error) = error {
            main = main.child(
                div()
                    .mt(px(12.0))
                    .p(px(10.0))
                    .rounded(px(8.0))
                    .bg(theme.danger.opacity(0.08))
                    .flex()
                    .items_start()
                    .gap(px(7.0))
                    .text_size(crate::typography::ui_rems(11.0))
                    .line_height(px(16.0))
                    .text_color(theme.danger)
                    .child(
                        icons::icon(icons::DANGER_TRIANGLE)
                            .size(px(13.0))
                            .flex_none()
                            .mt(px(1.0)),
                    )
                    .child(div().flex_1().min_w_0().truncate().child(error)),
            );
        }

        let header = div()
            .px(px(20.0))
            .pt(px(18.0))
            .pb(px(16.0))
            .flex()
            .items_start()
            .gap(px(16.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(popover::dialog_title(
                        theme,
                        i18n::translate(MessageId::ThemeImportTitle, locale),
                    ))
                    .child(
                        popover::dialog_body(
                            theme,
                            i18n::translate(MessageId::ThemeImportSubtitle, locale),
                        )
                        .mt(px(4.0)),
                    ),
            )
            .child(
                div()
                    .id("theme-import-close")
                    .size(px(28.0))
                    .rounded(px(7.0))
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.surface_raised.opacity(0.28))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.surface_raised_hover))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.import_dialog = None;
                        cx.notify();
                    }))
                    .child(
                        icons::icon(icons::CLOSE)
                            .size(px(12.0))
                            .text_color(theme.text_muted),
                    ),
            );

        let footer = div()
            .border_t_1()
            .border_color(hairline)
            .bg(theme.surface_raised.opacity(0.18))
            .px(px(20.0))
            .py(px(12.0))
            .flex()
            .items_center()
            .justify_end()
            .gap(px(8.0))
            .child(
                compact_action(
                    theme,
                    i18n::translate(MessageId::CommonCancel, locale),
                    "theme-import-cancel",
                )
                .h(px(34.0))
                .px(px(13.0))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.import_dialog = None;
                    cx.notify();
                })),
            )
            .child(
                popover::btn_primary(
                    theme,
                    i18n::translate(
                        if compilation.is_some() {
                            MessageId::ThemeImportActionImport
                        } else {
                            MessageId::ThemeImportActionAnalyze
                        },
                        locale,
                    ),
                )
                .id("theme-import-action")
                .h(px(34.0))
                .px(px(14.0))
                .py(px(0.0))
                .flex()
                .items_center()
                .when(compilation.is_some() && !ready, |button| {
                    button.opacity(0.45)
                })
                .when(compilation.is_none() || ready, |button| {
                    button.on_click(cx.listener(move |this, _, _, cx| {
                        if this
                            .import_dialog
                            .as_ref()
                            .is_some_and(|dialog| dialog.compilation.is_some())
                        {
                            this.finish_import(cx);
                        } else {
                            this.compile_import(cx);
                        }
                    }))
                }),
            );

        let card = popover::dialog_card(theme)
            .id("theme-import-card")
            .w(px(600.0))
            .max_h(px(760.0))
            .p(px(0.0))
            .overflow_hidden()
            .track_focus(&focus)
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                match popover::classify_key(
                    event.keystroke.key.as_str(),
                    event.keystroke.modifiers.platform,
                    event.keystroke.modifiers.control,
                ) {
                    popover::MenuKey::Escape => {
                        this.import_dialog = None;
                        cx.notify();
                    }
                    popover::MenuKey::Enter | popover::MenuKey::ModEnter => {
                        if this
                            .import_dialog
                            .as_ref()
                            .is_some_and(|dialog| dialog.compilation.is_some())
                        {
                            this.finish_import(cx);
                        } else {
                            this.compile_import(cx);
                        }
                    }
                    _ => {}
                }
            }))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.import_dialog = None;
                cx.notify();
            }))
            .child(header)
            .child(main)
            .child(footer)
            .into_any_element();

        Some(popover::modal("theme-import-dialog", viewport, card))
    }

    fn render_review_dialog(
        &mut self,
        viewport: gpui::Size<gpui::Pixels>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let entry_id = self.review_entry.as_ref()?;
        let entry = theme_library::entries(cx)
            .into_iter()
            .find(|entry| &entry.id == entry_id)?;
        let locale = i18n::locale(cx);
        let mut card = popover::dialog_card(theme)
            .id("theme-review-card")
            .w(px(660.0))
            .max_h(px(720.0))
            .overflow_y_scroll()
            .child(popover::dialog_title(
                theme,
                i18n::translate(MessageId::ThemeReviewTitle, locale),
            ))
            .child(
                popover::dialog_body(theme, format!("{} · {}", entry.name, entry.source.label()))
                    .mt(px(6.0)),
            );
        for variant in &entry.family.variants {
            card = card
                .child(
                    div()
                        .mt(px(14.0))
                        .text_size(crate::typography::ui_rems(12.5))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .child(SharedString::from(variant.name.clone())),
                )
                .child(import_scene_preview(variant));
            if let Some(report) = entry.reports.get(&variant.id) {
                card = card.child(report_panel(theme, report, locale));
            }
        }
        card = card.child(
            div().mt(px(16.0)).flex().justify_end().child(
                popover::btn_primary(theme, i18n::translate(MessageId::CommonDone, locale))
                    .id("theme-review-close")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.review_entry = None;
                        cx.notify();
                    })),
            ),
        );
        Some(popover::modal(
            "theme-review-dialog",
            viewport,
            card.into_any_element(),
        ))
    }

    fn render_library_entry(
        &mut self,
        entry: CustomThemeEntry,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = entry.id.clone();
        let linked = entry.source.is_linked();
        let locale = i18n::locale(cx);
        let source = entry
            .source
            .path()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| {
                i18n::translate(MessageId::ThemeLibrarySelfContained, locale).to_owned()
            });
        let status = match &entry.status {
            CustomThemeStatus::Ready => i18n::fill_many(
                MessageId::ThemeLibraryStatus,
                &[
                    ("{source}", entry.source.label()),
                    (
                        "{variants}",
                        &i18n::variants(entry.family.variants.len(), locale),
                    ),
                    ("{detail}", &source),
                ],
                locale,
            ),
            CustomThemeStatus::Warning { message } => i18n::fill(
                MessageId::ThemeLibraryLastKnownGood,
                "{message}",
                message,
                locale,
            ),
        };
        widgets::card_row(theme, false)
            .child(widgets::row_tile(
                theme,
                if linked {
                    icons::GLOBAL
                } else {
                    icons::DOCUMENT
                },
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(widgets::row_title(theme, &entry.name))
                    .child(
                        div()
                            .truncate()
                            .text_size(crate::typography::ui_rems(11.0))
                            .text_color(
                                if matches!(entry.status, CustomThemeStatus::Warning { .. }) {
                                    theme.warning
                                } else {
                                    theme.text_muted
                                },
                            )
                            .child(SharedString::from(status)),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(2.0))
                    .when(linked, |actions| {
                        actions.child(
                            compact_action(
                                theme,
                                i18n::translate(MessageId::ThemeLibraryReload, locale),
                                format!("theme-reload-{id}"),
                            )
                            .on_click(cx.listener({
                                let id = id.clone();
                                move |_, _, _, cx| {
                                    let _ = theme_library::reload(&id, cx);
                                    cx.notify();
                                }
                            })),
                        )
                    })
                    .child(
                        compact_action(
                            theme,
                            i18n::translate(MessageId::ThemeLibraryReveal, locale),
                            format!("theme-reveal-{id}"),
                        )
                        .on_click(cx.listener({
                            let id = id.clone();
                            move |this, _, _, cx| {
                                if let Err(error) = theme_library::reveal(&id, cx) {
                                    this.library_error = Some(
                                        theme_library::error_text(&error, i18n::locale(cx)).into(),
                                    );
                                }
                                cx.notify();
                            }
                        })),
                    )
                    .child(
                        compact_action(
                            theme,
                            i18n::translate(MessageId::ThemeLibraryReview, locale),
                            format!("theme-review-{id}"),
                        )
                        .on_click(cx.listener({
                            let id = id.clone();
                            move |this, _, _, cx| {
                                this.review_entry = Some(id.clone());
                                cx.notify();
                            }
                        })),
                    )
                    .child(
                        compact_action(
                            theme,
                            i18n::translate(MessageId::ThemeLibraryDuplicate, locale),
                            format!("theme-duplicate-{id}"),
                        )
                        .on_click(cx.listener({
                            let id = id.clone();
                            move |this, _, _, cx| {
                                if let Err(error) = theme_library::duplicate_as_editable(&id, cx) {
                                    this.library_error = Some(
                                        theme_library::error_text(&error, i18n::locale(cx)).into(),
                                    );
                                }
                                cx.notify();
                            }
                        })),
                    )
                    .when(linked, |actions| {
                        actions.child(
                            compact_action(
                                theme,
                                i18n::translate(MessageId::ThemeLibraryUnlink, locale),
                                format!("theme-unlink-{id}"),
                            )
                            .on_click(cx.listener({
                                let id = id.clone();
                                move |this, _, _, cx| {
                                    if let Err(error) = theme_library::unlink(&id, cx) {
                                        this.library_error = Some(
                                            theme_library::error_text(&error, i18n::locale(cx))
                                                .into(),
                                        );
                                    }
                                    cx.notify();
                                }
                            })),
                        )
                    })
                    .child(
                        compact_action(
                            theme,
                            i18n::translate(MessageId::CommonRemove, locale),
                            format!("theme-remove-{id}"),
                        )
                        .text_color(theme.danger)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Err(error) = theme_library::remove(&id, cx) {
                                this.library_error = Some(
                                    theme_library::error_text(&error, i18n::locale(cx)).into(),
                                );
                            }
                            cx.notify();
                        })),
                    ),
            )
            .into_any_element()
    }

    fn render_theme_library_rows(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let entries = theme_library::entries(cx);
        let (linked, imported): (Vec<_>, Vec<_>) = entries
            .into_iter()
            .partition(|entry| entry.source.is_linked());
        let locale = i18n::locale(cx);
        let mut rows = vec![
            widgets::card_row(theme, false)
                .child(widgets::row_tile(theme, icons::FOLDER_WITH_FILES))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(widgets::row_title(
                            theme,
                            i18n::translate(MessageId::ThemeLibraryTitle, locale),
                        ))
                        .child(widgets::meta_line(
                            theme,
                            vec![
                                div()
                                    .child(i18n::translate(MessageId::ThemeLibrarySubtitle, locale))
                                    .into_any_element(),
                            ],
                        )),
                )
                .child(
                    popover::btn_primary(
                        theme,
                        i18n::translate(MessageId::ThemeLibraryAdd, locale),
                    )
                    .id("theme-library-add")
                    .on_click(cx.listener(|this, _, _, cx| this.open_import(cx))),
                )
                .into_any_element(),
        ];
        if !imported.is_empty() {
            rows.push(
                div()
                    .px(px(16.0))
                    .pt(px(12.0))
                    .pb(px(4.0))
                    .border_t_1()
                    .border_color(theme.border)
                    .text_size(crate::typography::ui_rems(10.5))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme.text_faint)
                    .child(i18n::translate(MessageId::ThemeLibraryImported, locale))
                    .into_any_element(),
            );
            rows.extend(
                imported
                    .into_iter()
                    .map(|entry| self.render_library_entry(entry, theme, cx)),
            );
        }
        if !linked.is_empty() {
            rows.push(
                div()
                    .px(px(16.0))
                    .pt(px(12.0))
                    .pb(px(4.0))
                    .border_t_1()
                    .border_color(theme.border)
                    .text_size(crate::typography::ui_rems(10.5))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme.text_faint)
                    .child(i18n::translate(MessageId::ThemeLibraryLinked, locale))
                    .into_any_element(),
            );
            rows.extend(
                linked
                    .into_iter()
                    .map(|entry| self.render_library_entry(entry, theme, cx)),
            );
        }
        rows
    }
}

impl EventEmitter<AppearanceSettingsEvent> for AppearancePage {}

impl Render for AppearancePage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let availability = typography::availability(cx);
        let fixed = theme.font_sans_fixed.clone();
        let current_mode = appearance::mode(cx);
        let current_themes = appearance::themes(cx);
        let current_accent = appearance::accent(cx);
        let current_surface = appearance::surface(cx);
        let locale = i18n::locale(cx);
        let current_language = i18n::preference(cx);
        let ui_settings = crate::settings::current(cx);
        let current_background = ui_settings.new_thread_composer_background;
        let current_background_effect = ui_settings.new_thread_background_effect;
        let compact_mode = ui_settings.transcript_compact_mode;
        let cards = AppearanceMode::ALL
            .into_iter()
            .map(|mode| {
                widgets::option_card(
                    &theme,
                    mode.icon(),
                    i18n::translate(mode_message(mode), locale),
                    mode == current_mode,
                    preview(mode, &current_themes, current_accent, current_surface),
                )
                .id(SharedString::from(format!("appearance-{}", mode.label())))
                .on_click(cx.listener(move |_, _, _, cx| {
                    appearance::set_mode(mode, cx);
                    cx.notify();
                }))
            })
            .collect::<Vec<_>>();

        let mut theme_rows = Vec::new();
        for (index, appearance_kind) in [Appearance::Light, Appearance::Dark]
            .into_iter()
            .enumerate()
        {
            let (label, mode) = if appearance_kind.is_light() {
                (MessageId::AppearanceThemeLight, AppearanceMode::Light)
            } else {
                (MessageId::AppearanceThemeDark, AppearanceMode::Dark)
            };
            let selector = self.render_theme_selector(appearance_kind, &current_themes, &theme, cx);
            theme_rows.push(
                widgets::card_row(&theme, index == 0)
                    .child(widgets::row_tile(&theme, mode.icon()))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(widgets::row_title(&theme, i18n::translate(label, locale)))
                            .child(widgets::meta_line(
                                &theme,
                                vec![
                                    div()
                                        .child(i18n::translate(
                                            MessageId::AppearanceThemeActiveHint,
                                            locale,
                                        ))
                                        .into_any_element(),
                                ],
                            )),
                    )
                    .child(selector)
                    .into_any_element(),
            );
        }

        let mut accent_choices = vec![AccentSelection::ThemeDefault];
        accent_choices.extend(AccentPreset::ALL.map(AccentSelection::Preset));
        let accent_controls = accent_choices
            .into_iter()
            .map(|selection| {
                let selected = selection == current_accent;
                accent_swatch(&theme, selection, selected).on_click(cx.listener(
                    move |_, _, _, cx| {
                        appearance::set_accent(selection, cx);
                        cx.notify();
                    },
                ))
            })
            .collect::<Vec<_>>();
        let surface_controls = SurfacePreference::ALL
            .into_iter()
            .map(|surface| {
                surface_choice(&theme, surface, locale, surface == current_surface).on_click(
                    cx.listener(move |_, _, _, cx| {
                        appearance::set_surface(surface, cx);
                        cx.notify();
                    }),
                )
            })
            .collect::<Vec<_>>();
        let mut settings_rows = theme_rows;
        settings_rows.push(
            widgets::card_row(&theme, false)
                .child(widgets::row_tile(&theme, icons::TUNING))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(widgets::row_title(
                            &theme,
                            i18n::translate(MessageId::AppearanceAccentColor, locale),
                        ))
                        .child(widgets::meta_line(
                            &theme,
                            vec![
                                div()
                                    .child(SharedString::from(accent_helper(
                                        current_accent,
                                        locale,
                                    )))
                                    .into_any_element(),
                            ],
                        )),
                )
                .child(
                    div()
                        .flex_none()
                        .ml(px(10.0))
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .children(accent_controls),
                )
                .into_any_element(),
        );
        settings_rows.push(
            widgets::card_row(&theme, false)
                .child(widgets::row_tile(&theme, icons::WIDGET))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(widgets::row_title(
                            &theme,
                            i18n::translate(MessageId::AppearanceGlass, locale),
                        ))
                        .child(widgets::meta_line(
                            &theme,
                            vec![
                                div()
                                    .child(SharedString::from(surface_helper(
                                        current_surface,
                                        theme.surface_treatment,
                                        locale,
                                    )))
                                    .into_any_element(),
                            ],
                        )),
                )
                .child(
                    div()
                        .flex_none()
                        .ml(px(10.0))
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .children(surface_controls),
                )
                .into_any_element(),
        );
        let background_available = current_background
            .as_ref()
            .is_some_and(|background| Path::new(&background.path).is_file());
        let background_tile: AnyElement = if let Some(background) =
            current_background.as_ref().filter(|_| background_available)
        {
            div()
                .flex_none()
                .size(px(36.0))
                .rounded(px(10.0))
                .overflow_hidden()
                .border_1()
                .border_color(crate::theme::hairline(0.10))
                .child(
                    img(PathBuf::from(background.path.clone()))
                        .size(px(34.0))
                        .rounded(px(9.0))
                        .object_fit(ObjectFit::Cover),
                )
                .into_any_element()
        } else {
            widgets::row_tile(&theme, icons::FILE_IMAGE).into_any_element()
        };
        let background_meta = match current_background.as_ref() {
            Some(background) if background_available => vec![
                div()
                    .child(SharedString::from(background.name.clone()))
                    .into_any_element(),
                div()
                    .child(i18n::translate(MessageId::BackgroundSoftened, locale))
                    .into_any_element(),
            ],
            Some(_) => vec![
                div()
                    .child(i18n::translate(
                        MessageId::BackgroundImageUnavailable,
                        locale,
                    ))
                    .into_any_element(),
                div()
                    .child(i18n::translate(
                        MessageId::BackgroundChooseReplacement,
                        locale,
                    ))
                    .into_any_element(),
            ],
            None => vec![
                div()
                    .child(i18n::translate(MessageId::BackgroundHint, locale))
                    .into_any_element(),
            ],
        };
        settings_rows.push(
            widgets::card_row(&theme, false)
                .child(background_tile)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(widgets::row_title(
                            &theme,
                            i18n::translate(MessageId::BackgroundTitle, locale),
                        ))
                        .child(widgets::meta_line(&theme, background_meta)),
                )
                .child(
                    div()
                        .flex_none()
                        .ml(px(10.0))
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .when(current_background.is_some(), |actions| {
                            actions
                                .child(
                                    compact_action(
                                        &theme,
                                        i18n::translate(MessageId::BackgroundReplace, locale),
                                        "new-thread-background-replace",
                                    )
                                    .on_click(cx.listener(
                                        |this, _, _, cx| this.choose_new_thread_background(cx),
                                    )),
                                )
                                .child(
                                    compact_action(
                                        &theme,
                                        i18n::translate(MessageId::CommonRemove, locale),
                                        "new-thread-background-remove",
                                    )
                                    .text_color(theme.danger)
                                    .on_click(cx.listener(
                                        |this, _, _, cx| this.remove_new_thread_background(cx),
                                    )),
                                )
                        })
                        .when(current_background.is_none(), |actions| {
                            actions.child(
                                compact_action(
                                    &theme,
                                    i18n::translate(MessageId::BackgroundChoose, locale),
                                    "new-thread-background-choose",
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| this.choose_new_thread_background(cx),
                                )),
                            )
                        }),
                )
                .into_any_element(),
        );
        if background_available {
            let effect_controls = crate::settings::NewThreadBackgroundEffect::ALL
                .into_iter()
                .map(|effect| {
                    background_effect_choice(
                        &theme,
                        effect,
                        effect == current_background_effect,
                        locale,
                    )
                    .on_click(cx.listener(move |_, _, _, cx| {
                        crate::settings::set_new_thread_background_effect(effect, cx);
                        cx.notify();
                    }))
                })
                .collect::<Vec<_>>();
            settings_rows.push(
                widgets::card_row(&theme, false)
                    .child(widgets::row_tile(&theme, icons::TUNING))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(widgets::row_title(
                                &theme,
                                i18n::translate(MessageId::BackgroundEffect, locale),
                            ))
                            .child(widgets::meta_line(
                                &theme,
                                vec![
                                    div()
                                        .child(i18n::translate(
                                            current_background_effect.description_message(),
                                            locale,
                                        ))
                                        .into_any_element(),
                                ],
                            )),
                    )
                    .child(
                        div()
                            .flex_none()
                            .ml(px(10.0))
                            .max_w(px(430.0))
                            .flex()
                            .flex_wrap()
                            .justify_end()
                            .gap(px(6.0))
                            .children(effect_controls),
                    )
                    .into_any_element(),
            );
        }
        if let Some(error) = self.background_error.clone() {
            settings_rows.push(
                div()
                    .px(px(20.0))
                    .py(px(10.0))
                    .border_t_1()
                    .border_color(theme.border)
                    .child(widgets::error_strip(&theme, error))
                    .into_any_element(),
            );
        }
        settings_rows.push(
            widgets::card_row(&theme, false)
                .child(widgets::row_tile(&theme, icons::EYE_CLOSED))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(widgets::row_title(
                            &theme,
                            i18n::translate(MessageId::AppearanceCompactMode, locale),
                        ))
                        .child(widgets::meta_line(
                            &theme,
                            vec![
                                div()
                                    .child(i18n::translate(
                                        MessageId::AppearanceCompactModeHint,
                                        locale,
                                    ))
                                    .into_any_element(),
                            ],
                        )),
                )
                .child(
                    widgets::toggle_switch(&theme, compact_mode)
                        .id("transcript-compact-mode-toggle")
                        .cursor_pointer()
                        .on_click(cx.listener(move |_, _, _, cx| {
                            crate::settings::set_transcript_compact_mode(!compact_mode, cx);
                            cx.notify();
                        })),
                )
                .into_any_element(),
        );
        settings_rows.extend(self.render_theme_library_rows(&theme, cx));
        let library_warning = self
            .library_error
            .clone()
            .or_else(|| theme_library::load_warning(cx).map(SharedString::from));
        let modal = self
            .render_import_dialog(window.viewport_size(), &theme, window, cx)
            .or_else(|| self.render_review_dialog(window.viewport_size(), &theme, cx));

        let ui_picker =
            self.render_font_picker(FontKind::Ui, &theme, &availability, fixed.clone(), cx);
        let terminal_picker =
            self.render_font_picker(FontKind::Terminal, &theme, &availability, fixed.clone(), cx);
        let code_picker =
            self.render_font_picker(FontKind::Code, &theme, &availability, fixed.clone(), cx);
        let ui_size = self.render_size_picker(FontKind::Ui, &theme, fixed.clone(), cx);
        let terminal_size = self.render_size_picker(FontKind::Terminal, &theme, fixed.clone(), cx);
        let code_size = self.render_size_picker(FontKind::Code, &theme, fixed.clone(), cx);

        let mut font_section = div()
            .mt(px(36.0))
            .flex()
            .flex_col()
            .gap(px(18.0))
            .font_family(fixed.clone());
        for (kind, picker, size_control) in [
            (FontKind::Ui, ui_picker, ui_size),
            (FontKind::Terminal, terminal_picker, terminal_size),
            (FontKind::Code, code_picker, code_size),
        ] {
            font_section = font_section.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(px(24.0))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(widgets::field_label(&theme, kind.label(locale)))
                            .child(
                                div()
                                    .max_w(px(520.0))
                                    .text_size(typography::ui_rems(12.0))
                                    .line_height(px(18.0))
                                    .text_color(theme.text_muted)
                                    .child(kind.description(locale)),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            .child(picker)
                            .child(size_control),
                    ),
            );
        }
        font_section = font_section.child(self.render_transcript_width(&theme, window, cx));
        for kind in FontKind::ALL {
            let (requested, effective) = (kind.requested(cx), kind.effective(cx));
            if requested != effective {
                font_section = font_section.child(
                    widgets::error_strip(
                        &theme,
                        i18n::fill_many(
                            MessageId::FontFallbackUnavailable,
                            &[
                                ("{slot}", kind.label(locale)),
                                ("{family}", requested.label()),
                                ("{fallback}", effective.label()),
                            ],
                            locale,
                        ),
                    )
                    .font_family(fixed.clone()),
                );
            }
        }

        let scrollbar = popover::rail(self, "appearance-page-scrollbar", &theme, cx);
        div()
            .id("appearance-page-host")
            .on_drag_move(cx.listener(
                |this, event: &gpui::DragMoveEvent<TranscriptWidthDrag>, window, cx| {
                    this.drag_width(event.event.position.x, window, cx);
                },
            ))
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|this, _: &gpui::MouseUpEvent, _, cx| {
                    if this.width_pressed {
                        this.width_pressed = false;
                        cx.notify();
                    }
                    this.apply_pending_width(cx);
                }),
            )
            .on_mouse_up_out(
                gpui::MouseButton::Left,
                cx.listener(|this, _: &gpui::MouseUpEvent, _, cx| {
                    if this.width_pressed {
                        this.width_pressed = false;
                        cx.notify();
                    }
                    this.apply_pending_width(cx);
                }),
            )
            .relative()
            .size_full()
            .on_hover(cx.listener(Self::on_scroll_hovered))
            .child(
                div()
                    .id("appearance-page")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll.scroll)
                    .child(
                        widgets::page_column()
                            .child(widgets::page_header(
                                &theme,
                                i18n::translate(MessageId::AppearanceTitle, locale),
                                None,
                            ))
                            .child(
                                widgets::page_subtitle(
                                    &theme,
                                    i18n::translate(MessageId::AppearanceSubtitle, locale),
                                )
                                .max_w(px(512.0))
                                .line_height(px(20.0)),
                            )
                            .child(
                                div()
                                    .mt(px(32.0))
                                    .flex()
                                    .flex_col()
                                    .gap(px(12.0))
                                    .child(widgets::field_label(
                                        &theme,
                                        i18n::translate(MessageId::AppearanceTitle, locale),
                                    ))
                                    .child(widgets::option_card_row().children(cards)),
                            )
                            .child(widgets::section_card(&theme).children(settings_rows))
                            .child(
                                widgets::section_card(&theme).child(
                                    widgets::card_row(&theme, true)
                                        .child(widgets::row_tile(&theme, icons::GLOBE))
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .child(widgets::row_title(
                                                    &theme,
                                                    i18n::translate(
                                                        MessageId::LanguageLabel,
                                                        locale,
                                                    ),
                                                ))
                                                .child(widgets::meta_line(
                                                    &theme,
                                                    vec![
                                                        div()
                                                            .child(SharedString::from(
                                                                i18n::preference_status(
                                                                    current_language,
                                                                    locale,
                                                                    locale,
                                                                ),
                                                            ))
                                                            .into_any_element(),
                                                        div()
                                                            .child(i18n::translate(
                                                                MessageId::LanguageHint,
                                                                locale,
                                                            ))
                                                            .into_any_element(),
                                                    ],
                                                )),
                                        )
                                        .child(div().flex_none().ml(px(10.0)).child(
                                            language_selector(
                                                self,
                                                &theme,
                                                current_language,
                                                locale,
                                                cx,
                                            ),
                                        )),
                                ),
                            )
                            .child(font_section)
                            .when_some(library_warning, |page, warning| {
                                page.child(
                                    div()
                                        .mt(px(8.0))
                                        .text_size(crate::typography::ui_rems(11.5))
                                        .text_color(theme.warning)
                                        .child(warning),
                                )
                            }),
                    ),
            )
            .children(scrollbar)
            .children(modal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_mode_gets_a_card() {
        assert_eq!(AppearanceMode::ALL.len(), 3);
        for mode in AppearanceMode::ALL {
            assert!(!mode.label().is_empty());
        }
    }

    #[test]
    fn registry_offers_both_appearances_and_keeps_single_dark_families_valid() {
        let registry = ThemeRegistry::builtin();
        assert_eq!(
            registry
                .variants_for(zeron_theme::Appearance::Light)
                .count(),
            10
        );
        assert_eq!(
            registry.variants_for(zeron_theme::Appearance::Dark).count(),
            20
        );
    }

    #[test]
    fn accent_helper_explains_default_and_override_scope() {
        assert!(accent_helper(AccentSelection::ThemeDefault, Locale::En).contains("intended"));
        let copy = accent_helper(AccentSelection::Preset(AccentPreset::Pink), Locale::En);
        assert!(copy.starts_with("Pink ·"));
        assert!(copy.contains("glyphs"));
        // The preset name stays untranslated; only the surrounding copy does.
        assert!(
            accent_helper(AccentSelection::Preset(AccentPreset::Pink), Locale::ZhCn)
                .starts_with("Pink ·")
        );
    }

    #[test]
    fn surface_helper_explains_theme_default_and_global_overrides() {
        let default = surface_helper(
            SurfacePreference::ThemeDefault,
            SurfaceTreatment::Opaque,
            Locale::En,
        );
        assert!(default.contains("opaque default"));
        assert!(
            surface_helper(
                SurfacePreference::Frosted,
                SurfaceTreatment::Opaque,
                Locale::En
            )
            .contains("where supported")
        );
        assert!(
            surface_helper(
                SurfacePreference::Opaque,
                SurfaceTreatment::Frosted,
                Locale::En
            )
            .contains("every theme")
        );
        assert_eq!(
            surface_helper(
                SurfacePreference::Opaque,
                SurfaceTreatment::Frosted,
                Locale::ZhCn
            ),
            "所有主题均使用纯色表面。"
        );
    }

    #[test]
    fn font_options_appear_once_in_stable_order() {
        let catalog = FontAvailability::all();
        let labels: Vec<_> = catalog.choices().iter().map(UiFontFamily::label).collect();
        assert_eq!(labels.len(), 5);
        assert_eq!(
            labels,
            ["Geist", "Geist Mono", "System UI", "Arial", "Menlo"]
        );
        let unique = labels.into_iter().collect::<std::collections::HashSet<_>>();
        assert_eq!(unique.len(), 5);
    }

    #[test]
    fn only_the_terminal_catalog_is_narrowed_to_fixed_width() {
        let all = FontAvailability::all();
        let terminal: Vec<_> = FontKind::Terminal
            .choices_for(&all)
            .iter()
            .map(UiFontFamily::label)
            .collect();
        assert_eq!(terminal, ["Geist Mono", "Menlo"]);

        for kind in [FontKind::Ui, FontKind::Code] {
            assert_eq!(kind.choices_for(&all), all.choices());
            assert!(kind.is_available_for(&all, &UiFontFamily::System));
            assert!(kind.is_available_for(&all, &UiFontFamily::Installed("Arial".into())));
        }
        for proportional in [
            UiFontFamily::System,
            UiFontFamily::Geist,
            UiFontFamily::Installed("Arial".into()),
        ] {
            assert!(!FontKind::Terminal.is_available_for(&all, &proportional));
        }
        assert!(FontKind::Terminal.is_available_for(&all, &UiFontFamily::GeistMono));
        // A query that only matches proportional families leaves the terminal
        // highlight on the bundled mono face rather than on System UI.
        assert_eq!(
            first_available(&[], FontKind::Terminal, &all),
            UiFontFamily::GeistMono
        );
    }

    #[test]
    fn font_keyboard_navigation_stops_at_edges_and_skips_unavailable() {
        let all = FontAvailability::all();
        let choices = all.choices().to_vec();
        assert_eq!(
            step_font(&UiFontFamily::Geist, -1, &choices, FontKind::Ui, &all),
            UiFontFamily::Geist
        );
        assert_eq!(
            step_font(
                &UiFontFamily::Installed("Menlo".into()),
                1,
                &choices,
                FontKind::Ui,
                &all
            ),
            UiFontFamily::Installed("Menlo".into())
        );
        let without_arial = all.without(&UiFontFamily::Installed("Arial".into()));
        assert_eq!(
            step_font(
                &UiFontFamily::System,
                1,
                &choices,
                FontKind::Ui,
                &without_arial
            ),
            UiFontFamily::Installed("Menlo".into())
        );
    }

    #[test]
    fn filtering_narrows_to_matches_and_navigation_stays_inside_them() {
        let all = FontAvailability::all();
        let catalog = all.choices().to_vec();

        assert_eq!(filter_families("", &catalog), catalog);
        assert_eq!(filter_families("   ", &catalog), catalog);
        assert!(filter_families("helvetica", &catalog).is_empty());
        // Case-insensitive, and the bundled entries still lead when they match.
        assert_eq!(
            filter_families("GEIST", &catalog),
            vec![UiFontFamily::Geist, UiFontFamily::GeistMono]
        );

        // "Menlo" prefix-matches, so it outranks the "System UI" substring hit.
        let matches = filter_families("m", &catalog);
        assert_eq!(
            matches,
            vec![
                UiFontFamily::Installed("Menlo".into()),
                UiFontFamily::GeistMono,
                UiFontFamily::System,
            ]
        );
        // Stepping never escapes the filtered list.
        assert_eq!(
            step_font(&UiFontFamily::System, 1, &matches, FontKind::Ui, &all),
            UiFontFamily::System
        );
        assert_eq!(
            first_available(&matches, FontKind::Ui, &all),
            UiFontFamily::Installed("Menlo".into())
        );
        assert_eq!(
            last_available(&matches, FontKind::Ui, &all),
            UiFontFamily::System
        );
    }

    #[test]
    fn each_font_kind_gets_distinct_labels_and_element_ids() {
        let slugs: Vec<_> = FontKind::ALL.iter().map(|kind| kind.slug()).collect();
        let labels: Vec<_> = FontKind::ALL
            .iter()
            .map(|kind| kind.label(Locale::En))
            .collect();
        assert_eq!(
            slugs.iter().collect::<std::collections::HashSet<_>>().len(),
            3
        );
        assert_eq!(
            labels
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            3
        );
        // Element ids stay on `slug`; the copy is locale-dependent.
        assert_eq!(
            labels,
            ["Interface font", "Terminal font", "Code & diff font"]
        );
        let chinese: Vec<_> = FontKind::ALL
            .iter()
            .map(|kind| kind.label(Locale::ZhCn))
            .collect();
        assert_eq!(chinese, ["界面字体", "终端字体", "代码与差异字体"]);
    }

    #[test]
    fn pixel_sizes_render_whole_and_fractional_values() {
        assert_eq!(format_px(13.0), "13 px");
        assert_eq!(format_px(typography::CODE_FONT_SIZE_DEFAULT), "12.5 px");
        assert_eq!(
            typography::clamp_font_size(typography::FONT_SIZE_MAX + 1.0),
            typography::FONT_SIZE_MAX
        );
        assert_eq!(
            typography::clamp_font_size(typography::FONT_SIZE_MIN - 1.0),
            typography::FONT_SIZE_MIN
        );
    }

    #[test]
    fn font_size_options_are_ordered_and_include_the_default() {
        let values = UiFontSize::ALL.map(UiFontSize::pixels);
        assert!(values.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(UiFontSize::ALL.contains(&UiFontSize::default()));
    }

    #[test]
    fn mono_size_ladder_keeps_both_defaults_exactly_reachable() {
        assert!(MONO_FONT_SIZES.windows(2).all(|pair| pair[0] < pair[1]));
        for default in [
            typography::TERMINAL_FONT_SIZE_DEFAULT,
            typography::CODE_FONT_SIZE_DEFAULT,
        ] {
            assert_eq!(MONO_FONT_SIZES[nearest_mono_ix(default)], default);
        }
        // Off-ladder values (older settings, hand-edited files) snap, never drop.
        assert_eq!(MONO_FONT_SIZES[nearest_mono_ix(0.0)], MONO_FONT_SIZES[0]);
        assert_eq!(MONO_FONT_SIZES[nearest_mono_ix(99.0)], 20.0);
        assert_eq!(MONO_FONT_SIZES[nearest_mono_ix(12.4)], 12.5);
    }

    #[test]
    fn every_size_dropdown_labels_its_whole_ladder_in_pixels() {
        for kind in FontKind::ALL {
            let labels = kind.size_labels();
            assert_eq!(labels.len(), kind.size_count());
            assert!(labels.iter().all(|label| label.ends_with(" px")));
        }
        assert!(FontKind::Terminal.size_labels().contains(&"13 px".into()));
        assert!(FontKind::Code.size_labels().contains(&"12.5 px".into()));
    }

    #[gpui::test]
    fn conversation_width_drag_coalesces_and_persists_the_last_value(
        cx: &mut gpui::TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::dark());
            crate::settings::init(Default::default(), dir.path(), cx);
        });
        let window = cx.add_window(|_, cx| AppearancePage::new(cx));
        window
            .update(cx, |page, window, cx| {
                page.width_bounds.set(Some(gpui::Bounds::new(
                    gpui::point(px(100.0), px(0.0)),
                    gpui::size(px(240.0), px(28.0)),
                )));
                page.drag_width(px(0.0), window, cx);
                assert_eq!(page.pending_width, Some(560.0));
                page.drag_width(px(1000.0), window, cx);
                assert_eq!(page.pending_width, Some(1200.0));
                page.drag_width(px(220.0), window, cx);
                assert_eq!(page.pending_width, Some(880.0));
                assert_eq!(
                    crate::settings::transcript_width(cx),
                    736.0,
                    "pointer events must coalesce before publishing"
                );
                page.apply_pending_width(cx);
                assert_eq!(crate::settings::transcript_width(cx), 880.0);
                crate::settings::flush(cx);
                assert_eq!(
                    crate::settings::UiSettings::load(dir.path()).transcript_width,
                    880.0
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn size_dropdowns_commit_from_the_ladder_and_enter_leaves_menus_closed(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::dark());
            typography::init(
                UiFontFamily::Geist,
                UiFontSize::default(),
                UiFontFamily::GeistMono,
                typography::TERMINAL_FONT_SIZE_DEFAULT,
                UiFontFamily::GeistMono,
                typography::CODE_FONT_SIZE_DEFAULT,
                FontAvailability::all(),
                cx,
            );
        });
        let window = cx.add_window(|_, cx| AppearancePage::new(cx));
        window
            .update(cx, |page, window, cx| {
                for kind in [FontKind::Terminal, FontKind::Code] {
                    page.toggle_size_menu(kind, cx);
                    assert!(page.size_menu(kind).is_open());
                    assert_eq!(page.selected_size_ix(kind), kind.size_ix(cx));
                    let target = kind.size_ix(cx) + 1;
                    page.set_selected_size_ix(kind, target);
                    page.commit_size(kind, window, cx);
                    assert_eq!(kind.pixel_size(cx), MONO_FONT_SIZES[target]);
                    assert!(!page.size_menu(kind).is_open());
                }

                // Opening a family menu closes a size menu left open elsewhere.
                page.toggle_size_menu(FontKind::Ui, cx);
                assert!(page.size_menu(FontKind::Ui).is_open());
                page.toggle_font_menu(FontKind::Code, window, cx);
                assert!(!page.size_menu(FontKind::Ui).is_open());
                assert!(page.font_menu(FontKind::Code).is_open());

                // Enter commits and reports the key consumed — without that the
                // trigger's `is_open` guard reads the just-closed menu and
                // reopens it on the very same event.
                let enter = KeyDownEvent {
                    keystroke: gpui::Keystroke::parse("enter").expect("valid keystroke"),
                    is_held: false,
                    prefer_character_input: false,
                };
                assert!(page.on_font_key_down(FontKind::Code, &enter, window, cx));
                assert!(!page.font_menu(FontKind::Code).is_open());
            })
            .expect("window is open");
    }

    /// A settings file written before the terminal picker was constrained can
    /// still name a proportional family; startup must not hand it to the grid.
    #[gpui::test]
    fn persisted_proportional_terminal_family_resolves_to_geist_mono(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::dark());
            typography::init(
                UiFontFamily::Geist,
                UiFontSize::default(),
                UiFontFamily::Installed("Arial".into()),
                typography::TERMINAL_FONT_SIZE_DEFAULT,
                UiFontFamily::Installed("Arial".into()),
                typography::CODE_FONT_SIZE_DEFAULT,
                FontAvailability::all(),
                cx,
            );
            assert_eq!(
                typography::terminal_effective(cx),
                UiFontFamily::GeistMono,
                "proportional terminal family must fall back"
            );
            assert_eq!(
                typography::code_effective(cx),
                UiFontFamily::Installed("Arial".into()),
                "code and diffs keep proportional picks"
            );
            // The setter path rejects the same family too.
            assert!(!typography::set_terminal_family(UiFontFamily::System, cx));
            assert_eq!(typography::terminal_effective(cx), UiFontFamily::GeistMono);
        });
    }

    /// The switch path end to end: the persisted choice, the locale every render
    /// path reads, the copy each migrated surface resolves, and a draw of this
    /// page in both locales. gpui's test platform exposes no rendered-text API
    /// (`TestWindow` has only `simulate_input`/`simulate_resize`), so copy is
    /// asserted through the exact functions `render` calls, and the draw proves
    /// the Language row itself lays out without panicking.
    #[gpui::test]
    fn switching_language_updates_locale_copy_and_the_settings_file(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::dark());
            crate::settings::init(Default::default(), dir.path(), cx);
            typography::init(
                UiFontFamily::Geist,
                UiFontSize::default(),
                UiFontFamily::GeistMono,
                typography::TERMINAL_FONT_SIZE_DEFAULT,
                UiFontFamily::GeistMono,
                typography::CODE_FONT_SIZE_DEFAULT,
                FontAvailability::all(),
                cx,
            );
            crate::theme_library::init(dir.path().to_path_buf(), cx);
            i18n::init(LanguagePreference::English, cx);
        });
        let window = cx.add_window(|_, cx| AppearancePage::new(cx));

        let shell_copy = |locale: Locale| i18n::translate(MessageId::SidebarNoSessions, locale);
        let composer_copy =
            |locale: Locale| i18n::translate(MessageId::ComposerPlaceholder, locale);
        let diff_copy = |locale: Locale| {
            crate::changes::clean_message(crate::changes::DiffScope::LatestTurn, None, locale)
        };

        cx.update(|cx| {
            assert_eq!(i18n::locale(cx), Locale::En);
            assert_eq!(shell_copy(i18n::locale(cx)), "No sessions yet");
            assert_eq!(composer_copy(i18n::locale(cx)), "Do anything…");
            assert_eq!(diff_copy(i18n::locale(cx)), "No changes this turn");

            i18n::set_preference(LanguagePreference::SimplifiedChinese, cx);

            assert_eq!(i18n::preference(cx), LanguagePreference::SimplifiedChinese);
            assert_eq!(i18n::locale(cx), Locale::ZhCn);
            assert_eq!(shell_copy(i18n::locale(cx)), "还没有会话");
            assert_eq!(composer_copy(i18n::locale(cx)), "做任何事…");
            assert_eq!(diff_copy(i18n::locale(cx)), "本轮没有改动");
            assert_eq!(
                i18n::translate(MessageId::LanguageLabel, i18n::locale(cx)),
                "语言"
            );
            assert_eq!(
                i18n::preference_label(LanguagePreference::System, i18n::locale(cx)),
                "跟随系统"
            );
        });

        // Persisted before the call returns, so a restart keeps the choice.
        assert_eq!(
            crate::settings::UiSettings::load(dir.path()).language,
            LanguagePreference::SimplifiedChinese
        );

        // Back to English restores the original copy for the same surfaces.
        cx.update(|cx| {
            i18n::set_preference(LanguagePreference::English, cx);
            assert_eq!(i18n::locale(cx), Locale::En);
            assert_eq!(shell_copy(i18n::locale(cx)), "No sessions yet");
            assert_eq!(diff_copy(i18n::locale(cx)), "No changes this turn");
        });

        for preference in [
            LanguagePreference::SimplifiedChinese,
            LanguagePreference::English,
        ] {
            cx.update(|cx| i18n::set_preference(preference, cx));
            cx.update_window(window.into(), |_, window, cx| {
                window.refresh();
                window.draw(cx).clear();
            })
            .unwrap();
        }
    }
}
