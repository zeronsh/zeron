//! `@chat:<uuid>` pills inside transcript text — resolution, substitution,
//! and the icon/status/hitbox overlay painted over the shaped pill text.
//!
//! The renderer never touches `AppState` directly: the transcript resolves a
//! [`ChatRef`] snapshot per frame (its chat-ref fingerprint observer clears
//! the render cache and remeasures when a referenced chat's identity or
//! status flips) and wires [`ChatUi`] into [`super::render::RenderOptions`].
//! Surfaces without `chats` (file previews) keep the raw mention text.
use std::ops::Range;
use std::rc::Rc;

use gpui::{
    AnyElement, App, AvailableSpace, Bounds, Element, ElementId, GlobalElementId,
    InspectorElementId, LayoutId, Pixels, Role, SharedString, TextLayout, Window, div, prelude::*,
    px,
};

use super::link_presentation::OffsetMap;
use super::render::{INLINE_CODE_INSET_Y, INLINE_CODE_PAD_X, range_rects};
use crate::chat_pill::ChatRef;
use crate::theme::Theme;

/// NBSP pads the pill the way the composer's mention chips do; the leading
/// pair of slots hosts the harness mark, the trailing pair the status glyph —
/// so the substituted text alone fixes the geometry (no layout-time metrics).
const NBSP: &str = "\u{00A0}";
const SLOT: &str = "\u{00A0}\u{00A0}";

/// A chat id resolved to chip data.
pub type ResolveChat = Rc<dyn Fn(&str) -> ChatRef>;
/// Click activation for a resolved pill.
pub type OpenChat = Rc<dyn Fn(&str, &mut Window, &mut App)>;
/// A whole-paragraph-mention row body, built at layout time.
pub type BuildChatCard = Rc<dyn Fn(&str, &mut Window, &mut App) -> AnyElement>;

/// Transcript wiring for `@chat:` pills. `resolve` is a per-render snapshot —
/// cheap `Rc` clones — and `open` activates the target (right pane).
#[derive(Clone)]
pub struct ChatUi {
    /// Full chat id → resolved chip data. `known == false` renders the muted
    /// "Unavailable chat" pill: no icon, no status, not clickable.
    pub resolve: ResolveChat,
    /// Click activation — opens the referenced chat in the right pane.
    pub open: OpenChat,
    /// Whole-paragraph-mention row body. The renderer cannot build it
    /// itself: the badge's status spinner needs `App`, which only exists at
    /// element request_layout — see [`DeferredElement`].
    pub card: BuildChatCard,
    /// View entity leasing the status glyph's loader.
    pub owner: gpui::EntityId,
}

/// One resolved pill, in the element's displayed-text coordinates.
#[derive(Clone, Debug)]
pub struct PillSpan {
    /// Byte range covering the whole pill (icon slot + title + status slot).
    pub range: Range<usize>,
    /// The reserved leading NBSP band the harness mark overlays.
    pub icon_slot: Range<usize>,
    /// The pill's title text — the range [`pill_runs`] gives the sans label
    /// run, and the range the harness-name strip applies to.
    pub label: Range<usize>,
    /// The reserved trailing NBSP band the status glyph overlays.
    pub status_slot: Range<usize>,
    /// Render-time resolution; `chat.title` stays the full stored title.
    pub chat: ChatRef,
}

