//! Images in agent replies. An image part (a harness's generated image, or
//! one an agent showed with `show_image`) and a local image a reply names in
//! Markdown (`![alt](path)`) share one look: the image row.
//!
//! A Markdown image is drawn in place, never copied, and only when it is a
//! path on the chat's host — absolute, or relative to the chat's folder — of
//! a PNG, JPEG, WebP, or GIF by name. No URL is ever fetched (`http:`,
//! `file:`, `data:` …), and `~`, network and device paths are refused. The
//! host reads it with `ReadChatImageChunk`, from strictly inside that one
//! chat's folder (never home or above it, Zeron's data, or another chat's
//! folder) and never through a link, checks its first bytes, and the UI
//! decodes it within the shared raster limits. Anything refused or
//! unreadable keeps the reply's ordinary rendering — a link — as before.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::{SharedString, div, prelude::*, px};

use crate::attachments::{
    AttachmentKey, AttachmentSnapshot, CachedAttachmentImage, attachment_snapshot_for,
};
use crate::theme::Theme;

/// Reply image reads in flight at once, per transcript.
pub(crate) const MAX_LOADS: usize = 4;

/// The host path and type of an image a reply names, when it may be drawn
/// (see the module docs). `cwd` is the chat's folder on its host.
pub(crate) fn markdown_image(source: &str, cwd: Option<&str>) -> Option<(String, &'static str)> {
    let source = source.trim();
    if source.is_empty()
        || source.starts_with('~')
        || has_scheme(source)
        || crate::tool_images::is_remote_or_device_path(source)
    {
        return None;
    }
    let mime = raster_mime(source)?;
    let path = crate::tool_images::resolve_path(source, cwd);
    // A relative path needs a folder to resolve against; a home-relative
    // folder (a projectless chat's) can't be expanded here.
    crate::tool_images::is_host_absolute(&path).then_some((path, mime))
}

/// `http:`, `file:`, `data:`, `javascript:`…; a drive letter (`C:\`) is not one.
fn has_scheme(source: &str) -> bool {
    source.split_once(':').is_some_and(|(scheme, _)| {
        scheme.len() > 1
            && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    })
}

fn raster_mime(path: &str) -> Option<&'static str> {
    let (_, ext) = path.rsplit_once('.')?;
    match ext.to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "webp" => Some("image/webp"),
        "gif" => Some("image/gif"),
        _ => None,
    }
}

/// The open chat, as the Markdown image closure (which has no `cx`) needs it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ReplyChat {
    pub(crate) id: String,
    /// The chat's folder on its host, for relative paths.
    pub(crate) cwd: Option<String>,
    /// The device hosting the chat: the only one asked for its images.
    pub(crate) host: String,
}

/// One image to read: its cache key and what the read needs.
#[derive(Clone, Debug)]
pub(crate) struct ReplyImageRequest {
    pub(crate) key: AttachmentKey,
    pub(crate) chat_id: String,
    pub(crate) host: String,
    pub(crate) path: String,
    pub(crate) mime: &'static str,
}

/// How a reply draws one of the images it names.
pub(crate) enum ReplyImage {
    /// Drawn; the request says where it came from, for the image menu.
    Loaded(CachedAttachmentImage, Box<ReplyImageRequest>),
    /// The first read is under way: the frame holds the image's place.
    Loading,
}

/// What the transcript and its Markdown image closure share: the open chat,
/// the reads asked for since the transcript last started some, the rows each
/// image shows in (remeasured when it lands), and the images that failed.
#[derive(Default)]
pub(crate) struct ReplyImages {
    chat: Option<ReplyChat>,
    wanted: Vec<ReplyImageRequest>,
    rows: HashMap<AttachmentKey, HashSet<SharedString>>,
    /// Keyed by chat, so they outlive a switch to another one.
    failed: HashSet<AttachmentKey>,
}

impl ReplyImages {
    /// Follow the chat the transcript shows (`None`: draw no images). Another
    /// chat's pending reads and rows are dropped.
    pub(crate) fn show(&mut self, chat: Option<ReplyChat>) {
        if self.chat != chat {
            self.wanted.clear();
            self.rows.clear();
            self.chat = chat;
        }
    }

    /// How `![…](source)` in `row` draws in the open chat, asking for a read
    /// unless it's loaded. `None` keeps the ordinary rendering: so does a
    /// failed image while its retries (on the shared backoff) run behind it.
    pub(crate) fn lookup(&mut self, source: &str, row: SharedString) -> Option<ReplyImage> {
        let chat = self.chat.as_ref()?;
        let (path, mime) = markdown_image(source, chat.cwd.as_deref())?;
        let key = AttachmentKey::chat_image(&chat.host, &chat.id, &path, mime);
        self.rows.entry(key.clone()).or_default().insert(row);
        let snapshot = attachment_snapshot_for(&key);
        let first_read =
            matches!(snapshot, AttachmentSnapshot::Loading) && !self.failed.contains(&key);
        let request = ReplyImageRequest {
            key,
            chat_id: chat.id.clone(),
            host: chat.host.clone(),
            path,
            mime,
        };
        if let AttachmentSnapshot::Loaded(loaded) = snapshot {
            return Some(ReplyImage::Loaded(loaded, Box::new(request)));
        }
        self.wanted.push(request);
        first_read.then_some(ReplyImage::Loading)
    }

    pub(crate) fn take_wanted(&mut self) -> Vec<ReplyImageRequest> {
        std::mem::take(&mut self.wanted)
    }

