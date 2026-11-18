//! The plugin store, for Settings → Plugins (ui/screens/plugins/store.luau
//! names every call): Browse, a store plugin's page, and publishing from
//! Yours. The catalog is public: read from the edge as no one (no session
//! goes with it, so the edge's cache serves everyone the same), the
//! plan's contract (docs/plans/2026-11-17-plugin-store.md "Contract").
//! Your own plugins, their versions and your reports go with your session
//! (src/auth.rs `edge`: the bearer never reaches a script). What installs,
//! signs or makes a key is the engine's, on the device the call names.
//!
//!   plugins.store.search { q?, category?, keyword?, sort?, cursor?, limit?, featured? }
//!   plugins.store.detail(id)   plugins.store.versions(id)   plugins.store.categories
//!   plugins.store.picture { id, version, path }  { src }: a picture of its
//!       package, kept in `<data>/store-pictures/` (at most 5 MB each, 64 MB
//!       in all, the oldest let go), only a PNG, JPEG, GIF or WebP that is
//!       one; nothing else is ever fetched for a README
//!   plugins.store.icon { id, version, path }  { svg }: its SVG icon, at most
//!       32 KB, as an installed plugin's (screens/plugins/words `tile`)
//!   plugins.store.mine   plugins.store.withdraw|yank|unyank { id, version }
//!   plugins.store.keys(id)   plugins.store.revoke_key { id, key }
//!   plugins.store.report { id, reason, text }
//!   (the engine's) plugins.store.unblock { id, device? }   plugins.store.check(id)
//!   plugins.store.publish(id)   plugins.store.create_key { id, name, expires? }
//!
//! Each rejects with an outcome word ("offline", "signed_out", "not_found")
//! or the edge's `"<error>: <message>"`, which the page says in its words
//! where it knows them (screens/plugins/words `failure`).

use std::path::{Path, PathBuf};
use std::rc::Weak;

use serde_json::{Value as Json, json};
use sha2::{Digest, Sha256};
use zgx_script::{ScriptHost, Value};

use super::{Bridge, json_value};
use crate::auth::{self, text};

/// A picture of a package, at most.
const PICTURE_MOST: u64 = 5 * 1024 * 1024;
/// The pictures kept here, at most, in all.
const PICTURES_KEPT: u64 = 64 * 1024 * 1024;
/// An icon, at most (an installed plugin's too, mod.rs `icon_svg`).
const ICON_MOST: u64 = 32 * 1024;
/// A page of the catalog, at most (the store's own limit).
const PAGE_MOST: u64 = 50;

