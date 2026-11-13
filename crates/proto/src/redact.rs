//! Redaction for CLI / agent output that may reach a log line or the UI.
//!
//! Sign-in flows surface what an agent printed (its last line on failure, its
//! stderr in debug logs). That text can carry authorization urls (with
//! `state`, PKCE challenges, codes), device/user codes, and — from a buggy or
//! verbose CLI — tokens. [`redact_output`] keeps the human part ("Device code
//! request failed: 503") and replaces the rest. The doc fold runs a
//! command's output tail through it before the tail enters a chat's doc
//! (zeron-doc `command_output_tail`). (Harness crash diagnostics use the
//! narrower credential-marker redaction in zeron-harness's crate root.)

/// `text` with terminal escapes stripped and every secret-shaped token
/// replaced:
/// - url query strings and fragments (`?state=…`, `#code=…`) and userinfo;
/// - device / user codes (`ABCD-1234`);
/// - the value after a label naming a secret (`token: …`, `state=…`,
///   `"password": "…"`, OpenCode / Pi's `"access"` / `"refresh"`), whatever
///   it looks like — a quoted value through its closing quote;
/// - an `Authorization` / `Proxy-Authorization` value through end-of-line
///   (only its scheme word — `Bearer`, `Digest`, … — stays);
/// - JWTs (`eyJ…`), vendor-prefixed keys (`sk-…`, `ghp_…`, `gho_…`, `xai-…`,
///   …), the value after `Bearer`, and long base64 / hex runs.
pub fn redact_output(text: &str) -> String {
    redact_with(&strip_ansi(text), Redactor::default())
}

/// [`redact_output`], hardened for a command's output, which persists in a
/// chat's doc (synced to the person's other devices and the cloud) where a
/// sign-in error is only shown:
/// - private key blocks (`-----BEGIN … PRIVATE KEY-----` … `-----END …`,
///   PEM and OpenSSH) are dropped whole, an unterminated one to the end;
/// - an env-style assignment (`AWS_SECRET_ACCESS_KEY=…`, `export PATH=…`:
///   an uppercase `KEY=`) hides its value, whatever the key;
/// - keys naming a password, private key or webhook (`DB_PASS`, `pwd`,
///   `private_key`, `slack_webhook`) are secret labels too;
/// - (as everywhere) url userinfo and queries, webhook urls' paths
///   ([`WEBHOOK_HOSTS`]), JWTs, vendor keys and long opaque runs.
///
/// A command that prints secrets by nature keeps none of its output: see
/// [`command_prints_secrets`].
pub fn redact_command_output(text: &str) -> String {
    let text = drop_private_keys(&strip_ansi(text));
    redact_with(
        &text,
        Redactor {
            command: true,
            ..Redactor::default()
        },
    )
}

/// What a command's output is replaced with when the command prints
/// secrets by nature ([`command_prints_secrets`]).
pub const SECRET_OUTPUT_WITHHELD: &str = "(output not kept: may contain secrets)";

fn redact_with(text: &str, mut redactor: Redactor) -> String {
    let mut word = String::new();
    for c in text.chars() {
        if c.is_whitespace() {
            if !word.is_empty() {
                redactor.word(&word);
                word.clear();
            }
            redactor.space(c);
        } else {
            word.push(c);
        }
    }
    if !word.is_empty() {
        redactor.word(&word);
    }
    redactor.finish()
}

/// What follows a label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Label {
    /// An ordinary secret (`token`, `password`, `access`, …): its value is
    /// hidden whatever it looks like — `password: bearer` included.
    Secret,
    /// `Authorization` / `Proxy-Authorization`: the scheme word may stay;
    /// every parameter and credential after it is hidden.
    Authorization,
}

/// Where the redactor is within a line.
#[derive(Debug, Clone, Copy, Default)]
enum State {
    #[default]
    Text,
    /// A bare label (`state`, `"password"`, `Authorization`) whose `=` / `:`
    /// may come as the next word. Ends with the line.
    Separator(Label),
    /// The separator has been seen: the next word is the value.
    Value(Label),
    /// Inside a hidden value — through `quote`'s closing quote when it was
    /// quoted, else through end-of-line. `masked` once its `[redacted]` is
    /// written.
    Hidden { quote: Option<char>, masked: bool },
}

