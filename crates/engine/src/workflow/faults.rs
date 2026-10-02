//! Provider faults: what a failed child turn means for the run.
//!
//! A child turn that ends in an error carries the harness's message. The
//! scheduler turns it into one of:
//!
//! * [`Fault::RateLimited`] / [`Fault::Transient`] — retried with backoff, never
//!   shown to the script;
//! * [`Fault::Auth`] / [`Fault::Quota`] / [`Fault::ModelUnavailable`] — retrying
//!   cannot help: the run stops as `stopped(provider)` and is resumable;
//! * [`Fault::Other`] — the ask fails with a value the script can branch on.
//!
//! Classification is by message text — harnesses report errors as prose — so
//! it is deliberately conservative: anything unrecognised is `Other` (visible
//! to the script), never silently retried.

use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fault {
    RateLimited { retry_after: Option<Duration> },
    Transient,
    Auth,
    Quota,
    ModelUnavailable,
    Other,
}

impl Fault {
    /// Worth redriving after a pause.
    pub fn is_retryable(&self) -> bool {
        matches!(self, Fault::RateLimited { .. } | Fault::Transient)
    }

    /// Retrying cannot help; the run stops.
    pub fn is_deterministic(&self) -> bool {
        matches!(self, Fault::Auth | Fault::Quota | Fault::ModelUnavailable)
    }

    pub fn label(&self) -> &'static str {
        match self {
            Fault::RateLimited { .. } => "rate limited",
            Fault::Transient => "temporary provider error",
            Fault::Auth => "authentication failed",
            Fault::Quota => "quota or billing limit reached",
            Fault::ModelUnavailable => "model unavailable",
            Fault::Other => "error",
        }
    }
}

pub fn classify(message: &str) -> Fault {
    let m = message.to_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| m.contains(n));
    // Money first: "429 … quota exceeded" is not worth retrying.
    if has(&[
        "insufficient_quota",
        "quota",
        "billing",
        "credit balance",
        "out of credits",
        "usage limit",
        "spend limit",
        "plan limit",
    ]) {
        return Fault::Quota;
    }
    if has(&[
        "model_not_found",
        "model not found",
        "unknown model",
        "unsupported model",
        "model is not available",
        "model does not exist",
        "the model `",
        "invalid model",
    ]) {
        return Fault::ModelUnavailable;
    }
    if has(&[
        "invalid api key",
        "invalid x-api-key",
        "incorrect api key",
        "unauthorized",
        "authentication",
        "not logged in",
        "login required",
        "please log in",
        "please run /login",
        "401",
        "403",
        "permission_error",
    ]) {
        return Fault::Auth;
    }
    if has(&[
        "rate limit",
        "rate_limit",
        "ratelimit",
        "too many requests",
        "429",
        "overloaded",
        "529",
    ]) {
        return Fault::RateLimited {
            retry_after: parse_retry_after(&m),
        };
    }
    if has(&[
        "timeout",
        "timed out",
        "connection reset",
        "connection refused",
        "connection closed",
        "econnreset",
        "network",
        "stream disconnected",
        "stream closed",
        "broken pipe",
        "unexpected eof",
        "500",
        "502",
        "503",
        "504",
        "internal server error",
        "bad gateway",
        "service unavailable",
        "gateway timeout",
        "temporarily",
        "try again",
    ]) {
        return Fault::Transient;
    }
    Fault::Other
}

/// `retry-after: 30`, `retry after 12s`, `try again in 2 minutes`.
fn parse_retry_after(lower: &str) -> Option<Duration> {
    for marker in ["retry-after", "retry after", "try again in", "retry in"] {
        let Some(at) = lower.find(marker) else {
            continue;
        };
        let rest = lower[at + marker.len()..].trim_start_matches([':', ' ', '=']);
        let digits: String = rest
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        let Ok(n) = digits.parse::<f64>() else {
            continue;
        };
        let unit = rest[digits.len()..].trim_start();
        let seconds = if unit.starts_with("ms") || unit.starts_with("millisecond") {
            n / 1000.0
        } else if unit.starts_with("min") || unit.starts_with('m') && !unit.starts_with("ms") {
            n * 60.0
        } else if unit.starts_with('h') {
            n * 3600.0
        } else {
            n
        };
        return Some(Duration::from_secs_f64(seconds.clamp(0.0, 3600.0)));
    }
    None
}

