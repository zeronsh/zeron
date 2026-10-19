//! Mends half-streamed inline markdown for display (after streamdown's
//! `remend`, ported to zeron's incremental parser).
//!
//! While a block is streaming, an unclosed `**bold`, `*em`, `` `code ``,
//! `~~strike` or `[link](partial-url` parses as literal text; the closing
//! marker's arrival makes the marker characters vanish and restyles the run,
//! so wrap points shift and the paragraph's tail visibly reflows. Appending
//! synthetic closers to the *display* parse keeps styling stable from the
//! moment content follows an opener. Only the display tree sees mended text
//! ([`crate::parser::IncrementalParser::display_tree`]); the canonical tree is
//! untouched, so a marker that genuinely never closes settles honestly — one
//! flip when the row completes — instead of jittering throughout.
//!
//! Repairs, in the spirit of remend's handler set (its html/comparison
//! handlers don't apply here — zeron renders raw HTML literally):
//! - emphasis parity: `**a`→`**a**`, `*a`→`*a*`, `_a`/`__a`, `~~a`, nested
//!   closers innermost-first (`**a *b` → `**a *b***`), half-streamed closers
//!   completed (`**a*` → `**a**`);
//! - inline code backtick parity (fence-shaped tails never reach here — the
//!   parser gates mending on the last block not being a code block);
//! - math (`$…$`, `$$…$$`, `\(…\)`, `\[…\]`) shields its TeX like a code
//!   span — `a_1` or `x^*` inside never opens emphasis — but an unclosed
//!   formula is *not* closed: half a formula would render broken, so it stays
//!   literal until its own closer streams in. Only a half-streamed display
//!   closer (`$$x$`) is completed. A display formula also ends any emphasis
//!   or link text opened before it (closing across it would fold the formula
//!   inline). A lone `$` before a digit is a price and one before a shell
//!   variable (`$HOME/bin`) is prose, not openers, and an unclosed inline
//!   formula with no closer ahead shields only its own line;
//! - links/images: the URL never renders — `[text](partial` and a bare
//!   `[text` both mend to `[text](`[`PENDING_LINK_URL`]`)`, so the link text
//!   shows styled immediately and the settling URL can't collapse the line;
//! - a last line of just `-`/`--`/`=`/`==` after text gets a zero-width space
//!   so a streaming list item is never misread as a setext underline (the
//!   paragraph-flashes-to-heading flicker).
//!
//! The scanner is deliberately approximate (streamdown ships the same
//! trade-off): it prefers "stable and close to the final parse" over exact
//! CommonMark delimiter resolution, because any mid-stream misjudgment is
//! repaired by the very next append or by the settle. Known accepted quirks:
//! intraword `**` closes (`2**3` briefly bolds the `3`), and whitespace-only
//! or marker-only content after an opener leaves the opener literal for a
//! chunk until real content arrives.
//!
//! Cost: one pass over the text plus the returned allocation, and `None`
//! (zero further work) whenever nothing hangs. Callers feed it only the last
//! top-level block of the stream, so per-append work stays O(tail) — the same
//! bound as the incremental reparse itself. remend re-scans the entire
//! accumulated message every delta; feeding just the tail is what makes the
//! port fit zeron's parser.

/// Sentinel destination for a link whose URL is still streaming. The renderer
/// styles it like any link but must not register it as clickable.
pub const PENDING_LINK_URL: &str = "zeron:pending-link";

/// The open delimiter runs, innermost last, with each character's runs
/// indexed so a closer finds its opener without scanning the others (a
/// block of `_a ` openers then `a* ` closers stays linear).
#[derive(Default)]
struct Delims {
    list: Vec<OpenDelim>,
    by: [Vec<usize>; 3],
}

impl Delims {
    fn slot(c: char) -> usize {
        match c {
            '*' => 0,
            '_' => 1,
            _ => 2,
        }
    }

    fn push(&mut self, delim: OpenDelim) {
        self.by[Self::slot(delim.ch)].push(self.list.len());
        self.list.push(delim);
    }

    fn last_of(&self, c: char) -> Option<usize> {
        self.by[Self::slot(c)].last().copied()
    }

    fn truncate(&mut self, len: usize) {
        self.list.truncate(len);
        for by in &mut self.by {
            while by.last().is_some_and(|&i| i >= len) {
                by.pop();
            }
        }
    }

    fn clear(&mut self) {
        self.truncate(0);
    }