#[derive(Default)]
struct Redactor {
    out: String,
    /// Whitespace not yet written: dropped when it falls inside a hidden
    /// value, so `"correct horse battery staple"` becomes `"[redacted]"`.
    space: String,
    /// The previous whole word, lowercased.
    previous: String,
    state: State,
    /// A command's output ([`redact_command_output`]): env-style
    /// assignments and the wider set of secret keys are hidden too.
    command: bool,
}

impl Redactor {
    fn space(&mut self, c: char) {
        // Line-scoped states end with the line. A pending value (`token:` at
        // the end of a line) still hides the next word.
        if c == '\n' && matches!(self.state, State::Separator(_) | State::Hidden { .. }) {
            self.state = State::Text;
        }
        self.space.push(c);
    }

    fn emit(&mut self, text: &str) {
        self.out.push_str(&self.space);
        self.space.clear();
        self.out.push_str(text);
    }

    fn finish(mut self) -> String {
        self.out.push_str(&self.space);
        self.out
    }

    fn word(&mut self, word: &str) {
        self.segment(word);
        self.previous = word.to_ascii_lowercase();
    }

    /// `part` — a word, or what's left of one — in the current state.
    fn segment(&mut self, part: &str) {
        if part.is_empty() {
            return;
        }
        match self.state {
            State::Text => self.text(part),
            State::Separator(label) => match strip_separator(part) {
                Some(rest) => {
                    self.emit(&part[..part.len() - rest.len()]);
                    self.state = State::Value(label);
                    self.segment(rest);
                }
                None => {
                    self.state = State::Text;
                    self.text(part);
                }
            },
            State::Value(label) => {
                if matches!(part, "=" | ":" | "=>") {
                    self.emit(part);
                    return;
                }
                self.state = State::Text;
                match label {
                    Label::Secret => self.secret_value(part),
                    Label::Authorization => self.authorization_value(part),
                }
            }
            State::Hidden { quote, masked } => self.hidden(part, quote, masked),
        }
    }

    /// A word outside any value.
    fn text(&mut self, part: &str) {
        if let Some(parts) = parse_label(part) {
            let kind = label_kind(parts.key, &self.previous)
                .or_else(|| {
                    (self.command && command_secret_key(parts.key)).then_some(Label::Secret)
                })
                .or_else(|| {
                    // `KEY=value`: an environment variable's value, hidden
                    // whatever its name (only with `=` attached: `ERROR:`
                    // is a heading, not a key).
                    let assigned = parts.rest.is_some_and(|rest| {
                        part[..part.len() - rest.len()].ends_with('=') && !rest.is_empty()
                    });
                    (self.command && assigned && is_env_key(parts.key)).then_some(Label::Secret)
                });
            match (kind, parts.rest) {
                // `token`, `"password"`: the separator may follow.
                (Some(label), None) => {
                    let word = redact_word(part, &self.previous);
                    self.emit(&word);
                    self.state = State::Separator(label);
                    return;
                }
                // `token:`, `password=hunter2`, `Authorization:Bearer`.
                (Some(label), Some(rest)) => {
                    self.emit(&part[..part.len() - rest.len()]);
                    self.state = State::Value(label);
                    self.segment(rest);
                    return;
                }
                // `expires_in:3600`, `code:503` after `status`: keep the
                // key, read what follows it on its own.
                (None, Some(rest)) if !rest.is_empty() => {
                    self.emit(&part[..part.len() - rest.len()]);
                    self.segment(rest);
                    return;
                }
                _ => {}
            }
        }
        // A quoted item with more after it (`"x","token":"y"}`).
        if let Some(end) = quoted_item_end(part)
            && end < part.len()
        {
            let head = redact_word(&part[..end], &self.previous);
            self.emit(&head);
            self.segment(&part[end..]);
            return;
        }
        // Compact lists (`a=1,token=2`); a url keeps its commas.
        if !part.contains("://")
            && let Some(comma) = part.find(',')
            && comma + 1 < part.len()
        {
            self.segment(&part[..=comma]);
            self.segment(&part[comma + 1..]);
            return;
        }
        let word = redact_word(part, &self.previous);
        self.emit(&word);
    }

