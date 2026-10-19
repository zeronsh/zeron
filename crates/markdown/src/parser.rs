//! Block-level markdown parsing over pulldown-cmark.
//!
//! Full parses build a [`BlockTree`] — a list of top-level blocks with their
//! byte ranges in the source. The streaming path ([`IncrementalParser`]) reparses
//! only from the last stable top-level block boundary: text before the start of
//! the last top-level block cannot be affected by an append, so each streamed
//! delta costs roughly O(delta + last block) instead of O(document).
//!
//! Soundness guard: link-reference definitions (`[label]: url`) have non-local
//! effects (a definition anywhere resolves references anywhere), so a source
//! containing one drops to full reparses. The parity unit tests stream corpora
//! through both paths and assert equality.

use std::borrow::Cow;
use std::ops::Range;
use std::sync::Arc;

use pulldown_cmark::{
    Alignment, CodeBlockKind, Event, HeadingLevel, LinkType, Options, Parser, Tag, TagEnd,
};

// ---------------------------------------------------------------------------
// Tree model
// ---------------------------------------------------------------------------

/// Inline styling flags threaded through nested emphasis/links.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InlineStyle {
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub strikethrough: bool,
    /// Destination URL when inside a link.
    pub link: Option<String>,
    /// The label to show instead of `text` for a file link whose text is the
    /// path itself (an inline-code span) rather than a label the author
    /// wrote. Never set by the parser; `None` shows `text`. When set, copy
    /// and selection still map back to `text`.
    pub file_label: Option<String>,
    pub image: Option<InlineImage>,
    pub task: Option<TaskMarker>,
    /// Inline TeX math (`$…$`, `\(…\)`): `text` is the TeX source without delimiters.
    pub math: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskMarker {
    pub checked: bool,
    /// Byte range of `[ ]`, `[x]` or `[X]` in the original document.
    pub range: Range<usize>,
}

/// An image remains inline in the AST; hosts can opt in to visual media.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineImage {
    pub source: String,
    pub alt: String,
    pub title: String,
    pub link: Option<String>,
}

/// One run of identically-styled inline text.
#[derive(Debug, Clone, PartialEq)]
pub struct InlineRun {
    pub text: String,
    pub style: InlineStyle,
}

/// A markdown block. Containers nest.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Paragraph {
        runs: Vec<InlineRun>,
    },
    Heading {
        level: u8,
        runs: Vec<InlineRun>,
    },
    CodeBlock {
        language: Option<String>,
        code: String,
    },
    BlockQuote {
        children: Vec<Block>,
    },
    List {
        ordered_start: Option<u64>,
        items: Vec<Vec<Block>>,
    },
    Table {
        header: Vec<Vec<InlineRun>>,
        rows: Vec<Vec<Vec<InlineRun>>>,
        /// Per-column GFM alignment (`:--`/`:-:`/`--:`); unspecified is Left.
        align: Vec<TableAlign>,
    },
    /// Display math (`$$…$$`, `\[…\]`) on its own: the TeX source without delimiters.
    Math {
        tex: String,
    },
    Rule,
}

/// GFM column alignment for a table (mdast `align`; `None` renders as Left).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TableAlign {
    #[default]
    Left,
    Center,
    Right,
}

/// A top-level block plus its byte range in the source. The range start is the
/// stable-boundary anchor for incremental reparses.
#[derive(Debug, Clone, PartialEq)]
pub struct TopBlock {
    pub range: Range<usize>,
    pub block: Block,
}

/// The parse result: top-level blocks in document order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BlockTree {
    // Completed blocks are immutable. Canonical/display trees and old/new
    // transcript rows share their contents across streamed tail updates.
    pub blocks: Vec<Arc<TopBlock>>,
}

impl BlockTree {
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    pub fn len(&self) -> usize {
        self.blocks.len()
    }
}

// ---------------------------------------------------------------------------
// Full parse
// ---------------------------------------------------------------------------

fn options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_MATH
}

/// Parse a whole source into a [`BlockTree`].
pub fn parse_full(source: &str) -> BlockTree {
    parse_at(source, 0)
}

fn parse_at(source: &str, offset: usize) -> BlockTree {
    parse_with(source, offset, options())
}

fn parse_with(source: &str, offset: usize, options: Options) -> BlockTree {
    // pulldown-cmark 0.12 can panic on some malformed input (a slice index
    // in its first pass); a message must never take the app down with it.
    // Such a source renders as its plain text.
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        parse_rewritten(source, offset, options)
    }))
    .unwrap_or_else(|_| BlockTree {
        blocks: vec![Arc::new(TopBlock {
            range: offset..offset + source.len(),
            block: Block::Paragraph {
                runs: vec![InlineRun {
                    text: source.to_owned(),
                    style: InlineStyle::default(),
                }],
            },
        })],
    })
}

/// Where rewriting is off: blocks whose rewrites didn't come out as the
/// formula or link they were for, or (`all`) everywhere.
#[derive(Default)]
struct Skip {
    /// Top-level blocks (original ranges, ascending) whose rewrites failed:
    /// every dollar in them stays literal and nothing in them is rewritten.
    literal: Vec<Range<usize>>,
    all: bool,
}

impl Skip {
    /// Whether nothing at original offset `at` is rewritten.
    fn off(&self, at: usize) -> bool {
        let i = self.literal.partition_point(|range| range.end <= at);
        self.all || self.literal.get(i).is_some_and(|range| range.start <= at)
    }
}

/// Deepest block and inline nesting kept: pulldown-cmark itself is
/// iterative, but the tree builder and every renderer recurse per level, so
/// `>` × 20 000 would overflow a stack. A deeper subtree collapses to its
/// text, placed the way the innermost kept container takes children.
const MAX_NESTING: usize = 32;

fn parse_rewritten(source: &str, offset: usize, options: Options) -> BlockTree {
    let mut skip = Skip::default();
    // Every rewrite is checked against the parse it produced: a dollar pair
    // or a `\(…\)` pair must come out as exactly one formula, a destination
    // rewrite as the link it finishes. A block where one doesn't is reparsed
    // with its dollars literal and nothing in it rewritten (the rest keep
    // theirs); should that still fail, nothing anywhere is.
    for round in 0..3 {
        skip.all = round == 2;
        // Pre-passes run in order, each over the previous one's output;
        // `maps` records their edits so ranges walk back through them in
        // reverse.
        let mut text = Cow::Borrowed(source);
        let mut maps: Vec<OffsetMap> = Vec::new();
        let to_source = |maps: &[OffsetMap], range: Range<usize>| {
            maps.iter()
                .rev()
                .fold(range, |range, map| map.original_range(range))
        };
        let mut formulas = Vec::new();
        let mut destinations = Vec::new();
        // Links and images the source's first reading has: no formula may
        // break one.
        let mut links: Vec<Range<usize>> = Vec::new();
        if let Some(pass) = select_dollars(source, options, &skip) {
            text = Cow::Owned(pass.text);
            maps.push(pass.map);
            formulas = pass.pairs;
            links = pass.links;
        }
        if !skip.all {
            if let Some(pass) =
                rewrite_math_delimiters(&text, options, &skip, |at| to_source(&maps, at..at).start)
            {
                formulas.extend(pass.pairs.into_iter().map(|pair| to_source(&maps, pair)));
                if links.is_empty() {
                    links = pass
                        .links
                        .iter()
                        .map(|range| to_source(&maps, range.clone()))
                        .collect();
                }
                text = Cow::Owned(pass.text);
                maps.push(pass.map);
            }
            if let Some((rewritten, map, done)) =
                rewrite_absolute_destinations(&text, options, &skip, |at| {
                    to_source(&maps, at..at).start
                })
            {
                destinations = done;
                text = Cow::Owned(rewritten);
                maps.push(map);
            }
        }
        let events: Vec<(Event, Range<usize>)> = Parser::new_ext(&text, options)
            .into_offset_iter()
            .map(|(event, range)| (event, to_source(&maps, range)))
            .collect();
        let failed = if skip.all {
            Vec::new()
        } else {
            failed_rewrites(&events, &formulas, &destinations, &links)
        };
        if failed.is_empty() {
            return build_tree(events, source, offset);
        }
        // Each failure's innermost block (paragraph, heading, cell, item
        // text) stays literal next round — or, where no block holds it (a
        // rewrite that made its line a reference definition), its line.
        let mut leaves: Vec<Range<usize>> = events
            .iter()
            .filter(|(event, _)| {
                matches!(
                    event,
                    Event::Start(Tag::Paragraph | Tag::Heading { .. } | Tag::TableCell | Tag::Item)
                )
            })
            .map(|(_, range)| range.clone())
            .collect();
        leaves.sort_by_key(|range| range.start);
        for at in failed {
            // The innermost leaf holding `at`: the latest-starting one that
            // still reaches past it.
            let leaf = leaves[..leaves.partition_point(|leaf| leaf.start <= at)]
                .iter()
                .rev()
                .find(|leaf| at < leaf.end)
                .cloned();
            let block = leaf.unwrap_or_else(|| {
                let start = source[..at].rfind('\n').map_or(0, |nl| nl + 1);
                let end = source[at..]
                    .find('\n')
                    .map_or(source.len(), |nl| at + nl + 1);
                start..end
            });
            skip.literal.push(block);
        }
        // Kept sorted and disjoint for `Skip::off`'s binary search.
        skip.literal.sort_by_key(|range| range.start);
        let mut merged: Vec<Range<usize>> = Vec::new();
        for range in skip.literal.drain(..) {
            match merged.last_mut() {
                Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
                _ => merged.push(range),
            }
        }
        skip.literal = merged;
    }
    unreachable!("the last round rewrites nothing it checks")
}

/// The original offsets of rewrites the parse didn't keep: formula pairs
/// that aren't exactly one formula, destination rewrites whose link doesn't
/// end where they finished it, and links a formula broke.
fn failed_rewrites(
    events: &[(Event, Range<usize>)],
    formulas: &[Range<usize>],
    destinations: &[(usize, usize)],
    links: &[Range<usize>],
) -> Vec<usize> {
    if formulas.is_empty() && destinations.is_empty() {
        return Vec::new();
    }
    let math: std::collections::HashSet<&Range<usize>> = events
        .iter()
        .filter(|(event, _)| matches!(event, Event::InlineMath(_) | Event::DisplayMath(_)))
        .map(|(_, range)| range)
        .collect();
    let ends: std::collections::HashSet<usize> = events
        .iter()
        .filter(|(event, _)| matches!(event, Event::Start(Tag::Link { .. } | Tag::Image { .. })))
        .map(|(_, range)| range.end)
        .collect();
    let mut failed: Vec<usize> = formulas
        .iter()
        .filter(|pair| !math.contains(pair))
        .map(|pair| pair.start)
        .collect();
    failed.extend(
        destinations
            .iter()
            .filter(|(_, end)| !ends.contains(end))
            .map(|&(at, _)| at),
    );
    // A formula that ate part of a link (`[a $[$] b](u)`) broke it: a link
    // the first reading had is gone, and a formula overlaps where it was.
    // (One a destination rewrite displaced — nested links — isn't theirs.)
    if !formulas.is_empty() {
        let starts: std::collections::HashSet<usize> = events
            .iter()
            .filter(|(event, _)| {
                matches!(event, Event::Start(Tag::Link { .. } | Tag::Image { .. }))
            })
            .map(|(_, range)| range.start)
            .collect();
        let mut sorted: Vec<&Range<usize>> = formulas.iter().collect();
        sorted.sort_by_key(|pair| pair.start);
        for link in links.iter().filter(|link| !starts.contains(&link.start)) {
            let i = sorted.partition_point(|pair| pair.end <= link.start);
            if sorted.get(i).is_some_and(|pair| pair.start < link.end) {
                failed.push(link.start);
            }
        }
    }
    failed
}

/// The block tree of `events` (ranges into `source`, which starts at document
/// offset `offset`).
fn build_tree(events: Vec<(Event, Range<usize>)>, source: &str, offset: usize) -> BlockTree {
    /// What a kept container expects its children to be.
    #[derive(Clone, Copy, PartialEq)]
    enum Holds {
        Items,
        Rows,
        Cells,
        Anything,
    }
    let mut kept: Vec<(Event, Range<usize>)> = Vec::with_capacity(events.len());
    // Open items' ranges, and the kept containers' expectations.
    let mut items: Vec<Range<usize>> = Vec::new();
    let mut holds: Vec<Holds> = Vec::new();
    // A subtree past `MAX_NESTING` collapses to its text: (open starts in it,
    // its range, the text so far).
    let mut collapsing: Option<(usize, Range<usize>, String)> = None;
    for (event, range) in events {
        let range = range.start + offset..range.end + offset;
        if let Some((depth, _, text)) = &mut collapsing {
            match &event {
                Event::Start(tag) => {
                    *depth += 1;
                    if is_block_tag(tag) && !text.is_empty() && !text.ends_with(' ') {
                        text.push(' ');
                    }
                }
                Event::End(_) => *depth -= 1,
                Event::Text(t) | Event::Code(t) | Event::Html(t) | Event::InlineHtml(t) => {
                    text.push_str(t)
                }
                Event::InlineMath(t) => text.push_str(&format!("${t}$")),
                Event::DisplayMath(t) => text.push_str(&format!("$${t}$$")),
                Event::SoftBreak | Event::HardBreak => text.push(' '),
                _ => {}
            }
            if *depth > 0 {
                continue;
            }
            let Some((_, whole, text)) = collapsing.take() else {
                unreachable!()
            };
            if text.trim().is_empty() {
                continue;
            }
            // Wrapped the way the kept container takes children, so nothing
            // a list or table skips is lost; set off from text before it.
            let text = Event::Text(format!(" {}", text.trim()).into());
            let wrap: &[(Tag, TagEnd)] = match holds.last().copied().unwrap_or(Holds::Anything) {
                Holds::Items => &[(Tag::Item, TagEnd::Item)],
                Holds::Rows => &[
                    (Tag::TableRow, TagEnd::TableRow),
                    (Tag::TableCell, TagEnd::TableCell),
                ],
                Holds::Cells => &[(Tag::TableCell, TagEnd::TableCell)],
                Holds::Anything => &[],
            };
            for (start, _) in wrap {
                kept.push((Event::Start(start.clone()), whole.clone()));
            }
            kept.push((text, whole.clone()));
            for (_, end) in wrap.iter().rev() {
                kept.push((Event::End(*end), whole.clone()));
            }
            continue;
        }
        match &event {
            Event::Start(tag) => {
                if holds.len() >= MAX_NESTING {
                    collapsing = Some((1, range, String::new()));
                    continue;
                }
                holds.push(match tag {
                    Tag::List(_) => Holds::Items,
                    Tag::Table(_) => Holds::Rows,
                    Tag::TableHead | Tag::TableRow => Holds::Cells,
                    _ => Holds::Anything,
                });
                if matches!(tag, Tag::Item) {
                    items.push(range.clone());
                }
            }
            Event::End(end) => {
                holds.pop();
                if matches!(end, TagEnd::Item) {
                    items.pop();
                }
            }
            // pulldown-cmark can emit a task marker after its item has closed
            // (an item holding only a nested list or a heading: `- [x] *`),
            // so it lands in a later paragraph or item and would check that
            // one off. A marker belongs to the innermost open item, inside
            // its source.
            Event::TaskListMarker(_)
                if !items
                    .last()
                    .is_some_and(|item| item.start <= range.start && range.end <= item.end) =>
            {
                continue;
            }
            // pulldown can start a marker's range at the whitespace before it
            // (`- - \t[x]`); a host replacing `source[range]` must find just
            // the brackets.
            Event::TaskListMarker(_) => {
                let spelled = &source[range.start - offset..range.end - offset];
                let lead = spelled.len() - spelled.trim_start().len();
                kept.push((event, range.start + lead..range.end));
                continue;
            }
            _ => {}
        }
        kept.push((event, range));
    }
    let events = kept;
    let mut cur = Cursor {
        events: &events,
        ix: 0,
        source,
        offset,
    };
    let mut blocks = Vec::new();
    while let Some((event, range)) = cur.peek() {
        let range = range.clone();
        match event {
            Event::Rule => {
                cur.bump();
                blocks.push(Arc::new(TopBlock {
                    range,
                    block: Block::Rule,
                }));
            }
            Event::Start(_) => {
                for block in parse_started_block(&mut cur) {
                    blocks.push(Arc::new(TopBlock {
                        range: range.clone(),
                        block,
                    }));
                }
            }
            // Stray inline events at top level (shouldn't happen): skip.
            _ => cur.bump(),
        }
    }
    BlockTree { blocks }
}

/// Insert a backslash before the `$` at each (ascending) offset, so it reads
/// as a literal dollar.
fn escape_dollars(text: &str, at: &[usize]) -> (String, OffsetMap) {
    let mut out = String::with_capacity(text.len() + at.len());
    let mut map = OffsetMap::default();
    let mut copied = 0;
    for &at in at {
        out.push_str(&text[copied..at]);
        map.shift(out.len(), -1);
        out.push('\\');
        copied = at;
    }
    out.push_str(&text[copied..]);
    (out, map)
}

/// The edits a rewrite pass made, as `(position, shift)` pairs in rewritten
/// byte order: every rewritten offset past `position` maps back to the
/// original by adding `shift` (the running total of bytes removed minus bytes
/// inserted up to there). Each edit replaces a delimiter with the single byte
/// at `position`, so a range starting on it maps to the original delimiter's
/// start and a range ending after it maps past the original delimiter's end.
#[derive(Debug, Default)]
struct OffsetMap {
    edits: Vec<(usize, isize)>,
}

impl OffsetMap {
    /// Record `delta` original bytes more (or, negative, fewer) behind the
    /// rewritten byte at `position`.
    fn shift(&mut self, position: usize, delta: isize) {
        let total = self.total() + delta;
        self.edits.push((position, total));
    }

    fn total(&self) -> isize {
        self.edits.last().map_or(0, |&(_, total)| total)
    }

    fn original(&self, offset: usize) -> usize {
        match self.edits.partition_point(|&(at, _)| at < offset) {
            0 => offset,
            n => offset.saturating_add_signed(self.edits[n - 1].1),
        }
    }

    fn original_range(&self, range: Range<usize>) -> Range<usize> {
        self.original(range.start)..self.original(range.end)
    }
}

/// One stretch of inline text pulldown-cmark reads in a source: a `Text`
/// event's range; the inline container holding it (paragraph, heading, table
/// cell, list item text, link text — an id that changes at every block, link
/// or raw-HTML boundary); and whether its first byte is backslash-escaped.
/// Code (fenced or indented, in any container), raw HTML, autolinks, link
/// destinations, reference labels and definitions never produce such text,
/// so a rewrite that only touches it can't reach them.
struct InlineText {
    range: Range<usize>,
    container: u32,
    escaped: bool,
}

/// What one parse of a source says about its inline content: its text
/// stretches and its links and images.
struct Inlines {
    texts: Vec<InlineText>,
    links: Vec<Range<usize>>,
}