pub(super) fn install(host: &ScriptHost, me: Weak<Bridge>) {
    host.async_action("plugins.store.search", |arg| async move { search(&arg).await });
    host.async_action("plugins.store.detail", |id| async move {
        let id = store_id(&text(&id)).ok_or("not_found")?;
        public(format!("/store/v1/plugins/{id}")).await
    });
    host.async_action("plugins.store.versions", |id| async move {
        let id = store_id(&text(&id)).ok_or("not_found")?;
        public(format!("/store/v1/plugins/{id}/versions")).await
    });
    host.async_action("plugins.store.categories", |_| async move { public("/store/v1/categories".into()).await });
    host.async_action("plugins.store.picture", |arg| async move {
        let (id, version, path) = file_of(&arg).ok_or("not_a_picture")?;
        let kind = picture_kind(&path).ok_or("not_a_picture")?;
        picture(&id, &version, &path, kind).await
    });
    host.async_action("plugins.store.icon", |arg| async move {
        let (id, version, path) = file_of(&arg).ok_or("not_an_icon")?;
        if !path.ends_with(".svg") {
            return Err("not_an_icon".into());
        }
        icon(&id, &version, &path).await
    });
    host.async_action("plugins.store.mine", |_| async move { authed("GET", "/store/v1/me/plugins".into(), None).await });
    for step in ["withdraw", "yank", "unyank"] {
        host.async_action(&format!("plugins.store.{step}"), move |arg| async move {
            let (id, version) = id_version(&arg).ok_or("not_found")?;
            crate::plugins::preview::refuse("plugins.store.version")?;
            authed("POST", format!("/store/v1/plugins/{id}/versions/{}/{step}", auth::encode_path(&version)), None).await
        });
    }
    host.async_action("plugins.store.keys", |id| async move {
        let id = store_id(&text(&id)).ok_or("not_found")?;
        authed("GET", format!("/store/v1/plugins/{id}/keys"), None).await
    });
    host.async_action("plugins.store.revoke_key", |arg| async move {
        let id = arg.field("id").map(text).and_then(|i| store_id(&i)).ok_or("not_found")?;
        let key = arg.field("key").map(text).filter(|k| key_id(k)).ok_or("not_found")?;
        crate::plugins::preview::refuse("plugins.store.revoke_key")?;
        authed("DELETE", format!("/store/v1/plugins/{id}/keys/{}", auth::encode_path(&key)), None).await
    });
    host.async_action("plugins.store.report", |arg| async move {
        let id = arg.field("id").map(text).and_then(|i| store_id(&i)).ok_or("not_found")?;
        let reason = arg.field("reason").map(text).unwrap_or_default();
        if !REPORT_REASONS.contains(&reason.as_str()) {
            return Err("bad_reason".into());
        }
        let words: String = arg.field("text").map(text).unwrap_or_default().chars().take(2000).collect();
        authed("POST", format!("/store/v1/plugins/{id}/reports"), Some(json!({ "reason": reason, "text": words }))).await
    });
    // The engine's: what installs, signs, or makes a key (each on the
    // device named; publishing and keys are this device's own).
    host.async_action("plugins.store.unblock", {
        let me = me.clone();
        move |arg| {
            let me = me.clone();
            async move {
                crate::plugins::preview::refuse("plugins.store.unblock")?;
                let (id, device) = super::remote::id_and_device(&arg);
                let id = store_id(&id).ok_or("not_found")?;
                let reply = super::call_on(&me, device, "UnblockPlugin", json!({ "id": id })).await?;
                Ok(json_value(&reply))
            }
        }
    });
    host.async_action("plugins.store.check", |id| async move {
        let id = store_id(&text(&id)).ok_or("not_found")?;
        let reply = super::call("PublishCheck", json!({ "id": id })).await?;
        Ok(json_value(&reply))
    });
    host.async_action("plugins.store.publish", |id| async move {
        crate::plugins::preview::refuse("plugins.store.publish")?;
        let id = store_id(&text(&id)).ok_or("not_found")?;
        let reply = super::call("PublishPlugin", json!({ "id": id })).await?;
        Ok(json_value(&reply))
    });
    host.async_action("plugins.store.create_key", |arg| async move {
        crate::plugins::preview::refuse("plugins.store.create_key")?;
        let id = arg.field("id").map(text).and_then(|i| store_id(&i)).ok_or("not_found")?;
        let name: String = arg.field("name").map(text).unwrap_or_default().trim().chars().take(64).collect();
        if name.is_empty() {
            return Err("Give the key a name".into());
        }
        let mut params = json!({ "id": id, "name": name });
        if let Some(Value::Num(at)) = arg.field("expires")
            && at.is_finite()
            && *at > 0.0
        {
            params["expires"] = json!(*at as i64);
        }
        let reply = super::call("CreateDeployKey", params).await?;
        Ok(json_value(&reply))
    });
}

/// What a report may say is wrong (the edge's list).
const REPORT_REASONS: &[&str] = &["broken", "malware", "impersonation", "spam", "other"];

/// A store id, as a plugin's id is (crates/engine plugins/manifest.rs
/// `check_id`): `^[a-z0-9][a-z0-9-]{1,39}$`.
pub(crate) fn store_id(id: &str) -> Option<String> {
    let bytes = id.as_bytes();
    let ok = (2..=40).contains(&bytes.len())
        && matches!(bytes[0], b'a'..=b'z' | b'0'..=b'9')
        && bytes[1..].iter().all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'-'));
    ok.then(|| id.to_owned())
}

/// A version as the store names them: SemVer's characters, at most 64.
fn version_ok(v: &str) -> bool {
    !v.is_empty() && v.len() <= 64 && v.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+'))
}

