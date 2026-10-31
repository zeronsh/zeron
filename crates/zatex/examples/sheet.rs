//! Renders formulas side by side in several math fonts to a PNG.
//!
//! cargo run -p zatex --example sheet -- out.png [formula…]
//!
//! With no formulas, renders a built-in sample set. Fonts: the six faces
//! `zatex::bundled` ships (each but STIX Two falling back to STIX Two).

use std::sync::Arc;

use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Transform};
use zatex::{MathFont, Options, PathSink, layout};

const SAMPLES: &[&str] = &[
    r"x^2 + y^2 = z^2 \quad e^{i\pi} + 1 = 0",
    r"\frac{-b \pm \sqrt{b^2 - 4ac}}{2a}",
    r"\sum_{i=1}^{n} i = \frac{n(n+1)}{2} \qquad \int_0^\infty e^{-x^2}\,dx = \frac{\sqrt{\pi}}{2}",
    r"\lim_{x \to 0} \frac{\sin x}{x} = 1 \qquad \prod_{k=1}^{n} (1 + x_k)",
    r"\left( \frac{a}{b} \right)^2 \left[ \frac{\frac{1}{2}}{3} \right] \left\{ x \in \mathbb{Z} \middle| x > 0 \right\}",
    r"f(x) = \begin{cases} x^2 & \text{if } x \ge 0 \\ -x & \text{otherwise} \end{cases}",
    r"\begin{pmatrix} a & b \\ c & d \end{pmatrix} \begin{bmatrix} 1 & 0 & 0 \\ 0 & 1 & 0 \\ 0 & 0 & 1 \end{bmatrix} \begin{vmatrix} a & b \\ c & d \end{vmatrix}",
    r"\begin{aligned} (a+b)^2 &= a^2 + 2ab + b^2 \\ &\le 2(a^2+b^2) \end{aligned}",
    r"\hat{x}\ \bar{y}\ \vec{v}\ \dot{x}\ \ddot{x}\ \tilde{n}\ \hat{\imath}\ \widehat{abc}\ \overline{z + w}\ \overrightarrow{AB}",
    r"\overbrace{1 + 2 + \cdots + n}^{n \text{ terms}} \quad \underbrace{a + b}_{\text{sum}}",
    r"\binom{n}{k} = \frac{n!}{k!(n-k)!} \qquad \sqrt[3]{x^3 + y^3} \qquad \sqrt{1+\sqrt{1+\sqrt{x}}}",
    r"\operatorname{softmax}(z)_i = \frac{e^{z_i}}{\sum_j e^{z_j}} \qquad \arg\max_\theta \log p_\theta(x)",
    r"\mathbb{R} \ \mathcal{L} \ \mathfrak{g} \ \mathbf{v} \ \boldsymbol{\beta} \ \mathsf{A} \ \mathtt{x} \quad \alpha\beta\gamma\delta\epsilon\theta\lambda\pi\sigma\phi\omega \ \Gamma\Delta\Sigma\Omega",
    r"\nabla \times \mathbf{B} = \mu_0 \mathbf{J} + \mu_0\varepsilon_0 \frac{\partial \mathbf{E}}{\partial t}",
    r"\bigl( \Bigl( \biggl( \Biggl( x \Biggr) \biggr) \Bigr) \bigr) \quad \left( \begin{matrix} 1 \\ 2 \\ 3 \\ 4 \\ 5 \end{matrix} \right)",
    r"A \xrightarrow{f} B \xleftarrow[g]{h} C \quad a \stackrel{\text{def}}{=} b \quad \boxed{x = 42} \quad a \not\in B",
    r"\bigcup_{i=1}^n A_i \quad \bigoplus_k V_k \quad \oint_C \mathbf{F}\cdot d\mathbf{r} \quad A \setminus B",
];

struct Skia(PathBuilder);

impl PathSink for Skia {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.move_to(x, y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.line_to(x, y);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.0.quad_to(x1, y1, x, y);
    }
    fn cubic_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.0.cubic_to(x1, y1, x2, y2, x, y);
    }
    fn close(&mut self) {
        self.0.close();
    }
}

