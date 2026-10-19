//! The parsed formula: a TeX math list.

use crate::unicode::Variant;

/// TeX atom classes; they decide inter-atom spacing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Class {
    Ord,
    Op,
    Bin,
    Rel,
    Open,
    Close,
    Punct,
    Inner,
}

/// TeX's four math styles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Style {
    Display,
    Text,
    Script,
    ScriptScript,
}

/// A length: in ems of the current style, or in math units (1/18 em).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Dim {
    Em(f32),
    Mu(f32),
}

pub(crate) type List = Vec<Node>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Align {
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArrayKind {
    /// `matrix` family: centered columns, text style.
    Matrix,
    /// `smallmatrix`: script style, tight.
    Small,
    /// `cases`: two left-aligned columns.
    Cases { display: bool },
    /// `aligned`/`align`/`split`: right-left column pairs, display style.
    Aligned,
    /// `gathered`/`gather`: one centered column, display style.
    Gathered,
    /// `array{…}`: explicit columns, text style.
    Array,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Array {
    pub kind: ArrayKind,
    /// Explicit column alignments (`array`), otherwise derived from `kind`.
    pub cols: Vec<Align>,
    pub rows: Vec<Vec<List>>,
    /// Extra space after each row (`\\[4pt]`), in ems.
    pub gaps: Vec<f32>,
    pub left: Option<char>,
    pub right: Option<char>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Node {
    /// One character. `at` is its byte offset in the source.
    Char {
        ch: char,
        class: Class,
        variant: Variant,
        at: u32,
    },
    /// `{…}`; `class` overrides the atom class (`\mathrel{…}`), default Ord.
    Group {
        body: List,
        class: Option<Class>,
    },
    Scripts {
        base: Box<Node>,
        sup: Option<List>,
        sub: Option<List>,
    },
    /// `\sum`, `\int`, …; `limits` is `\limits`/`\nolimits`, else the default.
    LargeOp {
        ch: char,
        default_limits: bool,
        limits: Option<bool>,
        at: u32,
    },
    /// `\sin`, `\lim`, `\operatorname{…}`.
    NamedOp {
        name: String,
        default_limits: bool,
        limits: Option<bool>,
    },
    Frac {
        num: List,
        den: List,
        bar: bool,
        style: Option<Style>,
        delims: Option<(Option<char>, Option<char>)>,
    },
    Sqrt {
        index: Option<List>,
        body: List,
    },
    LeftRight {
        left: Option<char>,
        right: Option<char>,
        body: List,
    },
    Middle(Option<char>),
    /// `\big(` … `\Bigg)`: size 1–4.
    Big {
        delim: Option<char>,
        size: u8,
        class: Class,
    },
    Accent {
        ch: char,
        wide: bool,
        body: List,
    },
    /// `\overline` / `\underline`.
    Line {
        over: bool,
        body: List,
    },
    /// `\overbrace` / `\underbrace`.
    Brace {
        over: bool,
        body: List,
    },
    /// `\overset`, `\underset`, `\stackrel`.
    Stack {
        base: List,
        over: Option<List>,
        under: Option<List>,
    },
    /// `\xrightarrow[under]{over}`.
    XArrow {
        ch: char,
        over: List,
        under: Option<List>,
    },
    Space(Dim),
    /// `\displaystyle` …: applies to the rest of the list.
    Style(Style),
    Text {
        text: String,
        variant: crate::unicode::Variant,
        at: u32,
    },
    Phantom {
        body: List,
        width: bool,
        height: bool,
    },
    Boxed(List),
    /// `\not` over the next atom.
    Not(Box<Node>),
    Array(Array),
}

impl Node {
    pub(crate) fn group(body: List) -> Node {
        Node::Group { body, class: None }
    }
}
