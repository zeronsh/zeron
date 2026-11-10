//! Markdown's math, read before markdown reads it: ds/math_source.luau's
//! read, in Rust, for a reply streaming in. Outside code (fences and inline
//! code), `$$tex$$` becomes a ```math block and inline `$tex$` inline code
//! showing `$tex$`, by pulldown-cmark's rules; `\$` is a dollar; a `$$`
//! still open is left as it is until it closes. Byte for byte what the
//! script's `prepare` makes (tests/prose_pixels and this file's tests hold
//! them to it).
//!
//! A reply's words grow at their end: [`Prepared`] reads each chunk on from
//! where the last read could go on (as the script's `Scan` does), so a chunk
//! costs what's new, and says what the prepared text gained, to be appended
//! to the state a markdown view reads, or set whole when what it made
//! before changed (a `$x$` closing into code).
//!
//! (Only what the read makes is kept here: the script's scan also marks
//! where the text can be parsed in two, which only its own markdown reads.)

const BACKTICK: u8 = b'`';
const TILDE: u8 = b'~';
const DOLLAR: u8 = b'$';
const SLASH: u8 = b'\\';
const NEWLINE: u8 = b'\n';
const BRACKET: u8 = b'[';

/// Lua's `%s`.
fn space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

fn blank(b: Option<u8>) -> bool {
    matches!(b, None | Some(b' ' | b'\t' | b'\n'))
}

fn digit(b: Option<u8>) -> bool {
    b.is_some_and(|b| b.is_ascii_digit())
}

/// The bytes the read stops at; any other is copied as it is.
fn special(b: u8) -> bool {
    matches!(b, b'\n' | b'\\' | b'[' | b'`' | b'$')
}

/// `tex` as inline code (its fence longer than any run of backticks in it).
fn inline_code(tex: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for b in tex.bytes() {
        if b == BACKTICK {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    let fence = "`".repeat(longest + 1);
    format!("{fence}${tex}${fence}")
}

/// A text read by 1-based places, as the script reads it.
struct Text<'a> {
    s: &'a str,
    b: &'a [u8],
}
impl<'a> Text<'a> {
    fn new(s: &'a str) -> Self {
        Self { s, b: s.as_bytes() }
    }
    fn n(&self) -> usize {
        self.b.len()
    }
    /// The byte at `at` (from 1), if there is one.
    fn at(&self, at: usize) -> Option<u8> {
        at.checked_sub(1).and_then(|i| self.b.get(i).copied())
    }
    /// Bytes `from` to `to` (from 1, inclusive), as `string.sub`.
    fn sub(&self, from: usize, to: usize) -> &'a str {
        let to = to.min(self.n());
        if from == 0 || from > to {
            return "";
        }
        &self.s[from - 1..to]
    }
    /// Where `needle` is from `from` on (plain `string.find`).
    fn find(&self, needle: &[u8], from: usize) -> Option<usize> {
        let from = from.max(1);
        if from > self.n() + 1 || needle.is_empty() {
            return None;
        }
        self.b[from - 1..]
            .windows(needle.len())
            .position(|w| w == needle)
            .map(|p| p + from)
    }
    /// The first byte from `from` on that `is` says.
    fn find_by(&self, from: usize, is: impl Fn(u8) -> bool) -> Option<usize> {
        let from = from.max(1);
        if from > self.n() {
            return None;
        }
        self.b[from - 1..].iter().position(|&b| is(b)).map(|p| p + from)
    }
    /// How many of `b` run from `at`.
    fn run(&self, at: usize, b: u8, most: usize) -> usize {
        let mut k = 0;
        while k < most && self.at(at + k) == Some(b) {
            k += 1;
        }
        k
    }
    /// `^%[[^%]\n]*%]%(zeron%-[%a]+:[^%)%s]*%)` at `at`: its length.
    fn zeron_link(&self, at: usize) -> Option<usize> {
        if self.at(at) != Some(BRACKET) {
            return None;
        }
        let mut k = at + 1;
        while let Some(b) = self.at(k) {
            if b == b']' || b == b'\n' {
                break;
            }
            k += 1;
        }
        if self.at(k) != Some(b']') {
            return None;
        }
        k += 1;
        for &want in b"(zeron-" {
            if self.at(k) != Some(want) {
                return None;
            }
            k += 1;
        }
        let letters = k;
        while self.at(k).is_some_and(|b| b.is_ascii_alphabetic()) {
            k += 1;
        }
        if k == letters || self.at(k) != Some(b':') {
            return None;
        }
        k += 1;
        while let Some(b) = self.at(k) {
            if b == b')' || space(b) {
                break;
            }
            k += 1;
        }
        if self.at(k) != Some(b')') {
            return None;
        }
        Some(k + 1 - at)
    }
}

