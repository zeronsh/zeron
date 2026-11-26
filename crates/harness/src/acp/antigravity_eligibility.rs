//! antigravity refuses an account google hasn't cleared for it with a bare
//! "request failed (code 403): Verify your account to continue." in the reply
//! — no link, and no hint that the gemini web app on the same account works
//! because it doesn't ask. google's code assist says why, and where to verify,
//! in `loadCodeAssist`'s `ineligibleTiers` (`VALIDATION_REQUIRED`, with a
//! `validationUrl`): so a turn whose reply says so is followed by an error that
//! does, asked with the token antigravity runs on.

use std::path::Path;
use std::time::Duration;

use futures::StreamExt;
use futures::stream::{self, BoxStream};
use serde_json::Value;
use zeron_proto::AgentEvent;

use crate::HarnessError;

/// what antigravity's reply says when google wants the account verified.
const REFUSAL: &str = "Verify your account to continue";

const LOAD_CODE_ASSIST: &str = "https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist";

/// why google won't run antigravity on the account, and where it's fixed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Ineligible {
    pub reason: String,
    pub url: String,
}

/// the first tier `loadCodeAssist` names the account ineligible for that a
/// visit to google fixes (`validationUrl`).
pub(crate) fn ineligible(reply: &Value) -> Option<Ineligible> {
    reply.get("ineligibleTiers")?.as_array()?.iter().find_map(|tier| {
        let url = tier.get("validationUrl")?.as_str()?.trim();
        if !url.starts_with("https://") {
            return None;
        }
        let reason = tier
            .get("reasonMessage")
            .or_else(|| tier.get("validationErrorMessage"))
            .and_then(Value::as_str)
            .unwrap_or(REFUSAL)
            .trim()
            .to_owned();
        Some(Ineligible { reason, url: url.to_owned() })
    })
}

/// the error a refused turn gets: google's own reason and its link.
pub(crate) fn message(ineligible: &Ineligible) -> String {
    format!(
        "Google hasn't cleared this account for Antigravity yet (Gemini on the web doesn't ask): {} \
         Verify it at {}, then send again.",
        ineligible.reason.replace("\n\n", " "),
        ineligible.url
    )
}

/// the access token antigravity runs on, when it keeps it as a file in
/// `home` (a cloud machine's, a sign-in zeron made): checks the unified CLI's
/// `antigravity-oauth-token` (`token.access_token`) first, then falls back to
/// legacy `acp_token.json`'s `token`.
fn token_in(home: &Path) -> Option<String> {
    let dir = home.join("antigravity-acp");
    if let Ok(bytes) = std::fs::read(dir.join("antigravity-oauth-token"))
        && let Ok(doc) = serde_json::from_slice::<Value>(&bytes)
        && let Some(token) = doc
            .get("token")
            .and_then(|t| t.get("access_token"))
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
    {
        return Some(token.to_owned());
    }
    let bytes = std::fs::read(dir.join("acp_token.json")).ok()?;
    let token = serde_json::from_slice::<Value>(&bytes).ok()?;
    token
        .get("token")?
        .as_str()
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
}

async fn ask(token: &str) -> Option<Ineligible> {
    let reply = reqwest::Client::new()
        .post(LOAD_CODE_ASSIST)
        .bearer_auth(token)
        .json(&serde_json::json!({ "metadata": { "ideType": "ANTIGRAVITY", "pluginType": "GEMINI" } }))
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .ok()?
        .json::<Value>()
        .await
        .ok()?;
    ineligible(&reply)
}

