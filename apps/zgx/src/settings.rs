//! Settings' calls (ui/screens/settings): the account's profile and its
//! organization on the edge, as the account (src/auth.rs's bearer), and this
//! Mac's settings the engine keeps (commit credit, where worktrees go, which
//! model names threads). Each rejects with an outcome word as its `message`
//! (offline, signed_out, refused, invalid, self, not_found, rate_limited,
//! failed).
//!
//!   account.me                          { first_name, last_name, name, email,
//!                                         title?, avatar? }
//!   account.rename(name)                the same, renamed
//!   account.set_title(title)            the same, its title set ("" clears it)
//!   account.photo_size({ path })        { w, h }: a picture's size, to crop it
//!   account.set_photo({ path, x, y, side }) the square at x, y (pixels) cut
//!                                       from the picture, made 512 at most,
//!                                       as your photo: the profile
//!   account.remove_photo                the profile, without one
//!   account.upload_image({ path, x, y, side })
//!                                       an image of yours for something of
//!                                       yours (a project's icon): the square
//!                                       cut as a photo's, 256 at most, kept
//!                                       by the edge; its https URL
//!   account.delete_image({ url })       one of those, gone (true), when it
//!                                       is the edge's and yours
//!   account.photo_file({ url })         a photo's local copy (a script's
//!                                       `ui.image` takes files only): fetched
//!                                       once, kept by the URL's hash; nil if
//!                                       it can't be had
//!   account.delete({ confirm })         the account deleted by the engine
//!                                       (`DeleteAccount`, your address typed
//!                                       back; it answers {ok, error?}). Refused
//!                                       step_up_required until
//!                                       you've just signed in again (R16: no
//!                                       local agent can delete it), last_admin
//!                                       while you're the only admin of an org
//!                                       others are in, busy, try_again
//!   account.delete_signed_in({ confirm }) the same after signing in again: the
//!                                       engine's sign-in page (`StepUp`) opened
//!                                       in the browser, then the deletion
//!                                       retried until it lands; cancelled
//!                                       (account.cancel_step_up), timed_out
//!   account.cancel_step_up              stops waiting for that sign-in
//!   account.signing_keys                { keys = { { key, fingerprint,
//!                                         device_name, this_device,
//!                                         created_at, revoked_at? } },
//!                                         new_keys = { key } }: the keys the
//!                                         account's devices sign plugins and
//!                                         templates with (the engine's
//!                                         `PluginSigningKey`; times in seconds),
//!                                         and those added on other devices
//!                                         since this one last looked
//!   account.revoke_key(key)             one of them revoked
//!   account.members(organization_id)    { { id, name, email, role, you,
//!                                         avatar?, username?, joined_at? } }
//!   account.remove_member({ organization_id, id })
//!   account.set_role({ organization_id, id, role })
//!   account.invitations(organization_id) { { id, email, state, expires_at,
//!                                         created_at } }
//!   account.revoke_invite({ organization_id, id })
//!   account.resend_invite({ organization_id, id })
//!   account.rename_org({ organization_id, name })
//!   settings.engine({ method, params })  one of the engine's settings
//!                                         methods (ENGINE_METHODS), its answer
//!   notifications.get / .set({ desktop?, background_only?, agent_updates? })
//!                                       the banners the engine posts
//!   sound.play(kind)                    a chime: done, request, attention
//!   wallpaper.keep({ path })            a new chat's background image: a
//!                                       copy of the picked file kept in the
//!                                       engine's data folder (the shipping
//!                                       app's managed copy), checked to
//!                                       decode first: { path, name, w, h }
//!   wallpaper.forget({ path })          a copy `keep` made, removed (nothing
//!                                       outside that folder is touched)
//!
//! An id here is a membership's or an invitation's: no account's id reaches
//! the scripts.

use serde_json::{Value as Json, json};
use zgx_desktop::WindowContext;
use zgx_script::{ScriptHost, Value};

use crate::auth::{edge, edge_bytes, encode_path, text};
use crate::engine;
use crate::plugins::{json_value, value_json};

type Outcome = &'static str;

/// The engine's methods Settings may call: this Mac's own settings.
const ENGINE_METHODS: &[&str] = &[
    "GetCommitCredit",
    "SetCommitCredit",
    "GetWorktreeSettings",
    "SetWorktreeSettings",
    "GetTitleSettings",
    "SetTitleSettings",
    "GetNativeSubagents",
    "SetNativeSubagents",
    // Whether this window has the keys: banners set to "only when in the
    // background" hold while it does.
    "SetViewportFocus",
];

