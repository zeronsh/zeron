//! `zeron.measure_text` and `zeron.load_font`: text measured as the plugin's
//! window measures it (a canvas's `c:measure_text`), with no window. zgx's
//! own shaper, over the fonts a window has: the system's, the app's (Geist,
//! Instrument Serif), and those the service loaded: its plugin's files, or
//! bytes it fetched (`{ base64 = … }`).
//! A family it doesn't have falls back as in a window.
//!
//! Each VM has its own, made on its first use, so a font one plugin loads
//! reaches no other (a window's `ui.load_font` reaches every window, and
//! asks for the "fonts" grant).

use std::cell::OnceCell;
use std::path::Path;
use std::sync::Arc;

use mlua::Table;
use zgx::scene::Scene;
use zgx::text_layout::{FontFamily, FontFeatures, FontStyle, LetterSpacing, LineHeight};
use zgx_text::TextServices;
use zgx_text::text::FontData;

/// A canvas text's size where its options give none (zgx-script's).
const TEXT_SIZE: f32 = 12.;

/// What the shaper may keep of the words it shaped before it lets go of
/// them all: a window's renderer trims them as it draws, and here nothing
/// draws.
const MAX_SHAPED_BYTES: usize = 4 << 20;

/// The app's own fonts, which every window has beside the system's
/// (apps/zgx/src/main.rs `fonts`).
pub(super) const APP_FONTS: [&[u8]; 9] = [
    include_bytes!("../../../../../../apps/zgx/fonts/Geist.ttf"),
    include_bytes!("../../../../../../apps/zgx/fonts/Geist-Italic.ttf"),
    include_bytes!("../../../../../../apps/zgx/fonts/Geist-Medium.ttf"),
    include_bytes!("../../../../../../apps/zgx/fonts/Geist-SemiBold.ttf"),
    include_bytes!("../../../../../../apps/zgx/fonts/Geist-Bold.ttf"),
    include_bytes!("../../../../../../apps/zgx/fonts/GeistMono.ttf"),
    include_bytes!("../../../../../../apps/zgx/fonts/GeistMono-Medium.ttf"),
    include_bytes!("../../../../../../apps/zgx/fonts/InstrumentSerif-Regular.ttf"),
    include_bytes!("../../../../../../apps/zgx/fonts/InstrumentSerif-Italic.ttf"),
];

/// A VM's text: its fonts, and a scene to measure in, as a canvas does.
#[derive(Default)]
pub(super) struct Text {
    ready: OnceCell<(TextServices, Scene)>,
}

impl Text {
    fn ready(&self) -> &(TextServices, Scene) {
        self.ready.get_or_init(|| {
            let services = TextServices::new();
            for bytes in APP_FONTS {
                if let Ok(font) = FontData::new(Arc::<[u8]>::from(bytes)) {
                    font.install(&mut services.text_system().borrow_mut());
                }
            }
            let mut scene = Scene::new(0., 0.);
            services.install(&mut scene);
            (services, scene)
        })
    }

    /// `text`'s width and height, wrapped at `width` if given.
    pub(super) fn measure(&self, text: &str, size: f32, width: Option<f32>, font: &FontStyle) -> (f32, f32) {
        let (services, scene) = self.ready();
        let measured = scene.measure_text_with_font(text, size, width, font);
        if services.text_system().borrow().shape_run_cache.bytes() > MAX_SHAPED_BYTES {
            services.release_unused();
        }
        measured
    }

    /// The font in the plugin's file `rel`, beside the faces it has: the
    /// family names it holds.
    pub(super) fn load(&self, root: &Path, rel: &str) -> Result<Vec<String>, String> {
        let path = root.join(rel);
        // (A link inside the snapshot can't lead out of it.)
        let inside = Path::new(rel)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_) | std::path::Component::CurDir));
        let canonical = std::fs::canonicalize(&path).map_err(|_| {
            if inside { format!("the plugin has no {rel}") } else { format!("{rel} isn't in the plugin") }
        })?;
        let root = std::fs::canonicalize(root).map_err(|_| format!("{rel} isn't in the plugin"))?;
        if !canonical.starts_with(&root) {
            return Err(format!("{rel} isn't in the plugin"));
        }
        let bytes = std::fs::read(&canonical).map_err(|_| format!("{rel} can't be read"))?;
        self.install(bytes, rel)
    }

    /// The font in `bytes` (`what` names it in an error), beside the faces
    /// it has: the family names it holds.
    pub(super) fn install(&self, bytes: Vec<u8>, what: &str) -> Result<Vec<String>, String> {
        let font = FontData::new(bytes).map_err(|why| format!("{what}: {why}"))?;
        let (services, _) = self.ready();
        font.install(&mut services.text_system().borrow_mut());
        // Text shaped before in a family that fell back shapes in it now.
        services.release_unused();
        Ok(font.families().to_vec())
    }
}

/// The largest font `zeron.load_font` takes from bytes, as a window's
/// `ui.load_font` (zgx-script's `FONT_LIMIT`).
const FONT_LIMIT: usize = 64 << 20;

/// A font's bytes from base64 (`zeron.fetch`'s `binary` body), at most
/// `FONT_LIMIT` of them.
pub(super) fn decode(encoded: &[u8]) -> Result<Vec<u8>, String> {
    use base64::Engine as _;
    if encoded.len() / 4 * 3 > FONT_LIMIT {
        return Err("a font is 64 MiB at most".into());
    }
    base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|why| format!("`base64` isn't base64 ({why})"))
}

/// A canvas text's size and font from its options, as zgx-script reads them
/// (`font`, `size`, `weight`, `italic`, `tabular`), with `line_height` and
/// `letter_spacing` in points as `ui.text` takes them, and the `width` it
/// wraps at.
pub(super) fn style(options: Option<&Table>) -> mlua::Result<(f32, Option<f32>, FontStyle)> {
    let mut font = FontStyle::default();
    let Some(o) = options else {
        return Ok((TEXT_SIZE, None, font));
    };
    if let Some(weight) = o.get::<Option<u16>>("weight")? {
        font.weight = weight;
    }
    font.italic = o.get::<Option<bool>>("italic")?.unwrap_or(false);
    if o.get::<Option<bool>>("tabular")?.unwrap_or(false) {
        font.features = FontFeatures::new([(*b"tnum", 1)]);
    }
    match o.get::<Option<String>>("font")?.as_deref() {
        None => {}
        Some("mono" | "monospace") => font.family = FontFamily::Monospace,
        Some("sans" | "sans-serif") => font.family = FontFamily::SansSerif,
        Some("serif") => font.family = FontFamily::Serif,
        Some(name) => font.family = FontFamily::Named(name.into()),
    }
    if let Some(line_height) = o.get::<Option<f32>>("line_height")? {
        font.line_height = LineHeight::px(line_height);
    }
    if let Some(spacing) = o.get::<Option<f32>>("letter_spacing")? {
        font.letter_spacing = LetterSpacing::px(spacing);
    }
    let width = o.get::<Option<f32>>("width")?;
    if let Some(width) = width
        && (!width.is_finite() || width <= 0.)
    {
        return Err(mlua::Error::runtime(format!(
            "zeron.measure_text: `width` is a number of points above 0, not {width}"
        )));
    }
    let size = o.get::<Option<f32>>("size")?.unwrap_or(TEXT_SIZE);
    Ok((size.max(1.), width, font))
}