    /// A read ended; the rows showing the image, to remeasure.
    pub(crate) fn finish(&mut self, key: &AttachmentKey, loaded: bool) -> Vec<SharedString> {
        if loaded {
            self.failed.remove(key);
        } else {
            self.failed.insert(key.clone());
        }
        self.rows
            .get(key)
            .map(|rows| rows.iter().cloned().collect())
            .unwrap_or_default()
    }
}

/// The image row's frame, before its image loads (or when it can't): at most
/// 512×420 and the column's width, rounded, outlined by the hairline the tool
/// image strip uses — a screenshot of a dark app would otherwise melt into
/// the transcript.
pub(crate) fn frame(id: SharedString) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .w(px(512.0))
        .max_w_full()
        .h(px(320.0))
        .max_h(px(420.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(12.0))
        .overflow_hidden()
        .border_1()
        .border_color(crate::theme::hairline(0.07))
        .bg(crate::theme::ink(0.045))
}

/// `frame` sized to a decoded image, plus its hairline on each side (so the
/// image fills it instead of letterboxing — a wide strip would lose most), as
/// a button: the caller says what a click opens. A column narrower than the
/// image scales the frame down whole, keeping its aspect ratio.
pub(crate) fn sized(
    frame: gpui::Stateful<gpui::Div>,
    image: &Arc<gpui::Image>,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    let (width, height) = crate::appshots::png_dimensions(&image.bytes).unwrap_or((512, 320));
    let scale = (512.0 / width as f32).min(420.0 / height as f32).min(1.0);
    let accent = theme.accent;
    let (width, height) = (width as f32 * scale + 2.0, height as f32 * scale + 2.0);
    frame
        .w(px(width))
        .h_auto()
        .max_h_auto()
        .aspect_ratio(width / height)
        .role(gpui::Role::Button)
        .aria_label("Preview image")
        .tab_index(0)
        .cursor_pointer()
        .focus_visible(move |style| style.border_2().border_color(accent))
        .child(
            gpui::img(image.clone())
                .size_full()
                // The frame's overflow clip is rectangular; round the image
                // itself, just inside the frame's hairline.
                .rounded(px(11.0))
                .object_fit(gpui::ObjectFit::Contain),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_local_raster_paths_in_the_chat_are_drawn() {
        let cwd = Some("/work/app");
        let image = |source: &str| markdown_image(source, cwd);
        assert_eq!(
            image("/tmp/shots/before.png"),
            Some(("/tmp/shots/before.png".into(), "image/png"))
        );
        assert_eq!(
            image("shots/after.JPG"),
            Some(("/work/app/shots/after.JPG".into(), "image/jpeg"))
        );
        assert_eq!(
            image("./chart.webp"),
            Some(("/work/app/chart.webp".into(), "image/webp"))
        );
        assert_eq!(
            markdown_image(r"shots\a.gif", Some(r"C:\work\app")),
            Some((r"C:\work\app\shots\a.gif".into(), "image/gif"))
        );
        assert_eq!(
            markdown_image(r"C:\work\b.png", None),
            Some((r"C:\work\b.png".into(), "image/png"))
        );
        for source in [
            "",
            "https://example.com/a.png",
            "http://127.0.0.1/a.png",
            "file:///tmp/a.png",
            "data:image/png;base64,iVBORw0KGgo=",
            "javascript:alert(1).png",
            "~/Pictures/a.png",
            r"\\host\share\a.png",
            "//host/share/a.png",
            "C:a.png",
            "/tmp/logo.svg",
            "/tmp/scan.tiff",
            "/tmp/notes.txt",
            "/tmp/no-extension",
        ] {
            assert_eq!(image(source), None, "{source:?}");
        }
        // Relative paths need the chat's folder, and one the UI can expand.
        assert_eq!(markdown_image("a.png", None), None);
        assert_eq!(markdown_image("a.png", Some("~/.zeron/scratch/x")), None);
    }

    #[test]
    fn lookups_follow_the_open_chat_and_key_by_it() {
        let chat = |id: &str| ReplyChat {
            id: id.into(),
            cwd: Some("/reply-images-test".into()),
            host: "host".into(),
        };
        let mut images = ReplyImages::default();
        assert!(
            images.lookup("a.png", "row".into()).is_none(),
            "no chat open"
        );
        images.show(Some(chat("one")));
        assert!(matches!(
            images.lookup("a.png", "row-1".into()),
            Some(ReplyImage::Loading)
        ));
        assert!(images.lookup("https://x/a.png", "row-1".into()).is_none());
        let [one] = &images.take_wanted()[..] else {
            panic!("one read asked for");
        };
        assert_eq!(one.path, "/reply-images-test/a.png");
        assert_eq!((one.chat_id.as_str(), one.host.as_str()), ("one", "host"));
        // A failed image keeps its link, even while a retry is under way.
        assert_eq!(
            images.finish(&one.key, false),
            vec![SharedString::from("row-1")]
        );
        assert!(images.lookup("a.png", "row-1".into()).is_none());
        assert_eq!(images.take_wanted().len(), 1, "retries are still asked for");
        // The same path in another chat is another image.
        images.show(Some(chat("two")));
        assert!(images.finish(&one.key, false).is_empty());
        assert!(images.lookup("a.png", "row-2".into()).is_some());
        let [two] = &images.take_wanted()[..] else {
            panic!("one read asked for");
        };
        assert_ne!(one.key, two.key);
    }
}
