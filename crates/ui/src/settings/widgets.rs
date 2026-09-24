//! Shared scaffolding for the settings pages — a centered page column, large
//! title, small section labels over filled blocks of hairline-split rows,
//! badges and small buttons, so every page reads as the same product surface.

use std::rc::Rc;

use gpui::{AnyElement, Context, Pixels, ScrollHandle, SharedString, Window, div, prelude::*, px};

use crate::popover::{self, MenuScrollbarMetrics, MenuScrollbarState, ScrollRailHost};
use crate::theme::Theme;

/// Width of the section column beside the settings page, published by the
/// shell each frame so dropdowns and responsive pages measure the page pane.
struct SettingsSidebarWidth(f32);

impl gpui::Global for SettingsSidebarWidth {}

pub fn set_sidebar_width(width: f32, cx: &mut gpui::App) {
    if cx.try_global::<SettingsSidebarWidth>().map(|w| w.0) != Some(width) {
        cx.set_global(SettingsSidebarWidth(width));
    }
}

fn sidebar_width(cx: &gpui::App) -> f32 {
    cx.try_global::<SettingsSidebarWidth>()
        .map_or(crate::settings::SIDEBAR_DEFAULT, |w| w.0)
}

/// The page pane: right of the section column, below the titlebar strip.
pub fn pane_bounds(viewport: gpui::Size<Pixels>, sidebar_width: f32) -> gpui::Bounds<Pixels> {
    let left = px(sidebar_width).min(viewport.width);
    let top = px(Theme::TITLEBAR_HEIGHT).min(viewport.height);
    gpui::Bounds::new(
        gpui::point(left, top),
        gpui::size(viewport.width - left, viewport.height - top),
    )
}

/// Inner width of the [`page_column`] at the current window size, for pages
/// that choose a layout by the room they get (the Devices grid).
pub fn column_width(window: &gpui::Window, cx: &gpui::App) -> f32 {
    let pane = f32::from(
        pane_bounds(window.viewport_size(), sidebar_width(cx))
            .size
            .width,
    );
    pane.min(PAGE_MAX_WIDTH) - 2.0 * PAGE_PAD_X
}

pub fn dropdown(
    id: impl Into<SharedString>,
    content: gpui::Div,
    closing: Option<std::time::Instant>,
    trigger_height: f32,
) -> AnyElement {
    SettingsDropdown {
        id: id.into(),
        content,
        closing,
        trigger_height,
    }
    .into_any_element()
}

#[derive(IntoElement)]
struct SettingsDropdown {
    id: SharedString,
    content: gpui::Div,
    closing: Option<std::time::Instant>,
    trigger_height: f32,
}

impl RenderOnce for SettingsDropdown {
    fn render(self, window: &mut gpui::Window, cx: &mut gpui::App) -> impl IntoElement {
        let limits = dropdown_limits(window.viewport_size(), sidebar_width(cx));
        popover::contained_menu(
            self.id,
            self.content,
            self.closing,
            self.trigger_height,
            limits,
        )
    }
}

fn dropdown_limits(viewport: gpui::Size<Pixels>, sidebar_width: f32) -> gpui::Bounds<Pixels> {
    let pane = pane_bounds(viewport, sidebar_width);
    gpui::Bounds::new(
        pane.origin + gpui::point(px(8.0), px(8.0)),
        gpui::size(
            (pane.size.width - px(16.0)).max(px(1.0)),
            (pane.size.height - px(16.0)).max(px(1.0)),
        ),
    )
}

/// Scroll only a dropdown's options, keeping its frame and optional heading
/// fixed. The available height follows the same placement budget as the card.
pub fn dropdown_rows(
    id: impl Into<SharedString>,
    rows: impl IntoIterator<Item = AnyElement>,
    trigger_height: f32,
    chrome_height: f32,
) -> AnyElement {
    DropdownRows {
        id: id.into(),
        rows: rows.into_iter().collect(),
        trigger_height,
        chrome_height,
    }
    .into_any_element()
}

pub fn dropdown_list_height(
    viewport: gpui::Size<Pixels>,
    trigger_height: f32,
    chrome_height: f32,
) -> f32 {
    // Height alone matters here, and the pane's does not depend on the
    // section column's width.
    let limits = pane_bounds(viewport, 0.0);
    let card_height = ((f32::from(limits.size.height) - 16.0 - trigger_height) / 2.0 - 6.0)
        .max(1.0)
        .min(320.0);
    (card_height - chrome_height).max(1.0)
}

#[derive(IntoElement)]
struct DropdownRows {
    id: SharedString,
    rows: Vec<AnyElement>,
    trigger_height: f32,
    chrome_height: f32,
}

impl RenderOnce for DropdownRows {
    fn render(self, window: &mut gpui::Window, _: &mut gpui::App) -> impl IntoElement {
        let key: SharedString = format!("{}-list-scroll", self.id).into();
        let scroll = window.with_global_id(key.into(), |id, window| {
            window.with_element_state(id, |previous: Option<ScrollHandle>, _| {
                let scroll = previous.unwrap_or_default();
                (scroll.clone(), scroll)
            })
        });
        let list = div()
            .id(format!("{}-list", self.id))
            .debug_selector(|| "settings-dropdown-list".into())
            .max_h(px(dropdown_list_height(
                window.viewport_size(),
                self.trigger_height,
                self.chrome_height,
            )))
            .overflow_y_scroll()
            .track_scroll(&scroll)
            .flex()
            .flex_col()
            .gap(px(2.0))
            .children(self.rows);
        crate::edge_fade::edge_faded(12.0, true, true, list).fade_overflow_y(&scroll)
    }
}

/// Owned scroll + floating-scrollbar state for one settings page.
///
/// This is the dedicated settings scroll container state. It wraps the same
/// `MenuScrollbarState` treatment as the model-picker (`pickers.rs`) and the
/// composer popups (`composer.rs`): the rail is hidden until hover/drag and
/// floats above content without consuming layout width.
///
/// Every settings page follows the same shape: a `scroll: PageScroll` field,
/// a [`ScrollRailHost`] impl on the page delegating here, and a root
/// `.relative()` host carrying only the list-hover `on_hover` around the
/// `.overflow_y_scroll().track_scroll(&self.scroll.scroll)` page, with
/// [`popover::rail`] supplying the rail and all of its listeners.
pub struct PageScroll {
    /// Tracked by the page's scrolling list directly — including the appshots
    /// half of the shortcuts page, which renders from its own file.
    pub scroll: ScrollHandle,
    bar: MenuScrollbarState,
}

impl Default for PageScroll {
    fn default() -> Self {
        Self {
            scroll: ScrollHandle::new(),
            bar: MenuScrollbarState::default(),
        }
    }
}

impl ScrollRailHost for PageScroll {
    fn rail_bar(&mut self) -> &mut MenuScrollbarState {
        &mut self.bar
    }

    fn rail_scroll(&self) -> Option<ScrollHandle> {
        Some(self.scroll.clone())
    }
}

