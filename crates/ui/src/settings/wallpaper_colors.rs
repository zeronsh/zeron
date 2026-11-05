//! Wallpaper-derived accents and subtle surface tints. User theme choices stay intact.
use gpui::{App, Global};
use zeron_theme::{AccentRoles, Color, ThemeVariant};

/// Quantized dominant colour, favouring chromatic regions over neutral pixels.
/// Sampling is bounded by the caller; transparent pixels do not influence it.
pub(crate) fn extract(pixels: impl IntoIterator<Item = [u8; 4]>) -> Option<Color> {
    let mut bins = vec![(0.0_f64, [0.0_f64; 3]); 4096];
    for [r, g, b, a] in pixels {
        if a < 128 {
            continue;
        }
        let high = r.max(g).max(b) as f64;
        let low = r.min(g).min(b) as f64;
        let saturation = (high - low) / high.max(1.0);
        let weight = (0.2 + saturation * saturation) * a as f64 / 255.0;
        let index = ((r as usize >> 4) << 8) | ((g as usize >> 4) << 4) | (b as usize >> 4);
        let (count, channels) = &mut bins[index];
        *count += weight;
        for (sum, channel) in channels.iter_mut().zip([r, g, b]) {
            *sum += channel as f64 * weight;
        }
    }
    let (count, channels) = bins.into_iter().max_by(|a, b| a.0.total_cmp(&b.0))?;
    (count > 0.0).then(|| {
        Color::rgb(
            (channels[0] / count).round() as u8,
            (channels[1] / count).round() as u8,
            (channels[2] / count).round() as u8,
        )
    })
}

pub(crate) fn active(cx: &App) -> Option<Color> {
    let settings = super::current(cx);
    if settings.wallpaper_theme_colors && settings.new_thread_composer_background.is_some() {
        settings.wallpaper_color
    } else {
        None
    }
}

pub(crate) fn tint_variant(variant: &mut ThemeVariant, color: Color) {
    let neutral = if variant.appearance.is_dark() {
        Color::BLACK
    } else {
        Color::WHITE
    };
    let tint = color.mix(
        neutral,
        if variant.appearance.is_dark() {
            0.88
        } else {
            0.94
        },
    );
    let colors = &mut variant.colors;
    for surface in [
        &mut colors.background,
        &mut colors.shell,
        &mut colors.raised,
        &mut colors.card,
        &mut colors.dialog,
        &mut colors.overlay,
        &mut colors.input,
    ] {
        *surface = surface.mix(tint, 0.4);
    }
    // Interactive roles use the existing contrast-aware accent derivation.
    variant.accent = AccentRoles::derive(color, variant.appearance, colors.background);
    colors.hover = variant.accent.primary.with_alpha(0.09);
    colors.active = variant.accent.primary.with_alpha(0.15);
    colors.border = variant.accent.primary.with_alpha(0.14);
    colors.border_strong = variant.accent.primary.with_alpha(0.3);
    variant.terminal.background = variant.terminal.background.mix(tint, 0.4);
    variant.terminal.selection = variant.accent.selection;
    // Theme::from_variant hardens text against every surface after this overlay.
}

pub fn set_enabled(enabled: bool, cx: &mut App) {
    super::update(super::SavePolicy::Immediate, cx, |settings| {
        settings.wallpaper_theme_colors = enabled
    });
    crate::appearance::apply(cx);
    ensure_color(cx);
    cx.refresh_windows();
}

#[derive(Default)]
struct PendingColor(Option<String>);
impl Global for PendingColor {}

