//! antigravity's localharness hands background-task wakeups to the model as
//! `<SYSTEM_MESSAGE>\n[Message] timestamp=… sender=… priority=… content=…\n</SYSTEM_MESSAGE>`
//! context. the acp bridge never forwards those steps, but the model sometimes
//! parrots the whole block back as its own reply text (verified against
//! agy_acp_server 1.1.1), which would otherwise land verbatim in the chat.

use futures::StreamExt;
use futures::stream::{self, BoxStream};
use zeron_proto::AgentEvent;

use crate::HarnessError;

const OPEN_TAG: &str = "<SYSTEM_MESSAGE>";
const CLOSE_TAG: &str = "</SYSTEM_MESSAGE>";
// only wakeup-shaped blocks are stripped, so a reply that merely mentions the
// tag (e.g. explaining it inside backticks) streams through untouched.
const WAKEUP_MARKER: &str = "[Message]";

/// streaming stripper for one delta channel: blocks and their tags can be split
/// across arbitrary chunk boundaries, so a possible tag prefix is held back
/// until the next chunk decides it.
#[derive(Default)]
struct EchoStrip {
    pending: String,
    inside_block: bool,
}

impl EchoStrip {
    fn push(&mut self, chunk: &str) -> String {
        self.pending.push_str(chunk);
        let mut visible = String::new();
        loop {
            if self.inside_block {
                match self.pending.find(CLOSE_TAG) {
                    Some(start) => {
                        self.pending.drain(..start + CLOSE_TAG.len());
                        self.inside_block = false;
                    }
                    None => {
                        let keep = partial_tag_suffix(&self.pending, CLOSE_TAG);
                        self.pending.drain(..self.pending.len() - keep);
                        return visible;
                    }
                }
                continue;
            }
            let Some(start) = self.pending.find(OPEN_TAG) else {
                let keep = partial_tag_suffix(&self.pending, OPEN_TAG);
                visible.extend(self.pending.drain(..self.pending.len() - keep));
                return visible;
            };
            visible.extend(self.pending.drain(..start));
            let body = self.pending[OPEN_TAG.len()..].trim_start();
            if body.starts_with(WAKEUP_MARKER) {
                self.inside_block = true;
                self.pending.drain(..OPEN_TAG.len());
            } else if WAKEUP_MARKER.starts_with(body) {
                return visible;
            } else {
                visible.extend(self.pending.drain(..OPEN_TAG.len()));
            }
        }
    }

    /// an unterminated block is dropped: its content is the echoed wakeup, not
    /// reply prose.
    fn finish(&mut self) -> String {
        let inside_block = std::mem::take(&mut self.inside_block);
        let pending = std::mem::take(&mut self.pending);
        if inside_block { String::new() } else { pending }
    }
}

/// length of the longest proper prefix of `tag` that `text` ends with. tags are
/// ascii, so the split point is always a char boundary.
fn partial_tag_suffix(text: &str, tag: &str) -> usize {
    (1..tag.len())
        .rev()
        .find(|&len| text.ends_with(&tag[..len]))
        .unwrap_or(0)
}

#[derive(Default)]
pub(crate) struct SystemMessageEchoFilter {
    text: EchoStrip,
    reasoning: EchoStrip,
}

impl SystemMessageEchoFilter {
    /// metadata and tool updates can interleave with chunks of the same
    /// message, so only message boundaries reset the filters.
    pub(crate) fn apply(&mut self, event: AgentEvent) -> Vec<AgentEvent> {
        match event {
            AgentEvent::TextDelta { text } => text_delta(self.text.push(&text)),
            AgentEvent::ReasoningDelta { text } => reasoning_delta(self.reasoning.push(&text)),
            boundary @ (AgentEvent::SessionStarted { .. }
            | AgentEvent::AssistantMessageCompleted { .. }
            | AgentEvent::Steered { .. }
            | AgentEvent::UserMessage { .. }
            | AgentEvent::Done { .. }) => {
                let mut events = self.finish();
                events.push(boundary);
                events
            }
            event => vec![event],
        }
    }

    pub(crate) fn finish(&mut self) -> Vec<AgentEvent> {
        let mut events = reasoning_delta(self.reasoning.finish());
        events.extend(text_delta(self.text.finish()));
        events
    }
}

fn text_delta(text: String) -> Vec<AgentEvent> {
    if text.is_empty() {
        Vec::new()
    } else {
        vec![AgentEvent::TextDelta { text }]
    }
}

fn reasoning_delta(text: String) -> Vec<AgentEvent> {
    if text.is_empty() {
        Vec::new()
    } else {
        vec![AgentEvent::ReasoningDelta { text }]
    }
}

pub(crate) fn strip_system_message_echoes(
    events: BoxStream<'static, Result<AgentEvent, HarnessError>>,
) -> BoxStream<'static, Result<AgentEvent, HarnessError>> {
    let mut filter = SystemMessageEchoFilter::default();
    events
        .map(Some)
        .chain(stream::once(async { None }))
        .flat_map(move |item| {
            let filtered: Vec<Result<AgentEvent, HarnessError>> = match item {
                Some(Ok(event)) => filter.apply(event).into_iter().map(Ok).collect(),
                Some(Err(error)) => filter
                    .finish()
                    .into_iter()
                    .map(Ok)
                    .chain([Err(error)])
                    .collect(),
                None => filter.finish().into_iter().map(Ok).collect(),
            };
            stream::iter(filtered)
        })
        .boxed()
}

#[cfg(test)]
mod tests {
    use super::*;