pub fn install(_cx: &mut WindowContext, host: &ScriptHost) {
    host.async_action("account.me", |_| async { crate::plugins::preview::refuse("account.me")?; me("GET", None).await });
    host.async_action("account.rename", |name| {
        let body = json!({ "name": text(&name) });
        async move { crate::plugins::preview::refuse("account.rename")?; me("PATCH", Some(body)).await }
    });
    host.async_action("account.set_title", |title| {
        let body = json!({ "title": text(&title) });
        async move { crate::plugins::preview::refuse("account.set_title")?; me("PATCH", Some(body)).await }
    });
    host.async_action("account.photo_size", |args| {
        let path = args.field("path").map(text).unwrap_or_default();
        async move { crate::plugins::preview::refuse("account.photo_size")?;
            let (w, h) = off_thread(move || picture(&path).map(|(w, h, _)| (w, h))).await.ok_or("failed")??;
            Ok(Value::record([("w", Value::from(w as f64)), ("h", Value::from(h as f64))]))
        }
    });
    host.async_action("account.set_photo", |args| {
        let path = args.field("path").map(text).unwrap_or_default();
        let num = |key: &str| args.field(key).and_then(Value::as_f64).unwrap_or(0.0);
        let (x, y, side) = (num("x"), num("y"), num("side"));
        async move { crate::plugins::preview::refuse("account.set_photo")?;
            let png = off_thread(move || square_png(&path, x, y, side)).await.ok_or("failed")??;
            match edge_bytes("PUT", "/auth/me/photo".into(), png, "image/png").await? {
                (200, body) => Ok(me_value(&body)),
                (413, _) => Err("too_large".into()),
                (status, body) => Err(outcome(status, &body).into()),
            }
        }
    });
    host.async_action("account.photo_file", |args| {
        let url = args.field("url").map(text).unwrap_or_default();
        async move { crate::plugins::preview::refuse("account.photo_file")?;
            let Some(dir) = engine::data_dir().map(|d| d.join("photos")) else {
                return Ok(Value::Nil);
            };
            let fetched = off_thread(move || photo_file(&dir, &url)).await;
            Ok(match fetched {
                Some(Ok(path)) => Value::from(path.to_string_lossy().as_ref()),
                _ => Value::Nil,
            })
        }
    });
    // An image for something of yours (a project's icon): the square cut
    // as a photo's is, smaller (a tile is never past 128 points), kept by
    // the edge beside the photos; its public URL.
    host.async_action("account.upload_image", |args| {
        let path = args.field("path").map(text).unwrap_or_default();
        let num = |key: &str| args.field(key).and_then(Value::as_f64).unwrap_or(0.0);
        let (x, y, side) = (num("x"), num("y"), num("side"));
        async move { crate::plugins::preview::refuse("account.upload_image")?;
            let png = off_thread(move || square_png_at(&path, x, y, side, &[IMAGE_SIDE, 192])).await.ok_or("failed")??;
            match edge_bytes("PUT", "/auth/me/images".into(), png, "image/png").await? {
                (200, body) => body["url"].as_str().map(Value::from).ok_or_else(|| "failed".to_string()),
                (413, _) => Err("too_large".into()),
                // An edge without images of yours yet (production, before
                // its deploy): said plainly, so an emoji or a color is chosen.
                (404 | 405, _) => Err("unavailable".into()),
                (status, body) => Err(outcome(status, &body).into()),
            }
        }
    });
    // One of those let go of (replaced): only an image of the edge's own,
    // by its key; anything else is left alone.
    host.async_action("account.delete_image", |args| {
        let url = args.field("url").map(text).unwrap_or_default();
        async move { crate::plugins::preview::refuse("account.delete_image")?;
            let Some(key) = image_key(&url) else {
                return Ok(Value::Bool(false));
            };
            match edge("DELETE", format!("/auth/me/images/{key}"), None).await? {
                (200, _) => Ok(Value::Bool(true)),
                _ => Ok(Value::Bool(false)),
            }
        }
    });
    host.async_action("account.remove_photo", |_| async { crate::plugins::preview::refuse("account.remove_photo")?;
        match edge("DELETE", "/auth/me/photo".into(), None).await? {
            (200, body) => Ok(me_value(&body)),
            (status, body) => Err(outcome(status, &body).into()),
        }
    });
    // Deleting the account is the engine's, with the ticket of a sign-in
    // made just now (R16): the bearer alone can't, so no local agent can.
    host.async_action("account.delete", |args| {
        let confirm = args.field("confirm").map(text).unwrap_or_default();
        async move {
            crate::plugins::preview::refuse("account.delete")?;
            delete_account(&confirm).await.map(|()| Value::Bool(true)).map_err(String::from)
        }
    });
    host.async_action("account.delete_signed_in", |args| {
        let confirm = args.field("confirm").map(text).unwrap_or_default();
        async move {
            crate::plugins::preview::refuse("account.delete_signed_in")?;
            delete_signed_in(&confirm).await.map(|()| Value::Bool(true)).map_err(String::from)
        }
    });
    host.action("account.cancel_step_up", |_| {
        STEP_UP.with(|n| n.set(n.get() + 1));
    });
    host.async_action("account.signing_keys", |_| async {
        crate::plugins::preview::refuse("account.signing_keys")?;
        let answer = engine::get()
            .call("PluginSigningKey", json!({ "keys": true }))
            .await
            .map_err(|error| engine::outcome(&error))?;
        Ok(json_value(&signing_keys(&answer)))
    });
    host.async_action("account.revoke_key", |key| {
        let key = text(&key);
        async move {
            crate::plugins::preview::refuse("account.revoke_key")?;
            if key.trim().is_empty() {
                return Err("invalid".into());
            }
            engine::get()
                .call("PluginSigningKey", json!({ "revoke": key }))
                .await
                .map(|_| Value::Bool(true))
                .map_err(|error| engine::outcome(&error).into())
        }
    });
    host.async_action("account.members", |org| {
        let path = format!("/auth/orgs/{}/members", encode_path(&text(&org)));
        async move { crate::plugins::preview::refuse("account.members")?; list(path, "members").await }
    });
    host.async_action("account.invitations", |org| {
        let path = format!("/auth/orgs/{}/invitations", encode_path(&text(&org)));
        async move { crate::plugins::preview::refuse("account.invitations")?; list(path, "invitations").await }
    });
    host.async_action("account.remove_member", |args| {
        let path = item(&args, "members", None);
        async move { crate::plugins::preview::refuse("account.remove_member")?; done("DELETE", path, None).await }
    });
    host.async_action("account.set_role", |args| {
        let path = item(&args, "members", None);
        let body = json!({ "role": args.field("role").map(text).unwrap_or_default() });
        async move { crate::plugins::preview::refuse("account.set_role")?; done("PATCH", path, Some(body)).await }
    });
    host.async_action("account.revoke_invite", |args| {
        let path = item(&args, "invitations", None);
        async move { crate::plugins::preview::refuse("account.revoke_invite")?; done("DELETE", path, None).await }
    });
    host.async_action("account.resend_invite", |args| {
        let path = item(&args, "invitations", Some("resend"));
        async move { crate::plugins::preview::refuse("account.resend_invite")?; done("POST", path, None).await }
    });
    host.async_action("account.rename_org", |args| {
        let org = args.field("organization_id").map(text).unwrap_or_default();
        let path = format!("/auth/orgs/{}", encode_path(&org));
        let body = json!({ "name": args.field("name").map(text).unwrap_or_default() });
        async move { crate::plugins::preview::refuse("account.rename_org")?; done("PATCH", path, Some(body)).await }
    });
    // Banners are the engine's (crates/engine desktop_notify): it reads
    // these three from `ui-settings.json` in its data folder whenever one
    // is due, so a change here applies to the next.
    host.async_action("notifications.get", |_| async {
        // None yet: the engine's defaults (all on). There but unreadable:
        // said, not answered with defaults that aren't what it holds.
        let prefs = match ui_settings_path().map(|path| read_settings_at(&path)) {
            Some(Ok(Some(prefs))) => prefs,
            Some(Ok(None)) => json!({}),
            Some(Err(())) | None => return Err("failed".to_string()),
        };
        let flag = |key: &str| Value::Bool(prefs[key].as_bool().unwrap_or(true));
        Ok(Value::record([
            ("desktop", flag(BANNERS)),
            ("background_only", flag(BACKGROUND_ONLY)),
            ("agent_updates", flag(AGENT_UPDATES)),
        ]))
    });
    host.async_action("notifications.set", |args| {
        let mut changes = serde_json::Map::new();
        for (field, key) in [("desktop", BANNERS), ("background_only", BACKGROUND_ONLY), ("agent_updates", AGENT_UPDATES)] {
            if let Some(Value::Bool(on)) = args.field(field) {
                changes.insert(key.into(), Json::Bool(*on));
            }
        }
        async move { write_ui_settings(changes).map(|()| Value::Nil).map_err(|()| "failed".to_string()) }
    });
    // A session's chime, the shipping app's own: "done", "request" (an
    // agent waiting on you) or "attention" (failed, or the connection gone).
    host.action("sound.play", |kind| {
        let data: &'static [u8] = match text(&kind).as_str() {
            "done" => SOUND_DONE,
            "request" => SOUND_REQUEST,
            "attention" => SOUND_ATTENTION,
            _ => return,
        };
        std::thread::spawn(move || play(data));
    });
    // A new chat's background image, as the shipping app keeps it
    // (settings.rs `install_new_thread_composer_background`): a copy of its
    // own, so moving or deleting the original leaves the wallpaper as it is.
    host.async_action("wallpaper.keep", |args| {
        let path = args.field("path").map(text).unwrap_or_default();
        async move {
            if !std::path::Path::new(&path).is_absolute() {
                return Err("invalid".to_string());
            }
            let dir = engine::data_dir().ok_or("failed")?.join(WALLPAPER_DIR);
            let kept = off_thread(move || keep_wallpaper(&path, &dir, screen_pixels())).await.ok_or("failed")??;
            Ok(Value::record([
                ("path", Value::from(kept.path.as_str())),
                ("name", Value::from(kept.name.as_str())),
                ("w", Value::from(kept.w as f64)),
                ("h", Value::from(kept.h as f64)),
            ]))
        }
    });
    host.action("wallpaper.forget", |args| {
        let path = args.field("path").map(text).unwrap_or_default();
        if let Some(dir) = engine::data_dir() {
            forget_wallpaper(&path, &dir.join(WALLPAPER_DIR));
        }
    });
    host.async_action("settings.engine", |args| {
        let method = args.field("method").map(text).unwrap_or_default();
        let params = args.field("params").map(value_json).unwrap_or(json!({}));
        async move {
            if !ENGINE_METHODS.contains(&method.as_str()) {
                return Err("refused".into());
            }
            let params = if params.is_object() { params } else { json!({}) };
            engine::get()
                .call(&method, params)
                .await
                .map(|answer| json_value(&answer))
                .map_err(|error| {
                    // An engine too old for the method: said as what to do.
                    if error.to_ascii_lowercase().contains("unknown method") {
                        "outdated".into()
                    } else {
                        engine::outcome(&error).into()
                    }
                })
        }
    });
}

