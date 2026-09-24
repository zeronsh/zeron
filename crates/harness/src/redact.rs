//! Redaction for CLI / agent output that may reach a log line or the UI.
//!
//! Sign-in flows surface what an agent printed (its last line on failure, its
//! stderr in debug logs). That text can carry authorization urls (with
//! `state`, PKCE challenges, codes), device/user codes, and — from a buggy or
//! verbose CLI — tokens. [`redact_output`] keeps the human part ("Device code
//! request failed: 503") and replaces the rest. (Crash diagnostics use the
//! narrower credential-marker redaction in the crate root.)

/// `text` with terminal escapes stripped and every secret-shaped token
/// replaced:
/// - url query strings and fragments (`?state=…`, `#code=…`) and userinfo;
/// - device / user codes (`ABCD-1234`);
/// - `key=value` / `"key":"value"` pairs whose key names a secret (token,
///   secret, password, key, code, state, verifier, session, cookie, auth);
/// - JWTs (`eyJ…`), vendor-prefixed keys (`sk-…`, `ghp_…`, `gho_…`, `xai-…`,
///   …), the value after `Bearer`, and long base64 / hex runs.
pub fn redact_output(text: &str) -> String {
    let text = strip_ansi(text);
    let mut out = String::with_capacity(text.len());
    let mut previous = String::new();
    let mut word = String::new();
    let flush = |word: &mut String, previous: &mut String, out: &mut String| {
        if word.is_empty() {
            return;
        }
        out.push_str(&redact_word(word, previous));
        *previous = word.to_ascii_lowercase();
        word.clear();
    };
    for c in text.chars() {
        if c.is_whitespace() {
            flush(&mut word, &mut previous, &mut out);
            out.push(c);
        } else {
            word.push(c);
        }
    }
    flush(&mut word, &mut previous, &mut out);
    out
}

const REDACTED: &str = "[redacted]";

/// Key names whose values are secrets.
const SECRET_KEYS: &[&str] = &[
    "token",
    "secret",
    "password",
    "passwd",
    "key",
    "code",
    "state",
    "verifier",
    "session",
    "cookie",
    "auth",
    "credential",
    "otp",
];

/// Vendor key prefixes (the value follows the prefix).
const SECRET_PREFIXES: &[&str] = &[
    "sk-",
    "sk_",
    "pk_",
    "rk_",
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "ghr_",
    "github_pat_",
    "xai-",
    "xoxb-",
    "xoxp-",
    "glpat-",
    "AKIA",
    "ya29.",
];

fn redact_word(word: &str, previous: &str) -> String {
    const LEAD: &[char] = &['"', '\'', '(', '[', '<', '{', '`'];
    const TRAIL: &[char] = &[
        '"', '\'', '.', ',', ';', ':', ')', ']', '}', '>', '`', '!', '?',
    ];
    let core_start = word.len() - word.trim_start_matches(LEAD).len();
    let trimmed = &word[core_start..];
    let core = trimmed.trim_end_matches(TRAIL);
    let (lead, trail) = (&word[..core_start], &trimmed[core.len()..]);
    if core.is_empty() {
        return word.to_string();
    }
    let prev = previous.trim_matches(|c: char| !c.is_ascii_alphanumeric());
    let redacted = if prev == "bearer" || prev == "basic" {
        REDACTED.to_string()
    } else {
        redact_core(core)
    };
    format!("{lead}{redacted}{trail}")
}

fn redact_core(core: &str) -> String {
    if let Some(scheme_end) = core.find("://") {
        return redact_url(core, scheme_end);
    }
    // key=value, key:value, "key":"value" — keep the key, drop a secret value.
    for sep in ['=', ':'] {
        if let Some(at) = core.find(sep) {
            let key = core[..at].trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_');
            let value = &core[at + 1..];
            let lower = key.to_ascii_lowercase();
            if !value.is_empty() && SECRET_KEYS.iter().any(|k| lower.contains(k)) {
                return format!("{}{sep}{REDACTED}", &core[..at]);
            }
        }
    }
    if looks_like_secret(core) {
        return REDACTED.to_string();
    }
    if looks_like_device_code(core) {
        return "[code]".to_string();
    }
    core.to_string()
}

/// A url without its userinfo, query and fragment: the page is kept (it
/// tells the user where the sign-in went), the parameters are not.
fn redact_url(url: &str, scheme_end: usize) -> String {
    let (scheme, rest) = url.split_at(scheme_end + 3);
    let cut = rest.find(['?', '#']).unwrap_or(rest.len());
    let (base, tail) = rest.split_at(cut);
    let authority_end = base.find('/').unwrap_or(base.len());
    let (authority, path) = base.split_at(authority_end);
    let authority = match authority.rsplit_once('@') {
        Some((_, host)) => format!("{REDACTED}@{host}"),
        None => authority.to_string(),
    };
    let path = path
        .split('/')
        .map(|segment| {
            if looks_like_secret(segment) || looks_like_device_code(segment) {
                REDACTED
            } else {
                segment
            }
        })
        .collect::<Vec<_>>()
        .join("/");
    let tail = if tail.is_empty() { "" } else { "?…" };
    format!("{scheme}{authority}{path}{tail}")
}

