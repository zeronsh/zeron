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

/// The most style hints `plugin_status` gives.
pub const MAX_HINTS: usize = 20;

/// What a plugin's Luau uses that's off Zeron's design system, as hints for
/// an agent (`plugin_status { page = "settings" }`), never a block: a raw
/// colour, text size, corner or spacing where a token goes, and a settings
/// page not built from `@ds/settings`. Read from its files as written (its
/// own, not the SDK's copy), comments left out; each `{ file, line, text,
/// hint }`, [`MAX_HINTS`] at most, and how many more there were.
pub fn style_hints(dir: &Path) -> (Vec<Json>, usize) {
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else if name.ends_with(".luau") && !(name == "zeron.luau" && d == dir) {
                files.push(path);
            }
        }
    }
    files.sort();
    let (mut hints, mut more) = (Vec::new(), 0);
    let mut draws_page = false;
    let mut uses_settings = false;
    for path in &files {
        let Ok(text) = std::fs::read_to_string(path) else { continue };
        let rel = path.strip_prefix(dir).unwrap_or(path).display().to_string();
        draws_page |= text.contains("settings_page");
        uses_settings |= text.contains("@ds/settings") || text.contains("ds.Settings");
        for (at, line) in text.lines().enumerate() {
            let code = line.split("--").next().unwrap_or("");
            if let Some(hint) = style_hint(code) {
                if hints.len() < MAX_HINTS {
                    hints.push(json!({ "file": rel, "line": at + 1, "text": line.trim(), "hint": hint }));
                } else {
                    more += 1;
                }
            }
        }
    }
    if draws_page && !uses_settings {
        hints.insert(0, json!({ "hint": "Its settings page doesn't use @ds/settings: build it from Settings.Section, Settings.Group and Settings.Row, as Zeron's pages are." }));
    }
    (hints, more)
}

/// What one line of Luau sets off the system, if anything.
fn style_hint(code: &str) -> Option<&'static str> {
    // `name = <value>`: each assignment on the line, by its name.
    let mut rest = code;
    while let Some(eq) = rest.find('=') {
        let (before, after) = (&rest[..eq], &rest[eq + 1..]);
        rest = after;
        // (Not `==`, `~=`, `<=`, `>=`.)
        if after.starts_with('=') || before.ends_with(['=', '~', '<', '>']) {
            continue;
        }
        let name = before.trim_end().rsplit(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).next().unwrap_or("");
        let value = after.trim_start();
        let raw_colour = value.starts_with("\"#") || value.starts_with("'#") || value.starts_with("\"rgb") || value.starts_with("'rgb");
        let number = value.chars().next().is_some_and(|c| c.is_ascii_digit()) && !value.starts_with('0');
        match name {
            "color" | "bg" | "border_color" | "outline_color" | "fill" | "stroke" | "tint" | "caret_color" | "selection_color" if raw_colour => {
                return Some("A raw colour: use a token ($text.primary, $text.secondary, $text.tertiary, $accent, $border, $zeron.settings.card, $zeron.tone.<tone>.ink), so it follows the theme.");
            }
            "text_size" if number => return Some("A raw text size: use $font.size.sm (a row's description), $font.size.md (a row's name) or another $font.size token."),
            "line_height" if number => return Some("A raw line height: use the $font.line token of its size ($font.line.sm, $font.line.md)."),
            "rounded" if number => return Some("A raw corner: use $radius.sm…$radius.xl; inside something rounded, its corner less the inset."),
            "p" | "px" | "py" | "pt" | "pb" | "pl" | "pr" | "gap" | "m" | "mx" | "my" | "mt" | "mb" | "ml" | "mr" if number => {
                return Some("Raw spacing: use $space.1…$space.8 (a row's own is $space.4 across, $space.3 down).");
            }
            _ => {}
        }
    }
    None
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
/// { { x, y, w, h } } } } }`), those with a size: its own account, shown to
/// the agent as it said it and never what a picture is cut to (`measured`).
#[cfg(test)]
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

/// The boxes the host measured of a plugin's own nodes
/// (`ScriptHost::contribution_bounds`), as regions: what a picture of it is
/// cut around, never the boxes the plugin reports of itself.
pub fn measured(bounds: &[zgx::scene::Rect]) -> Vec<Region> {
    bounds.iter().filter(|b| b.width > 0. && b.height > 0.).map(|b| Region { x: b.x, y: b.y, w: b.width, h: b.height }).collect()
}