    const WAKEUP: &str = "<SYSTEM_MESSAGE>\n[Message] timestamp=2026-09-18T10:07:35Z \
        sender=1420b5b8/task-156 priority=MESSAGE_PRIORITY_HIGH content=Task id \
        \"1420b5b8/task-156\" finished with result:\n\nThe command exited with code 0. \
        Output: Compiling echo v0.1.0\n\nLog: file:///tmp/task-156.log\n</SYSTEM_MESSAGE>";

    fn stream_text(chunks: &[&str]) -> String {
        let mut strip = EchoStrip::default();
        let mut visible: String = chunks.iter().map(|chunk| strip.push(chunk)).collect();
        visible.push_str(&strip.finish());
        visible
    }

    fn every_split(text: &str) -> impl Iterator<Item = (&str, &str)> {
        (0..=text.len())
            .filter(|&at| text.is_char_boundary(at))
            .map(|at| text.split_at(at))
    }

    #[test]
    fn strips_a_whole_wakeup_block() {
        let reply = format!("Waiting for Echo to compile...\n\n{WAKEUP}\n\nIt built.");
        assert_eq!(
            stream_text(&[&reply]),
            "Waiting for Echo to compile...\n\n\n\nIt built."
        );
    }

    #[test]
    fn strips_a_block_split_at_every_position() {
        let reply = format!("before {WAKEUP} after");
        for (head, tail) in every_split(&reply) {
            assert_eq!(
                stream_text(&[head, tail]),
                "before  after",
                "split at {head:?}"
            );
        }
    }

    #[test]
    fn strips_a_block_streamed_one_char_at_a_time() {
        let reply = format!("a{WAKEUP}b{WAKEUP}c");
        let chars: Vec<String> = reply.chars().map(String::from).collect();
        let chunks: Vec<&str> = chars.iter().map(String::as_str).collect();
        assert_eq!(stream_text(&chunks), "abc");
    }

    #[test]
    fn keeps_a_prose_mention_of_the_tag() {
        let reply =
            "the environment sends a wakeup (`<SYSTEM_MESSAGE>`) and then `</SYSTEM_MESSAGE>`";
        for (head, tail) in every_split(reply) {
            assert_eq!(stream_text(&[head, tail]), reply, "split at {head:?}");
        }
    }

    #[test]
    fn keeps_a_trailing_partial_tag_and_multibyte_text() {
        assert_eq!(
            stream_text(&["tags look like <SYS", "TEM"]),
            "tags look like <SYSTEM"
        );
        assert_eq!(stream_text(&["héllo → <"]), "héllo → <");
        assert_eq!(stream_text(&["<SYSTEM_MESSAGE>\n"]), "<SYSTEM_MESSAGE>\n");
    }

    #[test]
    fn drops_an_unterminated_block() {
        assert_eq!(
            stream_text(&["done. <SYSTEM_MESSAGE> [Message] timestamp=x content=Task"]),
            "done. "
        );
    }

    #[test]
    fn a_boundary_event_flushes_held_text_before_itself() {
        let mut filter = SystemMessageEchoFilter::default();
        assert!(
            filter
                .apply(AgentEvent::TextDelta { text: "x <".into() })
                .contains(&AgentEvent::TextDelta { text: "x ".into() })
        );
        let done = AgentEvent::AssistantMessageCompleted {
            assistant_message_id: "m1".into(),
        };
        assert_eq!(
            filter.apply(done.clone()),
            vec![AgentEvent::TextDelta { text: "<".into() }, done]
        );
    }

    #[test]
    fn interleaved_updates_preserve_blocks_at_every_split() {
        let updates = [
            AgentEvent::ContextUsage {
                tokens: Some(42),
                window: Some(1000),
            },
            AgentEvent::Usage {
                input_tokens: 42,
                output_tokens: 10,
            },
            AgentEvent::AvailableCommands {
                commands: Vec::new(),
            },
            AgentEvent::ToolResult {
                id: "background-task".into(),
                is_error: false,
                output: None,
                diff: None,
            },
        ];
        for reasoning in [false, true] {
            let delta = |text| {
                if reasoning {
                    AgentEvent::ReasoningDelta { text }
                } else {
                    AgentEvent::TextDelta { text }
                }
            };
            for update in &updates {
                for (head, tail) in every_split(WAKEUP) {
                    let mut filter = SystemMessageEchoFilter::default();
                    let mut events = filter.apply(delta(format!("before {head}")));
                    events.extend(filter.apply(update.clone()));
                    events.extend(filter.apply(delta(format!("{tail} after"))));
                    events.extend(filter.finish());
                    assert_eq!(
                        events,
                        vec![
                            delta("before ".into()),
                            update.clone(),
                            delta(" after".into()),
                        ],
                        "split at {head:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn reasoning_between_text_deltas_does_not_break_a_block() {
        let mut filter = SystemMessageEchoFilter::default();
        let (head, tail) = WAKEUP.split_at(40);
        let mut events = filter.apply(AgentEvent::TextDelta {
            text: format!("ok {head}"),
        });
        events.extend(filter.apply(AgentEvent::ReasoningDelta {
            text: "thinking".into(),
        }));
        events.extend(filter.apply(AgentEvent::TextDelta {
            text: format!("{tail} fine"),
        }));
        events.extend(filter.finish());
        assert_eq!(
            events,
            vec![
                AgentEvent::TextDelta { text: "ok ".into() },
                AgentEvent::ReasoningDelta {
                    text: "thinking".into()
                },
                AgentEvent::TextDelta {
                    text: " fine".into()
                },
            ]
        );
    }
}