fn looks_like_secret(token: &str) -> bool {
    if token.starts_with("eyJ") && token.contains('.') && token.len() > 20 {
        return true;
    }
    if SECRET_PREFIXES
        .iter()
        .any(|p| token.starts_with(p) && token.len() >= p.len() + 8)
    {
        return true;
    }
    // A file path is not a secret (and is what makes an error actionable).
    if token.starts_with(['/', '~', '.']) || token.matches('/').count() >= 3 {
        return false;
    }
    // Long opaque runs: base64/base64url/hex of 32+ chars with letters AND
    // digits (a long english word doesn't qualify).
    token.len() >= 32
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '_' | '-' | '.'))
        && token.chars().any(|c| c.is_ascii_digit())
        && token.chars().any(|c| c.is_ascii_alphabetic())
        && !token.contains("..")
}

/// `ABCD-1234`, `WXYZ-98765`: two or more dash-joined groups of 4+
/// uppercase letters/digits, with at least one digit.
fn looks_like_device_code(token: &str) -> bool {
    let groups: Vec<&str> = token.split('-').collect();
    groups.len() >= 2
        && groups.iter().all(|g| {
            g.len() >= 4
                && g.chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        })
        && token.chars().any(|c| c.is_ascii_digit())
}

/// Terminal escape sequences (colours, cursor moves, OSC hyperlinks) removed.
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('[') => {
                chars.next();
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                chars.next();
                while let Some(c) = chars.next() {
                    if c == '\u{7}' {
                        break;
                    }
                    if c == '\u{1b}' {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {
                chars.next();
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_failure_text_survives() {
        for text in [
            "Device code request failed: 503",
            "error: the sign-in was declined",
            "Port 1455 is in use",
            "Could not open browser for authentication",
        ] {
            assert_eq!(redact_output(text), text);
        }
    }

    #[test]
    fn urls_keep_their_page_but_lose_parameters_and_userinfo() {
        assert_eq!(
            redact_output(
                "Open https://auth.openai.com/oauth/authorize?client_id=x&state=abc&code_challenge=y now"
            ),
            "Open https://auth.openai.com/oauth/authorize?… now"
        );
        assert_eq!(
            redact_output("(see https://user:pass@example.com/cb#access_token=zzz)."),
            "(see https://[redacted]@example.com/cb?…)."
        );
        assert_eq!(
            redact_output("https://github.com/login/device"),
            "https://github.com/login/device"
        );
    }

    #[test]
    fn codes_and_tokens_are_replaced() {
        assert_eq!(redact_output("Code: WXYZ-9876"), "Code: [code]");
        assert_eq!(
            redact_output("enter code: ABCD-1234 at the page"),
            "enter code: [code] at the page"
        );
        assert_eq!(
            redact_output("got eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.sig back"),
            "got [redacted] back"
        );
        assert_eq!(
            redact_output("Authorization: Bearer abc.def-123"),
            "Authorization: Bearer [redacted]"
        );
        assert_eq!(
            redact_output("token gho_16C7e42F292c6912E7710c838347Ae178B4a"),
            "token [redacted]"
        );
        assert_eq!(
            redact_output("key sk-ant-api03-abcdefghijklmnop"),
            "key [redacted]"
        );
        assert_eq!(
            redact_output("refresh_token=r1 state=s2 ok"),
            "refresh_token=[redacted] state=[redacted] ok"
        );
        assert_eq!(
            redact_output(r#"{"access_token":"abc","expires_in":3600}"#),
            r#"{"access_token":[redacted]"#.to_string() + r#"}"#
        );
        assert_eq!(
            redact_output("digest 3f786850e387550fdab836ed7e6dc881de23001b"),
            "digest [redacted]"
        );
        // Paths stay readable.
        let path = "/tmp/zeron/data/agent-accounts/.login-0123456789abcdef/auth.json";
        assert_eq!(redact_output(path), path);
    }

    #[test]
    fn escapes_are_stripped_first() {
        assert_eq!(redact_output("\u{1b}[94mQRST-5678\u{1b}[0m"), "[code]");
        assert_eq!(
            strip_ansi("\u{1b}]8;;https://a\u{7}link\u{1b}]8;;\u{7} \u{1b}[1mbold\u{1b}[0m"),
            "link bold"
        );
    }
}