/// Substitute `@chat:` token ranges in `text` with pill text and record the
/// mapping shown→source so copy/selection still yield the raw mention.
/// `mentions` must be sorted and non-overlapping (the parser guarantees it).
pub(crate) fn substitute(
    text: &str,
    mentions: &[(Range<usize>, String)],
    resolve: &dyn Fn(&str) -> ChatRef,
) -> (String, Vec<PillSpan>, OffsetMap) {
    let mut out = String::with_capacity(text.len());
    let mut pills = Vec::with_capacity(mentions.len());
    let mut offsets = OffsetMap::default();
    let mut at = 0;
    for (range, chat_id) in mentions {
        let chat = resolve(chat_id);
        out.push_str(&text[at..range.start]);
        let start = out.len();
        let title = crate::chat_pill::display_title(&chat.title, chat.harness);
        let (mut icon_slot, mut status_slot) = (start..start, start..start);
        if chat.known {
            out.push_str(SLOT);
            icon_slot = start..out.len();
            let label = out.len()..out.len() + title.len();
            out.push_str(title);
            out.push_str(NBSP);
            status_slot = out.len()..out.len() + SLOT.len();
            out.push_str(SLOT);
            pills.push(PillSpan {
                range: start..out.len(),
                icon_slot,
                label,
                status_slot,
                chat,
            });
        } else {
            out.push_str(NBSP);
            let label = out.len()..out.len() + title.len();
            out.push_str(title);
            out.push_str(NBSP);
            pills.push(PillSpan {
                range: start..out.len(),
                icon_slot,
                label,
                status_slot,
                chat,
            });
        }
        offsets.omissions.push((range.clone(), start..out.len()));
        at = range.end;
    }
    out.push_str(&text[at..]);
    (out, pills, offsets)
}

/// The pill's wash radius, shared by the inline underlay and the row pill.
pub(crate) const PILL_RADIUS: f32 = 5.0;

/// The pill's shaped runs. The NBSP bands keep their mono advance — the
/// reserved slots must stay wide enough for the 12px mark regardless of the
/// surrounding font — while the title follows the UI sans font the text
/// around it uses. `theme.text.opacity(0.85)` for a known chat, muted for the
/// placeholder.
pub(crate) fn pill_runs(pill: &PillSpan, theme: &Theme) -> Vec<gpui::TextRun> {
    let color = if pill.chat.known {
        theme.text.opacity(0.85)
    } else {
        theme.text_muted
    };
    let run = |len: usize, font: gpui::Font| gpui::TextRun {
        len,
        font,
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let mut runs = Vec::with_capacity(3);
    if pill.label.start > pill.range.start {
        runs.push(run(
            pill.label.start - pill.range.start,
            gpui::font(theme.font_mono.clone()),
        ));
    }
    runs.push(run(pill.label.len(), gpui::font(theme.font_sans.clone())));
    if pill.range.end > pill.label.end {
        runs.push(run(
            pill.range.end - pill.label.end,
            gpui::font(theme.font_mono.clone()),
        ));
    }
    runs
}

/// A file mention inside the composer or a user bubble keeps the inline-code
/// treatment: mono on the code wash. Chat pills no longer share it.
pub(crate) fn mention_run(len: usize, theme: &Theme) -> gpui::TextRun {
    gpui::TextRun {
        len,
        font: gpui::font(theme.font_mono.clone()),
        color: theme.code_text,
        background_color: None,
        underline: None,
        strikethrough: None,
    }
}

/// Every chat id mentioned in a block's inline runs — what
/// `Transcript::refresh_chat_refs` folds into `chat_refs` so a referenced
/// chat's title/status flips repaint the pill.
pub(crate) fn block_chat_ids(
    block: &super::parser::Block,
    out: &mut std::collections::BTreeSet<String>,
) {
    use super::parser::Block;
    fn runs<'a>(
        runs: impl Iterator<Item = &'a super::parser::InlineRun>,
        out: &mut std::collections::BTreeSet<String>,
    ) {
        out.extend(runs.filter_map(|run| run.style.chat.clone()));
    }
    match block {
        Block::Paragraph { runs: r } | Block::Heading { runs: r, .. } => runs(r.iter(), out),
        Block::BlockQuote { children } => children.iter().for_each(|b| block_chat_ids(b, out)),
        Block::List { items, .. } => items.iter().flatten().for_each(|b| block_chat_ids(b, out)),
        Block::Table { header, rows, .. } => {
            header.iter().for_each(|r| runs(r.iter(), out));
            rows.iter().flatten().for_each(|r| runs(r.iter(), out));
        }
        _ => {}
    }
}

