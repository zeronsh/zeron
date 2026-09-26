//! Text analysis: CSS `white-space` normalization (with span remapping) and UAX #14 break
//! opportunities, cut into segments with hanging trailing whitespace split off.

use unicode_linebreak::{BreakOpportunity, linebreaks};
use unicode_segmentation::GraphemeCursor;

use crate::chars::{OBJECT_REPLACEMENT, SOFT_HYPHEN, is_hard_break};
use crate::{Span, WhiteSpace};

/// How a line may end after a segment.
pub(crate) const BRK_MANDATORY: u8 = 0;
pub(crate) const BRK_ALLOWED: u8 = 1;
pub(crate) const BRK_SOFT_HYPHEN: u8 = 2;

/// A run of text between two break opportunities:
/// `[start, content_end)` visible content, `[content_end, ws_end)` trailing collapsible
/// whitespace that hangs at a line end, `[ws_end, end)` the hard-break character (if any).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RawSeg {
    pub start: u32,
    pub content_end: u32,
    pub ws_end: u32,
    pub end: u32,
    pub brk: u8,
}

/// Makes `spans` a contiguous in-order cover of `text` on char boundaries. Returns `None` when
/// the input is already valid.
pub(crate) fn repair_spans(text: &str, spans: &[Span]) -> Option<Vec<Span>> {
    let len = text.len();
    let valid = !spans.is_empty()
        && spans[0].range.start == 0
        && spans.last().is_some_and(|s| s.range.end == len)
        && spans.windows(2).all(|w| w[0].range.end == w[1].range.start)
        && spans.iter().all(|s| {
            s.range.start <= s.range.end
                && text.is_char_boundary(s.range.start)
                && text.is_char_boundary(s.range.end)
        });
    if valid {
        return None;
    }
    let mut out = Vec::with_capacity(spans.len());
    let mut pos = 0usize;
    for s in spans {
        let mut end = s.range.end.clamp(pos, len);
        while !text.is_char_boundary(end) {
            end += 1;
        }
        let mut s = s.clone();
        s.range = pos..end;
        pos = end;
        out.push(s);
    }
    if let Some(last) = out.last_mut() {
        last.range.end = len;
    }
    Some(out)
}

fn needs_normalization(text: &str, mode: WhiteSpace) -> bool {
    let b = text.as_bytes();
    match mode {
        WhiteSpace::PreWrap | WhiteSpace::Pre => b.iter().any(|&c| c == b'\r' || c == 0x0C),
        WhiteSpace::Normal | WhiteSpace::PreLine => {
            if b.first() == Some(&b' ') || b.last() == Some(&b' ') {
                return true;
            }
            let pre_line = mode == WhiteSpace::PreLine;
            let mut prev = 0u8;
            for &c in b {
                match c {
                    b'\t' | b'\r' | 0x0C => return true,
                    b'\n' if !pre_line => return true,
                    b'\n' if prev == b' ' => return true,
                    b' ' if prev == b' ' || prev == b'\n' => return true,
                    _ => {}
                }
                prev = c;
            }
            false
        }
    }
}

struct Normalizer<'a> {
    out: String,
    spans: &'a mut [Span],
    /// Index of the span currently receiving output.
    cur: usize,
}

impl Normalizer<'_> {
    /// Removes the last byte of output (always an ASCII space) and pulls span ranges back.
    fn pop_space(&mut self) {
        debug_assert!(self.out.ends_with(' '));
        self.out.pop();
        let len = self.out.len();
        for s in self.spans[..=self.cur].iter_mut().rev() {
            if s.range.start <= len && s.range.end <= len {
                break;
            }
            s.range.start = s.range.start.min(len);
            s.range.end = s.range.end.min(len);
        }
    }
}

/// Applies CSS white-space processing, remapping span ranges onto the output.
/// Spans must already be a valid cover (see [`repair_spans`]).
pub(crate) fn normalize(text: &str, spans: &[Span], mode: WhiteSpace) -> (String, Vec<Span>) {
    if !needs_normalization(text, mode) {
        return (text.to_owned(), spans.to_vec());
    }
    let mut new_spans = spans.to_vec();
    for s in &mut new_spans {
        s.range = usize::MAX..usize::MAX;
    }
    let mut n = Normalizer {
        out: String::with_capacity(text.len()),
        spans: &mut new_spans,
        cur: 0,
    };
    n.spans[0].range.start = 0;
    let mut in_ws = false;
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        // Close every span ending at or before `i`, opening the next at the current output end.
        while n.cur + 1 < spans.len() && spans[n.cur].range.end <= i {
            let len = n.out.len();
            n.spans[n.cur].range.end = len;
            n.cur += 1;
            n.spans[n.cur].range.start = len;
        }
        match mode {
            WhiteSpace::Normal => match c {
                ' ' | '\t' | '\n' | '\r' | '\u{0C}' => {
                    if !in_ws && !n.out.is_empty() {
                        n.out.push(' ');
                    }
                    in_ws = true;
                }
                _ => {
                    n.out.push(c);
                    in_ws = false;
                }
            },
            WhiteSpace::PreLine => match c {
                ' ' | '\t' => {
                    if !in_ws && !n.out.is_empty() && !n.out.ends_with('\n') {
                        n.out.push(' ');
                    }
                    in_ws = true;
                }
                '\r' if chars.peek().is_some_and(|&(_, d)| d == '\n') => {}
                '\n' | '\r' | '\u{0C}' => {
                    if n.out.ends_with(' ') && in_ws {
                        n.pop_space();
                    }
                    n.out.push('\n');
                    in_ws = false;
                }
                _ => {
                    n.out.push(c);
                    in_ws = false;
                }
            },
            WhiteSpace::PreWrap | WhiteSpace::Pre => match c {
                '\r' if chars.peek().is_some_and(|&(_, d)| d == '\n') => {}
                '\r' | '\u{0C}' => n.out.push('\n'),
                _ => n.out.push(c),
            },
        }
    }
    if in_ws && n.out.ends_with(' ') && matches!(mode, WhiteSpace::Normal | WhiteSpace::PreLine) {
        n.pop_space();
    }
    let len = n.out.len();
    for s in n.spans[n.cur..].iter_mut() {
        if s.range.start == usize::MAX {
            s.range.start = len;
        }
        s.range.end = len;
    }
    (n.out, new_spans)
}