fn inline_texts(source: &str, options: Options) -> Inlines {
    let bytes = source.as_bytes();
    let mut texts = Vec::new();
    let mut links = Vec::new();
    let mut container = 0u32;
    // Code blocks, autolinks and reference links: text that keeps its
    // spelling.
    let mut literal = 0usize;
    let mut kept: Vec<bool> = Vec::new();
    // End of the last inline event: an escape's backslash is the one byte no
    // event covers right before the escaped character.
    let mut covered = 0usize;
    for (event, range) in Parser::new_ext(source, options).into_offset_iter() {
        match &event {
            Event::Start(Tag::CodeBlock(_)) => {
                literal += 1;
                container += 1;
            }
            Event::End(TagEnd::CodeBlock) => {
                literal -= 1;
                container += 1;
            }
            // Autolinks spell a URL; a reference link's (or image's) text is
            // the label its definition must match, rewritten it would match
            // none.
            Event::Start(Tag::Link { link_type, .. } | Tag::Image { link_type, .. }) => {
                links.push(range.clone());
                let keep = !matches!(link_type, LinkType::Inline);
                literal += usize::from(keep);
                kept.push(keep);
                container += 1;
            }
            Event::End(TagEnd::Link | TagEnd::Image) => {
                literal -= usize::from(kept.pop().unwrap_or(false));
                container += 1;
            }
            Event::Start(Tag::Emphasis | Tag::Strong | Tag::Strikethrough)
            | Event::End(TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough) => {}
            Event::Start(_) | Event::End(_) => container += 1,
            Event::InlineHtml(_) | Event::Html(_) => {
                container += 1;
                covered = covered.max(range.end);
            }
            Event::Text(_) => {
                // Text pulldown reports inside an earlier event (raw HTML
                // that overlaps a later block) is no candidate.
                if literal == 0 && range.start >= covered {
                    let at = range.start;
                    texts.push(InlineText {
                        range: range.clone(),
                        container,
                        escaped: at > covered && bytes[at - 1] == b'\\',
                    });
                }
                covered = covered.max(range.end);
            }
            _ => covered = covered.max(range.end),
        }
    }
    Inlines { texts, links }
}

/// Which dollars are formula delimiters, decided once from a math-free parse
/// so pulldown-cmark never pairs the rest: every other `$` in inline text is
/// escaped (returned with its map, each kept pair's original range for the
/// caller to check against the parse, and the parse's links — a formula
/// mustn't break one). A run of one `$` opens before a non-space and closes
/// after one; `$$` opens and closes display math; longer runs are literal.
/// An opener pairs with the next closer of its kind in the same inline
/// container (TeX's own `*` and `_` may look like emphasis to this parse:
/// `$x^*$ and $y^*$` still pairs), unless the span reads as prose, shell or
/// code ([`dollar_prose`]) — then the opener stays a literal `$` and the
/// closer may open the next pair, as Pandoc reads `$5 and $10`. Dollars in
/// code, raw HTML or URLs aren't inline text (or are skipped) and keep their
/// spelling; where `skip` turns rewriting off, every dollar stays literal.
/// One parse, linear: each span is checked once.
fn select_dollars(source: &str, options: Options, skip: &Skip) -> Option<FormulaPass> {
    if !source.contains('$') {
        return None;
    }
    let bytes = source.as_bytes();
    struct Run {
        start: usize,
        end: usize,
        container: u32,
    }
    let Inlines { texts, links } = inline_texts(source, options - Options::ENABLE_MATH);
    let mut runs = Vec::new();
    for text in texts {
        let mut at = text.range.start;
        // An escaped `$` (`\$`) starts its own text and stays literal.
        if text.escaped && bytes[at] == b'$' {
            at += 1;
        }
        while at < text.range.end {
            if bytes[at] != b'$' {
                at += 1;
                continue;
            }
            let start = at;
            while at < text.range.end && bytes[at] == b'$' {
                at += 1;
            }
            runs.push(Run {
                start,
                end: at,
                container: text.container,
            });
        }
    }
    if runs.is_empty() {
        return None;
    }
    let space_before = |at: usize| {
        source[..at]
            .chars()
            .next_back()
            .is_none_or(char::is_whitespace)
    };
    let space_after = |at: usize| source[at..].chars().next().is_none_or(char::is_whitespace);
    // Kept delimiter pairs: (opener, closer) byte ranges.
    let mut pairs: Vec<(Range<usize>, Range<usize>)> = Vec::new();
    // The opener awaiting its closer, with its container.
    let mut pending: Option<(Range<usize>, u32)> = None;
    let mut words = UrlWords::default();
    for run in &runs {
        // Part of a URL or query string, or a sigil: never a delimiter.
        if words.url(bytes, run.start) {
            continue;
        }
        let mut start = run.start;
        let same = |open: &(Range<usize>, u32)| open.1 == run.container;
        // `$a$$b$`: a `$$` glued to an inline formula's content closes it
        // and opens the next.
        if run.end - start == 2
            && let Some(open) = pending.clone()
            && open.0.len() == 1
            && same(&open)
            && !space_before(start)
            && !dollar_prose(source, open.0.start..start + 1, false)
        {
            pairs.push((open.0, start..start + 1));
            pending = None;
            start += 1;
        }
        let len = run.end - start;
        if len > 2 {
            pending = None;
            continue;
        }
        if let Some(open) = pending.clone()
            && open.0.len() == len
            && same(&open)
            && (len == 2 || !space_before(start))
        {
            pending = None;
            if !dollar_prose(source, open.0.start..run.end, len == 2) {
                pairs.push((open.0, start..run.end));
                continue;
            }
        }
        let opens = len == 2 || !space_after(run.end);
        if opens && !skip.off(start) {
            pending = Some((start..run.end, run.container));
        } else if pending.as_ref().is_some_and(|open| open.1 != run.container) {
            pending = None;
        }
    }
    // Escape every dollar not in a kept pair.
    let mut kept: Vec<usize> = pairs
        .iter()
        .flat_map(|(open, close)| open.clone().chain(close.clone()))
        .collect();
    kept.sort_unstable();
    let escape: Vec<usize> = runs
        .iter()
        .flat_map(|run| run.start..run.end)
        .filter(|at| kept.binary_search(at).is_err())
        .collect();
    let (text, map) = escape_dollars(source, &escape);
    let pairs = pairs
        .into_iter()
        .map(|(open, close)| open.start..close.end)
        .collect();
    Some(FormulaPass {
        text,
        map,
        pairs,
        links,
    })
}

/// Tracks, for dollar spans in source order, whether each opens inside a URL
/// word (`scheme://…`) or right after a query or sigil character (`?$top`,
/// `&$x`, `@$ref`, `%$h` — but not TeX's `\%$`). Each word's start (past
/// leading brackets, quotes and emphasis: `**https://…`) is found once, so
/// it stays linear.
#[derive(Default)]
struct UrlWords {
    scanned: usize,
    start: usize,
}

impl UrlWords {
    fn url(&mut self, bytes: &[u8], at: usize) -> bool {
        let from = self.scanned.min(at);
        if let Some(space) = bytes[from..at].iter().rposition(u8::is_ascii_whitespace) {
            self.start = from + space + 1;
            self.start += bytes[self.start..at]
                .iter()
                .take_while(|byte| b"([<\"'*_~".contains(byte))
                .count();
        } else if self.scanned == 0 {
            self.start += bytes[self.start..at]
                .iter()
                .take_while(|byte| b"([<\"'*_~".contains(byte))
                .count();
        }
        self.scanned = self.scanned.max(at);
        let sigil = at > 0
            && matches!(bytes[at - 1], b'?' | b'&' | b'@' | b'%')
            && !(at > 1 && bytes[at - 2] == b'\\');
        let word = &bytes[self.start.min(at)..at];
        let name = word
            .iter()
            .take(32)
            .take_while(|byte| byte.is_ascii_alphanumeric() || b"+.-".contains(byte))
            .count();
        let scheme = name > 0 && word[0].is_ascii_alphabetic() && word[name..].starts_with(b"://");
        sigil || scheme
    }
}

/// Whether the dollar span `spelled` (in `source`, delimiters included)
/// reads as prose, shell or code rather than a formula ([`not_math`],
/// [`display_prose`]), judged with its neighbours: an identifier glued to
/// both sides (`TOKEN=$TOKEN -e ID=$ID`, R's `df$col`, Vue's `this.$refs`),
/// or a quoted string interpolating one (`"$name$suffix"`).
fn dollar_prose(source: &str, spelled: Range<usize>, display: bool) -> bool {
    let bytes = source.as_bytes();
    let delimiter = if display { 2 } else { 1 };
    if spelled.len() < 2 * delimiter {
        return true;
    }
    let tex = &source[spelled.start + delimiter..spelled.end - delimiter];
    let prev = spelled.start.checked_sub(1).map(|at| bytes[at]);
    let next = bytes.get(spelled.end).copied();
    if display {
        return display_prose(tex, next);
    }
    let word_after = next.is_some_and(|b| b.is_ascii_alphanumeric());
    let glued = prev.is_some_and(|b| b.is_ascii_alphanumeric() || b"=.".contains(&b));
    let quoted = prev.is_some_and(|b| b"\"'".contains(&b));
    ((glued || quoted) && word_after) || not_math(tex, next)
}

/// Whether a `$$…$$` span is prose, not display math: blank, a code span it
/// straddles (a backtick), a price (`$$5 vs $$10`: a closer glued to a
/// digit), or shell text around the PID variable (`kill $$ && echo $$`).
fn display_prose(tex: &str, next: Option<u8>) -> bool {
    // Words with no TeX syntax at all: the shell PID in a sentence (`Use $$
    // for the PID and $$ again`, `echo $$; echo $$`).
    let words = !tex.bytes().any(|b| b"\\^_{}=+<>|".contains(&b))
        && tex
            .split(|c: char| !c.is_ascii_alphabetic())
            .any(|word| word.len() >= 2);
    tex.trim().is_empty()
        || tex.contains('`')
        || next.is_some_and(|byte| byte.is_ascii_digit())
        || tex.contains("&&")
        || tex.contains("||")
        || words
}

/// Rewrite the LaTeX delimiters models favor — `\(…\)` inline, `\[…\]`
/// display — to the `$…$` / `$$…$$` pulldown-cmark understands, returning
/// each pair's original range (opener to past its closer) for the caller to
/// check against the parse. Only delimiters pulldown itself reads as
/// backslash escapes in inline text count ([`inline_texts`]), so code, raw
/// HTML, URLs, reference labels and definitions keep their spelling wherever
/// they sit, and `\\(` (a literal backslash) is never an opener. A closer
/// pairs with the nearest opener of its kind before it in the same inline
/// container — a still-streaming formula stays literal, so the rewrite is
/// append-stable, and both delimiters share one top-level block, so a tail
/// reparse rewrites exactly as the full parse does. Openers glued to a word
/// or inside a URL or path, openers where `skip` is off, and bodies that can't be (or
/// survive as) TeX ([`math_body`]) stay literal. Linear in the source: each
/// body is checked once.
fn rewrite_math_delimiters(
    source: &str,
    options: Options,
    skip: &Skip,
    to_source: impl Fn(usize) -> usize,
) -> Option<FormulaPass> {
    // Without a delimiter where rewriting is on, skip the parse.
    let live = |pattern: &str| {
        source
            .match_indices(pattern)
            .any(|(at, _)| !skip.off(to_source(at)))
    };
    if !live("\\(") && !live("\\[") {
        return None;
    }
    let bytes = source.as_bytes();
    let mut words = WordScan::default();
    let mut pairs: Vec<(usize, usize, bool)> = Vec::new();
    // The latest opener of each kind (inline, display) awaiting its closer.
    let mut pending: [Option<usize>; 2] = [None, None];
    let mut container = None;
    let Inlines { texts, links } = inline_texts(source, options);
    for text in texts {
        let at = text.range.start;
        if !text.escaped || !matches!(bytes[at], b'(' | b'[' | b')' | b']') {
            continue;
        }
        if container != Some(text.container) {
            container = Some(text.container);
            pending = [None, None];
        }
        let backslash = at - 1;
        let display = matches!(bytes[at], b'[' | b']');
        let kind = usize::from(display);
        if matches!(bytes[at], b'(' | b'[') {
            if !skip.off(to_source(backslash)) && !words.glued(bytes, backslash) {
                pending[kind] = Some(backslash);
            }
        } else if let Some(open) = pending[kind].take()
            && math_body(
                &source[open + 2..backslash],
                display,
                display && mid_line(bytes, open),
            )
        {
            pairs.push((open, backslash, display));
            pending = [None, None];
        }
    }
    if pairs.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(source.len());
    let mut map = OffsetMap::default();
    let mut copied = 0;
    for &(at, close, display) in &pairs {
        out.push_str(&source[copied..at]);
        let body = &source[at + 2..close];
        if display {
            // `\[` and `$$` are the same length: no offsets move.
            out.push_str("$$");
            out.push_str(body);
            out.push_str("$$");
        } else {
            let inner = body.trim_start();
            let lead = body.len() - inner.len();
            let inner = inner.trim_end();
            let trail = body.len() - lead - inner.len();
            // Each `$` stands in for its two-byte delimiter plus the
            // whitespace it swallowed.
            map.shift(out.len(), 1 + lead as isize);
            out.push('$');
            out.push_str(inner);
            map.shift(out.len(), 1 + trail as isize);
            out.push('$');
        }
        copied = close + 2;
    }
    out.push_str(&source[copied..]);
    let pairs = pairs.iter().map(|&(at, close, _)| at..close + 2).collect();
    Some(FormulaPass {
        text: out,
        map,
        pairs,
        links,
    })
}

/// Whether a delimited body can be a formula and survives as one: not blank;
/// no unescaped `$` (it would pair with the rewritten delimiters), no
/// backtick (TeX has no use for one; it belongs to a code span the
/// delimiters straddle), no unescaped `%` (a TeX comment would swallow the
/// rest) and no URL; an inline body's surrounding whitespace on one line
/// (trimming a newline would move a line); not ending in a backslash (it
/// would escape the closing `$`); and not prose ([`prose_like`]; a display
/// body opened `mid_line` is prose unless it has TeX syntax).
fn math_body(body: &str, display: bool, mid_line: bool) -> bool {
    let inner = body.trim();
    if inner.is_empty()
        || body.contains('`')
        || body.contains("**")
        || body.contains("__")
        || body.contains("://")
        || has_unescaped(body, b'$')
        || has_unescaped(body, b'%')
    {
        return false;
    }
    if !display {
        let lead = &body[..body.len() - body.trim_start().len()];
        let trail = &body[body.trim_end().len()..];
        if lead.contains('\n') || trail.contains('\n') {
            return false;
        }
    }
    // A one-character display body is an escaped checkbox (`\[x\] Done`).
    if display && inner.chars().count() <= 1 {
        return false;
    }
    let escaping = inner
        .bytes()
        .rev()
        .take_while(|&byte| byte == b'\\')
        .count()
        % 2
        == 1;
    !escaping && !prose_like(inner, mid_line)
}

/// Whether the backslash at `at` follows other text on its line (not just
/// indentation and block markers): display math written mid-sentence is
/// usually markdown escaping (`array \[i\]`). Looks back only to the first
/// byte that isn't indentation or a marker.
fn mid_line(bytes: &[u8], at: usize) -> bool {
    bytes[..at]
        .iter()
        .rev()
        .find(|byte| !b" \t>-*+".contains(byte))
        .is_some_and(|&byte| byte != b'\n')
}

/// Whether a delimited body reads as prose, not TeX: with no TeX syntax at
/// all (a command, `^`, `_`, braces, an operator), a citation or number
/// (`\[1, 2\]`, `\(2.3\)`), words (`\(optional\)`, `\(see **note**\)`),
/// quotes or a `#`; or anything opened `mid_line` (`array \[i\]`). A short
/// variable (`\(x\)`, `\(ab\)`, `\(x, y\)`) stays math.
fn prose_like(body: &str, mid_line: bool) -> bool {
    if body.bytes().any(|byte| b"\\^_{}=+<>|&".contains(&byte)) {
        return false;
    }
    // A token mixing letters and two or more digits is a version or an
    // architecture (`x86`, `arm64`, `win32`), not a product of variables.
    let version = |word: &str| {
        word.bytes().all(|byte| byte.is_ascii_alphanumeric())
            && word.bytes().any(|byte| byte.is_ascii_alphabetic())
            && word.bytes().filter(u8::is_ascii_digit).count() >= 2
    };
    mid_line
        || body.split_whitespace().any(version)
        || body
            .bytes()
            .all(|byte| byte.is_ascii_digit() || b" .,;:-".contains(&byte))
        || body
            .split(|c: char| !c.is_ascii_alphabetic())
            .any(|word| word.len() >= 3)
        // Punctuation only (`\(.*\)`): a regex group, not a formula.
        || !body.bytes().any(|byte| byte.is_ascii_alphanumeric())
        || body.contains(['"', '#'])
        || body.contains("**")
}

/// Whether `body` holds `byte` (an ASCII punctuation char) without a
/// backslash escaping it.
fn has_unescaped(body: &str, byte: u8) -> bool {
    let mut escaped = false;
    for &b in body.as_bytes() {
        if b == byte && !escaped {
            return true;
        }
        escaped = b == b'\\' && !escaped;
    }
    false
}

/// Whether a backslash at `at` is glued to what precedes it: a word character
/// (`f\(x\)`, `x_\(1\)`), or any non-space byte inside a URL- or path-like
/// word (one holding a `/`). Asked in ascending `at` order; tracks the current
/// word incrementally, so a long line of openers stays linear.
#[derive(Default)]
struct WordScan {
    scanned: usize,
    slash: bool,
}

impl WordScan {
    fn glued(&mut self, bytes: &[u8], at: usize) -> bool {
        for &byte in &bytes[self.scanned..at] {
            if byte.is_ascii_whitespace() {
                self.slash = false;
            } else if byte == b'/' {
                self.slash = true;
            }
        }
        self.scanned = at;
        let Some(&prev) = at.checked_sub(1).and_then(|prev| bytes.get(prev)) else {
            return false;
        };
        prev.is_ascii_alphanumeric() || prev == b'_' || (self.slash && !prev.is_ascii_whitespace())
    }
}

