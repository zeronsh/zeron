//! The fonts installed on this computer, for Settings → Appearance's font
//! pickers (the shipping app's `crates/ui/src/typography.rs`,
//! `register_fonts`): every family with a Latin `m` (script, symbol and
//! emoji families can't set an interface's words), and whether every face
//! of it is fixed width, which the code font's picker lists alone. Read on a
//! thread of its own the first time it's asked for, then kept.
//!
//! ```text
//! call  fonts.list   (none)   { { family, mono } }, by name; Geist and
//!                             Geist Mono (the app's own) left out
//! ```
//!
//! A family chosen is a theme token's name (`font.sans`, `font.mono`):
//! zgx's text system loads the same system fonts and finds it by name.

use std::collections::BTreeMap;
use std::sync::OnceLock;
use zgx_script::{ScriptHost, Value};

/// The families found, by name, each with whether it is fixed width.
static FAMILIES: OnceLock<Vec<(String, bool)>> = OnceLock::new();

/// The app's own families (`main.rs` `fonts()`), the pickers' defaults.
const BUNDLED: [&str; 2] = ["Geist", "Geist Mono"];

pub fn install(host: &ScriptHost) {
    host.async_action("fonts.list", |_| async {
        let families = match FAMILIES.get() {
            Some(families) => families,
            None => {
                let found = off_thread(installed).await.ok_or("failed")?;
                FAMILIES.get_or_init(|| found)
            }
        };
        Ok(Value::List(
            families
                .iter()
                .map(|(family, mono)| {
                    Value::record([
                        ("family", Value::from(family.as_str())),
                        ("mono", Value::Bool(*mono)),
                    ])
                })
                .collect(),
        ))
    });
}

/// `work` on a thread of its own, its answer awaited here (None only if it
/// panicked), as settings.rs's.
async fn off_thread<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tx, mut rx) = zgx::task::latest();
    std::thread::spawn(move || {
        tx.send(work());
    });
    rx.recv().await
}

/// The system's families (fontdb reads the directories zgx's text system
/// does), sorted by name, but the app's own and hidden ones (a leading `.`,
/// macOS's system UI faces).
fn installed() -> Vec<(String, bool)> {
    let mut database = fontdb::Database::new();
    database.load_system_fonts();
    let mut families: BTreeMap<String, bool> = BTreeMap::new();
    for face in database.faces() {
        let Some(Some(fixed)) = database.with_face_data(face.id, |data, index| {
            ttf_parser::Face::parse(data, index)
                .ok()
                .filter(|font| font.glyph_index('m').is_some())
                .map(|font| is_fixed_width(&font))
        }) else {
            continue;
        };
        for (name, _) in &face.families {
            // Choosing a family chooses all its faces: one proportional
            // italic is enough to break a grid of cells.
            families
                .entry(name.clone())
                .and_modify(|all| *all &= fixed)
                .or_insert(fixed);
        }
    }
    listed(families)
}

/// The families a picker offers: none hidden, none of the app's own.
fn listed(families: BTreeMap<String, bool>) -> Vec<(String, bool)> {
    families
        .into_iter()
        .filter(|(name, _)| !name.is_empty() && !name.starts_with('.') && !BUNDLED.contains(&name.as_str()))
        .collect()
}

/// How far apart the sampled advances may be and still count as one width:
/// hinting leaves sub-unit differences in real monospaced faces, where a
/// proportional face's differ by far more.
const FIXED_WIDTH_TOLERANCE: f32 = 0.01;

/// Whether every Latin cell of `font` is the same width, measured (a font's
/// own "monospaced" flag is metadata it can get wrong), as the shipping
/// app's `face_is_fixed_width`.
fn is_fixed_width(font: &ttf_parser::Face) -> bool {
    let Some(advances) = ['i', 'm', 'W', '0']
        .iter()
        .map(|ch| font.glyph_index(*ch).and_then(|id| font.glyph_hor_advance(id)))
        .collect::<Option<Vec<_>>>()
    else {
        return false;
    };
    let min = advances.iter().copied().min().unwrap_or(0);
    let max = advances.iter().copied().max().unwrap_or(0);
    min > 0 && f32::from(max - min) <= FIXED_WIDTH_TOLERANCE * f32::from(min)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(bytes: &[u8]) -> ttf_parser::Face<'_> {
        ttf_parser::Face::parse(bytes, 0).unwrap()
    }

    #[test]
    fn geist_mono_is_fixed_width_and_geist_is_not() {
        assert!(is_fixed_width(&face(include_bytes!("../fonts/GeistMono.ttf"))));
        assert!(is_fixed_width(&face(include_bytes!("../fonts/GeistMono-Medium.ttf"))));
        assert!(!is_fixed_width(&face(include_bytes!("../fonts/Geist.ttf"))));
        assert!(!is_fixed_width(&face(include_bytes!("../fonts/InstrumentSerif-Regular.ttf"))));
    }

    #[test]
    fn the_app_s_own_and_hidden_families_are_left_out() {
        let families = BTreeMap::from([
            ("Geist".to_string(), false),
            ("Geist Mono".to_string(), true),
            (".SF NS".to_string(), false),
            ("Inter".to_string(), false),
            ("JetBrains Mono".to_string(), true),
        ]);
        assert_eq!(
            listed(families),
            vec![("Inter".to_string(), false), ("JetBrains Mono".to_string(), true)]
        );
    }
}
