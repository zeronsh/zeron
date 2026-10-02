//! gpui pieces shared by the saved-workflow surfaces (the Settings detail
//! view, the launcher). The decisions are in `saved.rs`.

use gpui::{
    AnyElement, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use zeron_syntax::HighlightedDocument;

use crate::markdown::parser::Block;
use crate::markdown::render::{self, RenderOptions};
use crate::theme::Theme;

/// Script height before it scrolls.
pub const SCRIPT_MAX_HEIGHT: f32 = 260.0;

/// A script as a highlighted (Python) code block, scrollable past
/// [`SCRIPT_MAX_HEIGHT`].
pub fn script_block(
    code: &str,
    row_key: &str,
    highlight: Option<&HighlightedDocument>,
    theme: &Theme,
    window: &Window,
) -> AnyElement {
    let opts = RenderOptions {
        tasks: None,
        media: None,
        row_key: row_key.to_owned().into(),
        veil: None,
        cache: None,
        now: std::time::Instant::now(),
        copy: None,
        link: None,
        workspace_root: None,
        code: None,
    };
    let block = Block::CodeBlock {
        language: Some("python".into()),
        code: code.trim_end().to_owned(),
    };
    div()
        .id(gpui::SharedString::from(format!("{row_key}-scroll")))
        .max_h(px(SCRIPT_MAX_HEIGHT))
        .overflow_y_scroll()
        .rounded(px(10.0))
        .child(render::render_block(
            &block,
            0,
            0,
            &opts,
            theme,
            window,
            highlight.map(|h| h.lines.as_slice()),
        ))
        .into_any_element()
}
