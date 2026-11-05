//! What plugins add to Zeron beyond a node in place: tabs in a chat's panel
//! (`panel.tabs`) and pages of their own in the sidebar (`pages.add`), each
//! drawn by Zeron as an empty placeholder (`.plugin-tab.plugin-<id>`,
//! `.plugin-page.plugin-<id>`) that the plugin fills by selector; and what an
//! agent is told of where a plugin's nodes landed (`plugin_status`): the
//! places they matched, and a picture of the window cut to them.
//!
//! The checks here are the host's: a plugin's actions carry what it asked,
//! and nothing it sends is trusted further than these limits.

use std::path::Path;

use serde_json::{Value as Json, json};

/// A tab kind, page id or icon name: short, plain.
fn name(value: &Json, max: usize) -> Option<String> {
    let s = value.as_str()?.trim();
    let ok = !s.is_empty()
        && s.len() <= max
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    ok.then(|| s.to_string())
}

/// A title as a person reads it: one line, at most `max` characters.
fn title(value: &Json, max: usize) -> Option<String> {
    let s: String = value.as_str()?.lines().next()?.trim().chars().take(max).collect();
    (!s.is_empty()).then_some(s)
}

/// What a tab or page may carry of its own (`data`): JSON, at most this.
pub const DATA_BYTES: usize = 16 * 1024;

/// `open_tab { session | page, kind, title, icon?, data? }` as the host
/// takes it, or why not: in a session's panel, or a plugin's page's
/// (`page = { plugin, id }`, which the bridge holds to the plugin's own).
pub fn tab(asked: &Json) -> Result<Json, String> {
    let session = asked["session"].as_str().filter(|s| !s.is_empty());
    let page = match (name(&asked["page"]["plugin"], 40), name(&asked["page"]["id"], 40)) {
        (Some(plugin), Some(id)) => Some(json!({ "plugin": plugin, "id": id })),
        _ => None,
    };
    if session.is_none() && page.is_none() {
        return Err("open_tab needs the panel's session or page (zeron.need(\"panel\"))".into());
    }
    let kind = name(&asked["kind"], 40).ok_or("open_tab needs a `kind`: letters, digits, - and _")?;
    let title = title(&asked["title"], 80).ok_or("open_tab needs a `title`")?;
    let data = data(&asked["data"])?;
    let mut out = json!({ "kind": kind, "title": title, "data": data });
    if let Some(session) = session {
        out["session"] = json!(session);
    }
    if let Some(page) = page {
        out["page"] = page;
    }
    if let Some(icon) = name(&asked["icon"], 40) {
        out["icon"] = json!(icon);
    }
    Ok(out)
}

/// `add_page { id, title, icon? }` as the host takes it, or why not.
pub fn page(asked: &Json) -> Result<Json, String> {
    let id = name(&asked["id"], 40).ok_or("add_page needs an `id`: letters, digits, - and _")?;
    let title = title(&asked["title"], 60).ok_or("add_page needs a `title`")?;
    let mut out = json!({ "id": id, "title": title });
    if let Some(icon) = name(&asked["icon"], 40) {
        out["icon"] = json!(icon);
    }
    Ok(out)
}

fn data(value: &Json) -> Result<Json, String> {
    if value.is_null() {
        return Ok(Json::Null);
    }
    let size = serde_json::to_vec(value).map(|b| b.len()).unwrap_or(usize::MAX);
    if size > DATA_BYTES {
        return Err(format!("A tab's data is at most {} KB", DATA_BYTES / 1024));
    }
    Ok(value.clone())
}

