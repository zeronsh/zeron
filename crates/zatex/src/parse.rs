//! LaTeX math source → [`ast::Node`] list.
//!
//! A single-pass recursive-descent parser over a hand-rolled tokenizer (no
//! macro expansion). Strict: anything it doesn't know is an error with a byte
//! range, so a half-streamed or exotic formula falls back to source instead of
//! rendering wrong.

use std::ops::Range;

use crate::ast::{Align, Array, ArrayKind, Class, Dim, List, Node, Style};
use crate::symbols;
use crate::unicode::Variant;
use crate::{Error, ErrorKind};

#[derive(Debug, Clone, Copy, PartialEq)]
enum Tok<'a> {
    Cmd(&'a str),
    Char(char),
    Open,
    Close,
    Sup,
    Sub,
    Amp,
    Prime,
    Eof,
}

/// What ended a list.
#[derive(Debug)]
enum Ended<'a> {
    Eof,
    Brace,
    Bracket,
    Right,
    End(&'a str),
    Amp,
    Newline(f32),
}

#[derive(Clone, Copy)]
enum Infix {
    Over,
    Atop,
    Choose,
}

pub(crate) fn parse(src: &str, max_depth: usize) -> Result<List, Error> {
    let mut p = Parser {
        src,
        pos: 0,
        depth: 0,
        max_depth,
        variant: Variant::Normal,
    };
    let mut rows: Vec<Vec<List>> = Vec::new();
    let mut row: Vec<List> = Vec::new();
    let mut gaps = Vec::new();
    let (mut lines, mut columns) = (false, false);
    loop {
        let (list, ended, r) = p.list(false)?;
        row.push(list);
        match ended {
            Ended::Eof => {
                rows.push(row);
                break;
            }
            Ended::Amp => columns = true,
            Ended::Newline(gap) => {
                rows.push(std::mem::take(&mut row));
                gaps.push(gap);
                lines = true;
            }
            Ended::Brace => return Err(Error::new(ErrorKind::Unexpected("}".into()), r)),
            Ended::Right => return Err(Error::new(ErrorKind::Unexpected("\\right".into()), r)),
            Ended::End(name) => {
                return Err(Error::new(
                    ErrorKind::Unexpected(format!("\\end{{{name}}}")),
                    r,
                ));
            }
            Ended::Bracket => unreachable!("brackets only end optional arguments"),
        }
    }
    if !lines && !columns {
        return Ok(rows.pop().and_then(|mut r| r.pop()).unwrap_or_default());
    }
    // Bare `a &= b \\ &= c` (no environment): models write it inside `$$`;
    // treat it as the environment they meant.
    trim_last_row(&mut rows);
    Ok(vec![Node::Array(Array {
        kind: if columns {
            ArrayKind::Aligned
        } else {
            ArrayKind::Gathered
        },
        cols: Vec::new(),
        rows,
        gaps,
        left: None,
        right: None,
    })])
}

/// A trailing `\\` leaves an empty last row; drop it.
fn trim_last_row(rows: &mut Vec<Vec<List>>) {
    if rows.len() > 1 && rows.last().is_some_and(|r| r.iter().all(|c| c.is_empty())) {
        rows.pop();
    }
}

struct Parser<'a> {
    src: &'a str,
    pos: usize,
    depth: usize,
    max_depth: usize,
    /// The math alphabet letters get (`\bf`, `\mathbb{…}`).
    variant: Variant,
}

