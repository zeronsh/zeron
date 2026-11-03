//! Warm layout throughput and output size over a typical formula set.
//!
//! cargo run --release -p zatex --example bench

use std::time::Instant;

use zatex::{Item, MathFont, Options, layout};

const FORMULAS: &[&str] = &[
    r"x^2 + y^2 = z^2",
    r"\frac{-b \pm \sqrt{b^2 - 4ac}}{2a}",
    r"\sum_{i=1}^{n} i = \frac{n(n+1)}{2}",
    r"\int_0^\infty e^{-x^2}\,dx = \frac{\sqrt{\pi}}{2}",
    r"\lim_{x \to 0} \frac{\sin x}{x} = 1",
    r"\left( \frac{a}{b} \right)^2",
    r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}",
    r"f(x) = \begin{cases} x^2 & \text{if } x \ge 0 \\ -x & \text{otherwise} \end{cases}",
    r"\operatorname{softmax}(z)_i = \frac{e^{z_i}}{\sum_j e^{z_j}}",
    r"\nabla \times \mathbf{B} = \mu_0 \mathbf{J} + \mu_0\varepsilon_0 \frac{\partial \mathbf{E}}{\partial t}",
];

fn main() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/fonts/STIXTwoMath-Regular.otf");
    let font = MathFont::from_vec(std::fs::read(path).unwrap()).unwrap();
    for display in [false, true] {
        let opts = Options {
            display,
            ..Options::inline(16.0)
        };
        // Warm the glyph metric cache.
        for src in FORMULAS {
            layout(src, &font, &opts).unwrap();
        }
        let rounds = 2000;
        let started = Instant::now();
        let mut items = 0;
        for _ in 0..rounds {
            for src in FORMULAS {
                items += layout(src, &font, &opts).unwrap().items.len();
            }
        }
        let per = started.elapsed() / (rounds * FORMULAS.len() as u32);
        let avg_items = items as f64 / (rounds as f64 * FORMULAS.len() as f64);
        println!(
            "{}: {per:?} per formula, {avg_items:.1} items ≈ {:.0} bytes of display list",
            if display { "display" } else { "inline " },
            avg_items * std::mem::size_of::<Item>() as f64
        );
    }
}
