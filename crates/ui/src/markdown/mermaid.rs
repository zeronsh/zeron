//! Isolated native Mermaid adapter. Call only on a background executor.
use crate::theme::Theme;
use std::sync::Mutex;

pub const ENGINE_VERSION: &str = "mermaid-rs-renderer/0.3.1";
pub const MAX_SOURCE_BYTES: usize = 16 * 1024;
// This backend has no cooperative cancellation. Bound inputs and serialize its
// CPU work across previews; UI owners discard results from superseded revisions.
static RENDER_LOCK: Mutex<()> = Mutex::new(());

/// Zeron's diagram style. The canvas is left transparent so the diagram sits
/// on its fence body; `canvas` is that body's opaque approximation, used where
/// the engine needs a solid mask (edge label pills, hollow markers).
#[derive(Clone)]
pub struct Palette {
    dark: bool,
    font: String,
    canvas: String,
    node: String,
    group: String,
    text: String,
    label: String,
    line: String,
    border: String,
    grid: String,
    accent_line: String,
    accent_wash: String,
}

fn color(color: gpui::Hsla, background: gpui::Hsla) -> String {
    let mut c = color.to_rgb();
    let bg = background.to_rgb();
    c.r = c.r * c.a + bg.r * (1.0 - c.a);
    c.g = c.g * c.a + bg.g * (1.0 - c.a);
    c.b = c.b * c.a + bg.b * (1.0 - c.a);
    format!(
        "#{:02x}{:02x}{:02x}",
        (c.r * 255.0).round() as u8,
        (c.g * 255.0).round() as u8,
        (c.b * 255.0).round() as u8
    )
}

impl Palette {
    pub fn from_theme(theme: &Theme) -> Self {
        let dark = theme.appearance.is_dark();
        let canvas = Self::plate(theme);
        // Nodes are cards lifted off the fence: white in light, one ink step
        // up in dark, where the panel is already the deepest plane.
        let node = if dark {
            canvas.blend(theme.ink(0.06))
        } else {
            theme.bg
        };
        Self {
            dark,
            font: theme.font_sans.to_string(),
            canvas: color(canvas, theme.bg),
            node: color(node, canvas),
            group: color(theme.ink(0.03), canvas),
            text: color(theme.text, node),
            label: color(theme.text_muted, canvas),
            line: color(theme.text_faint, canvas),
            border: color(theme.border_strong, canvas),
            grid: color(theme.border, canvas),
            accent_line: color(theme.accent.opacity(0.6), canvas),
            accent_wash: color(theme.accent.opacity(0.12), node),
        }
    }

    /// The fence body, an ink wash over the panel (see `code_block_frame`).
    /// Also the fill behind a diagram shown off its fence, as in the lightbox.
    pub fn plate(theme: &Theme) -> gpui::Hsla {
        theme.bg.blend(theme.ink(0.035))
    }
}