impl PageScroll {
    pub fn set_list_hovered(&mut self, hovered: bool) -> bool {
        self.bar.set_list_hovered(hovered)
    }

    fn set_bar_hovered(&mut self, hovered: bool) -> bool {
        self.bar.set_bar_hovered(hovered)
    }

    fn begin_press(&mut self, pointer_y: Pixels) -> bool {
        self.bar.begin_press(&self.scroll, pointer_y)
    }

    fn drag_to(&self, pointer_y: Pixels) -> bool {
        self.bar.drag_to(&self.scroll, pointer_y)
    }

    fn end_press(&mut self) -> bool {
        self.bar.end_press()
    }

    /// Rewind to the top and drop the rail's activity baseline — what a page
    /// flip must do when one [`PageScroll`] is rerouted at a different list
    /// (shortcuts ↔ appshots), so the new page opens unscrolled and the
    /// offset jump is not read back as scrolling.
    pub fn reset(&mut self) {
        popover::reset_menu_scroll(&self.scroll, &mut self.bar);
    }

    /// Records scroll activity, then computes the rail geometry. Call once
    /// per render ([`rail`] pairs this with the hide-countdown scheduling).
    fn metrics(&mut self) -> Option<MenuScrollbarMetrics> {
        self.bar.note_scroll(&self.scroll);
        self.bar.metrics(&self.scroll)
    }
}

/// [`popover::rail`] for a [`PageScroll`] that is not its view's only rail:
/// `popover::rail` binds to the view's single [`ScrollRailHost`] impl, and a
/// page carrying a second, menu-local scroll host (the appearance page's
/// interface-font dropdown) cannot route that impl at both. The wiring
/// mirrors [`popover::rail`]; `reach` re-borrows the state from the view
/// inside the strip's pointer listeners, which fire with a `&mut V`.
pub fn rail<V: 'static>(
    scroll: &mut PageScroll,
    id: &'static str,
    theme: &Theme,
    cx: &mut Context<V>,
    reach: impl Fn(&mut V) -> &mut PageScroll + Copy + 'static,
) -> Option<AnyElement> {
    let metrics = scroll.metrics()?;
    popover::schedule_scrollbar_hide(&mut scroll.bar, cx);
    let strip = scroll.bar.render_rail(theme, metrics)?;
    Some(
        strip
            .id(id)
            .on_hover(cx.listener(move |view, hovered: &bool, _, cx| {
                if reach(view).set_bar_hovered(*hovered) {
                    cx.notify();
                }
            }))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |view, event: &gpui::MouseDownEvent, _, cx| {
                    if reach(view).begin_press(event.position.y) {
                        cx.stop_propagation();
                        cx.notify();
                    }
                }),
            )
            .on_drag(popover::MenuScrollbarDrag, |_, _, _, cx| {
                cx.stop_propagation();
                cx.new(|_| popover::MenuScrollbarDragGhost)
            })
            .on_drag_move(cx.listener(
                move |view, event: &gpui::DragMoveEvent<popover::MenuScrollbarDrag>, _, cx| {
                    if reach(view).drag_to(event.event.position.y) {
                        cx.notify();
                    }
                },
            ))
            .on_mouse_up_out(
                gpui::MouseButton::Left,
                cx.listener(move |view, _: &gpui::MouseUpEvent, _, cx| {
                    reach(view).end_press();
                    cx.notify();
                }),
            )
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(move |view, _: &gpui::MouseUpEvent, _, cx| {
                    reach(view).end_press();
                    cx.notify();
                }),
            )
            .into_any_element(),
    )
}

/// Shared typography for a settings component's title and description. The
/// Shortcuts page established this compact rhythm; list-style settings reuse
/// it instead of drifting by page.
pub const ROW_TITLE_SIZE: f32 = 13.0;
pub const ROW_DESCRIPTION_SIZE: f32 = 12.0;

/// The page column's outer max width and side padding: a centered ~680px
/// content measure with generous air on either side.
const PAGE_MAX_WIDTH: f32 = 760.0;
const PAGE_PAD_X: f32 = 40.0;

/// Centered page column under the titlebar strip.
pub fn page_column() -> gpui::Div {
    div()
        .w_full()
        .max_w(px(PAGE_MAX_WIDTH))
        .mx_auto()
        .px(px(PAGE_PAD_X))
        // Titlebar clearance lives inside the scroll so content can scroll
        // up to the window edge and fade there, mirroring the bottom.
        .pt(px(Theme::TITLEBAR_HEIGHT + 16.0))
        .pb(px(48.0))
        .flex()
        .flex_col()
}

/// Page headline row: a large title, optionally followed by a muted count on
/// the same baseline. Inset to line up with the section labels.
pub fn page_header(theme: &Theme, title: &str, count: Option<usize>) -> gpui::Div {
    div()
        .px(px(SECTION_LABEL_INSET))
        .flex()
        .flex_row()
        .items_baseline()
        .gap(px(10.0))
        .child(
            div()
                .text_size(crate::typography::ui_rems(20.0))
                .line_height(crate::typography::ui_rems(26.0))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(theme.text)
                .child(SharedString::from(title.to_string())),
        )
        .when_some(count, |el, count| {
            el.child(
                div()
                    .text_size(crate::typography::ui_rems(13.0))
                    .text_color(theme.text_muted)
                    .child(SharedString::from(format!("{count}"))),
            )
        })
}

/// Subtitle under the headline: `mt-1 text-[13px] text-muted-foreground`.
pub fn page_subtitle(theme: &Theme, copy: impl Into<SharedString>) -> gpui::Div {
    div()
        .mt(px(4.0))
        .px(px(SECTION_LABEL_INSET))
        .min_w_0()
        .text_size(crate::typography::ui_rems(13.0))
        .line_height(crate::typography::ui_rems(17.0))
        .text_color(theme.text_muted)
        .child(copy.into())
}

/// Small label above a group of controls (`text-[13px] font-medium`) — the
/// "Theme" caption over a picker, not a page headline.
pub fn field_label(theme: &Theme, label: impl Into<SharedString>) -> gpui::Div {
    div()
        .text_size(crate::typography::ui_rems(13.0))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(theme.text)
        .child(label.into())
}

/// A row of equally-sized preview cards for picking one of N *visual* options.
///
/// Deliberately knows nothing about themes: the caller supplies each preview as
/// an arbitrary element and picks however many cards it wants, so the same
/// control works for a density picker, a layout picker or anything else where
/// the choice is easier to show than to describe. Pair with [`option_card`].
pub fn option_card_row() -> gpui::Div {
    div().flex().flex_row().items_start().gap(px(16.0)).w_full()
}

/// Default height of an [`option_card`] preview frame.
pub const OPTION_CARD_HEIGHT: f32 = 148.0;
/// Corner radius of the preview frame.
///
/// Public because the preview has to round *itself* to this. gpui content masks
/// are axis-aligned rectangles, so `overflow_hidden` on the frame clips to its
/// bounding box and not to its corner radius — a preview that paints its own
/// background will square off the corners and cover the frame's border with it.
pub const OPTION_CARD_RADIUS: f32 = 6.0;

