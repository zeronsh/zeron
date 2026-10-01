//! Release notes for the "What's new" window: fetched from the project's
//! GitHub releases and parsed into a small structure the UI can lay out.
//!
//! GitHub's generated notes are a `## What's Changed` bullet list
//! (`* Title by @author in https://github.com/org/repo/pull/123`) followed by
//! `## New Contributors` and a `**Full Changelog**` link. Hand-written notes are
//! free-form markdown. [`parse_body`] lifts the generated bullets into
//! [`Change`] rows (area tag, kind, PR, author) and keeps everything else as
//! markdown [`Item::Note`]s so nothing a maintainer wrote is lost.
//!
//! The fetch is advisory: any failure leaves the window unshown and is retried
//! on the next launch.

use std::time::Duration;

use serde::Deserialize;

use crate::version_newer;

/// The releases listing the window reads (unauthenticated, newest first).
pub const RELEASES_API: &str = "https://api.github.com/repos/zeronsh/zeron/releases";

/// Most versions one window stacks; a longer gap links to the releases page.
pub const MAX_RELEASES: usize = 5;

/// How many releases to request — comfortably more than [`MAX_RELEASES`] so
/// skipped drafts and prereleases do not starve the window.
const PAGE_SIZE: usize = 30;

/// One published release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// Dotted version without the leading `v`.
    pub version: String,
    pub url: String,
    /// RFC 3339 timestamp, as GitHub reports it.
    pub published_at: Option<String>,
    pub notes: Notes,
}

/// A release body, structured.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Notes {
    pub sections: Vec<Section>,
    /// The "compare" link from GitHub's `**Full Changelog**` footer.
    pub full_changelog: Option<String>,
}

impl Notes {
    pub fn is_empty(&self) -> bool {
        self.sections.iter().all(|section| section.items.is_empty())
    }

    /// Entries across every section.
    pub fn item_count(&self) -> usize {
        self.sections
            .iter()
            .map(|section| section.items.len())
            .sum()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Section {
    /// `None` for text before the first heading.
    pub title: Option<String>,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Change(Change),
    /// A first-time contributor from GitHub's `## New Contributors` list.
    Contributor {
        handle: String,
        url: Option<String>,
    },
    /// Anything else, as markdown (a paragraph, a hand-written bullet, a
    /// fenced block).
    Note(String),
}

/// One `* Title by @author in …/pull/N` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// Inline markdown (titles carry `code` spans).
    pub title: String,
    /// Feature area from a `Area: title` prefix.
    pub area: Option<String>,
    pub kind: ChangeKind,
    pub author: Option<String>,
    pub pull: Option<u64>,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    New,
    Fix,
    Improvement,
}

/// Parse a release body. Never fails: unrecognized text becomes [`Item::Note`].
pub fn parse_body(body: &str) -> Notes {
    let mut notes = Notes::default();
    let mut section = Section::default();
    let mut paragraph = String::new();
    let mut fence: Option<String> = None;
    let mut contributors = false;

    fn flush_paragraph(section: &mut Section, paragraph: &mut String) {
        let text = paragraph.trim();
        if !text.is_empty() {
            section.items.push(Item::Note(text.to_owned()));
        }
        paragraph.clear();
    }
    fn finish_section(notes: &mut Notes, section: &mut Section) {
        let done = std::mem::take(section);
        if done.title.is_some() || !done.items.is_empty() {
            notes.sections.push(done);
        }
    }

    for raw in body.lines() {
        let line = raw.trim_end();

        if let Some(marker) = &fence {
            paragraph.push('\n');
            paragraph.push_str(line);
            if line.trim_start().starts_with(marker.as_str()) {
                fence = None;
                flush_paragraph(&mut section, &mut paragraph);
            }
            continue;
        }
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            flush_paragraph(&mut section, &mut paragraph);
            fence = Some(trimmed.chars().take(3).collect());
            paragraph.push_str(line);
            continue;
        }

        if trimmed.is_empty() {
            flush_paragraph(&mut section, &mut paragraph);
            continue;
        }

        if let Some(title) = heading(trimmed) {
            flush_paragraph(&mut section, &mut paragraph);
            finish_section(&mut notes, &mut section);
            contributors = title.eq_ignore_ascii_case("new contributors");
            section.title = Some(title.to_owned());
            continue;
        }

        if let Some(url) = full_changelog(trimmed) {
            flush_paragraph(&mut section, &mut paragraph);
            notes.full_changelog = Some(url);
            continue;
        }

        // Top-level bullets only; indented lines continue the previous item.
        let indented = line.starts_with(' ') || line.starts_with('\t');
        if !indented && let Some(text) = bullet(trimmed) {
            flush_paragraph(&mut section, &mut paragraph);
            let item = if contributors {
                parse_contributor(text)
            } else {
                parse_change(text)
            }
            .unwrap_or_else(|| Item::Note(format!("- {text}")));
            section.items.push(item);
            continue;
        }
        if indented && paragraph.is_empty() {
            // Continuation of a hand-written bullet.
            if let Some(Item::Note(last)) = section.items.last_mut() {
                last.push('\n');
                last.push_str(line);
                continue;
            }
        }

        if !paragraph.is_empty() {
            paragraph.push('\n');
        }
        paragraph.push_str(line);
    }
    if fence.is_some() {
        // Unterminated fence: keep what was written.
        paragraph.push_str("\n```");
    }
    flush_paragraph(&mut section, &mut paragraph);
    finish_section(&mut notes, &mut section);
    notes
}

