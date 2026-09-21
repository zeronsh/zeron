//! Transcript annotations: quotes staged from agent text, folded into the
//! next prompt, and addressed by the agent with an inline marker.
//!
//! [`with_annotations`] prepends the block the agent reads; [`extract_badge`]
//! lifts it back out so the transcript draws a pill instead of the preamble.
//! [`rewrite_markers`] turns `:zeron-annotation{index="N"}` into the visible
//! "Annotation N" label.

use std::ops::Range;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use crate::markdown::selection::Span;

pub const ANNOTATION_HEADER: &str = "# Response annotations:";
pub const ANNOTATION_INSTRUCTIONS: &str = "Each item contains text selected from an earlier response and may include a user comment. Treat items as Annotation 1, Annotation 2, and so on in array order. Use every selection as context and address every comment. For every annotation you address, include its inline directive `:zeron-annotation{index=\"N\"}`, where N is its one-based array position (for example, `:zeron-annotation{index=\"1\"}`). Do not use unstructured annotation labels.";
pub const ANNOTATIONS_OPEN: &str = "<response-annotations>";
pub const ANNOTATIONS_CLOSE: &str = "</response-annotations>";
pub const REQUEST_HEADER: &str = "## My request:";
pub const MARKER_URL_PREFIX: &str = "zeron-annotation:";

const ZERON_MARKER_HEAD: &str = ":zeron-annotation{index=\"";
const CODEX_MARKER_HEAD: &str = ":codex-annotation{index=\"";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptAnnotation {
    pub id: String,
    pub message_id: String,
    pub text: String,
    pub comment: String,
    pub start_offset: usize,
    pub end_offset: usize,
    pub spans: Vec<Span>,
}

impl TranscriptAnnotation {
    pub fn from_selection(message_id: impl Into<String>, spans: Vec<Span>, text: String) -> Self {
        let start_offset = spans.first().map(|span| span.range.start).unwrap_or(0);
        let end_offset = spans
            .last()
            .map(|span| span.range.end)
            .unwrap_or(start_offset);
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            message_id: message_id.into(),
            text,
            comment: String::new(),
            start_offset,
            end_offset,
            spans,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AnnotationPayload {
    text: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    annotation: String,
    source: AnnotationSource,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AnnotationSource {
    message_id: String,
    start_offset: usize,
    end_offset: usize,
    #[serde(default)]
    spans: Vec<Span>,
}

pub fn with_annotations(text: &str, annotations: &[TranscriptAnnotation]) -> String {
    if annotations.is_empty() {
        return text.to_string();
    }
    let payload: Vec<AnnotationPayload> = annotations
        .iter()
        .map(|annotation| AnnotationPayload {
            text: annotation.text.clone(),
            annotation: annotation.comment.trim().to_string(),
            source: AnnotationSource {
                message_id: annotation.message_id.clone(),
                start_offset: annotation.start_offset,
                end_offset: annotation.end_offset,
                spans: annotation.spans.clone(),
            },
        })
        .collect();
    let json = serde_json::to_string(&payload).unwrap_or_else(|_| "[]".into());
    format!(
        "\n{ANNOTATION_HEADER}\n{ANNOTATION_INSTRUCTIONS}\n{ANNOTATIONS_OPEN}\n{json}\n{ANNOTATIONS_CLOSE}\n\n{REQUEST_HEADER}\n{text}"
    )
}

/// [`crate::badges::Extractor`] for the annotation preamble. The visible
/// prompt is whatever follows `## My request:`; an empty request (annotation-
/// only send) leaves no bubble.
pub fn extract_badge(text: &str) -> Option<(String, crate::badges::MessageBadge)> {
    let (prefix, request, payload) = parse_annotation_block(text)?;
    let visible = if prefix.is_empty() {
        request
    } else if request.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix}\n{request}")
    };
    Some((
        visible,
        crate::badges::MessageBadge {
            icon: crate::icons::CHAT_ROUND_LINE,
            label: chip_label(payload.len()).into(),
            details: payload
                .iter()
                .enumerate()
                .map(|(ix, item)| crate::badges::BadgeDetail {
                    location: format!("{}. Selected text", ix + 1).into(),
                    tag: None,
                    body: item.text.clone().into(),
                })
                .collect(),
        },
    ))
}

pub fn annotations_from_text(text: &str) -> Option<Vec<TranscriptAnnotation>> {
    let (_, _, payload) = parse_annotation_block(text)?;
    Some(
        payload
            .into_iter()
            .map(|item| TranscriptAnnotation {
                id: uuid::Uuid::new_v4().to_string(),
                message_id: item.source.message_id.clone(),
                text: item.text,
                comment: item.annotation,
                start_offset: item.source.start_offset,
                end_offset: item.source.end_offset,
                spans: item.source.spans,
            })
            .collect(),
    )
}

fn parse_annotation_block(text: &str) -> Option<(&str, String, Vec<AnnotationPayload>)> {
    let at = if text.starts_with(ANNOTATION_HEADER) {
        0
    } else {
        text.find(&format!("\n{ANNOTATION_HEADER}"))?
    };
    let after_header = if at == 0 { 0 } else { at + 1 };
    let request_marker = format!("\n{REQUEST_HEADER}\n");
    let request_rel = text[after_header..].find(&request_marker)?;
    let request_at = after_header + request_rel;
    let block = &text[after_header..request_at];
    let json = block
        .split_once(ANNOTATIONS_OPEN)?
        .1
        .rsplit_once(ANNOTATIONS_CLOSE)?
        .0
        .trim();
    let payload: Vec<AnnotationPayload> = serde_json::from_str(json).ok()?;
    (!payload.is_empty()).then(|| {
        (
            text[..at].trim_end(),
            text[request_at + request_marker.len()..]
                .trim_start_matches('\n')
                .to_string(),
            payload,
        )
    })
}

pub fn chip_label(count: usize) -> String {
    if count == 1 {
        "1 annotation".to_string()
    } else {
        format!("{count} annotations")
    }
}

pub fn badge_details(annotations: &[TranscriptAnnotation]) -> Vec<crate::badges::BadgeDetail> {
    annotations
        .iter()
        .enumerate()
        .map(|(ix, annotation)| crate::badges::BadgeDetail {
            location: format!("{}. Selected text", ix + 1).into(),
            tag: None,
            body: annotation.text.clone().into(),
        })
        .collect()
}

/// One display fragment after rewriting annotation markers inside a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarkerPiece<'a> {
    Text(&'a str),
    Marker { index: usize },
}

/// Split `text` into ordinary copy and `:zeron-annotation{index="N"}` markers
/// (Codex's identical directive is accepted when reading a reply).
pub fn split_markers(text: &str) -> Vec<MarkerPiece<'_>> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some((at, index, consumed)) = next_marker(rest) {
        if at > 0 {
            out.push(MarkerPiece::Text(&rest[..at]));
        }
        out.push(MarkerPiece::Marker { index });
        rest = &rest[at + consumed..];
    }
    if !rest.is_empty() {
        out.push(MarkerPiece::Text(rest));
    }
    out
}

