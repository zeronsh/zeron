//! Canonical file chips shared by the editor and harness delivery boundary.
use std::ops::Range;
pub const FILE_MENTION_SCHEME: &str = "zeron-file:";

pub struct FileMentionLink {
    pub range: Range<usize>,
    pub basename: String,
    pub path: String,
    pub is_dir: bool,
}

fn percent_encode_path(path: &str) -> String {
    let mut out = String::new();
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/') {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push_str(&format!("{byte:02X}"));
        }
    }
    out
}

fn percent_decode_path(encoded: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(encoded.len());
    let raw = encoded.as_bytes();
    let mut at = 0;
    while at < raw.len() {
        if raw[at] == b'%' {
            let hex = std::str::from_utf8(raw.get(at + 1..at + 3)?).ok()?;
            bytes.push(u8::from_str_radix(hex, 16).ok()?);
            at += 3;
        } else {
            bytes.push(raw[at]);
            at += 1;
        }
    }
    String::from_utf8(bytes).ok()
}

fn escape_mention_label(label: &str) -> String {
    label
        .replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace('`', "\\`")
}

pub fn local_file_link(path: &str, is_dir: bool) -> String {
    let path = path.trim_end_matches('/');
    let basename = path
        .rsplit('/')
        .next()
        .filter(|part| !part.is_empty())
        .unwrap_or(path);
    format!(
        "[{}]({}{})",
        escape_mention_label(basename),
        FILE_MENTION_SCHEME,
        percent_encode_path(&format!("{path}{}", if is_dir { "/" } else { "" }))
    )
}

pub fn local_path_is_safe(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && !path.chars().any(char::is_control)
        && !path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
}

/// Only decode canonical chip links, never escaped text or code examples.
pub fn file_mention_links(text: &str) -> Vec<FileMentionLink> {
    if !text.contains(FILE_MENTION_SCHEME) {
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
            let target = percent_decode_path(dest_url.strip_prefix(FILE_MENTION_SCHEME)?)?;
            let is_dir = target.ends_with('/');
            let path = target.strip_suffix('/').unwrap_or(&target);
            if !local_path_is_safe(path) {
                return None;
            }
            let canonical = local_file_link(path, is_dir);
            let source = &text[range.clone()];
            // Keep old recognizable selections after escaping new labels.
            if canonical != source && canonical.replace("\\`", "`") != source {
                return None;
            }
            Some(FileMentionLink {
                range,
                basename: path.rsplit('/').next()?.into(),
                path: path.into(),
                is_dir,
            })
        })
        .collect()
}

/// Replace our private URI with a provider-readable, workspace-relative link.
/// The durable transcript retains the original chip; only outgoing text changes.
pub fn file_mention_prompt(text: &str) -> String {
    let mut out = String::new();
    let mut at = 0;
    for link in file_mention_links(text) {
        out.push_str(&text[at..link.range.start]);
        out.push_str(&format!(
            "[{}]({}{})",
            escape_mention_label(&link.basename),
            percent_encode_path(&link.path),
            if link.is_dir { "/" } else { "" }
        ));
        at = link.range.end;
    }
    out.push_str(&text[at..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn provider_text_preserves_paths_and_literal_examples() {
        let file = local_file_link("src/a file#[x].rs", false);
        let folder = local_file_link("src/components", true);
        let prompt = format!("Check {file} and {folder}");
        assert_eq!(
            file_mention_prompt(&prompt),
            "Check [a file#\\[x\\].rs](src/a%20file%23%5Bx%5D.rs) and [components](src/components/)"
        );
        for literal in [
            format!("`{file}`"),
            format!("```\n{file}\n```"),
            format!("\\{file}"),
            format!("![example {file}](example.png)"),
            "[x](zeron-file:../x)".into(),
            "[other](zeron-file:src/x)".into(),
        ] {
            assert!(file_mention_links(&literal).is_empty());
            assert_eq!(file_mention_prompt(&literal), literal);
        }
        let image_then_file = format!("![example {file}](example.png) then {file}");
        let links = file_mention_links(&image_then_file);
        assert_eq!(links.len(), 1);
        assert_eq!(&image_then_file[links[0].range.clone()], file);
        for name in ["src/é.rs", "src/](zeron-file:x)", "src/what?.rs"] {
            let raw = local_file_link(name, false);
            assert_eq!(file_mention_links(&raw)[0].path, name);
            assert!(!file_mention_prompt(&raw).contains("](zeron-file:src/"));
        }
    }
}
