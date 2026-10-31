use crate::{ErrorKind, Formula, Item, MathFont, Options, layout};
use std::sync::{Arc, OnceLock};

const STIX: &[u8] = include_bytes!("../fonts/STIXTwoMath-Regular.otf");
const LIBERTINUS: &[u8] = include_bytes!("../fonts/LibertinusMath-Regular.otf");
const FIRA: &[u8] = include_bytes!("../fonts/FiraMath-Regular.otf");

/// STIX Two Math alone; Libertinus Math and Fira Math falling back to it.
pub(crate) fn fonts() -> &'static [MathFont] {
    static FONTS: OnceLock<Vec<MathFont>> = OnceLock::new();
    FONTS.get_or_init(|| {
        let stix = Arc::new(MathFont::new(STIX).unwrap());
        vec![
            MathFont::new(STIX).unwrap(),
            MathFont::new(LIBERTINUS)
                .unwrap()
                .with_fallback(stix.clone()),
            MathFont::new(FIRA).unwrap().with_fallback(stix),
        ]
    })
}

fn stix() -> &'static MathFont {
    &fonts()[0]
}

fn inline(src: &str) -> Formula {
    layout(src, stix(), &Options::inline(16.0)).unwrap_or_else(|e| panic!("{src}: {e}"))
}

fn display(src: &str) -> Formula {
    layout(src, stix(), &Options::display(16.0)).unwrap_or_else(|e| panic!("{src}: {e}"))
}

fn glyphs(f: &Formula) -> usize {
    f.items
        .iter()
        .filter(|i| matches!(i, Item::Glyph { .. }))
        .count()
}

fn rules(f: &Formula) -> usize {
    f.items
        .iter()
        .filter(|i| matches!(i, Item::Rule { .. }))
        .count()
}

/// What models actually write. Every font must lay all of it out.
pub(crate) const CORPUS: &[&str] = &[
    r"x^2 + y^2 = z^2",
    r"E = mc^2",
    r"e^{i\pi} + 1 = 0",
    r"\frac{a}{b}",
    r"\frac{-b \pm \sqrt{b^2 - 4ac}}{2a}",
    r"\sum_{i=1}^{n} i = \frac{n(n+1)}{2}",
    r"\int_0^\infty e^{-x^2}\,dx = \frac{\sqrt{\pi}}{2}",
    r"\lim_{x \to 0} \frac{\sin x}{x} = 1",
    r"\prod_{k=1}^{n} (1 + x_k)",
    r"f'(x) = \lim_{h\to 0}\frac{f(x+h)-f(x)}{h}",
    r"\nabla \cdot \mathbf{E} = \frac{\rho}{\varepsilon_0}",
    r"\mathbb{R}^n \to \mathbb{R}",
    r"\mathcal{O}(n \log n)",
    r"\left( \frac{a}{b} \right)^2",
    r"\left\{ x \in \mathbb{Z} \middle| x > 0 \right\}",
    r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}",
    r"\begin{bmatrix} 1 & 0 & 0 \\ 0 & 1 & 0 \\ 0 & 0 & 1 \end{bmatrix}",
    r"\det \begin{vmatrix} a & b \\ c & d \end{vmatrix} = ad - bc",
    r"f(x) = \begin{cases} x^2 & \text{if } x \ge 0 \\ -x & \text{otherwise} \end{cases}",
    r"\begin{aligned} (a+b)^2 &= a^2 + 2ab + b^2 \\ &\le 2(a^2+b^2) \end{aligned}",
    r"\hat{x}, \bar{y}, \vec{v}, \dot{x}, \ddot{x}, \tilde{n}",
    r"\widehat{abc} \quad \overline{z} \quad \underline{w}",
    r"\overbrace{1 + 2 + \cdots + n}^{n \text{ terms}}",
    r"\underbrace{a + b}_{\text{sum}}",
    r"\binom{n}{k} = \frac{n!}{k!(n-k)!}",
    r"\sqrt[3]{x^3 + y^3}",
    r"P(A \mid B) = \frac{P(B \mid A)\,P(A)}{P(B)}",
    r"\mathbb{E}[X] = \sum_x x\,p(x)",
    r"\operatorname{softmax}(z)_i = \frac{e^{z_i}}{\sum_j e^{z_j}}",
    r"\arg\max_{\theta} \log p_\theta(x)",
    r"\alpha\beta\gamma\delta\epsilon\zeta\eta\theta\iota\kappa\lambda\mu\nu\xi\pi\rho\sigma\tau\upsilon\phi\chi\psi\omega",
    r"\Gamma\Delta\Theta\Lambda\Xi\Pi\Sigma\Upsilon\Phi\Psi\Omega",
    r"a \leq b \geq c \neq d \approx e \equiv f \sim g \propto h",
    r"A \subseteq B \cup C \cap D \setminus E",
    r"\forall \epsilon > 0\ \exists \delta > 0",
    r"x \mapsto x^2, \quad f: X \to Y",
    r"\boxed{x = 42}",
    r"\xrightarrow{f} \quad \xleftarrow[g]{h}",
    r"\overset{?}{=} \quad \underset{n}{\max} \quad a \stackrel{\text{def}}{=} b",
    r"\langle u, v \rangle = \|u\| \|v\| \cos\theta",
    r"\lfloor x \rfloor + \lceil y \rceil",
    r"\bigl( \Bigl( \biggl( \Biggl( x \Biggr) \biggr) \Bigr) \bigr)",
    r"\iint_D f \, dA \quad \oint_C \mathbf{F}\cdot d\mathbf{r}",
    r"\frac{\partial^2 u}{\partial t^2} = c^2 \nabla^2 u",
    r"a_{ij}^{(k)} \quad x_1, x_2, \ldots, x_n \quad 1 + \cdots + n",
    r"\displaystyle\sum_{n=1}^\infty \frac{1}{n^2} = \frac{\pi^2}{6}",
    r"\text{Var}(X) = \mathbb{E}[(X - \mu)^2]",
    r"\not\in \quad \not= \quad \not\subset",
    r"\phantom{x} \hphantom{y} \vphantom{\int}",
    r"\left. \frac{df}{dx} \right|_{x=0}",
    r"\begin{array}{lcr} a & b & c \\ 10 & 20 & 30 \end{array}",
    r"\begin{smallmatrix} a & b \\ c & d \end{smallmatrix}",
    r"\mathrm{d}x \quad \mathsf{A} \quad \mathtt{x} \quad \boldsymbol{\beta}",
    r"\frac{1}{1 + \frac{1}{1 + \frac{1}{x}}}",
    r"\sqrt{\sqrt{\sqrt{x}}}",
    r"10\% \quad \$5 \quad \#1 \quad a\_b",
    r"{n \choose k} \quad {a \over b}",
];

