//! Antigravity reports Google verification failures as text followed by end_turn.
use crate::HarnessError;
use futures::StreamExt;
use futures::stream::BoxStream;
use zeron_proto::{AgentEvent, DoneStatus};

fn verification_required(message: &str) -> bool {
    message.contains("Verify your account to continue")
}

fn explain(message: &str, account: Option<&str>) -> String {
    let account = account
        .map(|name| format!(" \"{name}\""))
        .unwrap_or_default();
    format!(
        "{message}\n\nGoogle requires verification of the Antigravity account{account} assigned to this chat. \
         Open Antigravity, sign in with that Google account and verify the account when prompted, \
         then retry. Switching accounts in Zeron Settings applies to new chats; this chat keeps its account."
    )
}

pub(super) fn normalize(
    events: BoxStream<'static, Result<AgentEvent, HarnessError>>,
    account: Option<String>,
) -> BoxStream<'static, Result<AgentEvent, HarnessError>> {
    let mut verification_error = None;
    events
        .map(move |event| {
            Ok(match event? {
                // shortcut: ACP 1.2.1 sends execution errors in one chunk; buffer if that changes.
                AgentEvent::TextDelta { text }
                    if text.starts_with("Agent execution error:")
                        && verification_required(&text) =>
                {
                    let message = explain(&text, account.as_deref());
                    verification_error = Some(message.clone());
                    AgentEvent::Error { message }
                }
                AgentEvent::Error { message } if verification_required(&message) => {
                    let message = explain(&message, account.as_deref());
                    verification_error = Some(message.clone());
                    AgentEvent::Error { message }
                }
                AgentEvent::Done {
                    mut status,
                    result,
                    mut error,
                    session_id,
                } => {
                    let failure = verification_error.take().or_else(|| {
                        error
                            .as_deref()
                            .filter(|message| verification_required(message))
                            .map(|message| explain(message, account.as_deref()))
                    });
                    if status != DoneStatus::Interrupted
                        && let Some(failure) = failure
                    {
                        status = DoneStatus::Errored;
                        error = Some(failure);
                    }
                    AgentEvent::Done {
                        status,
                        result,
                        error,
                        session_id,
                    }
                }
                event => event,
            })
        })
        .boxed()
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{StreamExt, stream};
    use serde_json::json;
    use zeron_proto::DoneStatus;

    const REJECTION: &str = "Agent execution error: Agent execution terminated due to error. (\"request failed (code 403): Verify your account to continue.\")";

    fn done() -> AgentEvent {
        AgentEvent::Done {
            status: DoneStatus::Completed,
            result: None,
            error: None,
            session_id: Some("session".into()),
        }
    }

    #[tokio::test]
    async fn verification_text_is_an_account_error_even_when_rpc_succeeds() {
        let update = json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": REJECTION}});
        let events = super::super::normalize::map_update(&update)
            .into_iter()
            .chain([done()])
            .map(Ok);
        let output = normalize(
            stream::iter(events).boxed(),
            Some("Google account 4".into()),
        )
        .collect::<Vec<_>>()
        .await;
        assert!(matches!(&output[0], Ok(AgentEvent::Error { message })
            if message.contains("Google account 4") && message.contains("verify") && message.contains("new chat")));
        assert!(matches!(
            &output[1],
            Ok(AgentEvent::Done {
                status: DoneStatus::Errored,
                error: Some(_),
                ..
            })
        ));
    }

    #[tokio::test]
    async fn normal_replies_and_following_turns_are_not_failed() {
        let text = "Google may say Verify your account to continue. This is an explanation.";
        let input = [
            AgentEvent::TextDelta {
                text: REJECTION.into(),
            },
            done(),
            AgentEvent::TextDelta { text: text.into() },
            done(),
        ];
        let output = normalize(stream::iter(input.into_iter().map(Ok)).boxed(), None)
            .collect::<Vec<_>>()
            .await;
        assert!(matches!(&output[2], Ok(AgentEvent::TextDelta { text: actual }) if actual == text));
        assert!(matches!(
            &output[3],
            Ok(AgentEvent::Done {
                status: DoneStatus::Completed,
                error: None,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn rpc_rejections_get_the_same_guidance_without_overriding_interrupts() {
        let input = [
            AgentEvent::Done {
                status: DoneStatus::Errored,
                result: None,
                error: Some(REJECTION.into()),
                session_id: None,
            },
            AgentEvent::TextDelta {
                text: REJECTION.into(),
            },
            AgentEvent::Done {
                status: DoneStatus::Interrupted,
                result: None,
                error: None,
                session_id: None,
            },
        ];
        let output = normalize(
            stream::iter(input.into_iter().map(Ok)).boxed(),
            Some("Work".into()),
        )
        .collect::<Vec<_>>()
        .await;
        assert!(
            matches!(&output[0], Ok(AgentEvent::Done { status: DoneStatus::Errored, error: Some(message), .. }) if message.contains("Work") && message.contains("Open Antigravity"))
        );
        assert!(matches!(
            &output[2],
            Ok(AgentEvent::Done {
                status: DoneStatus::Interrupted,
                error: None,
                ..
            })
        ));
    }
}