impl<'a> Parser<'a> {
    fn rest(&self) -> &'a str {
        &self.src[self.pos..]
    }

    fn skip_ws(&mut self) {
        loop {
            let rest = self.rest();
            let trimmed = rest.trim_start();
            self.pos += rest.len() - trimmed.len();
            if trimmed.starts_with('%') {
                self.pos += trimmed.find('\n').unwrap_or(trimmed.len());
            } else {
                break;
            }
        }
    }

    fn next(&mut self) -> (Tok<'a>, Range<usize>) {
        self.skip_ws();
        let start = self.pos;
        let Some(ch) = self.rest().chars().next() else {
            return (Tok::Eof, start..start);
        };
        self.pos += ch.len_utf8();
        let tok = match ch {
            '\\' => {
                let rest = self.rest();
                match rest.chars().next() {
                    None => Tok::Cmd(""),
                    Some(c) if c.is_ascii_alphabetic() => {
                        let len = rest
                            .find(|c: char| !c.is_ascii_alphabetic())
                            .unwrap_or(rest.len());
                        self.pos += len;
                        Tok::Cmd(&rest[..len])
                    }
                    Some(c) => {
                        self.pos += c.len_utf8();
                        Tok::Cmd(&rest[..c.len_utf8()])
                    }
                }
            }
            '{' => Tok::Open,
            '}' => Tok::Close,
            '^' => Tok::Sup,
            '_' => Tok::Sub,
            '&' => Tok::Amp,
            '\'' => Tok::Prime,
            c => Tok::Char(c),
        };
        (tok, start..self.pos)
    }

    fn peek(&mut self) -> Tok<'a> {
        let pos = self.pos;
        let (tok, _) = self.next();
        self.pos = pos;
        tok
    }

    /// Consumes `*` right after a command (`\operatorname*`).
    fn star(&mut self) -> bool {
        if self.rest().starts_with('*') {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn deeper(&mut self, range: &Range<usize>) -> Result<(), Error> {
        self.depth += 1;
        if self.depth > self.max_depth {
            return Err(Error::new(ErrorKind::TooDeep, range.clone()));
        }
        Ok(())
    }

    /// Parses a math list up to whatever ends it.
    fn list(&mut self, bracket: bool) -> Result<(List, Ended<'a>, Range<usize>), Error> {
        let saved = self.variant;
        let mut list: List = Vec::new();
        let mut infix: Option<(Infix, List, Range<usize>)> = None;
        let (ended, range) = loop {
            let (tok, range) = self.next();
            let ended = match tok {
                Tok::Eof => Some(Ended::Eof),
                Tok::Close => Some(Ended::Brace),
                Tok::Amp => Some(Ended::Amp),
                Tok::Char(']') if bracket => Some(Ended::Bracket),
                Tok::Cmd("right") => Some(Ended::Right),
                Tok::Cmd("end") => Some(Ended::End(self.env_name(&range)?)),
                Tok::Cmd("\\") | Tok::Cmd("cr") | Tok::Cmd("newline") => {
                    Some(Ended::Newline(self.row_gap(&range)?))
                }
                _ => None,
            };
            if let Some(ended) = ended {
                break (ended, range);
            }
            match tok {
                Tok::Sup | Tok::Sub => {
                    let arg = self.arg(&range)?;
                    attach(&mut list, tok == Tok::Sup, arg, &range)?;
                }
                Tok::Prime => {
                    let mut n = 1;
                    while self.peek() == Tok::Prime {
                        self.next();
                        n += 1;
                    }
                    let primes = match n {
                        1 => vec!['′'],
                        2 => vec!['″'],
                        3 => vec!['‴'],
                        _ => vec!['′'; n],
                    };
                    let arg = primes
                        .into_iter()
                        .map(|ch| Node::Char {
                            ch,
                            class: Class::Ord,
                            variant: Variant::Upright,
                            at: range.start as u32,
                        })
                        .collect();
                    attach(&mut list, true, arg, &range)?;
                }
                Tok::Cmd(name @ ("over" | "atop" | "choose")) => {
                    if infix.is_some() {
                        return Err(Error::new(
                            ErrorKind::Unexpected(format!("\\{name}")),
                            range,
                        ));
                    }
                    let kind = match name {
                        "over" => Infix::Over,
                        "atop" => Infix::Atop,
                        _ => Infix::Choose,
                    };
                    infix = Some((kind, std::mem::take(&mut list), range));
                }
                Tok::Cmd(name @ ("limits" | "nolimits")) => {
                    let value = Some(name == "limits");
                    match list.last_mut() {
                        Some(Node::LargeOp { limits, .. } | Node::NamedOp { limits, .. }) => {
                            *limits = value;
                        }
                        _ => {
                            return Err(Error::new(
                                ErrorKind::Unexpected(format!("\\{name}")),
                                range,
                            ));
                        }
                    }
                }
                _ => {
                    if let Some(node) = self.atom(tok, range)? {
                        list.push(node);
                    }
                }
            }
        };
        self.variant = saved;
        if let Some((kind, num, _)) = infix {
            let den = list;
            list = vec![Node::Frac {
                num,
                den,
                bar: matches!(kind, Infix::Over),
                style: None,
                delims: matches!(kind, Infix::Choose).then_some((Some('('), Some(')'))),
            }];
        }
        Ok((list, ended, range))
    }

    /// `{…}` after its opening brace.
    fn group_body(&mut self, open: &Range<usize>) -> Result<List, Error> {
        self.deeper(open)?;
        let (list, ended, range) = self.list(false)?;
        self.depth -= 1;
        match ended {
            Ended::Brace => Ok(list),
            Ended::Eof => Err(Error::new(ErrorKind::MissingBrace, open.clone())),
            other => Err(Error::new(ErrorKind::Unexpected(describe(&other)), range)),
        }
    }

    /// One argument: a `{group}` or a single token.
    fn arg(&mut self, cmd: &Range<usize>) -> Result<List, Error> {
        let (tok, range) = self.next();
        match tok {
            Tok::Open => self.group_body(&range),
            Tok::Char(_) | Tok::Cmd(_) if !matches!(tok, Tok::Cmd("\\" | "right" | "end")) => {
                Ok(self.atom(tok, range)?.into_iter().collect())
            }
            _ => Err(Error::new(ErrorKind::MissingArgument, cmd.clone())),
        }
    }

    /// `[…]` if present.
    fn optional(&mut self) -> Result<Option<List>, Error> {
        if self.peek() != Tok::Char('[') {
            return Ok(None);
        }
        let (_, open) = self.next();
        self.deeper(&open)?;
        let (list, ended, range) = self.list(true)?;
        self.depth -= 1;
        match ended {
            Ended::Bracket => Ok(Some(list)),
            Ended::Eof => Err(Error::new(ErrorKind::MissingBracket, open)),
            other => Err(Error::new(ErrorKind::Unexpected(describe(&other)), range)),
        }
    }

    /// Raw text of a `{…}` argument (balanced braces), with its range.
    fn raw_group(&mut self, cmd: &Range<usize>) -> Result<(&'a str, Range<usize>), Error> {
        self.skip_ws();
        if !self.rest().starts_with('{') {
            return Err(Error::new(ErrorKind::MissingArgument, cmd.clone()));
        }
        let start = self.pos;
        let body = start + 1;
        let mut depth = 0usize;
        let mut escaped = false;
        for (i, ch) in self.rest().char_indices() {
            if escaped {
                escaped = false;
                continue;
            }
            match ch {
                '\\' => escaped = true,
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        self.pos = start + i + 1;
                        return Ok((&self.src[body..start + i], start..self.pos));
                    }
                }
                _ => {}
            }
        }
        Err(Error::new(ErrorKind::MissingBrace, start..start + 1))
    }

    fn env_name(&mut self, cmd: &Range<usize>) -> Result<&'a str, Error> {
        let (raw, _) = self.raw_group(cmd)?;
        Ok(raw.trim())
    }

    /// `\\[4pt]`: extra space after a row, in ems.
    fn row_gap(&mut self, cmd: &Range<usize>) -> Result<f32, Error> {
        let pos = self.pos;
        self.skip_ws();
        if !self.rest().starts_with('[') {
            self.pos = pos;
            return Ok(0.0);
        }
        let Some(end) = self.rest().find(']') else {
            return Err(Error::new(ErrorKind::MissingBracket, cmd.clone()));
        };
        let raw = &self.rest()[1..end];
        let range = self.pos..self.pos + end + 1;
        self.pos += end + 1;
        match parse_dim(raw) {
            Some(Dim::Em(v)) => Ok(v),
            Some(Dim::Mu(v)) => Ok(v / 18.0),
            None => Err(Error::new(ErrorKind::BadDimension, range)),
        }
    }

    /// A dimension argument: `{1em}` or a bare `1em`.
    fn dim_arg(&mut self, cmd: &Range<usize>) -> Result<Dim, Error> {
        self.skip_ws();
        if self.rest().starts_with('{') {
            let (raw, range) = self.raw_group(cmd)?;
            return parse_dim(raw).ok_or(Error::new(ErrorKind::BadDimension, range));
        }
        let rest = self.rest();
        let number = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+' || c == ' '))
            .unwrap_or(rest.len());
        let unit = rest[number..]
            .find(|c: char| !c.is_ascii_alphabetic())
            .map_or(rest.len(), |u| number + u);
        let unit = (unit - number).min(2) + number;
        let raw = &rest[..unit];
        let range = self.pos..self.pos + unit;
        self.pos += unit;
        parse_dim(raw).ok_or(Error::new(ErrorKind::BadDimension, range))
    }

    fn delim(&mut self, cmd: &Range<usize>) -> Result<Option<char>, Error> {
        let (tok, range) = self.next();
        let found = match tok {
            Tok::Char(c) => symbols::delimiter_char(c),
            Tok::Cmd(name) => symbols::delimiter_command(name),
            Tok::Eof => return Err(Error::new(ErrorKind::MissingDelimiter, cmd.clone())),
            _ => None,
        };
        found.ok_or(Error::new(ErrorKind::BadDelimiter, range))
    }

    /// `\text{…}`-style content: literal characters, whitespace collapsed.
    fn text(&mut self, cmd: &Range<usize>) -> Result<String, Error> {
        self.skip_ws();
        if !self.rest().starts_with('{') {
            // `\text x`: one character.
            let (tok, range) = self.next();
            return match tok {
                Tok::Char(c) => Ok(c.to_string()),
                _ => Err(Error::new(ErrorKind::MissingArgument, range)),
            };
        }
        let (raw, range) = self.raw_group(cmd)?;
        let base = range.start + 1;
        let mut out = String::with_capacity(raw.len());
        let mut chars = raw.char_indices().peekable();
        while let Some((i, ch)) = chars.next() {
            match ch {
                '{' | '}' => {}
                '~' => out.push('\u{a0}'),
                '$' => {
                    return Err(Error::new(
                        ErrorKind::Unsupported("math inside \\text".into()),
                        base + i..base + i + 1,
                    ));
                }
                '\\' => {
                    let rest = &raw[i + 1..];
                    let Some(next) = rest.chars().next() else {
                        continue;
                    };
                    if next.is_ascii_alphabetic() {
                        let len = rest
                            .find(|c: char| !c.is_ascii_alphabetic())
                            .unwrap_or(rest.len());
                        let name = &rest[..len];
                        for _ in 0..len {
                            chars.next();
                        }
                        match name {
                            "textbackslash" => out.push('\\'),
                            "quad" => out.push('\u{2003}'),
                            "qquad" => out.push_str("\u{2003}\u{2003}"),
                            // Nested styling flattens: the braces that follow are skipped.
                            "textbf" | "textit" | "textrm" | "text" | "emph" | "textsf"
                            | "texttt" | "textup" | "textnormal" => {}
                            _ => {
                                return Err(Error::new(
                                    ErrorKind::UnknownCommand(name.into()),
                                    base + i..base + i + 1 + len,
                                ));
                            }
                        }
                    } else {
                        chars.next();
                        match next {
                            ' ' | ',' | ';' | ':' | '!' => out.push(' '),
                            '\\' => out.push(' '),
                            c => out.push(c),
                        }
                    }
                }
                c if c.is_whitespace() => {
                    if !out.ends_with(' ') {
                        out.push(' ');
                    }
                }
                c => out.push(c),
            }
        }
        Ok(out)
    }

    fn atom(&mut self, tok: Tok<'a>, range: Range<usize>) -> Result<Option<Node>, Error> {
        self.deeper(&range)?;
        let node = self.atom_inner(tok, range);
        self.depth -= 1;
        node
    }

    fn atom_inner(&mut self, tok: Tok<'a>, range: Range<usize>) -> Result<Option<Node>, Error> {
        let at = range.start as u32;
        Ok(Some(match tok {
            Tok::Char('~') => Node::Space(Dim::Em(0.25)),
            Tok::Char(c) => {
                let (ch, class) = symbols::char_class(c);
                Node::Char {
                    ch,
                    class,
                    variant: self.variant,
                    at,
                }
            }
            Tok::Open => Node::group(self.group_body(&range)?),
            Tok::Cmd(name) => return self.command(name, range),
            Tok::Close => return Err(Error::new(ErrorKind::Unexpected("}".into()), range)),
            Tok::Sup | Tok::Sub | Tok::Amp | Tok::Prime => {
                let text = self.src[range.clone()].to_string();
                return Err(Error::new(ErrorKind::Unexpected(text), range));
            }
            Tok::Eof => return Err(Error::new(ErrorKind::MissingArgument, range)),
        }))
    }

    fn command(&mut self, name: &'a str, range: Range<usize>) -> Result<Option<Node>, Error> {
        use Class::*;
        let at = range.start as u32;
        let node = match name {
            "left" => {
                let left = self.delim(&range)?;
                self.deeper(&range)?;
                let (body, ended, end) = self.list(false)?;
                self.depth -= 1;
                match ended {
                    Ended::Right => {}
                    Ended::Eof => return Err(Error::new(ErrorKind::MissingRight, range)),
                    other => {
                        return Err(Error::new(ErrorKind::Unexpected(describe(&other)), end));
                    }
                }
                let right = self.delim(&end)?;
                Node::LeftRight { left, right, body }
            }
            "middle" => Node::Middle(self.delim(&range)?),
            "big" | "bigl" | "bigr" | "bigm" | "Big" | "Bigl" | "Bigr" | "Bigm" | "bigg"
            | "biggl" | "biggr" | "biggm" | "Bigg" | "Biggl" | "Biggr" | "Biggm" => {
                let size = if name.starts_with("Bigg") {
                    4
                } else if name.starts_with("bigg") {
                    3
                } else if name.starts_with("Big") {
                    2
                } else {
                    1
                };
                let class = match name.as_bytes()[name.len() - 1] {
                    b'l' => Open,
                    b'r' => Close,
                    b'm' => Rel,
                    _ => Ord,
                };
                Node::Big {
                    delim: self.delim(&range)?,
                    size,
                    class,
                }
            }
            "frac" | "dfrac" | "tfrac" | "cfrac" | "binom" | "dbinom" | "tbinom" => {
                let num = self.arg(&range)?;
                let den = self.arg(&range)?;
                let style = match name {
                    "dfrac" | "cfrac" | "dbinom" => Some(Style::Display),
                    "tfrac" | "tbinom" => Some(Style::Text),
                    _ => None,
                };
                let binom = name.ends_with("binom");
                Node::Frac {
                    num,
                    den,
                    bar: !binom,
                    style,
                    delims: binom.then_some((Some('('), Some(')'))),
                }
            }
            "sqrt" => {
                let index = self.optional()?;
                let body = self.arg(&range)?;
                Node::Sqrt { index, body }
            }
            "begin" => Node::Array(self.environment(&range)?).flatten_equation(),
            "operatorname" | "operatornamewithlimits" => {
                let star = self.star() || name == "operatornamewithlimits";
                let text = self.text(&range)?;
                Node::NamedOp {
                    name: text.trim().to_string(),
                    default_limits: star,
                    limits: None,
                }
            }
            "text" | "textrm" | "textup" | "textnormal" | "mbox" | "hbox" | "textmd" | "textbf"
            | "textit" | "emph" | "textsf" | "texttt" | "fbox" => {
                let variant = match name {
                    "textbf" => Variant::Bold,
                    "textit" | "emph" => Variant::Italic,
                    "textsf" => Variant::Sans,
                    "texttt" => Variant::Mono,
                    _ => Variant::Upright,
                };
                let text = self.text(&range)?;
                let node = Node::Text { text, variant, at };
                if name == "fbox" {
                    Node::Boxed(vec![node])
                } else {
                    node
                }
            }
            "mathrm" | "mathit" | "mathbf" | "mathbb" | "mathcal" | "mathscr" | "mathfrak"
            | "mathsf" | "mathtt" | "boldsymbol" | "bm" | "mathbfit" | "mathnormal" | "pmb"
            | "mathsfbf" | "mathbfsf" => {
                let variant = match name {
                    "mathrm" => Variant::Upright,
                    "mathit" => Variant::Italic,
                    "mathbf" => Variant::Bold,
                    "mathbb" => Variant::DoubleStruck,
                    "mathcal" | "mathscr" => Variant::Script,
                    "mathfrak" => Variant::Fraktur,
                    "mathsf" => Variant::Sans,
                    "mathtt" => Variant::Mono,
                    "mathnormal" => Variant::Normal,
                    "mathsfbf" | "mathbfsf" => Variant::SansBold,
                    _ => Variant::BoldItalic,
                };
                let saved = std::mem::replace(&mut self.variant, variant);
                let body = self.arg(&range);
                self.variant = saved;
                Node::group(body?)
            }
            "rm" | "bf" | "it" | "cal" | "sf" | "tt" | "mit" => {
                self.variant = match name {
                    "rm" => Variant::Upright,
                    "bf" => Variant::Bold,
                    "it" => Variant::Italic,
                    "cal" => Variant::Script,
                    "sf" => Variant::Sans,
                    "tt" => Variant::Mono,
                    _ => Variant::Normal,
                };
                return Ok(None);
            }
            "displaystyle" => Node::Style(Style::Display),
            "textstyle" => Node::Style(Style::Text),
            "scriptstyle" => Node::Style(Style::Script),
            "scriptscriptstyle" => Node::Style(Style::ScriptScript),
            "," | "thinspace" => Node::Space(Dim::Mu(3.0)),
            ":" | ">" | "medspace" => Node::Space(Dim::Mu(4.0)),
            ";" | "thickspace" => Node::Space(Dim::Mu(5.0)),
            "!" | "negthinspace" => Node::Space(Dim::Mu(-3.0)),
            "negmedspace" => Node::Space(Dim::Mu(-4.0)),
            "negthickspace" => Node::Space(Dim::Mu(-5.0)),
            " " | "space" => Node::Space(Dim::Em(0.25)),
            "enspace" => Node::Space(Dim::Em(0.5)),
            "quad" => Node::Space(Dim::Em(1.0)),
            "qquad" => Node::Space(Dim::Em(2.0)),
            "hspace" | "hskip" | "kern" | "mkern" | "mskip" => {
                self.star();
                Node::Space(self.dim_arg(&range)?)
            }
            "overline" | "underline" => Node::Line {
                over: name == "overline",
                body: self.arg(&range)?,
            },
            "overbrace" | "underbrace" => Node::Brace {
                over: name == "overbrace",
                body: self.arg(&range)?,
            },
            "overset" | "stackrel" => {
                let over = self.arg(&range)?;
                let base = self.arg(&range)?;
                Node::Stack {
                    base,
                    over: Some(over),
                    under: None,
                }
            }
            "underset" => {
                let under = self.arg(&range)?;
                let base = self.arg(&range)?;
                Node::Stack {
                    base,
                    over: None,
                    under: Some(under),
                }
            }
            "xrightarrow" | "xleftarrow" | "xRightarrow" | "xLeftarrow" | "xleftrightarrow"
            | "xLeftrightarrow" | "xmapsto" => {
                let ch = match name {
                    "xrightarrow" => '→',
                    "xleftarrow" => '←',
                    "xRightarrow" => '⇒',
                    "xLeftarrow" => '⇐',
                    "xleftrightarrow" => '↔',
                    "xLeftrightarrow" => '⇔',
                    _ => '↦',
                };
                let under = self.optional()?;
                let over = self.arg(&range)?;
                Node::XArrow { ch, over, under }
            }
            "phantom" | "hphantom" | "vphantom" => Node::Phantom {
                body: self.arg(&range)?,
                width: name != "vphantom",
                height: name != "hphantom",
            },
            "mathstrut" => Node::Phantom {
                body: vec![Node::Char {
                    ch: '(',
                    class: Open,
                    variant: Variant::Upright,
                    at,
                }],
                width: false,
                height: true,
            },
            "boxed" => Node::Boxed(self.arg(&range)?),
            "not" => {
                let (tok, next) = self.next();
                let Some(node) = self.atom(tok, next)? else {
                    return Err(Error::new(ErrorKind::MissingArgument, range));
                };
                negate(node)
            }
            "color" => {
                self.raw_group(&range)?;
                return Ok(None);
            }
            "textcolor" | "colorbox" => {
                self.raw_group(&range)?;
                Node::group(self.arg(&range)?)
            }
            "mathop" | "mathbin" | "mathrel" | "mathord" | "mathopen" | "mathclose"
            | "mathpunct" | "mathinner" => {
                let class = match name {
                    "mathop" => Op,
                    "mathbin" => Bin,
                    "mathrel" => Rel,
                    "mathopen" => Open,
                    "mathclose" => Close,
                    "mathpunct" => Punct,
                    "mathinner" => Inner,
                    _ => Ord,
                };
                Node::Group {
                    body: self.arg(&range)?,
                    class: Some(class),
                }
            }
            "tag" => {
                self.star();
                let text = self.text(&range)?;
                Node::Group {
                    body: vec![
                        Node::Space(Dim::Em(2.0)),
                        Node::Text {
                            text: format!("({text})"),
                            variant: Variant::Upright,
                            at,
                        },
                    ],
                    class: Some(Ord),
                }
            }
            "label" => {
                self.raw_group(&range)?;
                return Ok(None);
            }
            "nonumber" | "notag" | "displaybreak" | "allowbreak" | "nobreak" => return Ok(None),
            _ => {
                if let Some((ch, class)) = symbols::symbol(name) {
                    Node::Char {
                        ch,
                        class,
                        variant: self.variant,
                        at,
                    }
                } else if let Some((ch, default_limits)) = symbols::large_op(name) {
                    Node::LargeOp {
                        ch,
                        default_limits,
                        limits: None,
                        at,
                    }
                } else if let Some((text, default_limits)) = symbols::named_op(name) {
                    Node::NamedOp {
                        name: text.to_string(),
                        default_limits,
                        limits: None,
                    }
                } else if let Some((ch, wide)) = symbols::accent(name) {
                    Node::Accent {
                        ch,
                        wide,
                        body: self.arg(&range)?,
                    }
                } else {
                    let name = if name.is_empty() { "\\" } else { name };
                    return Err(Error::new(ErrorKind::UnknownCommand(name.into()), range));
                }
            }
        };
        Ok(Some(node))
    }

    fn environment(&mut self, cmd: &Range<usize>) -> Result<Array, Error> {
        let name = self.env_name(cmd)?;
        let (mut left, mut right) = (None, None);
        let mut cols = Vec::new();
        let kind = match name {
            "matrix" => ArrayKind::Matrix,
            "pmatrix" => {
                (left, right) = (Some('('), Some(')'));
                ArrayKind::Matrix
            }
            "bmatrix" => {
                (left, right) = (Some('['), Some(']'));
                ArrayKind::Matrix
            }
            "Bmatrix" => {
                (left, right) = (Some('{'), Some('}'));
                ArrayKind::Matrix
            }
            "vmatrix" => {
                (left, right) = (Some('|'), Some('|'));
                ArrayKind::Matrix
            }
            "Vmatrix" => {
                (left, right) = (Some('‖'), Some('‖'));
                ArrayKind::Matrix
            }
            "smallmatrix" => ArrayKind::Small,
            "cases" | "dcases" => {
                left = Some('{');
                ArrayKind::Cases {
                    display: name == "dcases",
                }
            }
            "rcases" => {
                right = Some('}');
                ArrayKind::Cases { display: false }
            }
            "aligned" | "align" | "align*" | "split" | "eqnarray" | "eqnarray*" | "flalign"
            | "flalign*" => ArrayKind::Aligned,
            "alignat" | "alignat*" | "alignedat" => {
                self.raw_group(cmd)?;
                ArrayKind::Aligned
            }
            "gathered" | "gather" | "gather*" | "multline" | "multline*" | "equation"
            | "equation*" | "displaymath" => ArrayKind::Gathered,
            "array" | "darray" | "subarray" => {
                let (spec, _) = self.raw_group(cmd)?;
                cols = column_spec(spec);
                ArrayKind::Array
            }
            _ => {
                return Err(Error::new(
                    ErrorKind::UnknownEnvironment(name.into()),
                    cmd.clone(),
                ));
            }
        };
        let mut rows: Vec<Vec<List>> = Vec::new();
        let mut row: Vec<List> = Vec::new();
        let mut gaps = Vec::new();
        self.deeper(cmd)?;
        loop {
            let (cell, ended, range) = self.list(false)?;
            row.push(cell);
            match ended {
                Ended::Amp => {}
                Ended::Newline(gap) => {
                    rows.push(std::mem::take(&mut row));
                    gaps.push(gap);
                }
                Ended::End(end) if end == name => {
                    rows.push(row);
                    break;
                }
                Ended::End(end) => {
                    return Err(Error::new(
                        ErrorKind::MismatchedEnvironment {
                            expected: name.into(),
                            found: end.into(),
                        },
                        range,
                    ));
                }
                Ended::Eof => {
                    return Err(Error::new(ErrorKind::MissingEnd(name.into()), cmd.clone()));
                }
                other => return Err(Error::new(ErrorKind::Unexpected(describe(&other)), range)),
            }
            if rows.len() > 4096 {
                return Err(Error::new(ErrorKind::TooLarge, cmd.clone()));
            }
        }
        self.depth -= 1;
        trim_last_row(&mut rows);
        gaps.truncate(rows.len());
        Ok(Array {
            kind,
            cols,
            rows,
            gaps,
            left,
            right,
        })
    }
}