/// One card in an [`option_card_row`]: a fixed-height preview frame with a quiet
/// selected edge and caption underneath. There is deliberately no outer card
/// or ring; the preview itself is the control.
///
/// `preview` fills the frame and **must round its own corners** to
/// [`OPTION_CARD_RADIUS`] if it paints a background — see that constant.
///
/// Returns a plain `Div` like the rest of this module — the caller adds `.id(..)`
/// and `.on_click(..)`, so selection behaviour stays with the page that owns the
/// state.
pub fn option_card(
    theme: &Theme,
    icon_path: &'static str,
    label: impl Into<SharedString>,
    selected: bool,
    selection_t: f32,
    preview: AnyElement,
) -> gpui::Div {
    let selected_color = crate::motion::mix(theme.text_muted, theme.accent, selection_t);
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(8.0))
        .cursor_pointer()
        .child(
            div()
                .h(px(OPTION_CARD_HEIGHT))
                .flex_none()
                .w_full()
                .rounded(px(OPTION_CARD_RADIUS))
                .overflow_hidden()
                .border_1()
                .border_color(crate::motion::mix(theme.border, theme.accent, selection_t))
                .child(preview),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .text_size(crate::typography::ui_rems(13.0))
                .font_weight(if selected {
                    gpui::FontWeight::MEDIUM
                } else {
                    gpui::FontWeight::NORMAL
                })
                .text_color(selected_color)
                .child(
                    crate::icons::icon(icon_path)
                        .size(px(16.0))
                        .text_color(selected_color),
                )
                .child(label.into()),
        )
}

/// Section labels and page titles sit a little inside the blocks' edge.
const SECTION_LABEL_INSET: f32 = 8.0;
/// Air between one section and the next.
const SECTION_GAP: f32 = 32.0;

/// The small plain label above a block ("Startup", "Fonts"): sentence case,
/// muted, never an uppercase eyebrow.
pub fn section_label(theme: &Theme, label: impl Into<SharedString>) -> gpui::Div {
    div()
        .px(px(SECTION_LABEL_INSET))
        .text_size(crate::typography::ui_rems(13.0))
        .line_height(crate::typography::ui_rems(17.0))
        .text_color(theme.text_muted)
        .child(label.into())
}

/// A [`section_label`] for a group nested inside a block row (an expanded
/// provider's "Completion", "Accounts"): flush with the row text it sits
/// under, and as tall as a small button so a trailing action never shifts it.
pub fn details_label(theme: &Theme, label: impl Into<SharedString>) -> gpui::Div {
    div()
        .min_h(px(32.0))
        .flex()
        .flex_row()
        .items_center()
        .text_size(crate::typography::ui_rems(13.0))
        .line_height(crate::typography::ui_rems(17.0))
        .text_color(theme.text_muted)
        .child(label.into())
}

/// A labeled section: the label, then its block tucked close underneath.
/// Pass a [`section_card`] (or any block) as `block`.
pub fn section(
    theme: &Theme,
    label: impl Into<SharedString>,
    block: impl IntoElement,
) -> gpui::Div {
    div()
        .mt(px(SECTION_GAP))
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(section_label(theme, label))
        .child(block)
}

/// The block's fill: a faint solid tint of the theme's ink, no outline.
pub fn block_fill(theme: &Theme) -> gpui::Hsla {
    theme.wash(0.045)
}

/// The hairline between rows inside a block.
pub fn row_divider(theme: &Theme) -> gpui::Hsla {
    theme.border.opacity(0.6)
}

/// A filled, rounded block of related settings rows. Standalone blocks keep
/// a section's worth of air above them; inside [`section`] the wrapper owns
/// the spacing and zeroes this margin.
pub fn section_card(theme: &Theme) -> gpui::Div {
    div()
        .mt(px(24.0))
        .rounded(px(12.0))
        .bg(block_fill(theme))
        .overflow_hidden()
        .flex()
        .flex_col()
}

/// One row in a [`section_card`]: title and muted description on the left,
/// the control on the right, split from the previous row by a hairline
/// inset to the text edge.
pub fn card_row(theme: &Theme, first: bool) -> gpui::Div {
    div()
        .mx(px(16.0))
        .py(px(12.0))
        .min_h(px(60.0))
        .when(!first, |el| {
            el.border_t_1().border_color(row_divider(theme))
        })
        .flex()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(16.0))
}

/// Bare leading glyph for rows whose icon carries identity (a device's
/// platform), sized to sit beside a two-line title.
pub fn row_tile(theme: &Theme, icon_path: &'static str) -> gpui::Div {
    div()
        .flex_none()
        .w(px(24.0))
        .h(px(32.0))
        .flex()
        .items_center()
        .justify_center()
        .child(
            crate::icons::icon(icon_path)
                .size(px(16.0))
                .text_color(theme.text_muted),
        )
}

/// Row title. These metrics intentionally match the Shortcuts rows, whose
/// title/description rhythm is the reference for the other settings cards.
pub fn row_title(theme: &Theme, title: impl Into<SharedString>) -> gpui::Div {
    div()
        .min_w_0()
        .text_size(crate::typography::ui_rems(ROW_TITLE_SIZE))
        .line_height(crate::typography::ui_rems(17.0))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(theme.text)
        .child(title.into())
}

/// The quiet meta line under a row title: `text-[12px]
/// text-muted-foreground/65` fragments joined by dots.
pub fn meta_line(theme: &Theme, fragments: Vec<AnyElement>) -> gpui::Div {
    let mut line = div()
        .mt(px(Theme::TEXT_STACK_GAP))
        .min_w_0()
        .max_w_full()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap_x(px(8.0))
        .gap_y(px(2.0))
        .text_size(crate::typography::ui_rems(ROW_DESCRIPTION_SIZE))
        .line_height(crate::typography::ui_rems(16.0))
        .text_color(theme.text_muted);
    let mut first = true;
    for fragment in fragments {
        if !first {
            line = line.child(
                div()
                    .text_color(theme.text_muted.opacity(0.3))
                    .child(SharedString::from("·")),
            );
        }
        line = line.child(div().min_w_0().max_w_full().child(fragment));
        first = false;
    }
    line
}

/// Right-anchored badge pill: a filled wash, like the settings blocks it
/// sits on (no outline).
pub fn badge(theme: &Theme, label: impl Into<SharedString>) -> gpui::Div {
    div()
        .flex_none()
        .px(px(8.0))
        .py(px(2.0))
        .rounded_full()
        .bg(theme.wash(0.08))
        .text_size(crate::typography::ui_rems(10.5))
        .text_color(theme.text_muted)
        .child(label.into())
}

