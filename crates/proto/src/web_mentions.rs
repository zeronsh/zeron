//! Web page chips: a browser tab dropped into a composer. The draft and the
//! transcript keep the private link; the agent receives the page's title and
//! address, which it can fetch with its own web tools.
use std::ops::Range;

use crate::file_mentions::escape_mention_label;

pub const WEB_MENTION_SCHEME: &str = "zeron-web:";

pub struct WebMentionLink {
    pub range: Range<usize>,
    pub title: String,
    pub url: String,
}

/// Bytes a link destination can carry verbatim. Everything else, including
/// parentheses, spaces and non-ASCII, is percent-encoded, so a chip's
/// destination never needs Markdown escaping and parses back unchanged.
fn plain_url_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'-' | b'.'
                | b'_'
                | b'~'
                | b':'
                | b'/'
                | b'?'
                | b'#'
                | b'@'
                | b'!'
                | b'$'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b','
                | b';'
                | b'='
                | b'%'
        )
}

fn encode_url(url: &str) -> String {
    let mut out = String::with_capacity(url.len());
    for byte in url.bytes() {
        if plain_url_byte(byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// The host of an http(s) address without credentials, or `None`.
fn web_host(url: &str) -> Option<&str> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let authority = &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())];
    if authority.contains('@') {
        return None;
    }
    let host = match authority.strip_prefix('[') {
        // An IPv6 literal keeps its brackets.
        Some(v6) => &authority[..v6.find(']')? + 2],
        None => authority.split(':').next()?,
    };
    (!host.is_empty()).then_some(host)
}

/// Only plain web addresses: no credentials, no other schemes.
fn valid_url(url: &str) -> bool {
    url.len() <= 2048 && url.bytes().all(plain_url_byte) && web_host(url).is_some()
}

/// One line, so the chip and the agent's text read as a single phrase. An
/// untitled page is named by its host.
fn clean_title(title: &str, url: &str) -> String {
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if !title.is_empty() {
        return title;
    }
    web_host(url).unwrap_or("Web page").to_owned()
}

/// `[title](zeron-web:<url>)`, or `None` for an address that isn't a plain
/// http(s) page.
pub fn web_mention_link(url: &str, title: &str) -> Option<String> {
    let url = encode_url(url);
    valid_url(&url).then(|| {
        format!(
            "[{}]({WEB_MENTION_SCHEME}{url})",
            escape_mention_label(&clean_title(title, &url))
        )
    })
}

/// Only decode canonical chip links, never escaped text or code examples.
pub fn web_mention_links(text: &str) -> Vec<WebMentionLink> {
    if !text.contains(WEB_MENTION_SCHEME) {
        return Vec::new();
    }
    let mut image_depth = 0;
    let mut open: Option<(Range<usize>, String)> = None;
    let mut title = String::new();
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
                if let Some(url) = dest_url.strip_prefix(WEB_MENTION_SCHEME)
                    && valid_url(url)
                {
                    open = Some((range, url.to_owned()));
                    title.clear();
                }
            }
            pulldown_cmark::Event::Text(text) | pulldown_cmark::Event::Code(text)
                if open.is_some() =>
            {
                title.push_str(&text);
            }
            pulldown_cmark::Event::End(pulldown_cmark::TagEnd::Link) => {
                if let Some((range, url)) = open.take()
                    && web_mention_link(&url, &title).as_deref() == Some(&text[range.clone()])
                {
                    links.push(WebMentionLink {
                        range,
                        title: clean_title(&title, &url),
                        url,
                    });
                }
            }
            _ => {}
        }
    }
    links
}

/// Replace each web chip with text any agent can act on. The durable
/// transcript keeps the chip; only outgoing text changes.
pub fn web_mention_prompt(text: &str) -> String {
    let mut out = String::new();
    let mut at = 0;
    for link in web_mention_links(text) {
        out.push_str(&text[at..link.range.start]);
        out.push_str(&format!("the web page \"{}\" ({})", link.title, link.url));
        at = link.range.end;
    }
    out.push_str(&text[at..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chips_round_trip_and_reach_the_agent_as_title_and_address() {
        let link = web_mention_link("https://docs.rs/gpui/latest/", "gpui  -\nRust").unwrap();
        assert_eq!(
            link,
            "[gpui - Rust](zeron-web:https://docs.rs/gpui/latest/)"
        );
        let text = format!("Read {link} first");
        let links = web_mention_links(&text);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].title, "gpui - Rust");
        assert_eq!(links[0].url, "https://docs.rs/gpui/latest/");
        assert_eq!(&text[links[0].range.clone()], link);
        assert_eq!(
            web_mention_prompt(&text),
            "Read the web page \"gpui - Rust\" (https://docs.rs/gpui/latest/) first"
        );
        // Markup in a title stays text.
        let marked = web_mention_link("https://a.dev", "[x](y) `z`").unwrap();
        assert_eq!(web_mention_links(&marked)[0].title, "[x](y) `z`");
    }

    #[test]
    fn awkward_addresses_are_encoded_once_and_untitled_pages_use_their_host() {
        let link = web_mention_link(
            "https://en.wikipedia.org/wiki/Rust_(programming language)?q=a&b=ñ",
            "",
        )
        .unwrap();
        assert_eq!(
            link,
            "[en.wikipedia.org](zeron-web:https://en.wikipedia.org/wiki/\
             Rust_%28programming%20language%29?q=a&b=%C3%B1)"
        );
        let links = web_mention_links(&link);
        assert_eq!(links.len(), 1);
        assert_eq!(
            links[0].url,
            "https://en.wikipedia.org/wiki/Rust_%28programming%20language%29?q=a&b=%C3%B1"
        );
        // Re-encoding an encoded address changes nothing.
        assert_eq!(web_mention_link(&links[0].url, "").unwrap(), link);
        assert_eq!(
            web_mention_link("http://localhost:3000/", "").unwrap(),
            "[localhost](zeron-web:http://localhost:3000/)"
        );
    }

    #[test]
    fn only_canonical_http_pages_become_chips() {
        for url in [
            "",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "https://user:pass@example.com/",
            "zeron-web:https://a.dev",
        ] {
            assert!(web_mention_link(url, "t").is_none(), "{url}");
        }
        let link = web_mention_link("https://a.dev/", "Title").unwrap();
        for literal in [
            format!("`{link}`"),
            format!("```\n{link}\n```"),
            format!("\\{link}"),
            "[x](zeron-web:https://a.dev/a b)".to_owned(),
            "[x](zeron-web:ftp://a.dev/)".to_owned(),
            "[Title](zeron-web:https://a.dev/(x))".to_owned(),
        ] {
            assert!(web_mention_links(&literal).is_empty(), "{literal}");
            assert_eq!(web_mention_prompt(&literal), literal);
        }
        assert_eq!(web_mention_links("[](zeron-web:https://a.dev/)").len(), 0);
    }
}