/// The largest background image taken.
const WALLPAPER_MAX_BYTES: u64 = 64 * 1024 * 1024;
/// Where the kept copies are, in the engine's data folder.
const WALLPAPER_DIR: &str = "new-chat-backgrounds";

/// A background image kept: its copy, the original's name, its size in pixels.
#[derive(Debug)]
struct KeptWallpaper {
    path: String,
    name: String,
    w: u32,
    h: u32,
}

/// The largest screen here, in pixels (its current mode's, not its
/// points): what a background needs to cover at most. None where it can't
/// be asked.
fn screen_pixels() -> Option<(u32, u32)> {
    #[cfg(target_os = "macos")]
    {
        use core_graphics::display::CGDisplay;
        let ids = CGDisplay::active_displays().ok()?;
        ids.into_iter()
            .filter_map(|id| CGDisplay::new(id).display_mode())
            .map(|mode| (mode.pixel_width() as u32, mode.pixel_height() as u32))
            .max_by_key(|(w, h)| u64::from(*w) * u64::from(*h))
    }
    #[cfg(not(target_os = "macos"))]
    None
}

/// The size a `w` by `h` picture is kept at to cover a `screen` (both
/// ways, any shape of window inside it), or none when it's no larger than
/// that already.
fn kept_size(w: u32, h: u32, screen: Option<(u32, u32)>) -> Option<(u32, u32)> {
    let (sw, sh) = screen?;
    let scale = (sw as f64 / w.max(1) as f64).max(sh as f64 / h.max(1) as f64);
    (scale < 1.).then(|| (((w as f64 * scale).ceil() as u32).max(1), ((h as f64 * scale).ceil() as u32).max(1)))
}