fn heading(line: &str) -> Option<&str> {
    let rest = line.trim_start_matches('#');
    let level = line.len() - rest.len();
    ((1..=6).contains(&level) && rest.starts_with(' '))
        .then(|| rest.trim().trim_end_matches('#').trim())
}

fn bullet(line: &str) -> Option<&str> {
    line.strip_prefix("* ")
        .or_else(|| line.strip_prefix("- "))
        .or_else(|| line.strip_prefix("+ "))
        .map(str::trim)
}

fn full_changelog(line: &str) -> Option<String> {
    let rest = line
        .strip_prefix("**Full Changelog**:")
        .or_else(|| line.strip_prefix("**Full Changelog**"))?;
    let url = rest.trim().trim_start_matches(':').trim();
    url.starts_with("https://").then(|| url.to_owned())
}

/// `Title by @author in https://github.com/org/repo/pull/N`.
fn parse_change(text: &str) -> Option<Item> {
    let (head, url) = text.rsplit_once(" in ")?;
    let url = url.trim();
    let pull = pull_number(url)?;
    let (title, author) = head.rsplit_once(" by @")?;
    let author = author.trim();
    if title.trim().is_empty() || author.is_empty() || author.contains(char::is_whitespace) {
        return None;
    }
    let (area, title, conventional) = split_area(title.trim());
    Some(Item::Change(Change {
        kind: conventional.unwrap_or_else(|| classify(&title)),
        title,
        area,
        author: Some(author.to_owned()),
        pull: Some(pull),
        url: Some(url.to_owned()),
    }))
}

/// `@handle made their first contribution in https://…/pull/N`.
fn parse_contributor(text: &str) -> Option<Item> {
    let rest = text.strip_prefix('@')?;
    let (handle, rest) = rest.split_once(' ')?;
    rest.starts_with("made their first contribution")
        .then(|| Item::Contributor {
            handle: handle.to_owned(),
            url: rest
                .rsplit_once(" in ")
                .map(|(_, url)| url.trim().to_owned())
                .filter(|url| url.starts_with("https://")),
        })
}

fn pull_number(url: &str) -> Option<u64> {
    let rest = url.strip_prefix("https://github.com/")?;
    let (_, number) = rest.split_once("/pull/")?;
    number.trim_end_matches('/').parse().ok()
}

/// Split a `Cursor harness: load user MCP servers` prefix into an area tag
/// and a capitalized title. Longer or code-bearing prefixes are not areas.
/// Conventional-commit prefixes (`fix:`, `feat(ui):`, `chore:`…) are not areas
/// either: they set the kind instead (`fix` → Fix, `feat` → New, the rest →
/// Improvement), and a `(scope)` becomes the area.
fn split_area(title: &str) -> (Option<String>, String, Option<ChangeKind>) {
    if let Some((prefix, rest)) = title.split_once(": ") {
        let prefix = prefix.trim();
        let rest = rest.trim();
        if !rest.is_empty() {
            let (word, scope) = match prefix.split_once('(') {
                Some((word, scope)) => (word.trim_end_matches('!'), scope.strip_suffix(')')),
                None => (prefix.trim_end_matches('!'), None),
            };
            let kind = match word.to_ascii_lowercase().as_str() {
                "fix" | "bugfix" | "hotfix" => Some(ChangeKind::Fix),
                "feat" | "feature" => Some(ChangeKind::New),
                "chore" | "docs" | "refactor" | "perf" | "test" | "tests" | "ci" | "build"
                | "style" | "revert" => Some(ChangeKind::Improvement),
                _ => None,
            };
            if kind.is_some() && (scope.is_some() || prefix == word) {
                let area = scope.map(str::trim).filter(|scope| !scope.is_empty());
                return (area.map(capitalize), capitalize(rest), kind);
            }
        }
        let words = prefix.split_whitespace().count();
        if (1..=3).contains(&words)
            && prefix.chars().count() <= 24
            && !rest.is_empty()
            && !prefix.contains(['`', '[', '(', '/', ':'])
        {
            return (Some(prefix.to_owned()), capitalize(rest), None);
        }
    }
    (None, title.to_owned(), None)
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) if first.is_lowercase() => first.to_uppercase().chain(chars).collect(),
        _ => text.to_owned(),
    }
}