pub fn render(source: &str, palette: &Palette) -> Result<String, String> {
    if source.len() > MAX_SOURCE_BYTES
        || source.lines().count() > 256
        || source
            .split(|c: char| c.is_whitespace() || matches!(c, ';' | '>' | '{' | '}'))
            .count()
            > 2048
    {
        return Err("Diagram exceeds preview complexity limit".into());
    }
    let _guard = RENDER_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::panic::catch_unwind(|| {
        let mut options = mermaid_rs_renderer::RenderOptions::default();
        options.theme = if palette.dark {
            mermaid_rs_renderer::Theme::dark()
        } else {
            mermaid_rs_renderer::Theme::modern()
        };
        // A denser layout than the engine's default: diagrams are scaled to the
        // reading column, so slack spacing shrinks the labels.
        options.layout.node_spacing = 36.0;
        options.layout.rank_spacing = 40.0;
        options.layout.node_padding_x = 18.0;
        options.layout.node_padding_y = 10.0;
        let theme = &mut options.theme;
        theme.font_family = palette.font.clone();
        theme.font_size = 14.0;
        theme.background = palette.canvas.clone();
        theme.primary_color = palette.node.clone();
        theme.primary_text_color = palette.text.clone();
        theme.primary_border_color = palette.border.clone();
        theme.text_color = palette.label.clone();
        theme.line_color = palette.line.clone();
        theme.secondary_color = palette.node.clone();
        theme.tertiary_color = palette.group.clone();
        theme.edge_label_background = palette.canvas.clone();
        theme.cluster_background = palette.group.clone();
        theme.cluster_border = palette.border.clone();
        theme.sequence_actor_fill = palette.node.clone();
        theme.sequence_actor_border = palette.border.clone();
        theme.sequence_actor_line = palette.border.clone();
        theme.sequence_note_fill = palette.accent_wash.clone();
        theme.sequence_note_border = palette.accent_line.clone();
        theme.sequence_activation_fill = palette.accent_wash.clone();
        theme.sequence_activation_border = palette.accent_line.clone();
        if diagram_keyword(source) == Some("gantt") {
            // Gantt derives its bar hues from this color; a neutral gray would
            // turn every section red. The softened accent keeps bar labels
            // legible in both appearances.
            theme.primary_border_color = palette.accent_line.clone();
        }
        let svg =
            mermaid_rs_renderer::render_with_options(source, options).map_err(|e| e.to_string())?;
        let svg = restyle(svg, palette);
        if svg.len() > 2 * 1024 * 1024 {
            return Err("Diagram output exceeds preview size limit".into());
        }
        Ok(svg)
    })
    .unwrap_or_else(|_| Err("Diagram could not be rendered".into()))
}

/// The diagram type: the first word after any front matter and comments.
fn diagram_keyword(source: &str) -> Option<&str> {
    let mut lines = source
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .peekable();
    if lines.next_if_eq(&"---").is_some() {
        lines.by_ref().find(|line| *line == "---");
    }
    lines
        .find(|line| !line.starts_with("%%"))
        .and_then(|line| line.split_whitespace().next())
}

/// Finishing touches the engine's theme cannot express, applied to its
/// pinned output: a transparent canvas, softer node corners, cards for
/// boxes the engine fills with its background, decisions in the accent and
/// themed Gantt gridlines. Only elements still carrying the palette's own
/// colors change, so explicit `style`/`classDef` colors survive.
fn restyle(svg: String, palette: &Palette) -> String {
    let canvas_fill = format!(" fill=\"{}\"", palette.canvas);
    let node_fill = format!(" fill=\"{}\"", palette.node);
    let border_stroke = format!(" stroke=\"{}\"", palette.border);
    let mut out = String::with_capacity(svg.len());
    let mut rest = svg.as_str();
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let Some(end) = rest.find('>') else { break };
        let tag = &rest[..=end];
        rest = &rest[end + 1..];
        if is_canvas(tag, palette) {
            continue;
        }
        if tag.starts_with("<polygon ") && is_default_diamond(tag, palette) {
            out.push_str(
                &tag.replacen(&node_fill, &format!(" fill=\"{}\"", palette.accent_wash), 1)
                    .replacen(
                        &border_stroke,
                        &format!(" stroke=\"{}\"", palette.accent_line),
                        1,
                    ),
            );
        } else if tag.starts_with("<rect ") {
            let mut tag = tag.replacen(" rx=\"3\" ry=\"3\" ", " rx=\"8\" ry=\"8\" ", 1);
            if tag.contains(&border_stroke) {
                tag = tag.replacen(&canvas_fill, &node_fill, 1);
            }
            out.push_str(&tag);
        } else if tag.starts_with("<line ") {
            out.push_str(&tag.replacen(
                " stroke=\"#E2E8F0\"",
                &format!(" stroke=\"{}\"", palette.grid),
                1,
            ));
        } else {
            out.push_str(tag);
        }
    }
    out.push_str(rest);
    out
}