/// `^ ? ? ?(`+)%s*$` (or `~`) of a line: its closing run's length.
fn close_run(line: &[u8], mark: u8) -> usize {
    let lead = line.iter().take(3).take_while(|&&b| b == b' ').count();
    let run = line[lead..].iter().take_while(|&&b| b == mark).count();
    if run == 0 || !line[lead + run..].iter().all(|&b| space(b)) {
        return 0;
    }
    run
}

/// `^%s*(.-)%s*$`.
fn trimmed(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_ascii() && space(c as u8))
}

/// Inline code left unclosed in a paragraph that's over: its run's length,
/// and where looking for its close goes on.
#[derive(Clone, Copy, Debug)]
struct Tick {
    run: usize,
    next: usize,
}

/// A fence still open at the end of the text: where it starts, its mark
/// and run, and its last whole line's end.
#[derive(Clone, Copy, Debug)]
struct Fence {
    i: usize,
    mark: u8,
    run: usize,
    at: usize,
}

/// A read of a text as it grows (ds/math_source's `Scan`): what it made up
/// to the last place it can go on from (`settled`), and past it (`tail`).
#[derive(Debug)]
pub struct Scan {
    seen: usize,
    settled: String,
    /// Whether the last piece made ends a line (none made: `None`).
    last_nl: Option<bool>,
    tail: String,
    i: usize,
    from: usize,
    ticks: Vec<Tick>,
    fence: Option<Fence>,
}

impl Default for Scan {
    fn default() -> Self {
        Self {
            seen: 0,
            settled: String::new(),
            last_nl: None,
            tail: String::new(),
            i: 1,
            from: 1,
            ticks: Vec::new(),
            fence: None,
        }
    }
}

impl Scan {
    /// What it made of the text read: the text, prepared.
    pub fn made(&self) -> String {
        let mut out = String::with_capacity(self.settled.len() + self.tail.len());
        out.push_str(&self.settled);
        out.push_str(&self.tail);
        out
    }

    /// Whether inline code left unclosed before where the read goes on is
    /// still unclosed with `src`, each looking on from where it stopped.
    fn still_unclosed(&mut self, src: &Text) -> bool {
        let seen = self.seen;
        for t in &mut self.ticks {
            let run = "`".repeat(t.run);
            let mut next = t.next.max((seen + 2).saturating_sub(t.run));
            let mut close = src.find(run.as_bytes(), next);
            while let Some(c) = close
                && src.at(c + t.run) == Some(BACKTICK)
            {
                next = c + t.run + 1;
                close = src.find(run.as_bytes(), next);
            }
            if close.is_some() {
                return false;
            }
            t.next = next;
        }
        true
    }

    /// Read `src`, the text as it is now. `extends`: it goes on from the
    /// last read's (what was read is its start), so it is read on from
    /// where that read could go on; any other is read from its start.
    /// Whether it started over.
    pub fn feed(&mut self, src: &str, extends: bool) -> bool {
        let text = Text::new(src);
        let over = !extends || src.len() < self.seen || !self.still_unclosed(&text);
        if over {
            *self = Self::default();
        }
        self.seen = src.len();
        self.read(&text);
        over
    }