/// `events`, with an error after any turn whose reply says google wants the
/// account verified, naming why and where (`home`: antigravity's home).
pub(crate) fn explain_refusals(
    events: BoxStream<'static, Result<AgentEvent, HarnessError>>,
    home: Option<std::path::PathBuf>,
) -> BoxStream<'static, Result<AgentEvent, HarnessError>> {
    stream::unfold(
        (events, String::new(), false),
        move |(mut events, mut turn_text, mut said)| {
            let home = home.clone();
            async move {
                let event = events.next().await?;
                let mut out = vec![];
                match &event {
                    Ok(AgentEvent::TextDelta { text }) => {
                        // (only enough of the turn to find the refusal in.)
                        if turn_text.len() < 4096 {
                            turn_text.push_str(text);
                        }
                    }
                    Ok(AgentEvent::Done { .. }) => {
                        if turn_text.contains(REFUSAL) && !said {
                            let token = home.as_deref().and_then(token_in);
                            if let Some(found) = match token {
                                Some(token) => ask(&token).await,
                                None => None,
                            } {
                                said = true;
                                out.push(Ok(AgentEvent::Error { message: message(&found) }));
                            }
                        }
                        turn_text.clear();
                    }
                    _ => {}
                }
                out.push(event);
                Some((stream::iter(out), (events, turn_text, said)))
            }
        },
    )
    .flatten()
    .boxed()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `loadCodeAssist` for an account google wants verified (live reply,
    /// shortened): the free tier ineligible, with its reason and link.
    #[test]
    fn google_names_why_and_where() {
        let reply = serde_json::json!({
            "currentTier": { "id": "standard-tier", "name": "Antigravity" },
            "ineligibleTiers": [{
                "reasonCode": "VALIDATION_REQUIRED",
                "reasonMessage": "Your current account is not eligible for Antigravity. Verify your account to continue.\n\nAlternatively, try signing in with another personal Google account.",
                "tierId": "free-tier",
                "validationErrorMessage": "Verify your account to continue.",
                "validationUrl": "https://accounts.google.com/v3/signin/continue?plt=x&flowName=GlifWebSignIn"
            }]
        });
        let found = ineligible(&reply).unwrap();
        assert_eq!(found.url, "https://accounts.google.com/v3/signin/continue?plt=x&flowName=GlifWebSignIn");
        let said = message(&found);
        assert!(said.contains("not eligible for Antigravity") && said.contains(&found.url), "{said}");
        assert!(!said.contains("\n\n"));
        assert_eq!(ineligible(&serde_json::json!({ "currentTier": {} })), None);
        let no_link = serde_json::json!({ "ineligibleTiers": [{ "reasonCode": "INELIGIBLE_ACCOUNT", "reasonMessage": "No." }] });
        assert_eq!(ineligible(&no_link), None, "nothing to send the person to");
    }

    /// a turn without the refusal passes as it came; antigravity keeping its
    /// token where zeron can't read it (the keychain) adds nothing either.
    #[tokio::test]
    async fn only_a_refused_turn_with_a_readable_token_is_explained() {
        let done = || Ok(AgentEvent::Done { status: zeron_proto::DoneStatus::Completed, result: None, error: None, session_id: None });
        let text = |t: &str| Ok(AgentEvent::TextDelta { text: t.into() });
        let collect = |events: Vec<Result<AgentEvent, HarnessError>>, home| async move {
            explain_refusals(stream::iter(events).boxed(), home).collect::<Vec<_>>().await
        };
        let plain = collect(vec![text("OK"), done()], None).await;
        assert_eq!(plain.len(), 2);
        let refused = collect(vec![text("request failed (code 403): Verify your account to continue."), done()], None).await;
        assert_eq!(refused.len(), 2, "no token to ask google with");
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(token_in(empty.path()), None);
        std::fs::create_dir_all(empty.path().join("antigravity-acp")).unwrap();
        std::fs::write(
            empty.path().join("antigravity-acp/acp_token.json"),
            r#"{"token":"ya29.legacy","refresh_token":""}"#,
        )
        .unwrap();
        assert_eq!(token_in(empty.path()).as_deref(), Some("ya29.legacy"));
        std::fs::write(
            empty.path().join("antigravity-acp/antigravity-oauth-token"),
            r#"{"token":{"access_token":"ya29.unified","token_type":"Bearer"},"auth_method":"consumer"}"#,
        )
        .unwrap();
        assert_eq!(token_in(empty.path()).as_deref(), Some("ya29.unified"));
    }
}
