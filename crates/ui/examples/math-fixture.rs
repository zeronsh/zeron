//! Renders Markdown with LaTeX math through the transcript renderer and saves
//! the frame as `math-light.png` / `math-dark.png` for visual review:
//!
//! ```sh
//! cargo run --release -p zeron-ui --example math-fixture --features math-fixture -- /tmp/zeron-math
//! ```
//!
//! `ZERON_MATH_FIXTURE_DOC=<file.md>` renders another document.
use std::path::PathBuf;
use std::time::Duration;

use gpui::{
    AppContext, Bounds, Context, IntoElement, ParentElement, Render, Styled, Window, WindowBounds,
    WindowOptions, div, point, px, size,
};
use zeron_ui::markdown::{self, BlockTree};
use zeron_ui::theme::Theme;
use zeron_ui::*;

const DOC: &str = r#"## Utility maximization

A household maximizes $U(x_1, x_2) = x_1^{\alpha} x_2^{1-\alpha}$ subject to the budget constraint \(p_1 x_1 + p_2 x_2 = m\). The Lagrangian is

$$
\mathcal{L} = x_1^{\alpha} x_2^{1-\alpha} + \lambda \left(m - p_1 x_1 - p_2 x_2\right)
$$

The first-order conditions give $MRS_{1,2} = \frac{\alpha}{1-\alpha} \cdot \frac{x_2}{x_1} = \frac{p_1}{p_2}$, so **$x_1^* = \frac{\alpha m}{p_1}$**.

- Prices like $5 or $5-$10 stay text, `$x$` stays code.
- Estimator: \(\hat{\beta} = (X^\top X)^{-1} X^\top y\) with \(\operatorname{Var}(\hat\beta) = \sigma^2 (X^\top X)^{-1}\).

\[
f(x) = \begin{cases} x^2 & \text{if } x \ge 0 \\ -x & \text{otherwise} \end{cases}
\qquad
\begin{pmatrix} a & b \\ c & d \end{pmatrix}^{-1} = \frac{1}{ad-bc}\begin{pmatrix} d & -b \\ -c & a \end{pmatrix}
\]

| Quantity | Formula |
|---|---|
| Expected value | $\mathbb{E}[X] = \sum_x x \, p(x)$ |
| Variance | $\operatorname{Var}(X) = \mathbb{E}[X^2] - \mathbb{E}[X]^2$ |

> Quoted: $\lim_{n \to \infty} \left(1 + \frac{1}{n}\right)^n = e$ and a broken $\frac{a}$ stays source text.

### Heading with $\int_0^1 x \, dx = \tfrac{1}{2}$

A matrix in running text $A = \begin{pmatrix} 1 & 2 \\ 3 & 4 \end{pmatrix}$ spreads only this paragraph so its lines do not overlap. The paragraph deliberately runs over several lines to make the line spacing visible.

$$
\sum_{i=1}^{n} \sum_{j=1}^{m} a_{ij} x_i y_j + \int_0^\infty e^{-t} t^{z-1} \, dt + \prod_{k=1}^{K} \left(1 + \frac{r_k}{100}\right) - \lim_{h \to 0} \frac{f(x+h) - f(x)}{h}
$$

Too wide for the column (scrolls sideways):

$$
a_1 + a_2 + a_3 + a_4 + a_5 + a_6 + a_7 + a_8 + a_9 + a_{10} + a_{11} + a_{12} + a_{13} + a_{14} + a_{15} + a_{16} + a_{17} + a_{18} + a_{19} + a_{20} + a_{21} + a_{22}
$$

Inline too long: $b_1 + b_2 + b_3 + b_4 + b_5 + b_6 + b_7 + b_8 + b_9 + b_{10} + b_{11} + b_{12} + b_{13} + b_{14} + b_{15} + b_{16} + b_{17} + b_{18} + b_{19} + b_{20} + b_{21}$ then text.
"#;

struct MathDoc {
    tree: BlockTree,
}

impl Render for MathDoc {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let opts = markdown::render::RenderOptions::settled("math-fixture".into());
        div()
            .size_full()
            .bg(theme.bg)
            .text_color(theme.text)
            .p(px(32.0))
            .child(div().w(px(736.0)).child(markdown::render::render_tree(
                &self.tree,
                &opts,
                &theme,
                window,
                &|_| None,
            )))
    }
}

fn main() -> anyhow::Result<()> {
    let out = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/zeron-math"));
    std::fs::create_dir_all(&out)?;
    let doc = match std::env::var_os("ZERON_MATH_FIXTURE_DOC") {
        Some(path) => std::fs::read_to_string(path)?,
        None => DOC.to_owned(),
    };
    let runtime = tokio::runtime::Runtime::new()?;
    let _guard = runtime.enter();
    tracing_subscriber::fmt().with_env_filter("warn").init();
    let temp = tempfile::tempdir()?;
    let data = temp.path().to_path_buf();
    gpui_platform::application()
        .with_assets(icons::Assets)
        .run(move |cx| {
            gpui_tokio::init(cx);
            gpui_base::init(cx);
            let settings = settings::UiSettings::default();
            settings::init(settings.clone(), data.clone(), cx);
            let fonts = typography::register_fonts(cx);
            typography::init(
                settings.ui_font_family.clone(),
                settings.ui_font_size,
                settings.terminal_font_family.clone(),
                settings.terminal_font_size,
                settings.code_font_family.clone(),
                settings.code_font_size,
                fonts,
                cx,
            );
            let tree = markdown::parse_full(&doc);
            cx.spawn(async move |cx| {
                for (name, theme) in [("light", Theme::light()), ("dark", Theme::dark())] {
                    let tree = tree.clone();
                    let window = cx
                        .update(|cx| {
                            cx.set_global(theme);
                            cx.open_window(
                                WindowOptions {
                                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                                        point(px(40.0), px(40.0)),
                                        size(px(800.0), px(1180.0)),
                                    ))),
                                    ..Default::default()
                                },
                                |_, cx| cx.new(|_| MathDoc { tree }),
                            )
                        })
                        .expect("window opens");
                    cx.background_executor()
                        .timer(Duration::from_millis(1500))
                        .await;
                    let path = out.join(format!("math-{name}.png"));
                    window
                        .update(cx, |_, window, _| {
                            window.refresh();
                        })
                        .ok();
                    cx.background_executor()
                        .timer(Duration::from_millis(500))
                        .await;
                    window
                        .update(cx, |_, window, _| -> anyhow::Result<()> {
                            window.render_to_image()?.save(&path)?;
                            Ok(())
                        })
                        .expect("window alive")
                        .expect("frame saved");
                    eprintln!("wrote {}", path.display());
                    window
                        .update(cx, |_, window, _| window.remove_window())
                        .ok();
                }
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    Ok(())
}