#[test]
fn corpus_lays_out_in_every_font() {
    for font in fonts() {
        let name = font.name().unwrap_or_default();
        for src in CORPUS {
            for opts in [Options::inline(16.0), Options::display(16.0)] {
                let f = layout(src, font, &opts).unwrap_or_else(|e| panic!("{name}: {src}: {e}"));
                assert!(f.width > 0.0, "{name}: {src}: zero width");
                assert!(f.height() > 0.0, "{name}: {src}: zero height");
                for item in &f.items {
                    let (x, y) = match *item {
                        Item::Glyph { x, y, .. } | Item::Rule { x, y, .. } => (x, y),
                    };
                    assert!(x.is_finite() && y.is_finite(), "{name}: {src}: {item:?}");
                }
            }
        }
    }
}

#[test]
fn fonts_load_with_names() {
    let names: Vec<_> = fonts().iter().map(|f| f.name().unwrap()).collect();
    assert_eq!(names.len(), 3);
    assert!(names[0].contains("STIX"), "{names:?}");
    assert!(names[1].contains("Libertinus"), "{names:?}");
    assert!(names[2].contains("Fira"), "{names:?}");
}

#[test]
fn non_math_font_is_rejected() {
    // Any font without a MATH table; the bytes of one of ours minus the table
    // would be fiddly, so use something that isn't a font at all, too.
    assert!(MathFont::new(b"not a font").is_err());
}

#[test]
fn fonts_change_the_layout() {
    let src = r"\frac{a}{b} + \sqrt{x}";
    let widths: Vec<f32> = fonts()
        .iter()
        .map(|f| layout(src, f, &Options::inline(16.0)).unwrap().width)
        .collect();
    assert!(
        (widths[0] - widths[1]).abs() > 0.1 && (widths[0] - widths[2]).abs() > 0.1,
        "{widths:?}"
    );
}