/// A web link a plugin may open: `http:` or `https:`, with a host, no
/// spaces or control characters, at most 2,000 characters.
pub fn web_link(url: &str) -> bool {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"));
    url.len() <= 2000
        && rest.is_some_and(|r| !r.is_empty() && !r.starts_with('/'))
        && !url.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// What building a plugin's nodes may cost: each build of a node at a place
/// (a row's badge), and all of a plugin's builds in one frame.
pub const BUDGET_PER_BUILD_MS: f64 = 1.0;
pub const BUDGET_PER_FRAME_MS: f64 = 8.0;

fn ms(v: f64) -> String {
    if v < 10.0 { format!("{v:.1}") } else { format!("{v:.0}") }
}

/// What a plugin's costs (its SDK's report: `{ places = { { at, builds,
/// mean_ms, max_ms } }, frame_ms }`) say past the budget, in words: "Slow:
/// 4.2 ms per row", or nothing within it.
pub fn cost_words(cost: &Json) -> Option<String> {
    let worst = cost["places"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| p["mean_ms"].as_f64().unwrap_or(0.) > BUDGET_PER_BUILD_MS)
        .max_by(|a, b| a["mean_ms"].as_f64().unwrap_or(0.).total_cmp(&b["mean_ms"].as_f64().unwrap_or(0.)));
    if let Some(p) = worst {
        let each = if p["builds"].as_u64().unwrap_or(1) > 1 { "per row" } else { "to build" };
        return Some(format!("Slow: {} ms {each} (keep it under {} ms)", ms(p["mean_ms"].as_f64().unwrap_or(0.)), ms(BUDGET_PER_BUILD_MS)));
    }
    let frame = cost["frame_ms"].as_f64().unwrap_or(0.);
    (frame > BUDGET_PER_FRAME_MS).then(|| format!("Slow: {} ms in one frame (keep it under {} ms)", ms(frame), ms(BUDGET_PER_FRAME_MS)))
}

/// A box in the window, in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Region {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Each region a plugin's placements report (`{ placements = { { regions =
/// { { x, y, w, h } } } } }`), those with a size.
pub fn regions(placements: &Json) -> Vec<Region> {
    let mut out = Vec::new();
    for p in placements.as_array().into_iter().flatten() {
        for r in p["regions"].as_array().into_iter().flatten() {
            let f = |k: &str| r[k].as_f64().unwrap_or(0.) as f32;
            let region = Region { x: f("x"), y: f("y"), w: f("w"), h: f("h") };
            if region.w > 0. && region.h > 0. {
                out.push(region);
            }
        }
    }
    out
}

/// The box around all of `regions`, `pad` points wider each way, within a
/// window `width` × `height`; `None` with none.
pub fn around(regions: &[Region], pad: f32, width: f32, height: f32) -> Option<Region> {
    let first = regions.first()?;
    let (mut x0, mut y0, mut x1, mut y1) = (first.x, first.y, first.x + first.w, first.y + first.h);
    for r in &regions[1..] {
        x0 = x0.min(r.x);
        y0 = y0.min(r.y);
        x1 = x1.max(r.x + r.w);
        y1 = y1.max(r.y + r.h);
    }
    let (x0, y0) = ((x0 - pad).max(0.), (y0 - pad).max(0.));
    let (x1, y1) = ((x1 + pad).min(width), (y1 + pad).min(height));
    (x1 > x0 && y1 > y0).then_some(Region { x: x0, y: y0, w: x1 - x0, h: y1 - y0 })
}

/// `rgba` (`width` × `height` pixels) cut to `region`, in points at `scale`
/// pixels each: the cut's size and pixels.
pub fn crop(rgba: &[u8], width: u32, height: u32, region: Region, scale: f32) -> (u32, u32, Vec<u8>) {
    let px = |v: f32, max: u32| ((v * scale).round().max(0.) as u32).min(max);
    let (x0, y0) = (px(region.x, width), px(region.y, height));
    let (x1, y1) = (px(region.x + region.w, width), px(region.y + region.h, height));
    let (w, h) = (x1.saturating_sub(x0), y1.saturating_sub(y0));
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for row in y0..y1 {
        let start = ((row * width + x0) * 4) as usize;
        let end = start + (w * 4) as usize;
        if let Some(line) = rgba.get(start..end) {
            out.extend_from_slice(line);
        }
    }
    (w, h, out)
}

/// `rgba` as a PNG at `path`.
pub fn save_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .and_then(|mut w| w.write_image_data(rgba))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tab_is_taken_within_its_limits() {
        let t = tab(&json!({ "session": "s1", "kind": "pr", "title": "PR #12\nmore", "icon": "pull-request", "data": { "n": 12 } })).unwrap();
        assert_eq!(t, json!({ "session": "s1", "kind": "pr", "title": "PR #12", "icon": "pull-request", "data": { "n": 12 } }));
        assert!(tab(&json!({ "kind": "pr", "title": "x" })).unwrap_err().contains("session"));
        // Or a plugin's page's panel.
        let on_page = tab(&json!({ "page": { "plugin": "github", "id": "pulls" }, "kind": "pr", "title": "x" })).unwrap();
        assert_eq!((on_page["page"]["plugin"].as_str(), on_page["page"]["id"].as_str(), on_page.get("session")), (Some("github"), Some("pulls"), None));
        assert!(tab(&json!({ "page": { "plugin": "../x", "id": "p" }, "kind": "pr", "title": "x" })).is_err());
        assert!(tab(&json!({ "session": "s1", "kind": "p r", "title": "x" })).unwrap_err().contains("kind"));
        let big = "x".repeat(DATA_BYTES + 1);
        assert!(tab(&json!({ "session": "s1", "kind": "pr", "title": "x", "data": big })).unwrap_err().contains("KB"));
        let odd = tab(&json!({ "session": "s1", "kind": "pr", "title": "x", "icon": "../etc" })).unwrap();
        assert!(odd.get("icon").is_none(), "an icon that isn't a name is left out");
    }

    #[test]
    fn a_slow_plugin_is_said_so_in_words() {
        let fast = json!({ "places": [{ "at": ".session-title", "builds": 40, "mean_ms": 0.2, "max_ms": 0.6 }], "frame_ms": 3.0 });
        assert_eq!(cost_words(&fast), None);
        let rows = json!({ "places": [{ "at": ".session-title", "builds": 40, "mean_ms": 4.24, "max_ms": 9.0 }], "frame_ms": 30.0 });
        assert_eq!(cost_words(&rows).as_deref(), Some("Slow: 4.2 ms per row (keep it under 1.0 ms)"));
        let once = json!({ "places": [{ "at": "#titlebar-actions", "builds": 1, "mean_ms": 12.0, "max_ms": 12.0 }] });
        assert_eq!(cost_words(&once).as_deref(), Some("Slow: 12 ms to build (keep it under 1.0 ms)"));
        let crowded = json!({ "places": [{ "at": ".message", "builds": 200, "mean_ms": 0.5, "max_ms": 0.9 }], "frame_ms": 20.0 });
        assert_eq!(cost_words(&crowded).as_deref(), Some("Slow: 20 ms in one frame (keep it under 8.0 ms)"));
    }

    #[test]
    fn only_web_links_open() {
        assert!(web_link("https://github.com/zeronsh/zgx/pull/1264"));
        assert!(web_link("http://localhost:3000/"));
        for bad in ["file:///etc/passwd", "javascript:alert(1)", "zeron://settings", "https://", "https:///x", "https://a b", "ftp://x"] {
            assert!(!web_link(bad), "{bad}");
        }
    }

    #[test]
    fn a_page_is_an_id_and_a_title() {
        assert_eq!(page(&json!({ "id": "prs", "title": "Pull requests" })).unwrap(), json!({ "id": "prs", "title": "Pull requests" }));
        assert!(page(&json!({ "title": "x" })).is_err());
    }

    #[test]
    fn the_picture_is_cut_around_every_placement() {
        let placed = json!([
            { "at": ".session-row", "regions": [{ "x": 10, "y": 20, "w": 30, "h": 10 }, { "x": 10, "y": 40, "w": 30, "h": 10 }] },
            { "at": "#bar", "regions": [{ "x": 0, "y": 0, "w": 0, "h": 0 }] },
        ]);
        let rs = regions(&placed);
        assert_eq!(rs.len(), 2, "a box with no size isn't one");
        let r = around(&rs, 4., 100., 100.).unwrap();
        assert_eq!(r, Region { x: 6., y: 16., w: 38., h: 38. });
        assert_eq!(around(&[], 4., 100., 100.), None);
        // A 4×2-pixel image at 2 pixels a point, cut to its right half.
        let rgba: Vec<u8> = (0..32).collect();
        let (w, h, px) = crop(&rgba, 4, 2, Region { x: 1., y: 0., w: 1., h: 1. }, 2.);
        assert_eq!((w, h), (2, 2));
        assert_eq!(px, vec![8, 9, 10, 11, 12, 13, 14, 15, 24, 25, 26, 27, 28, 29, 30, 31]);
    }
}