    /// Drop the runs opened at or after char index `pos`.
    fn retain_before(&mut self, pos: usize) {
        let keep = self.list.partition_point(|d| d.pos < pos);
        self.truncate(keep);
    }
}

/// One unclosed emphasis-family delimiter run (`*`, `_`, or `~~`).
struct OpenDelim {
    ch: char,
    len: usize,
    /// Char index just past the run: nesting order for closers, and the
    /// content-must-follow guard.
    pos: usize,
}

/// Repair hanging inline markers in a streaming block's source. Returns
/// `None` when the text needs no repair (the overwhelmingly common case).
pub fn close_hanging(text: &str) -> Option<String> {
    mend(text, false).0
}

/// [`close_hanging`] that also keeps a trailing lone `$` literal: `None`
/// unless the text ends in one that closes no formula this scan opened
/// (`$5 to $`, `**$5** to **$`, `$HOME and $`) — the parse could still pair
/// it with an earlier dollar and flash the prose between as a formula until
/// the next character shows it's a price. The caller keeps it only where
/// that `$` really closes a formula in the parse.
pub fn close_hanging_dollar(text: &str) -> Option<String> {
    if !text.ends_with('$') {
        return None;
    }
    match mend(text, true) {
        (Some(mended), true) => Some(mended),
        _ => None,
    }
}