    fn read(&mut self, src: &Text) {
        let n = src.n();
        // The pieces made since the last place it can go on from.
        let mut pending: Vec<String> = Vec::new();
        let (mut i, mut from) = (self.i, self.from);
        let mut nt = self.ticks.len();
        let was_fence = self.fence;
        let mut fence: Option<Fence> = None;
        let mut last_nl = self.last_nl;
        // The furthest byte looked at by what was decided so far.
        let mut reach = 0usize;
        // The last place it can go on from.
        let (mut ci, mut cfrom, mut cnt, mut clast) = (i, from, nt, last_nl);
        let push = |pending: &mut Vec<String>, last_nl: &mut Option<bool>, piece: String| {
            *last_nl = Some(piece.ends_with('\n'));
            pending.push(piece);
        };
        loop {
            // (A reach past the end: what was decided waits on text not there yet.)
            if reach < i && reach <= n {
                ci = i;
                cfrom = from;
                cnt = nt;
                clast = last_nl;
                for piece in pending.drain(..) {
                    self.settled.push_str(&piece);
                }
            }
            if i > n {
                break;
            }
            let b = src.at(i).unwrap_or(0);
            if i == 1 || src.at(i - 1) == Some(NEWLINE) {
                // A fence (up to three spaces in): copied as it is, to its close.
                let lead = src.run(i, b' ', 3);
                let mark = src.at(i + lead);
                reach = reach.max(i + lead);
                if let Some(mark @ (BACKTICK | TILDE)) = mark {
                    let run = src.run(i + lead, mark, usize::MAX);
                    reach = reach.max(i + lead + run);
                    if run >= 3 {
                        let mut at = match was_fence {
                            // (Its lines up to the last whole one aren't its close.)
                            Some(open) if open.i == i && open.mark == mark && open.run == run => {
                                Some(open.at)
                            }
                            _ => src.find(b"\n", i),
                        };
                        let mut last = at;
                        while let Some(a) = at {
                            last = Some(a);
                            let end = src.find(b"\n", a + 1).unwrap_or(n + 1);
                            let line = &src.b[a.min(n)..(end - 1).min(n)];
                            if close_run(line, mark) >= run {
                                at = Some(a + line.len() + 1);
                                break;
                            }
                            at = src.find(b"\n", a + 1);
                        }
                        match at {
                            Some(a) => {
                                reach = reach.max(a);
                                i = a;
                            }
                            None => {
                                reach = n + 1;
                                if let Some(last) = last {
                                    fence = Some(Fence { i, mark, run, at: last });
                                }
                                i = n + 1;
                            }
                        }
                        continue;
                    }
                }
            }
            let flush = |pending: &mut Vec<String>, last_nl: &mut Option<bool>, from: usize, to: usize| {
                if to >= from {
                    let piece = src.sub(from, to).to_string();
                    *last_nl = Some(piece.ends_with('\n'));
                    pending.push(piece);
                }
            };
            if b == SLASH {
                reach = reach.max(i);
                i += 2;
            } else if b == BRACKET
                && let Some(len) = src.zeron_link(i)
            {
                i += len;
                reach = reach.max(i - 1);
            } else if b == BRACKET {
                // (Not one: the furthest the pattern looked is its `]` and the
                // `)` or space after it, on this line.)
                let mut close = src.find_by(i + 1, |b| b == b']' || b == NEWLINE);
                if let Some(c) = close
                    && src.at(c) != Some(NEWLINE)
                {
                    close = src.find_by(c + 1, |b| b == b')' || space(b));
                }
                reach = reach.max(close.unwrap_or(n + 1));
                i = src.find_by(i + 1, special).unwrap_or(n + 1);
            } else if b == BACKTICK {
                // Inline code: past its closing run of as many backticks.
                let run = src.run(i, BACKTICK, usize::MAX).max(1);
                let ticks = "`".repeat(run);
                reach = reach.max(i + run);
                let mut next = i + run;
                let mut close = src.find(ticks.as_bytes(), next);
                while let Some(c) = close
                    && src.at(c + run) == Some(BACKTICK)
                {
                    next = c + run + 1;
                    close = src.find(ticks.as_bytes(), next);
                }
                if let Some(c) = close {
                    i = c + run;
                    reach = reach.max(i);
                } else {
                    // (Unclosed in a paragraph that's over: a stray one, which
                    // the next read checks again. In the last one, it's likely
                    // being written: the read goes on from before it.)
                    if src.find(b"\n\n", i + run).is_some() {
                        self.ticks.truncate(nt);
                        self.ticks.push(Tick { run, next });
                        nt += 1;
                    } else {
                        reach = n + 1;
                    }
                    i += run;
                }
            } else if b == DOLLAR {
                reach = reach.max(i + 1);
                if src.at(i + 1) == Some(DOLLAR) {
                    // (Still open: what's decided waits on its close, which
                    // the read stops for.)
                    let Some(close) = src.find(b"$$", i + 2) else {
                        break;
                    };
                    let tex = trimmed(src.sub(i + 2, close - 1)).to_string();
                    flush(&mut pending, &mut last_nl, from, i - 1);
                    let lead = if last_nl == Some(false) { "\n" } else { "" };
                    let trail = match src.at(close + 2) {
                        None | Some(NEWLINE) => "",
                        _ => "\n",
                    };
                    push(&mut pending, &mut last_nl, format!("{lead}```math\n{tex}\n```{trail}"));
                    reach = reach.max(close + 2);
                    i = close + 2;
                    from = i;
                } else if !blank(src.at(i + 1)) {
                    // Inline: to the next dollar on its paragraph that closes it.
                    let mut j = i + 1;
                    let mut found = None;
                    while j <= n {
                        let c = src.at(j);
                        if c == Some(SLASH) {
                            j += 2;
                        } else if c == Some(DOLLAR) {
                            if !blank(src.at(j - 1)) && !digit(src.at(j + 1)) {
                                found = Some(j);
                            }
                            break;
                        } else if c == Some(NEWLINE) && src.at(j + 1) == Some(NEWLINE) {
                            break;
                        } else {
                            j += 1;
                        }
                    }
                    reach = reach.max(if j > n { n + 1 } else { j + 1 });
                    if let Some(f) = found {
                        flush(&mut pending, &mut last_nl, from, i - 1);
                        push(&mut pending, &mut last_nl, inline_code(src.sub(i + 1, f - 1)));
                        i = f + 1;
                        from = i;
                    } else {
                        i += 1;
                    }
                } else {
                    i += 1;
                }
            } else {
                reach = reach.max(i);
                i = if b == NEWLINE { i + 1 } else { src.find_by(i + 1, special).unwrap_or(n + 1) };
            }
        }
        let mut tail: String = pending.concat();
        if from <= n {
            tail.push_str(src.sub(from, n));
        }
        self.tail = tail;
        self.i = ci;
        self.from = cfrom;
        self.ticks.truncate(cnt);
        self.fence = fence;
        self.last_nl = clast;
    }
}