    /// The value after an ordinary secret label: hidden, always.
    fn secret_value(&mut self, part: &str) {
        // A nested object: its own keys are labels.
        if part == "{" {
            self.emit(part);
            return;
        }
        let Some(quote) = opening_quote(part) else {
            self.emit(&redact_labeled_value(part));
            return;
        };
        let inner = &part[1..];
        self.emit(&part[..1]);
        match find_closing(inner, quote) {
            Some(close) => {
                let content = &inner[..close];
                if !content.is_empty() {
                    self.emit(mask(content));
                }
                self.emit(&inner[close..=close]);
                self.segment(&inner[close + 1..]);
            }
            None => {
                if !inner.is_empty() {
                    self.emit(REDACTED);
                }
                self.state = State::Hidden {
                    quote: Some(quote),
                    masked: !inner.is_empty(),
                };
            }
        }
    }

    /// The first word of an `Authorization` value: a scheme word stays, and
    /// everything after it is hidden through end-of-line (or the value's
    /// closing quote).
    fn authorization_value(&mut self, part: &str) {
        let quote = opening_quote(part);
        let body = match quote {
            Some(_) => {
                self.emit(&part[..1]);
                &part[1..]
            }
            None => part,
        };
        if let Some(quote) = quote
            && let Some(close) = find_closing(body, quote)
        {
            if close > 0 {
                self.emit(REDACTED);
            }
            self.emit(&body[close..=close]);
            self.segment(&body[close + 1..]);
            return;
        }
        let masked = if is_auth_scheme(body) {
            self.emit(body);
            false
        } else if body.is_empty() {
            false
        } else {
            self.emit(REDACTED);
            true
        };
        self.state = State::Hidden { quote, masked };
    }

    /// A word inside a hidden value: one `[redacted]` stands for all of it.
    fn hidden(&mut self, part: &str, quote: Option<char>, mut masked: bool) {
        let close = quote.and_then(|quote| find_closing(part, quote));
        let content = &part[..close.unwrap_or(part.len())];
        if masked {
            self.space.clear();
        } else if !content.is_empty() {
            self.emit(REDACTED);
            masked = true;
        }
        match close {
            Some(close) => {
                self.state = State::Text;
                self.emit(&part[close..=close]);
                self.segment(&part[close + 1..]);
            }
            None => self.state = State::Hidden { quote, masked },
        }
    }
}

/// A word that starts with a label: `key`, `key:`, `"key":`, `--key=value`.
struct LabelParts<'a> {
    key: &'a str,
    /// `None` for a bare key (its separator may be the next word), else what
    /// follows the separator (possibly nothing).
    rest: Option<&'a str>,
}

fn parse_label(word: &str) -> Option<LabelParts<'_>> {
    let body = word
        .trim_start_matches(|c: char| matches!(c, '"' | '\'' | '`' | '{' | '[' | '(' | ',' | '-'));
    let quote = word[..word.len() - body.len()]
        .chars()
        .last()
        .filter(|c| is_quote(*c));
    let key_len = body
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')))
        .unwrap_or(body.len());
    let key = &body[..key_len];
    // A token-shaped "key" is a secret, never a label to print.
    if !key.starts_with(|c: char| c.is_ascii_alphabetic())
        || key.len() > 64
        || looks_like_secret(key)
        || looks_like_device_code(key)
    {
        return None;
    }
    let mut after = &body[key_len..];
    if let Some(quote) = quote
        && let Some(unquoted) = after.strip_prefix(quote)
    {
        after = unquoted;
    }
    if after.is_empty() {
        return Some(LabelParts { key, rest: None });
    }
    let rest = strip_separator(after)?;
    // `https://…` is a url, not a label.
    if rest.starts_with("//") {
        return None;
    }
    Some(LabelParts {
        key,
        rest: Some(rest),
    })
}

/// What `key` labels. `access` / `refresh` (OpenCode's and Pi's OAuth
/// fields) match exactly — `accessible:` is not a secret — while the other
/// names match anywhere in the key (`id_token`, `x-api-key`, `passwd`).
fn label_kind(key: &str, previous: &str) -> Option<Label> {
    let key = key.to_ascii_lowercase();
    if matches!(key.as_str(), "authorization" | "proxy-authorization") {
        return Some(Label::Authorization);
    }
    // `status code: 503` is a response code, not a secret.
    let previous = previous.trim_matches(|c: char| !c.is_ascii_alphanumeric());
    if key == "code"
        && matches!(
            previous,
            "status" | "http" | "exit" | "error" | "response" | "return"
        )
    {
        return None;
    }
    (EXACT_SECRET_KEYS.contains(&key.as_str()) || SECRET_KEYS.iter().any(|k| key.contains(k)))
        .then_some(Label::Secret)
}