/// A deploy key's id as the edge names it (base64url).
fn key_id(k: &str) -> bool {
    !k.is_empty() && k.len() <= 128 && k.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

fn id_version(arg: &Value) -> Option<(String, String)> {
    let id = store_id(&arg.field("id").map(text)?)?;
    let version = arg.field("version").map(text).filter(|v| version_ok(v))?;
    Some((id, version))
}

/// A file of a package, as a README or a listing names it: relative, inside
/// it, no `..`, no scheme, no backslash, at most 512 bytes; its parts as the
/// store's file route takes them (each escaped).
fn package_path(path: &str) -> Option<String> {
    if path.is_empty() || path.len() > 512 || path.starts_with('/') || path.contains('\\') || path.contains(':') || path.contains('\0') {
        return None;
    }
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => return None,
            part => parts.push(part),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

fn file_of(arg: &Value) -> Option<(String, String, String)> {
    let (id, version) = id_version(arg)?;
    let path = package_path(&arg.field("path").map(text)?)?;
    Some((id, version, path))
}

/// A picture's kind by its name: the extension it's kept under, and the
/// content types that may say it.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Picture {
    Png,
    Jpeg,
    Gif,
    Webp,
}

fn picture_kind(path: &str) -> Option<Picture> {
    let ext = path.rsplit_once('.')?.1.to_ascii_lowercase();
    match ext.as_str() {
        "png" => Some(Picture::Png),
        "jpg" | "jpeg" => Some(Picture::Jpeg),
        "gif" => Some(Picture::Gif),
        "webp" => Some(Picture::Webp),
        _ => None,
    }
}

impl Picture {
    fn ext(self) -> &'static str {
        match self {
            Picture::Png => "png",
            Picture::Jpeg => "jpg",
            Picture::Gif => "gif",
            Picture::Webp => "webp",
        }
    }
    /// Whether `bytes` are one: its signature, whatever was said of it.
    fn is(self, bytes: &[u8]) -> bool {
        match self {
            Picture::Png => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
            Picture::Jpeg => bytes.starts_with(&[0xFF, 0xD8, 0xFF]),
            Picture::Gif => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
            Picture::Webp => bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP",
        }
    }
}

/// The catalog, searched: the query's parts, each checked and escaped.
async fn search(arg: &Value) -> Result<Value, String> {
    let mut query: Vec<String> = Vec::new();
    let field = |k: &str| arg.field(k).map(text).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    if let Some(q) = field("q") {
        query.push(format!("q={}", auth::encode_path(&q.chars().take(200).collect::<String>())));
    }
    if let Some(c) = field("category").filter(|c| c.len() <= 40 && c.bytes().all(|b| b.is_ascii_lowercase() || b == b'-')) {
        query.push(format!("category={c}"));
    }
    if let Some(k) = field("keyword").map(|k| k.chars().take(40).collect::<String>()) {
        query.push(format!("keyword={}", auth::encode_path(&k)));
    }
    if let Some(s) = field("sort").filter(|s| ["relevance", "popular", "updated", "new"].contains(&s.as_str())) {
        query.push(format!("sort={s}"));
    }
    if let Some(c) = field("cursor").filter(|c| c.len() <= 512) {
        query.push(format!("cursor={}", auth::encode_path(&c)));
    }
    let limit = match arg.field("limit") {
        Some(Value::Num(n)) if n.is_finite() && *n >= 1.0 => (*n as u64).min(PAGE_MOST),
        _ => 30,
    };
    query.push(format!("limit={limit}"));
    if matches!(arg.field("featured"), Some(Value::Bool(true))) {
        query.push("featured=1".into());
    }
    public(format!("/store/v1/plugins?{}", query.join("&"))).await
}

