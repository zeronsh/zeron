//! Agents from the ACP registry in their own colours: each agent's real
//! logo, found once, kept on disk, and drawn by ds.AgentMark in place of the
//! registry's one-ink icon (which is all the registry publishes). Where it
//! comes from, in order:
//!
//! 1. the agent's site: its largest square icon (the web app manifest's,
//!    `apple-touch-icon`, a sized `icon` link), else `/apple-touch-icon.png`,
//!    else `/favicon.ico`; at least 32 px, as `ui.image` can draw it
//!    (favicons.rs's check);
//! 2. else, for one whose only link is a GitHub repository (or whose site
//!    gave nothing usable), its owner's avatar, usually the org's logo;
//! 3. else none: the card keeps the registry's icon in plain ink.
//!
//! Fetched in the background, a few at a time, once per agent (a miss is
//! remembered for a week); nothing waits on it. What scripts see,
//! `agent_logos`: `{ [registry id] = { path, fill, vector, source } }`: the
//! file, whether the logo is its own tile (opaque to its corners: drawn to
//! fill the mark, clipped to its corner) or a shape on transparency (drawn
//! inset on the white tile), whether it's an SVG (a site with no raster
//! icon; a plain one only), and where it came from.

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    io::Read,
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, SystemTime},
};

use serde_json::Value as Json;
use zgx_script::{ScriptHost, Shared, Value};

const TIMEOUT: Duration = Duration::from_secs(8);
const MAX_PAGE: u64 = 768 * 1024;
const MAX_IMAGE: u64 = 1024 * 1024;
/// Below this the logo would blur on a 32 pt tile: the next source instead.
const MIN_SIDE: u32 = 32;
/// A miss is tried again after this long (a site that had no icon may add
/// one; one that was down comes back).
const MISS_AGAIN: Duration = Duration::from_secs(7 * 24 * 3600);
/// At once: polite to sites, and quick enough for the registry's tens.
const AT_ONCE: usize = 4;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Logo {
    pub path: PathBuf,
    /// An SVG (drawn as a vector, as authored), not a raster.
    pub vector: bool,
    /// Opaque to its corners: its own tile.
    pub fill: bool,
    pub source: &'static str,
}

struct Logos {
    shared: RefCell<Vec<Shared>>,
    known: RefCell<HashMap<String, Logo>>,
    /// Asked for this run (found, missed or under way): never asked twice.
    asked: RefCell<HashSet<String>>,
    queue: RefCell<Vec<Want>>,
    running: std::cell::Cell<usize>,
}

#[derive(Clone)]
struct Want {
    id: String,
    website: Option<String>,
    repository: Option<String>,
}

thread_local! {
    static LOGOS: Rc<Logos> = Rc::new(Logos {
        shared: RefCell::default(),
        known: RefCell::default(),
        asked: RefCell::default(),
        queue: RefCell::default(),
        running: std::cell::Cell::new(0),
    });
}

/// Where logos are kept: beside the engine's data (as the browser's icons
/// are); `$ZERON_ZGX_AGENT_LOGOS` in its place (a fixture with no engine).
fn dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("ZERON_ZGX_AGENT_LOGOS").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    crate::engine::data_dir().map(|d| d.join("agent-logos"))
}

pub fn install(host: &ScriptHost) {
    let logos = LOGOS.with(Rc::clone);
    // What's kept from before, at once.
    if let Some(dir) = dir() {
        for (id, logo) in kept(&dir) {
            logos.known.borrow_mut().insert(id, logo);
        }
    }
    logos.shared.borrow_mut().push(host.shared("agent_logos", logos.value()));
}

/// The agents a listing names (an id, its site, its repository): each one's
/// logo looked for, if it hasn't been.
pub(crate) fn want(agents: impl IntoIterator<Item = (String, Option<String>, Option<String>)>) {
    let logos = LOGOS.with(Rc::clone);
    let Some(dir) = dir() else { return };
    for (id, website, repository) in agents {
        if !valid_id(&id) || !logos.asked.borrow_mut().insert(id.clone()) {
            continue;
        }
        if logos.known.borrow().contains_key(&id) || missed_lately(&dir, &id) {
            continue;
        }
        logos.queue.borrow_mut().push(Want { id, website, repository });
    }
    pump(&logos);
}