#[test]
fn superscript_rises_and_shrinks() {
    let f = inline("x^2");
    let Item::Glyph {
        y: base_y,
        size: base_size,
        ..
    } = f.items[0]
    else {
        panic!()
    };
    let Item::Glyph { y, size, .. } = f.items[1] else {
        panic!()
    };
    assert_eq!(base_y, 0.0);
    assert!(y < -3.0, "superscript baseline {y}");
    assert!(size < base_size);
}

#[test]
fn subscript_drops() {
    let f = inline("x_2");
    let Item::Glyph { y, .. } = f.items[1] else {
        panic!()
    };
    assert!(y > 1.0, "subscript baseline {y}");
}

#[test]
fn fraction_has_bar_and_stacks() {
    let f = inline(r"\frac{a}{b}");
    assert_eq!(rules(&f), 1);
    assert_eq!(glyphs(&f), 2);
    let ys: Vec<f32> = f
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Glyph { y, .. } => Some(*y),
            _ => None,
        })
        .collect();
    assert!(ys[0] < 0.0 && ys[1] > 0.0, "{ys:?}");
    // Display fractions are taller.
    assert!(display(r"\frac{a}{b}").height() > f.height());
}

#[test]
fn binom_has_no_bar() {
    let f = inline(r"\binom{n}{k}");
    assert_eq!(rules(&f), 0);
    assert_eq!(glyphs(&f), 4);
}

#[test]
fn display_sum_is_bigger_with_limits_above() {
    let t = inline(r"\sum_{i=1}^n");
    let d = display(r"\sum_{i=1}^n");
    assert!(d.height() > t.height() * 1.3);
    // Display limits are centered over the operator: narrower than side scripts.
    assert!(d.width < t.width * 1.2);
}

#[test]
fn left_right_grows_with_content() {
    let small = inline(r"\left( x \right)");
    let tall = inline(r"\left( \frac{\frac{a}{b}}{\frac{c}{d}} \right)");
    let first = |f: &Formula| match f.items[0] {
        Item::Glyph { glyph, .. } => glyph,
        _ => panic!(),
    };
    assert_ne!(
        first(&small),
        first(&tall),
        "a taller paren variant or assembly"
    );
    assert!(tall.height() > small.height() * 2.0);
}

#[test]
fn huge_delimiters_assemble() {
    let src = r"\left( \begin{matrix} 1 \\ 2 \\ 3 \\ 4 \\ 5 \\ 6 \\ 7 \\ 8 \end{matrix} \right)";
    for font in fonts() {
        let f = layout(src, font, &Options::display(16.0)).unwrap();
        // 8 digits plus at least a three-part paren on each side.
        assert!(
            glyphs(&f) >= 8 + 6,
            "{}: {}",
            font.name().unwrap(),
            glyphs(&f)
        );
    }
}

#[test]
fn sqrt_has_overbar() {
    let f = inline(r"\sqrt{x}");
    assert_eq!(rules(&f), 1);
}

#[test]
fn spacing_follows_atom_classes() {
    // a+b has medium spaces, a=b thick: a=b is wider.
    let plus = inline("a+b").width;
    let eq = inline("a=b").width;
    let plus_glyph = inline("+").width;
    let eq_glyph = inline("=").width;
    assert!(eq - eq_glyph > plus - plus_glyph);
    // A leading binary operator is unary: no space after it.
    assert!(inline("-b").width < inline("a-b").width - inline("a").width);
}

#[test]
fn errors_are_located() {
    let err = |src: &str| layout(src, stix(), &Options::default()).unwrap_err();
    let e = err(r"x + \foo");
    assert_eq!(e.kind, ErrorKind::UnknownCommand("foo".into()));
    assert_eq!(e.range, 4..8);
    assert_eq!(err(r"\frac{a}{b").kind, ErrorKind::MissingBrace);
    assert_eq!(err(r"\left( x").kind, ErrorKind::MissingRight);
    assert_eq!(err(r"x^2^3").kind, ErrorKind::DoubleScript);
    assert_eq!(err(r"\frac{a}").kind, ErrorKind::MissingArgument);
    assert!(matches!(
        err(r"\begin{matrix} a").kind,
        ErrorKind::MissingEnd(_)
    ));
    assert!(matches!(
        err(r"\begin{matrix} a \end{pmatrix}").kind,
        ErrorKind::MismatchedEnvironment { .. }
    ));
    assert!(matches!(
        err(r"\begin{foo} a \end{foo}").kind,
        ErrorKind::UnknownEnvironment(_)
    ));
}