/// What a failed answer says: "not_found", "offline", or the edge's
/// `"<error>: <message>"` (its stable word first).
fn failure(status: u16, body: &Json) -> String {
    if status == 404 {
        return "not_found".into();
    }
    let word = body["error"].as_str().filter(|w| !w.is_empty() && w.len() <= 40 && w.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'));
    let message: String = body["message"].as_str().unwrap_or_default().chars().take(300).collect();
    match word {
        Some(word) if !message.is_empty() => format!("{word}: {message}"),
        Some(word) => word.to_string(),
        None => format!("failed: the store answered {status}"),
    }
}

async fn public(path: String) -> Result<Value, String> {
    match auth::edge_public(path).await {
        Ok((200, body)) => Ok(json_value(&body)),
        Ok((status, body)) => Err(failure(status, &body)),
        Err(word) => Err(word.into()),
    }
}

async fn authed(method: &'static str, path: String, body: Option<Json>) -> Result<Value, String> {
    match auth::edge(method, path, body).await {
        Ok((200..=299, body)) => Ok(json_value(&body)),
        Ok((status, body)) => Err(failure(status, &body)),
        Err(word) => Err(word.into()),
    }
}

/// Where its pictures are kept here.
fn pictures_dir() -> Option<PathBuf> {
    crate::engine::data_dir().map(|d| d.join("store-pictures"))
}

/// A picture's file here: named by what it is (its plugin, version and
/// path), never by anything it says.
fn picture_file(dir: &Path, id: &str, version: &str, path: &str, kind: Picture) -> PathBuf {
    let mut hash = Sha256::new();
    hash.update(format!("{id}\0{version}\0{path}"));
    dir.join(format!("{:x}.{}", hash.finalize(), kind.ext()))
}

async fn picture(id: &str, version: &str, path: &str, kind: Picture) -> Result<Value, String> {
    let dir = pictures_dir().ok_or("offline")?;
    let file = picture_file(&dir, id, version, path, kind);
    let src = || Value::record([("src", Value::from(file.to_string_lossy().as_ref()))]);
    // (A version's files never change: kept, it's that file.)
    let there = file.clone();
    if off_thread(move || there.is_file()).await.unwrap_or(false) {
        return Ok(src());
    }
    let route = format!("/store/v1/plugins/{id}/files/{}/{}", auth::encode_path(version), escaped(path));
    let (status, said, bytes) = auth::edge_public_file(route, PICTURE_MOST).await?;
    if status != 200 {
        return Err(if status == 404 { "not_found".into() } else { format!("failed: the store answered {status}") });
    }
    if !said.starts_with("image/") || !kind.is(&bytes) {
        return Err("not_a_picture".into());
    }
    let kept = file.clone();
    off_thread(move || keep(&dir, &kept, &bytes))
        .await
        .ok_or("failed")?
        .map_err(|e| e.to_string())?;
    Ok(src())
}

/// `work` on a thread of its own, its answer awaited here (None only if it
/// panicked): files are never read or written on the window's thread.
async fn off_thread<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tx, mut rx) = zgx::task::latest();
    std::thread::spawn(move || {
        tx.send(work());
    });
    rx.recv().await
}

/// `bytes` kept at `file`, written aside then moved; then the oldest let go
/// past [`PICTURES_KEPT`]. (On a thread of its own: files.)
fn keep(dir: &Path, file: &Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let aside = file.with_extension("part");
    std::fs::write(&aside, bytes)?;
    std::fs::rename(&aside, file)?;
    prune(dir, PICTURES_KEPT);
    Ok(())
}

/// The pictures in `dir`, the oldest first let go until they're at most
/// `most` bytes in all.
fn prune(dir: &Path, most: u64) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            meta.is_file().then(|| (meta.modified().unwrap_or(std::time::UNIX_EPOCH), meta.len(), e.path()))
        })
        .collect();
    let mut total: u64 = files.iter().map(|(_, len, _)| len).sum();
    files.sort_by_key(|(at, ..)| *at);
    for (_, len, path) in files {
        if total <= most {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= len;
        }
    }
}

/// A package path's parts, each escaped, joined by `/`.
fn escaped(path: &str) -> String {
    path.split('/').map(auth::encode_path).collect::<Vec<_>>().join("/")
}

thread_local! {
    /// Icons as read, by `id@version` (a version's never change).
    static ICONS: std::cell::RefCell<std::collections::HashMap<String, Option<String>>> = Default::default();
}

async fn icon(id: &str, version: &str, path: &str) -> Result<Value, String> {
    let key = format!("{id}@{version}");
    if let Some(known) = ICONS.with(|i| i.borrow().get(&key).cloned()) {
        return Ok(known.map_or(Value::Nil, |svg| Value::record([("svg", Value::from(svg.as_str()))])));
    }
    let route = format!("/store/v1/plugins/{id}/files/{}/{}", auth::encode_path(version), escaped(path));
    let svg = match auth::edge_public_file(route, ICON_MOST).await {
        Ok((200, _, bytes)) => svg_of(bytes),
        Ok(_) | Err("too_large") => None,
        Err(word) => return Err(word.into()),
    };
    ICONS.with(|i| i.borrow_mut().insert(key, svg.clone()));
    Ok(svg.map_or(Value::Nil, |svg| Value::record([("svg", Value::from(svg.as_str()))])))
}