fn pump(logos: &Rc<Logos>) {
    while logos.running.get() < AT_ONCE {
        let Some(want) = logos.queue.borrow_mut().pop() else { return };
        let Some(dir) = dir() else { return };
        logos.running.set(logos.running.get() + 1);
        let logos = logos.clone();
        crate::engine::get().spawn(async move {
            let id = want.id.clone();
            let found = crate::favicons::off_thread(move || find(&dir, &want)).await.flatten();
            logos.running.set(logos.running.get() - 1);
            if let Some(logo) = found {
                logos.known.borrow_mut().insert(id, logo);
                logos.publish();
            }
            pump(&logos);
        });
    }
}

impl Logos {
    fn value(&self) -> Value {
        let known = self.known.borrow();
        Value::record(known.iter().map(|(id, logo)| {
            (
                id.as_str(),
                Value::record([
                    ("path", Value::from(logo.path.to_string_lossy().as_ref())),
                    ("fill", Value::Bool(logo.fill)),
                    ("vector", Value::Bool(logo.vector)),
                    ("source", Value::from(logo.source)),
                ]),
            )
        }))
    }

    fn publish(&self) {
        let value = self.value();
        for shared in self.shared.borrow().iter() {
            shared.set(value.clone());
        }
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// The logos kept: `<id>.logo` (the image) beside `<id>.json` (how to draw it).
fn kept(dir: &Path) -> Vec<(String, Logo)> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let id = name.strip_suffix(".json")?.to_string();
            let meta: Json = serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok()?;
            let path = dir.join(format!("{id}.logo"));
            path.is_file().then(|| {
                let source = match meta["source"].as_str() {
                    Some("github") => "github",
                    _ => "site",
                };
                let vector = meta["vector"].as_bool().unwrap_or(false);
                (id, Logo { path, fill: meta["fill"].as_bool().unwrap_or(false), vector, source })
            })
        })
        .collect()
}

fn missed_lately(dir: &Path, id: &str) -> bool {
    std::fs::metadata(dir.join(format!("{id}.miss")))
        .and_then(|m| m.modified())
        .ok()
        .and_then(|at| SystemTime::now().duration_since(at).ok())
        .is_some_and(|age| age < MISS_AGAIN)
}