/// The mended text (`None` when nothing changes), and whether the text ends
/// in an ambiguous lone `$` (escaped in the result when `escape_dollar`).
fn mend(text: &str, escape_dollar: bool) -> (Option<String>, bool) {
    let cs: Vec<(usize, char)> = text.char_indices().collect();
    let n = cs.len();
    let at = |i: usize| cs.get(i).map(|&(_, c)| c);

    let mut delims = Delims::default();
    // Char indices of unmatched `[`.
    let mut brackets: Vec<usize> = Vec::new();
    // Open inline code span: (backtick run length, content char index).
    let mut code: Option<(usize, usize)> = None;
    // Char index of the last substantive character — content that justifies
    // closing an opener (not whitespace, not a bare marker).
    let mut last_content: Option<usize> = None;
    // Char index of the `]` of a `](…` whose URL runs off the end.
    let mut pending_url: Option<usize> = None;
    // Open math: (dollar run length, 1 inline or 2 display; content char
    // index). Shields its content and is never closed synthetically.
    let mut math: Option<(usize, usize)> = None;
    // Char index of a lone `$` ending open display math — the first half of
    // its closing `$$`.
    let mut half_closer: Option<usize> = None;
    // Open `\(…` or `\[…` formula: the char that closes it after a `\`.
    // Shields like `math`, and is never closed either.
    let mut latex: Option<char> = None;
    // Char index of the `$` that last closed an inline formula.
    let mut inline_closed: Option<usize> = None;
    // The last places an inline formula could still close: an unclosed one
    // with no closer ahead shields only its own line.
    let last_dollar = cs
        .iter()
        .enumerate()
        .rposition(|(j, &(_, c))| c == '$' && j > 0 && !cs[j - 1].1.is_whitespace());
    let last_paren = cs.windows(2).rposition(|w| w[0].1 == '\\' && w[1].1 == ')');
    let last_bracket = cs.windows(2).rposition(|w| w[0].1 == '\\' && w[1].1 == ']');
    // Whether TeX syntax follows each char: an unclosed `\[` streams a
    // display formula only if what follows reads as one.
    let mut tex_after = vec![false; n + 1];
    for j in (0..n).rev() {
        tex_after[j] = tex_after[j + 1] || matches!(cs[j].1, '\\' | '^' | '_' | '{' | '}' | '=');
    }
    // Whether the word (since the last whitespace) up to each char holds a
    // `/`: the parser reads an opener inside a URL or path as an escape.
    let mut slash = false;
    let word_slash: Vec<bool> = cs
        .iter()
        .map(|&(_, c)| {
            if c.is_whitespace() {
                slash = false;
            } else if c == '/' {
                slash = true;
            }
            slash
        })
        .collect();

    let mut i = 0;
    while i < n {
        let c = cs[i].1;
        if code.is_none() && c == '\\' {
            let next = at(i + 1);
            if let Some(closer) = latex {
                if next == Some(closer) {
                    latex = None;
                }
            } else if math.is_none()
                && matches!(next, Some('(' | '['))
                && !i.checked_sub(1).is_some_and(|p| glued_to_word(cs[p].1, word_slash[p]))
                // A `\[` with no `\]` ahead and no TeX after it is an
                // escape (`Escape \[ in regex`), not a formula still
                // streaming.
                && (next == Some('(')
                    || last_bracket.is_some_and(|last| last > i)
                    || tex_after[(i + 2).min(n)])
            {
                // The parser reads `\(…\)` / `\[…\]` as math (and a
                // display formula ends emphasis opened before it, like `$$`).
                if next == Some('[') {
                    delims.clear();
                    brackets.clear();
                }
                latex = Some(if next == Some('(') { ')' } else { ']' });
            }
            // Escaped char: both literal; the escapee still counts as content.
            if i + 1 < n {
                last_content = Some(i + 1);
            }
            half_closer = None;
            i += 2;
            continue;
        }
        if let Some(closer) = latex {
            if c == '\n' && closer == ')' && last_paren.is_none_or(|last| last < i) {
                latex = None;
            } else if !c.is_whitespace() {
                last_content = Some(i);
            }
            i += 1;
            continue;
        }
        if let Some((open, _)) = math {
            if c == '\n' && open == 1 && last_dollar.is_none_or(|last| last < i) {
                math = None;
                i += 1;
                continue;
            }
            if c == '$' {
                let run = run_len(&cs, i);
                // Inline math closes on a `$` glued to its content; display
                // math on `$$` wherever it falls.
                let glued = i > 0 && !cs[i - 1].1.is_whitespace();
                if run >= open && (open == 2 || glued) {
                    math = None;
                    if open == 1 {
                        inline_closed = Some(i);
                    }
                } else if open == 2 && run == 1 {
                    half_closer = Some(i);
                }
                last_content = Some(i + run - 1);
                i += run;
                continue;
            }
            if !c.is_whitespace() {
                half_closer = None;
                last_content = Some(i);
            }
            i += 1;
            continue;
        }
        if c == '`' {
            let run = run_len(&cs, i);
            match code {
                // A span closes only on a run of the opening length.
                Some((open, _)) if run == open => code = None,
                Some(_) => last_content = Some(i + run - 1),
                None => code = Some((run, i + run)),
            }
            i += run;
            continue;
        }
        if code.is_some() {
            last_content = Some(i);
            i += 1;
            continue;
        }
        match c {
            '$' => {
                let run = run_len(&cs, i);
                let next = at(i + run);
                if run >= 2 {
                    // Emphasis and link text opened before a display formula
                    // don't reach past it.
                    delims.clear();
                    brackets.clear();
                    math = Some((2, i + run));
                } else if next.is_some_and(|c| !c.is_whitespace() && !c.is_ascii_digit())
                    && !shell_variable(&cs[i + 1..])
                {
                    math = Some((1, i + run));
                }
                last_content = Some(i + run - 1);
                i += run;
            }
            '*' | '_' | '~' => {
                let run = run_len(&cs, i);
                delim(&mut delims, &cs, c, run, i, &mut last_content);
                i += run;
            }
            '[' => {
                brackets.push(i);
                i += 1;
            }
            ']' => {
                if let Some(open) = brackets.pop() {
                    // Emphasis opened inside a completed `[…]` and never
                    // closed there stays literal — as the final parse will
                    // decide too.
                    delims.retain_before(open);
                    if at(i + 1) == Some('(') {
                        // Consume the URL through its balanced `)`.
                        let mut j = i + 2;
                        let mut depth = 0usize;
                        loop {
                            match at(j) {
                                Some('(') => depth += 1,
                                Some(')') if depth == 0 => break,
                                Some(')') => depth -= 1,
                                Some(_) => {}
                                None => {
                                    pending_url = Some(i);
                                    break;
                                }
                            }
                            j += 1;
                        }
                        if pending_url.is_some() {
                            break;
                        }
                        last_content = Some(j);
                        i = j + 1;
                        continue;
                    }
                }
                last_content = Some(i);
                i += 1;
            }
            c if c.is_whitespace() => i += 1,
            _ => {
                last_content = Some(i);
                i += 1;
            }
        }
    }

    // Text ends inside a link/image URL: drop the partial URL, keep the text.
    if let Some(close) = pending_url {
        let byte = cs[close].0;
        return (
            Some(format!("{}]({PENDING_LINK_URL})", &text[..byte])),
            false,
        );
    }

    // Collect closers innermost-first (descending open position). A `[` open
    // splits the emphasis closers naturally: delimiters opened inside the
    // link text close before the `](…)`, ones opened before it close after.
    let mut pending: Vec<(usize, String)> = Vec::new();
    if let (Some((2, mpos)), Some(_)) = (math, half_closer) {
        pending.push((mpos, "$".into()));
    }
    if let Some((ticks, cpos)) = code
        && last_content.is_some_and(|lc| lc >= cpos)
    {
        pending.push((cpos, "`".repeat(ticks)));
    }
    for d in &delims.list {
        if last_content.is_some_and(|lc| lc >= d.pos) {
            pending.push((d.pos, d.ch.to_string().repeat(d.len)));
        }
    }
    if let Some(&open) = brackets.last()
        && last_content.is_some_and(|lc| lc > open)
    {
        pending.push((open, format!("]({PENDING_LINK_URL})")));
    }
    pending.sort_by_key(|b| std::cmp::Reverse(b.0));
    let closers: String = pending.into_iter().map(|(_, s)| s).collect();

    // A lone `$` ending the stream that closes no formula this scan opened
    // (see [`close_hanging_dollar`]). A formula that does close there (`$x$`)
    // renders at once.
    let trailing_dollar = n > 0
        && cs[n - 1].1 == '$'
        && (n < 2 || cs[n - 2].1 != '$')
        && cs[..n - 1]
            .iter()
            .rev()
            .take_while(|&&(_, c)| c == '\\')
            .count()
            % 2
            == 0
        && code.is_none()
        && math.is_none()
        && latex.is_none()
        && inline_closed != Some(n - 1);
    let escape = trailing_dollar && escape_dollar;
    let text: std::borrow::Cow<str> = if escape {
        let at = cs[n - 1].0;
        format!("{}\\$", &text[..at]).into()
    } else {
        text.into()
    };
    let text = text.as_ref();

    // A trailing line of only `-`/`--`/`=`/`==` under text is a setext
    // underline (or hr) to the parser but almost always a streaming list
    // item; a zero-width space breaks the reading invisibly until the next
    // characters decide. (Streamdown's setext handler, verbatim trick.)
    let setext = setext_partial(text);

    if closers.is_empty() && !setext {
        return (escape.then(|| text.to_owned()), trailing_dollar);
    }
    if setext {
        // Closers belong to the paragraph content above the underline line.
        let nl = text.rfind('\n');
        let mended = match (nl, closers.is_empty()) {
            (Some(nl), false) => {
                format!("{}{closers}{}\u{200B}", &text[..nl], &text[nl..])
            }
            _ => format!("{text}\u{200B}"),
        };
        return (Some(mended), trailing_dollar);
    }
    // Insert before trailing whitespace: a closer after a trailing space is
    // not right-flanking and would not close (remend trims the space for the
    // same reason).
    let end = text.trim_end().len();
    (
        Some(format!("{}{closers}{}", &text[..end], &text[end..])),
        trailing_dollar,
    )
}