impl Node {
    /// `equation` is just its body.
    fn flatten_equation(self) -> Node {
        match self {
            Node::Array(a)
                if a.kind == ArrayKind::Gathered
                    && a.rows.len() == 1
                    && a.rows[0].len() == 1
                    && a.left.is_none() =>
            {
                let mut rows = a.rows;
                Node::group(rows.pop().and_then(|mut r| r.pop()).unwrap_or_default())
            }
            other => other,
        }
    }
}

/// Attaches a script to the last atom of `list`.
fn attach(list: &mut List, sup: bool, arg: List, range: &Range<usize>) -> Result<(), Error> {
    let mut node = match list.pop() {
        Some(n @ Node::Scripts { .. }) => n,
        Some(base) => Node::Scripts {
            base: Box::new(base),
            sup: None,
            sub: None,
        },
        None => Node::Scripts {
            base: Box::new(Node::group(Vec::new())),
            sup: None,
            sub: None,
        },
    };
    if let Node::Scripts {
        sup: sup_slot,
        sub: sub_slot,
        ..
    } = &mut node
    {
        let slot = if sup { sup_slot } else { sub_slot };
        match slot {
            None => *slot = Some(arg),
            // `f'^2`: primes and an explicit superscript share the slot.
            Some(existing) if sup && existing.iter().all(is_prime) => existing.extend(arg),
            Some(_) => return Err(Error::new(ErrorKind::DoubleScript, range.clone())),
        }
    }
    list.push(node);
    Ok(())
}