/// Looks for one agent's logo (blocking: off the main thread); keeps what it
/// finds, or remembers that it found nothing.
fn find(dir: &Path, want: &Want) -> Option<Logo> {
    std::fs::create_dir_all(dir).ok()?;
    let agent = ureq::AgentBuilder::new()
        .timeout(TIMEOUT)
        .user_agent("Mozilla/5.0 (Macintosh) Zeron (agent logos)")
        .build();
    // Its site; else a "repository" that is a site of its own (a vendor's
    // docs page, not a code host).
    let site = want.website.as_deref().filter(|w| w.starts_with("https://")).or_else(|| {
        want.repository
            .as_deref()
            .filter(|r| r.starts_with("https://") && github_owner(r).is_none() && !r.contains("gitlab.com/"))
    });
    let mut found: Option<(Vec<u8>, &'static str)> = None;
    if let Some(site) = site {
        found = site_icon(&agent, site).map(|bytes| (bytes, "site"));
    }
    // (An organization's avatar is its logo; a person's is their face: a
    // community wrapper's author isn't the agent's brand.)
    if found.is_none()
        && let Some(owner) = want.repository.as_deref().and_then(github_owner)
        && is_organization(&agent, &owner)
    {
        found = fetch_image(&agent, &format!("https://github.com/{owner}.png?size=160")).map(|bytes| (bytes, "github"));
    }
    let Some((bytes, source)) = found else {
        let _ = std::fs::write(dir.join(format!("{}.miss", want.id)), "");
        return None;
    };
    let fill = opaque_corners(&bytes);
    let vector = is_svg(&bytes);
    let path = dir.join(format!("{}.logo", want.id));
    let tmp = path.with_extension("part");
    std::fs::write(&tmp, &bytes).ok()?;
    std::fs::rename(&tmp, &path).ok()?;
    let meta = serde_json::json!({ "fill": fill, "vector": vector, "source": source });
    std::fs::write(dir.join(format!("{}.json", want.id)), meta.to_string()).ok()?;
    let _ = std::fs::remove_file(dir.join(format!("{}.miss", want.id)));
    Some(Logo { path, fill, vector, source })
}

/// Whether a GitHub account is an organization (GitHub's public API).
fn is_organization(agent: &ureq::Agent, owner: &str) -> bool {
    agent
        .get(&format!("https://api.github.com/users/{owner}"))
        .set("Accept", "application/vnd.github+json")
        .call()
        .ok()
        .and_then(|r| r.into_json::<Json>().ok())
        .is_some_and(|user| user["type"].as_str() == Some("Organization"))
}

/// `https://github.com/<owner>/<repo>` → `<owner>`.
fn github_owner(repository: &str) -> Option<String> {
    let rest = repository.strip_prefix("https://github.com/")?;
    let owner = rest.split('/').next()?;
    (!owner.is_empty() && owner.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')).then(|| owner.to_string())
}

/// A site's best icon: every candidate its page names (the manifest's
/// icons, `apple-touch-icon`, `icon` links), largest declared first, then
/// the conventional paths; the first that is a drawable image at least
/// [`MIN_SIDE`] square-ish wins.
fn site_icon(agent: &ureq::Agent, site: &str) -> Option<Vec<u8>> {
    let base = url::Url::parse(site).ok()?;
    let page = fetch_text(agent, site);
    let (page_url, html) = match page {
        Some((final_url, html)) => (url::Url::parse(&final_url).unwrap_or(base.clone()), html),
        None => (base.clone(), String::new()),
    };
    let mut candidates: Vec<(u32, String)> = Vec::new();
    for link in links(&html) {
        let rel = link.get("rel").map(|r| r.to_ascii_lowercase()).unwrap_or_default();
        let Some(href) = link.get("href") else { continue };
        let Ok(url) = page_url.join(href) else { continue };
        if rel.split_whitespace().any(|r| r == "manifest") {
            for (side, icon) in manifest_icons(agent, url.as_str()) {
                candidates.push((side, icon));
            }
            continue;
        }
        let touch = rel.contains("apple-touch-icon");
        let icon = rel.split_whitespace().any(|r| r == "icon") || rel.contains("shortcut");
        if !(touch || icon) {
            continue;
        }
        let side = link.get("sizes").and_then(|s| largest_size(s)).unwrap_or(if touch { 180 } else { 0 });
        candidates.push((side, url.to_string()));
    }
    candidates.sort_by(|a, b| b.0.cmp(&a.0));
    for path in ["/apple-touch-icon.png", "/favicon.ico"] {
        if let Ok(url) = page_url.join(path) {
            candidates.push((0, url.to_string()));
        }
    }
    // A raster first (an app icon, a PNG, an .ico): the renderer draws any
    // of those as the site does; a site's SVG icon only when there is no
    // raster (they lean on CSS, nested documents and clip paths, which don't
    // all draw).
    let mut tried = HashSet::new();
    let mut vector: Option<Vec<u8>> = None;
    for (_, url) in candidates {
        if !tried.insert(url.clone()) || !url.starts_with("https://") {
            continue;
        }
        if let Some(bytes) = fetch_image(agent, &url) {
            if !is_svg(&bytes) {
                return Some(bytes);
            }
            vector.get_or_insert(bytes);
        }
    }
    vector.filter(|svg| plain_svg(svg))
}

/// An SVG simple enough to draw as authored: no stylesheet, no nested
/// document, no scripts or outside references.
fn plain_svg(bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(bytes).to_ascii_lowercase();
    text.matches("<svg").count() == 1
        && !["<style", "<script", "<foreignobject", "<image", "href=\"http", "@media"].iter().any(|bad| text.contains(bad))
}

/// A web app manifest's icons, by their largest declared side (any purpose
/// but `monochrome`).
fn manifest_icons(agent: &ureq::Agent, url: &str) -> Vec<(u32, String)> {
    let Some((final_url, text)) = fetch_text(agent, url) else { return Vec::new() };
    let Ok(base) = url::Url::parse(&final_url) else { return Vec::new() };
    let Ok(manifest) = serde_json::from_str::<Json>(&text) else { return Vec::new() };
    manifest["icons"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|i| !i["purpose"].as_str().is_some_and(|p| p.contains("monochrome")))
        .filter_map(|i| {
            let src = base.join(i["src"].as_str()?).ok()?;
            let side = i["sizes"].as_str().and_then(largest_size).unwrap_or(0);
            Some((side, src.to_string()))
        })
        .collect()
}

/// `"16x16 32x32"` → 32 (`any`: a vector, as large as wanted).
fn largest_size(sizes: &str) -> Option<u32> {
    sizes
        .split_whitespace()
        .filter_map(|s| {
            if s.eq_ignore_ascii_case("any") {
                return Some(512);
            }
            let (w, h) = s.to_ascii_lowercase().split_once('x').map(|(w, h)| (w.to_owned(), h.to_owned()))?;
            Some(w.parse::<u32>().ok()?.min(h.parse::<u32>().ok()?))
        })
        .max()
}

/// The `<link …>` tags of a page, each its attributes (lowercase names).
fn links(html: &str) -> Vec<HashMap<String, String>> {
    let lower = html.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(start) = lower[at..].find("<link") {
        let start = at + start;
        let Some(end) = lower[start..].find('>') else { break };
        let tag = &html[start + 5..start + end];
        out.push(attributes(tag));
        at = start + end;
    }
    out
}

fn attributes(tag: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let bytes = tag.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b'/') {
            i += 1;
        }
        let name_start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'=' && bytes[i] != b'/' {
            i += 1;
        }
        let name = tag[name_start..i].to_ascii_lowercase();
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let mut value = String::new();
        if i < bytes.len() && bytes[i] == b'=' {
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < bytes.len() && (bytes[i] == b'"' || bytes[i] == b'\'') {
                let quote = bytes[i];
                i += 1;
                let start = i;
                while i < bytes.len() && bytes[i] != quote {
                    i += 1;
                }
                value = tag[start..i].to_string();
                i += 1;
            } else {
                let start = i;
                while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                value = tag[start..i].to_string();
            }
        }
        if !name.is_empty() {
            out.insert(name, value.replace("&amp;", "&"));
        } else {
            i += 1;
        }
    }
    out
}

