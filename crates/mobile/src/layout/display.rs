//! Display lists: everything a platform painter needs to draw one row, with
//! every position already decided. Painters never measure text.
//!
//! Text is shipped once per row (`RowDisplay::text`); runs index it in UTF-16
//! code units so a painter can slice an `NSString`/`java.lang.String` without
//! re-encoding. Colors are roles, not RGB — theming is paint-only.

/// Paint roles. The painter maps these to its palette (light/dark).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum ColorRole {
    Text,
    TextSecondary,
    TextTertiary,
    Link,
    Accent,
    Danger,
    Success,
    Warning,
    InlineCodeText,
    InlineCodeBackground,
    CodeText,
    CodeBackground,
    CodeBorder,
    QuoteBar,
    Rule,
    TableBorder,
    TableHeaderBackground,
    UserBubble,
    ChipBackground,
    SyntaxKeyword,
    SyntaxString,
    SyntaxComment,
    SyntaxNumber,
    SyntaxFunction,
    SyntaxType,
    SyntaxConstant,
    SyntaxVariable,
    SyntaxProperty,
    SyntaxOperator,
    SyntaxPunctuation,
    SyntaxTag,
    SyntaxAttribute,
    SyntaxEscape,
}

/// Text decorations, painted relative to the run's baseline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, uniffi::Enum)]
pub enum Decoration {
    #[default]
    None,
    Underline,
    Strikethrough,
}

/// One positioned piece of text: `text[start..start+len]` (UTF-16) drawn with
/// `style` so its origin sits at (`x`, `baseline`).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct TextRun {
    pub start: u32,
    pub len: u32,
    pub x: f32,
    pub baseline: f32,
    /// Rust-measured advance — painters may use it for selection/hit rects.
    pub width: f32,
    pub style: u16,
    pub color: ColorRole,
    pub decoration: Decoration,
    /// Index into [`RowDisplay::scrollers`] when the run scrolls horizontally.
    pub scroller: Option<u32>,
    /// Fade-in start in ms since the row's layout epoch (streaming veil);
    /// `None` = fully visible.
    pub reveal_ms: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum BoxStyle {
    Fill,
    /// A hairline (1 device pixel) stroke on the rect's inside edge.
    Hairline,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct BoxPrim {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub radius: f32,
    pub style: BoxStyle,
    pub color: ColorRole,
    pub scroller: Option<u32>,
}

/// A tappable link region (one per fragment; multi-line links repeat).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct LinkHit {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub url: String,
    pub scroller: Option<u32>,
}

/// A horizontally scrolling viewport (code blocks, wide tables). Primitives
/// that reference it are positioned in its content coordinates, whose origin
/// is the viewport's top-left.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Scroller {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub content_width: f32,
}

/// Native affordances the painter renders itself (icons, images, spinners).
#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum WidgetKind {
    /// Copy button for a code block; `payload` holds the code.
    CopyCode,
    /// Tool group / user message disclosure; toggles via `TranscriptView::toggle`.
    Disclosure { expanded: bool },
    /// Status glyph for a tool (running spinner / done check / failed cross).
    ToolStatus { running: bool, failed: bool },
    /// A remote image to load into the rect (generated images, attachments).
    Image { reference: String },
    /// Working indicator at the tail of a live turn. The painter ticks the
    /// elapsed label itself so time never forces a relayout.
    Working { since_ms: Option<i64>, streaming: bool },
    /// A small activity spinner (running tools).
    Spinner,
    /// Tap target revealing the full text in `payload` (truncated tool lines).
    Detail { title: String },
    /// A small SF-symbol-like icon by name.
    Icon { name: String, color: ColorRole },
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Widget {
    pub id: u32,
    pub kind: WidgetKind,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub payload: Option<String>,
    pub scroller: Option<u32>,
}

/// Everything needed to paint a row at its current width.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct RowDisplay {
    pub key: u64,
    pub version: u64,
    pub width: f32,
    pub height: f32,
    pub text: String,
    pub runs: Vec<TextRun>,
    pub boxes: Vec<BoxPrim>,
    pub links: Vec<LinkHit>,
    pub scrollers: Vec<Scroller>,
    pub widgets: Vec<Widget>,
    /// Plain text for copy / accessibility.
    pub copy_text: String,
}

/// Accumulates one row's primitives. `origin` offsets let nested layout code
/// place children in local coordinates.
#[derive(Default)]
pub(crate) struct DisplayBuilder {
    pub text: String,
    utf16_len: u32,
    pub runs: Vec<TextRun>,
    pub boxes: Vec<BoxPrim>,
    pub links: Vec<LinkHit>,
    pub scrollers: Vec<Scroller>,
    pub widgets: Vec<Widget>,
    /// Scroller that newly pushed primitives belong to.
    pub scroller: Option<u32>,
    next_widget: u32,
}

impl DisplayBuilder {
    /// Append `s` to the row text; returns its UTF-16 start offset.
    pub fn push_text(&mut self, s: &str) -> u32 {
        let start = self.utf16_len;
        self.text.push_str(s);
        self.utf16_len += s.encode_utf16().count() as u32;
        start
    }

    pub fn fill(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32, color: ColorRole) {
        self.boxes.push(BoxPrim {
            x,
            y,
            w,
            h,
            radius,
            style: BoxStyle::Fill,
            color,
            scroller: self.scroller,
        });
    }

    pub fn hairline(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32, color: ColorRole) {
        self.boxes.push(BoxPrim {
            x,
            y,
            w,
            h,
            radius,
            style: BoxStyle::Hairline,
            color,
            scroller: self.scroller,
        });
    }

    pub fn widget(
        &mut self,
        kind: WidgetKind,
        rect: (f32, f32, f32, f32),
        payload: Option<String>,
    ) -> u32 {
        let id = self.next_widget;
        self.next_widget += 1;
        self.widgets.push(Widget {
            id,
            kind,
            x: rect.0,
            y: rect.1,
            w: rect.2,
            h: rect.3,
            payload,
            scroller: self.scroller,
        });
        id
    }

    pub fn begin_scroller(&mut self, x: f32, y: f32, w: f32, h: f32, content_width: f32) {
        self.scrollers.push(Scroller {
            x,
            y,
            w,
            h,
            content_width,
        });
        self.scroller = Some(self.scrollers.len() as u32 - 1);
    }

    pub fn end_scroller(&mut self) {
        self.scroller = None;
    }
}