/// Whether the text after a `$` spells a shell variable rather than a
/// formula: an all-caps name (`$HOME`, `$PATH_2`) that ends the text or runs
/// into a space or a joining character, where `$ABC$` would close a formula.
fn shell_variable(after: &[(usize, char)]) -> bool {
    let name = after
        .iter()
        .take_while(|&&(_, c)| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        .count();
    name > 0
        && !after[0].1.is_ascii_digit()
        && after.get(name).is_none_or(|&(_, c)| {
            c.is_whitespace() || matches!(c, '/' | ':' | '@' | '.' | ',' | ';' | '\'' | '"' | ')')
        })
}

/// A character an escaped `\(` or `\[` glued to is markdown escaping, not
/// math (mirrors the parser's rewrite).
/// Whether a `\(` after `c` is glued to it, as the parser decides: after a
/// word character, or any non-space inside a URL or path (`in_path`).
fn glued_to_word(c: char, in_path: bool) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || (in_path && !c.is_whitespace())
}

fn run_len(cs: &[(usize, char)], i: usize) -> usize {
    let c = cs[i].1;
    cs[i..].iter().take_while(|&&(_, x)| x == c).count()
}

/// Match or open one delimiter run. Closes against the innermost same-char
/// opener (partially, for half-streamed closers like `**a*`); delimiters
/// opened after a consumed opener were inside the closed span and stay
/// literal, exactly as the final parse treats them.
fn delim(
    delims: &mut Delims,
    cs: &[(usize, char)],
    c: char,
    run: usize,
    i: usize,
    last_content: &mut Option<usize>,
) {
    let end = i + run;
    // GFM strikethrough is `~~` only; longer tilde runs are literal. A run of
    // one may still be the half-streamed first tilde of a closing `~~`, so it
    // proceeds to the matcher (but never opens).
    if c == '~' && run > 2 {
        *last_content = Some(end - 1);
        return;
    }
    let prev = i.checked_sub(1).map(|p| cs[p].1);
    let next = cs.get(end).map(|&(_, c)| c);
    let word = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
    // Intraword `_` never delimits (CommonMark); intraword single `*` is
    // treated the same, conservatively — `2*3` must not flash italic.
    if word(prev) && word(next) && (c == '_' || (c == '*' && run == 1)) {
        *last_content = Some(end - 1);
        return;
    }
    let can_close = prev.is_some_and(|c| !c.is_whitespace());
    let can_open = next.is_some_and(|c| !c.is_whitespace());
    let mut rest = run;
    if can_close && let Some(k) = delims.last_of(c) {
        let take = rest.min(delims.list[k].len);
        delims.list[k].len -= take;
        rest -= take;
        let keep = if delims.list[k].len == 0 { k } else { k + 1 };
        delims.truncate(keep);
    }
    if rest > 0 {
        // A lone `~` never opens; `~` entries of len 1 exist only as the
        // remainder of a half-streamed closer.
        if can_open && (c != '~' || rest == 2) {
            delims.push(OpenDelim {
                ch: c,
                len: rest,
                pos: end,
            });
        } else {
            *last_content = Some(end - 1);
        }
    }
}