/// `name="value"` pairs of a start tag, as the engine writes them.
fn attributes(tag: &str) -> Vec<(&str, &str)> {
    let parts: Vec<_> = tag.split('"').collect();
    parts
        .as_chunks::<2>()
        .0
        .iter()
        .filter_map(|[name, value]| {
            let name = name.trim_end().strip_suffix('=')?;
            Some((name.rsplit(' ').next()?, *value))
        })
        .collect()
}

/// A full-bleed background: a plain rectangle in the canvas color.
fn is_canvas(tag: &str, palette: &Palette) -> bool {
    if !tag.starts_with("<rect ") {
        return false;
    }
    let mut fill = None;
    for (name, value) in attributes(tag) {
        match name {
            "x" | "y" | "width" | "height" => {}
            "fill" => fill = Some(value),
            _ => return false,
        }
    }
    fill == Some(palette.canvas.as_str())
}

fn is_default_diamond(tag: &str, palette: &Palette) -> bool {
    let attrs = attributes(tag);
    let get = |key: &str| attrs.iter().find(|(name, _)| *name == key).map(|(_, v)| *v);
    if get("fill") != Some(palette.node.as_str()) || get("stroke") != Some(palette.border.as_str())
    {
        return false;
    }
    let Some(points) = get("points") else {
        return false;
    };
    let points: Vec<(f32, f32)> = points
        .split_whitespace()
        .filter_map(|point| {
            let (x, y) = point.split_once(',')?;
            Some((x.parse().ok()?, y.parse().ok()?))
        })
        .collect();
    let [top, right, bottom, left] = points[..] else {
        return false;
    };
    (top.0 - bottom.0).abs() < 0.05
        && (left.1 - right.1).abs() < 0.05
        && left.0 < top.0
        && top.0 < right.0
        && top.1 < left.1
        && left.1 < bottom.1
}

#[cfg(test)]
mod tests {
    use super::*;
    const CORPUS: &[(&str, &str)] = &[
        (
            "flowchart",
            include_str!("../../../../scripts/fixtures/markdown-preview/flowchart.mmd"),
        ),
        (
            "sequence",
            include_str!("../../../../scripts/fixtures/markdown-preview/sequence.mmd"),
        ),
        (
            "class",
            include_str!("../../../../scripts/fixtures/markdown-preview/class.mmd"),
        ),
        (
            "state",
            include_str!("../../../../scripts/fixtures/markdown-preview/state.mmd"),
        ),
        (
            "er",
            include_str!("../../../../scripts/fixtures/markdown-preview/er.mmd"),
        ),
        (
            "gantt",
            include_str!("../../../../scripts/fixtures/markdown-preview/gantt.mmd"),
        ),
    ];
    #[test]
    fn corpus_renders_through_gpui_in_both_themes() {
        let renderer = gpui::SvgRenderer::new(std::sync::Arc::new(crate::icons::Assets));
        let mut light_mono = Theme::light();
        light_mono.font_sans = "Geist Mono".into();
        let mut dark_mono = Theme::dark();
        dark_mono.font_sans = "Geist Mono".into();
        for (mode, theme) in [
            ("light", Theme::light()),
            ("dark", Theme::dark()),
            ("light-mono", light_mono),
            ("dark-mono", dark_mono),
        ] {
            let palette = Palette::from_theme(&theme);
            for (name, source) in CORPUS {
                let svg = render(source, &palette).unwrap_or_else(|e| panic!("{mode}/{name}: {e}"));
                assert!(!svg.contains("<foreignObject"));
                let raster = renderer.render_single_frame(svg.as_bytes(), 1.0).unwrap();
                assert!(raster.size(0).width.0 > 0);
                let prepared =
                    crate::image_media::decode_image("image/svg+xml", svg.as_bytes().to_vec())
                        .unwrap();
                let prepared_raster = prepared
                    .image
                    .to_image_data(gpui::SvgRenderer::new(std::sync::Arc::new(
                        crate::icons::Assets,
                    )))
                    .unwrap();
                let size = prepared_raster.size(0);
                assert!(size.width.0 > 0 && size.height.0 > 0);
                assert!(size.width.0 <= 4096 && size.height.0 <= 4096);
                assert!(size.width.0 as usize * size.height.0 as usize <= 1024 * 1024);
                let ratio = size.width.0 as f32 / size.height.0 as f32;
                assert!((ratio / (prepared.width / prepared.height) - 1.0).abs() < 0.02);
                assert_eq!(svg, render(source, &palette).unwrap());
                if let Ok(dir) = std::env::var("ZERON_MERMAID_ARTIFACTS") {
                    std::fs::create_dir_all(&dir).unwrap();
                    std::fs::write(format!("{dir}/{name}-{mode}.svg"), svg).unwrap();
                    let size = prepared_raster.size(0);
                    let mut rgba = prepared_raster.as_bytes(0).unwrap().to_vec();
                    for pixel in rgba.chunks_exact_mut(4) {
                        pixel.swap(0, 2);
                    }
                    image::save_buffer(
                        format!("{dir}/{name}-{mode}.png"),
                        &rgba,
                        size.width.0 as u32,
                        size.height.0 as u32,
                        image::ColorType::Rgba8,
                    )
                    .unwrap();
                }
            }
        }
    }
    #[test]
    fn malformed_and_oversized_diagrams_are_recoverable() {
        let palette = Palette::from_theme(&Theme::dark());
        assert!(render("this is not a diagram", &palette).is_err());
        assert!(render(&"x".repeat(MAX_SOURCE_BYTES + 1), &palette).is_err());
        assert!(render("flowchart TD\nA[Hola<br/>mundo] --> B[Fin]", &palette).is_ok());
    }