/// A page's text and where it ended up (after redirects).
fn fetch_text(agent: &ureq::Agent, url: &str) -> Option<(String, String)> {
    let response = agent.get(url).call().ok()?;
    let final_url = response.get_url().to_string();
    let mut bytes = Vec::new();
    response.into_reader().take(MAX_PAGE).read_to_end(&mut bytes).ok()?;
    Some((final_url, String::from_utf8_lossy(&bytes).into_owned()))
}

/// An image at `url` that `ui.image` can draw, at least [`MIN_SIDE`] on its
/// short side (a vector always).
fn fetch_image(agent: &ureq::Agent, url: &str) -> Option<Vec<u8>> {
    let response = agent.get(url).call().ok()?;
    let mut bytes = Vec::new();
    response.into_reader().take(MAX_IMAGE + 1).read_to_end(&mut bytes).ok()?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_IMAGE || !crate::favicons::drawable(&bytes) {
        return None;
    }
    if is_svg(&bytes) {
        return Some(bytes);
    }
    let image = image::load_from_memory(&bytes).ok()?;
    let (w, h) = (image.width(), image.height());
    // (Square-ish: a wordmark banner isn't a mark.)
    let square = w.max(h) <= w.min(h) * 3 / 2;
    (w.min(h) >= MIN_SIDE && square).then_some(bytes)
}

fn is_svg(bytes: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]);
    let head = head.trim_start_matches('\u{feff}').trim_start();
    head.starts_with("<svg") || (head.starts_with("<?xml") && head.contains("<svg"))
}