#[test]
fn half_streamed_formulas_fail_cleanly() {
    let full = r"\frac{-b \pm \sqrt{b^2 - 4ac}}{2a} + \left( \sum_{i=1}^{n} x_i \right)";
    for end in 0..=full.len() {
        if !full.is_char_boundary(end) {
            continue;
        }
        let _ = layout(&full[..end], stix(), &Options::default());
    }
}

#[test]
fn limits_are_enforced() {
    let deep = "{".repeat(500) + &"}".repeat(500);
    assert_eq!(
        layout(&deep, stix(), &Options::default()).unwrap_err().kind,
        ErrorKind::TooDeep
    );
    let nested = r"\frac".repeat(300) + &"1".repeat(301);
    assert_eq!(
        layout(&nested, stix(), &Options::default())
            .unwrap_err()
            .kind,
        ErrorKind::TooDeep
    );
    let long = "x".repeat(20_000);
    assert_eq!(
        layout(&long, stix(), &Options::default()).unwrap_err().kind,
        ErrorKind::TooLong
    );
    let many = "x".repeat(10_000);
    let opts = Options {
        max_items: 1000,
        ..Options::default()
    };
    assert_eq!(
        layout(&many, stix(), &opts).unwrap_err().kind,
        ErrorKind::TooLarge
    );
}

#[test]
fn random_input_never_panics() {
    // xorshift over an alphabet dense in TeX syntax.
    let alphabet: Vec<char> =
        r"\{}^_&'[]()|.x1 +=-\frac\sqrt\left\right\begin{matrix}\end{matrix}\\%$~αβ∑"
            .chars()
            .collect();
    let mut s: u64 = 0x9E37_79B9_7F4A_7C15;
    for _ in 0..20_000 {
        let len = (s % 40) as usize;
        let mut src = String::new();
        for _ in 0..len {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            src.push(alphabet[(s % alphabet.len() as u64) as usize]);
        }
        let _ = layout(&src, stix(), &Options::default());
        let _ = layout(&src, stix(), &Options::display(12.0));
    }
}

#[test]
fn missing_glyph_falls_back_or_errors() {
    // Every font has plain letters; a private-use code point has no glyph.
    let e = layout("x + \u{378}", stix(), &Options::default()).unwrap_err();
    assert_eq!(e.kind, ErrorKind::MissingGlyph('\u{378}'));
    assert_eq!(e.range, 4..6);
    // A command's error covers the whole command.
    let fira = MathFont::new(FIRA).unwrap();
    let e = layout(r"A \bigcup B", &fira, &Options::default()).unwrap_err();
    assert_eq!(e.kind, ErrorKind::MissingGlyph('⋃'));
    assert_eq!(e.range, 2..9);
    // A look-alike stands in when the whole chain lacks a symbol.
    assert!(layout(r"A \setminus B", &fira, &Options::default()).is_ok());
}

#[test]
fn svg_renders() {
    let svg = inline(r"\frac{1}{2}").to_svg(stix());
    assert!(svg.starts_with("<svg") && svg.contains("<path d=\"M"));
}

#[test]
fn bare_rows_become_an_alignment() {
    let f = display(r"a &= b \\ &= c");
    assert!(f.height() > display("a = b").height() * 1.8);
}

#[test]
fn item_is_small() {
    assert!(std::mem::size_of::<Item>() <= 20);
}

/// Every command in the symbol tables, scraped from the source.
fn symbol_commands() -> Vec<String> {
    let mut names = Vec::new();
    for line in include_str!("symbols.rs").lines() {
        let line = line.trim();
        if !line.starts_with('"') || !line.contains("=>") {
            continue;
        }
        for part in line.split("=>").next().unwrap().split('|') {
            let name = part.trim().trim_matches('"');
            if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphabetic()) {
                names.push(name.to_string());
            }
        }
    }
    assert!(names.len() > 300, "{}", names.len());
    names
}

#[test]
fn every_symbol_resolves_in_every_font() {
    let names = symbol_commands();
    for font in fonts() {
        for name in &names {
            let src = format!("\\{name}{{x}}");
            if let Err(e) = layout(&src, font, &Options::display(16.0)) {
                assert!(
                    !matches!(e.kind, ErrorKind::MissingGlyph(_)),
                    "{}: \\{name}: {e}",
                    font.name().unwrap()
                );
            }
        }
    }
}