/// `path` copied into `dir` under a name of its own (a new name each time,
/// so nothing cached under the old one shows), once it decodes as a
/// picture: written beside it, then moved into place. A picture larger than
/// it takes to cover `screen` is kept at that size (a PNG), so the window
/// never holds more of it than it can show.
fn keep_wallpaper(path: &str, dir: &std::path::Path, screen: Option<(u32, u32)>) -> Result<KeptWallpaper, String> {
    let source = std::path::Path::new(path);
    let meta = std::fs::metadata(source).map_err(|_| "not_found".to_string())?;
    if meta.len() > WALLPAPER_MAX_BYTES {
        return Err("invalid".into());
    }
    let bytes = std::fs::read(source).map_err(|_| "failed".to_string())?;
    let image = zgx_gpu::assets::decode_image(&bytes).map_err(|_| "invalid".to_string())?;
    let (mut w, mut h) = (image.width(), image.height());
    std::fs::create_dir_all(dir).map_err(|_| "failed".to_string())?;
    let mut ext = source.extension().and_then(|e| e.to_str()).filter(|e| e.chars().all(|c| c.is_ascii_alphanumeric())).unwrap_or("png").to_ascii_lowercase();
    let mut bytes = bytes;
    if let Some((nw, nh)) = kept_size(w, h, screen) {
        let full = image::RgbaImage::from_raw(w, h, image.pixels().to_vec()).ok_or("invalid")?;
        let small = image::imageops::resize(&full, nw, nh, image::imageops::FilterType::CatmullRom);
        let mut png = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png, nw, nh);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.write_header().and_then(|mut w| w.write_image_data(small.as_raw())).map_err(|_| "failed".to_string())?;
        }
        (bytes, w, h, ext) = (png, nw, nh, "png".to_string());
    }
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or_default();
    let target = dir.join(format!("new-chat-{stamp:x}-{}.{ext}", std::process::id()));
    let temp = target.with_extension(format!("{ext}.tmp"));
    if std::fs::write(&temp, &bytes).and_then(|()| std::fs::rename(&temp, &target)).is_err() {
        let _ = std::fs::remove_file(&temp);
        return Err("failed".into());
    }
    let name = source.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Ok(KeptWallpaper { path: target.to_string_lossy().into_owned(), name, w, h })
}

/// `keep_wallpaper` for another module's test: its path and size.
#[cfg(test)]
pub(crate) fn keep_wallpaper_for_tests(path: &str, dir: &std::path::Path, screen: Option<(u32, u32)>) -> (String, u32, u32) {
    let kept = keep_wallpaper(path, dir, screen).unwrap();
    (kept.path, kept.w, kept.h)
}

/// A copy `keep_wallpaper` made, removed: only a file directly in `dir`.
fn forget_wallpaper(path: &str, dir: &std::path::Path) {
    let path = std::path::Path::new(path);
    if path.parent() == Some(dir) && path.is_file() {
        let _ = std::fs::remove_file(path);
    }
}

/// `work` on a thread of its own, its answer awaited here (None only if it
/// panicked), as images.rs's.
async fn off_thread<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tx, mut rx) = zgx::task::latest();
    std::thread::spawn(move || {
        tx.send(work());
    });
    rx.recv().await
}

const BANNERS: &str = "notificationsEnabled";
const BACKGROUND_ONLY: &str = "notificationsBackgroundOnly";
const AGENT_UPDATES: &str = "agentUpdateNotifications";

static SOUND_DONE: &[u8] = include_bytes!("../sounds/done.wav");
static SOUND_REQUEST: &[u8] = include_bytes!("../sounds/request.wav");
static SOUND_ATTENTION: &[u8] = include_bytes!("../sounds/attention.wav");

/// The engine's `ui-settings.json` (the shipping app's file, in the engine's
/// data folder), or none.
fn ui_settings_path() -> Option<std::path::PathBuf> {
    engine::get().ipc_endpoint().map(|(_, dir)| dir.join("ui-settings.json"))
}

/// The file as it stands: `None` when there is none yet; an error when it
/// is there but isn't a JSON object (or can't be read), so a write never
/// replaces the shipping app's settings with ours alone.
fn read_settings_at(path: &std::path::Path) -> Result<Option<Json>, ()> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(()),
    };
    match serde_json::from_slice::<Json>(&bytes) {
        Ok(value) if value.is_object() => Ok(Some(value)),
        _ => Err(()),
    }
}

/// `changes` merged into the file, every other key kept as it was; written
/// whole to a file of its own beside it (unique to this write), then moved
/// over it. A file that can't be read as settings is left alone.
fn write_ui_settings(changes: serde_json::Map<String, Json>) -> Result<(), ()> {
    write_settings_at(&ui_settings_path().ok_or(())?, changes)
}

fn write_settings_at(path: &std::path::Path, changes: serde_json::Map<String, Json>) -> Result<(), ()> {
    let mut prefs = read_settings_at(path)?.unwrap_or_else(|| json!({}));
    let object = prefs.as_object_mut().ok_or(())?;
    for (key, value) in changes {
        object.insert(key, value);
    }
    let bytes = serde_json::to_vec_pretty(&prefs).map_err(|_| ())?;
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let next = path.with_extension(format!("json.zgx-{}-{id}", std::process::id()));
    std::fs::write(&next, bytes).map_err(|_| ())?;
    std::fs::rename(&next, path).map_err(|_| {
        let _ = std::fs::remove_file(&next);
    })
}

