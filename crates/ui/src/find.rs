//! Find-in-thread state and matching for the transcript.
//!
//! Matches are counted per rendered row (user text, markdown blocks) and
//! stepped occurrence by occurrence; the painters wash them inline.

use gpui::{Entity, FocusHandle, SharedString, Subscription};

use crate::composer::ComposerInput;
use crate::markdown::{Block, InlineRun};

/// The find field's height, matching the popover rows' card rhythm.
pub(crate) const BAR_HEIGHT: f32 = 38.0;

/// The open find bar's input, query and match cursor.
pub(crate) struct FindState {
    pub(crate) input: Entity<ComposerInput>,
    pub(crate) query: String,
    /// Every occurrence as `(row index, nth match in that row)`, in
    /// transcript order.
    pub(crate) matches: Vec<(usize, usize)>,
    /// Index into [`Self::matches`], or `None` while nothing matches.
    pub(crate) current: Option<usize>,
    pub(crate) previous_focus: Option<FocusHandle>,
    pub(crate) focus_pending: bool,
    /// Jump to the first match once one loads (opened from global search
    /// before the chat's rows arrived).
    pub(crate) jump_pending: bool,
    pub(crate) _edited: Subscription,
}

impl FindState {
    /// `i/n` for the bar; `0/0` while nothing matches.
    pub(crate) fn count_label(&self) -> String {
        match self.current.filter(|_| !self.matches.is_empty()) {
            Some(ix) => format!("{}/{}", ix + 1, self.matches.len()),
            None => "0/0".to_string(),
        }
    }
}

/// Every occurrence as `(row index, nth in row)`, given each row's match
/// count.
pub(crate) fn occurrences(counts: impl IntoIterator<Item = usize>) -> Vec<(usize, usize)> {
    counts
        .into_iter()
        .enumerate()
        .flat_map(|(row, n)| (0..n).map(move |nth| (row, nth)))
        .collect()
}

/// Matches of `query` in a rendered markdown block, fragment by fragment the
/// way the painters wash them (a match never spans two fragments).
pub(crate) fn block_count(block: &Block, query: &str) -> usize {
    let runs = |runs: &[InlineRun]| {
        let text: String = runs.iter().map(|run| run.text.as_str()).collect();
        match_ranges(&text, query).len()
    };
    match block {
        Block::Paragraph { runs: r } | Block::Heading { runs: r, .. } => runs(r),
        Block::CodeBlock { code, .. } => code
            .lines()
            .map(|line| match_ranges(line, query).len())
            .sum(),
        Block::BlockQuote { children } => children.iter().map(|b| block_count(b, query)).sum(),
        Block::List { items, .. } => items.iter().flatten().map(|b| block_count(b, query)).sum(),
        Block::Table { header, rows, .. } => header
            .iter()
            .chain(rows.iter().flatten())
            .map(|cell| runs(cell))
            .sum(),
        Block::Math { .. } | Block::Rule => 0,
    }
}

/// Byte range of `needle` in `text`, matched case-insensitively per char so the
/// returned offsets always fall on char boundaries.
pub(crate) fn char_range(text: &str, needle: &str) -> Option<(usize, usize)> {
    let want: Vec<char> = needle.trim().to_lowercase().chars().collect();
    want.first()?;
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    for (i, &(start, _)) in chars.iter().enumerate() {
        let mut matched = true;
        for (offset, expected) in want.iter().enumerate() {
            match chars.get(i + offset) {
                Some(&(_, found)) if found.to_lowercase().next() == Some(*expected) => {}
                _ => {
                    matched = false;
                    break;
                }
            }
        }
        if matched {
            let end = chars
                .get(i + want.len())
                .map(|&(byte, _)| byte)
                .unwrap_or(text.len());
            return Some((start, end));
        }
    }
    None
}

/// Matches of `query` in `text` as byte ranges, left to right.
pub(crate) fn match_ranges(text: &str, query: &str) -> Vec<std::ops::Range<usize>> {
    let mut ranges = Vec::new();
    let mut from = 0;
    while from < text.len() {
        let Some((start, end)) = char_range(&text[from..], query) else {
            break;
        };
        let (start, end) = (from + start, from + end);
        if end <= start {
            break;
        }
        ranges.push(start..end);
        from = end;
    }
    ranges
}

thread_local! {
    /// The open find bar's query, read by the painters so matches can be
    /// washed inline. Only one transcript is visible per window.
    static ACTIVE_QUERY: std::cell::RefCell<Option<String>> =
        const { std::cell::RefCell::new(None) };
    /// The current occurrence: `(row id, nth match in that row)`.
    static CURRENT: std::cell::RefCell<Option<(SharedString, usize)>> =
        const { std::cell::RefCell::new(None) };
    /// Matches handed out so far in the row being built, so each fragment
    /// knows the in-row ordinal of its first match.
    static ORDINAL: std::cell::RefCell<(String, usize)> =
        const { std::cell::RefCell::new((String::new(), 0)) };
}

pub(crate) fn set_active_query(query: Option<&str>) {
    ACTIVE_QUERY.with(|slot| {
        *slot.borrow_mut() = query
            .map(str::trim)
            .filter(|query| !query.is_empty())
            .map(str::to_owned);
    });
}

pub(crate) fn active_query() -> Option<String> {
    ACTIVE_QUERY.with(|slot| slot.borrow().clone())
}

pub(crate) fn set_current(current: Option<(SharedString, usize)>) {
    CURRENT.with(|slot| *slot.borrow_mut() = current);
}

/// Restart in-row ordinals: call before building a row's elements.
pub(crate) fn begin_row(row: &str) {
    ORDINAL.with(|slot| {
        let mut slot = slot.borrow_mut();
        slot.0.clear();
        slot.0.push_str(row);
        slot.1 = 0;
    });
}

/// The active query's matches in one painted fragment of `row`, each flagged
/// when it is the current occurrence. Fragments must be built in document
/// order after [`begin_row`].
// ponytail: ordinals follow construction order; text laid out lazily (link
// truncation) can mark the wrong match current, key ordinals per fragment if so.
pub(crate) fn washes(row: &str, text: &str) -> Vec<(std::ops::Range<usize>, bool)> {
    let Some(query) = active_query() else {
        return Vec::new();
    };
    let ranges = match_ranges(text, &query);
    if ranges.is_empty() {
        return Vec::new();
    }
    let first = ORDINAL.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.0 != row {
            slot.0 = row.to_owned();
            slot.1 = 0;
        }
        let first = slot.1;
        slot.1 += ranges.len();
        first
    });
    let current = CURRENT.with(|slot| {
        slot.borrow()
            .as_ref()
            .filter(|(id, _)| id.as_ref() == row)
            .map(|(_, nth)| *nth)
    });
    ranges
        .into_iter()
        .enumerate()
        .map(|(i, range)| (range, current == Some(first + i)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{char_range, match_ranges, occurrences};

    #[test]
    fn occurrences_expand_counts_in_row_order() {
        assert_eq!(occurrences([1, 0, 2]), vec![(0, 0), (2, 0), (2, 1)]);
        assert!(occurrences([0, 0]).is_empty());
    }

    #[test]
    fn matches_keep_char_boundaries() {
        assert_eq!(char_range("naïve MAIN", "main"), Some((9, 13)));
        assert!(char_range("nothing", "main").is_none());
        assert_eq!(match_ranges("main and MAIN", "main").len(), 2);
    }
}