fn load(path: &str) -> MathFont {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    MathFont::from_vec(bytes).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn main() {
    let mut args = std::env::args().skip(1);
    let out = args.next().unwrap_or_else(|| "sheet.png".into());
    let custom: Vec<String> = args.collect();
    let formulas: Vec<&str> = if custom.is_empty() {
        SAMPLES.to_vec()
    } else {
        custom.iter().map(String::as_str).collect()
    };

    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/fonts");
    let stix = Arc::new(load(&format!("{dir}/STIXTwoMath-Regular.otf")));
    let fonts = [
        load(&format!("{dir}/FiraMath-Regular.otf")).with_fallback(stix.clone()),
        load(&format!("{dir}/STIXTwoMath-Regular.otf")),
        load(&format!("{dir}/NewCMMath-Book.otf")).with_fallback(stix.clone()),
        load(&format!("{dir}/LibertinusMath-Regular.otf")).with_fallback(stix.clone()),
        load(&format!("{dir}/texgyrepagella-math.otf")).with_fallback(stix.clone()),
        load(&format!("{dir}/Euler-Math.otf")).with_fallback(stix),
    ];

    let scale = 2.0;
    let size = 18.0;
    let pad = 16.0;
    let mut rows = Vec::new();
    for src in &formulas {
        let mut cells = Vec::new();
        for font in &fonts {
            let started = std::time::Instant::now();
            let formula = layout(src, font, &Options::display(size));
            let took = started.elapsed();
            match &formula {
                Ok(f) => eprintln!(
                    "{:>8.1?} {:>4} items  {}: {src}",
                    took,
                    f.items.len(),
                    font.name().unwrap_or_default()
                ),
                Err(e) => eprintln!("ERROR {}: {src}: {e}", font.name().unwrap_or_default()),
            }
            cells.push(formula.ok());
        }
        rows.push(cells);
    }
    let col_w: Vec<f32> = (0..fonts.len())
        .map(|j| {
            rows.iter()
                .filter_map(|r| r[j].as_ref().map(|f| f.width))
                .fold(0.0, f32::max)
        })
        .collect();
    let row_h: Vec<f32> = rows
        .iter()
        .map(|r| r.iter().flatten().map(|f| f.height()).fold(size, f32::max))
        .collect();
    let width = col_w.iter().sum::<f32>() + pad * (fonts.len() + 1) as f32;
    let height = row_h.iter().sum::<f32>() + pad * (rows.len() + 1) as f32;
    let mut pixmap = Pixmap::new((width * scale) as u32, (height * scale) as u32).unwrap();
    pixmap.fill(tiny_skia::Color::WHITE);
    let mut ink = Paint::default();
    ink.set_color_rgba8(20, 20, 24, 255);
    ink.anti_alias = true;
    let mut guide = Paint::default();
    guide.set_color_rgba8(235, 238, 245, 255);
    let transform = Transform::from_scale(scale, scale);

    let mut y = pad;
    for (i, row) in rows.iter().enumerate() {
        let mut x = pad;
        for (j, cell) in row.iter().enumerate() {
            if let Some(f) = cell {
                let top = y + (row_h[i] - f.height()) / 2.0;
                // The formula's box, so ink overhang is visible.
                let rect =
                    tiny_skia::Rect::from_xywh(x, top, f.width.max(0.5), f.height().max(0.5));
                if let Some(rect) = rect {
                    pixmap.fill_rect(rect, &guide, transform, None);
                }
                let mut sink = Skia(PathBuilder::new());
                f.paint(&fonts[j], x, top + f.ascent, &mut sink);
                if let Some(path) = sink.0.finish() {
                    pixmap.fill_path(&path, &ink, FillRule::Winding, transform, None);
                }
            }
            x += col_w[j] + pad;
        }
        y += row_h[i] + pad;
    }
    pixmap.save_png(&out).unwrap();
    eprintln!("wrote {out}");
}
