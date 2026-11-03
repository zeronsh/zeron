//! What a run's error means for the person reading it: the provider needs a
//! sign-in, its usage limit was reached, or something else. Both the desktop
//! and the phone read the same error text the harnesses and engine write, and
//! offer the same fix (signing in to that provider) for the first two.

/// A provider-side reason a turn couldn't run that signing in can fix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderProblem {
    /// Not signed in, signed out, or a dead/expired login.
    SignedOut,
    /// The account's usage or rate limit was hit (another account helps).
    LimitReached,
}

impl ProviderProblem {
    /// The button that fixes it.
    pub fn action_label(self) -> &'static str {
        "Log in"
    }
}

const SIGNED_OUT: &[&str] = &[
    "isn't signed in",
    "is not signed in",
    "not signed in",
    "not logged in",
    "please run /login",
    "run `claude login`",
    "run codex login",
    "authentication failed",
    "authentication_error",
    "sign in to claude again",
    "sign in again",
    "log in again",
    "invalid api key",
    "invalid x-api-key",
    "oauth token has expired",
    "token has expired",
    "token expired",
    "refresh token",
    "unauthorized",
    "http 401",
    "status 401",
    "(401)",
    "401 ",
    "logged out",
];

const LIMIT_REACHED: &[&str] = &[
    "usage limit",
    "limit reached",
    "rate limit",
    "rate-limit",
    "you've hit your",
    "you have hit your",
    "hit your limit",
    "quota",
    "http 429",
    "status 429",
    "(429)",
    "too many requests",
];

/// The problem `message` reports, if signing in can fix it.
pub fn provider_problem(message: &str) -> Option<ProviderProblem> {
    let text = message.to_ascii_lowercase();
    // Cloud credits are Zeron's, not the provider's: not a sign-in.
    if text.contains("out of credits") || text.contains("cloud credits") {
        return None;
    }
    if SIGNED_OUT.iter().any(|needle| text.contains(needle)) {
        return Some(ProviderProblem::SignedOut);
    }
    if LIMIT_REACHED.iter().any(|needle| text.contains(needle)) {
        return Some(ProviderProblem::LimitReached);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_errors_the_harnesses_and_engine_write() {
        for signed_out in [
            "Claude Code isn't signed in on Cloud. Sign in with Claude, pick an account or add an Anthropic API key in Settings → Providers → Cloud, then send again.",
            "Codex isn't signed in on Cloud. Sign in with ChatGPT, pick an account or add an OpenAI API key in Settings → Providers → Cloud, then send again.",
            "Authentication failed — sign in to Claude again.",
            "Invalid API key · Please run /login",
            "OAuth token has expired. Please obtain a new token or refresh your existing token.",
            "Not logged in · Please run /login",
        ] {
            assert_eq!(
                provider_problem(signed_out),
                Some(ProviderProblem::SignedOut),
                "{signed_out}"
            );
        }
        for limited in [
            "Claude usage limit reached — try again after the limit resets.",
            "Claude 5-hour limit reached — the turn was blocked. Try again after it resets.",
            "You've hit your usage limit. Upgrade to Pro or try again later.",
            "429 Too Many Requests",
        ] {
            assert_eq!(
                provider_problem(limited),
                Some(ProviderProblem::LimitReached),
                "{limited}"
            );
        }
        for other in [
            "Claude is overloaded right now — try again shortly.",
            "The selected model isn't available.",
            "You're out of Cloud credits. Add credits to keep going.",
            "Harness exited with status 1",
            "Listening on port 4012",
        ] {
            assert_eq!(provider_problem(other), None, "{other}");
        }
    }
}