/// Keys a command's output hides beyond [`SECRET_KEYS`], matched by a whole
/// `_`/`-`/`.` separated word of the key (`DB_PASS`, `pwd`, `private_key`;
/// `passed` and `compass` stay readable) or anywhere for `webhook`.
fn command_secret_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key.contains("webhook")
        || key.contains("passphrase")
        || key.contains("privatekey")
        || key
            .split(['_', '-', '.'])
            .any(|w| matches!(w, "pass" | "pwd" | "passwd" | "private" | "pat" | "dsn"))
}

/// An environment variable's name: `AWS_SECRET_ACCESS_KEY`, `PATH`, `HOME`
/// (uppercase letters, digits and `_`, starting with a letter).
fn is_env_key(key: &str) -> bool {
    key.len() >= 2
        && key.starts_with(|c: char| c.is_ascii_uppercase())
        && key
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// `text` without its private key blocks: from a `-----BEGIN … PRIVATE
/// KEY-----` line through its `-----END …` line, one marker line instead.
/// An unterminated block (the output was cut, or the key printed oddly) is
/// dropped to the end.
fn drop_private_keys(text: &str) -> String {
    if !text.contains("PRIVATE KEY") {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut inside = false;
    for line in text.split_inclusive('\n') {
        let begin = line.contains("-----BEGIN") && line.contains("PRIVATE KEY");
        if !inside && begin {
            inside = true;
            out.push_str("[private key redacted]");
            if line.ends_with('\n') {
                out.push('\n');
            }
            // A key on one line (`…KEY----- MII… -----END …KEY-----`).
            if line.contains("-----END") {
                inside = false;
            }
            continue;
        }
        if inside {
            if line.contains("-----END") && line.contains("PRIVATE KEY") {
                inside = false;
            }
            continue;
        }
        out.push_str(line);
    }
    out
}

/// Programs and invocations whose output is a secret by nature: their
/// output is never kept, not even redacted ([`SECRET_OUTPUT_WITHHELD`]).
/// `command` is the whole command line as the harness gave it — several
/// commands (`a | b`, `a && b`, `a; b`) and a shell wrapper (`bash -lc
/// '…'`, Codex's `/usr/bin/zsh -lc '…'`) are looked through.
pub fn command_prints_secrets(command: &str) -> bool {
    // Quotes go: `zsh -lc 'env'` is `env` (a quoted file name reads the
    // same unquoted).
    let flat: String = command
        .chars()
        .map(|c| {
            if matches!(c, '\'' | '"' | '`' | '(' | ')' | '{' | '}') {
                ' '
            } else {
                c
            }
        })
        .collect();
    flat.split(['|', ';', '&', '\n'])
        .any(|segment| segment_prints_secrets(segment.split_whitespace().collect()))
}

fn segment_prints_secrets(words: Vec<&str>) -> bool {
    let mut words = words.as_slice();
    // What runs another command: `sudo`, `exec`, `time`, `VAR=x`, a shell's
    // `-c` / `-lc`.
    loop {
        let Some((first, rest)) = words.split_first() else {
            return false;
        };
        let name = first.rsplit('/').next().unwrap_or(first);
        let shell = matches!(name, "sh" | "bash" | "zsh" | "dash" | "fish" | "ksh");
        if matches!(
            name,
            "sudo" | "exec" | "command" | "nohup" | "time" | "doas" | "xargs"
        ) || (first.contains('=') && is_env_key(first.split('=').next().unwrap_or("")))
        {
            words = rest;
        } else if shell
            && rest
                .first()
                .is_some_and(|f| f.starts_with('-') && f.contains('c'))
        {
            words = &rest[1..];
        } else {
            break;
        }
    }
    let name = words[0].rsplit('/').next().unwrap_or(words[0]);
    let args = &words[1..];
    let has = |w: &str| args.contains(&w);
    match name {
        "printenv" => true,
        // `env` alone (or with options/assignments only) lists the
        // environment; `env FOO=1 cmd` runs `cmd`.
        "env" => args.iter().all(|a| a.starts_with('-') || a.contains('=')),
        "export" | "declare" | "typeset" => args.is_empty() || has("-p") || has("-x"),
        "set" => args.is_empty(),
        "cat" | "bat" | "less" | "more" | "head" | "tail" | "strings" | "xxd" | "nl" => {
            args.iter().any(|a| secret_file(a))
        }
        "base64" => has("-d") || has("--decode") || has("-D"),
        "gpg" | "gpg2" => has("-d") || has("--decrypt"),
        "security" => args
            .first()
            .is_some_and(|a| matches!(*a, "find-generic-password" | "find-internet-password")),
        "pass" | "gopass" => args.first().is_some_and(|a| matches!(*a, "show" | "cat")),
        "op" => {
            args.first()
                .is_some_and(|a| matches!(*a, "read" | "inject"))
                || (args.first() == Some(&"item") && args.get(1) == Some(&"get"))
        }
        "aws" => {
            args.first() == Some(&"configure")
                && args
                    .get(1)
                    .is_some_and(|a| matches!(*a, "get" | "export-credentials"))
        }
        "gcloud" => args.windows(2).any(|w| {
            w[0] == "auth" && matches!(w[1], "print-access-token" | "print-identity-token")
        }),
        "gh" => args.first() == Some(&"auth") && args.get(1) == Some(&"token"),
        "az" => args
            .windows(2)
            .any(|w| w == ["account", "get-access-token"]),
        _ => false,
    }
}

/// A file whose contents are secrets: `.env*`, `*credentials*`, `*.pem`,
/// `*.key`, an ssh key (`id_*`), `.netrc`, `.pgpass`.
fn secret_file(arg: &str) -> bool {
    if arg.starts_with('-') {
        return false;
    }
    let base = arg.rsplit('/').next().unwrap_or(arg).to_ascii_lowercase();
    base.starts_with(".env")
        || base.contains("credentials")
        || base.ends_with(".pem")
        || base.ends_with(".key")
        || base.starts_with("id_")
        || matches!(base.as_str(), ".netrc" | ".pgpass" | ".npmrc" | ".pypirc")
}

/// Hosts whose url path IS the credential (a webhook's token rides it):
/// only the host is kept.
pub const WEBHOOK_HOSTS: &[&str] = &[
    "hooks.slack.com",
    "discord.com",
    "discordapp.com",
    "ptb.discord.com",
    "canary.discord.com",
    "hooks.zapier.com",
    "outlook.office.com",
    "api.telegram.org",
    "maker.ifttt.com",
    "chat.googleapis.com",
];

/// `host` is a webhook host ([`WEBHOOK_HOSTS`], or a `*.webhook.office.com`).
fn webhook_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    let host = host.split(':').next().unwrap_or(&host);
    WEBHOOK_HOSTS.contains(&host) || host.ends_with(".webhook.office.com")
}

/// `part` after a leading `=>`, `:` or `=`.
fn strip_separator(part: &str) -> Option<&str> {
    part.strip_prefix("=>")
        .or_else(|| part.strip_prefix(':'))
        .or_else(|| part.strip_prefix('='))
}

fn is_quote(c: char) -> bool {
    matches!(c, '"' | '\'' | '`')
}

fn opening_quote(part: &str) -> Option<char> {
    part.chars().next().filter(|c| is_quote(*c))
}

/// Byte index of the first unescaped `quote` in `text` (`\"` is JSON's
/// escaped quote, not the end of the string).
fn find_closing(text: &str, quote: char) -> Option<usize> {
    let mut escaped = false;
    for (ix, c) in text.char_indices() {
        if escaped {
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == quote {
            return Some(ix);
        }
    }
    None
}

/// The end (exclusive) of a leading `"quoted"` item.
fn quoted_item_end(part: &str) -> Option<usize> {
    let quote = opening_quote(part)?;
    find_closing(&part[1..], quote).map(|close| close + 2)
}

/// An HTTP authorization scheme word (`Bearer`, GitHub's `token`, …).
fn is_auth_scheme(word: &str) -> bool {
    matches!(
        word.to_ascii_lowercase().as_str(),
        "bearer" | "basic" | "digest" | "token" | "negotiate" | "ntlm" | "hawk" | "apikey"
    )
}

/// A hidden value's marker: a device code keeps its own, so a sign-in hint
/// still reads right.
fn mask(value: &str) -> &'static str {
    if looks_like_device_code(value) {
        "[code]"
    } else {
        REDACTED
    }
}

/// An unquoted value after a secret label, with its trailing punctuation
/// kept. Always redacted — a short PIN or OTP (`otp: 1234`) is still a
/// secret; status codes stay readable because `status code` is not a label
/// (see [`label_kind`]).
fn redact_labeled_value(word: &str) -> String {
    const EDGE: &[char] = &['"', '\'', '`', ',', ';', ')', '}', ']'];
    let start = word.len() - word.trim_start_matches(EDGE).len();
    let core = word[start..].trim_end_matches(EDGE);
    if core.is_empty() {
        return word.to_string();
    }
    let end = start + core.len();
    format!("{}{}{}", &word[..start], mask(core), &word[end..])
}

const REDACTED: &str = "[redacted]";

/// Key names whose values are secrets, matched anywhere in a key.
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

/// Generic words that are secrets only as a whole key: OpenCode / Pi keep
/// OAuth tokens under `access` and `refresh`.
const EXACT_SECRET_KEYS: &[&str] = &["access", "refresh"];

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
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let webhook = webhook_host(host);
    let authority = match authority.rsplit_once('@') {
        Some((_, host)) => format!("{REDACTED}@{host}"),
        None => authority.to_string(),
    };
    if webhook && path.len() > 1 {
        let tail = if tail.is_empty() { "" } else { "?…" };
        return format!("{scheme}{authority}/{REDACTED}{tail}");
    }
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

    #[test]
    fn spaced_and_pretty_printed_labels_hide_their_values() {
        let cases = [
            ("token: abc123def", "token: [redacted]"),
            ("state = secret-value", "state = [redacted]"),
            ("password: hunter2", "password: [redacted]"),
            (
                r#""access_token": "abc123""#,
                r#""access_token": "[redacted]""#,
            ),
            (
                "{\n  \"refresh_token\": \"r-1\",\n  \"expires\": 3600\n}",
                "{\n  \"refresh_token\": \"[redacted]\",\n  \"expires\": 3600\n}",
            ),
            ("api_key : zzz", "api_key : [redacted]"),
        ];
        for (input, want) in cases {
            assert_eq!(redact_output(input), want, "{input}");
        }
    }

    #[test]
    fn short_labelled_secrets_and_other_auth_schemes_are_redacted() {
        assert_eq!(redact_output("otp: 1234"), "otp: [redacted]");
        assert_eq!(redact_output("state: 7"), "state: [redacted]");
        assert_eq!(
            redact_output("Authorization: token abc123def"),
            "Authorization: token [redacted]"
        );
        assert_eq!(
            redact_output("Authorization: Bearer abc123def"),
            "Authorization: Bearer [redacted]"
        );
        assert_eq!(
            redact_output("authorization: Negotiate YIIabc"),
            "authorization: Negotiate [redacted]"
        );
        assert_eq!(
            redact_output("Authorization: abc123def"),
            "Authorization: [redacted]"
        );
    }

    /// Every case of the consolidated security review (the seven leaks and
    /// the spaced-status regression) plus the earlier rounds', in one table.
    #[test]
    fn adversarial_matrix() {
        let cases = [
            // Scheme words are passed through after `Authorization` only.
            ("password: bearer", "password: [redacted]"),
            (" token: token", " token: [redacted]"),
            ("secret = Basic", "secret = [redacted]"),
            // Compact `Authorization:Bearer` still hides the credential.
            (
                "Authorization:Bearer abc123def",
                "Authorization:Bearer [redacted]",
            ),
            // Parameterised schemes: every parameter, through end-of-line.
            (
                "Authorization: Digest username=alice response=abc123 nonce=xyz",
                "Authorization: Digest [redacted]",
            ),
            (
                r#"Authorization: Hawk id="dh37", ts="1353832234", nonce="j4h3g2", mac="6R4rV5iE+NPoym+WwjeHzjAGXUtLNIxmo1vpMofpLAE=""#,
                "Authorization: Hawk [redacted]",
            ),
            (
                "Proxy-Authorization: Basic dXNlcjpwYXNz",
                "Proxy-Authorization: Basic [redacted]",
            ),
            (
                "authorization = Bearer abc, retry=1\nnext line stays",
                "authorization = Bearer [redacted]\nnext line stays",
            ),
            (
                r#"{"Authorization": "Bearer abc def", "x": 1}"#,
                r#"{"Authorization": "Bearer [redacted]", "x": 1}"#,
            ),
            (
                r#"-H "Authorization: token ghx""#,
                r#"-H "Authorization: token [redacted]"#,
            ),
            // Quoted values through their closing quote, JSON escapes too.
            (
                r#""password": "correct horse battery staple""#,
                r#""password": "[redacted]""#,
            ),
            (
                r#""password": "a \"quoted\" pass phrase", "user": "wing""#,
                r#""password": "[redacted]", "user": "wing""#,
            ),
            (
                "password='two words' then text",
                "password='[redacted]' then text",
            ),
            (
                "password: \"unterminated secret\nvisible line",
                "password: \"[redacted]\nvisible line",
            ),
            // OpenCode / Pi OAuth fields, exact keys only.
            (
                r#""access": "short-access-secret""#,
                r#""access": "[redacted]""#,
            ),
            (
                r#""refresh": "short-refresh-secret""#,
                r#""refresh": "[redacted]""#,
            ),
            (
                r#"{"type":"oauth","access":"a1","refresh":"r1","expires":3}"#,
                r#"{"type":"oauth","access":"[redacted]","refresh":"[redacted]","expires":3}"#,
            ),
            ("id_token: abc", "id_token: [redacted]"),
            ("access_token=abc", "access_token=[redacted]"),
            ("refresh_token : abc", "refresh_token : [redacted]"),
            ("accessible: yes", "accessible: yes"),
            ("refreshing: true", "refreshing: true"),
            ("access denied", "access denied"),
            // Status codes stay readable, spaced separator included.
            ("status code : 503", "status code : 503"),
            ("status code: 503", "status code: 503"),
            ("status code:503", "status code:503"),
            ("HTTP code: 429", "HTTP code: 429"),
            ("exit code: 1", "exit code: 1"),
            // Earlier rounds.
            ("otp: 1234", "otp: [redacted]"),
            (
                "Authorization: token abc123def",
                "Authorization: token [redacted]",
            ),
            ("Code: WXYZ-9876", "Code: [code]"),
            (r#""code": "ABCD-1234""#, r#""code": "[code]""#),
            ("--token=abc tail", "--token=[redacted] tail"),
            // An unquoted value keeps the rest of its word (a comma may be
            // part of the secret).
            ("a=1,token=2,b=3", "a=1,token=[redacted]"),
            (
                "https://auth.example/cb?code=abc&state=xyz",
                "https://auth.example/cb?…",
            ),
            (
                "redirect=https://auth.example/cb?code=abc",
                "redirect=https://auth.example/cb?…",
            ),
            ("with ghp_abcOTPdefKEYghi0123456789", "with [redacted]"),
        ];
        for (input, want) in cases {
            assert_eq!(redact_output(input), want, "{input}");
        }
    }

    #[test]
    fn ordinary_labels_and_short_codes_stay_readable() {
        assert_eq!(redact_output("status code: 503"), "status code: 503");
        assert_eq!(redact_output("HTTP code: 429"), "HTTP code: 429");
        assert_eq!(redact_output("exit code: 1"), "exit code: 1");
        assert_eq!(redact_output("retry after: 30s"), "retry after: 30s");
        assert_eq!(
            redact_output("Sign-in failed: network"),
            "Sign-in failed: network"
        );
    }

    use super::*;

    #[test]
    fn command_output_hides_environment_assignments_and_wider_keys() {
        let out = redact_command_output(
            "AWS_SECRET_ACCESS_KEY=abc/def+ghi\nexport GITHUB_TOKEN=gho_x\nRUST_LOG=debug\n\
             db_pass=hunter2 pwd: s3cret private_key=\"k\" slack_webhook=https://x\n\
             Tests passed: 5 compass=north ERROR: failed\n",
        );
        for secret in [
            "abc/def",
            "gho_x",
            "debug",
            "hunter2",
            "s3cret",
            "\"k\"",
            "https://x",
        ] {
            assert!(!out.contains(secret), "{secret} survived: {out}");
        }
        assert!(out.contains("AWS_SECRET_ACCESS_KEY=[redacted]"), "{out}");
        assert!(out.contains("export GITHUB_TOKEN=[redacted]"), "{out}");
        // Not secrets: a heading, a count, a word that merely contains `pass`.
        assert!(
            out.contains("Tests passed: 5 compass=north ERROR: failed"),
            "{out}"
        );
        // The sign-in redactor is unchanged: env-style keys only in commands.
        assert_eq!(redact_output("RUST_LOG=debug"), "RUST_LOG=debug");
    }

    #[test]
    fn command_output_drops_private_key_blocks() {
        let pem = "before\n-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA\nabc\n-----END RSA PRIVATE KEY-----\nafter\n";
        assert_eq!(
            redact_command_output(pem),
            "before\n[private key redacted]\nafter\n"
        );
        // Cut before its end: dropped through the end.
        let cut = "x\n-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjE\nmore";
        assert_eq!(redact_command_output(cut), "x\n[private key redacted]\n");
        // A public key or certificate stays.
        let cert = "-----BEGIN CERTIFICATE-----\nabc\n-----END CERTIFICATE-----";
        assert_eq!(redact_command_output(cert), cert);
    }

    #[test]
    fn webhook_urls_keep_only_their_host() {
        for (url, kept) in [
            (
                "https://hooks.slack.com/services/T00/B00/XXXX",
                "https://hooks.slack.com/[redacted]",
            ),
            (
                "https://discord.com/api/webhooks/123/abc",
                "https://discord.com/[redacted]",
            ),
            (
                "https://api.telegram.org/bot123:AAH/sendMessage",
                "https://api.telegram.org/[redacted]",
            ),
            (
                "https://acme.webhook.office.com/webhookb2/x?y=1",
                "https://acme.webhook.office.com/[redacted]?…",
            ),
        ] {
            assert_eq!(redact_command_output(url), kept);
            assert_eq!(redact_output(url), kept);
        }
        assert_eq!(
            redact_output("https://github.com/zeronsh/zeron"),
            "https://github.com/zeronsh/zeron"
        );
    }

    #[test]
    fn command_output_hides_tokens_after_separators() {
        let out = redact_command_output(
            "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.c2lnbmF0dXJl\n\
             id=eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.c2lnbmF0dXJl\n\
             sha: 0123456789abcdef0123456789abcdef01234567\n",
        );
        assert!(!out.contains("eyJ"), "{out}");
        assert!(!out.contains("0123456789abcdef0123"), "{out}");
    }

    #[test]
    fn commands_that_print_secrets_are_recognised() {
        for command in [
            "env",
            "env | sort",
            "printenv GITHUB_TOKEN",
            "sudo env -0",
            "export -p",
            "set",
            "cat .env",
            "cat ./config/.env.production",
            "head -n 5 ~/.aws/credentials",
            "cat server.pem",
            "cat ~/.ssh/id_ed25519",
            "echo $X | base64 -d",
            "base64 --decode secret.b64",
            "gpg -d vault.gpg",
            "security find-generic-password -s x -w",
            "pass show work/github",
            "op read op://vault/item/field",
            "aws configure get aws_secret_access_key",
            "gcloud auth print-access-token",
            "gh auth token",
            "cd /w && FOO=1 printenv",
            "/usr/bin/zsh -lc 'cat .env.local'",
            "bash -c \"env\"",
        ] {
            assert!(command_prints_secrets(command), "{command}");
        }
        for command in [
            "cargo test",
            "env RUST_LOG=debug cargo test",
            "/usr/bin/env python3 x.py",
            "cat README.md",
            "set -euo pipefail; make",
            "export PATH=$PATH:/x && make",
            "base64 image.png",
            "git log --oneline 2>&1 | head",
            "pass",
            "aws s3 ls",
            "/usr/bin/zsh -lc 'ls -la'",
        ] {
            assert!(!command_prints_secrets(command), "{command}");
        }
    }

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
            r#"{"access_token":"[redacted]","expires_in":3600}"#
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