/// A chime, from a file of its own that goes once it has played: on macOS
/// through the System Sound server (src/system_sound.rs says why not
/// `afplay`), on Linux through `paplay`; on Windows, `PlaySound` straight
/// from memory. It returns once the sound has played.
pub fn play(data: &[u8]) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Media::Audio::{PlaySoundW, SND_MEMORY, SND_NODEFAULT, SND_SYNC};
        // SAFETY: with SND_MEMORY the first argument is the WAV's bytes,
        // which `data` keeps alive for the whole (synchronous) play.
        unsafe { PlaySoundW(data.as_ptr().cast(), std::ptr::null_mut(), SND_MEMORY | SND_SYNC | SND_NODEFAULT) };
    }
    #[cfg(not(windows))]
    play_file(data);
}

#[cfg(not(windows))]
fn play_file(data: &[u8]) {
    let Some(path) = sound_file(data) else {
        return;
    };
    #[cfg(target_os = "macos")]
    if let Err(error) = crate::system_sound::play_file(&path) {
        eprintln!("zeron-zgx: sound: {error}");
    }
    #[cfg(not(target_os = "macos"))]
    let _ = std::process::Command::new("paplay")
        .arg(&path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    let _ = std::fs::remove_file(&path);
}

/// `data` in a temp file of its own, named for this process.
#[cfg(not(windows))]
fn sound_file(data: &[u8]) -> Option<std::path::PathBuf> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("zeron-zgx-sound-{}-{id}.wav", std::process::id()));
    std::fs::write(&path, data).ok()?;
    Some(path)
}

/// `/auth/orgs/{organization_id}/{kind}/{id}[/{then}]`.
fn item(args: &Value, kind: &str, then: Option<&str>) -> String {
    let org = args.field("organization_id").map(text).unwrap_or_default();
    let id = args.field("id").map(text).unwrap_or_default();
    let mut path = format!("/auth/orgs/{}/{kind}/{}", encode_path(&org), encode_path(&id));
    if let Some(then) = then {
        path.push('/');
        path.push_str(then);
    }
    path
}

thread_local! {
    /// Which wait for a sign-in again is the live one: cancelling moves it on.
    static STEP_UP: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// How long a sign-in again is waited for (the engine keeps its ticket 10
/// minutes at most), and how often the deletion is tried meanwhile (each try
/// is the engine's alone until the ticket lands: it asks the edge nothing).
const STEP_UP_WAIT: std::time::Duration = std::time::Duration::from_secs(10 * 60);
const STEP_UP_EVERY: std::time::Duration = std::time::Duration::from_secs(1);

/// The engine's `DeleteAccount`, once.
async fn delete_account(confirm: &str) -> Result<(), Outcome> {
    let answer = engine::get()
        .call("DeleteAccount", json!({ "confirm": confirm }))
        .await
        // (An RPC error: the engine itself didn't answer.)
        .map_err(|error| engine::outcome(&error))?;
    delete_answer(&answer)
}

/// Signs in again (the engine's `StepUp` page, in the browser), then deletes
/// once that sign-in lands: tried every second until it does, it's
/// cancelled, or ten minutes pass.
async fn delete_signed_in(confirm: &str) -> Result<(), Outcome> {
    let live = STEP_UP.with(|n| {
        n.set(n.get() + 1);
        n.get()
    });
    let answer = engine::get().call("StepUp", json!({})).await.map_err(|error| engine::outcome(&error))?;
    let url = answer["url"].as_str().unwrap_or_default();
    if !url.starts_with("https://") {
        return Err("failed");
    }
    if let Err(error) = crate::auth::open_sign_in(url) {
        eprintln!("zeron-zgx: account: the browser didn't open: {error}");
        return Err("failed");
    }
    let until = std::time::Instant::now() + STEP_UP_WAIT;
    loop {
        zgx::timer::sleep(STEP_UP_EVERY).await;
        if STEP_UP.with(std::cell::Cell::get) != live {
            return Err("cancelled");
        }
        match delete_account(confirm).await {
            Err("step_up_required") if std::time::Instant::now() < until => {}
            Err("step_up_required") => return Err("timed_out"),
            other => return other,
        }
    }
}

/// `DeleteAccount`'s answer: `{ok: true}`, or `{ok: false, error}` with the
/// engine's code, as the word the page says.
fn delete_answer(answer: &Json) -> Result<(), Outcome> {
    if answer["ok"].as_bool() == Some(true) {
        return Ok(());
    }
    Err(match answer["error"].as_str().unwrap_or_default() {
        "step_up_required" => "step_up_required",
        "signed_out" => "signed_out",
        "offline" => "offline",
        "invalid" => "invalid",
        "last_admin" => "last_admin",
        "busy" => "busy",
        // Anything else on the edge: nothing was deleted, and the same
        // ticket still works.
        "upstream" => "try_again",
        _ => "failed",
    })
}

/// What a refusal says, by its status and the edge's word.
fn outcome(status: u16, body: &Json) -> Outcome {
    match (status, body["error"].as_str()) {
        (400, Some("self")) => "self",
        (400, _) => "invalid",
        (409, Some("exists")) => "exists",
        (409, _) => "not_found",
        (403, _) => "refused",
        (404, _) => "not_found",
        (429, _) => "rate_limited",
        _ => "failed",
    }
}

/// The account's profile, read or changed.
async fn me(method: &'static str, body: Option<Json>) -> Result<Value, String> {
    match edge(method, "/auth/me".into(), body).await? {
        (200, me) => Ok(me_value(&me)),
        (status, body) => Err(outcome(status, &body).into()),
    }
}

/// The edge's `Me` as the scripts read it.
fn me_value(me: &Json) -> Value {
    let str = |key: &str| me[key].as_str().map_or(Value::Nil, Value::from);
    Value::record([
        ("first_name", str("firstName")),
        ("last_name", str("lastName")),
        ("name", str("name")),
        ("email", str("email")),
        ("title", str("title")),
        ("avatar", str("avatar")),
    ])
}

/// The largest picture read for a photo.
const PHOTO_MAX_BYTES: u64 = 64 * 1024 * 1024;
/// A photo's side, at most, in pixels; and the edge's largest.
const PHOTO_SIDE: u32 = 512;
const PHOTO_MAX_UPLOAD: usize = 1_000_000;

/// A picture's size and straight RGBA pixels.
fn picture(path: &str) -> Result<(u32, u32, Vec<u8>), String> {
    if !std::path::Path::new(path).is_absolute() {
        return Err("invalid".into());
    }
    let meta = std::fs::metadata(path).map_err(|_| "not_found".to_string())?;
    if meta.len() > PHOTO_MAX_BYTES {
        return Err("too_large".into());
    }
    let bytes = std::fs::read(path).map_err(|_| "failed".to_string())?;
    let image = zgx_gpu::assets::decode_image(&bytes).map_err(|_| "invalid".to_string())?;
    Ok((image.width(), image.height(), image.pixels().to_vec()))
}

/// An uploaded image's side, at most (a tile's largest, 64 points, at 2x
/// and some).
const IMAGE_SIDE: u32 = 256;

/// The edge's key in one of its image URLs (`…/auth/avatars/<32 hex>`).
fn image_key(url: &str) -> Option<&str> {
    let key = url.strip_prefix("https://")?.split_once("/auth/avatars/")?.1;
    (key.len() == 32 && key.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))).then_some(key)
}