/// Wrap inline link and image destinations that spell an absolute path with
/// literal spaces in pulldown-cmark's angle-bracket form: CommonMark ends a
/// destination at its first space, so such a link otherwise stays plain text.
/// Only a `](` inside a paragraph, heading, cell or item — and outside code,
/// formulas, raw HTML and autolinks, per pulldown's own parse — is a
/// candidate, so those keep their spelling wherever they sit. A destination
/// already bracketed, or whose closing `)` hasn't streamed in yet, is left
/// alone, which makes the rewrite idempotent and safe while text is still
/// arriving. Returns each rewrite's `](` and the end its link must reach, as
/// original offsets (`to_source`), for the caller to check. Linear.
fn rewrite_absolute_destinations(
    source: &str,
    options: Options,
    skip: &Skip,
    to_source: impl Fn(usize) -> usize,
) -> Option<Rewritten<(usize, usize)>> {
    // Only an absolute destination can need the rewrite: without one, skip
    // the parse.
    let absolute = source.match_indices("](").any(|(at, _)| {
        let rest = source[at + 2..].trim_start_matches(' ');
        (rest.starts_with('/') || rest.starts_with("file://")) && !skip.off(to_source(at))
    });
    if !absolute {
        return None;
    }
    let bytes = source.as_bytes();
    let escaped = escapes(bytes);
    // Where inline syntax is read, and where inside it it isn't.
    let mut allowed: Vec<Range<usize>> = Vec::new();
    let mut excluded: Vec<Range<usize>> = Vec::new();
    for (event, range) in Parser::new_ext(source, options).into_offset_iter() {
        match event {
            Event::Start(Tag::Paragraph | Tag::Heading { .. } | Tag::TableCell | Tag::Item) => {
                allowed.push(range)
            }
            Event::Start(Tag::CodeBlock(_))
            | Event::Start(Tag::Link {
                link_type: LinkType::Autolink | LinkType::Email,
                ..
            })
            | Event::Code(_)
            | Event::InlineMath(_)
            | Event::DisplayMath(_)
            | Event::Html(_)
            | Event::InlineHtml(_) => excluded.push(range),
            _ => {}
        }
    }
    let inside = |ranges: &mut Vec<Range<usize>>| {
        ranges.sort_by_key(|range| range.start);
        let mut merged: Vec<Range<usize>> = Vec::new();
        for range in ranges.drain(..) {
            match merged.last_mut() {
                Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
                _ => merged.push(range),
            }
        }
        merged
    };
    let (allowed, excluded) = (inside(&mut allowed), inside(&mut excluded));
    let (mut a, mut e) = (0, 0);
    let mut starts = Vec::new();
    let mut at = 0;
    while let Some(found) = source[at..].find("](") {
        let pos = at + found;
        at = pos + 1;
        while a < allowed.len() && allowed[a].end <= pos {
            a += 1;
        }
        while e < excluded.len() && excluded[e].end <= pos {
            e += 1;
        }
        let readable = allowed.get(a).is_some_and(|range| range.start <= pos)
            && excluded.get(e).is_none_or(|range| range.start > pos);
        if readable && !escaped[pos] {
            starts.push((pos, skip_destination_spaces(bytes, pos + 2)));
        }
    }
    let closes = destination_closes(bytes, &escaped, starts.iter().map(|&(_, start)| start));
    let mut rewritten: Option<String> = None;
    let mut insertions = OffsetMap::default();
    let mut copied = 0usize;
    let mut done = Vec::new();
    // A `](` inside a destination that closed is part of that destination.
    let mut free = 0usize;
    for (&(at, start), close) in starts.iter().zip(closes) {
        let Some(close) = close.filter(|_| at >= free) else {
            continue;
        };
        free = close + 1;
        let destination = source[start..close].trim_end_matches(' ');
        if absolute_destination_with_spaces(destination) && !skip.off(to_source(at)) {
            done.push((to_source(at), to_source(close) + 1));
            let out = rewritten.get_or_insert_with(|| String::with_capacity(source.len() + 8));
            out.push_str(&source[copied..start]);
            insertions.shift(out.len(), -1);
            out.push('<');
            out.push_str(destination);
            insertions.shift(out.len(), -1);
            out.push('>');
            copied = start + destination.len();
        }
    }
    let mut out = rewritten?;
    out.push_str(&source[copied..]);
    Some((out, insertions, done))
}

/// A pre-pass's output: the rewritten text, its offset map, and what each
/// rewrite is for the caller to check against the parse.
type Rewritten<T> = (String, OffsetMap, Vec<T>);

/// A formula pre-pass's output: the rewritten text and its offset map, each
/// kept pair's range, and where the links and images it parsed start (in
/// the pass's input) — a formula must not break any of them.
struct FormulaPass {
    text: String,
    map: OffsetMap,
    pairs: Vec<Range<usize>>,
    links: Vec<Range<usize>>,
}

/// Which bytes a backslash escapes: an odd run of backslashes before a byte.
fn escapes(bytes: &[u8]) -> Vec<bool> {
    let mut escaped = vec![false; bytes.len()];
    for at in 1..bytes.len() {
        escaped[at] = bytes[at - 1] == b'\\' && !escaped[at - 1];
    }
    escaped
}

/// Whether a destination needs the angle-bracket rewrite: an absolute path
/// with at least one literal space that stays inside the absolute-path
/// envelope the link classifier accepts. `file://` destinations are exempt
/// from the file-name dot rule, exactly as they are when classified.
fn absolute_destination_with_spaces(destination: &str) -> bool {
    if !destination.contains(' ') {
        return false;
    }
    let path = match destination.strip_prefix("file://") {
        Some(rest) => {
            // Only an empty host keeps this a file path; `file://localhost`
            // and friends are ordinary URLs.
            let host_end = rest.find('/').unwrap_or(rest.len());
            if !rest[..host_end].is_empty() {
                return false;
            }
            &rest[host_end..]
        }
        None if destination.starts_with('/') => destination,
        None => return false,
    };
    if path.len() <= 1
        || path.starts_with("//")
        || path.ends_with('/')
        || destination.contains(['\\', '?'])
        || destination.chars().any(char::is_control)
        || destination
            .split('/')
            .any(|part| matches!(part, "." | ".."))
    {
        return false;
    }
    let file_name = path.rsplit('/').next().unwrap_or(path);
    destination.starts_with("file://") || file_name.contains('.')
}

fn skip_destination_spaces(bytes: &[u8], at: usize) -> usize {
    let mut at = at;
    while bytes.get(at) == Some(&b' ') {
        at += 1;
    }
    at
}

/// For each destination start (ascending), the first unescaped `)` at paren
/// depth zero from it — the destination's end. `)` inside balanced
/// parentheses belongs to the path, an escaped `)` does not close, and a
/// control character (a newline mid-destination, a tab) leaves the link alone
/// rather than guessing at its shape. One backward pass for all starts: the
/// `)`s still unmatched by a `(` at or after a position are exactly the ones a
/// forward scan from there would reach at depth zero, the nearest first.
fn destination_closes(
    bytes: &[u8],
    escaped: &[bool],
    starts: impl Iterator<Item = usize>,
) -> Vec<Option<usize>> {
    let starts: Vec<usize> = starts.collect();
    let mut closes = vec![None; starts.len()];
    let mut query = starts.partition_point(|&start| start < bytes.len());
    let mut pending: Vec<usize> = Vec::new();
    for at in (0..bytes.len()).rev() {
        // A line break ends a destination, escaped or not.
        if matches!(bytes[at], b'\n' | b'\r') {
            pending.clear();
        } else if !escaped[at] {
            match bytes[at] {
                b')' => pending.push(at),
                b'(' => {
                    pending.pop();
                }
                b'\\' => {}
                byte if byte.is_ascii_control() => pending.clear(),
                _ => {}
            }
        }
        while query > 0 && starts[query - 1] == at {
            query -= 1;
            closes[query] = pending.last().copied();
        }
    }
    closes
}

/// The forward scan [`destination_closes`] answers in bulk (the tests hold
/// the two equal).
#[cfg(test)]
fn destination_close(bytes: &[u8], start: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut at = start;
    while at < bytes.len() {
        match bytes[at] {
            b'\\'
                if bytes
                    .get(at + 1)
                    .is_some_and(|byte| !matches!(byte, b'\n' | b'\r')) =>
            {
                at += 2
            }
            b'(' => {
                depth += 1;
                at += 1;
            }
            b')' if depth == 0 => return Some(at),
            b')' => {
                depth -= 1;
                at += 1;
            }
            byte if byte.is_ascii_control() => return None,
            _ => at += 1,
        }
    }
    None
}

struct Cursor<'a, 'e> {
    events: &'a [(Event<'e>, Range<usize>)],
    ix: usize,
    /// The original (unrewritten) source the event ranges point into, and
    /// the document offset of its first byte.
    source: &'a str,
    offset: usize,
}

impl<'a, 'e> Cursor<'a, 'e> {
    fn peek(&self) -> Option<&(Event<'e>, Range<usize>)> {
        self.events.get(self.ix)
    }

    fn peek_event(&self) -> Option<&Event<'e>> {
        self.peek().map(|(e, _)| e)
    }

    fn bump(&mut self) {
        self.ix += 1;
    }

    /// The original source spelling of a document range, plus the byte after
    /// it (if any).
    fn spelling(&self, range: &Range<usize>) -> (&'a str, Option<u8>) {
        let (start, end) = (range.start - self.offset, range.end - self.offset);
        (
            &self.source[start..end],
            self.source.as_bytes().get(end).copied(),
        )
    }

    fn next_event(&mut self) -> Option<Event<'e>> {
        let event = self.events.get(self.ix).map(|(e, _)| e.clone());
        if event.is_some() {
            self.ix += 1;
        }
        event
    }
}

fn is_block_tag(tag: &Tag) -> bool {
    matches!(
        tag,
        Tag::Paragraph
            | Tag::Heading { .. }
            | Tag::CodeBlock(_)
            | Tag::BlockQuote(_)
            | Tag::List(_)
            | Tag::Item
            | Tag::Table(_)
            | Tag::HtmlBlock
            | Tag::FootnoteDefinition(_)
    )
}

/// Consume a `Start(tag)` and everything through its matching `End`, producing
/// block(s). Unknown containers are transparent (children splice in).
fn parse_started_block(cur: &mut Cursor) -> Vec<Block> {
    let Some(Event::Start(tag)) = cur.next_event() else {
        return Vec::new();
    };
    match tag {
        Tag::Paragraph => parse_paragraph(cur),
        Tag::Heading { level, .. } => vec![Block::Heading {
            level: heading_level(level),
            runs: parse_inline_container(cur, &InlineStyle::default()),
        }],
        Tag::CodeBlock(kind) => {
            let language = match kind {
                CodeBlockKind::Fenced(info) => {
                    let lang = info.split_whitespace().next().unwrap_or("");
                    if lang.is_empty() {
                        None
                    } else {
                        Some(lang.to_string())
                    }
                }
                CodeBlockKind::Indented => None,
            };
            let mut code = String::new();
            loop {
                match cur.next_event() {
                    Some(Event::Text(t)) => code.push_str(&t),
                    Some(Event::End(_)) | None => break,
                    Some(_) => {}
                }
            }
            // Fenced blocks carry a trailing newline; render per-line without it.
            if code.ends_with('\n') {
                code.pop();
            }
            vec![Block::CodeBlock { language, code }]
        }
        Tag::BlockQuote(_) => vec![Block::BlockQuote {
            children: parse_block_sequence(cur),
        }],
        Tag::List(ordered_start) => {
            let mut items = Vec::new();
            loop {
                match cur.peek_event() {
                    Some(Event::Start(Tag::Item)) => {
                        cur.bump();
                        items.push(parse_block_sequence(cur));
                    }
                    Some(Event::End(_)) | None => {
                        cur.bump();
                        break;
                    }
                    Some(_) => cur.bump(),
                }
            }
            vec![Block::List {
                ordered_start,
                items,
            }]
        }
        Tag::Table(align) => {
            let align = align
                .iter()
                .map(|a| match a {
                    Alignment::Center => TableAlign::Center,
                    Alignment::Right => TableAlign::Right,
                    Alignment::None | Alignment::Left => TableAlign::Left,
                })
                .collect();
            vec![parse_table(cur, align)]
        }
        Tag::HtmlBlock => {
            // Render raw HTML blocks as plain text (zeron's markdown does the same).
            let mut text = String::new();
            loop {
                match cur.next_event() {
                    Some(Event::Html(t)) | Some(Event::Text(t)) => text.push_str(&t),
                    Some(Event::End(_)) | None => break,
                    Some(_) => {}
                }
            }
            let text = text.trim_end_matches('\n').to_string();
            if text.is_empty() {
                Vec::new()
            } else {
                vec![Block::Paragraph {
                    runs: vec![InlineRun {
                        text,
                        style: InlineStyle::default(),
                    }],
                }]
            }
        }
        // Transparent containers (footnote definitions when enabled, etc.).
        _ => parse_block_sequence(cur),
    }
}

/// Parse a block sequence until the container's `End` (consumed). Bare inline
/// events (tight list items) accumulate into an implicit paragraph.
fn parse_block_sequence(cur: &mut Cursor) -> Vec<Block> {
    let mut out: Vec<Block> = Vec::new();
    let mut inline_acc: Vec<InlineRun> = Vec::new();
    // Text after a display formula drops its leading break (see
    // `take_display_math`).
    let mut after_math = false;
    while let Some(event) = cur.peek_event() {
        match event {
            Event::End(_) => {
                cur.bump();
                break;
            }
            Event::Start(tag) if is_block_tag(tag) => {
                flush_paragraph(&mut out, &mut inline_acc, std::mem::take(&mut after_math));
                out.extend(parse_started_block(cur));
            }
            Event::Rule => {
                flush_paragraph(&mut out, &mut inline_acc, std::mem::take(&mut after_math));
                cur.bump();
                out.push(Block::Rule);
            }
            _ => {
                if !take_display_math(cur, &mut out, &mut inline_acc, &mut after_math) {
                    parse_inline_event(cur, &mut inline_acc, &InlineStyle::default());
                }
            }
        }
    }
    flush_paragraph(&mut out, &mut inline_acc, after_math);
    out
}

fn flush_paragraph(out: &mut Vec<Block>, acc: &mut Vec<InlineRun>, after_math: bool) {
    if after_math {
        trim_runs_start(acc);
    }
    if !acc.is_empty() {
        out.push(Block::Paragraph {
            runs: merge_runs(std::mem::take(acc)),
        });
    }
}

/// A paragraph's inline content, split around display math: `$$…$$` on its
/// own is a [`Block::Math`], and one mid-paragraph ends the text before it
/// and starts a new paragraph after it. Display math nested in emphasis or a
/// link stays an inline math run. All the pieces share the paragraph's range.
fn parse_paragraph(cur: &mut Cursor) -> Vec<Block> {
    let mut out = Vec::new();
    let mut runs = Vec::new();
    let mut after_math = false;
    while let Some(event) = cur.peek_event() {
        if matches!(event, Event::End(_)) {
            cur.bump();
            break;
        }
        if !take_display_math(cur, &mut out, &mut runs, &mut after_math) {
            parse_inline_event(cur, &mut runs, &InlineStyle::default());
        }
    }
    if after_math {
        trim_runs_start(&mut runs);
    }
    if !runs.is_empty() || out.is_empty() {
        out.push(Block::Paragraph {
            runs: autolink_runs(merge_runs(runs)),
        });
    }
    out
}

/// Consume a display-math event at paragraph level: flush the text before
/// it (minus the soft break or space leading up to the formula) and push the
/// formula as its own block. `false` (nothing consumed) for any other event.
fn take_display_math(
    cur: &mut Cursor,
    out: &mut Vec<Block>,
    acc: &mut Vec<InlineRun>,
    after_math: &mut bool,
) -> bool {
    let Some(Event::DisplayMath(tex)) = cur.peek_event() else {
        return false;
    };
    let tex = tex.trim().to_string();
    cur.bump();
    trim_runs_end(acc);
    if *after_math {
        trim_runs_start(acc);
    }
    if !acc.is_empty() {
        out.push(Block::Paragraph {
            runs: autolink_runs(merge_runs(std::mem::take(acc))),
        });
    }
    out.push(Block::Math { tex });
    *after_math = true;
    true
}

/// Plain text whose edge whitespace is layout, not content: code, math,
/// images and task markers keep their spelling.
fn is_plain(run: &InlineRun) -> bool {
    !run.style.code && !run.style.math && run.style.image.is_none() && run.style.task.is_none()
}

fn trim_runs_end(runs: &mut Vec<InlineRun>) {
    while let Some(last) = runs.last_mut().filter(|run| is_plain(run)) {
        last.text.truncate(last.text.trim_end().len());
        if !last.text.is_empty() {
            break;
        }
        runs.pop();
    }
}

fn trim_runs_start(runs: &mut Vec<InlineRun>) {
    let mut drop = 0;
    for run in runs.iter_mut() {
        if !is_plain(run) {
            break;
        }
        let trimmed = run.text.len() - run.text.trim_start().len();
        run.text.drain(..trimmed);
        if !run.text.is_empty() {
            break;
        }
        drop += 1;
    }
    runs.drain(..drop);
}

fn parse_table(cur: &mut Cursor, align: Vec<TableAlign>) -> Block {
    let mut header = Vec::new();
    let mut rows = Vec::new();
    loop {
        match cur.peek_event() {
            Some(Event::Start(Tag::TableHead)) => {
                cur.bump();
                header = parse_table_cells(cur);
            }
            Some(Event::Start(Tag::TableRow)) => {
                cur.bump();
                rows.push(parse_table_cells(cur));
            }
            Some(Event::End(_)) | None => {
                cur.bump();
                break;
            }
            Some(_) => cur.bump(),
        }
    }
    Block::Table {
        header,
        rows,
        align,
    }
}

fn parse_table_cells(cur: &mut Cursor) -> Vec<Vec<InlineRun>> {
    let mut cells = Vec::new();
    loop {
        match cur.peek_event() {
            Some(Event::Start(Tag::TableCell)) => {
                cur.bump();
                cells.push(parse_inline_container(cur, &InlineStyle::default()));
            }
            Some(Event::End(_)) | None => {
                cur.bump();
                break;
            }
            Some(_) => cur.bump(),
        }
    }
    cells
}

/// Parse inline events until the container's `End` (consumed).
fn parse_inline_container(cur: &mut Cursor, style: &InlineStyle) -> Vec<InlineRun> {
    let mut runs = Vec::new();
    while let Some(event) = cur.peek_event() {
        if matches!(event, Event::End(_)) {
            cur.bump();
            break;
        }
        parse_inline_event(cur, &mut runs, style);
    }
    // Autolink AFTER merging: pulldown splits Text events at would-be
    // emphasis chars ("…/Foo_(bar)" arrives as three events), so scanning
    // per-event would truncate URLs at every underscore.
    autolink_runs(merge_runs(runs))
}