/// Emerald status pill (the Accounts "Active" badge:
/// `bg-emerald-400/[0.12] text-emerald-300/90`).
pub fn badge_active(theme: &Theme, label: impl Into<SharedString>) -> gpui::Div {
    let emerald = theme.success;
    let emerald_text = theme.success_muted; // emerald-300
    div()
        .flex_none()
        .px(px(8.0))
        .py(px(2.0))
        .rounded_full()
        .bg(emerald.opacity(0.12))
        .text_size(crate::typography::ui_rems(10.5))
        .text_color(emerald_text.opacity(0.9))
        .child(label.into())
}

pub const SWITCH_WIDTH: f32 = 44.8;
const SWITCH_HEIGHT: f32 = 28.8;
const SWITCH_TRACK_HEIGHT: f32 = 20.8;
const SWITCH_SIDE_INSET: f32 = 1.6;
const SWITCH_THUMB_WIDTH: f32 = 24.0;
const SWITCH_THUMB_HEIGHT: f32 = SWITCH_TRACK_HEIGHT - 2.0 * SWITCH_SIDE_INSET;
const SWITCH_MARK_SIZE: f32 = 7.2;

/// A pill switch with the on/off marks nested beneath a sliding thumb.
/// The caller owns activation and accessibility; only the thumb interpolates.
pub fn toggle_switch(theme: &Theme, on: bool, key: impl Into<SharedString>) -> gpui::Div {
    let key: SharedString = key.into();
    div()
        .flex_none()
        .w(px(SWITCH_WIDTH))
        .h(px(SWITCH_HEIGHT))
        .child(SwitchVisual {
            theme: theme.clone(),
            on,
            key: format!("settings-switch-{key}").into(),
        })
}

#[derive(IntoElement)]
struct SwitchVisual {
    theme: Theme,
    on: bool,
    key: SharedString,
}

struct SwitchTravel {
    from: f32,
    target: f32,
    started: std::time::Instant,
}

impl SwitchTravel {
    fn value(&self, now: std::time::Instant) -> f32 {
        let t = (now.duration_since(self.started).as_secs_f32() / 0.18).min(1.0);
        self.from + (self.target - self.from) * (1.0 - (1.0 - t).powi(3))
    }
}

fn switch_track_color(theme: &Theme, on: bool) -> gpui::Hsla {
    let dark = theme.appearance.is_dark();
    if on {
        if dark {
            // Keep the accent saturated and opaque, but give the enabled
            // track more depth against the dark settings surface.
            crate::theme::flatten(gpui::black().opacity(0.14), theme.accent_strong)
        } else {
            // Preserve the current light opaque treatment.
            let accent = theme.accent;
            crate::theme::flatten(
                gpui::hsla(accent.h, accent.s, accent.l + (1.0 - accent.l) * 0.10, 0.98),
                theme.surface,
            )
        }
    } else {
        let opacity = match (dark, theme.is_frost()) {
            (true, true) => 0.22,
            (true, false) => 0.18,
            (false, true) => 0.12,
            (false, false) => 0.10,
        };
        crate::theme::flatten(theme.ink(opacity), theme.surface)
    }
}

fn switch_thumb_color(theme: &Theme) -> gpui::Hsla {
    let white = if theme.is_frost() {
        if theme.appearance.is_dark() {
            0.94
        } else {
            0.96
        }
    } else if theme.appearance.is_dark() {
        0.96
    } else {
        1.0
    };
    crate::theme::flatten(gpui::white().opacity(white), theme.surface)
}

/// Frosted switches catch a little light across their rim and thumb. Both
/// gradient stops are composited to opaque colors before painting.
fn switch_surface_tones(theme: &Theme, base: gpui::Hsla, thumb: bool) -> (gpui::Hsla, gpui::Hsla) {
    if !theme.is_frost() {
        return (base, base);
    }
    let (light, shade) = if thumb { (0.12, 0.07) } else { (0.07, 0.09) };
    (
        crate::theme::flatten(gpui::white().opacity(light), base),
        crate::theme::flatten(gpui::black().opacity(shade), base),
    )
}

impl RenderOnce for SwitchVisual {
    fn render(self, window: &mut gpui::Window, cx: &mut gpui::App) -> impl IntoElement {
        let now = std::time::Instant::now();
        let target = if self.on { 1.0 } else { 0.0 };
        let reduced = crate::motion::reduced_motion(cx);
        let position = window.with_global_id(self.key.into(), |id, window| {
            window.with_element_state(id, |previous: Option<SwitchTravel>, _| {
                let mut travel = previous.unwrap_or(SwitchTravel {
                    from: target,
                    target,
                    started: now,
                });
                let current = travel.value(now);
                if travel.target != target {
                    travel = SwitchTravel {
                        from: current,
                        target,
                        started: now,
                    };
                }
                if reduced {
                    travel.from = target;
                    travel.target = target;
                }
                (travel.value(now), travel)
            })
        });
        if (position - target).abs() > 0.001 {
            window.request_animation_frame();
        }
        let dark = self.theme.appearance.is_dark();
        let track = switch_track_color(&self.theme, self.on);
        let (track_light, track_shade) = switch_surface_tones(&self.theme, track, false);
        let thumb = switch_thumb_color(&self.theme);
        let (thumb_light, thumb_shade) = switch_surface_tones(&self.theme, thumb, true);
        let empty_width = SWITCH_WIDTH - SWITCH_THUMB_WIDTH - SWITCH_SIDE_INSET;
        let mark_padding = (empty_width - SWITCH_MARK_SIZE) / 2.0;
        let thumb_left = SWITCH_SIDE_INSET
            + (SWITCH_WIDTH - SWITCH_THUMB_WIDTH - 2.0 * SWITCH_SIDE_INSET) * position;
        let track_element = div()
            .absolute()
            .top(px((SWITCH_HEIGHT - SWITCH_TRACK_HEIGHT) / 2.0))
            .left_0()
            .w(px(SWITCH_WIDTH))
            .h(px(SWITCH_TRACK_HEIGHT))
            .rounded_full()
            .bg(gpui::linear_gradient(
                180.0,
                gpui::linear_color_stop(track_light, 0.0),
                gpui::linear_color_stop(track_shade, 1.0),
            ))
            .border_1()
            .border_color(if self.on {
                crate::theme::flatten(
                    gpui::white().opacity(if self.theme.is_frost() { 0.16 } else { 0.12 }),
                    track,
                )
            } else {
                crate::theme::flatten(self.theme.border, track)
            })
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .px(px(mark_padding))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .size(px(SWITCH_MARK_SIZE))
                            .flex()
                            .items_center()
                            .justify_center()
                            .opacity(position)
                            .child(
                                div()
                                    .w(px(1.2))
                                    .h(px(7.2))
                                    .rounded_full()
                                    .bg(gpui::white().opacity(0.96)),
                            ),
                    )
                    .child(
                        div()
                            .size(px(SWITCH_MARK_SIZE))
                            .flex()
                            .items_center()
                            .justify_center()
                            .opacity(1.0 - position)
                            .child(
                                div()
                                    .size(px(6.4))
                                    .rounded_full()
                                    .border(px(1.0))
                                    .border_color(gpui::white().opacity(0.92)),
                            ),
                    ),
            );
        let thumb_element = div()
            .absolute()
            .top(px((SWITCH_HEIGHT - SWITCH_THUMB_HEIGHT) / 2.0))
            .left(px(thumb_left))
            .w(px(SWITCH_THUMB_WIDTH))
            .h(px(SWITCH_THUMB_HEIGHT))
            .rounded_full()
            .bg(gpui::linear_gradient(
                180.0,
                gpui::linear_color_stop(thumb_light, 0.0),
                gpui::linear_color_stop(thumb_shade, 1.0),
            ))
            .border_1()
            .border_color(crate::theme::flatten(
                gpui::black().opacity(if dark { 0.10 } else { 0.08 }),
                thumb,
            ))
            // The rim highlight only belongs to the on state; it fades with
            // the thumb's travel so switching off doesn't pop.
            .when(self.theme.is_frost() && position > 0.001, |el| {
                el.child(
                    div()
                        .absolute()
                        .top(px(1.6))
                        .left(px(7.2))
                        .w(px(9.6))
                        .h(px(1.0))
                        .opacity(position)
                        .rounded_full()
                        .bg(crate::theme::flatten(
                            gpui::white().opacity(0.45),
                            thumb_light,
                        )),
                )
            });
        div()
            .relative()
            .w(px(SWITCH_WIDTH))
            .h(px(SWITCH_HEIGHT))
            .child(track_element)
            .child(thumb_element)
    }
}