/// Last line is only 1–2 `-` or `=` (no trailing whitespace) under a
/// non-empty line — the incomplete-setext ambiguity.
fn setext_partial(text: &str) -> bool {
    let Some(nl) = text.rfind('\n') else {
        return false;
    };
    let last = &text[nl + 1..];
    let trimmed = last.trim_start();
    let underline =
        |c: char| !trimmed.is_empty() && trimmed.len() <= 2 && trimmed.chars().all(|x| x == c);
    (underline('-') || underline('='))
        && text[..nl]
            .lines()
            .last()
            .is_some_and(|l| !l.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[track_caller]
    fn mends(input: &str, expected: &str) {
        assert_eq!(close_hanging(input).as_deref(), Some(expected), "{input:?}");
    }

    #[track_caller]
    fn stays(input: &str) {
        assert_eq!(close_hanging(input), None, "{input:?}");
    }

    #[test]
    fn balanced_text_needs_nothing() {
        stays("plain words, no markers");
        stays("a **b** and *c* and `d` and ~~e~~");
        stays("[docs](https://x.dev) done");
        stays("");
    }

    #[test]
    fn bold_and_italic_close() {
        mends("**bold", "**bold**");
        mends("some *em", "some *em*");
        mends("a __b", "a __b__");
        mends("a _b", "a _b_");
        mends("***both", "***both***");
    }

    #[test]
    fn half_streamed_closers_complete() {
        mends("**bold*", "**bold**");
        mends("__b_", "__b__");
        mends("~~gone~", "~~gone~~");
    }

    #[test]
    fn nested_closers_come_innermost_first() {
        mends("**a *b", "**a *b***");
        mends("*a **b", "*a **b***");
        mends("_a **b", "_a **b**_");
    }

    #[test]
    fn bare_openers_stay_literal_until_content() {
        stays("**");
        stays("text **");
        stays("text ** ");
        stays("*");
        stays("~~");
        stays("`");
    }

    #[test]
    fn closers_insert_before_trailing_whitespace() {
        // `**bold **` would be left-flanking only — it must mend to
        // `**bold** ` to actually close.
        mends("**bold ", "**bold** ");
        mends("*em\n", "*em*\n");
    }

    #[test]
    fn intraword_and_escapes_are_literal() {
        stays("2*3 equals 6");
        stays("snake_case_name");
        stays("20~25 degrees");
        stays(r"\*not emphasis");
        stays(r"a \** b");
    }

    #[test]
    fn list_markers_are_not_openers() {
        stays("* item one");
        stays("- a\n* b");
    }

    #[test]
    fn strikethrough_closes() {
        mends("~~gone", "~~gone~~");
        stays("~single~x"); // run of 1 is literal
    }

    #[test]
    fn inline_code_closes_and_shields_markers() {
        mends("`code", "`code`");
        mends("call `a ** b", "call `a ** b`");
        mends("``a`", "``a```"); // shorter run is span content, not a closer
        stays("`done` after");
    }

    #[test]
    fn math_shields_markers_and_is_never_closed() {
        // Unclosed formulas stay literal: no synthetic `$`.
        stays("so $x_{1} + b");
        stays("so $$\\sum_{i} x_i^*");
        // Markers inside a closed formula don't open anything.
        stays("so $a_1 * b_2$ done");
        stays("$$x_*$$ and $y_1$");
        // Emphasis after a formula still mends.
        mends("$a_1$ then **bo", "$a_1$ then **bo**");
        // Escaped dollars and dollars in code are not math.
        mends("\\$5 **bo", "\\$5 **bo**");
        mends("`$x` **bo", "`$x` **bo**");
    }

    #[test]
    fn math_prices_and_half_display_closers() {
        // A lone `$` before a digit reads as a price and shields nothing.
        mends("costs $5 and **bo", "costs $5 and **bo**");
        // `$$x$` is a display formula whose closer is half streamed.
        mends("$$x^2$", "$$x^2$$");
        mends("$$\nx_1\n$", "$$\nx_1\n$$");
        stays("$$x$$");
    }

    #[test]
    fn shell_variables_open_no_formula() {
        mends(
            "set $HOME and [docs](https://x.dev/lo",
            "set $HOME and [docs](zeron:pending-link)",
        );
        mends("echo $PATH:$HOME/bin **bo", "echo $PATH:$HOME/bin **bo**");
        mends("'$USER@$HOST' then *em", "'$USER@$HOST' then *em*");
        // All-caps formulas still shield.
        stays("so $ABC_*");
        stays("so $X_1$ and $A_i *b");
    }

    #[test]
    fn unclosed_inline_math_shields_only_its_line() {
        mends("cost $x_1\n**bold", "cost $x_1\n**bold**");
        // A closer further down keeps the formula open across the line.
        stays("so $x\n_1$ done");
        stays("so \\(x\n_1\\) done");
        mends("so \\(x_1\nthen **bo", "so \\(x_1\nthen **bo**");
        // Display math stays shielded to the end.
        stays("$$\nx_1 *\ny");
    }

    #[test]
    fn latex_delimiters_shield_like_dollars() {
        mends("\\(x^*\\) then **bo", "\\(x^*\\) then **bo**");
        mends("\\[x_*\\] then *em", "\\[x_*\\] then *em*");
        stays("so \\(a_1 * b");
        stays("so \\[\\sum_{i} x_i^*");
        // A display formula ends emphasis opened before it.
        stays("**a \\[x\\] b");
        // Escapes glued to a word are not math.
        mends("f\\(x\\) **bo", "f\\(x\\) **bo**");
        mends("arr\\[0\\] *em", "arr\\[0\\] *em*");
        // An unclosed `\[` before prose is an escape: emphasis still mends.
        mends(
            "Escape \\[ in regex, **bold",
            "Escape \\[ in regex, **bold**",
        );
        // Glued as the parser glues: after an operator it opens a formula.
        mends("x=\\(*a\\) **bold", "x=\\(*a\\) **bold**");
    }

    #[test]
    fn display_math_ends_emphasis_opened_before_it() {
        stays("**a $$x$$ b");
        mends("**a $$x$$ *b", "**a $$x$$ *b*");
    }

    #[test]
    fn links_mend_to_pending_sentinel() {
        mends("[docs](https://x.dev/lo", "[docs](zeron:pending-link)");
        mends("[docs](", "[docs](zeron:pending-link)");
        mends("see [do", "see [do](zeron:pending-link)");
        mends("![alt](https://x/i.p", "![alt](zeron:pending-link)");
        stays("see [");
        stays("[x] task-like"); // completed bracket, no url
    }

    #[test]
    fn link_urls_allow_nested_parens() {
        stays("[a](https://x.dev/(y)) done");
        mends("[a](https://x.dev/(y", "[a](zeron:pending-link)");
    }

    #[test]
    fn emphasis_inside_link_text_closes_inside() {
        mends("[**a", "[**a**](zeron:pending-link)");
        mends("**a [b", "**a [b](zeron:pending-link)**");
    }

    #[test]
    fn emphasis_unclosed_in_completed_bracket_is_dropped() {
        stays("[**a] done");
    }

    #[test]
    fn setext_partials_get_zero_width_space() {
        mends("para\n-", "para\n-\u{200B}");
        mends("para\n--", "para\n--\u{200B}");
        mends("para\n=", "para\n=\u{200B}");
        stays("para\n---"); // real hr / settled setext
        stays("-"); // no line above
        stays("\n-"); // empty line above
        mends("**b\n-", "**b**\n-\u{200B}"); // closers go above the underline
    }
}