fn parse_inline_event(cur: &mut Cursor, runs: &mut Vec<InlineRun>, style: &InlineStyle) {
    let range = cur.peek().map(|(_, range)| range.clone());
    let Some(event) = cur.next_event() else {
        return;
    };
    let push = |runs: &mut Vec<InlineRun>, text: String, style: InlineStyle| {
        if !text.is_empty() {
            runs.push(InlineRun { text, style });
        }
    };
    match event {
        Event::Text(t) => push(runs, t.into_string(), style.clone()),
        Event::Code(t) => {
            let mut s = style.clone();
            s.code = true;
            push(runs, t.into_string(), s);
        }
        Event::InlineMath(tex) => {
            let range = range.unwrap();
            let (spelling, next) = cur.spelling(&range);
            if spelling.starts_with('$') && not_math(&tex, next) {
                push(runs, spelling.to_string(), style.clone());
            } else {
                let mut s = style.clone();
                s.math = true;
                push(runs, tex.trim().to_string(), s);
            }
        }
        // Display math reaching here sits in a heading, a table cell or a
        // styled span — places that hold runs, so it renders inline.
        Event::DisplayMath(tex) => {
            let mut s = style.clone();
            s.math = true;
            push(runs, tex.trim().to_string(), s);
        }
        Event::SoftBreak => push(runs, " ".into(), style.clone()),
        Event::HardBreak => push(runs, "\n".into(), style.clone()),
        Event::Html(t) | Event::InlineHtml(t) => push(runs, t.into_string(), style.clone()),
        Event::TaskListMarker(done) => {
            let mut style = style.clone();
            style.task = Some(TaskMarker {
                checked: done,
                range: range.unwrap(),
            });
            push(
                runs,
                if done { "[x] ".into() } else { "[ ] ".into() },
                style,
            );
        }
        Event::FootnoteReference(t) => push(runs, format!("[{t}]"), style.clone()),
        Event::Start(tag) => {
            let mut inner = style.clone();
            match tag {
                Tag::Emphasis => inner.italic = true,
                Tag::Strong => inner.bold = true,
                Tag::Strikethrough => inner.strikethrough = true,
                Tag::Image {
                    dest_url, title, ..
                } => {
                    // Alt text is plain: a formula in it keeps its `$…$`.
                    let alt: String = parse_inline_container(cur, style)
                        .iter()
                        .map(|run| {
                            if run.style.math {
                                format!("${}$", run.text)
                            } else {
                                run.text.clone()
                            }
                        })
                        .collect();
                    inner.image = Some(InlineImage {
                        source: dest_url.to_string(),
                        alt: alt.clone(),
                        title: title.to_string(),
                        link: style.link.clone(),
                    });
                    inner.link = Some(dest_url.into_string());
                    runs.push(InlineRun {
                        text: alt,
                        style: inner,
                    });
                    return;
                }
                Tag::Link { dest_url, .. } => {
                    inner.link = Some(dest_url.into_string());
                }
                _ => {}
            }
            runs.extend(parse_inline_container(cur, &inner));
        }
        // `End` is consumed by the container loop; anything else is ignored.
        _ => {}
    }
}

/// Whether a `$…$` span (`tex` between the dollars, `next` the byte after
/// the closer) is really prose that happens to hold two dollar signs:
/// - Pandoc's currency rule: a closer glued to a digit is a price (`$5-$10`);
/// - a span ending in `/`, `:` or `@` joins shell variables
///   (`$HOME/$USER`, `PATH=$PATH:$HOME`, `$USER@$HOST`) — no formula ends so;
/// - an all-caps identifier whose closer runs into another identifier is a
///   shell variable (`$FOO$BAR`), while `$AB$` or `$n$th` stay math;
/// - TeX has no use for a backtick: one inside belongs to a code span the
///   dollars straddle (`$HOME and `$PATH``); nor for a `"` (`"$HOME" and
///   "$USER"`);
/// - shell and regex shapes: a trailing `,`/`;` (`array($a,$b)`), a bracket
///   closed before opened (`($foo) or ($bar)`), a braced name (`${A}${B}`),
///   an anchor (`/x$/i`, `'a$|b$'`), an amount then words (`$10k, or 5$`).
fn not_math(tex: &str, next: Option<u8>) -> bool {
    let shell_name = tex.len() > 1
        && tex
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
        && tex.as_bytes()[0].is_ascii_uppercase();
    // `${PREFIX}${NAME}`, `${arr[0]}`: a braced shell expansion; `$(CC)`: a
    // Make variable.
    let wrapped = |open: char, close: char, allowed: &dyn Fn(u8) -> bool| {
        tex.len() >= 3
            && tex.starts_with(open)
            && tex.ends_with(close)
            && tex[1..tex.len() - 1].bytes().all(allowed)
    };
    let braced = wrapped('{', '}', &|b| {
        b.is_ascii_alphanumeric() || b"_[].".contains(&b)
    });
    let make = wrapped('(', ')', &|b| {
        b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_'
    });
    // A sign raised or lowered onto what precedes it: `\mathbb{Z}^+`.
    let script_sign = {
        let mut before = tex.chars();
        before.next_back();
        before.as_str().ends_with(['^', '_', '{'])
    };
    // `($foo) or ($bar)`: a bracket closed before any opened.
    let mut depth = 0i32;
    let unbalanced = tex.bytes().any(|b| {
        match b {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            _ => {}
        }
        depth < 0
    });
    // `$10k, or 5$`: an amount followed by words, with no TeX in it.
    let amount = tex.starts_with(|c: char| c.is_ascii_digit())
        && !tex.bytes().any(|b| b"\\^_{}=".contains(&b))
        && tex
            .split(|c: char| !c.is_ascii_alphabetic())
            .any(|word| word.len() >= 2);
    next.is_some_and(|b| b.is_ascii_digit())
        || tex.ends_with(['/', ':', '@', ',', ';'])
        || tex.contains(['`', '"'])
        || (shell_name && next.is_some_and(|b| b.is_ascii_uppercase() || b == b'_'))
        || braced
        || make
        || unbalanced
        || amount
        // Markdown, not TeX: emphasis runs (`**bold $x** y$`).
        || tex.contains("**")
        || tex.contains("__")
        // Regex anchors: `/x$/i or /y$/g`, `grep 'foo$' a && grep 'bar$'`,
        // `'a$|b$'` (a bar-led span with no space, closed by a quote — not
        // `$|x - y| < \delta$`).
        || tex.starts_with(['/', '\''])
        || (tex.starts_with('|')
            && !tex.contains(' ')
            && next.is_some_and(|b| b == b'\'' || b == b'"'))
        // Separators after a sigil (RxJS `click$, hover$`, `5$, 10$`).
        || tex.starts_with([',', ';', ':', ')'])
        // A dangling operator: `x=$a y=$b`, `$i<$n`, `$obj->$prop` — not a
        // sign in a script (`\mathbb{Z}^+`, `Na$^+$`).
        || tex.ends_with(['=', '.'])
        || (tex.ends_with(['-', '<', '>', '*', '/'])
            && !script_sign
            && next.is_some_and(|b| b.is_ascii_alphabetic()))
        || (tex.ends_with('+') && !script_sign)
}

/// Promote bare `http(s)://` URLs into link runs — GFM's autolink extension,
/// which pulldown-cmark has no option for (agents paste naked PR/issue URLs
/// constantly; user report: the link isn't clickable). Runs already inside a
/// link, code span or formula pass through untouched. Idempotent, so nested containers
/// re-applying it on their merged output is harmless.
fn autolink_runs(runs: Vec<InlineRun>) -> Vec<InlineRun> {
    let mut out = Vec::with_capacity(runs.len());
    for run in runs {
        if run.style.link.is_some() || run.style.code || run.style.math {
            out.push(run);
        } else {
            push_text_autolinked(&mut out, &run.text, &run.style);
        }
    }
    out
}

fn push_text_autolinked(runs: &mut Vec<InlineRun>, text: &str, style: &InlineStyle) {
    let push = |runs: &mut Vec<InlineRun>, text: &str, style: InlineStyle| {
        if !text.is_empty() {
            runs.push(InlineRun {
                text: text.to_string(),
                style,
            });
        }
    };
    let mut rest = text;
    while let Some(at) = find_url_start(rest) {
        let from = &rest[at..];
        let scheme = if from.starts_with("https://") {
            "https://".len()
        } else {
            "http://".len()
        };
        let len = bare_url_len(from);
        if len <= scheme {
            // A scheme with nothing after it stays text (don't re-find it).
            push(runs, &rest[..at + scheme], style.clone());
            rest = &from[scheme..];
            continue;
        }
        push(runs, &rest[..at], style.clone());
        let mut linked = style.clone();
        linked.link = Some(from[..len].to_string());
        push(runs, &from[..len], linked);
        rest = &from[len..];
    }
    push(runs, rest, style.clone());
}

/// First viable `http(s)://` occurrence: not glued to a preceding
/// alphanumeric (`foohttps://…` stays text, per GFM's boundary rule).
fn find_url_start(text: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(rel) = text[from..].find("http") {
        let at = from + rel;
        let after = &text[at..];
        let is_scheme = after.starts_with("http://") || after.starts_with("https://");
        let boundary = text[..at]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric());
        if is_scheme && boundary {
            return Some(at);
        }
        from = at + "http".len();
    }
    None
}

/// Byte length of the bare URL at the start of `text`: run to whitespace (or
/// a delimiter that never appears in pasted URLs), then trim the trailing
/// punctuation GFM excludes — a closing paren only stays when an opener
/// inside the URL balances it ("…/Foo_(bar))" keeps one, sheds one).
fn bare_url_len(text: &str) -> usize {
    let end = text
        .char_indices()
        .find(|(_, c)| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '\'' | '`'))
        .map_or(text.len(), |(i, _)| i);
    let mut url = &text[..end];
    // Parens counted once and kept current as the tail is trimmed: counting
    // them per trimmed char was quadratic in a run of `)`.
    let (mut open, mut close) = (url.matches('(').count(), url.matches(')').count());
    while let Some(last) = url.chars().next_back() {
        let trim = match last {
            '.' | ',' | ';' | ':' | '!' | '?' | '*' | '_' | '~' => true,
            ')' => open < close,
            _ => false,
        };
        if !trim {
            break;
        }
        match last {
            '(' => open -= 1,
            ')' => close -= 1,
            _ => {}
        }
        url = &url[..url.len() - last.len_utf8()];
    }
    url.len()
}

/// Merge adjacent identically-styled runs (keeps run counts small and makes the
/// tree canonical for equality tests). Images and math are atoms: two formulas
/// side by side stay two formulas.
fn merge_runs(runs: Vec<InlineRun>) -> Vec<InlineRun> {
    let mut out: Vec<InlineRun> = Vec::with_capacity(runs.len());
    for run in runs {
        match out.last_mut() {
            Some(last)
                if last.style == run.style && run.style.image.is_none() && !run.style.math =>
            {
                last.text.push_str(&run.text)
            }
            _ => out.push(run),
        }
    }
    out
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

// ---------------------------------------------------------------------------
// Incremental parse
// ---------------------------------------------------------------------------

/// Streaming parser: appends reparse only from the last stable top-level block
/// boundary (snapped back to a line start so indentation context survives).
#[derive(Debug, Default)]
pub struct IncrementalParser {
    source: String,
    tree: BlockTree,
    /// Display-only replacement for the last top-level block when its source
    /// has hanging inline markers ([`crate::mend`]): `None` means the display
    /// tree is exactly [`Self::tree`]. Never fed back into the incremental
    /// state — the canonical tree stays parity-exact with `parse_full`.
    display_tail: Option<Vec<Arc<TopBlock>>>,
    /// Index of the first canonical block the display tail replaces: the last
    /// block and any siblings sharing its range (a paragraph split around
    /// display math reparses as a unit).
    display_from: usize,
    /// Link-reference definitions act at a distance — full reparses only.
    full_only: bool,
    /// Bytes fed through `parse_full` by the most recent `set_text`/`append`/
    /// `reset` — instrumentation proving per-append work is O(tail), not
    /// O(total). 0 for a no-op set_text.
    last_parse_bytes: usize,
    /// Number of leading top-level blocks guaranteed untouched by the most
    /// recent update (render caches for these blocks stay valid).
    stable_prefix_blocks: usize,
}

impl IncrementalParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn tree(&self) -> &BlockTree {
        &self.tree
    }

    /// The tree to render while streaming: the canonical tree with the last
    /// block swapped for its mended parse when inline markers hang (an
    /// unclosed `**bold`, a half-streamed `[link](url…`). Same shape and cost
    /// as `tree().clone()` — the stable prefix shares its blocks; only a
    /// hanging tail adds one O(tail) reparse, done at append time.
    pub fn display_tree(&self) -> BlockTree {
        let Some(tail) = &self.display_tail else {
            return self.tree.clone();
        };
        let stable = &self.tree.blocks[..self.display_from];
        let mut blocks = Vec::with_capacity(stable.len() + tail.len());
        blocks.extend_from_slice(stable);
        blocks.extend_from_slice(tail);
        BlockTree { blocks }
    }

    /// Bytes actually reparsed by the last update (see field docs).
    pub fn last_parse_bytes(&self) -> usize {
        self.last_parse_bytes
    }

    /// Leading top-level blocks left untouched by the last update.
    pub fn stable_prefix_blocks(&self) -> usize {
        self.stable_prefix_blocks
    }

    /// Set the source: appends take the incremental path, anything else resets.
    pub fn set_text(&mut self, text: &str) {
        if text.len() >= self.source.len() && text.starts_with(self.source.as_str()) {
            let delta = &text[self.source.len()..];
            if delta.is_empty() {
                self.last_parse_bytes = 0;
                self.stable_prefix_blocks = self.tree.blocks.len();
                return;
            }
            self.append(delta);
        } else {
            self.reset(text);
        }
    }

    pub fn reset(&mut self, text: &str) {
        self.source = text.to_string();
        self.full_only = has_link_defs(text);
        self.tree = parse_full(text);
        self.last_parse_bytes = text.len();
        self.stable_prefix_blocks = 0;
        self.remend();
    }

    /// Append streamed text, reparsing from the last stable boundary.
    pub fn append(&mut self, delta: &str) {
        if delta.is_empty() {
            self.last_parse_bytes = 0;
            self.stable_prefix_blocks = self.tree.blocks.len();
            return;
        }
        // The delta may complete a line begun earlier — rescan from that line's
        // start when checking for definitions.
        let scan_from = self.source.rfind('\n').map(|i| i + 1).unwrap_or(0);
        self.source.push_str(delta);
        if !self.full_only && has_link_defs(&self.source[scan_from..]) {
            self.full_only = true;
        }
        if self.full_only {
            self.tree = parse_full(&self.source);
            self.last_parse_bytes = self.source.len();
            self.stable_prefix_blocks = 0;
            self.remend();
            return;
        }

        // Stable boundary: start of the SECOND-to-last top-level block, snapped
        // back to its line start (keeps indented-code / fenced-indent context
        // intact). Reparsing the last two blocks — not just the last — covers
        // continuation merges: a trailing paragraph like `3` can become `3.`
        // and fuse into the preceding loose list. Merges cannot cascade
        // further back (a block's separation from its predecessor is decided
        // by its own already-streamed leading bytes), so two blocks suffice;
        // the parity tests stream corpora to hold this invariant.
        // Blocks split from one source block (a paragraph around display
        // math) share its range and count as one here.
        let last_start = self.tree.blocks.last().map_or(0, |b| b.range.start);
        let mut boundary = self
            .tree
            .blocks
            .iter()
            .rev()
            .map(|b| b.range.start)
            .find(|&start| start < last_start)
            .unwrap_or(0);
        // Lazy continuation reaches one block further: a paragraph (or
        // table, heading) right under a quote or list, with no blank line
        // between, can still fold back into it once later text changes its
        // reading — `>a\n|b\n>|-c` streams through a table that splits `|b`
        // out of the quote, and `c` folds all three back into one. Reparse
        // from that container too. One step only: the container's own extent
        // was settled while it was in the window, and a run of containers
        // (`- a\n* b\n…`) never re-merges, so this stays O(tail).
        let blocks = &self.tree.blocks;
        if let Some(at) = blocks.iter().position(|b| b.range.start == boundary)
            && at > 0
            && !matches!(
                blocks[at].block,
                Block::BlockQuote { .. } | Block::List { .. }
            )
            && matches!(
                blocks[at - 1].block,
                Block::BlockQuote { .. } | Block::List { .. }
            )
        {
            let above = blocks[at - 1].range.start;
            let between = &self.source[above..boundary];
            if between[between.trim_end().len()..].matches('\n').count() < 2 {
                boundary = above;
            }
        }
        let boundary = self.source[..boundary]
            .rfind('\n')
            .map(|i| i + 1)
            .unwrap_or(0);

        let tail = parse_at(&self.source[boundary..], boundary);
        self.last_parse_bytes = self.source.len() - boundary;
        self.tree.blocks.retain(|b| b.range.start < boundary);
        self.stable_prefix_blocks = self.tree.blocks.len();
        for top in tail.blocks {
            self.tree.blocks.push(top);
        }
        self.remend();
    }

    /// Recompute the display tail: mend hanging inline markers in the last
    /// top-level block (only place they can hang — a blank line settles a
    /// block, and CommonMark keeps unclosed markers literal across it) and
    /// reparse just that block's source. `close_hanging` is one O(last block)
    /// scan and returns `None` when nothing hangs, so the extra parse happens
    /// only while a marker is actually open.
    fn remend(&mut self) {
        self.display_tail = None;
        let Some(last) = self.tree.blocks.last() else {
            return;
        };
        // Code blocks render an unclosed fence verbatim (already stable);
        // rules, tables and a paragraph ending in display math have no inline
        // tail to mend.
        if matches!(
            last.block,
            Block::CodeBlock { .. } | Block::Rule | Block::Table { .. } | Block::Math { .. }
        ) {
            return;
        }
        let start = last.range.start;
        let tail = &self.source[start..];
        let mut mended = crate::mend::close_hanging(tail);
        // A trailing lone `$` may yet be a price; keep it literal while it
        // would close a formula in the parse (`$5 to $`). In code or raw HTML
        // it is plain text already, and stays as written.
        // Only a block that holds a formula can have its trailing `$` close one.
        let has_formula = self
            .tree
            .blocks
            .iter()
            .rev()
            .take_while(|block| block.range.start == start)
            .any(|block| formulas(std::slice::from_ref(block)) > 0);
        if let Some(escaped) = has_formula
            .then(|| crate::mend::close_hanging_dollar(tail))
            .flatten()
        {
            let plain = mended.as_deref().unwrap_or(tail);
            self.last_parse_bytes += escaped.len() + plain.len();
            if formulas(&parse_at(&escaped, start).blocks)
                < formulas(&parse_at(plain, start).blocks)
            {
                mended = Some(escaped);
            }
        }
        let Some(mended) = mended else {
            return;
        };
        // Count toward the O(tail) instrumentation — this is real parse work,
        // in the same bound as the reparse that produced the block.
        self.last_parse_bytes += mended.len();
        let mut tail = parse_at(&mended, start).blocks;
        for top in &mut tail {
            let top = Arc::make_mut(top);
            // Display ranges point back into the unmended source; synthetic
            // closers at the end clamp away — onto a char boundary.
            // (A synthetic block past the end — the setext guard's zero-width
            // space — gets an empty range at the end.)
            let clamp = |at: usize| {
                let mut at = at.min(self.source.len());
                while !self.source.is_char_boundary(at) {
                    at -= 1;
                }
                at
            };
            let start = clamp(top.range.start);
            top.range = start..clamp(top.range.end).max(start);
        }
        self.display_from = self
            .tree
            .blocks
            .iter()
            .rposition(|b| b.range.start != start)
            .map_or(0, |i| i + 1);
        self.display_tail = Some(tail);
    }
}

