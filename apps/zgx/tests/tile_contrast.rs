//! A letter tile's initial and a risky ask read as WCAG asks (Wing's
//! decision, Nov 18), and no later change undoes it: every tile's fill
//! (`zeron.tile` in ui/ds/tokens.luau) reads 3:1 or more under its 50, as
//! large text needs, a hue whose 500 already does keeping its 500; Zeron
//! light's risky ask (`asks.risky`) reads 4.5:1 or more on its window.

use std::collections::HashMap;
use std::path::PathBuf;

fn read(path: &str) -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn luminance(hex: &str) -> f64 {
    let h = hex.trim_start_matches('#');
    let c = |i: usize| {
        let v = u8::from_str_radix(&h[i..i + 2], 16).unwrap() as f64 / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * c(0) + 0.7152 * c(2) + 0.0722 * c(4)
}

fn contrast(a: &str, b: &str) -> f64 {
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// `palette[hue][step]`, from ui/ds/palette.luau.
fn palette() -> HashMap<String, HashMap<String, String>> {
    let mut out = HashMap::new();
    for line in read("ui/ds/palette.luau").lines() {
        let line = line.trim();
        let Some((hue, rest)) = line.split_once(" = {") else { continue };
        let mut steps = HashMap::new();
        for part in rest.split(", ") {
            let part = part.trim().trim_end_matches("},").trim_end_matches('}').trim();
            if let Some((k, v)) = part.split_once("] = ") {
                steps.insert(k.trim_matches(|c| c == '[' || c == '"').to_string(), v.trim_matches('"').to_string());
            }
        }
        out.insert(hue.to_string(), steps);
    }
    out
}

/// The `tile = { … }` block of a tokens file: hue to its value as written.
fn tiles(path: &str) -> Vec<(String, String)> {
    let src = read(path);
    let start = src.find("\t\t\ttile = {\n").unwrap_or_else(|| panic!("{path}: no tile block"));
    let body = &src[start..];
    let end = body.find("\n\t\t\t},").unwrap();
    body[..end]
        .lines()
        .skip(1)
        .filter_map(|l| {
            let (k, v) = l.trim().trim_end_matches(',').split_once(" = ")?;
            Some((k.to_string(), v.trim_matches('"').to_string()))
        })
        .collect()
}

#[test]
fn every_tile_reads_three_to_one_a_hue_already_there_unchanged() {
    let pal = palette();
    let tiles = tiles("ui/ds/tokens.luau");
    assert_eq!(tiles.len(), pal.len(), "a fill for every hue");
    for (hue, value) in &tiles {
        let steps = &pal[hue];
        let (p500, p50) = (&steps["500"], &steps["50"]);
        let own = format!("{{palette.{hue}.500}}");
        let fill = if *value == own { p500.clone() } else { value.clone() };
        let ratio = contrast(&fill, p50);
        assert!(ratio >= 3.0, "{hue}: {fill} reads {ratio:.2} under its 50, under 3:1");
        if contrast(p500, p50) >= 3.0 {
            assert_eq!(value, &own, "{hue}: its 500 reads {:.2}, so it stays", contrast(p500, p50));
        } else {
            // Moved no further than it takes: lighter than its 600 where
            // that reads 3.1 already (yellow's reads 3.05).
            let p600 = &steps["600"];
            assert!(luminance(&fill) < luminance(p500), "{hue}: {fill}");
            assert!(contrast(p600, p50) < 3.1 || luminance(&fill) > luminance(p600), "{hue}: {fill}");
        }
    }
}

#[test]
fn zeron_v2_carries_the_same_tiles() {
    assert_eq!(tiles("ui/ds/tokens.luau"), tiles("ui/templates/zeron-v2/ds/tokens.luau"));
}

#[test]
fn a_risky_ask_reads_four_point_five_to_one_on_the_light_window() {
    let theme = read("ui/ds/themes/zeron-light.json");
    let at = theme.find("\"asks\": { \"risky\": \"").expect("light's risky ask") + "\"asks\": { \"risky\": \"".len();
    let ink = &theme[at..at + 7];
    // (The frosted window, the darker of its two grounds; the opaque
    // #f6f6f7 reads more.)
    let frosted = contrast(ink, "#eeeef0");
    assert!(frosted >= 4.5, "{ink} reads {frosted:.2} on #eeeef0, under 4.5:1");
    assert!(contrast(ink, "#f6f6f7") >= frosted);
}