/// `<title> · <Harness> · <model> · <status>` for the pill tooltip.
fn pill_tooltip(chat: &ChatRef) -> SharedString {
    let harness = chat.harness.map(harness_name).unwrap_or("Agent");
    let mut parts = vec![chat.title.to_string(), harness.to_string()];
    if let Some(model) = &chat.model {
        parts.push(model.to_string());
    }
    parts.push(indicator_name(chat.indicator).to_string());
    parts.join(" · ").into()
}

fn harness_name(harness: zeron_proto::HarnessId) -> &'static str {
    crate::chat_pill::harness_display_name(harness)
}

fn indicator_name(indicator: zeron_proto::ChatIndicator) -> &'static str {
    use zeron_proto::ChatIndicator::*;
    match indicator {
        Working => "Working",
        AwaitingInput => "Awaiting input",
        Errored => "Errored",
        Completed => "Completed",
        Idle => "Idle",
    }
}

struct PillTooltip(SharedString);

impl Render for PillTooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        div()
            .px(px(8.0))
            .py(px(6.0))
            .rounded(px(6.0))
            .border_1()
            .border_color(theme.border_strong)
            .bg(theme.surface_raised)
            .shadow_md()
            .text_size(px(11.0))
            .text_color(theme.text)
            .child(self.0.clone())
    }
}

/// Builds its child inside `request_layout`, where `App` exists — the
/// sole-mention card's status spinner needs a loader lease.
pub(crate) struct DeferredElement {
    build: DeferredBuilder,
}

/// Lazy element construction from a layout pass.
type DeferredBuilder = Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>;

impl DeferredElement {
    pub(crate) fn new(build: impl Fn(&mut Window, &mut App) -> AnyElement + 'static) -> Self {
        Self {
            build: Rc::new(build),
        }
    }
}

impl IntoElement for DeferredElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for DeferredElement {
    type RequestLayoutState = AnyElement;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, AnyElement) {
        let mut child = (self.build)(window, cx);
        (child.request_layout(window, cx), child)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        child: &mut AnyElement,
        window: &mut Window,
        cx: &mut App,
    ) {
        child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        child: &mut AnyElement,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        child.paint(window, cx);
    }
}

/// Per-pill hit targets + icon/status overlays over the text's shaped
/// geometry (the [`super::link_interaction::LinkRanges`] pattern): glyph
/// positions come from the laid-out text, so wrapped pills get per-line
/// rects and click anywhere inside the wash.
pub(crate) struct ChatPillRanges {
    pub id: SharedString,
    pub child: AnyElement,
    pub layout: TextLayout,
    pub pills: Vec<PillSpan>,
    pub ui: ChatUi,
}