/// How many formulas (inline runs and display blocks) `blocks` hold.
fn formulas(blocks: &[Arc<TopBlock>]) -> usize {
    fn runs(runs: &[InlineRun]) -> usize {
        runs.iter().filter(|run| run.style.math).count()
    }
    fn in_block(block: &Block) -> usize {
        match block {
            Block::Paragraph { runs: r } | Block::Heading { runs: r, .. } => runs(r),
            Block::BlockQuote { children } => children.iter().map(in_block).sum(),
            Block::List { items, .. } => items.iter().flatten().map(in_block).sum(),
            Block::Table { header, rows, .. } => {
                header.iter().map(|cell| runs(cell)).sum::<usize>()
                    + rows.iter().flatten().map(|cell| runs(cell)).sum::<usize>()
            }
            Block::Math { .. } => 1,
            Block::CodeBlock { .. } | Block::Rule => 0,
        }
    }
    blocks.iter().map(|top| in_block(&top.block)).sum()
}

/// Conservative detector for link-reference-definition lines
/// (`[label]: destination`), at top level or inside block quotes and list
/// items (`> [label]: …`, `- [label]: …`).
fn has_link_defs(text: &str) -> bool {
    text.lines().any(|line| {
        let line = strip_container_markers(line);
        // `[label]: …`, or the end of a label begun on an earlier line
        // (`[a\nb]: /u`).
        line.find("]:")
            .is_some_and(|at| line.starts_with('[') || !line[..at].contains('['))
    })
}