fn is_prime(node: &Node) -> bool {
    matches!(
        node,
        Node::Char {
            ch: '′' | '″' | '‴',
            ..
        }
    )
}

/// `\not` before a relation with a precomposed negation uses it.
fn negate(node: Node) -> Node {
    if let Node::Char {
        ch,
        class,
        variant,
        at,
    } = node
    {
        let negated = match ch {
            '=' => Some('≠'),
            '∈' => Some('∉'),
            '∋' => Some('∌'),
            '≡' => Some('≢'),
            '⊂' => Some('⊄'),
            '⊃' => Some('⊅'),
            '⊆' => Some('⊈'),
            '⊇' => Some('⊉'),
            '∼' => Some('≁'),
            '≃' => Some('≄'),
            '≅' => Some('≇'),
            '≈' => Some('≉'),
            '<' => Some('≮'),
            '>' => Some('≯'),
            '≤' => Some('≰'),
            '≥' => Some('≱'),
            '∣' => Some('∤'),
            '∥' => Some('∦'),
            '∃' => Some('∄'),
            _ => None,
        };
        let ch = negated.unwrap_or(ch);
        let node = Node::Char {
            ch,
            class,
            variant,
            at,
        };
        return if negated.is_some() {
            node
        } else {
            Node::Not(Box::new(node))
        };
    }
    Node::Not(Box::new(node))
}