/// An SVG's text, as an installed plugin's icon is taken (mod.rs
/// `icon_svg`): UTF-8 that starts as an SVG does.
fn svg_of(bytes: Vec<u8>) -> Option<String> {
    let text = String::from_utf8(bytes).ok()?;
    text.trim_start().starts_with("<svg").then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_store_id_is_one() {
        assert_eq!(store_id("focus-timer").as_deref(), Some("focus-timer"));
        for bad in ["", "a", "-a", "A1", "a_b", "a/b", "a..b/", &"a".repeat(41), "fócus"] {
            assert_eq!(store_id(bad), None, "{bad}");
        }
    }

    /// A README's picture is a file of its package: never a URL, a path out
    /// of it, or one with a scheme.
    #[test]
    fn a_picture_is_a_file_of_its_package_alone() {
        assert_eq!(package_path("shots/a.png").as_deref(), Some("shots/a.png"));
        assert_eq!(package_path("./shots//a.png").as_deref(), Some("shots/a.png"));
        for bad in [
            "",
            "/etc/passwd",
            "../x.png",
            "shots/../../x.png",
            "https://example.com/a.png",
            "file:///tmp/a.png",
            "C:\\a.png",
            "data:image/png;base64,AAAA",
            "a\0.png",
        ] {
            assert_eq!(package_path(bad), None, "{bad:?}");
        }
        assert_eq!(picture_kind("a.PNG"), Some(Picture::Png));
        assert_eq!(picture_kind("a.svg"), None, "a README's picture is never an SVG (it could reach out)");
        assert_eq!(picture_kind("a.png.exe"), None);
        assert_eq!(escaped("shots/a b.png"), "shots/a%20b.png");
    }

    /// A picture is what its bytes are, not what it's called or said to be.
    #[test]
    fn a_picture_is_checked_by_its_bytes() {
        assert!(Picture::Png.is(b"\x89PNG\r\n\x1a\nrest"));
        assert!(!Picture::Png.is(b"<svg onload=alert(1)>"));
        assert!(Picture::Jpeg.is(&[0xFF, 0xD8, 0xFF, 0xE0]));
        assert!(Picture::Gif.is(b"GIF89a.."));
        assert!(Picture::Webp.is(b"RIFF\0\0\0\0WEBPVP8 "));
        assert!(!Picture::Webp.is(b"RIFF\0\0\0\0AVI "));
    }

    #[test]
    fn a_kept_picture_is_named_by_what_it_is() {
        let dir = Path::new("/tmp/x");
        let a = picture_file(dir, "focus", "1.0.0", "a.png", Picture::Png);
        let b = picture_file(dir, "focus", "1.0.1", "a.png", Picture::Png);
        assert_ne!(a, b);
        assert!(a.starts_with(dir));
        assert_eq!(a.extension().and_then(|e| e.to_str()), Some("png"));
    }

    #[test]
    fn pictures_kept_stay_under_their_room_the_oldest_going_first() {
        let dir = std::env::temp_dir().join(format!("zeron-store-pictures-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (i, name) in ["old.png", "mid.png", "new.png"].iter().enumerate() {
            let path = dir.join(name);
            std::fs::write(&path, vec![0u8; 100]).unwrap();
            let at = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000 + i as u64 * 10);
            std::fs::File::options().write(true).open(&path).unwrap().set_modified(at).unwrap();
        }
        prune(&dir, 200);
        assert!(!dir.join("old.png").exists());
        assert!(dir.join("mid.png").exists() && dir.join("new.png").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_icon_is_an_svg_s_text() {
        assert!(svg_of(b"  <svg viewBox=\"0 0 1 1\"/>".to_vec()).is_some());
        assert!(svg_of(b"<html><svg/></html>".to_vec()).is_none());
        assert!(svg_of(vec![0xFF, 0xFE]).is_none());
    }

    /// What the store says went wrong, its stable word first; anything else
    /// never passes for one.
    #[test]
    fn failures_say_the_store_s_word() {
        assert_eq!(failure(404, &Json::Null), "not_found");
        assert_eq!(failure(429, &json!({ "error": "rate_limited", "message": "Too many requests" })), "rate_limited: Too many requests");
        assert_eq!(failure(500, &json!({ "error": "Not A Word <b>", "message": "x" })), "failed: the store answered 500");
        assert_eq!(failure(400, &json!({ "error": "not_semver" })), "not_semver");
    }

    #[test]
    fn versions_and_keys_are_only_their_characters() {
        assert!(version_ok("1.2.3-beta.1+build"));
        assert!(!version_ok("1.2.3/../x"));
        assert!(!version_ok(""));
        assert!(key_id("AbC-_09"));
        assert!(!key_id("a/b"));
    }
}