/// Backfill an existing wallpaper saved before colour extraction was introduced.
pub(crate) fn ensure_color(cx: &mut App) {
    let settings = super::current(cx);
    if !settings.wallpaper_theme_colors || settings.wallpaper_color.is_some() {
        return;
    }
    let Some(background) = settings.new_thread_composer_background else {
        return;
    };
    if cx
        .try_global::<PendingColor>()
        .is_some_and(|pending| pending.0.as_ref() == Some(&background.path))
    {
        return;
    }
    cx.set_global(PendingColor(Some(background.path.clone())));
    cx.spawn(async move |cx| {
        let path = background.path.clone();
        let color = cx
            .background_executor()
            .spawn(async move {
                let bytes = std::fs::read(path).ok()?;
                let image = crate::new_thread_background_image::decode(&bytes)
                    .ok()?
                    .thumbnail(64, 64)
                    .to_rgba8();
                extract(image.pixels().map(|pixel| pixel.0))
            })
            .await;
        cx.update(|cx| {
            if super::current(cx)
                .new_thread_composer_background
                .as_ref()
                .map(|bg| &bg.path)
                != Some(&background.path)
            {
                return;
            }
            if let Some(color) = color {
                super::update(super::SavePolicy::Immediate, cx, |settings| {
                    settings.wallpaper_color = Some(color)
                });
                crate::appearance::apply(cx);
            }
        });
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extraction_ignores_transparency_and_favours_prominent_colour() {
        let mut pixels = vec![[90, 90, 90, 255]; 100];
        pixels.extend(vec![[30, 130, 220, 255]; 80]);
        pixels.extend(vec![[255, 0, 0, 0]; 1000]);
        assert_eq!(extract(pixels), Some(Color::rgb(30, 130, 220)));
        assert_eq!(
            extract([[100, 100, 100, 255]; 10]),
            Some(Color::rgb(100, 100, 100))
        );
        assert_eq!(extract([[255, 0, 0, 0]; 10]), None);
    }

    #[test]
    fn overlays_preserve_semantic_colours_and_accessible_accents() {
        let registry = zeron_theme::ThemeRegistry::builtin();
        for original in registry.families.iter().flat_map(|family| &family.variants) {
            for color in [
                Color::BLACK,
                Color::WHITE,
                Color::rgb(250, 220, 30),
                Color::rgb(20, 70, 200),
            ] {
                let mut variant = original.clone();
                tint_variant(&mut variant, color);
                assert!(variant.accent.primary.contrast(variant.colors.background) >= 3.0);
                assert!(variant.accent.on.contrast(variant.accent.strong) >= 4.5);
                assert_eq!(variant.colors.danger, original.colors.danger);
                assert_eq!(variant.colors.success, original.colors.success);
                assert_eq!(variant.syntax, original.syntax);
            }
        }
    }

    #[gpui::test]
    fn wallpaper_palette_updates_and_disabling_restores_manual_choices(
        cx: &mut gpui::TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("blue.png");
        let second = dir.path().join("orange.png");
        image::RgbaImage::from_pixel(4, 4, image::Rgba([30, 130, 220, 255]))
            .save(&first)
            .unwrap();
        image::RgbaImage::from_pixel(4, 4, image::Rgba([230, 100, 25, 255]))
            .save(&second)
            .unwrap();
        cx.update(|cx| {
            gpui_base::init(cx);
            let settings = super::super::UiSettings {
                accent: zeron_theme::AccentSelection::Preset(zeron_theme::AccentPreset::Pink),
                appearance: crate::appearance::AppearanceMode::Dark,
                ..Default::default()
            };
            super::super::init(settings.clone(), dir.path().join("data"), cx);
            crate::appearance::init(
                settings.appearance,
                settings.theme_selection.clone(),
                settings.accent,
                settings.surface,
                cx,
            );
            let manual = crate::theme::Theme::of(cx).clone();
            super::super::install_new_thread_composer_background(&first, cx).unwrap();
            assert_eq!(crate::theme::Theme::of(cx).accent, manual.accent);
            set_enabled(true, cx);
            assert_eq!(active(cx), Some(Color::rgb(30, 130, 220)));
            assert_ne!(crate::theme::Theme::of(cx).accent, manual.accent);
            assert_ne!(crate::theme::Theme::of(cx).bg, manual.bg);
            assert_eq!(super::super::current(cx).accent, settings.accent);
            super::super::install_new_thread_composer_background(&second, cx).unwrap();
            assert_eq!(
                crate::theme::Theme::of(cx).wallpaper_color,
                Some(Color::rgb(230, 100, 25))
            );
            crate::appearance::set_mode(crate::appearance::AppearanceMode::Light, cx);
            assert_eq!(
                crate::theme::Theme::of(cx).wallpaper_color,
                Some(Color::rgb(230, 100, 25))
            );
            crate::appearance::set_mode(crate::appearance::AppearanceMode::Dark, cx);
            set_enabled(false, cx);
            assert_eq!(crate::theme::Theme::of(cx).accent, manual.accent);
            assert_eq!(crate::theme::Theme::of(cx).bg, manual.bg);
            set_enabled(true, cx);
            super::super::remove_new_thread_composer_background(cx).unwrap();
            assert_eq!(crate::theme::Theme::of(cx).accent, manual.accent);
            let saved = super::super::UiSettings::load(&dir.path().join("data"));
            assert!(saved.wallpaper_theme_colors);
            assert_eq!(saved.accent, settings.accent);
            assert_eq!(saved.theme_selection, settings.theme_selection);
        });
    }

    #[gpui::test]
    fn existing_wallpaper_gets_a_palette_when_option_is_enabled(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("existing.png");
        image::RgbaImage::from_pixel(4, 4, image::Rgba([30, 130, 220, 255]))
            .save(&image)
            .unwrap();
        cx.update(|cx| {
            gpui_base::init(cx);
            super::super::init(
                super::super::UiSettings {
                    new_thread_composer_background: Some(
                        super::super::NewThreadComposerBackground {
                            path: image.to_string_lossy().into_owned(),
                            name: "existing.png".into(),
                            adjustment: super::super::NewThreadBackgroundAdjustment::default(),
                        },
                    ),
                    ..Default::default()
                },
                dir.path(),
                cx,
            );
            set_enabled(true, cx);
        });
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(active(cx), Some(Color::rgb(30, 130, 220))));
    }
}