fn column_spec(spec: &str) -> Vec<Align> {
    let mut cols = Vec::new();
    let mut depth = 0;
    for ch in spec.chars() {
        match ch {
            '{' => depth += 1,
            '}' => depth -= 1,
            _ if depth > 0 => {}
            'l' => cols.push(Align::Left),
            'c' => cols.push(Align::Center),
            'r' => cols.push(Align::Right),
            'p' | 'm' | 'b' => cols.push(Align::Left),
            _ => {}
        }
    }
    cols
}

/// `1.5em`, `-3mu`, `4pt` … as a [`Dim`]. Points assume a 10pt em, like TeX.
pub(crate) fn parse_dim(raw: &str) -> Option<Dim> {
    let raw = raw.trim();
    let split = raw
        .find(|c: char| c.is_ascii_alphabetic())
        .unwrap_or(raw.len());
    let number: String = raw[..split]
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let value: f32 = number.parse().ok()?;
    if !value.is_finite() || value.abs() > 1000.0 {
        return None;
    }
    let em = match raw[split..].trim() {
        "em" => value,
        "ex" => value * 0.431,
        "mu" => return (value.abs() <= MAX_DIM_EM * 18.0).then_some(Dim::Mu(value)),
        "pt" | "px" | "bp" | "dd" => value * 0.1,
        "pc" => value * 1.2,
        "mm" => value * 0.2845,
        "cm" => value * 2.845,
        "in" => value * 7.227,
        _ => return None,
    };
    // Checked after unit conversion: `1000in` is 7227em. No real kern or row
    // gap comes near 100em; beyond it a dimension can only blow up the box.
    (em.abs() <= MAX_DIM_EM).then_some(Dim::Em(em))
}

/// Largest accepted dimension (`\kern`, `\hspace`, `\\[…]`), in ems.
const MAX_DIM_EM: f32 = 100.0;

fn describe(ended: &Ended) -> String {
    match ended {
        Ended::Eof => "end of input".into(),
        Ended::Brace => "}".into(),
        Ended::Bracket => "]".into(),
        Ended::Right => "\\right".into(),
        Ended::End(name) => format!("\\end{{{name}}}"),
        Ended::Amp => "&".into(),
        Ended::Newline(_) => "\\\\".into(),
    }
}
