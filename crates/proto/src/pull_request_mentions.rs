//! Pull request chips: a reference to a pull request, dropped into a composer
//! from a pull request tab. The draft and the transcript keep the private
//! link; the agent receives a plain description with the pull request's URL.
use std::ops::Range;

use crate::file_mentions::escape_mention_label;

pub const PULL_REQUEST_MENTION_SCHEME: &str = "zeron-pr:";

pub struct PullRequestMentionLink {
    pub range: Range<usize>,
    /// What the chip shows, e.g. `#42 Fix the composer`.
    pub label: String,
    pub url: String,
}

/// Pull request URLs are plain web addresses: never markup, whitespace or
/// anything Markdown would unescape differently.
fn valid_url(url: &str) -> bool {
    url.len() <= 512
        && url.strip_prefix("https://").is_some_and(|rest| {
            !rest.is_empty()
                && rest.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/')
                })
        })
}

/// One line, so the chip and the agent's text read as a single phrase.
fn clean_label(label: &str) -> String {
    let label = label.split_whitespace().collect::<Vec<_>>().join(" ");
    if label.is_empty() {
        "Pull request".into()
    } else {
        label
    }
}

/// The chip's label: the number, when known, before the title.
pub fn pull_request_mention_label(number: Option<u64>, title: &str) -> String {
    match number {
        Some(number) => clean_label(&format!("#{number} {title}")),
        None => clean_label(title),
    }
}

/// `[label](zeron-pr:<url>)`, or `None` for a URL that isn't a plain https
/// address.
pub fn pull_request_mention_link(url: &str, label: &str) -> Option<String> {
    valid_url(url).then(|| {
        format!(
            "[{}]({PULL_REQUEST_MENTION_SCHEME}{url})",
            escape_mention_label(&clean_label(label))
        )
    })
}

/// Only decode canonical chip links, never escaped text or code examples.
pub fn pull_request_mention_links(text: &str) -> Vec<PullRequestMentionLink> {
    if !text.contains(PULL_REQUEST_MENTION_SCHEME) {
        return Vec::new();
    }
    let mut image_depth = 0;
    let mut open: Option<(Range<usize>, String)> = None;
    let mut label = String::new();
    let mut links = Vec::new();
    for (event, range) in pulldown_cmark::Parser::new(text).into_offset_iter() {
        match &event {
            pulldown_cmark::Event::Start(pulldown_cmark::Tag::Image { .. }) => image_depth += 1,
            pulldown_cmark::Event::End(pulldown_cmark::TagEnd::Image) => image_depth -= 1,
            _ => {}
        }
        if image_depth > 0 {
            continue;
        }
        match event {
            pulldown_cmark::Event::Start(pulldown_cmark::Tag::Link { dest_url, .. }) => {
                if let Some(url) = dest_url.strip_prefix(PULL_REQUEST_MENTION_SCHEME)
                    && valid_url(url)
                {
                    open = Some((range, url.to_owned()));
                    label.clear();
                }
            }
            pulldown_cmark::Event::Text(text) | pulldown_cmark::Event::Code(text)
                if open.is_some() =>
            {
                label.push_str(&text);
            }
            pulldown_cmark::Event::End(pulldown_cmark::TagEnd::Link) => {
                if let Some((range, url)) = open.take()
                    && pull_request_mention_link(&url, &label).as_deref()
                        == Some(&text[range.clone()])
                {
                    links.push(PullRequestMentionLink {
                        range,
                        label: clean_label(&label),
                        url,
                    });
                }
            }
            _ => {}
        }
    }
    links
}

/// Replace each pull request chip with text any agent can act on. The durable
/// transcript keeps the chip; only outgoing text changes.
pub fn pull_request_mention_prompt(text: &str) -> String {
    let mut out = String::new();
    let mut at = 0;
    for link in pull_request_mention_links(text) {
        out.push_str(&text[at..link.range.start]);
        out.push_str(&format!(
            "the pull request \"{}\" ({})",
            link.label, link.url
        ));
        at = link.range.end;
    }
    out.push_str(&text[at..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const URL: &str = "https://github.com/acme/zeron/pull/42";

    #[test]
    fn chips_round_trip_and_reach_the_agent_with_their_url() {
        let label = pull_request_mention_label(Some(42), "Fix  the\ncomposer");
        assert_eq!(label, "#42 Fix the composer");
        let link = pull_request_mention_link(URL, &label).unwrap();
        assert_eq!(link, format!("[#42 Fix the composer](zeron-pr:{URL})"));
        let text = format!("Review {link} please");
        let links = pull_request_mention_links(&text);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].label, "#42 Fix the composer");
        assert_eq!(links[0].url, URL);
        assert_eq!(&text[links[0].range.clone()], link);
        assert_eq!(
            pull_request_mention_prompt(&text),
            format!("Review the pull request \"#42 Fix the composer\" ({URL}) please")
        );
        // Markup in a title stays text.
        let marked = pull_request_mention_link(URL, "[x](y) `z`").unwrap();
        assert_eq!(pull_request_mention_links(&marked)[0].label, "[x](y) `z`");
        assert_eq!(pull_request_mention_label(None, " "), "Pull request");
    }

    #[test]
    fn only_canonical_plain_urls_become_chips() {
        for url in [
            "",
            "http://github.com/a/b/pull/1",
            "https://",
            "https://github.com/a b/pull/1",
            "https://github.com/a/b/pull/1)",
            "https://github.com/a/b/pull/1?x=<y>",
        ] {
            assert!(pull_request_mention_link(url, "t").is_none(), "{url}");
        }
        let link = pull_request_mention_link(URL, "Title").unwrap();
        for literal in [
            format!("`{link}`"),
            format!("```\n{link}\n```"),
            format!("\\{link}"),
            "[x](zeron-pr:https://a b)".to_owned(),
            "[x](zeron-pr:javascript:alert)".to_owned(),
        ] {
            assert!(pull_request_mention_links(&literal).is_empty(), "{literal}");
            assert_eq!(pull_request_mention_prompt(&literal), literal);
        }
        assert!(pull_request_mention_links(&format!("[](zeron-pr:{URL})")).is_empty());
    }
}