/// A line without its leading indentation and block-quote and list-item
/// markers, however deeply nested.
fn strip_container_markers(mut line: &str) -> &str {
    loop {
        line = line.trim_start_matches([' ', '\t']);
        let bytes = line.as_bytes();
        let digits = bytes
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        let marker = match bytes.first() {
            Some(b'>') => 1,
            Some(b'-' | b'*' | b'+') if matches!(bytes.get(1), Some(b' ' | b'\t')) => 1,
            Some(b'0'..=b'9')
                if digits <= 9
                    && matches!(bytes.get(digits), Some(b'.' | b')'))
                    && matches!(bytes.get(digits + 1), Some(b' ' | b'\t')) =>
            {
                digits + 1
            }
            _ => return line,
        };
        line = &line[marker..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_ranges_survive_nested_lists_unicode_crlf_and_streaming() {
        let source = "Título 🦀\r\n\r\nIntro\r\n\r\n- [ ] repetida\r\n  - [X] repetida\r\n\r\n> - [x] citada\r\n\r\n```md\r\n- [ ] literal\r\n```\r\n";
        fn collect(block: &Block, tasks: &mut Vec<TaskMarker>) {
            match block {
                Block::Paragraph { runs } => {
                    tasks.extend(runs.iter().filter_map(|run| run.style.task.clone()))
                }
                Block::List { items, .. } => items
                    .iter()
                    .flatten()
                    .for_each(|block| collect(block, tasks)),
                Block::BlockQuote { children } => {
                    children.iter().for_each(|block| collect(block, tasks))
                }
                _ => {}
            }
        }
        let tree = parse_full(source);
        let mut tasks = Vec::new();
        for top in &tree.blocks {
            collect(&top.block, &mut tasks);
        }
        assert_eq!(tasks.len(), 3);
        assert_eq!(
            tasks
                .iter()
                .map(|task| &source[task.range.clone()])
                .collect::<Vec<_>>(),
            ["[ ]", "[X]", "[x]"]
        );
        let mut stream = IncrementalParser::new();
        for ch in source.chars() {
            stream.append(&ch.to_string());
            assert_eq!(stream.tree(), &parse_full(stream.source()));
        }
    }

    #[test]
    fn display_snapshots_share_stable_blocks_without_mutating_old_frames() {
        let mut parser = IncrementalParser::new();
        parser.set_text("first **bold** paragraph\n\nsecond paragraph\n\nlast **open");
        let before = parser.display_tree();
        let frozen = format!("{before:?}");
        assert!(Arc::ptr_eq(&before.blocks[0], &parser.tree().blocks[0]));
        parser.append(" tail**\n\nnext paragraph");
        let after = parser.display_tree();
        assert!(Arc::ptr_eq(&before.blocks[0], &after.blocks[0]));
        assert_eq!(
            format!("{before:?}"),
            frozen,
            "old paint snapshot is immutable"
        );
        assert_eq!(parser.tree(), &parse_full(parser.source()));
        parser.reset("replacement");
        assert_eq!(
            format!("{before:?}"),
            frozen,
            "reset cannot alter an old frame"
        );
        assert_eq!(parser.tree(), &parse_full("replacement"));
    }

    fn stream(chunks: usize, text: &str) -> IncrementalParser {
        let mut p = IncrementalParser::new();
        let bytes = text.as_bytes();
        let mut start = 0;
        while start < bytes.len() {
            let mut end = (start + chunks).min(bytes.len());
            while end < bytes.len() && !text.is_char_boundary(end) {
                end += 1;
            }
            p.append(&text[start..end]);
            start = end;
        }
        p
    }

    const CORPORA: &[&str] = &[
        "# Title\n\nHello **bold** and *italic* and `code` and ~~gone~~.\n",
        "Paragraph one\nlazy continuation\n\nParagraph two with a [link](https://x.dev).\n",
        "- item one\n- item two\n  - nested a\n  - nested b\n- item three\n\ntail\n",
        "1. first\n2. second\n\n   loose paragraph in item\n\n3. third\n",
        "```rust\nfn main() {\n    println!(\"hi\");\n}\n```\n\nafter code\n",
        "intro\n\n```\nunclosed fence streaming",
        "> quoted line\n> more quote\n>\n> - a list in a quote\n\nplain\n",
        "| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n\ndone\n",
        "setext candidate\n===\n\nnext para\n---\n",
        "***\n\ntext between rules\n\n---\n",
        "- [x] done task\n- [ ] open task\n",
        "    indented code line one\n    line two\n\npara\n",
        "see [it's here](/tmp/2026/Some Folder/it's here.txt) and `[raw](/tmp/a b.md)`\n",
        "text\n\n```\n[x](/tmp/a b.md)\n```\n\nafter [y](file:///tmp/c d.md)\n",
        "para with <span>inline html</span> inside\n\n<div>\nblock html\n</div>\n",
        "###### deep heading\n\n#### h4\n",
        "Euler: $e^{i\\pi} + 1 = 0$ and $a_1 * b_2$.\n\n$$\n\\sum_{k=1}^n k = \\frac{n(n+1)}{2}\n$$\n\nafter\n",
        "Before\n$$x^2$$\nmiddle **bold** $$y$$ tail\n\n- item $$z$$\n- $a$\n\n> $$q$$\n",
        "Prose \\(a^2 + b^2\\) here.\n\n\\[\n\\int_0^1 f(x)\\,dx\n\\]\n\nand `\\(code\\)` then \\[ e = mc^2 \\] end\n",
        "costs $5-$10 or between $5 and $10, but $x$ is math\n\n# Heading $$h$$\n\n| m |\n|---|\n| $$c$$ |\n",
        "# T \\(\nx \\) y\npara \\[\n- item \\]\n\n> \\[\n> q_1\n> \\]\n\ntext\n```\n\\(raw\\)\n```\n",
    ];

    #[test]
    fn incremental_matches_full_on_streamed_corpora() {
        for (ci, corpus) in CORPORA.iter().enumerate() {
            let full = parse_full(corpus);
            for chunk in [1usize, 2, 3, 7, 16, 64] {
                let inc = stream(chunk, corpus);
                assert_eq!(
                    inc.tree(),
                    &full,
                    "corpus {ci} diverged at chunk size {chunk}:\n{corpus}"
                );
            }
        }
    }

    /// Random documents over the characters the pre-passes and math rules
    /// key on, streamed in every chunk size: the canonical tree must equal a
    /// one-shot parse, and the display tree a one-shot `set_text`'s.
    #[test]
    fn incremental_matches_full_on_random_documents() {
        const PIECES: &[&str] = &[
            "\\", "(", ")", "[", "]", "$", "$$", "`", "```", "-", "=", "|", "#", ">", "*", "_",
            "\n", "\n\n", " ", " ", "é", "漢", "x", "y", "1", ":", "/", "**", "\\(", "\\)", "\\[",
            "\\]", "|---|", "](u)", "~~", "\n> ", "\n- ", "\n1. ", "\n    ", "\t", "<a:b>", "<p>",
            "$5", "$HOME", "@", "'", ":-:",
        ];
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut failures = Vec::new();
        for case in 0..8000 {
            let len = 8 + (next() % 56) as usize;
            let doc: String = (0..len)
                .map(|_| PIECES[(next() % PIECES.len() as u64) as usize])
                .collect();
            let full = parse_full(&doc);
            let mut settled = IncrementalParser::new();
            settled.set_text(&doc);
            let chunk = 1 + case % 17;
            let streamed = stream(chunk, &doc);
            if streamed.tree() != &full || streamed.display_tree() != settled.display_tree() {
                failures.push((chunk, doc));
            }
        }
        assert!(
            failures.is_empty(),
            "{} diverged: {failures:#?}",
            failures.len()
        );
    }

    #[test]
    fn appends_keep_committed_blocks_identical() {
        // Streaming stability invariant: blocks before the reparse boundary
        // (everything but the last two source blocks — pieces of a paragraph
        // split around display math share a range and count as one) must be
        // reused as-is across appends — same index, same value — so
        // row/element keys never re-mount and earlier blocks can never
        // visibly reflow.
        for corpus in CORPORA {
            let mut p = IncrementalParser::new();
            let mut prev = p.tree().clone();
            let bytes = corpus.as_bytes();
            let mut start = 0;
            while start < bytes.len() {
                let mut end = (start + 3).min(bytes.len());
                while end < bytes.len() && !corpus.is_char_boundary(end) {
                    end += 1;
                }
                p.append(&corpus[start..end]);
                start = end;

                let cur = p.tree();
                let mut starts: Vec<usize> = prev.blocks.iter().map(|b| b.range.start).collect();
                starts.dedup();
                let committed = match starts.len() {
                    0..=2 => 0,
                    n => prev
                        .blocks
                        .partition_point(|b| b.range.start < starts[n - 2]),
                };
                assert!(
                    cur.blocks.len() >= committed,
                    "committed blocks disappeared:\n{corpus}"
                );
                for i in 0..committed {
                    assert_eq!(
                        cur.blocks[i], prev.blocks[i],
                        "block {i} changed across an append:\n{corpus}"
                    );
                }
                prev = cur.clone();
            }
        }
    }

    #[test]
    fn incremental_matches_full_with_link_definitions() {
        // Definitions act at a distance → parser falls back to full reparses,
        // so parity must still hold.
        let corpus = "See [docs] for more.\n\nMore text.\n\n[docs]: https://example.com\n";
        let full = parse_full(corpus);
        for chunk in [1usize, 3, 9] {
            assert_eq!(stream(chunk, corpus).tree(), &full, "chunk {chunk}");
        }
        // The reference actually resolved into a link.
        let has_link = full.blocks.iter().any(|b| match &b.block {
            Block::Paragraph { runs } => runs.iter().any(|r| r.style.link.is_some()),
            _ => false,
        });
        assert!(has_link, "expected [docs] to resolve to a link");
    }

    #[test]
    fn set_text_appends_or_resets() {
        let mut p = IncrementalParser::new();
        p.set_text("hello");
        p.set_text("hello world");
        assert_eq!(p.tree(), &parse_full("hello world"));
        // Non-append rewrites reset cleanly.
        p.set_text("different");
        assert_eq!(p.tree(), &parse_full("different"));
        assert_eq!(p.source(), "different");
    }

    #[test]
    fn block_structure_basics() {
        let tree = parse_full("## Head\n\npara **b _bi_** text\n\n```ts\nlet x = 1;\n```\n");
        assert_eq!(tree.len(), 3);
        match &tree.blocks[0].block {
            Block::Heading { level, runs } => {
                assert_eq!(*level, 2);
                assert_eq!(runs[0].text, "Head");
            }
            other => panic!("unexpected {other:?}"),
        }
        match &tree.blocks[1].block {
            Block::Paragraph { runs } => {
                assert_eq!(runs.len(), 4); // "para ", "b ", "bi" (bold+italic), " text"
                assert!(runs[1].style.bold && !runs[1].style.italic);
                assert!(runs[2].style.bold && runs[2].style.italic);
            }
            other => panic!("unexpected {other:?}"),
        }
        match &tree.blocks[2].block {
            Block::CodeBlock { language, code } => {
                assert_eq!(language.as_deref(), Some("ts"));
                assert_eq!(code, "let x = 1;");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn nested_lists_and_tight_items() {
        let tree = parse_full("- a\n  - a1\n  - a2\n- b\n");
        let Block::List {
            ordered_start,
            items,
        } = &tree.blocks[0].block
        else {
            panic!("expected list");
        };
        assert_eq!(*ordered_start, None);
        assert_eq!(items.len(), 2);
        // Tight item text became an implicit paragraph, nested list follows.
        assert!(matches!(items[0][0], Block::Paragraph { .. }));
        assert!(matches!(items[0][1], Block::List { .. }));
    }

    #[test]
    fn tables_parse_header_and_rows() {
        let tree = parse_full("| a | b |\n|---|---|\n| 1 | 2 |\n");
        let Block::Table {
            header,
            rows,
            align,
        } = &tree.blocks[0].block
        else {
            panic!("expected table");
        };
        assert_eq!(header.len(), 2);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][1][0].text, "2");
        assert_eq!(align, &vec![TableAlign::Left, TableAlign::Left]);
    }

    #[test]
    fn tables_parse_column_alignment() {
        let tree = parse_full("| a | b | c |\n|:--|:-:|--:|\n| 1 | 2 | 3 |\n");
        let Block::Table { align, .. } = &tree.blocks[0].block else {
            panic!("expected table");
        };
        assert_eq!(
            align,
            &vec![TableAlign::Left, TableAlign::Center, TableAlign::Right]
        );
    }

    #[test]
    fn links_carry_urls() {
        let tree = parse_full("go to [zed](https://zed.dev) now\n");
        let Block::Paragraph { runs } = &tree.blocks[0].block else {
            panic!()
        };
        let link = runs
            .iter()
            .find(|r| r.style.link.is_some())
            .expect("link run");
        assert_eq!(link.text, "zed");
        assert_eq!(link.style.link.as_deref(), Some("https://zed.dev"));
    }

    /// The paragraph's single link run: (text, url).
    fn only_link(source: &str) -> Option<(String, String)> {
        let tree = parse_full(source);
        let Block::Paragraph { runs } = &tree.blocks[0].block else {
            panic!()
        };
        let links: Vec<_> = runs
            .iter()
            .filter_map(|r| Some((r.text.clone(), r.style.link.clone()?)))
            .collect();
        assert!(links.len() <= 1, "expected at most one link: {links:?}");
        links.into_iter().next()
    }

    /// Bare URLs autolink (the GFM extension pulldown-cmark lacks): the URL
    /// becomes a clickable run, trailing sentence punctuation stays text.
    #[test]
    fn bare_urls_autolink() {
        assert_eq!(
            only_link("PR is updated: https://github.com/zeronsh/comet/pull/31\n"),
            Some((
                "https://github.com/zeronsh/comet/pull/31".into(),
                "https://github.com/zeronsh/comet/pull/31".into()
            ))
        );
        assert_eq!(
            only_link("see https://x.dev/a, then rest.\n").map(|l| l.1),
            Some("https://x.dev/a".into())
        );
        // A wrapping paren is shed; one balanced by an opener in the path stays.
        assert_eq!(
            only_link("(docs: https://x.dev/Foo_(bar))\n").map(|l| l.1),
            Some("https://x.dev/Foo_(bar)".into())
        );
        // Bold text still autolinks, and the run keeps the emphasis.
        let tree = parse_full("**see https://x.dev now**\n");
        let Block::Paragraph { runs } = &tree.blocks[0].block else {
            panic!()
        };
        let link = runs.iter().find(|r| r.style.link.is_some()).unwrap();
        assert!(link.style.bold);
        assert_eq!(link.style.link.as_deref(), Some("https://x.dev"));
    }

    /// Non-links stay text: glued schemes, bare schemes, code spans, and the
    /// destination text of a real markdown link.
    #[test]
    fn autolink_leaves_non_urls_alone() {
        assert_eq!(only_link("foohttps://x.dev is glued\n"), None);
        assert_eq!(only_link("the https:// scheme alone\n"), None);
        assert_eq!(only_link("`https://x.dev` in code\n"), None);
        // A markdown link whose TEXT is a URL keeps the written destination.
        assert_eq!(
            only_link("[https://shown.dev](https://real.dev)\n"),
            Some(("https://shown.dev".into(), "https://real.dev".into()))
        );
    }

    #[test]
    fn top_level_ranges_are_stable_anchors() {
        let src = "first\n\nsecond\n\nthird";
        let tree = parse_full(src);
        assert_eq!(tree.len(), 3);
        assert!(
            tree.blocks
                .windows(2)
                .all(|w| w[0].range.start < w[1].range.start)
        );
        assert_eq!(&src[tree.blocks[1].range.clone()], "second\n");
    }

    /// Concatenated visible text of a block (what the user reads).
    fn flat(block: &Block) -> String {
        fn walk(b: &Block, out: &mut String) {
            match b {
                Block::Paragraph { runs } | Block::Heading { runs, .. } => {
                    for r in runs {
                        out.push_str(&r.text);
                    }
                }
                Block::BlockQuote { children } => children.iter().for_each(|c| walk(c, out)),
                Block::List { items, .. } => items.iter().flatten().for_each(|c| walk(c, out)),
                _ => {}
            }
        }
        let mut s = String::new();
        walk(block, &mut s);
        s
    }

    #[test]
    fn display_tree_styles_hanging_bold_immediately() {
        let mut p = IncrementalParser::new();
        p.set_text("intro **bo");
        let display = p.display_tree();
        let Block::Paragraph { runs } = &display.blocks[0].block else {
            panic!("expected paragraph");
        };
        let bold: Vec<_> = runs.iter().filter(|r| r.style.bold).collect();
        assert_eq!(bold.len(), 1);
        assert_eq!(bold[0].text, "bo");
        assert!(!flat(&display.blocks[0].block).contains("**"));
        // The canonical tree stays honest: literal markers until truly closed.
        assert!(flat(&p.tree().blocks[0].block).contains("**"));
    }

    #[test]
    fn display_tree_converges_to_canonical_when_balanced() {
        let corpus = "a **b** *c* `d` [e](https://x.dev) ~~f~~";
        let mut p = IncrementalParser::new();
        p.set_text(corpus);
        assert_eq!(p.display_tree(), *p.tree());
        assert_eq!(p.tree(), &parse_full(corpus));
    }

    #[test]
    fn display_tree_never_leaks_streaming_urls() {
        let full = "read [docs](https://example.com/long/path) now";
        let mut p = IncrementalParser::new();
        for i in 1..=full.len() {
            if !full.is_char_boundary(i) {
                continue;
            }
            p.set_text(&full[..i]);
            let text = flat(&p.display_tree().blocks[0].block);
            assert!(!text.contains("http"), "url leaked at {i}: {text:?}");
        }
        // Mid-URL the link text carries the pending sentinel destination.
        let mut p = IncrementalParser::new();
        p.set_text("read [docs](https://exa");
        let Block::Paragraph { runs } = &p.display_tree().blocks[0].block else {
            panic!("expected paragraph");
        };
        let link = runs
            .iter()
            .find(|r| r.style.link.is_some())
            .expect("link run");
        assert_eq!(link.text, "docs");
        assert_eq!(
            link.style.link.as_deref(),
            Some(crate::mend::PENDING_LINK_URL)
        );
    }

    #[test]
    fn display_tree_leaves_code_blocks_alone() {
        let mut p = IncrementalParser::new();
        p.set_text("intro\n\n```\nunclosed **fence");
        assert_eq!(p.display_tree(), *p.tree());
    }

    #[test]
    fn display_tree_suppresses_setext_flicker() {
        // "para" + "\n-" parses as an H2 for exactly one chunk before the
        // list item's text arrives; the display tree keeps it a paragraph.
        let mut p = IncrementalParser::new();
        p.set_text("para\n-");
        let display = p.display_tree();
        assert!(
            matches!(
                display.blocks.last().unwrap().block,
                Block::Paragraph { .. }
            ),
            "expected paragraph, got {display:?}"
        );
    }

    #[test]
    fn display_tree_prefix_matches_canonical_across_streams() {
        // Mending swaps only the last block: everything before it must be
        // byte-identical to the canonical tree so render caches and row keys
        // survive.
        for corpus in CORPORA {
            let mut p = IncrementalParser::new();
            let bytes = corpus.as_bytes();
            let mut start = 0;
            while start < bytes.len() {
                let mut end = (start + 3).min(bytes.len());
                while end < bytes.len() && !corpus.is_char_boundary(end) {
                    end += 1;
                }
                p.append(&corpus[start..end]);
                start = end;

                let display = p.display_tree();
                let canonical = p.tree();
                for i in 0..canonical.blocks.len().saturating_sub(1) {
                    assert_eq!(
                        display.blocks[i], canonical.blocks[i],
                        "display prefix diverged:\n{corpus}"
                    );
                }
            }
        }
    }

    #[test]
    fn empty_and_whitespace_sources() {
        assert!(parse_full("").is_empty());
        assert!(parse_full("\n\n  \n").is_empty());
        let mut p = IncrementalParser::new();
        p.append("");
        assert!(p.tree().is_empty());
    }

    /// Lazy continuation lines can fold blocks above the two-block reparse
    /// window back together; streaming must still match a one-shot parse.
    #[test]
    fn lazy_continuations_stream_like_a_full_parse() {
        const PIECES: &[&str] = &[
            ">", "> ", "\n", "\n>", "\n> ", "- ", "\n- ", "1. ", "|", "|-", "-", "a", "é", " ",
            "\\", "#", "\n\n", "*", "[x] ", "`", "b", ":", "=",
        ];
        for source in [
            ">é\n|\\\n>|-é",
            ">a\n|b\n>|-c",
            "- a\n|b\n- |-c",
            "- [x] *\n\nb\n>-",
            "- [x] >\n\na\n\n*\n",
            "- [x] #\n>\n- =",
        ] {
            let full = parse_full(source);
            for chunk in 1..=6 {
                assert_eq!(
                    stream(chunk, source).tree(),
                    &full,
                    "chunk {chunk}: {source:?}"
                );
            }
        }
        let mut seed = 0x51_7cc1_b727_220au64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut failures = Vec::new();
        for case in 0..6000 {
            let len = 4 + (next() % 24) as usize;
            let doc: String = (0..len)
                .map(|_| PIECES[(next() % PIECES.len() as u64) as usize])
                .collect();
            let chunk = 1 + case % 7;
            if stream(chunk, &doc).tree() != &parse_full(&doc) {
                failures.push((chunk, doc));
            }
        }
        assert!(
            failures.is_empty(),
            "{} diverged: {failures:#?}",
            failures.len()
        );
    }
}

#[cfg(test)]
mod closing_quote_blocks {
    use super::*;

    const STORY: &str = "\"How do we negotiate with machines that won't speak?\" someone asked.\n\nYuki almost laughed. \"You don't. You listen to the silence. And you finally understand what it means to be powerless.\"";

    #[test]
    fn full_parse_keeps_trailing_quote_in_block() {
        let tree = parse_full(STORY);
        for b in &tree.blocks {
            eprintln!("block {:?} => {:?}", b.range, &STORY[b.range.clone()]);
        }
        assert_eq!(tree.blocks.len(), 2, "two paragraphs expected");
        let last = &tree.blocks[1];
        assert!(STORY[last.range.clone()].ends_with("powerless.\""));
    }

    #[test]
    fn streamed_boundary_at_quote_adds_no_block() {
        // Stream with a commit boundary exactly between `powerless.` and `"`.
        let split = STORY.len() - 1;
        let mut p = IncrementalParser::new();
        p.set_text(&STORY[..split]);
        p.set_text(STORY);
        let tree = p.tree();
        for b in &tree.blocks {
            eprintln!("block {:?} => {:?}", b.range, &STORY[b.range.clone()]);
        }
        assert_eq!(tree.blocks.len(), 2, "streamed split must not add blocks");
        assert!(STORY[tree.blocks[1].range.clone()].ends_with("powerless.\""));
    }

    #[test]
    fn streamed_small_chunks_match_full_parse() {
        let mut p = IncrementalParser::new();
        let mut fed = String::new();
        for chunk in STORY.as_bytes().chunks(7) {
            fed.push_str(std::str::from_utf8(chunk).unwrap());
            p.set_text(&fed);
        }
        let full = parse_full(STORY);
        assert_eq!(p.tree().blocks.len(), full.blocks.len());
        for (a, b) in p.tree().blocks.iter().zip(full.blocks.iter()) {
            assert_eq!(a.range, b.range);
        }
    }
}

#[cfg(test)]
mod destination_rewrite_tests {
    use super::*;

    fn paragraph(source: &str) -> Vec<InlineRun> {
        let tree = parse_full(source);
        let Some(TopBlock {
            block: Block::Paragraph { runs },
            ..
        }) = tree.blocks.first().map(|top| top.as_ref())
        else {
            panic!("expected a paragraph for {source:?}");
        };
        runs.clone()
    }

    fn link_of(runs: &[InlineRun]) -> Option<&str> {
        runs.iter().find_map(|run| run.style.link.as_deref())
    }

    fn text(runs: &[InlineRun]) -> String {
        runs.iter().map(|run| run.text.as_str()).collect()
    }

    #[test]
    fn absolute_destinations_with_spaces_become_links() {
        let runs = paragraph("see [it's here](/tmp/2026/Some Folder/it's here.txt) now");
        assert_eq!(text(&runs), "see it's here now");
        assert_eq!(link_of(&runs), Some("/tmp/2026/Some Folder/it's here.txt"));

        let runs = paragraph("[x](file:///tmp/a b/c.md)");
        assert_eq!(link_of(&runs), Some("file:///tmp/a b/c.md"));

        let runs = paragraph("shot ![two](/tmp/pics/two words.png) done");
        let image = runs.iter().find_map(|run| run.style.image.as_ref());
        assert_eq!(
            image.map(|image| image.source.as_str()),
            Some("/tmp/pics/two words.png")
        );
    }

    #[test]
    fn non_absolute_or_unsafe_destinations_keep_their_text() {
        for source in [
            // Relative destinations with spaces stay plain text.
            "[x](Some Folder/a.md)",
            // The file-name dot rule applies to plain absolute paths.
            "[x](/usr/bin/some thing)",
            // `file://localhost` is an ordinary URL, not a host path.
            "[x](file://localhost/tmp/a b.md)",
            "[x](/tmp/a b/c/)",
            "[x](/tmp/a?b c.md)",
            "[x](/tmp/../a b.md)",
        ] {
            let runs = paragraph(source);
            assert_eq!(link_of(&runs), None, "{source}");
            assert!(text(&runs).contains('('), "{source} should stay literal");
        }
    }

    #[test]
    fn already_bracketed_destinations_are_left_alone() {
        let runs = paragraph("see [it](/tmp/Some Folder/it's here.txt) now");
        assert_eq!(text(&runs), "see it now");
        assert_eq!(link_of(&runs), Some("/tmp/Some Folder/it's here.txt"));
    }

    #[test]
    fn code_keeps_its_spelling() {
        let tree = parse_full("```\n[x](/tmp/a b.md)\n```\n");
        let Block::CodeBlock { code, .. } = &tree.blocks[0].block else {
            panic!("fenced code");
        };
        assert_eq!(code.trim_end(), "[x](/tmp/a b.md)");

        let tree = parse_full("    [x](/tmp/a b.md)\n");
        let Block::CodeBlock { code, .. } = &tree.blocks[0].block else {
            panic!("indented code");
        };
        assert_eq!(code.trim_end(), "[x](/tmp/a b.md)");

        let runs = paragraph("`[x](/tmp/a b.md)`");
        assert!(runs.iter().all(|run| run.style.link.is_none()));
        assert_eq!(text(&runs), "[x](/tmp/a b.md)");
    }

    #[test]
    fn unclosed_destinations_wait_for_their_closing_paren() {
        let mut parser = IncrementalParser::new();
        parser.append("see [x](/tmp/a b");
        assert!(link_of(&paragraph(parser.source())).is_none());
        parser.append(".md) now");
        assert_eq!(link_of(&paragraph(parser.source())), Some("/tmp/a b.md"));
        assert_eq!(parser.tree(), &parse_full(parser.source()));
    }

    #[test]
    fn rewritten_ranges_map_back_to_the_original_source() {
        let source = "intro [x](/tmp/a b.md) tail\n\nsecond paragraph\n";
        let tree = parse_full(source);
        assert_eq!(tree.blocks.len(), 2);
        assert_eq!(
            &source[tree.blocks[0].range.clone()],
            "intro [x](/tmp/a b.md) tail\n"
        );
        assert_eq!(&source[tree.blocks[1].range.clone()], "second paragraph\n");
    }

    #[test]
    fn rewrite_is_idempotent_on_its_own_output() {
        let source = "see [it](/tmp/Some Folder/it's here.txt) and [web](https://x.dev/a b)";
        let once = rewrite_absolute_destinations(source, options(), &Skip::default(), |at| at)
            .expect("rewrite");
        assert!(once.0.contains("](</tmp/Some Folder/it's here.txt>)"));
        assert!(once.0.contains("https://x.dev/a b"));
        assert!(
            rewrite_absolute_destinations(&once.0, options(), &Skip::default(), |at| at).is_none(),
            "second pass must find nothing to rewrite"
        );
    }
}

#[cfg(test)]
mod math_tests {
    use super::*;

    fn blocks(source: &str) -> Vec<Block> {
        parse_full(source)
            .blocks
            .iter()
            .map(|top| top.block.clone())
            .collect()
    }

    fn para(source: &str) -> Vec<InlineRun> {
        match blocks(source).as_slice() {
            [Block::Paragraph { runs }] => runs.clone(),
            other => panic!("expected one paragraph for {source:?}: {other:?}"),
        }
    }

    fn math(text: &str) -> InlineRun {
        InlineRun {
            text: text.into(),
            style: InlineStyle {
                math: true,
                ..InlineStyle::default()
            },
        }
    }

    fn plain(text: &str) -> InlineRun {
        InlineRun {
            text: text.into(),
            style: InlineStyle::default(),
        }
    }

    fn display(tex: &str) -> Block {
        Block::Math { tex: tex.into() }
    }

    fn paragraph(runs: Vec<InlineRun>) -> Block {
        Block::Paragraph { runs }
    }

    #[test]
    fn inline_math_is_an_atomic_run() {
        assert_eq!(para("a $x^2$ b"), [plain("a "), math("x^2"), plain(" b")]);
        // Neighbouring formulas never merge; their TeX is not autolinked
        // and its markers are not emphasis.
        assert_eq!(para("$a$$b$"), [math("a"), math("b")]);
        assert_eq!(
            para("$\\text{https://x.dev}$"),
            [math("\\text{https://x.dev}")]
        );
        assert_eq!(para("$a_1 * b_2$"), [math("a_1 * b_2")]);
        // Inline math keeps the surrounding style.
        let runs = para("**bold $x$**");
        assert!(runs[1].style.math && runs[1].style.bold);
    }

    #[test]
    fn display_math_alone_is_a_block() {
        assert_eq!(blocks("$$x$$"), [display("x")]);
        assert_eq!(blocks("$$\n\\frac{a}{b}\n$$\n"), [display("\\frac{a}{b}")]);
    }

    #[test]
    fn display_math_mid_paragraph_splits_it() {
        assert_eq!(
            blocks("before\n$$x$$\nafter **b**"),
            [
                paragraph(vec![plain("before")]),
                display("x"),
                paragraph(vec![
                    plain("after "),
                    InlineRun {
                        text: "b".into(),
                        style: InlineStyle {
                            bold: true,
                            ..InlineStyle::default()
                        }
                    }
                ]),
            ]
        );
        assert_eq!(
            blocks("lead $$x$$ $$y$$ tail"),
            [
                paragraph(vec![plain("lead")]),
                display("x"),
                display("y"),
                paragraph(vec![plain("tail")]),
            ]
        );
        assert_eq!(
            blocks("$$x$$ trailing"),
            [display("x"), paragraph(vec![plain("trailing")])]
        );
        // Every piece shares the source paragraph's range.
        let source = "intro\n\nbefore $$x$$ after\n";
        let tree = parse_full(source);
        assert_eq!(tree.len(), 4);
        for top in &tree.blocks[1..] {
            assert_eq!(&source[top.range.clone()], "before $$x$$ after\n");
        }
    }

    #[test]
    fn display_math_splits_inside_lists_and_quotes() {
        let Block::List { items, .. } = &blocks("- see $$x$$ ok\n- $$y$$\n")[0] else {
            panic!("list");
        };
        assert_eq!(
            items[0],
            [
                paragraph(vec![plain("see")]),
                display("x"),
                paragraph(vec![plain("ok")])
            ]
        );
        assert_eq!(items[1], [display("y")]);
        let Block::BlockQuote { children } = &blocks("> q\n> $$z$$\n")[0] else {
            panic!("quote");
        };
        assert_eq!(children, &[paragraph(vec![plain("q")]), display("z")]);
    }

    #[test]
    fn display_math_stays_inline_in_headings_tables_and_spans() {
        let Block::Heading { runs, .. } = &blocks("# Sum $$x$$")[0] else {
            panic!("heading");
        };
        assert_eq!(runs, &[plain("Sum "), math("x")]);
        let Block::Table { rows, .. } = &blocks("| m |\n|---|\n| $$c$$ |\n")[0] else {
            panic!("table");
        };
        assert_eq!(rows[0][0], [math("c")]);
        let runs = para("*a $$x$$ b*");
        assert!(runs.iter().any(|run| run.style.math && run.style.italic));
    }

    #[test]
    fn currency_stays_text() {
        assert_eq!(para("costs $5-$10 total"), [plain("costs $5-$10 total")]);
        assert_eq!(para("between $5 and $10"), [plain("between $5 and $10")]);
        assert_eq!(para("$x$5"), [plain("$x$5")]);
        // A formula followed by a non-digit is still math.
        assert_eq!(para("$5$ apples"), [math("5"), plain(" apples")]);
    }

    #[test]
    fn shell_variables_stay_text() {
        assert_eq!(para("Set $HOME/$USER now"), [plain("Set $HOME/$USER now")]);
        assert_eq!(para("echo $FOO$BAR"), [plain("echo $FOO$BAR")]);
        // Capital letters alone are still geometry, and suffixes still attach.
        assert_eq!(
            para("triangle $ABC$ here"),
            [plain("triangle "), math("ABC"), plain(" here")]
        );
        assert_eq!(
            para("the $n$th one"),
            [plain("the "), math("n"), plain("th one")]
        );
        // Joined with `:` or `@`, as in shell lines.
        assert_eq!(
            para("PATH=$PATH:$HOME/bin"),
            [plain("PATH=$PATH:$HOME/bin")]
        );
        assert_eq!(para("'$USER@$HOST'"), [plain("'$USER@$HOST'")]);
        assert_eq!(
            para("so $x$ holds"),
            [plain("so "), math("x"), plain(" holds")]
        );
    }

    /// Dollars that aren't math parse exactly as they did without math
    /// support: a rejected span's markup still applies.
    #[test]
    fn rejected_dollar_spans_keep_their_markdown() {
        let without_math = |source: &str| parse_with(source, 0, options() - Options::ENABLE_MATH);
        for source in [
            "**$5** to **$10**",
            "from $5 to [$10](u)",
            "costs $5 (`$4` with code)",
            "costs $5-$10 total, between $5 and $10",
            "$5 *a* $6 *b* $7 *c* $8",
            "pay $5, get _$10_ back and ~~$2~~",
            "PATH=$PATH:$HOME/bin and `$HOME`",
            "'$USER@$HOST' **now**",
            "> quoted $5 **b** $6\n",
            "- $1 [x](u) $2\n- $3 *i* $4\n",
            "| a | b |\n|---|---|\n| $5 **x** | $6 |\n",
            "# $5 *off* $9\n",
        ] {
            assert_eq!(parse_full(source), without_math(source), "{source:?}");
        }
        // Mixed with real math, each span is judged on its own.
        let runs = para("**$5** and $x$ then **$10**");
        assert!(runs.iter().any(|run| run.style.bold && run.text == "$5"));
        assert!(runs.contains(&math("x")));
        assert!(runs.iter().any(|run| run.style.bold && run.text == "$10"));
        // Ranges still point at the original source.
        let source = "**$5** to **$10**\n\nnext\n";
        let tree = parse_full(source);
        assert_eq!(&source[tree.blocks[0].range.clone()], "**$5** to **$10**\n");
        assert_eq!(&source[tree.blocks[1].range.clone()], "next\n");
    }

    #[test]
    fn latex_delimiters_become_math() {
        assert_eq!(
            para("so \\(a^2 + b^2\\) holds"),
            [plain("so "), math("a^2 + b^2"), plain(" holds")]
        );
        // Whitespace inside `\( … \)` is dropped (`$ x $` would not parse).
        assert_eq!(para("\\( x \\)"), [math("x")]);
        // A rewritten closer before a digit is not a price.
        assert_eq!(para("\\(x\\)2"), [math("x"), plain("2")]);
        assert_eq!(blocks("\\[ e = mc^2 \\]"), [display("e = mc^2")]);
        assert_eq!(
            blocks("Thus\n\\[\n\\int_0^1 f\\,dx \\\\\ny\n\\]\nso"),
            [
                paragraph(vec![plain("Thus")]),
                display("\\int_0^1 f\\,dx \\\\\ny"),
                paragraph(vec![plain("so")]),
            ]
        );
    }

    #[test]
    fn unclosed_latex_openers_scan_in_linear_time() {
        // 20k openers with no closer in their paragraph: each would rescan it.
        // Likewise a long line of backtick runs, each asking for its line's
        // context.
        let mut source = "\\(".repeat(20_000);
        source.push_str(&"`a` ``b ".repeat(10_000));
        source.push_str(" tail\n\nand \\(y\\) after");
        let started = std::time::Instant::now();
        let out = rewrite_math_delimiters(&source, options(), &Skip::default(), |at| at)
            .unwrap()
            .text;
        assert!(
            started.elapsed().as_millis() < 500,
            "{:?}",
            started.elapsed()
        );
        // The openers stay literal; the next paragraph's pair still closes.
        assert!(out.starts_with("\\(\\(\\("));
        assert!(
            out.ends_with(" tail\n\nand $y$ after"),
            "{}",
            &out[out.len() - 30..]
        );
    }

    #[test]
    fn latex_delimiters_need_a_closer_in_the_same_paragraph() {
        // Unclosed (still streaming): markdown escapes, so `(` / `[` show.
        assert_eq!(para("so \\(a^2"), [plain("so (a^2")]);
        assert_eq!(
            blocks("\\[\nx\n\nmore \\]"),
            [
                paragraph(vec![plain("[ x")]),
                paragraph(vec![plain("more ]")])
            ]
        );
        // A closer past the start of another block doesn't pair either — not
        // into a fence's body, not across a heading or list item.
        let tree = blocks("a \\[\n```\n\\]\n```\n");
        assert_eq!(tree[0], paragraph(vec![plain("a [")]));
        assert_eq!(
            tree[1],
            Block::CodeBlock {
                language: None,
                code: "\\]".into()
            }
        );
        assert_eq!(blocks("# h \\(\nx \\)")[1], paragraph(vec![plain("x )")]));
        assert_eq!(blocks("a \\(\n- x \\)").len(), 2);
        // Quoted paragraphs continue over their `>` lines.
        let Block::BlockQuote { children } = &blocks("> \\[\n> x\n> \\]\n")[0] else {
            panic!("quote");
        };
        assert_eq!(children, &[display("x")]);
    }

    #[test]
    fn latex_delimiters_leave_escapes_and_code_alone() {
        // `\\(` is a literal backslash then `(`.
        assert_eq!(para("a \\\\(b\\\\) c"), [plain("a \\(b\\) c")]);
        // Escaped brackets glued to a word, and escaped task boxes.
        assert_eq!(para("arr\\[0\\] and f\\(x\\)"), [plain("arr[0] and f(x)")]);
        assert_eq!(para("\\[ \\] todo"), [plain("[ ] todo")]);
        // Code spans, fences and indented code keep their spelling.
        let runs = para("`\\(x\\)` and \\(y\\)");
        assert_eq!(runs[0].text, "\\(x\\)");
        assert!(runs[0].style.code);
        assert_eq!(runs[2], math("y"));
        let Block::CodeBlock { code, .. } = &blocks("```\n\\[x\\]\n```\n")[0] else {
            panic!("fence");
        };
        assert_eq!(code, "\\[x\\]");
        let Block::CodeBlock { code, .. } = &blocks("    \\(x\\)\n")[0] else {
            panic!("indented");
        };
        assert_eq!(code, "\\(x\\)");
    }

    /// Random prose where every `$` is a price (and no backslash can form a
    /// LaTeX delimiter): whatever spans pulldown pairs, the result must match
    /// a parse without math.
    #[test]
    fn random_prices_parse_as_without_math() {
        const PIECES: &[&str] = &[
            "$5", "$10", "$1.50", " ", " ", "a", "b c", "**", "*", "_", "~~", "`", "``", "[",
            "](u)", "]", "(", ")", "\\*", "\n", "\n\n", "\n- ", "\n> ", "|", "é", "-", ":",
        ];
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..3000 {
            let len = 4 + (next() % 40) as usize;
            let source: String = (0..len)
                .map(|_| PIECES[(next() % PIECES.len() as u64) as usize])
                .collect();
            assert_eq!(
                parse_full(&source),
                parse_with(&source, 0, options() - Options::ENABLE_MATH),
                "{source:?}"
            );
        }
    }

    #[test]
    fn latex_delimiters_never_rewrite_code_blocks() {
        // A stray backtick run can't pair past its paragraph into a fence.
        let tree = blocks("Wrap it in ```latex like this:\n\n```latex\n\\(x\\) and \\[y\\]\n```\n");
        assert_eq!(
            tree[1],
            Block::CodeBlock {
                language: Some("latex".into()),
                code: "\\(x\\) and \\[y\\]".into()
            }
        );
        // Fences in a quote or a list item keep their spelling too.
        let Block::BlockQuote { children } = &blocks("> ```\n> \\(x\\)\n> ```\n")[0] else {
            panic!("quote");
        };
        assert_eq!(
            children[0],
            Block::CodeBlock {
                language: None,
                code: "\\(x\\)".into()
            }
        );
        let Block::List { items, .. } = &blocks("- ```\n  \\[y\\]\n  ```\n")[0] else {
            panic!("list");
        };
        assert_eq!(
            items[0][0],
            Block::CodeBlock {
                language: None,
                code: "\\[y\\]".into()
            }
        );
    }

    #[test]
    fn latex_delimiters_skip_link_destinations() {
        let runs = para("[w](https://en.wikipedia.org/wiki/Foo_\\(bar\\))");
        assert_eq!(
            runs[0].style.link.as_deref(),
            Some("https://en.wikipedia.org/wiki/Foo_(bar)")
        );
        assert_eq!(runs[0].text, "w");
        // Escapes glued to URL and identifier characters stay escapes.
        assert_eq!(para("x_\\(1\\) and a/\\(b\\)"), [plain("x_(1) and a/(b)")]);
        // Autolinks and reference definitions keep their spelling.
        let runs = para("<https://x.dev/\\(a\\)> \\(y\\)");
        assert_eq!(runs[0].style.link.as_deref(), Some("https://x.dev/\\(a\\)"));
        assert_eq!(runs.last(), Some(&math("y")));
        // An opener before the link doesn't pair with a paren in its URL.
        let runs = para("\\(x [w](https://x.dev/Foo_\\(bar\\))");
        assert_eq!(runs[0], plain("(x "));
        assert_eq!(
            runs[1].style.link.as_deref(),
            Some("https://x.dev/Foo_(bar)")
        );
        let runs = para("[w]\n\n[w]: https://x.dev/\\(a\\)\n");
        assert_eq!(runs[0].style.link.as_deref(), Some("https://x.dev/(a)"));
    }

    #[test]
    fn latex_delimiters_stop_at_setext_underlines_and_tables() {
        // An underline ends the paragraph: no formula spans it.
        let tree = blocks("a \\(x\n---\ny\\)\n");
        assert_eq!(
            tree[0],
            Block::Heading {
                level: 2,
                runs: vec![plain("a (x")]
            }
        );
        assert_eq!(tree[1], paragraph(vec![plain("y)")]));
        let tree = blocks("a \\(x\n===\ny\\)\n");
        assert!(matches!(tree[0], Block::Heading { level: 1, .. }));
        // A table's header row starts a new block.
        let tree = blocks("p \\(x\n| a |\n|:-:|\n| y\\) |\n");
        assert_eq!(tree[0], paragraph(vec![plain("p (x")]));
        assert!(matches!(tree[1], Block::Table { .. }));
    }

    /// Minimized streaming divergences (some from review, the rest from the
    /// randomized parity test): stray backticks, setext underlines, escaped
    /// newlines, fences in quotes and list items, HTML blocks.
    #[test]
    fn streamed_regressions_match_full_parse() {
        for source in [
            "a ` b\n\nc\n\n\\(x\\) `y`\n",
            "a \\(x\n---\ny\\)\n\nz\n\nw\n",
            "a \\(x\n|---|\ny\\)\n\nz\n\nw\n",
            "Wrap it in ```latex like this:\n\n```latex\n\\(x\\) and \\[y\\]\n```\n",
            "\\(\\\n\n=\\)\n\n](",
            "```\\`\n>\n\\[y\\]",
            "```\\`\n\n\\[*\\]\n\n漢|",
            ">```\\[\\\n\\]\n\n)*",
            ">```\n\\[>\\]```\n> ",
            "\\(`\n    \n    \\)\n-]",
            "1. <p>\\('\n`\\)\n- ``",
            "1. # \\[漢\n\\]\n> ",
            "><p>\\(\n    x\n\\)",
            "<p\n```\n\n>\n\\(|\\)",
            "- \\[\n    <p\n\\]\n\né\n",
        ] {
            let full = parse_full(source);
            for chunk in 1..=17 {
                let mut parser = IncrementalParser::new();
                let mut start = 0;
                while start < source.len() {
                    let mut end = (start + chunk).min(source.len());
                    while !source.is_char_boundary(end) {
                        end += 1;
                    }
                    parser.append(&source[start..end]);
                    start = end;
                }
                assert_eq!(parser.tree(), &full, "chunk {chunk}: {source:?}");
            }
        }
    }

    #[test]
    fn rewritten_ranges_map_back_to_the_original_source() {
        let source = "a \\( x \\) [l](/tmp/a b.md)\n\n\\[y\\] tail\n";
        let tree = parse_full(source);
        assert_eq!(
            &source[tree.blocks[0].range.clone()],
            "a \\( x \\) [l](/tmp/a b.md)\n"
        );
        assert_eq!(&source[tree.blocks[1].range.clone()], "\\[y\\] tail\n");
        let Block::Paragraph { runs } = &tree.blocks[0].block else {
            panic!("paragraph");
        };
        assert_eq!(runs[1], math("x"));
        assert_eq!(
            runs.iter().find_map(|run| run.style.link.as_deref()),
            Some("/tmp/a b.md")
        );
        // Composed offsets: the event ranges themselves land on the
        // original delimiters.
        let pass = rewrite_math_delimiters(source, options(), &Skip::default(), |at| at).unwrap();
        let (text, map) = (pass.text, pass.map);
        assert_eq!(&text[..6], "a $x$ ");
        assert_eq!(map.original_range(2..5), 2..9);
        assert_eq!(&source[2..9], "\\( x \\)");
    }

    #[test]
    fn streamed_math_matches_full_parse() {
        let mut parser = IncrementalParser::new();
        parser.set_text("text $$x");
        assert_eq!(parser.display_tree(), *parser.tree());
        parser.append("$$ more **b");
        assert_eq!(parser.tree(), &parse_full(parser.source()));
        // The display tail replaces the whole split paragraph exactly once.
        let shown = parser.display_tree();
        assert_eq!(shown.len(), 3);
        assert_eq!(shown.blocks[1].block, display("x"));
        let Block::Paragraph { runs } = &shown.blocks[2].block else {
            panic!("paragraph");
        };
        assert!(runs.iter().any(|run| run.style.bold && run.text == "b"));
    }
    /// No formula anywhere in the tree (any depth).
    fn assert_no_math(source: &str) {
        let tree = format!("{:?}", parse_full(source));
        assert!(
            !tree.contains("math: true") && !tree.contains("Math {"),
            "{source:?} grew math: {tree}"
        );
    }

    /// The rewrite follows pulldown-cmark's own block structure, so code,
    /// HTML and reference definitions keep their spelling in any container,
    /// and no formula spans a heading or HTML block nested in one.
    #[test]
    fn latex_delimiters_follow_container_structure() {
        let Block::BlockQuote { children } = &blocks(">     \\(x\\) code\n")[0] else {
            panic!("quote");
        };
        assert_eq!(
            children[0],
            Block::CodeBlock {
                language: None,
                code: "\\(x\\) code".into()
            }
        );
        let Block::List { items, .. } = &blocks("-     \\(x\\)\n")[0] else {
            panic!("list");
        };
        assert_eq!(
            items[0][0],
            Block::CodeBlock {
                language: None,
                code: "\\(x\\)".into()
            }
        );
        for source in [
            "- > \t\\(b\\)\n",
            "  - \t\\(\n    !\n=\\)\n- u)",
            "> # Title \\(\n> body \\) end\n",
            "- # Title \\(\n  body \\) end\n",
            "> <!-- note -->\\(\n> text \\) end\n",
            "- <p>\\(\n  text \\) end\n",
            "<!-- note \\(x\\) -->\n",
            "<div>\n\\(x\\)\n</div>\n",
        ] {
            assert_no_math(source);
        }
        let Block::BlockQuote { children } =
            &blocks("> [r]: https://x.y/a,\\(b\\)\n>\n> see [w][r]\n")[0]
        else {
            panic!("quote");
        };
        let Block::Paragraph { runs } = &children[0] else {
            panic!("paragraph");
        };
        assert_eq!(
            runs.iter().find_map(|run| run.style.link.as_deref()),
            Some("https://x.y/a,(b)")
        );
        // Math still renders inside containers.
        let Block::BlockQuote { children } = &blocks("> q \\(x\n> y\\) z\n")[0] else {
            panic!("quote");
        };
        assert_eq!(
            children[0],
            paragraph(vec![plain("q "), math("x\ny"), plain(" z")])
        );
    }

    #[test]
    fn latex_openers_after_operators_are_math_but_not_inside_urls() {
        assert_eq!(para("k=\\(2\\pi\\)"), [plain("k="), math("2\\pi")]);
        assert_eq!(para("-\\(x\\)"), [plain("-"), math("x")]);
        assert_eq!(para("(\\(x\\))"), [plain("("), math("x"), plain(")")]);
        assert_no_math("see https://x.dev/a,\\(b\\) and /tmp/p-\\(1\\)");
        // A lone `$` inside the body is a literal dollar, as `\$` is.
        assert_eq!(para("\\(a $ b\\)"), [math("a \\$ b")]);
        assert_eq!(para("\\(a \\$ b\\)"), [math("a \\$ b")]);
        // A source dollar never pairs with a rewritten opener.
        let runs = para("[$]()\\(a\\)");
        assert_eq!(runs[0].text, "$");
        assert!(runs[0].style.link.is_some());
        assert_eq!(runs[1], math("a"));
        assert_eq!(
            para("costs $5 and \\(x\\)"),
            [plain("costs $5 and "), math("x")]
        );
    }

    /// Long single lines of the shapes that once scanned their line (or
    /// their backslash run, or their unclosed destinations) per byte.
    #[test]
    fn delimiter_rewrites_stay_linear() {
        let line = |unit: &str| unit.repeat((256 << 10) / unit.len());
        for source in [
            line("\\(x\\) *a* b "),
            line("\\( *a* "),
            line("`\\(` "),
            format!("\\(x\\) {}", "\\".repeat(256 << 10)),
            line("[a](\\( "),
            line("[a](x "),
            line("-\\(x\\)"),
            format!("{}$ \\)", "\\( ".repeat(85_000)),
            format!("{}$ \\]", "\\[ ".repeat(85_000)),
            format!("see http://a{}", ")".repeat(64 << 10)),
            "_a ".repeat(40_000) + &"a* ".repeat(40_000),
        ] {
            let started = std::time::Instant::now();
            parse_full(&source);
            let elapsed = started.elapsed();
            assert!(
                elapsed < std::time::Duration::from_secs(3),
                "{elapsed:?} for {:?}…",
                &source[..24]
            );
        }
    }

    /// The backward pass answers exactly what a forward scan from each start
    /// would.
    #[test]
    fn destination_closes_match_a_forward_scan() {
        const BYTES: &[u8] = b"()\\\\ \n\tx)(";
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..4000 {
            let len = (next() % 40) as usize;
            let bytes: Vec<u8> = (0..len)
                .map(|_| BYTES[(next() % BYTES.len() as u64) as usize])
                .collect();
            // Starts follow a `(` or a space, as destination starts do.
            let starts: Vec<usize> = (1..=bytes.len())
                .filter(|&at| matches!(bytes[at - 1], b'(' | b' '))
                .collect();
            let bulk = destination_closes(&bytes, &escapes(&bytes), starts.iter().copied());
            for (&start, close) in starts.iter().zip(bulk) {
                assert_eq!(
                    close,
                    destination_close(&bytes, start),
                    "{:?} from {start}",
                    String::from_utf8_lossy(&bytes)
                );
            }
        }
    }

    #[test]
    fn quoted_and_listed_link_definitions_reparse_in_full() {
        for source in [
            "> [r]: https://x.y/a\n>\n> see [w][r]\n",
            "- [r]: https://x.y/a\n\n  see [w][r]\n",
            "> - [r]: u\n> \n>   [w][r] \\(x\\)\n",
        ] {
            assert!(has_link_defs(source), "{source:?}");
            let full = parse_full(source);
            for chunk in 1..=9 {
                let mut parser = IncrementalParser::new();
                let mut start = 0;
                while start < source.len() {
                    let end = (start + chunk).min(source.len());
                    parser.append(&source[start..end]);
                    start = end;
                }
                assert_eq!(parser.tree(), &full, "chunk {chunk}: {source:?}");
            }
        }
    }

    /// A `$` that ends the stream can't be judged until the next character
    /// (`$5 to $10` is prose): the display never flashes the prose between
    /// the dollars as a formula, while one that does close renders at once.
    #[test]
    fn a_trailing_dollar_never_flashes_prose_as_math() {
        for prefix in [
            "costs $5 to $",
            "**$5** to **$",
            "from $5 to [$",
            "set $HOME and $",
        ] {
            let mut parser = IncrementalParser::new();
            parser.set_text(prefix);
            let shown = format!("{:?}", parser.display_tree());
            assert!(!shown.contains("math: true"), "{prefix:?}: {shown}");
        }
        let mut parser = IncrementalParser::new();
        parser.set_text("so $x^2$");
        assert!(format!("{:?}", parser.display_tree()).contains("math: true"));
        // Where that `$` is plain text anyway (code, raw HTML) it stays as
        // written, never as `\$`.
        for source in ["- item\n\n      echo $", "<div>cost $", "`a $"] {
            let mut parser = IncrementalParser::new();
            parser.set_text(source);
            let shown = format!("{:?}", parser.display_tree());
            assert!(!shown.contains("\\\\$"), "{source:?}: {shown}");
        }
    }

    /// Markdown escapes around prose — citations, numbers, words — stay
    /// brackets and parens; TeX-looking bodies are math.
    #[test]
    fn escaped_brackets_around_prose_stay_text() {
        for source in [
            "See \\[1\\] for details.",
            "Pandoc \\[Online\\] style.",
            "Value \\(optional\\) here.",
            "Like \\(see above\\) and \\(2.3\\).",
        ] {
            assert_no_math(source);
        }
        assert_eq!(para("so \\(x\\)"), [plain("so "), math("x")]);
        assert_eq!(para("so \\(ab\\)"), [plain("so "), math("ab")]);
        assert_eq!(para("so \\(3x\\)"), [plain("so "), math("3x")]);
        assert_eq!(
            para("so \\(\\text{cost}\\)"),
            [plain("so "), math("\\text{cost}")]
        );
        assert_eq!(blocks("\\[ x^2 \\]"), [display("x^2")]);
    }
    /// Every rewrite is checked against the parse it produced: a code span
    /// the dollar pass reopens, a trimmed newline that moves a line, a quote
    /// marker inside the body — all keep the source's spelling.
    #[test]
    fn rewrites_that_dont_survive_the_parse_are_undone() {
        fn walk(blocks: &[Block], out: &mut Vec<String>) {
            for block in blocks {
                match block {
                    Block::Paragraph { runs } | Block::Heading { runs, .. } => out.extend(
                        runs.iter()
                            .filter(|run| run.style.code)
                            .map(|run| run.text.clone()),
                    ),
                    Block::BlockQuote { children } => walk(children, out),
                    Block::List { items, .. } => items.iter().for_each(|item| walk(item, out)),
                    _ => {}
                }
            }
        }
        let code = |source: &str| {
            let mut out = Vec::new();
            walk(&blocks(source), &mut out);
            out
        };
        assert_eq!(
            code("With $HOME set, the regex `^$\\(a\\)` matches"),
            ["^$\\(a\\)"]
        );
        assert_eq!(
            code("Costs $5. Use `printf \"%s$\" \\(x\\)` here"),
            ["printf \"%s$\" \\(x\\)"]
        );
        for source in [
            "> \\(\n> x\\)\n",
            "- > \\(\n  > x\\)\n",
            "> \\(x\n> \\)\n",
            "\\(x \\ \\)",
            "\\(\\ \\)",
            "In BRE write \\( … `grep 'a\\(b\\)'` … close it with \\).",
            "Escape them as \\[ and \\] in the regex",
        ] {
            let tree = format!("{:?}", parse_full(source));
            assert!(!tree.contains("Math {"), "{source:?}: {tree}");
            assert!(!tree.contains("$$"), "{source:?}: {tree}");
        }
        // A lone `$` beside rewritten delimiters stays a literal dollar (and
        // a bare `\\[y \\]` mid-line is an escape).
        assert_eq!(
            blocks("$\\(x\\)\\[y \\]"),
            [paragraph(vec![plain("$"), math("x"), plain("[y ]")])]
        );
        // Delimiters across a table delimiter row stay literal, the same
        // streamed or not.
        let source = "\\(x\n|-|\n\\)\n---|---\n";
        assert_no_math(source);
        let full = parse_full(source);
        for chunk in 1..=12 {
            let mut parser = IncrementalParser::new();
            let mut start = 0;
            while start < source.len() {
                let end = (start + chunk).min(source.len());
                parser.append(&source[start..end]);
                start = end;
            }
            assert_eq!(parser.tree(), &full, "chunk {chunk}");
        }
        // A reference link keeps its label (and so its definition).
        let runs = para("[\\(x\\)]\n\n[\\(x\\)]: /u\n");
        assert_eq!(runs[0].style.link.as_deref(), Some("/u"));
        // A `](` with no link to finish never shows the brackets it got.
        let tree = format!("{:?}", parse_full("see ](/abs path/a b.png) here"));
        assert!(!tree.contains('<'), "{tree}");
    }

    #[test]
    fn image_alt_text_keeps_its_formula_spelling() {
        let runs = para("![img $a$](u)");
        assert_eq!(
            runs[0].style.image.as_ref().map(|image| image.alt.as_str()),
            Some("img $a$")
        );
    }

    #[test]
    fn task_markers_stay_with_their_own_item() {
        let tasks = |source: &str| {
            format!("{:?}", parse_full(source))
                .matches("task: Some")
                .count()
        };
        assert_eq!(tasks("- [ ] > quote\n"), 1);
        assert_eq!(tasks("- [x] Title\n  =====\n"), 1);
        // Markers pulldown emits after their item closed check nothing off.
        for source in ["- [ ] -\n\n- p\n", "- [x] -\n\n> - p\n", "- [x] *\n\nb\n"] {
            assert_eq!(tasks(source), 0, "{source:?}");
        }
    }

    /// Hostile nesting and inputs pulldown-cmark itself panics on: the tree
    /// stays shallow and the parse never takes the process down.
    #[test]
    fn hostile_nesting_and_parser_panics_are_contained() {
        fn depth(blocks: &[Block]) -> usize {
            blocks
                .iter()
                .map(|block| match block {
                    Block::BlockQuote { children } => 1 + depth(children),
                    Block::List { items, .. } => {
                        1 + items.iter().map(|item| depth(item)).max().unwrap_or(0)
                    }
                    _ => 1,
                })
                .max()
                .unwrap_or(0)
        }
        for source in [
            ">".repeat(20_000),
            "- ".repeat(20_000),
            format!("{}x{}", "![".repeat(20_000), "](u)".repeat(20_000)),
            "~~a ".repeat(20_000),
            "*a ".repeat(20_000),
        ] {
            let tree = std::thread::Builder::new()
                .stack_size(1 << 20)
                .spawn(move || parse_full(&source))
                .unwrap()
                .join()
                .expect("no stack overflow");
            let blocks: Vec<Block> = tree.blocks.iter().map(|top| top.block.clone()).collect();
            assert!(depth(&blocks) <= MAX_NESTING + 1);
        }
        let source = "- [ ] [b]:/\n      \n  -";
        assert!(!parse_full(source).blocks.is_empty());
        let mut parser = IncrementalParser::new();
        parser.set_text(source);
    }

    /// Dollars are decided once, before the parse: prices, shell text and
    /// `$$` PIDs stay literal however many a paragraph holds, and code
    /// spans between them keep their spelling.
    #[test]
    fn dollar_prose_never_eats_code_spans() {
        fn code(blocks: &[Block], out: &mut Vec<String>) {
            for block in blocks {
                if let Block::Paragraph { runs } = block {
                    out.extend(
                        runs.iter()
                            .filter(|run| run.style.code)
                            .map(|run| run.text.clone()),
                    );
                }
            }
        }
        let source = "Pay $5 for `$x`.\n".repeat(40);
        let mut spans = Vec::new();
        code(&blocks(&source), &mut spans);
        assert_eq!(spans.len(), 40, "{spans:?}");
        assert!(spans.iter().all(|span| span == "$x"));
        for source in [
            "The PID $$ differs from `$$` in a subshell",
            "kill $$ && echo $$",
            "cost $$5 vs $$10",
            "**bold $x** y$",
        ] {
            assert_no_math(source);
        }
        let mut spans = Vec::new();
        code(
            &blocks("The PID $$ differs from `$$` in a subshell"),
            &mut spans,
        );
        assert_eq!(spans, ["$$"]);
        assert_eq!(
            blocks("$x^2$ and $$y$$"),
            [paragraph(vec![math("x^2"), plain(" and")]), display("y")]
        );
    }

    /// A block whose rewrites fail stays literal on its own: the message's
    /// other formulas still render.
    #[test]
    fn a_failing_block_keeps_other_blocks_math() {
        let snippet = "$$$`$`$\\ $$$`$`$\\(\\({](\\))$$\\[]\\]{$``$\\`$) $`$$$( $/$ ";
        let tree = blocks(&format!("so $x^2$ here\n\n{snippet}\n\nand \\(y\\) too\n"));
        assert_eq!(
            tree[0],
            paragraph(vec![plain("so "), math("x^2"), plain(" here")])
        );
        assert_eq!(
            tree.last(),
            Some(&paragraph(vec![plain("and "), math("y"), plain(" too")]))
        );
    }

    /// The reviewer's 60-byte snippet once forced ~74 full parses.
    #[test]
    fn hostile_dollars_cost_a_bounded_number_of_parses() {
        let snippet = "$$$`$`$\\ $$$`$`$\\(\\({](\\))$$\\[]\\]{$``$\\`$) $`$$$( $/$ ";
        let source = format!("{snippet}\n{}", "a\n".repeat(64 << 10));
        let started = std::time::Instant::now();
        parse_full(&source);
        let elapsed = started.elapsed();
        assert!(elapsed < std::time::Duration::from_secs(2), "{elapsed:?}");
    }

    #[test]
    fn deep_collapsed_content_keeps_its_text() {
        let lists = format!("> {}hello world\n", "- ".repeat(40));
        assert!(format!("{:?}", parse_full(&lists)).contains("hello world"));
        let table = format!("{}| a | b |\n|---|---|\n| cell | two |\n", ">".repeat(31));
        let tree = format!("{:?}", parse_full(&table));
        assert!(tree.contains("cell") && tree.contains("two"), "{tree}");
    }

    #[test]
    fn reference_images_and_spaced_destinations_keep_their_links() {
        let runs = para("![\\(x\\)]\n\n[\\(x\\)]: /i.png\n");
        assert_eq!(
            runs[0]
                .style
                .image
                .as_ref()
                .map(|image| image.source.as_str()),
            Some("/i.png")
        );
        // A spaced destination after a label that also names a definition.
        let runs = para("[a](/x y.md)\n\n[a]: /u\n");
        assert_eq!(runs[0].style.link.as_deref(), Some("/x y.md"));
        // A rewrite inside another link's text that doesn't finish its own
        // link is undone: no injected brackets.
        let tree = format!("{:?}", parse_full("[x ![a](/p q<r.png) y](/u)"));
        assert!(!tree.contains("</p q"), "{tree}");
    }

    #[test]
    fn prose_and_urls_in_latex_delimiters_stay_text() {
        for source in [
            "\\(see **note**\\)",
            "\\(optional, default \"x\"\\)",
            "\\(e.g. 5%\\)",
            "\\(https://x.y/z\\)",
            "cite \\[1, 2\\]",
            "\\[see above, or #3\\]",
            "array \\[i\\]",
            "<span>\\(x</span>\\)",
            "**\\(x** y\\)",
        ] {
            assert_no_math(source);
        }
        assert_eq!(para("\\(a*b*c\\)"), [math("a*b*c")]);
        assert_eq!(para("\\(x, y\\)"), [math("x, y")]);
    }

    #[test]
    fn multi_line_reference_labels_reparse_in_full() {
        assert!(has_link_defs("b]: /u"));
        let source = "[a\nb]: /u\n\n[a b]\n";
        let full = parse_full(source);
        for chunk in 1..=6 {
            let mut parser = IncrementalParser::new();
            let mut start = 0;
            while start < source.len() {
                let end = (start + chunk).min(source.len());
                parser.append(&source[start..end]);
                start = end;
            }
            assert_eq!(parser.tree(), &full, "chunk {chunk}");
        }
    }

    /// Dollars inside URLs and query strings are part of them: the link
    /// stays whole and nothing becomes a formula.
    #[test]
    fn dollars_in_urls_and_queries_stay_literal() {
        let runs = para(
            "Call https://graph.microsoft.com/v1.0/me/messages?$select=subject&$top=10 to list",
        );
        assert_eq!(
            runs.iter().find_map(|run| run.style.link.as_deref()),
            Some("https://graph.microsoft.com/v1.0/me/messages?$select=subject&$top=10")
        );
        for source in [
            "Visit https://x.dev/$a$b now",
            "**https://example.com/$id$/x**",
            "(_https://example.com/$a$b_)",
            "GET /api/items?$select=name&$expand=owner",
            "Program Files \\(x86\\)",
            "built for \\(arm64\\) only",
        ] {
            assert_no_math(source);
        }
        assert_eq!(para("so \\(x2\\)"), [plain("so "), math("x2")]);
        // A table pipe isn't part of the URL word: the formula cell keeps
        // its math.
        let tree = format!("{:?}", parse_full("|https://a.com|$x^2$|\n|-|-|\n"));
        assert!(tree.contains("math: true"), "{tree}");
    }

    /// Round-6 review: prose and shell shapes that once swallowed text.
    #[test]
    fn shell_regex_and_quoted_dollars_stay_text() {
        for source in [
            "Set \"$HOME\" and \"$USER\"",
            "\"$@\" and \"$*\"",
            "($foo) or ($bar)",
            "array($a,$b)",
            "${PREFIX}${NAME}",
            "@$ref and %$ref",
            "/x$/i or /y$/g",
            "grep -E 'a$|b$'",
            "$5k-$10k, or 5$",
            "Use $$ for the PID and $$ again",
            "echo $$; echo $$",
            "Escape parens as \\( and \\) in BRE",
            "Use \\(.*\\) to capture",
        ] {
            assert_no_math(source);
        }
        // Still math.
        assert_eq!(para("$|x|$"), [math("|x|")]);
        assert_eq!(para("$(a+b)^2$"), [math("(a+b)^2")]);
        assert_eq!(para("$[0,1)$"), [math("[0,1)")]);
        assert_eq!(para("$2 + 2 = 4$"), [math("2 + 2 = 4")]);
        assert_eq!(blocks("$$E = mc^2$$"), [display("E = mc^2")]);
    }

    /// A failing rewrite leaves only its innermost block literal: other
    /// items, and other formulas in the message, keep theirs.
    #[test]
    fn failures_stay_in_their_own_leaf() {
        let tree = format!(
            "{:?}",
            parse_full(
                "- Energy $E=mc^2$ and [doc](/tmp/My Docs/a.md)\n- item two\n- a stray ](/tmp/x y.md) here\n"
            )
        );
        assert!(tree.contains("math: true"), "{tree}");
        assert!(tree.contains("/tmp/My Docs/a.md"), "{tree}");
        let tree =
            blocks("Energy is $E=mc^2$ and \\(F=ma\\).\n\n[1]: \\( x \\)\n\nMore $a^2$ text.\n");
        assert_eq!(
            tree[0],
            paragraph(vec![
                plain("Energy is "),
                math("E=mc^2"),
                plain(" and "),
                math("F=ma"),
                plain(".")
            ])
        );
        assert_eq!(
            tree.last(),
            Some(&paragraph(vec![
                plain("More "),
                math("a^2"),
                plain(" text.")
            ]))
        );
        // A formula never breaks the link around it.
        let runs = para("[a $[$] b](u)");
        assert!(
            runs.iter()
                .all(|run| run.style.link.as_deref() == Some("u")),
            "{runs:?}"
        );
    }

    #[test]
    fn collapsed_deep_items_keep_word_breaks() {
        let source: String = (0..40)
            .map(|i| format!("{}- item{i}\n", "  ".repeat(i)))
            .collect();
        let tree = format!("{:?}", parse_full(&source));
        assert!(!tree.contains("item15item16"), "{tree}");
    }

    /// Round-7 review: code and shell identifiers, quoted anchors, escaped
    /// percentages, emphasis-crossing pairs, a long run of opening brackets.
    #[test]
    fn identifiers_anchors_and_escaped_percent() {
        for source in [
            "docker run -e TOKEN=$TOKEN -e ID=$ID image",
            "--user=$USER --home=$HOME",
            "DB_URL=$DB_URL, API_KEY=$API_KEY",
            "make CC=$CC CXX=$CXX",
            "x=$a y=$b",
            "use df$col and df$other",
            "this.$refs.input and call this.$emit(e)",
            "grep 'foo$' a.txt && grep 'bar$' b.txt",
            "'^v[0-9]+$' for tags and '^release-.*$'",
            "use \\(.*\\) to: s/\\(.*\\)/",
        ] {
            assert_no_math(source);
        }
        assert_eq!(
            para("rate $50\\%$ up"),
            [plain("rate "), math("50\\%"), plain(" up")]
        );
        assert_eq!(
            para("so $\\&$ too"),
            [plain("so "), math("\\&"), plain(" too")]
        );
        assert_eq!(para("$n$th"), [math("n"), plain("th")]);
        let started = std::time::Instant::now();
        parse_full(&format!(
            "{}{}",
            "(".repeat(64 << 10),
            "$a".repeat(32 << 10)
        ));
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }

    #[test]
    fn display_ranges_stay_in_bounds_while_streaming() {
        for source in ["***o\r\n=", "- [ ] ``\n-", "- [ ] <!--[\n-"] {
            let mut parser = IncrementalParser::new();
            parser.set_text(source);
            for top in &parser.display_tree().blocks {
                assert!(
                    source.get(top.range.clone()).is_some(),
                    "{source:?}: {:?}",
                    top.range
                );
            }
        }
    }

    /// TeX's own `*`, `_` and `|` never read as markdown around formulas,
    /// while code, shell and checkbox spellings stay text.
    #[test]
    fn tex_markers_pair_across_what_looks_like_emphasis() {
        assert_eq!(
            para("$x^*$ and $y^*$"),
            [math("x^*"), plain(" and "), math("y^*")]
        );
        assert_eq!(
            para("$\\bar{x}_{n}$ and $\\bar{y}_{n}$"),
            [math("\\bar{x}_{n}"), plain(" and "), math("\\bar{y}_{n}")]
        );
        assert_eq!(
            para("\\(x^*\\) and \\(y^*\\)"),
            [math("x^*"), plain(" and "), math("y^*")]
        );
        assert_eq!(para("$|x - y| < \\delta$"), [math("|x - y| < \\delta")]);
        assert_eq!(
            para("$n \\in \\mathbb{Z}^+$"),
            [math("n \\in \\mathbb{Z}^+")]
        );
        assert_eq!(
            para("Na$^+$ ions"),
            [plain("Na"), math("^+"), plain(" ions")]
        );
        let runs = para("_$y$_");
        assert!(
            runs.iter().any(|run| run.style.math && run.text == "y"),
            "{runs:?}"
        );
        for source in [
            "Observables like clicks$ and values$ in RxJS",
            "val s = \"$name and $age\"",
            "$(CC) -o $@ $^",
            "for $i<$n; $i++",
            "$obj->$prop",
            "${a}${b}",
            "\\[x\\] Done",
        ] {
            assert_no_math(source);
        }
    }
}

#[cfg(test)]
mod image_model_tests {
    use super::*;
    #[test]
    fn images_preserve_alt_title_position_links_and_empty_alt() {
        let tree =
            parse_full("Before [![**alt**](a.png \"Title\")](next.md) after ![](b.png) ![](b.png)");
        let Block::Paragraph { runs } = &tree.blocks[0].block else {
            panic!("paragraph");
        };
        let images: Vec<_> = runs.iter().filter_map(|r| r.style.image.as_ref()).collect();
        assert_eq!(images.len(), 3);
        assert_eq!(images[0].alt, "alt");
        assert_eq!(images[0].title, "Title");
        assert_eq!(images[0].link.as_deref(), Some("next.md"));
        assert_eq!(images[1].source, "b.png");
        assert!(images[1].alt.is_empty());
        assert_eq!(runs.first().unwrap().text, "Before ");
    }
}