/// Whether a raster logo is its own tile: opaque at all four corners (an
/// app icon, an avatar), so it fills the mark; a shape on transparency
/// (most favicons, any SVG) sits inset on the white tile instead.
fn opaque_corners(bytes: &[u8]) -> bool {
    if is_svg(bytes) {
        return false;
    }
    let Ok(image) = image::load_from_memory(bytes) else { return false };
    let image = image.to_rgba8();
    let (w, h) = (image.width(), image.height());
    if w == 0 || h == 0 {
        return false;
    }
    let inset = (w.min(h) / 16).max(1).min(w - 1).min(h - 1);
    [(inset, inset), (w - 1 - inset, inset), (inset, h - 1 - inset), (w - 1 - inset, h - 1 - inset)]
        .iter()
        .all(|&(x, y)| image.get_pixel(x, y).0[3] >= 240)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pages_icon_links_are_read_whole() {
        let html = r#"<head><link rel="icon" href="/favicon-32.png" sizes="32x32">
            <LINK REL='apple-touch-icon' HREF='/apple-touch-icon.png?v=2&amp;x=1'>
            <link rel="manifest" href="/site.webmanifest"/></head>"#;
        let links = links(html);
        assert_eq!(links.len(), 3);
        assert_eq!(links[0]["href"], "/favicon-32.png");
        assert_eq!(links[0]["sizes"], "32x32");
        assert_eq!(links[1]["rel"], "apple-touch-icon");
        assert_eq!(links[1]["href"], "/apple-touch-icon.png?v=2&x=1");
        assert_eq!(links[2]["rel"], "manifest");
        assert_eq!(largest_size("16x16 32x32 192x180"), Some(180));
        assert_eq!(largest_size("any"), Some(512));
        assert_eq!(largest_size("huge"), None);
    }

    /// Every agent of the live registry: which logo each finds (or none),
    /// where from, and how it's drawn. `cargo test --bin zeron-zgx
    /// live_registry_logos -- --ignored --nocapture`; ZERON_LOGOS_OUT keeps
    /// them there (else a scratch folder).
    #[test]
    #[ignore = "fetches every registry agent's site"]
    fn live_registry_logos() {
        let index: Json = ureq::get("https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json")
            .call()
            .unwrap()
            .into_json()
            .unwrap();
        let dir = std::env::var("ZERON_LOGOS_OUT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir().join(format!("agent-logos-{}", std::process::id())));
        std::fs::create_dir_all(&dir).unwrap();
        let wants: Vec<Want> = index["agents"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| Want {
                id: a["id"].as_str().unwrap().to_string(),
                website: a["website"].as_str().map(str::to_owned),
                repository: a["repository"].as_str().map(str::to_owned),
            })
            .collect();
        let found: Vec<(String, Option<Logo>, Option<String>, Option<String>)> = std::thread::scope(|scope| {
            let handles: Vec<_> = wants
                .iter()
                .map(|w| {
                    let dir = dir.clone();
                    scope.spawn(move || (w.id.clone(), find(&dir, w), w.website.clone(), w.repository.clone()))
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        for (id, logo, site, repo) in &found {
            match logo {
                Some(l) => eprintln!("{id:22} {:6} {:6} {}", l.source, if l.fill { "fill" } else { "inset" }, l.path.display()),
                None => eprintln!("{id:22} NONE   site={site:?} repo={repo:?}"),
            }
        }
        eprintln!("dir {}", dir.display());
    }

    #[test]
    fn a_repositorys_owner_is_its_avatar() {
        assert_eq!(github_owner("https://github.com/block/goose").as_deref(), Some("block"));
        assert_eq!(github_owner("https://gitlab.com/x/y"), None);
        assert_eq!(github_owner("https://github.com/../etc"), None);
    }

    #[test]
    fn a_logo_opaque_to_its_corners_fills_the_mark() {
        let mut tile = image::RgbaImage::from_pixel(64, 64, image::Rgba([224, 116, 52, 255]));
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(tile.clone())
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        assert!(opaque_corners(&png));
        for (x, y) in [(0, 0), (63, 63), (2, 2), (61, 61), (61, 2), (2, 61)] {
            tile.put_pixel(x, y, image::Rgba([0, 0, 0, 0]));
        }
        let mut shape = Vec::new();
        image::DynamicImage::ImageRgba8(tile)
            .write_to(&mut std::io::Cursor::new(&mut shape), image::ImageFormat::Png)
            .unwrap();
        // (Its corners are clear: a shape, inset on the tile.)
        let cleared = image::RgbaImage::from_fn(64, 64, |x, y| {
            if x < 8 && y < 8 { image::Rgba([0, 0, 0, 0]) } else { image::Rgba([10, 10, 10, 255]) }
        });
        let mut corner = Vec::new();
        image::DynamicImage::ImageRgba8(cleared)
            .write_to(&mut std::io::Cursor::new(&mut corner), image::ImageFormat::Png)
            .unwrap();
        assert!(!opaque_corners(&corner));
        assert!(!opaque_corners(b"<svg xmlns='http://www.w3.org/2000/svg'/>"));
    }
}