fn classify(title: &str) -> ChangeKind {
    let first = title
        .split(|c: char| !c.is_alphanumeric())
        .find(|word| !word.is_empty())
        .unwrap_or("")
        .to_ascii_lowercase();
    match first.as_str() {
        "fix" | "fixes" | "fixed" | "resolve" | "resolves" | "prevent" | "prevents" | "correct"
        | "stop" | "repair" | "patch" | "avoid" => ChangeKind::Fix,
        "add" | "adds" | "added" | "introduce" | "introduces" | "new" | "support" | "supports"
        | "enable" | "enables" | "allow" | "allows" | "show" | "shows" | "offer" | "offers" => {
            ChangeKind::New
        }
        _ => ChangeKind::Improvement,
    }
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

/// The releases a user who last saw `since` should read on reaching
/// `current`, newest first and capped at [`MAX_RELEASES`].
///
/// `since: None` (the previous version is unknown) selects only `current`.
/// Returns the selection plus how many older releases were left out.
pub fn select_releases(
    api_json: &str,
    since: Option<&str>,
    current: &str,
) -> anyhow::Result<(Vec<Release>, usize)> {
    let mut parsed: Vec<ApiRelease> = serde_json::from_str(api_json)?;
    parsed.retain(|release| !release.draft && !release.prerelease);
    let version_of = |release: &ApiRelease| release.tag_name.trim_start_matches('v').to_owned();
    parsed.sort_by(|a, b| {
        let (a, b) = (version_of(a), version_of(b));
        if version_newer(&a, &b) {
            std::cmp::Ordering::Less
        } else if version_newer(&b, &a) {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        }
    });
    let mut selected: Vec<Release> = parsed
        .into_iter()
        .filter(|release| {
            let version = version_of(release);
            let reached = !version_newer(&version, current);
            reached
                && match since {
                    Some(since) => version_newer(&version, since),
                    None => version == current.trim_start_matches('v'),
                }
        })
        .map(|release| Release {
            version: version_of(&release),
            url: release.html_url,
            published_at: release.published_at,
            notes: parse_body(release.body.as_deref().unwrap_or("")),
        })
        .collect();
    let omitted = selected.len().saturating_sub(MAX_RELEASES);
    selected.truncate(MAX_RELEASES);
    Ok((selected, omitted))
}

/// Fetch the notes for the versions between `since` and `current` from
/// `api_url` (normally [`RELEASES_API`]).
pub async fn fetch_release_notes(
    api_url: &str,
    since: Option<&str>,
    current: &str,
) -> anyhow::Result<(Vec<Release>, usize)> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(20))
        .user_agent(concat!("zeron/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let response = client
        .get(api_url)
        .query(&[("per_page", PAGE_SIZE.to_string())])
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await?;
    let status = response.status();
    anyhow::ensure!(status.is_success(), "GitHub releases returned {status}");
    select_releases(&response.text().await?, since, current)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verbatim body of zeronsh/zeron v0.2.100.
    const V0_2_100: &str = "## What's Changed\n* Fix file tree edge fades at scroll boundaries by @jsgrrchg in https://github.com/zeronsh/zeron/pull/665\n* Fix Markdown drag selection across table columns by @katulevskiy in https://github.com/zeronsh/zeron/pull/681\n* Question panel: paint an opaque background by @katulevskiy in https://github.com/zeronsh/zeron/pull/669\n* Providers: update Homebrew-installed CLIs with brew by @senxd in https://github.com/zeronsh/zeron/pull/661\n* Fix subagent lifecycle identity and restart recovery by @katulevskiy in https://github.com/zeronsh/zeron/pull/676\n* Appearance: add background positioning and zoom by @Mohaddz in https://github.com/zeronsh/zeron/pull/660\n* Fix drag-select failing when several transcripts share a frame by @katulevskiy in https://github.com/zeronsh/zeron/pull/632\n\n## New Contributors\n* @Mohaddz made their first contribution in https://github.com/zeronsh/zeron/pull/660\n\n**Full Changelog**: https://github.com/zeronsh/zeron/compare/v0.2.99...v0.2.100";

    #[test]
    fn parses_github_generated_notes() {
        let notes = parse_body(V0_2_100);
        assert_eq!(notes.sections.len(), 2);
        assert_eq!(
            notes.full_changelog.as_deref(),
            Some("https://github.com/zeronsh/zeron/compare/v0.2.99...v0.2.100")
        );
        let changed = &notes.sections[0];
        assert_eq!(changed.title.as_deref(), Some("What's Changed"));
        assert_eq!(changed.items.len(), 7);
        let Item::Change(first) = &changed.items[0] else {
            panic!("expected a change");
        };
        assert_eq!(first.title, "Fix file tree edge fades at scroll boundaries");
        assert_eq!(first.kind, ChangeKind::Fix);
        assert_eq!(first.author.as_deref(), Some("jsgrrchg"));
        assert_eq!(first.pull, Some(665));
        assert_eq!(first.area, None);
        let Item::Change(area) = &changed.items[2] else {
            panic!("expected a change");
        };
        assert_eq!(area.area.as_deref(), Some("Question panel"));
        assert_eq!(area.title, "Paint an opaque background");
        let Item::Change(appearance) = &changed.items[5] else {
            panic!("expected a change");
        };
        assert_eq!(appearance.area.as_deref(), Some("Appearance"));
        assert_eq!(appearance.title, "Add background positioning and zoom");
        assert_eq!(appearance.kind, ChangeKind::New);
        assert_eq!(
            notes.sections[1].items,
            vec![Item::Contributor {
                handle: "Mohaddz".into(),
                url: Some("https://github.com/zeronsh/zeron/pull/660".into()),
            }]
        );
    }

    #[test]
    fn titles_keep_inline_markdown_and_colons_are_not_always_areas() {
        let notes = parse_body(
            "## What's Changed\n* Handle `a: b` in config by @x in https://github.com/zeronsh/zeron/pull/1\n* Make the whole landing page faster: details by @y in https://github.com/zeronsh/zeron/pull/2\n",
        );
        let Item::Change(first) = &notes.sections[0].items[0] else {
            panic!()
        };
        assert_eq!(first.title, "Handle `a: b` in config");
        assert_eq!(first.area, None);
        let Item::Change(second) = &notes.sections[0].items[1] else {
            panic!()
        };
        // Long prefixes are sentences, not areas.
        assert_eq!(second.area, None);
        assert_eq!(second.title, "Make the whole landing page faster: details");
    }

    #[test]
    fn conventional_commit_prefixes_set_the_kind() {
        let pull = "https://github.com/zeronsh/zeron/pull/1";
        let change = |line: &str| {
            let Some(Item::Change(change)) = parse_change(&format!("{line} by @a in {pull}"))
            else {
                panic!("not a change: {line}")
            };
            change
        };
        let fix = change("fix: restore antigravity detection");
        assert_eq!(
            (fix.kind, fix.area, fix.title.as_str()),
            (ChangeKind::Fix, None, "Restore antigravity detection")
        );
        let scoped = change("feat(ui): shiny thing");
        assert_eq!(
            (scoped.kind, scoped.area.as_deref(), scoped.title.as_str()),
            (ChangeKind::New, Some("Ui"), "Shiny thing")
        );
        let chore = change("chore: bump deps");
        assert_eq!((chore.kind, chore.area), (ChangeKind::Improvement, None));
        // A real area that merely starts like a prefix stays an area.
        let area = change("Fixtures: tidy up");
        assert_eq!(area.area.as_deref(), Some("Fixtures"));
    }

    #[test]
    fn hand_written_notes_survive_as_markdown() {
        let notes = parse_body(
            "Big release.\nSecond line.\n\n### Highlights\n- **Faster** startup\n  with a continuation\n- Plain bullet\n\n```sh\nzeron update\n\nzeron --version\n```\n\nBye",
        );
        assert_eq!(notes.sections.len(), 2);
        assert_eq!(notes.sections[0].title, None);
        assert_eq!(
            notes.sections[0].items,
            vec![Item::Note("Big release.\nSecond line.".into())]
        );
        let items = &notes.sections[1].items;
        assert_eq!(
            items[0],
            Item::Note("- **Faster** startup\n  with a continuation".into())
        );
        assert_eq!(items[1], Item::Note("- Plain bullet".into()));
        assert_eq!(
            items[2],
            Item::Note("```sh\nzeron update\n\nzeron --version\n```".into())
        );
        assert_eq!(items[3], Item::Note("Bye".into()));
    }

    #[test]
    fn empty_and_unterminated_bodies_do_not_panic() {
        assert!(parse_body("").is_empty());
        assert!(parse_body("\n\n  \n").sections.is_empty());
        let notes = parse_body("```\nunclosed");
        assert_eq!(notes.item_count(), 1);
    }

    #[test]
    fn contributor_and_change_parsers_reject_lookalikes() {
        assert_eq!(parse_change("Just a bullet"), None);
        assert_eq!(
            parse_change("Thing by @a b in https://github.com/o/r/pull/3"),
            None
        );
        assert_eq!(parse_change("Thing by @a in https://example.com/x"), None);
        assert!(parse_contributor("@a did something").is_none());
    }

    fn api(entries: &[(&str, &str)]) -> String {
        let releases: Vec<_> = entries
            .iter()
            .map(|(tag, body)| {
                serde_json::json!({
                    "tag_name": tag,
                    "body": body,
                    "html_url": format!("https://github.com/zeronsh/zeron/releases/tag/{tag}"),
                    "published_at": "2026-10-01T01:04:21Z",
                    "draft": false,
                    "prerelease": false,
                })
            })
            .collect();
        serde_json::to_string(&releases).unwrap()
    }

    #[test]
    fn selects_the_versions_since_the_last_one_seen() {
        let json = api(&[
            ("v0.2.101", "future"),
            ("v0.2.100", "- c"),
            ("v0.2.99", "- b"),
            ("v0.2.98", "- a"),
            ("v0.2.10", "old"),
        ]);
        let (releases, omitted) = select_releases(&json, Some("0.2.98"), "0.2.100").unwrap();
        let versions: Vec<_> = releases.iter().map(|r| r.version.as_str()).collect();
        assert_eq!(versions, ["0.2.100", "0.2.99"]);
        assert_eq!(omitted, 0);

        // Unknown previous version: just the running release.
        let (releases, _) = select_releases(&json, None, "0.2.99").unwrap();
        assert_eq!(releases.len(), 1);
        assert_eq!(releases[0].version, "0.2.99");

        // Nothing newer than what was seen.
        let (releases, _) = select_releases(&json, Some("0.2.100"), "0.2.100").unwrap();
        assert!(releases.is_empty());
    }

    #[test]
    fn long_gaps_are_capped_and_drafts_skipped() {
        let mut entries: Vec<(String, String)> = (90..=100)
            .map(|n| (format!("v0.2.{n}"), format!("- r{n}")))
            .collect();
        entries.reverse();
        let mut value: Vec<serde_json::Value> = entries
            .iter()
            .map(|(tag, body)| serde_json::json!({"tag_name": tag, "body": body, "html_url": ""}))
            .collect();
        value[0]["draft"] = true.into();
        let json = serde_json::to_string(&value).unwrap();
        let (releases, omitted) = select_releases(&json, Some("0.2.80"), "0.2.100").unwrap();
        assert_eq!(releases.len(), MAX_RELEASES);
        assert_eq!(releases[0].version, "0.2.99");
        assert_eq!(omitted, 10 - MAX_RELEASES);
    }

    #[test]
    fn null_bodies_parse_as_empty_notes() {
        let json = r#"[{"tag_name":"v0.2.5","body":null,"html_url":"u"}]"#;
        let (releases, _) = select_releases(json, None, "0.2.5").unwrap();
        assert!(releases[0].notes.is_empty());
    }

    #[tokio::test]
    async fn fetches_and_selects_from_a_server() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let body = api(&[("v0.2.100", V0_2_100)]);
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let n = socket.read(&mut buf).await.unwrap();
            let request = String::from_utf8_lossy(&buf[..n]).to_ascii_lowercase();
            assert!(request.contains("per_page=30"), "{request}");
            assert!(request.contains("application/vnd.github+json"), "{request}");
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        let (releases, _) = fetch_release_notes(
            &format!("http://{addr}/releases"),
            Some("0.2.99"),
            "0.2.100",
        )
        .await
        .unwrap();
        server.await.unwrap();
        assert_eq!(releases.len(), 1);
        assert_eq!(releases[0].notes.item_count(), 8);
    }

    #[tokio::test]
    async fn http_errors_are_reported() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 1024];
            let _ = socket.read(&mut buf).await;
            socket
                .write_all(
                    b"HTTP/1.1 403 Forbidden\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                )
                .await
                .unwrap();
        });
        let err = fetch_release_notes(&format!("http://{addr}/releases"), None, "0.2.100")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("403"), "{err}");
    }
}