/// The largest photo fetched.
const PHOTO_FETCH_MAX: u64 = 4 * 1024 * 1024;

/// `url`'s photo as a file under `dir`: kept by the URL's hash (a photo's URL
/// never changes what it shows), fetched the first time. https only.
fn photo_file(dir: &std::path::Path, url: &str) -> Result<std::path::PathBuf, String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    if !url.starts_with("https://") || url.chars().any(char::is_control) {
        return Err("invalid".into());
    }
    let name: String = Sha256::digest(url.as_bytes()).iter().take(16).map(|b| format!("{b:02x}")).collect();
    let path = dir.join(format!("{name}.img"));
    if path.is_file() {
        return Ok(path);
    }
    let agent = ureq::AgentBuilder::new().timeout(std::time::Duration::from_secs(15)).user_agent("zeron-zgx").build();
    let response = agent.get(url).call().map_err(|_| "failed".to_string())?;
    if !response.content_type().to_ascii_lowercase().starts_with("image/") {
        return Err("invalid".into());
    }
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(PHOTO_FETCH_MAX + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "failed".to_string())?;
    if bytes.is_empty() || bytes.len() as u64 > PHOTO_FETCH_MAX {
        return Err("too_large".into());
    }
    std::fs::create_dir_all(dir).map_err(|_| "failed".to_string())?;
    // (Whole or not at all: written beside, then moved into place.)
    let part = path.with_extension("part");
    std::fs::write(&part, &bytes).map_err(|_| "failed".to_string())?;
    std::fs::rename(&part, &path).map_err(|_| "failed".to_string())?;
    Ok(path)
}

/// The square at `x`, `y` (`side` pixels, kept inside the picture) cut from
/// the picture, made `PHOTO_SIDE` at most, as a PNG; smaller again if that's
/// past what the edge takes.
fn square_png(path: &str, x: f64, y: f64, side: f64) -> Result<Vec<u8>, String> {
    square_png_at(path, x, y, side, &[PHOTO_SIDE, 384, 256])
}

/// The same, made the first of `sides` (pixels) it fits the edge at.
fn square_png_at(path: &str, x: f64, y: f64, side: f64, sides: &[u32]) -> Result<Vec<u8>, String> {
    let (w, h, pixels) = picture(path)?;
    let rgba = image::RgbaImage::from_raw(w, h, pixels).ok_or("invalid")?;
    let short = w.min(h).max(1);
    let side = (side.round() as u32).clamp(1, short);
    let x = (x.round().max(0.0) as u32).min(w - side);
    let y = (y.round().max(0.0) as u32).min(h - side);
    let square = image::imageops::crop_imm(&rgba, x, y, side, side).to_image();
    for &target in sides {
        let out = if side > target {
            image::imageops::resize(&square, target, target, image::imageops::FilterType::Lanczos3)
        } else {
            square.clone()
        };
        let mut png = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png, out.width(), out.height());
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_compression(png::Compression::High);
            encoder
                .write_header()
                .and_then(|mut w| w.write_image_data(out.as_raw()))
                .map_err(|_| "failed".to_string())?;
        }
        if png.len() <= PHOTO_MAX_UPLOAD {
            return Ok(png);
        }
    }
    Err("too_large".into())
}

/// A list the edge answers under `key`, its fields as the scripts read them
/// (snake_case).
async fn list(path: String, key: &str) -> Result<Value, String> {
    match edge("GET", path, None).await? {
        (200, body) => {
            let items: Vec<Value> = body[key]
                .as_array()
                .map(|items| items.iter().map(|item| json_value(&snake(item))).collect())
                .unwrap_or_default();
            Ok(Value::List(items.into()))
        }
        (status, body) => Err(outcome(status, &body).into()),
    }
}

/// A change: nil once made.
async fn done(method: &'static str, path: String, body: Option<Json>) -> Result<Value, String> {
    match edge(method, path, body).await? {
        (200, _) => Ok(Value::Nil),
        (status, body) => Err(outcome(status, &body).into()),
    }
}

/// `PluginSigningKey {keys: true}`'s answer as the scripts read it: each
/// key's row in snake_case with its times in seconds (the engine's are ms),
/// and `new_keys` as the keys alone.
fn signing_keys(answer: &Json) -> Json {
    let seconds = |ms: &Json| ms.as_f64().map_or(Json::Null, |ms| json!(ms / 1000.0));
    let keys: Vec<Json> = answer["keys"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .map(|row| {
                    let mut row = snake(row);
                    row["created_at"] = seconds(&row["created_at"]);
                    row["revoked_at"] = seconds(&row["revoked_at"]);
                    row
                })
                .collect()
        })
        .unwrap_or_default();
    let new_keys: Vec<Json> = answer["new_keys"]
        .as_array()
        .map(|rows| rows.iter().filter_map(|row| row["key"].as_str().map(|key| json!(key))).collect())
        .unwrap_or_default();
    json!({ "keys": keys, "new_keys": new_keys })
}