#[cfg(test)]
mod switch_tests {
    use super::*;

    #[test]
    fn tab_selection_reverses_from_its_current_opacity() {
        use std::time::{Duration, Instant};

        let start = Instant::now();
        let forward = TabSelectionTravel {
            from: 0.0,
            target: 1.0,
            started: start,
        };
        let halfway = start + Duration::from_millis(75);
        let current = forward.value(halfway);
        let reverse = TabSelectionTravel {
            from: current,
            target: 0.0,
            started: halfway,
        };
        assert_eq!(reverse.value(halfway), current);
        assert_eq!(
            reverse.value(
                halfway
                    + crate::motion::TAB_SLIDE
                        .total()
                        .mul_f32(crate::motion::speed_scale())
            ),
            0.0
        );
    }

    #[test]
    fn dropdowns_stay_in_the_settings_content_pane() {
        for (width, sidebar) in [(600.0, 224.0), (1200.0, 256.0), (1200.0, 400.0)] {
            let viewport = gpui::size(px(width), px(700.0));
            let pane = pane_bounds(viewport, sidebar);
            let limits = dropdown_limits(viewport, sidebar);
            assert_eq!(pane.left(), px(sidebar));
            assert_eq!(pane.right(), px(width));
            assert!(limits.left() >= pane.left());
            assert!(limits.right() <= pane.right());
            assert!(limits.top() >= px(Theme::TITLEBAR_HEIGHT));
            assert!(limits.bottom() <= pane.bottom());
        }
    }

    #[test]
    fn switch_material_keeps_dark_accent_and_opaque_fills() {
        use zeron_theme::SurfaceTreatment;

        let mut dark = Theme::dark();
        dark.surface_treatment = SurfaceTreatment::Opaque;
        let dark_on = switch_track_color(&dark, true);
        assert_eq!(dark_on.a, 1.0);
        assert!(dark_on.l < dark.accent_strong.l);
        assert_eq!(switch_track_color(&dark, false).a, 1.0);
        assert_eq!(switch_thumb_color(&dark).a, 1.0);
        assert_eq!(
            switch_surface_tones(&dark, dark_on, false),
            (dark_on, dark_on)
        );
        let opaque_off = switch_track_color(&dark, false);

        dark.surface_treatment = SurfaceTreatment::Frosted;
        assert_eq!(switch_track_color(&dark, true), dark_on);
        assert_eq!(switch_track_color(&dark, false).a, 1.0);
        assert_eq!(switch_thumb_color(&dark).a, 1.0);
        assert_ne!(switch_track_color(&dark, false), opaque_off);
        for (base, thumb) in [(dark_on, false), (switch_thumb_color(&dark), true)] {
            let (light, shade) = switch_surface_tones(&dark, base, thumb);
            assert_eq!((light.a, shade.a), (1.0, 1.0));
            assert!(light.l > base.l && shade.l < base.l);
        }

        let mut light = Theme::light();
        light.surface_treatment = SurfaceTreatment::Opaque;
        assert_eq!(switch_track_color(&light, true).a, 1.0);
        assert!(switch_track_color(&light, true).l > light.accent.l);
        let light_on = switch_track_color(&light, true);
        assert_eq!(
            switch_surface_tones(&light, light_on, false),
            (light_on, light_on)
        );
    }

    #[test]
    fn switch_reversal_keeps_current_position_and_settles() {
        use std::time::{Duration, Instant};
        let now = Instant::now();
        let forward = SwitchTravel {
            from: 0.0,
            target: 1.0,
            started: now,
        };
        let halfway = now + Duration::from_millis(90);
        let current = forward.value(halfway);
        let reverse = SwitchTravel {
            from: current,
            target: 0.0,
            started: halfway,
        };
        assert_eq!(reverse.value(halfway), current);
        assert_eq!(reverse.value(halfway + Duration::from_millis(180)), 0.0);
        assert_eq!(forward.value(now + Duration::from_millis(180)), 1.0);
    }
}

struct TabSelectionTravel {
    from: f32,
    target: f32,
    started: std::time::Instant,
}

impl TabSelectionTravel {
    fn value(&self, now: std::time::Instant) -> f32 {
        let seconds = crate::motion::TAB_SLIDE.total().as_secs_f32() * crate::motion::speed_scale();
        let elapsed = now.saturating_duration_since(self.started).as_secs_f32();
        let progress = crate::motion::TAB_SLIDE.progress((elapsed / seconds).min(1.0));
        self.from + (self.target - self.from) * progress
    }
}

/// Retargetable selected-state fade for a settings tab. First paint and
/// reduced-motion changes settle immediately instead of flashing an entrance.
pub fn tab_selection_t(
    window: &mut gpui::Window,
    key: impl Into<SharedString>,
    selected: bool,
    reduced_motion: bool,
) -> f32 {
    let now = std::time::Instant::now();
    let target = if selected { 1.0 } else { 0.0 };
    let value = window.with_global_id(key.into().into(), |id, window| {
        window.with_element_state(id, |previous: Option<TabSelectionTravel>, _| {
            let mut travel = previous.unwrap_or(TabSelectionTravel {
                from: target,
                target,
                started: now,
            });
            let current = travel.value(now);
            if travel.target != target {
                travel = TabSelectionTravel {
                    from: current,
                    target,
                    started: now,
                };
            }
            if reduced_motion {
                travel.from = target;
                travel.target = target;
            }
            (travel.value(now), travel)
        })
    });
    if (value - target).abs() > 0.001 {
        window.request_animation_frame();
    }
    value
}