#[test]
fn alphabets_come_from_the_fallback_when_missing() {
    for font in fonts() {
        for v in [
            "mathbb",
            "mathcal",
            "mathfrak",
            "mathbf",
            "mathsf",
            "mathtt",
            "boldsymbol",
        ] {
            for c in ('A'..='Z').chain('a'..='z') {
                let styled =
                    layout(&format!("\\{v}{{{c}}}"), font, &Options::inline(16.0)).unwrap();
                let plain =
                    layout(&format!("\\mathrm{{{c}}}"), font, &Options::inline(16.0)).unwrap();
                assert_ne!(
                    styled.items,
                    plain.items,
                    "{}: \\{v}{{{c}}}",
                    font.name().unwrap()
                );
            }
        }
    }
    // Fira has no script letters: they come from STIX, font 1 of its chain.
    let f = layout(r"\mathcal{L}", &fonts()[2], &Options::inline(16.0)).unwrap();
    assert!(matches!(f.items[0], Item::Glyph { font: 1, .. }));
    // Its own letters stay its own.
    let f = layout("x", &fonts()[2], &Options::inline(16.0)).unwrap();
    assert!(matches!(f.items[0], Item::Glyph { font: 0, .. }));
}

#[test]
fn nested_middle_is_bounded() {
    // Each `\middle` body is laid out once, so nesting to the depth limit is
    // linear work, not 2^31.
    let n = 31;
    let src = "\\left.".repeat(n) + &"\\middle.\\right.".repeat(n);
    let started = std::time::Instant::now();
    assert!(layout(&src, stix(), &Options::default()).is_ok());
    assert!(
        started.elapsed().as_millis() < 50,
        "{:?}",
        started.elapsed()
    );
    let honest = "\\left(".repeat(8) + "x" + &"\\middle| y \\right)".repeat(8);
    assert!(layout(&honest, stix(), &Options::default()).is_ok());
}

#[test]
fn bad_sizes_are_rejected() {
    for size in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        let e = layout(r"\left(\frac ab\right)", stix(), &Options::display(size)).unwrap_err();
        assert_eq!(e.kind, ErrorKind::BadSize, "{size}");
    }
}

#[test]
fn missing_glyphs_point_at_their_source() {
    let src = "abc + \\operatorname{x\u{378}}";
    let e = layout(src, stix(), &Options::default()).unwrap_err();
    assert_eq!(&src[e.range.clone()], "\u{378}");
}