fn next_marker(text: &str) -> Option<(usize, usize, usize)> {
    let zeron = text
        .find(ZERON_MARKER_HEAD)
        .map(|at| (at, ZERON_MARKER_HEAD));
    let codex = text
        .find(CODEX_MARKER_HEAD)
        .map(|at| (at, CODEX_MARKER_HEAD));
    let (at, head) = match (zeron, codex) {
        (Some(zeron), Some(codex)) if zeron.0 <= codex.0 => zeron,
        (Some(_), Some(codex)) => codex,
        (Some(zeron), None) => zeron,
        (None, Some(codex)) => codex,
        (None, None) => return None,
    };
    let digits = &text[at + head.len()..];
    let end = digits.find('"')?;
    let index: usize = digits[..end].parse().ok()?;
    if !digits[end..].starts_with("\"}") {
        return None;
    }
    Some((at, index, head.len() + end + 2))
}

pub fn marker_url(index: usize) -> String {
    format!("{MARKER_URL_PREFIX}{index}")
}

pub fn parse_marker_url(url: &str) -> Option<usize> {
    url.strip_prefix(MARKER_URL_PREFIX)?.parse().ok()
}

pub fn marker_label(index: usize) -> String {
    format!("Annotation {index}")
}

/// Paint-time wash for staged (not yet sent) quotes. Independent of the live
/// markdown selection so Add to chat can clear the drag without losing the
/// highlight.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnnotationWash {
    pub key: String,
    pub range: Range<usize>,
}

fn staged_washes() -> &'static Mutex<Vec<AnnotationWash>> {
    static STATE: OnceLock<Mutex<Vec<AnnotationWash>>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(Vec::new()))
}

pub fn set_staged_washes(washes: Vec<AnnotationWash>) {
    *staged_washes().lock().unwrap() = washes;
}

pub fn sync_staged_washes(annotations: &[TranscriptAnnotation]) {
    set_staged_washes(
        annotations
            .iter()
            .flat_map(|annotation| {
                annotation
                    .spans
                    .iter()
                    .filter(|span| !span.range.is_empty())
                    .map(|span| AnnotationWash {
                        key: span.key.clone(),
                        range: span.range.clone(),
                    })
            })
            .collect(),
    );
}