/// One treatment for settings section tabs: the selected wash and text ease
/// over Zeron's tab timing, while hover keeps the normal sidebar color fade.
pub fn section_tab(
    theme: &Theme,
    selected: bool,
    selection_t: f32,
    id: impl Into<SharedString>,
    hover_key: impl Into<SharedString>,
) -> gpui::Stateful<gpui::Div> {
    let hover_key = hover_key.into();
    let base_bg = crate::motion::mix(
        crate::theme::wash(0.0),
        crate::theme::glass_selected_bg(),
        selection_t,
    );
    let base_text = crate::motion::mix(theme.text_muted, theme.text, selection_t);
    let hover_bg = if selected {
        base_bg
    } else {
        theme.glass_hover()
    };
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .rounded(px(8.0))
        .px(px(Theme::SPACE_SM))
        .py(px(6.0))
        .min_h(px(32.0))
        .flex_shrink_0()
        .text_size(crate::typography::ui_rems(13.0))
        .when(selected, |el| el.font_weight(gpui::FontWeight::MEDIUM))
        .text_color(crate::motion::hover_blend(
            &hover_key, base_text, theme.text,
        ))
        .bg(crate::motion::hover_blend(&hover_key, base_bg, hover_bg))
        .id(id.into())
        .on_hover(crate::motion::hover_listener(hover_key))
}

/// Height of every settings select trigger: centered in a 60px card row.
pub const SELECT_HEIGHT: f32 = 32.0;
/// Menus never shrink below this, so short values still open a readable list.
const SELECT_MENU_MIN_WIDTH: f32 = 160.0;

/// A select's fill on a settings block: a translucent wash one step above
/// [`block_fill`], lifting while hovered or open. Washes read through frost
/// and stay tonal on solid surfaces, in both appearances.
pub(crate) fn select_fill(theme: &Theme, lifted: bool) -> gpui::Hsla {
    theme.wash(if lifted { 0.10 } else { 0.06 })
}

/// The trigger of every settings dropdown: a borderless filled pill. The
/// caller adds the value ([`select_value`], after any leading glyph), then
/// [`select_chevron`], plus its own listeners. The transparent edge is held
/// for the focus ring, so focusing never shifts the label.
pub fn select_trigger(
    theme: &Theme,
    id: impl Into<SharedString>,
    open: bool,
) -> gpui::Stateful<gpui::Div> {
    let id: SharedString = id.into();
    let hover_key: SharedString = format!("{id}-hover").into();
    let lifted = select_fill(theme, true);
    let accent = theme.accent;
    div()
        .id(id)
        .relative()
        .flex_none()
        .max_w_full()
        .h(px(SELECT_HEIGHT))
        .pl(px(10.0))
        .pr(px(8.0))
        .rounded(px(8.0))
        .border_1()
        .border_color(gpui::transparent_black())
        .bg(if open {
            lifted
        } else {
            crate::motion::hover_blend(&hover_key, select_fill(theme, false), lifted)
        })
        .on_hover(crate::motion::hover_listener(hover_key))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .cursor_pointer()
        .text_size(crate::typography::ui_rems(12.5))
        .text_color(theme.text)
        .tab_index(0)
        .role(gpui::Role::Button)
        .aria_expanded(open)
        .focus_visible(move |s| s.border_color(accent))
}

/// The trigger's current value, truncating before the chevron.
pub fn select_value(label: impl Into<SharedString>) -> gpui::Div {
    div().flex_1().min_w_0().truncate().child(label.into())
}

pub fn select_chevron(theme: &Theme, open: bool) -> gpui::Svg {
    crate::icons::icon(crate::icons::ALT_ARROW_DOWN)
        .size(px(14.0))
        .flex_none()
        .text_color(if open { theme.text } else { theme.text_muted })
}

/// The open menu: the shared popover card, which [`dropdown`] mounts inside
/// the frosted menu layer (an opaque card where frost is off).
pub fn select_menu(theme: &Theme) -> gpui::Div {
    popover::popover_card(theme).flex().flex_col()
}

/// One option row. `highlighted` is the keyboard cursor; the caller adds the
/// label, then [`select_check`].
pub fn select_row(
    theme: &Theme,
    selected: bool,
    highlighted: bool,
    fade_key: impl Into<SharedString>,
) -> gpui::Div {
    popover::menu_row_nav(theme, selected, highlighted, fade_key)
}

/// The trailing check slot, kept on every row so labels align.
pub fn select_check(theme: &Theme, selected: bool) -> gpui::Div {
    div()
        .w(px(18.0))
        .flex_none()
        .flex()
        .justify_end()
        .when(selected, |slot| {
            slot.child(
                crate::icons::icon(crate::icons::CHECK)
                    .size(px(14.0))
                    .text_color(theme.accent),
            )
        })
}

/// Open state for one [`select`], owned by its page: the popup with its exit
/// phase, and the keyboard highlight.
#[derive(Default)]
pub struct SelectState {
    menu: popover::Popup<()>,
    highlighted: usize,
}

impl SelectState {
    pub fn is_open(&self) -> bool {
        self.menu.is_open()
    }

    /// Open with the keyboard highlight on option `highlighted`.
    pub fn open(&mut self, highlighted: usize) {
        self.highlighted = highlighted;
        self.menu.open(());
    }
}

/// Close a [`select`] from outside it — another menu on the page opening.
/// `reach` as in [`select`].
pub fn close_select<V: 'static>(
    view: &mut V,
    reach: impl Fn(&mut V) -> &mut SelectState + 'static,
    cx: &mut Context<V>,
) {
    let reach: SelectReach<V> = Rc::new(reach);
    close_select_rc(view, &reach, cx);
}

/// One option in a [`select`] menu.
pub struct SelectOption {
    label: SharedString,
    leading: Option<Rc<dyn Fn() -> AnyElement>>,
    detail: Option<SharedString>,
}

impl SelectOption {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            leading: None,
            detail: None,
        }
    }

    /// A glyph or swatch before the label, in the row and (when selected)
    /// the trigger.
    pub fn leading(mut self, leading: impl Fn() -> AnyElement + 'static) -> Self {
        self.leading = Some(Rc::new(leading));
        self
    }

    /// A quiet note after the label, in the row only.
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

type SelectReach<V> = Rc<dyn Fn(&mut V) -> &mut SelectState>;
type SelectHandler<V> = Rc<dyn Fn(&mut V, usize, &mut Window, &mut Context<V>)>;

/// The settings dropdown: a [`select_trigger`] opening a [`select_menu`] of
/// options with a check on the current one. Pointer: the trigger toggles and
/// an outside press dismisses. Keyboard: ↑/↓ open and move the highlight,
/// Home/End jump, Enter/Space open or commit, Escape closes. `reach`
/// re-borrows the page's [`SelectState`] inside listeners; `label` names the
/// control for assistive tech, which hears "label: value".
pub fn select<V: 'static>(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    theme: &Theme,
    reach: impl Fn(&mut V) -> &mut SelectState + 'static,
) -> Select<V> {
    Select {
        id: id.into(),
        label: label.into(),
        theme: theme.clone(),
        options: Vec::new(),
        selected: 0,
        width: None,
        menu_width: None,
        font: None,
        heading: None,
        reach: Rc::new(reach),
        on_select: None,
    }
}