/// `src` with its math read into code, as the script's `prepare`.
#[cfg_attr(not(test), allow(dead_code))]
pub fn prepare(src: &str) -> String {
    if !src.contains('$') {
        return src.to_string();
    }
    let mut scan = Scan::default();
    scan.feed(src, false);
    scan.made()
}

/// What the prepared text did with a chunk.
#[derive(Debug, PartialEq)]
pub enum Edit {
    /// Nothing.
    None,
    /// It gained this at its end.
    Append(String),
    /// It is this, whole (what it made before changed).
    Set(String),
}

/// A text growing at its end, prepared as it comes (ds/prose's `prepared`):
/// what's settled of the read is never joined again, only what's after it,
/// so a chunk costs what's new, not the whole text.
#[derive(Debug, Default)]
pub struct Prepared {
    scan: Scan,
    /// How much of the settled read was given, and what followed it then.
    settled: usize,
    tail: String,
}

impl Prepared {
    /// The text read last, prepared.
    pub fn made(&self) -> String {
        self.scan.made()
    }
    /// The text as it is now; `extends`: it goes on from the last (only
    /// grown at its end).
    pub fn feed(&mut self, src: &str, extends: bool) -> Edit {
        let over = self.scan.feed(src, extends);
        let scan = &self.scan;
        let edit = if over || scan.settled.len() < self.settled {
            Edit::Set(scan.made())
        } else {
            // What the given settled start is followed by now: what settled
            // since, and what's after it.
            let mut now = String::with_capacity(scan.settled.len() - self.settled + scan.tail.len());
            now.push_str(&scan.settled[self.settled..]);
            now.push_str(&scan.tail);
            if now.starts_with(&self.tail) {
                if now.len() > self.tail.len() {
                    Edit::Append(now[self.tail.len()..].to_string())
                } else {
                    Edit::None
                }
            } else {
                Edit::Set(scan.made())
            }
        };
        self.settled = self.scan.settled.len();
        self.tail = self.scan.tail.clone();
        edit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_math_becomes_a_block_inline_math_code_and_prices_stay_text() {
        let out = prepare("Say $x^2$ here.\n\n$$\\frac{a}{b} \\\\ c$$\n\nIt costs $5 and $10, or \\$3.");
        assert!(out.contains("Say `$x^2$` here."), "{out}");
        assert!(out.contains("```math\n\\frac{a}{b} \\\\ c\n```"), "{out}");
        assert!(out.contains("It costs $5 and $10, or \\$3."), "{out}");
        assert_eq!(prepare("before $$x$$ after"), "before \n```math\nx\n```\n after");
        assert_eq!(
            prepare("```sh\necho $HOME $$\n```\nand `a $b$ c`"),
            "```sh\necho $HOME $$\n```\nand `a $b$ c`"
        );
        assert_eq!(prepare("so $$\\frac{1}{"), "so $$\\frac{1}{");
        assert_eq!(prepare("a $ b $ c"), "a $ b $ c");
        assert_eq!(prepare("$a`b$"), "``$a`b$``");
        let sent = "run [$commit](zeron-invoke:7b22) and [$gh-pr](zeron-invoke:7b23) to main";
        assert_eq!(prepare(sent), sent);
        assert_eq!(prepare("[$a](zeron-invoke:7b) costs $x^2$"), "[$a](zeron-invoke:7b) costs `$x^2$`");
    }

    /// Fed a byte or a few at a time, the text the edits build is the
    /// text prepared whole, at every step.
    #[test]
    fn a_text_fed_as_it_grows_is_the_text_prepared_whole_at_every_step() {
        for text in CORPUS {
            for step in [1, 3, 7, 24] {
                let mut p = Prepared::default();
                let mut built = String::new();
                let mut end = 0;
                while end < text.len() {
                    end = (end + step).min(text.len());
                    while !text.is_char_boundary(end) {
                        end += 1;
                    }
                    match p.feed(&text[..end], end > 0) {
                        Edit::None => {}
                        Edit::Append(more) => built.push_str(&more),
                        Edit::Set(whole) => built = whole,
                    }
                    assert_eq!(built, prepare(&text[..end]), "{text:?} at {end} by {step}");
                }
            }
        }
    }

    /// The script's `prepare` makes the same of each text and each of its
    /// starts (ui/math_source_fixture.luau).
    #[test]
    fn the_scripts_prepare_makes_the_same() {
        let mut inputs: Vec<String> = Vec::new();
        for text in CORPUS {
            for end in 0..=text.len() {
                if text.is_char_boundary(end) {
                    inputs.push(text[..end].to_string());
                }
            }
        }
        let list = zgx_script::Value::List(inputs.iter().map(|s| zgx_script::Value::from(s.as_str())).collect::<Vec<_>>().into());
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/math_source_fixture.luau");
        let made = std::rc::Rc::new(std::cell::RefCell::new(None));
        let slot = made.clone();
        let app = zgx_script::testing::TestApp::new(move |runtime| {
            let host = zgx_script::ScriptHost::new(runtime, path);
            host.shared("math.inputs", list);
            *slot.borrow_mut() = Some(host.shared("math.outputs", zgx_script::Value::Nil));
            host
        });
        app.frames(2);
        assert_eq!(app.error(), None);
        let outputs = made.borrow().clone().unwrap().signal().with_untracked(Clone::clone);
        let outputs = outputs.list();
        assert_eq!(outputs.len(), inputs.len());
        for (input, output) in inputs.iter().zip(outputs) {
            let script = match output {
                zgx_script::Value::Str(s) => s.to_string(),
                other => other.to_string(),
            };
            assert_eq!(prepare(input), script, "{input:?}");
        }
    }

    pub(crate) const CORPUS: &[&str] = &[
        "Say $x^2$ here.\n\n$$\\frac{a}{b} \\\\ c$$\n\nIt costs $5 and $10, or \\$3.",
        "before $$x$$ after and $y$ too\n\n- a $list$ item\n- `code $x$` and ``more `$` code``",
        "```sh\necho $HOME $$\n```\nand `a $b$ c`\n\n~~~\n$$ in tilde $$\n~~~\n\nend $z$.",
        "a stray ` backtick\n\nthen $a$ and `closed $b$` and $$\nmulti\nline\n$$ after",
        "[$a](zeron-invoke:7b) costs $x^2$ and [a [link](x) $c$ and [b]: def $d$",
        "  ```\n  indented $x$\n  ```\n\n    ````\n4 spaces $y$\n\n```` unclosed fence $z$\nstill $w$",
        "The bound is $x^2$ and $O(n \\log n)$ for each part, the sum $\\sum_{k=1}^{n} k$; it costs $5 and $10 a seat, or \\$3.\n\nInline $a_{1}$, $b$, and `$not math$` stays code.",
        "ünïcode $é$ and 日本 $$\\alpha$$ then `tick`` run`` and $ spaced $ and $1 $2$3",
        "Display math:\n\n$$\n\\sum_{k=1}^{n} k = \\frac{n(n+1)}{2}\n$$\n\n| a | $b$ |\n|---|---|\n| $c$ | d |",
    ];
}