/// An object's keys in snake_case (`joinedAt` → `joined_at`), one level.
fn snake(item: &Json) -> Json {
    let Some(map) = item.as_object() else {
        return item.clone();
    };
    let mut out = serde_json::Map::new();
    for (key, value) in map {
        let mut name = String::with_capacity(key.len() + 2);
        for c in key.chars() {
            if c.is_ascii_uppercase() {
                name.push('_');
                name.push(c.to_ascii_lowercase());
            } else {
                name.push(c);
            }
        }
        out.insert(name, value.clone());
    }
    Json::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Registering is everything short of sounding: the System Sound server
    // opens and accepts every sound zgx plays, from the file it's played
    // from. (A test can't listen for it.)
    #[cfg(target_os = "macos")]
    #[test]
    fn every_sound_registers_with_the_system_sound_server() {
        for data in [SOUND_DONE, SOUND_REQUEST, SOUND_ATTENTION, crate::appshots::SOUND] {
            let path = sound_file(data).expect("write the sound");
            let registered = crate::system_sound::register(&path);
            let _ = std::fs::remove_file(&path);
            registered.expect("the sound server should take the sound");
        }
    }

    // Sounds a chime, the app's own way, for checking by hand who holds the
    // audio assertion: `log stream --process runningboardd` should name
    // systemsoundserverd, not this process. Returns once it has played and
    // its file is gone.
    #[test]
    #[ignore = "plays a sound"]
    fn a_chime_plays_and_its_file_goes() {
        let before = std::time::Instant::now();
        play(SOUND_DONE);
        eprintln!("played in {} ms", before.elapsed().as_millis());
        #[cfg(not(windows))]
        {
            let prefix = format!("zeron-zgx-sound-{}-", std::process::id());
            let left = std::fs::read_dir(std::env::temp_dir())
                .unwrap()
                .flatten()
                .any(|entry| entry.file_name().to_string_lossy().starts_with(&prefix));
            assert!(!left, "the chime's file should be gone");
        }
    }

    #[test]
    fn signing_keys_read_as_the_scripts_do() {
        let answer = json!({
            "key": "k1",
            "registered": true,
            "new_keys": [{ "key": "k2", "fingerprint": "bbbb", "deviceId": "d2", "createdAt": 2000 }],
            "keys": [
                { "key": "k1", "fingerprint": "aaaa", "deviceId": "d1", "createdAt": 1000, "revokedAt": null, "this_device": true, "device_name": "Mac" },
                { "key": "k3", "fingerprint": "cccc", "deviceId": "d3", "createdAt": 3000, "revokedAt": 4000, "this_device": false, "device_name": "dev" },
            ],
        });
        let read = signing_keys(&answer);
        assert_eq!(read["new_keys"], json!(["k2"]));
        assert_eq!(read["keys"][0]["this_device"], true);
        assert_eq!(read["keys"][0]["created_at"], 1.0);
        assert_eq!(read["keys"][0]["revoked_at"], Json::Null);
        assert_eq!(read["keys"][1]["revoked_at"], 4.0);
        assert_eq!(read["keys"][1]["device_name"], "dev");
        assert_eq!(read["keys"][1]["device_id"], "d3");
    }

    #[test]
    fn delete_answers_are_said_as_words() {
        assert_eq!(delete_answer(&json!({ "ok": true })), Ok(()));
        let refused = |code: &str| delete_answer(&json!({ "ok": false, "error": code, "message": "…" }));
        for code in ["step_up_required", "signed_out", "offline", "invalid", "last_admin", "busy"] {
            assert_eq!(refused(code), Err(code));
        }
        assert_eq!(refused("upstream"), Err("try_again"));
        assert_eq!(refused("something_new"), Err("failed"));
        assert_eq!(delete_answer(&json!({})), Err("failed"));
    }

    #[test]
    fn keys_read_as_the_scripts_do() {
        let item = snake(&json!({ "joinedAt": "2026-10-31", "id": "om_1", "expiresAt": 1 }));
        assert_eq!(item, json!({ "joined_at": "2026-10-31", "id": "om_1", "expires_at": 1 }));
    }

    #[test]
    fn a_settings_file_keeps_its_other_keys_and_a_broken_one_is_left_alone() {
        let dir = std::env::temp_dir().join(format!("zgx-ui-settings-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ui-settings.json");
        let mut changes = serde_json::Map::new();
        changes.insert("notificationsEnabled".into(), Json::Bool(false));
        // None yet: made, with ours.
        write_settings_at(&path, changes.clone()).unwrap();
        assert_eq!(read_settings_at(&path).unwrap(), Some(json!({ "notificationsEnabled": false })));
        // The shipping app's keys stay.
        std::fs::write(&path, r#"{"theme":"dark","notificationsEnabled":true}"#).unwrap();
        write_settings_at(&path, changes.clone()).unwrap();
        assert_eq!(read_settings_at(&path).unwrap(), Some(json!({ "theme": "dark", "notificationsEnabled": false })));
        // Not settings (half written, or another shape): refused, untouched.
        std::fs::write(&path, "{\"theme\": ").unwrap();
        assert!(write_settings_at(&path, changes.clone()).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"theme\": ");
        std::fs::write(&path, "[1, 2]").unwrap();
        assert!(write_settings_at(&path, changes).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[1, 2]");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_background_image_is_kept_as_a_copy_and_forgotten_only_there() {
        let dir = std::env::temp_dir().join(format!("zgx-wallpaper-test-{}", std::process::id()));
        let kept_dir = dir.join("kept");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Orange.PNG");
        let mut png = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png, 200, 90);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.write_header().unwrap().write_image_data(&[230, 100, 25, 255].repeat(200 * 90)).unwrap();
        }
        std::fs::write(&path, &png).unwrap();
        let kept = keep_wallpaper(path.to_str().unwrap(), &kept_dir, None).unwrap();
        assert_eq!((kept.name.as_str(), kept.w, kept.h), ("Orange.PNG", 200, 90));
        assert!(kept.path.ends_with(".png") && kept.path.starts_with(kept_dir.to_str().unwrap()));
        assert_eq!(std::fs::read(&kept.path).unwrap(), png);
        // Not a picture, or not there: refused, nothing kept.
        std::fs::write(dir.join("notes.txt"), "hello").unwrap();
        assert_eq!(keep_wallpaper(dir.join("notes.txt").to_str().unwrap(), &kept_dir, None).unwrap_err(), "invalid");
        assert_eq!(keep_wallpaper(dir.join("gone.png").to_str().unwrap(), &kept_dir, None).unwrap_err(), "not_found");
        assert_eq!(std::fs::read_dir(&kept_dir).unwrap().count(), 1);
        // Larger than the screen needs: kept just covering it, as a PNG.
        let small = keep_wallpaper(path.to_str().unwrap(), &kept_dir, Some((100, 30))).unwrap();
        assert_eq!((small.w, small.h), (100, 45), "200 × 90 covering 100 × 30");
        assert!(small.path.ends_with(".png"));
        let shrunk = zgx_gpu::assets::decode_image(&std::fs::read(&small.path).unwrap()).unwrap();
        assert_eq!((shrunk.width(), shrunk.height()), (100, 45));
        assert_eq!(&shrunk.pixels()[..4], &[230, 100, 25, 255], "its colours kept");
        forget_wallpaper(&small.path, &kept_dir);
        // No larger than it: kept as it is.
        assert_eq!(kept_size(200, 90, Some((400, 300))), None);
        assert_eq!(kept_size(4000, 3000, Some((3456, 2234))), Some((3456, 2592)));
        // Forgetting touches only a kept copy: the original stays.
        forget_wallpaper(path.to_str().unwrap(), &kept_dir);
        assert!(path.is_file());
        forget_wallpaper(&kept.path, &kept_dir);
        assert!(!std::path::Path::new(&kept.path).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_image_key_is_only_the_edge_s() {
        let key = "0123456789abcdef0123456789abcdef";
        assert_eq!(image_key(&format!("https://edge.zeron.sh/auth/avatars/{key}")), Some(key));
        assert_eq!(image_key(&format!("http://edge.zeron.sh/auth/avatars/{key}")), None);
        assert_eq!(image_key("https://edge.zeron.sh/auth/avatars/../me"), None);
        assert_eq!(image_key(&format!("https://edge.zeron.sh/auth/avatars/{key}x")), None);
        assert_eq!(image_key("https://example.net/icon.png"), None);
    }

    #[test]
    fn a_photo_is_the_square_asked_for_made_512_at_most() {
        let dir = std::env::temp_dir().join(format!("zgx-photo-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("wide.png");
        // 1200 × 700: left half red, right half blue.
        let (w, h) = (1200u32, 700u32);
        let mut raw = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..h {
            for x in 0..w {
                raw.extend_from_slice(if x < w / 2 { &[220, 30, 30, 255] } else { &[30, 30, 220, 255] });
            }
        }
        let mut file = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut file, w, h);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.write_header().unwrap().write_image_data(&raw).unwrap();
        }
        std::fs::write(&path, file).unwrap();
        let path = path.to_str().unwrap();
        assert_eq!(picture(path).map(|(w, h, _)| (w, h)), Ok((1200, 700)));
        // Asked past the right and top edges: moved back inside (x 500,
        // y 0), so it starts in the red and ends in the blue; made 512.
        let png = square_png(path, 900.0, -40.0, 700.0).unwrap();
        let image = zgx_gpu::assets::decode_image(&png).unwrap();
        assert_eq!((image.width(), image.height()), (512, 512));
        assert_eq!(&image.pixels()[..4], &[220, 30, 30, 255]);
        let last = image.pixels().len() - 4;
        assert_eq!(&image.pixels()[last..], &[30, 30, 220, 255]);
        // A small square stays its size.
        let small = zgx_gpu::assets::decode_image(&square_png(path, 0.0, 0.0, 100.0).unwrap()).unwrap();
        assert_eq!((small.width(), small.height()), (100, 100));
        assert_eq!(&small.pixels()[..4], &[220, 30, 30, 255]);
        assert_eq!(square_png("relative.png", 0.0, 0.0, 10.0), Err("invalid".into()));
        // Only https photos are fetched; one already kept is the file.
        assert_eq!(photo_file(&dir, "http://example.net/a.png"), Err("invalid".into()));
        assert_eq!(photo_file(&dir, "file:///etc/passwd"), Err("invalid".into()));
        let kept_url = "https://example.net/kept.png";
        let kept: String = {
            use sha2::{Digest, Sha256};
            Sha256::digest(kept_url.as_bytes()).iter().take(16).map(|b| format!("{b:02x}")).collect()
        };
        std::fs::write(dir.join(format!("{kept}.img")), b"x").unwrap();
        assert_eq!(photo_file(&dir, kept_url), Ok(dir.join(format!("{kept}.img"))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refusals_say_what_happened() {
        assert_eq!(outcome(400, &json!({ "error": "self" })), "self");
        assert_eq!(outcome(400, &json!({ "error": "invalid" })), "invalid");
        assert_eq!(outcome(403, &json!({})), "refused");
        assert_eq!(outcome(404, &json!({})), "not_found");
        assert_eq!(outcome(429, &json!({})), "rate_limited");
        assert_eq!(outcome(502, &json!({})), "failed");
    }
}
