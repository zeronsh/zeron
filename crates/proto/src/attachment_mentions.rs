//! Canonical attachment chips shared by the editor, transcript and harness
//! delivery.
//!
//! A composer attachment is referenced from the prompt by a strict local
//! Markdown link carrying only its per-draft number, never a path. Images read
//! `[Image N]`; other files carry their file name. The editor projects either
//! to a chip; providers receive the plain label, which pairs with the upload of
//! the same name.
use std::ops::Range;

pub const IMAGE_MENTION_SCHEME: &str = "zeron-image:";
pub const ATTACHMENT_MENTION_SCHEME: &str = "zeron-attachment:";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentMention {
    pub range: Range<usize>,
    pub index: u32,
    /// `Image N` for images, the file name for other attachments.
    pub label: String,
    pub is_image: bool,
}

impl AttachmentMention {
    /// Whether this chip names the attachment at `path`. Images match by
    /// draft number (`Image 2` ↔ `ab12cd34-Image_2.png`), files by name as
    /// the engine sanitizes it.
    pub fn names_attachment(&self, path: &str) -> bool {
        if self.is_image {
            return image_index_from_name(path) == Some(self.index);
        }
        let sanitized = |name: &str| -> String {
            name.chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                        c
                    } else {
                        '_'
                    }
                })
                .collect()
        };
        let name = attachment_display_name(path.rsplit(['/', '\\']).next().unwrap_or(path));
        sanitized(&self.label) == sanitized(name)
    }
}

/// The user-visible name of the `index`th image of a draft.
pub fn image_label(index: u32) -> String {
    format!("Image {index}")
}

/// The chip link for attachment `index`: an image when `file_name` is `None`.
pub fn attachment_mention_link(index: u32, file_name: Option<&str>) -> String {
    match file_name {
        None => format!("[{}]({IMAGE_MENTION_SCHEME}{index})", image_label(index)),
        Some(name) => format!(
            "[{}]({ATTACHMENT_MENTION_SCHEME}{index})",
            crate::file_mentions::escape_mention_label(name)
        ),
    }
}

/// Recover the draft number from an upload or staged image name such as
/// `Image 2.png` / `ab12cd34-Image_2.png` (uploads sanitize spaces).
pub fn image_index_from_name(name: &str) -> Option<u32> {
    let name = name.rsplit(['/', '\\']).next()?;
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    let digits_at = stem.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    let (head, digits) = stem.split_at(digits_at);
    let head = head.strip_suffix([' ', '_'])?;
    if !(head == "Image" || head.ends_with("-Image")) {
        return None;
    }
    digits.parse().ok().filter(|index| *index > 0)
}

/// Whether an attachment ref names an image, as opposed to any other file.
/// The same extensions the desktop decodes (`format_by_extension`), so every
/// client shows a given attachment the same way.
pub fn is_image_path(path: &str) -> bool {
    path.rsplit_once('.').is_some_and(|(_, extension)| {
        matches!(
            extension.to_ascii_lowercase().as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "tif" | "tiff"
        )
    })
}

/// The file name to show for an attachment ref: uploads are stored as
/// `{id8}-{name}`, and the prefix is not part of the name.
pub fn attachment_display_name(name: &str) -> &str {
    match name.split_once('-') {
        Some((id, rest)) if id.len() == 8 && id.bytes().all(|b| b.is_ascii_hexdigit()) => rest,
        _ => name,
    }
}

/// Undo [`crate::file_mentions::escape_mention_label`], rejecting any label
/// that is not exactly what escaping produces.
fn unescape_label(raw: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next()? {
                escaped @ ('\\' | '[' | ']' | '`') => out.push(escaped),
                _ => return None,
            },
            '[' | ']' | '`' => return None,
            c if c.is_control() => return None,
            c => out.push(c),
        }
    }
    (!out.is_empty()).then_some(out)
}

/// Only canonical chip links are decoded, never escaped text or code examples.
pub fn attachment_mentions(text: &str) -> Vec<AttachmentMention> {
    if !text.contains(IMAGE_MENTION_SCHEME) && !text.contains(ATTACHMENT_MENTION_SCHEME) {
        return Vec::new();
    }
    let mut image_depth = 0;
    pulldown_cmark::Parser::new(text)
        .into_offset_iter()
        .filter_map(|(event, range)| {
            match &event {
                pulldown_cmark::Event::Start(pulldown_cmark::Tag::Image { .. }) => {
                    image_depth += 1;
                }
                pulldown_cmark::Event::End(pulldown_cmark::TagEnd::Image) => {
                    image_depth -= 1;
                }
                _ => {}
            }
            if image_depth > 0 {
                return None;
            }
            let pulldown_cmark::Event::Start(pulldown_cmark::Tag::Link { dest_url, .. }) = event
            else {
                return None;
            };
            let (scheme, is_image) = if dest_url.starts_with(IMAGE_MENTION_SCHEME) {
                (IMAGE_MENTION_SCHEME, true)
            } else {
                (ATTACHMENT_MENTION_SCHEME, false)
            };
            let digits = dest_url.strip_prefix(scheme)?;
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let index: u32 = digits.parse().ok().filter(|index| *index > 0)?;
            let source = &text[range.clone()];
            let label = if is_image {
                (source == attachment_mention_link(index, None)).then(|| image_label(index))?
            } else {
                let tail = format!("]({scheme}{index})");
                unescape_label(source.strip_prefix('[')?.strip_suffix(&tail)?)?
            };
            Some(AttachmentMention {
                range,
                index,
                label,
                is_image,
            })
        })
        .collect()
}

