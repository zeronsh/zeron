//! Browser text input for a PTY: commit text, never edit the rendered grid.
//!
//! A software keyboard may emit only beforeinput/composition events, not useful
//! keydowns. The shell owns its input buffer; terminal output/selection is not an
//! editable document. Preedit stays in the browser and only its final commit is
//! sent to the PTY. Native desktop input remains on the existing keydown path.
use super::*;
use gpui::{Bounds, EntityInputHandler, Point, UTF16Selection};
use std::ops::Range;

impl EntityInputHandler for TerminalPanel {
    fn text_for_range(
        &mut self,
        _: Range<usize>,
        _: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        // No locally editable text range. Browser deletion must use the
        // terminal's Backspace/Delete key handler instead of deleting output.
        None
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        None
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {}

    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // beforeinput's Enter is a newline; the PTY Enter key is CR.
        let text = if text == "\n" { "\r" } else { text };
        self.queue_input(text.as_bytes(), cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        _: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
        // Sending intermediate composition updates would type duplicate text
        // into the remote shell. compositionend supplies the single commit.
    }

    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let geometry = self.geometry?;
        let cursor = self.active_tab(cx)?.emulator.cursor()?;
        Some(Bounds::new(
            geometry.origin
                + gpui::point(
                    px(geometry.cell_w * cursor.col as f32),
                    px(geometry.line_h * cursor.row as f32),
                ),
            gpui::size(px(geometry.cell_w), px(geometry.line_h)),
        ))
    }

    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}