/// Break opportunities `(position, mandatory)` in increasing order, ending with `text.len()`.
pub(crate) fn break_opportunities(
    text: &str,
    spans: &[Span],
    mode: WhiteSpace,
    out: &mut Vec<(u32, bool)>,
) {
    out.clear();
    let len = text.len();
    if mode == WhiteSpace::Pre {
        for (i, c) in text.char_indices() {
            if is_hard_break(c) {
                out.push(((i + c.len_utf8()) as u32, true));
            }
        }
        if out.last().is_none_or(|&(p, _)| p as usize != len) {
            out.push((len as u32, true));
        }
        return;
    }

    // Atomic spans behave like CSS atomic inlines: line breaking sees them as one U+FFFC.
    let has_atomic = spans.iter().any(|s| s.atomic && !s.range.is_empty());
    if has_atomic {
        let mut sub = String::with_capacity(text.len());
        // (sub offset just past the U+FFFC, original end of the atomic span)
        let mut repl: Vec<(usize, usize)> = Vec::new();
        for s in spans {
            if s.atomic && !s.range.is_empty() {
                sub.push(OBJECT_REPLACEMENT);
                repl.push((sub.len(), s.range.end));
            } else {
                sub.push_str(&text[s.range.clone()]);
            }
        }
        let (mut sub_base, mut orig_base) = (0usize, 0usize);
        let mut r = 0;
        for (p, kind) in linebreaks(&sub) {
            while r < repl.len() && repl[r].0 <= p {
                sub_base = repl[r].0;
                orig_base = repl[r].1;
                r += 1;
            }
            let orig = p - sub_base + orig_base;
            out.push((orig as u32, kind == BreakOpportunity::Mandatory));
        }
    } else {
        out.extend(
            linebreaks(text).map(|(p, kind)| (p as u32, kind == BreakOpportunity::Mandatory)),
        );
    }

    // Never break inside an extended grapheme cluster (emoji ZWJ sequences, flags, keycaps,
    // skin tones, combining sequences).
    // Between two ASCII bytes every position is a boundary (CR LF never survives
    // normalization, and UAX #14 doesn't break it anyway), so only non-ASCII neighborhoods pay
    // for a cursor query.
    if !text.is_ascii() {
        let bytes = text.as_bytes();
        out.retain(|&(p, mandatory)| {
            let p = p as usize;
            if p == len || p == 0 || mandatory || (bytes[p - 1] < 0x80 && bytes[p] < 0x80) {
                return true;
            }
            GraphemeCursor::new(p, len, true)
                .is_boundary(text, 0)
                .unwrap_or(true)
        });
    }
    if out.last().is_none_or(|&(p, _)| p as usize != len) {
        out.push((len as u32, true));
    }
}

/// Cuts the text at `breaks` into segments, splitting off hanging whitespace and folding
/// whitespace-only segments into the previous segment's hang.
pub(crate) fn segments(text: &str, breaks: &[(u32, bool)], mode: WhiteSpace, out: &mut Vec<RawSeg>) {
    out.clear();
    let bytes = text.as_bytes();
    let hang_tabs = mode == WhiteSpace::PreWrap;
    let hangs = mode != WhiteSpace::Pre;
    let mut prev = 0u32;
    for &(end, mandatory) in breaks {
        if end <= prev {
            continue;
        }
        let mut ws_end = end;
        if mandatory
            && let Some(c) = text[prev as usize..end as usize].chars().next_back()
            && is_hard_break(c)
        {
            ws_end = end - c.len_utf8() as u32;
        }
        let mut content_end = ws_end;
        if hangs {
            while content_end > prev {
                let b = bytes[content_end as usize - 1];
                if b == b' ' || (hang_tabs && b == b'\t') {
                    content_end -= 1;
                } else {
                    break;
                }
            }
        }
        let brk = if mandatory {
            BRK_MANDATORY
        } else if content_end == end
            && content_end > prev
            && text[..content_end as usize].ends_with(SOFT_HYPHEN)
        {
            BRK_SOFT_HYPHEN
        } else {
            BRK_ALLOWED
        };
        if content_end == prev
            && let Some(last) = out.last_mut()
            && last.brk != BRK_MANDATORY
        {
            // Whitespace-only run after a soft break: it can only ever hang with the previous
            // segment, so merge it into that segment's hang.
            last.ws_end = ws_end;
            last.end = end;
            last.brk = brk;
        } else {
            out.push(RawSeg {
                start: prev,
                content_end,
                ws_end,
                end,
                brk,
            });
        }
        prev = end;
    }
}