pub struct Select<V: 'static> {
    id: SharedString,
    label: SharedString,
    theme: Theme,
    options: Vec<SelectOption>,
    selected: usize,
    width: Option<f32>,
    menu_width: Option<f32>,
    font: Option<SharedString>,
    heading: Option<&'static str>,
    reach: SelectReach<V>,
    on_select: Option<SelectHandler<V>>,
}

fn close_select_rc<V: 'static>(view: &mut V, reach: &SelectReach<V>, cx: &mut Context<V>) {
    if reach(view).menu.begin_close() {
        let reach = reach.clone();
        popover::reap_popup(cx, move |view: &mut V| &mut reach(view).menu);
    }
    cx.notify();
}

fn commit_select<V: 'static>(
    view: &mut V,
    ix: usize,
    reach: &SelectReach<V>,
    on_select: &Option<SelectHandler<V>>,
    window: &mut Window,
    cx: &mut Context<V>,
) {
    close_select_rc(view, reach, cx);
    if let Some(on_select) = on_select {
        on_select(view, ix, window, cx);
    }
}

impl<V: 'static> Select<V> {
    pub fn options(
        mut self,
        options: impl IntoIterator<Item = SelectOption>,
        selected: usize,
    ) -> Self {
        self.options = options.into_iter().collect();
        self.selected = selected;
        self
    }

    pub fn on_select(
        mut self,
        on_select: impl Fn(&mut V, usize, &mut Window, &mut Context<V>) + 'static,
    ) -> Self {
        self.on_select = Some(Rc::new(on_select));
        self
    }

    /// Fixed trigger width; the menu matches it unless [`Self::menu_width`]
    /// says otherwise. Without one the trigger fits its value.
    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }

    pub fn menu_width(mut self, width: f32) -> Self {
        self.menu_width = Some(width);
        self
    }

    /// Font for the value and the rows (key names read in mono).
    pub fn font_family(mut self, font: SharedString) -> Self {
        self.font = Some(font);
        self
    }

    /// A small heading over the rows.
    pub fn heading(mut self, heading: &'static str) -> Self {
        self.heading = Some(heading);
        self
    }

    pub fn render(self, state: &SelectState, cx: &mut Context<V>) -> gpui::Stateful<gpui::Div> {
        let Self {
            id,
            label,
            theme,
            options,
            selected,
            width,
            menu_width,
            font,
            heading,
            reach,
            on_select,
        } = self;
        let count = options.len();
        let open = state.menu.is_open();
        let current = options.get(selected);
        let value = current
            .map(|option| option.label.clone())
            .unwrap_or_default();
        let trigger_leading = current.and_then(|option| option.leading.clone());

        let mut trigger = select_trigger(&theme, id.clone(), open)
            .debug_selector({
                let id = id.clone();
                move || id.to_string()
            })
            .aria_label(format!("{label}: {value}"))
            .when_some(width, |el, width| el.w(px(width)))
            .when_some(font.clone(), |el, font| el.font_family(font))
            .on_mouse_down(gpui::MouseButton::Left, {
                let reach = reach.clone();
                cx.listener(move |view, _, _, _| reach(view).menu.note_trigger_press())
            })
            .on_click({
                let reach = reach.clone();
                let on_select = on_select.clone();
                cx.listener(move |view, event: &gpui::ClickEvent, window, cx| {
                    let state = reach(view);
                    if event.is_keyboard() && state.menu.is_open() {
                        let ix = state.highlighted;
                        commit_select(view, ix, &reach, &on_select, window, cx);
                    } else if state.menu.take_press_was_open() || state.menu.is_open() {
                        close_select_rc(view, &reach, cx);
                    } else {
                        state.open(selected);
                        cx.notify();
                    }
                })
            })
            // Enter/Space arrive as keyboard clicks above; this handles
            // movement and dismissal.
            .on_key_down({
                let reach = reach.clone();
                cx.listener(move |view, event: &gpui::KeyDownEvent, _, cx| {
                    let key = event.keystroke.key.as_str();
                    let modifiers = event.keystroke.modifiers;
                    let menu_key =
                        popover::classify_key(key, modifiers.platform, modifiers.control);
                    let state = reach(view);
                    if !state.menu.is_open() {
                        if matches!(menu_key, popover::MenuKey::Up | popover::MenuKey::Down) {
                            state.open(selected);
                            cx.stop_propagation();
                            cx.notify();
                        }
                        return;
                    }
                    let current = Some(state.highlighted.min(count.saturating_sub(1)));
                    state.highlighted = match (key, menu_key) {
                        (_, popover::MenuKey::Up) => {
                            popover::menu_step(current, count, -1).unwrap_or_default()
                        }
                        (_, popover::MenuKey::Down) => {
                            popover::menu_step(current, count, 1).unwrap_or_default()
                        }
                        ("home", _) => 0,
                        ("end", _) => count.saturating_sub(1),
                        (_, popover::MenuKey::Escape) => {
                            cx.stop_propagation();
                            close_select_rc(view, &reach, cx);
                            return;
                        }
                        _ => return,
                    };
                    cx.stop_propagation();
                    cx.notify();
                })
            })
            .children(trigger_leading.map(|leading| leading()))
            .child(select_value(value))
            .child(select_chevron(&theme, open));

        if state.menu.get().is_some() {
            let highlighted = state.highlighted;
            let rows = options.into_iter().enumerate().map(|(ix, option)| {
                let row_id: SharedString = format!("{id}-option-{ix}").into();
                let reach = reach.clone();
                let on_select = on_select.clone();
                select_row(&theme, ix == selected, ix == highlighted, row_id.clone())
                    .id(row_id.clone())
                    .debug_selector(move || row_id.to_string())
                    .role(gpui::Role::MenuItemRadio)
                    .aria_label(option.label.clone())
                    .aria_toggled(if ix == selected {
                        gpui::Toggled::True
                    } else {
                        gpui::Toggled::False
                    })
                    .on_click(cx.listener(move |view, _, window, cx| {
                        cx.stop_propagation();
                        commit_select(view, ix, &reach, &on_select, window, cx);
                    }))
                    .children(option.leading.map(|leading| leading()))
                    .child(div().flex_1().min_w_0().truncate().child(option.label))
                    .when_some(option.detail, |row, detail| {
                        row.child(
                            div()
                                .flex_none()
                                .text_size(crate::typography::ui_rems(10.5))
                                .text_color(theme.text_muted)
                                .child(detail),
                        )
                    })
                    .child(select_check(&theme, ix == selected))
                    .into_any_element()
            });
            let menu = select_menu(&theme)
                .w(px(menu_width
                    .or(width)
                    .unwrap_or(0.0)
                    .max(SELECT_MENU_MIN_WIDTH)))
                .when_some(font, |menu, font| menu.font_family(font))
                .on_mouse_down_out({
                    let reach = reach.clone();
                    cx.listener(move |view, _, _, cx| close_select_rc(view, &reach, cx))
                })
                .when_some(heading, |menu, heading| {
                    menu.child(popover::menu_heading(&theme, heading))
                })
                .child(dropdown_rows(
                    format!("{id}-list"),
                    rows,
                    SELECT_HEIGHT,
                    if heading.is_some() { 32.0 } else { 8.0 },
                ));
            trigger = trigger.child(dropdown(
                format!("{id}-menu"),
                menu,
                state.menu.closing_since(),
                SELECT_HEIGHT,
            ));
        }
        trigger
    }
}

