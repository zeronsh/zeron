//! Chat chips: a reference to another Zeron chat, dropped into a composer
//! from the sidebar. The draft and the transcript keep the private link; the
//! agent receives a plain description it can act on with Zeron's `read_chat`
//! tool, which accepts the chat's id.
use std::ops::Range;

use crate::file_mentions::escape_mention_label;

pub const CHAT_MENTION_SCHEME: &str = "zeron-chat:";

pub struct ChatMentionLink {
    pub range: Range<usize>,
    pub title: String,
    pub chat_id: String,
}

/// Chat ids are opaque but plain: never a path, query or markup.
fn valid_chat_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// One line, so the chip and the agent's text read as a single phrase.
fn clean_title(title: &str) -> String {
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.is_empty() {
        "Untitled chat".into()
    } else {
        title
    }
}

/// `[title](zeron-chat:<id>)`, or `None` for an id that isn't a plain chat id.
pub fn chat_mention_link(chat_id: &str, title: &str) -> Option<String> {
    valid_chat_id(chat_id).then(|| {
        format!(
            "[{}]({CHAT_MENTION_SCHEME}{chat_id})",
            escape_mention_label(&clean_title(title))
        )
    })
}

/// Only decode canonical chip links, never escaped text or code examples.
pub fn chat_mention_links(text: &str) -> Vec<ChatMentionLink> {
    if !text.contains(CHAT_MENTION_SCHEME) {
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
                if let Some(id) = dest_url.strip_prefix(CHAT_MENTION_SCHEME)
                    && valid_chat_id(id)
                {
                    open = Some((range, id.to_owned()));
                    title.clear();
                }
            }
            pulldown_cmark::Event::Text(text) | pulldown_cmark::Event::Code(text)
                if open.is_some() =>
            {
                title.push_str(&text);
            }
            pulldown_cmark::Event::End(pulldown_cmark::TagEnd::Link) => {
                if let Some((range, chat_id)) = open.take()
                    && chat_mention_link(&chat_id, &title).as_deref() == Some(&text[range.clone()])
                {
                    links.push(ChatMentionLink {
                        range,
                        title: clean_title(&title),
                        chat_id,
                    });
                }
            }
            _ => {}
        }
    }
    links
}

/// Replace each chat chip with text any agent can act on. The durable
/// transcript keeps the chip; only outgoing text changes.
pub fn chat_mention_prompt(text: &str) -> String {
    let mut out = String::new();
    let mut at = 0;
    for link in chat_mention_links(text) {
        out.push_str(&text[at..link.range.start]);
        out.push_str(&format!(
            "the Zeron chat \"{}\" (chat id {}; read it with the Zeron read_chat tool)",
            link.title, link.chat_id
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
    fn chips_round_trip_and_reach_the_agent_as_a_readable_reference() {
        let link = chat_mention_link("0fd51606-ab", "Brief  Hello\nInteraction").unwrap();
        assert_eq!(link, "[Brief Hello Interaction](zeron-chat:0fd51606-ab)");
        let text = format!("Compare {link} with this one");
        let links = chat_mention_links(&text);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].title, "Brief Hello Interaction");
        assert_eq!(links[0].chat_id, "0fd51606-ab");
        assert_eq!(&text[links[0].range.clone()], link);
        assert_eq!(
            chat_mention_prompt(&text),
            "Compare the Zeron chat \"Brief Hello Interaction\" (chat id 0fd51606-ab; \
             read it with the Zeron read_chat tool) with this one"
        );
        // Markup in a title stays text.
        let marked = chat_mention_link("a1", "[x](y) `z`").unwrap();
        assert_eq!(chat_mention_links(&marked)[0].title, "[x](y) `z`");
    }

    #[test]
    fn only_canonical_plain_ids_become_chips() {
        assert!(chat_mention_link("../x", "t").is_none());
        assert!(chat_mention_link("", "t").is_none());
        let link = chat_mention_link("a1", "Title").unwrap();
        for literal in [
            format!("`{link}`"),
            format!("```\n{link}\n```"),
            format!("\\{link}"),
            "[x](zeron-chat:a b)".to_owned(),
            "[x](zeron-chat:a/b)".to_owned(),
        ] {
            assert!(chat_mention_links(&literal).is_empty(), "{literal}");
            assert_eq!(chat_mention_prompt(&literal), literal);
        }
        assert_eq!(chat_mention_links("[](zeron-chat:a1)").len(), 0);
    }
}