/// The reviewer's reproducer: `\middle` nesting around a long `\text`.
#[test]
fn nested_middle_with_text_stays_fast() {
    for d in [8, 10, 12, 14, 20, 30] {
        let open = "\\left.\\middle|".repeat(d);
        let fill = 16_000usize.saturating_sub(open.len() + 7 * d + 8);
        let src = format!(
            "{open}\\text{{{}}}{}",
            "x".repeat(fill),
            "\\right.".repeat(d)
        );
        let started = std::time::Instant::now();
        let _ = layout(&src, stix(), &Options::default());
        let took = started.elapsed();
        assert!(took.as_millis() < 250, "d={d}: {took:?}");
    }
    // `\middle` bodies are laid out once: deep honest nesting costs linear work.
    let honest = "\\left(".repeat(30) + "x" + &"\\middle| y \\right)".repeat(30);
    let started = std::time::Instant::now();
    assert!(layout(&honest, stix(), &Options::default()).is_ok());
    assert!(
        started.elapsed().as_millis() < 50,
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn empty_cells_and_parts_count_as_work() {
    // 16 KB of empty cells: bounded by the work budget, never a hang.
    let src = format!("\\begin{{matrix}}{}\\end{{matrix}}", "&".repeat(16_000));
    let started = std::time::Instant::now();
    let _ = layout(&src, stix(), &Options::default());
    assert!(
        started.elapsed().as_millis() < 250,
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn runaway_sizes_are_too_large() {
    let err = |src: &str| layout(src, stix(), &Options::default()).unwrap_err().kind;
    // Dimensions are capped after unit conversion.
    assert_eq!(err(r"a\kern1000in b"), ErrorKind::BadDimension);
    assert_eq!(err(r"a\kern-1000in b"), ErrorKind::BadDimension);
    assert_eq!(err(r"a \\[1000in] b"), ErrorKind::BadDimension);
    assert_eq!(err(r"\mkern2000mu"), ErrorKind::BadDimension);
    // Within each cap, but adding up to a runaway box.
    let wide = r"a\kern99em".repeat(30);
    assert_eq!(err(&wide), ErrorKind::TooLarge);
    let tall = format!("x{}", r"\\".repeat(8000));
    assert_eq!(err(&tall), ErrorKind::TooLarge);
    // A kern out and back still lands its items far away.
    let hidden = format!("{}b{}", r"\kern99em".repeat(30), r"\kern-99em".repeat(30));
    assert_eq!(err(&hidden), ErrorKind::TooLarge);
    // Room is bounded by the source: a few bytes of spacing can't reserve a
    // page of blank slot.
    assert_eq!(err(r"\kern50em x"), ErrorKind::TooLarge);
    assert_eq!(err(&r"\hspace{100em}".repeat(19)), ErrorKind::TooLarge);
    assert_eq!(err(r"a\\[100em]b"), ErrorKind::TooLarge);
    // Ordinary large formulas are fine.
    assert!(layout(r"\kern10em x", stix(), &Options::default()).is_ok());
    let matrix = format!(
        "\\begin{{pmatrix}}{}\\end{{pmatrix}}",
        "1 \\\\ ".repeat(100)
    );
    assert!(layout(&matrix, stix(), &Options::display(16.0)).is_ok());
}

/// Hosts size a formula's slot from its box, so nothing may paint outside
/// it: vertically the box grows to the ink, and horizontally `left`/`right`
/// cover the ink and the advance box.
#[test]
fn the_box_holds_all_ink() {
    let options = Options::inline(16.0);
    let check = |src: &str| {
        let f = layout(src, stix(), &options).unwrap_or_else(|e| panic!("{src}: {e}"));
        let ink = crate::ink_bounds(&f.items, stix());
        assert!(ink.top <= f.ascent + 1e-3, "{src}: ink above the box");
        assert!(ink.bottom <= f.descent + 1e-3, "{src}: ink below the box");
        assert!(
            ink.left >= f.left - 1e-3 && f.left <= f.width.min(0.0),
            "{src}"
        );
        assert!(
            ink.right <= f.right + 1e-3 && f.right >= f.width.max(0.0),
            "{src}"
        );
        f
    };
    // A negative row gap moves a row out of the TeX box; the box follows.
    let spoof = check(r"\text{Approve}\\[-40em]\\");
    assert!(spoof.descent >= 19.0 * 16.0, "{}", spoof.descent);
    for src in [
        r"x^2 + \frac{a}{b}",
        r"\int\!\!\!\int_D f",
        r"a\hspace{-3em}b",
        r"\sum_{i=1}^n \overbrace{a+b}^{c}",
        r"a\\[-0.5em]b",
        r"\begin{matrix} a \\[-2em] b \end{matrix}",
    ] {
        check(src);
    }
    // Negative space: the reported extent covers ink left of the origin and
    // the negative advance.
    let left = check(r"\hspace{-1em}x");
    assert!(left.left < -0.4 * 16.0 && left.width < 0.0, "{}", left.left);
    let hidden = check(r"\hspace{-30em}\hspace{-30em}");
    assert!(hidden.span() >= 59.0 * 16.0, "{}", hidden.span());
    for src in [
        r"x\\[-100em]x\\[-100em]x\\[-100em]x".to_owned(),
        r"\hspace{-100em}".repeat(25),
        r"\hspace{-100em}\hspace{-100em}".to_owned(),
    ] {
        assert_eq!(
            layout(&src, stix(), &options)
                .map(|_| ())
                .map_err(|e| e.kind),
            Err(ErrorKind::TooLarge),
            "{src}"
        );
    }
    // Bounds that overflow to infinity refuse rather than pass everything.
    assert_eq!(
        layout("x", stix(), &Options::inline(1e36))
            .map(|_| ())
            .map_err(|e| e.kind),
        Err(ErrorKind::TooLarge)
    );
}

#[test]
fn rules_over_negative_content_are_empty() {
    for tex in [
        r"\overline{\hspace{-5em}}",
        r"\sqrt{\hspace{-2em}}",
        r"\frac{\hspace{-3em}}{\hspace{-3em}}",
        r"\boxed{\hspace{-4em}}",
    ] {
        for item in &inline(tex).items {
            if let Item::Rule { width, height, .. } = item {
                assert!(*width >= 0.0 && *height >= 0.0, "{tex}: {item:?}");
            }
        }
    }
}