/// Backoff before redrive `attempt` (0-based): 2 s doubling to 60 s, ±25 %
/// jitter derived from `salt` (so tests are deterministic and a herd of
/// asks spreads out). A server-provided `retry_after` wins when longer.
pub fn backoff(attempt: u32, salt: u64, retry_after: Option<Duration>) -> Duration {
    let base_ms = (2_000u64.saturating_mul(1 << attempt.min(10))).min(60_000);
    // splitmix64-style scramble of (salt, attempt) → [-0.25, +0.25].
    let mut x = salt.wrapping_add((attempt as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^= x >> 31;
    let unit = (x % 10_000) as f64 / 10_000.0; // 0..1
    let jitter = 1.0 + (unit - 0.5) * 0.5;
    let jittered = Duration::from_millis((base_ms as f64 * jitter) as u64);
    match retry_after {
        Some(server) => jittered.max(server.min(Duration::from_secs(600))),
        None => jittered,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn faults_are_classified_by_message() {
        use Fault::*;
        assert_eq!(
            classify("Error: 429 Too Many Requests"),
            RateLimited { retry_after: None }
        );
        assert_eq!(
            classify("rate limit reached; retry-after: 12"),
            RateLimited {
                retry_after: Some(Duration::from_secs(12))
            }
        );
        assert_eq!(
            classify("Overloaded. Please try again in 2 minutes"),
            RateLimited {
                retry_after: Some(Duration::from_secs(120))
            }
        );
        assert_eq!(classify("connection reset by peer"), Transient);
        assert_eq!(classify("503 Service Unavailable"), Transient);
        assert_eq!(
            classify("You exceeded your current quota, check billing"),
            Quota
        );
        assert_eq!(
            classify("429: insufficient_quota"),
            Quota,
            "money beats rate limit"
        );
        assert_eq!(classify("401 Unauthorized: invalid API key"), Auth);
        assert_eq!(
            classify("The model `gpt-9` does not exist"),
            ModelUnavailable
        );
        assert_eq!(classify("the schema was wrong"), Other);
        assert_eq!(classify("the child's turn was interrupted"), Other);
        assert!(Quota.is_deterministic() && !Quota.is_retryable());
        assert!(Transient.is_retryable() && !Transient.is_deterministic());
    }

    #[test]
    fn backoff_doubles_to_a_minute_with_bounded_jitter() {
        let at = |a| backoff(a, 7, None).as_millis() as f64;
        for (attempt, base) in [
            (0, 2000.0),
            (1, 4000.0),
            (2, 8000.0),
            (3, 16000.0),
            (4, 32000.0),
            (5, 60000.0),
            (9, 60000.0),
        ] {
            let d = at(attempt);
            assert!(
                d >= base * 0.75 && d <= base * 1.25,
                "attempt {attempt}: {d}"
            );
        }
        // Deterministic per salt, different across salts.
        assert_eq!(backoff(2, 7, None), backoff(2, 7, None));
        assert_ne!(backoff(2, 7, None), backoff(2, 8, None));
        // A longer Retry-After wins; a shorter one does not shorten the backoff.
        assert_eq!(
            backoff(0, 1, Some(Duration::from_secs(90))),
            Duration::from_secs(90)
        );
        assert!(backoff(5, 1, Some(Duration::from_secs(1))) >= Duration::from_secs(45));
        // …but a hostile Retry-After is capped.
        assert_eq!(
            backoff(0, 1, Some(Duration::from_secs(99_999))),
            Duration::from_secs(600)
        );
    }
}
