//! Platform-neutral, line-granular checkout diffs for native mobile viewers.
//! Uses the same host capture and unified patch conventions as desktop.

use super::{CoreClient, CoreResult};
use crate::layout::display::ColorRole;

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ChangeLineKind {
    Context,
    Addition,
    Deletion,
    Hunk,
    Notice,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ChangeHighlight {
    pub start_utf16: u32,
    pub len_utf16: u32,
    pub color: ColorRole,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ChangeLine {
    pub kind: ChangeLineKind,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    pub text: String,
    pub highlights: Vec<ChangeHighlight>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ChangedFile {
    pub path: String,
    pub old_path: Option<String>,
    pub status: String,
    pub additions: u32,
    pub deletions: u32,
    pub binary: bool,
    pub lines: Vec<ChangeLine>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct CheckoutChanges {
    pub checksum: String,
    pub additions: u32,
    pub deletions: u32,
    pub truncated: bool,
    pub files: Vec<ChangedFile>,
}

#[uniffi::export]
impl CoreClient {
    /// Read a working-tree diff on the chat's host. Parsing/highlighting runs
    /// on the core runtime, never on the platform's UI thread.
    pub async fn checkout_changes(
        &self,
        chat_id: String,
        known_checksum: Option<String>,
    ) -> CoreResult<Option<CheckoutChanges>> {
        let client = self.client.clone();
        super::on_runtime(async move {
            let diff = client.checkout_changes(&chat_id).await?;
            if known_checksum.as_deref() == Some(&diff.checksum) {
                return Ok(None);
            }
            tokio::task::spawn_blocking(move || Some(parse_changes(diff)))
                .await
                .map_err(|e| zeron_client::ClientError::HostError(e.to_string()))
        })
        .await
    }
}

fn line(
    kind: ChangeLineKind,
    old_line: Option<u32>,
    new_line: Option<u32>,
    text: &str,
) -> ChangeLine {
    ChangeLine {
        kind,
        old_line,
        new_line,
        text: text.to_owned(),
        highlights: Vec::new(),
    }
}

/// Git quotes paths as UTF-8 bytes with C escapes (including octal).
fn path(raw: &str) -> String {
    let mut bytes = Vec::new();
    if raw.starts_with('"') && raw.ends_with('"') {
        let mut chars = raw.as_bytes()[1..raw.len() - 1].iter().copied().peekable();
        while let Some(c) = chars.next() {
            if c != b'\\' {
                bytes.push(c);
                continue;
            }
            match chars.next() {
                Some(c @ b'0'..=b'7') => {
                    let mut n = (c - b'0') as u16;
                    for _ in 0..2 {
                        if let Some(c @ b'0'..=b'7') = chars.peek().copied() {
                            chars.next();
                            n = n * 8 + (c - b'0') as u16;
                        } else {
                            break;
                        }
                    }
                    bytes.push(n as u8);
                }
                Some(b't') => bytes.push(b'\t'),
                Some(b'n') => bytes.push(b'\n'),
                Some(b'r') => bytes.push(b'\r'),
                Some(c) => bytes.push(c),
                None => {}
            }
        }
    } else {
        bytes.extend_from_slice(raw.as_bytes());
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn git_path(raw: &str) -> String {
    let p = path(raw);
    p.strip_prefix("a/")
        .or_else(|| p.strip_prefix("b/"))
        .unwrap_or(&p)
        .to_owned()
}

fn parse_changes(diff: zeron_proto::CheckoutDiff) -> CheckoutChanges {
    use ChangeLineKind::*;
    let mut files: Vec<ChangedFile> = Vec::new();
    let (mut old, mut new) = (0u32, 0u32);
    let mut in_hunk = false;
    for raw in diff.patch.lines() {
        if let Some(rest) = raw.strip_prefix("diff --git ") {
            let (a, b) = rest
                .rfind(" b/")
                .or_else(|| rest.rfind(" \"b/"))
                .map(|p| (&rest[..p], &rest[p + 1..]))
                .unwrap_or((rest, rest));
            let (a, b) = (git_path(a), git_path(b));
            files.push(ChangedFile {
                old_path: (a != b).then_some(a),
                path: b,
                status: "modified".into(),
                additions: 0,
                deletions: 0,
                binary: false,
                lines: Vec::new(),
            });
            in_hunk = false;
            continue;
        }
        let Some(file) = files.last_mut() else {
            continue;
        };
        if raw.starts_with("@@ ") {
            let mut parts = raw.split_whitespace();
            parts.next();
            let starts = parts
                .next()
                .and_then(|s| s.strip_prefix('-'))
                .and_then(|s| s.split(',').next())
                .and_then(|s| s.parse().ok())
                .zip(
                    parts
                        .next()
                        .and_then(|s| s.strip_prefix('+'))
                        .and_then(|s| s.split(',').next())
                        .and_then(|s| s.parse().ok()),
                );
            in_hunk = starts.is_some();
            if let Some((o, n)) = starts {
                old = o;
                new = n;
                file.lines.push(line(Hunk, None, None, raw));
            }
            continue;
        }
        if in_hunk {
            let l = match raw.as_bytes().first() {
                Some(b'+') => {
                    file.additions += 1;
                    let l = line(Addition, None, Some(new), &raw[1..]);
                    new = new.saturating_add(1);
                    Some(l)
                }
                Some(b'-') => {
                    file.deletions += 1;
                    let l = line(Deletion, Some(old), None, &raw[1..]);
                    old = old.saturating_add(1);
                    Some(l)
                }
                Some(b' ') => {
                    let l = line(Context, Some(old), Some(new), &raw[1..]);
                    old = old.saturating_add(1);
                    new = new.saturating_add(1);
                    Some(l)
                }
                Some(b'\\') => Some(line(Notice, None, None, raw)),
                _ => None,
            };
            if let Some(l) = l {
                file.lines.push(l);
                continue;
            }
            in_hunk = false;
        }
        if raw.starts_with("new file mode") {
            file.status = "added".into();
        } else if raw.starts_with("deleted file mode") {
            file.status = "deleted".into();
        } else if let Some(p) = raw.strip_prefix("rename from ") {
            file.old_path = Some(path(p));
            file.status = "renamed".into();
        } else if let Some(p) = raw.strip_prefix("rename to ") {
            file.path = path(p);
            file.status = "renamed".into();
        } else if raw.starts_with("Binary files") || raw == "GIT binary patch" {
            file.binary = true;
        } else if let Some(p) = raw.strip_prefix("+++ ") {
            if p == "/dev/null" {
                file.status = "deleted".into();
            } else {
                file.path = git_path(p);
            }
        } else if let Some(mode) = raw.strip_prefix("new mode ") {
            file.lines.push(line(
                Notice,
                None,
                None,
                &format!("File mode changed to {mode}"),
            ));
        }
    }
    // The host's complete summary survives its patch byte cap. Include files
    // omitted from that patch rather than claiming they have no changes.
    let mut by_path: std::collections::HashMap<String, usize> = files
        .iter()
        .enumerate()
        .map(|(i, f)| (f.path.clone(), i))
        .collect();
    for summary in &diff.files {
        if let Some(index) = by_path.get(&summary.path) {
            let file = &mut files[*index];
            file.status = summary.status.clone();
            file.additions = summary.additions;
            file.deletions = summary.deletions;
            file.binary |= summary.binary;
            if summary.old_path.is_some() {
                file.old_path = summary.old_path.clone();
            }
        } else {
            by_path.insert(summary.path.clone(), files.len());
            files.push(ChangedFile {
                path: summary.path.clone(),
                old_path: summary.old_path.clone(),
                status: summary.status.clone(),
                additions: summary.additions,
                deletions: summary.deletions,
                binary: summary.binary,
                lines: Vec::new(),
            });
        }
    }
    for file in &mut files {
        if file.binary {
            file.lines
                .push(line(Notice, None, None, "Binary file changed"));
        } else if file.lines.is_empty() {
            let text = if diff.truncated {
                "Diff unavailable in this partial snapshot"
            } else if file.old_path.is_some() {
                "File renamed without content changes"
            } else {
                "No textual diff"
            };
            file.lines.push(line(Notice, None, None, text));
        }
        highlight(file);
    }
    CheckoutChanges {
        checksum: diff.checksum,
        additions: diff.additions,
        deletions: diff.deletions,
        truncated: diff.truncated,
        files,
    }
}

fn highlight(file: &mut ChangedFile) {
    // Parse each contiguous side of each hunk together, so strings and
    // comments retain context across lines. Bound source just like desktop.
    let mut start = 0;
    while start < file.lines.len() {
        if matches!(
            file.lines[start].kind,
            ChangeLineKind::Hunk | ChangeLineKind::Notice
        ) {
            start += 1;
            continue;
        }
        let end = (start..file.lines.len())
            .find(|i| {
                matches!(
                    file.lines[*i].kind,
                    ChangeLineKind::Hunk | ChangeLineKind::Notice
                )
            })
            .unwrap_or(file.lines.len());
        for old in [true, false] {
            let indices: Vec<usize> = (start..end)
                .filter(|i| {
                    if old {
                        file.lines[*i].old_line.is_some()
                    } else {
                        file.lines[*i].new_line.is_some()
                    }
                })
                .collect();
            let source = indices
                .iter()
                .map(|i| file.lines[*i].text.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            if source.len() > 128 * 1024 {
                continue;
            }
            if let Ok(doc) = zeron_syntax::highlight(zeron_syntax::HighlightRequest {
                source: &source,
                path: Some(&file.path),
                fence_tag: None,
            }) {
                for (index, spans) in indices.iter().zip(doc.lines.iter()) {
                    let l = &mut file.lines[*index];
                    if old && l.new_line.is_some() {
                        continue;
                    }
                    l.highlights = spans
                        .iter()
                        .filter_map(|s| {
                            let prefix = l.text.get(..s.range.start)?;
                            let token = l.text.get(s.range.clone())?;
                            Some(ChangeHighlight {
                                start_utf16: prefix.encode_utf16().count() as u32,
                                len_utf16: token.encode_utf16().count() as u32,
                                color: crate::layout::markdown::syntax_color(s.kind),
                            })
                        })
                        .collect();
                }
            }
        }
        start = end;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse(patch: &str) -> CheckoutChanges {
        parse_changes(serde_json::from_value(serde_json::json!({ "checkoutId":"test", "deviceId":"host", "cwd":"/fixture", "patch":patch, "files":[], "additions":1,"deletions":1,"truncated":false,"checksum":"abc","updatedAt":"2026-11-08T00:00:00Z" })).unwrap())
    }
    #[test]
    fn line_numbers_and_code_starting_with_patch_markers() {
        let d = parse(
            "diff --git a/a.swift b/a.swift\n--- a/a.swift\n+++ b/a.swift\n@@ -8,2 +12,2 @@\n same\n---old\n+++new\n\\ No newline at end of file\n",
        );
        let l = &d.files[0].lines;
        assert_eq!((l[1].old_line, l[1].new_line), (Some(8), Some(12)));
        assert_eq!(
            (l[2].kind, l[2].old_line, l[2].text.as_str()),
            (ChangeLineKind::Deletion, Some(9), "--old")
        );
        assert_eq!(
            (l[3].kind, l[3].new_line, l[3].text.as_str()),
            (ChangeLineKind::Addition, Some(13), "++new")
        );
        assert_eq!(l[4].kind, ChangeLineKind::Notice);
    }
    #[test]
    fn rename_unicode_binary_and_multiple_files() {
        let d = parse(
            "diff --git \"a/old name.swift\" \"b/new name.swift\"\nsimilarity index 100%\nrename from old name.swift\nrename to new name.swift\ndiff --git \"a/\\303\\251.png\" \"b/\\303\\251.png\"\nBinary files differ\n",
        );
        assert_eq!(d.files[0].old_path.as_deref(), Some("old name.swift"));
        assert_eq!(d.files[0].path, "new name.swift");
        assert_eq!(d.files[1].path, "é.png");
        assert!(d.files[1].binary);
    }
    #[test]
    fn empty_added_deleted_and_partial_patch() {
        assert!(parse("").files.is_empty());
        let d = parse(
            "diff --git a/a b/a\nnew file mode 100644\n@@ -0,0 +1 @@\n+hello\ndiff --git a/b b/b\ndeleted file mode 100644\n@@ -1 +0,0 @@\n-bye",
        );
        assert_eq!(d.files[0].status, "added");
        assert_eq!(d.files[1].status, "deleted");
        assert_eq!(d.files[0].lines[1].new_line, Some(1));
        assert_eq!(d.files[1].lines[1].old_line, Some(1));
        let mut snapshot: zeron_proto::CheckoutDiff = serde_json::from_value(serde_json::json!({"checkoutId":"t","deviceId":"h","cwd":"/fixture","patch":"","files":[{"path":"missing.swift","status":"modified","additions":20,"deletions":2}],"additions":20,"deletions":2,"truncated":true,"checksum":"x","updatedAt":"2026-11-08T00:00:00Z"})).unwrap();
        let partial = parse_changes(snapshot.clone());
        assert!(partial.truncated);
        assert_eq!(partial.files[0].additions, 20);
        assert!(partial.files[0].lines[0].text.contains("partial"));
        snapshot.files.clear();
        assert!(parse_changes(snapshot).files.is_empty());
    }
}