pub fn staged_wash_ranges(key: &str) -> Vec<Range<usize>> {
    staged_washes()
        .lock()
        .unwrap()
        .iter()
        .filter(|wash| wash.key == key)
        .map(|wash| wash.range.clone())
        .collect()
}

/// Keys belonging to a selectable agent reply (`{messageId}#{part}:{element}`).
pub fn agent_message_id(key: &str) -> Option<&str> {
    if key.starts_with("md-preview-") {
        return None;
    }
    let (id, rest) = key.split_once('#')?;
    if id.is_empty() || rest.is_empty() {
        None
    } else {
        Some(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quote(text: &str) -> TranscriptAnnotation {
        TranscriptAnnotation {
            id: "a1".into(),
            message_id: "msg-1".into(),
            text: text.into(),
            comment: String::new(),
            start_offset: 0,
            end_offset: text.len(),
            spans: Vec::new(),
        }
    }

    #[test]
    fn empty_set_leaves_the_prompt_untouched() {
        assert_eq!(with_annotations("ship it", &[]), "ship it");
    }

    #[test]
    fn annotations_prepend_the_request() {
        let mut staged = quote("Sun in just 225.");
        staged.comment = "How many hours is that?".into();
        let out = with_annotations("go", &[staged]);
        assert!(out.contains(ANNOTATION_HEADER));
        assert!(out.contains(":zeron-annotation{index=\"N\"}"));
        assert!(out.contains("\"text\":\"Sun in just 225.\""));
        assert!(out.contains("\"annotation\":\"How many hours is that?\""));
        assert!(out.contains("\"messageId\":\"msg-1\""));
        assert!(out.ends_with("go"));
        let (text, badge) = extract_badge(&out).unwrap();
        assert_eq!(text, "go");
        assert_eq!(badge.label.as_ref(), "1 annotation");
        assert_eq!(badge.details[0].body.as_ref(), "Sun in just 225.");
    }

    #[test]
    fn persisted_annotations_keep_selection_spans() {
        let staged = TranscriptAnnotation::from_selection(
            "msg-1",
            vec![Span {
                key: "msg-1#text.0:0".into(),
                range: 2..7,
                text: "hello world".into(),
            }],
            "llo w".into(),
        );
        let restored = annotations_from_text(&with_annotations("go", &[staged])).unwrap();
        assert_eq!(restored[0].message_id, "msg-1");
        assert_eq!(restored[0].spans[0].range, 2..7);
        assert_eq!(restored[0].spans[0].key, "msg-1#text.0:0");
    }

    #[test]
    fn blank_comments_omit_the_annotation_field() {
        let out = with_annotations("", &[quote("hello")]);
        assert!(!out.contains("\"annotation\""));
        let (text, badge) = extract_badge(&out).unwrap();
        assert_eq!(text, "");
        assert_eq!(badge.label.as_ref(), "1 annotation");
    }

    #[test]
    fn annotation_only_send_has_no_bubble_text() {
        let staged = vec![quote("one"), quote("two")];
        let (text, badge) = extract_badge(&with_annotations("", &staged)).unwrap();
        assert_eq!(text, "");
        assert_eq!(badge.label.as_ref(), "2 annotations");
        assert_eq!(badge.details.len(), 2);
    }

    #[test]
    fn a_body_quoting_the_header_is_left_alone() {
        let text = "see # Response annotations: in the docs";
        assert!(extract_badge(text).is_none());
    }

    #[test]
    fn split_markers_rewrites_zeron_and_codex_directives() {
        let pieces = split_markers("225 days. :zeron-annotation{index=\"1\"} done");
        assert_eq!(
            pieces,
            vec![
                MarkerPiece::Text("225 days. "),
                MarkerPiece::Marker { index: 1 },
                MarkerPiece::Text(" done"),
            ]
        );
        assert_eq!(
            split_markers(":codex-annotation{index=\"2\"}"),
            vec![MarkerPiece::Marker { index: 2 }]
        );
        assert_eq!(split_markers("plain"), vec![MarkerPiece::Text("plain")]);
    }

    #[test]
    fn marker_urls_round_trip() {
        assert_eq!(parse_marker_url(&marker_url(3)), Some(3));
        assert_eq!(parse_marker_url("https://example"), None);
        assert_eq!(marker_label(1), "Annotation 1");
    }

    #[test]
    fn agent_keys_expose_the_message_id() {
        assert_eq!(agent_message_id("abc#text.0:2"), Some("abc"));
        assert_eq!(agent_message_id("abc:u"), None);
        assert_eq!(agent_message_id("md-preview-1|x:0"), None);
    }

    #[test]
    fn chip_label_pluralizes() {
        assert_eq!(chip_label(1), "1 annotation");
        assert_eq!(chip_label(2), "2 annotations");
    }
}