/// A cut's pixels (`width` × `height`, of `cut` at `scale` pixels a point)
/// with every `private` box in it made blank: what the app marks private
/// (a deploy token shown once) never leaves in a picture.
pub fn blank(rgba: &mut [u8], width: u32, height: u32, cut: Region, private: &[Region], scale: f32) {
    for p in private {
        let x0 = ((p.x - cut.x) * scale).floor().max(0.) as u32;
        let y0 = ((p.y - cut.y) * scale).floor().max(0.) as u32;
        let x1 = (((p.x + p.w - cut.x) * scale).ceil().max(0.) as u32).min(width);
        let y1 = (((p.y + p.h - cut.y) * scale).ceil().max(0.) as u32).min(height);
        for row in y0..y1 {
            let start = ((row * width + x0) * 4) as usize;
            let end = ((row * width + x1.max(x0)) * 4) as usize;
            if let Some(line) = rgba.get_mut(start..end) {
                line.fill(0);
            }
        }
    }
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
    fn what_a_plugin_draws_off_the_system_is_said_as_hints() {
        let dir = std::env::temp_dir().join(format!("zgx-style-hints-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("parts")).unwrap();
        std::fs::write(dir.join("zeron.luau"), "local x = ui.div { bg = \"#ff0000\" }\n").unwrap();
        std::fs::write(
            dir.join("init.luau"),
            "zeron.settings_page { build = function()\n\treturn ui.column {\n\t\tbg = \"#1f1f1f\", -- a card\n\t\tp = 12,\n\t\tgap = \"$space.2\",\n\t\trounded = 0,\n\t\tui.text { \"Hi\", text_size = 13, color = \"$text.secondary\" },\n\t\t-- color = \"#000\" in a comment\n\t\tif n == 3 then nil else nil,\n\t}\nend }\n",
        )
        .unwrap();
        std::fs::write(dir.join("parts/row.luau"), "return ui.div { border_color = 'rgb(1,2,3)', w = 220 }\n").unwrap();
        let (hints, more) = style_hints(&dir);
        let at: Vec<(String, u64)> = hints.iter().filter(|h| h.get("line").is_some()).map(|h| (h["file"].as_str().unwrap().to_string(), h["line"].as_u64().unwrap())).collect();
        assert_eq!(at, [("init.luau".into(), 3), ("init.luau".into(), 4), ("init.luau".into(), 7), ("parts/row.luau".into(), 1)], "{hints:#?}");
        assert!(hints[0]["hint"].as_str().unwrap().contains("@ds/settings"), "a page not built from Zeron's settings parts");
        assert!(hints[1]["hint"].as_str().unwrap().starts_with("A raw colour"));
        assert_eq!(more, 0);
        // Built from them: no such hint.
        std::fs::write(dir.join("parts/page.luau"), "local Settings = require(\"@ds/settings\")\n").unwrap();
        assert!(style_hints(&dir).0.iter().all(|h| h.get("line").is_some()));
        // At most 20, the rest counted.
        std::fs::write(dir.join("parts/many.luau"), "ui.div { p = 3 }\n".repeat(30)).unwrap();
        let (hints, more) = style_hints(&dir);
        assert_eq!((hints.len(), more), (MAX_HINTS, 34 - MAX_HINTS));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_page_is_an_id_and_a_title() {
        assert_eq!(page(&json!({ "id": "prs", "title": "Pull requests" })).unwrap(), json!({ "id": "prs", "title": "Pull requests" }));
        assert!(page(&json!({ "title": "x" })).is_err());
    }

    /// A plugin's picture is cut around its own nodes as the host measures
    /// them: one reporting the whole window as its own gets its own nodes
    /// (and the margin round them), and what the app marks private in the
    /// cut is blank.
    #[test]
    fn a_plugin_reporting_the_whole_window_gets_only_its_own_nodes() {
        // What it says of itself: the whole window. Never what's cut.
        let reported = regions(&json!([{ "at": "#titlebar-actions", "regions": [{ "x": 0, "y": 0, "w": 100000, "h": 100000 }] }]));
        assert_eq!(reported.len(), 1);
        // What the host measured: its one node in the title bar.
        let own = measured(&[zgx::scene::Rect { x: 900., y: 8., width: 24., height: 24. }, zgx::scene::Rect { x: 0., y: 0., width: 0., height: 10. }]);
        assert_eq!(own, vec![Region { x: 900., y: 8., w: 24., h: 24. }], "a box with no size isn't one");
        let cut = around(&own, 24., 1200., 800.).unwrap();
        assert_eq!(cut, Region { x: 876., y: 0., w: 72., h: 56. });
        // A private box (a token's) half in the cut: blank there, the rest kept.
        let (w, h) = (72u32, 56u32);
        let mut px = vec![255u8; (w * h * 4) as usize];
        blank(&mut px, w, h, cut, &[Region { x: 940., y: 40., w: 200., h: 200. }], 1.);
        let at = |x: u32, y: u32| px[((y * w + x) * 4) as usize];
        assert_eq!(at(64, 40), 0, "inside the private box");
        assert_eq!(at(71, 55), 0);
        assert_eq!(at(63, 40), 255, "beside it");
        assert_eq!(at(64, 39), 255, "above it");
        // One outside the cut leaves it whole.
        let mut whole = vec![255u8; (w * h * 4) as usize];
        blank(&mut whole, w, h, cut, &[Region { x: 0., y: 400., w: 50., h: 50. }], 1.);
        assert!(whole.iter().all(|&b| b == 255));
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