impl IntoElement for ChatPillRanges {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for ChatPillRanges {
    type RequestLayoutState = ();
    type PrepaintState = Vec<AnyElement>;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone().into())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Vec<AnyElement> {
        self.child.prepaint(window, cx);
        let theme = Theme::of(cx).clone();
        let mut overlays = Vec::new();
        for (index, pill) in self.pills.iter().enumerate() {
            if !pill.chat.known {
                continue;
            }
            // Harness mark, centered in the leading NBSP slot.
            if let Some(rect) = range_rects(&self.layout, &pill.icon_slot, 0.0, INLINE_CODE_INSET_Y)
                .first()
                .copied()
            {
                let (mark, tint) = pill
                    .chat
                    .harness
                    .map(crate::pickers::harness_brand_icon)
                    .unwrap_or((crate::icons::BOT, None));
                let mut icon_el = div()
                    .w(rect.size.width)
                    .h(rect.size.height)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        crate::icons::icon(mark)
                            .size(px(12.0))
                            .text_color(tint.unwrap_or(theme.text_muted)),
                    )
                    .into_any_element();
                icon_el.prepaint_as_root(
                    rect.origin,
                    rect.size.map(AvailableSpace::Definite),
                    window,
                    cx,
                );
                overlays.push(icon_el);
            }
            // Status glyph, centered in the trailing NBSP slot (on the last
            // visual line when the pill wraps).
            if let Some(rect) =
                range_rects(&self.layout, &pill.status_slot, 0.0, INLINE_CODE_INSET_Y)
                    .last()
                    .copied()
            {
                let mut status_el = div()
                    .w(rect.size.width)
                    .h(rect.size.height)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(crate::chat_pill::status_glyph(
                        format!("chat-pill-{}", pill.chat.chat_id),
                        pill.chat.indicator,
                        self.ui.owner,
                        &theme,
                        cx,
                    ))
                    .into_any_element();
                status_el.prepaint_as_root(
                    rect.origin,
                    rect.size.map(AvailableSpace::Definite),
                    window,
                    cx,
                );
                overlays.push(status_el);
            }
            // The hitbox rides LAST so it also covers the overlay slots.
            for (part, rect) in range_rects(
                &self.layout,
                &pill.range,
                INLINE_CODE_PAD_X,
                INLINE_CODE_INSET_Y,
            )
            .into_iter()
            .enumerate()
            {
                let chat_id = pill.chat.chat_id.clone();
                let open = self.ui.open.clone();
                let tip = pill_tooltip(&pill.chat);
                let mut hit = div()
                    .id(format!("chat-pill-{index}-{part}"))
                    .w(rect.size.width)
                    .h(rect.size.height)
                    .cursor_pointer()
                    .role(Role::Button)
                    .aria_label(pill.chat.title.clone())
                    .tooltip(move |_, cx| cx.new(|_| PillTooltip(tip.clone())).into())
                    .on_click(move |event, window, cx| {
                        if super::link_interaction::click_is_activation(event)
                            && super::selection::selected_text().is_none()
                        {
                            open(&chat_id, window, cx);
                        }
                    })
                    .into_any_element();
                hit.prepaint_as_root(
                    rect.origin,
                    rect.size.map(AvailableSpace::Definite),
                    window,
                    cx,
                );
                overlays.push(hit);
            }
        }
        overlays
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        overlays: &mut Vec<AnyElement>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
        for overlay in overlays {
            overlay.paint(window, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_proto::{ChatIndicator, HarnessId};

    fn known(chat_id: &str, title: &str) -> ChatRef {
        ChatRef {
            chat_id: chat_id.into(),
            title: title.into(),
            harness: Some(HarnessId::ClaudeCode),
            model: None,
            indicator: ChatIndicator::Working,
            known: true,
        }
    }

    fn resolve_known(chat_id: &str) -> ChatRef {
        if chat_id == "child-1" {
            known(chat_id, "Scout")
        } else {
            ChatRef::unavailable(chat_id)
        }
    }

    #[test]
    fn substitute_known_chat_reserves_icon_and_status_slots() {
        let text = "asked @chat:child-1 to look";
        let mention = text.find('@').unwrap()..text.find(" to").unwrap();
        let (shown, pills, _) = substitute(text, &[(mention, "child-1".into())], &resolve_known);
        assert_eq!(shown, format!("asked {SLOT}Scout{NBSP}{SLOT} to look"));
        assert_eq!(pills.len(), 1);
        let pill = &pills[0];
        assert!(pill.chat.known);
        assert_eq!(&shown[pill.icon_slot.clone()], SLOT);
        assert_eq!(&shown[pill.status_slot.clone()], SLOT);
        assert_eq!(
            &shown[pill.range.clone()],
            format!("{SLOT}Scout{NBSP}{SLOT}")
        );
    }

    #[test]
    fn substitute_strips_a_repeated_harness_from_the_title() {
        let resolve = |chat_id: &str| ChatRef {
            chat_id: chat_id.into(),
            title: "Audit the sync layer (Codex)".into(),
            harness: Some(HarnessId::Codex),
            model: None,
            indicator: ChatIndicator::Working,
            known: true,
        };
        let text = "see @chat:child-1 there";
        let mention = text.find('@').unwrap()..text.find(" there").unwrap();
        let (shown, pills, _) = substitute(text, &[(mention, "child-1".into())], &resolve);
        assert_eq!(
            &shown[pills[0].label.clone()],
            "Audit the sync layer",
            "the pill drops a trailing harness the mark already names"
        );
        assert_eq!(pills[0].chat.title.as_ref(), "Audit the sync layer (Codex)");
    }

    #[test]
    fn pill_runs_keep_mono_slots_and_a_sans_label() {
        let theme = Theme::dark();
        let text = "see @chat:child-1 there";
        let mention = text.find('@').unwrap()..text.find(" there").unwrap();
        let (_, pills, _) = substitute(text, &[(mention, "child-1".into())], &resolve_known);
        let runs = pill_runs(&pills[0], &theme);
        assert_eq!(
            runs.iter().map(|run| run.len).sum::<usize>(),
            pills[0].range.len()
        );
        assert_eq!(runs.len(), 3, "icon slot, label, status slot");
        assert_eq!(runs[0].font.family.as_ref(), theme.font_mono.as_ref());
        assert_eq!(runs[1].font.family.as_ref(), theme.font_sans.as_ref());
        assert_eq!(runs[2].font.family.as_ref(), theme.font_mono.as_ref());
        assert_eq!(runs[1].color, theme.text.opacity(0.85));
    }

    #[test]
    fn substitute_unknown_chat_is_muted_without_slots() {
        let text = "asked @chat:gone already";
        let mention = text.find('@').unwrap()..text.find(" already").unwrap();
        let (shown, pills, _) = substitute(text, &[(mention, "gone".into())], &resolve_known);
        assert_eq!(shown, format!("asked {NBSP}Unavailable chat{NBSP} already"));
        assert!(!pills[0].chat.known);
        assert!(pills[0].icon_slot.is_empty());
        assert!(pills[0].status_slot.is_empty());
    }

    #[test]
    fn substitute_offsets_round_trip_through_widened_pills() {
        let text = "see @chat:child-1 and @chat:child-1 done";
        let first = text.find('@').unwrap();
        let second = text[first + 1..].find('@').unwrap() + first + 1;
        let end_first = text[first..].find(' ').unwrap() + first;
        let end_second = text.len() - " done".len();
        let mentions = vec![
            (first..end_first, "child-1".to_string()),
            (second..end_second, "child-1".to_string()),
        ];
        let (shown, pills, offsets) = substitute(text, &mentions, &resolve_known);
        assert!(shown.len() > text.len());

        // Every source offset outside mentions round-trips; offsets inside a
        // mention collapse onto the pill's start.
        for (offset, _) in text.char_indices() {
            if mentions.iter().any(|(range, _)| range.contains(&offset)) {
                continue;
            }
            assert_eq!(offsets.original(offsets.displayed(offset)), offset);
        }
        assert_eq!(offsets.displayed(first), pills[0].range.start);
        assert_eq!(offsets.displayed(second), pills[1].range.start);
        // Shown text inside a pill maps back to the mention's start.
        for displayed in pills[0].range.clone() {
            assert_eq!(offsets.original(displayed), first);
        }
    }

    #[test]
    fn block_chat_ids_collects_nested_mentions_once() {
        use super::super::parser::{Block, InlineRun, InlineStyle};
        let run = |text: &str, chat: Option<&str>| InlineRun {
            text: text.into(),
            style: InlineStyle {
                chat: chat.map(str::to_owned),
                ..Default::default()
            },
        };
        let blocks = vec![
            Block::Paragraph {
                runs: vec![run("see ", None), run("@chat:a", Some("a"))],
            },
            Block::BlockQuote {
                children: vec![Block::Paragraph {
                    runs: vec![run("@chat:b", Some("b"))],
                }],
            },
            Block::List {
                ordered_start: None,
                items: vec![vec![Block::Paragraph {
                    runs: vec![run("@chat:a", Some("a")), run("@chat:c", Some("c"))],
                }]],
            },
        ];
        let mut ids = std::collections::BTreeSet::new();
        for block in &blocks {
            block_chat_ids(block, &mut ids);
        }
        let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
        assert_eq!(ids, ["a", "b", "c"]);
    }
}