    #[test]
    fn diagrams_take_zeron_style_and_keep_explicit_colors() {
        for theme in [Theme::light(), Theme::dark()] {
            let palette = Palette::from_theme(&theme);
            let svg = render(
                "flowchart TD\nA[Inicio] --> B{¿Listo?}\nB --> C[Fin]\nB --> D{Otra}\nstyle D fill:#dbeafe,stroke:#2563eb",
                &palette,
            )
            .unwrap();
            // The fence body shows through: no full-bleed background remains.
            assert!(!svg.contains(&format!("fill=\"{}\"/>", palette.canvas)));
            assert!(svg.contains(" rx=\"8\" ry=\"8\" "));
            assert!(!svg.contains(" rx=\"3\" ry=\"3\" "));
            let diamonds: Vec<_> = svg.match_indices("<polygon ").collect();
            let accented = svg
                .matches(&format!(
                    "fill=\"{}\" stroke=\"{}\"",
                    palette.accent_wash, palette.accent_line
                ))
                .count();
            assert!(diamonds.len() >= 2);
            assert_eq!(accented, 1, "only the default-colored decision is tinted");
            assert!(svg.contains("fill=\"#dbeafe\" stroke=\"#2563eb\""));

            let gantt = render(
                "%% plan\ngantt\ndateFormat YYYY-MM-DD\nsection A\nTask :2026-09-08, 2d",
                &palette,
            )
            .unwrap();
            assert!(!gantt.contains("#E2E8F0"));
        }
    }

    #[test]
    fn diagram_keyword_skips_front_matter_and_comments() {
        assert_eq!(diagram_keyword("gantt\n  title X"), Some("gantt"));
        assert_eq!(
            diagram_keyword("---\ntitle: Plan\n---\n%% note\n\n gantt"),
            Some("gantt")
        );
        assert_eq!(diagram_keyword("flowchart LR; A-->B"), Some("flowchart"));
        assert_eq!(diagram_keyword("  \n%% only"), None);
    }
}