/// Settings actions share one size, corner radius, and hover language. Use
/// outlined for selectors, quiet for secondary actions, filled for row
/// actions on a block (the dropdown trigger's glass wash), and solid for
/// commits.
#[derive(Clone, Copy)]
pub enum ActionTone {
    Quiet,
    Outlined,
    Filled,
    Solid,
}

pub fn action_button(theme: &Theme, tone: ActionTone) -> gpui::Div {
    let button = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .rounded(px(8.0))
        .min_h(px(32.0))
        .px(px(10.0))
        .py(px(5.0))
        .text_size(crate::typography::ui_rems(12.5))
        .cursor_pointer();
    match tone {
        ActionTone::Quiet => button
            .text_color(theme.text_muted)
            .hover(|s| s.bg(theme.glass_hover()).text_color(theme.text)),
        ActionTone::Outlined => button
            .bg(theme.input_glass_bg())
            .border_1()
            .border_color(theme.border)
            .text_color(theme.text)
            .hover(|s| s.bg(theme.glass_hover()).border_color(theme.border_strong)),
        ActionTone::Filled => {
            let lifted = select_fill(theme, true);
            button
                .bg(select_fill(theme, false))
                .text_color(theme.text)
                .hover(move |s| s.bg(lifted))
        }
        ActionTone::Solid => button
            .bg(theme.solid)
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(theme.on_solid)
            .hover(|s| s.opacity(0.9)),
    }
}

pub fn text_action(theme: &Theme, tone: ActionTone, label: impl Into<SharedString>) -> gpui::Div {
    action_button(theme, tone).child(label.into())
}

pub fn ghost_action(theme: &Theme) -> gpui::Div {
    action_button(theme, ActionTone::Quiet)
}

/// The dismissible red error strip (`flex items-start gap-2 rounded-xl border
/// border-red-400/20 bg-red-400/[0.06] text-red-300/90` with a leading
/// `DangerTriangle mt-0.5 size-4`).
pub fn error_strip(theme: &Theme, message: impl Into<SharedString>) -> gpui::Div {
    let red = theme.danger; // red-400
    let red_text = theme.danger_muted; // red-300
    div()
        .mt(px(16.0))
        .px(px(16.0))
        .py(px(12.0))
        .rounded(px(12.0))
        .border_1()
        .border_color(red.opacity(0.2))
        .bg(red.opacity(0.06))
        .text_size(crate::typography::ui_rems(12.5))
        .text_color(red_text.opacity(0.9))
        .flex()
        .flex_row()
        .items_start()
        .gap(px(8.0))
        .child(
            div().flex_none().mt(px(2.0)).child(
                crate::icons::icon(crate::icons::DANGER_TRIANGLE)
                    .size(px(16.0))
                    .text_color(red_text.opacity(0.9)),
            ),
        )
        .child(div().min_w_0().child(message.into()))
}

/// The amber warning strip (`flex items-start gap-2 border-amber-400/20
/// bg-amber-400/[0.06] text-amber-200/90` with a leading `DangerTriangle
/// mt-0.5 size-3.5`).
pub fn warning_strip(theme: &Theme, message: impl Into<SharedString>) -> gpui::Div {
    let amber = theme.warning; // amber-400
    let amber_text = theme.warning_muted; // amber-200
    div()
        .mt(px(8.0))
        .px(px(16.0))
        .py(px(10.0))
        .rounded(px(12.0))
        .border_1()
        .border_color(amber.opacity(0.2))
        .bg(amber.opacity(0.06))
        .text_size(crate::typography::ui_rems(12.0))
        .text_color(amber_text.opacity(0.9))
        .flex()
        .flex_row()
        .items_start()
        .gap(px(8.0))
        .child(
            div().flex_none().mt(px(2.0)).child(
                crate::icons::icon(crate::icons::DANGER_TRIANGLE)
                    .size(px(14.0))
                    .text_color(amber_text.opacity(0.9)),
            ),
        )
        .child(div().min_w_0().child(message.into()))
}

/// The sidebar's paint-time overflow fade, with a persistent scroll handle per
/// settings surface. No fade is painted when the content fits or at a reached edge.
pub fn scroll_faded(
    key: impl Into<SharedString>,
    area: gpui::Stateful<gpui::Div>,
) -> impl IntoElement {
    SettingsScroll {
        key: key.into(),
        area,
    }
}

#[derive(IntoElement)]
struct SettingsScroll {
    key: SharedString,
    area: gpui::Stateful<gpui::Div>,
}

impl RenderOnce for SettingsScroll {
    fn render(self, window: &mut gpui::Window, _: &mut gpui::App) -> impl IntoElement {
        let scroll = window.with_global_id(self.key.into(), |id, window| {
            window.with_element_state(id, |previous: Option<gpui::ScrollHandle>, _| {
                let scroll = previous.unwrap_or_default();
                (scroll.clone(), scroll)
            })
        });
        crate::edge_fade::edge_faded(16.0, true, true, self.area.track_scroll(&scroll))
            .fade_overflow_y(&scroll)
    }
}

/// A one-line hover note for settings controls (reset times, icon-only
/// actions), in the same frosted chip as the rest of the app's tooltips.
pub struct TextTooltip(pub SharedString);

impl Render for TextTooltip {
    fn render(&mut self, _window: &mut gpui::Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        let card = div()
            .max_w(px(320.0))
            .px(px(9.0))
            .py(px(6.0))
            .rounded(px(6.0))
            .border_1()
            .border_color(theme.border)
            .bg(crate::popover::surface_bg(theme))
            .text_size(px(11.0))
            .text_color(theme.text_muted)
            .child(self.0.clone());
        crate::frost::frosted(6.0, crate::frost::MENU_BLUR, card)
    }
}

/// `.tooltip(...)` builder for a [`TextTooltip`].
pub fn text_tooltip(
    text: impl Into<SharedString>,
) -> impl Fn(&mut gpui::Window, &mut gpui::App) -> gpui::AnyView + 'static {
    let text: SharedString = text.into();
    move |_, cx| cx.new(|_| TextTooltip(text.clone())).into()
}