/// The attachment numbers a draft mentions, in order of appearance, without
/// repeats.
pub fn attachment_mention_indices(text: &str) -> Vec<u32> {
    let mut seen = Vec::new();
    for mention in attachment_mentions(text) {
        if !seen.contains(&mention.index) {
            seen.push(mention.index);
        }
    }
    seen
}

/// Replace chips with their plain label. Providers read prose, and the label
/// pairs with the attached upload.
pub fn attachment_mention_prompt(text: &str) -> String {
    replace_mentions(text, |_| true)
}

/// Replace chips whose attachment is not attached by plain text, so a dangling
/// reference never leaves the composer as a link.
pub fn demote_unattached_mentions(text: &str, attached: &[u32]) -> String {
    replace_mentions(text, |index| !attached.contains(&index))
}

fn replace_mentions(text: &str, replace: impl Fn(u32) -> bool) -> String {
    let mut out = String::new();
    let mut at = 0;
    for mention in attachment_mentions(text) {
        if !replace(mention.index) {
            continue;
        }
        out.push_str(&text[at..mention.range.start]);
        out.push_str(&mention.label);
        at = mention.range.end;
    }
    out.push_str(&text[at..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(index: u32) -> String {
        attachment_mention_link(index, None)
    }

    #[test]
    fn links_round_trip_and_become_plain_labels() {
        assert_eq!(image(2), "[Image 2](zeron-image:2)");
        let notes = attachment_mention_link(3, Some("my [notes].md"));
        assert_eq!(notes, "[my \\[notes\\].md](zeron-attachment:3)");
        let text = format!("compare {} with {notes} and {}", image(2), image(10));
        let mentions = attachment_mentions(&text);
        assert_eq!(
            mentions
                .iter()
                .map(|m| (m.index, m.label.as_str(), m.is_image))
                .collect::<Vec<_>>(),
            vec![
                (2, "Image 2", true),
                (3, "my [notes].md", false),
                (10, "Image 10", true)
            ]
        );
        assert_eq!(&text[mentions[1].range.clone()], notes);
        assert_eq!(
            attachment_mention_prompt(&text),
            "compare Image 2 with my [notes].md and Image 10"
        );
        assert_eq!(
            attachment_mention_indices(&format!("{}{}", image(2), image(2))),
            vec![2]
        );
    }

    #[test]
    fn hostile_and_literal_links_stay_ordinary_text() {
        let link = image(1);
        let file = attachment_mention_link(1, Some("a.md"));
        for literal in [
            format!("`{link}`"),
            format!("```\n{link}\n```"),
            format!("\\{link}"),
            format!("![example {link}](example.png)"),
            format!("`{file}`"),
            format!("\\{file}"),
            "[Image 1](zeron-image:0)".to_string(),
            "[Image 1](zeron-image:)".to_string(),
            "[Image 1](zeron-image:1x)".to_string(),
            "[Image 1](zeron-image:-1)".to_string(),
            "[Image 1](zeron-image:../x)".to_string(),
            "[Image 2](zeron-image:1)".to_string(),
            "[Other](zeron-image:1)".to_string(),
            "[Image 1](zeron-image:99999999999)".to_string(),
            "[Image 01](zeron-image:01)".to_string(),
            "[](zeron-attachment:1)".to_string(),
            "[a.md](zeron-attachment:0)".to_string(),
            "[a.md](zeron-attachment:1 \"title\")".to_string(),
            "[a\nb.md](zeron-attachment:1)".to_string(),
            "[a`b.md](zeron-attachment:1)".to_string(),
            "[a\\xb.md](zeron-attachment:1)".to_string(),
        ] {
            assert!(attachment_mentions(&literal).is_empty(), "{literal}");
            assert_eq!(attachment_mention_prompt(&literal), literal);
        }
    }

    #[test]
    fn unattached_mentions_demote_to_labels_only() {
        let text = format!(
            "{} and {}",
            image(1),
            attachment_mention_link(2, Some("a.md"))
        );
        assert_eq!(
            demote_unattached_mentions(&text, &[2]),
            format!("Image 1 and {}", attachment_mention_link(2, Some("a.md")))
        );
    }

    #[test]
    fn image_names_recover_their_number() {
        for (name, index) in [
            ("Image 3.png", Some(3)),
            ("ab12cd34-Image_12.png", Some(12)),
            ("/uploads/ab-Image_2.webp", Some(2)),
            ("Image 0.png", None),
            ("image.png", None),
            ("Imaged 2.png", None),
            ("cat.png", None),
        ] {
            assert_eq!(image_index_from_name(name), index, "{name}");
        }
    }

    #[test]
    fn attachment_refs_split_images_from_files() {
        assert!(is_image_path("/uploads/ab12cd34-shot.PNG"));
        assert!(is_image_path("pending://u1/Image_1.jpeg"));
        assert!(!is_image_path("/uploads/ab12cd34-notes.zip"));
        assert!(!is_image_path("/uploads/png"));
        assert_eq!(attachment_display_name("ab12cd34-notes.zip"), "notes.zip");
        assert_eq!(attachment_display_name("my-notes.zip"), "my-notes.zip");
    }
}
