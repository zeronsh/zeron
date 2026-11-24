//! Deploy tokens (contract a315eef5a): what a CI job publishes with. The
//! edge makes one, answers it once, and keeps only its SHA-256 and short
//! id; a device never makes one, and never keeps or logs one.
//!
//! A token is `zdt_`, a body, and a checksum. The body is 32 random bytes
//! as one big-endian number in base62 (`0-9A-Za-z`, a digit's value its
//! index), most significant digit first, padded with `0` to 43 characters.
//! The checksum is CRC-32 (zlib's) of the ASCII of everything before it
//! (`zdt_` and the body), in base62 padded with `0` to 6 characters. So a
//! leak scanner can tell one, and a typo fails before anything is sent.
//! Its short id, what it's named by beside its name, is the first 8
//! characters after `zdt_`.
//!
//! A version published with a token is unsigned (`via: "deploy_token"`,
//! `key: null` in its record): a device trusts it as the store's, by the
//! record's hash, as any store install.

use std::fmt;

use zeroize::Zeroizing;

pub const PREFIX: &str = "zdt_";
/// The environment variable `zeron plugin publish` reads the token from.
pub const ENV: &str = "ZERON_DEPLOY_TOKEN";
/// Base62's digits, in value order.
pub const ALPHABET: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
/// The random part's bytes.
pub const RANDOM_BYTES: usize = 32;
/// The body's characters: 62^43 > 2^256.
pub const BODY_LEN: usize = 43;
/// The checksum's characters: 62^6 > 2^32.
pub const CHECK_LEN: usize = 6;
/// A whole token's characters.
pub const LEN: usize = PREFIX.len() + BODY_LEN + CHECK_LEN;
/// A short id's characters.
pub const SHORT_LEN: usize = 8;
/// The most plugins one token's scope names.
pub const MAX_SCOPE_PLUGINS: usize = 50;

/// The words a token's refusal carries (the CLI's and the edge's).
pub mod reason {
    /// 401: the token is wrong (a bad checksum, unknown), revoked or
    /// expired.
    pub const INVALID_TOKEN: &str = "invalid_token";
    /// 403: a live token that may not publish this plugin (out of its
    /// scope, not the account's, or the plugin's first version).
    pub const TOKEN_NOT_ALLOWED: &str = "token_not_allowed";
    /// 401: a token on a route that takes a signed-in session.
    pub const SESSION_REQUIRED: &str = "session_required";
}

/// Why a token can't be used, in words a CI log can carry (never quoting it).
pub const NOT_A_TOKEN: &str = "That isn't a deploy token (it starts zdt_): copy it again from where it was shown when it was made";
pub const DAMAGED: &str = "That deploy token is cut short or mistyped: copy it again from where it was shown when it was made";

/// A deploy token, as read from CI's secrets. Never printed: `Debug` shows
/// only its short id.
pub struct DeployToken(Zeroizing<String>);

impl fmt::Debug for DeployToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("DeployToken").field(&self.short()).finish()
    }
}

impl DeployToken {
    /// The token `text` is (surrounding whitespace ignored): `zdt_` and 49
    /// base62 characters, else [`NOT_A_TOKEN`]; a checksum that doesn't
    /// match, [`DAMAGED`].
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        let Some(rest) = text.strip_prefix(PREFIX) else {
            return Err(NOT_A_TOKEN.into());
        };
        if !rest.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(NOT_A_TOKEN.into());
        }
        if rest.len() != BODY_LEN + CHECK_LEN {
            return Err(DAMAGED.into());
        }
        let (before, check) = text.split_at(LEN - CHECK_LEN);
        if checksum(before) != check {
            return Err(DAMAGED.into());
        }
        Ok(Self(Zeroizing::new(text.to_owned())))
    }

    /// The token made from `random` (what the edge does with its own
    /// randomness; here for tests and the shared vectors).
    pub fn from_random(random: &[u8; RANDOM_BYTES]) -> Self {
        let before = format!("{PREFIX}{}", base62(random, BODY_LEN));
        let check = checksum(&before);
        Self(Zeroizing::new(format!("{before}{check}")))
    }

    /// The whole token, to send as a bearer or show the once it's made.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Its short id: the first 8 characters after `zdt_`.
    pub fn short(&self) -> &str {
        &self.0[PREFIX.len()..PREFIX.len() + SHORT_LEN]
    }

    /// What the edge keeps of it: the SHA-256 (hex) of its ASCII.
    pub fn hash(&self) -> String {
        super::super::signing::sha256_hex(self.0.as_bytes())
    }

    /// The `Authorization` header that publishes with it.
    pub fn authorization(&self) -> Zeroizing<String> {
        Zeroizing::new(format!("Bearer {}", self.as_str()))
    }
}

/// `bytes` as one big-endian number in base62, padded with `0` to `width`
/// digits.
pub fn base62(bytes: &[u8], width: usize) -> String {
    let mut number = bytes.to_vec();
    let mut digits = Vec::with_capacity(width);
    while number.iter().any(|b| *b != 0) {
        let mut remainder = 0u32;
        for byte in number.iter_mut() {
            let value = (remainder << 8) | u32::from(*byte);
            *byte = (value / 62) as u8;
            remainder = value % 62;
        }
        digits.push(ALPHABET[remainder as usize]);
    }
    while digits.len() < width {
        digits.push(b'0');
    }
    digits.reverse();
    String::from_utf8(digits).expect("base62 is ASCII")
}

/// The checksum of a token's first 47 characters: CRC-32 of their ASCII,
/// in base62 padded to 6.
pub fn checksum(before: &str) -> String {
    base62(&crc32fast::hash(before.as_bytes()).to_be_bytes(), CHECK_LEN)
}

/// What a token may publish, fixed when it's made (as a key's was): every
/// plugin of its account's, those it publishes later included, or the
/// plugins named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    All,
    /// Sorted and unique.
    Plugins(Vec<String>),
}

impl Scope {
    /// The plugins named (`ids`, trimmed and sorted by code unit), or `Err`
    /// when there are none or more than [`MAX_SCOPE_PLUGINS`], one isn't an
    /// id, or one is named twice: the store refuses a duplicate rather than
    /// dropping it, so this does too.
    pub fn plugins(ids: &[String]) -> Result<Self, String> {
        let mut ids: Vec<String> = ids.iter().map(|id| id.trim().to_owned()).collect();
        ids.sort();
        if ids.is_empty() || ids.len() > MAX_SCOPE_PLUGINS {
            return Err(format!("A token is for 1 to {MAX_SCOPE_PLUGINS} plugins, or for all of yours"));
        }
        if let Some(bad) = ids.iter().find(|id| super::super::manifest::check_id(id).is_err()) {
            return Err(format!("\"{bad}\" isn't a plugin's id"));
        }
        if let Some(twice) = ids.windows(2).find(|pair| pair[0] == pair[1]) {
            return Err(format!("\"{}\" is named twice", twice[0]));
        }
        Ok(Self::Plugins(ids))
    }

    /// As the store takes and answers it: `{all: true}` or `{plugins: [...]}`.
    pub fn to_value(&self) -> serde_json::Value {
        match self {
            Scope::All => serde_json::json!({ "all": true }),
            Scope::Plugins(ids) => serde_json::json!({ "plugins": ids }),
        }
    }

    /// Whether it covers plugin `id`.
    pub fn covers(&self, id: &str) -> bool {
        match self {
            Scope::All => true,
            Scope::Plugins(ids) => ids.iter().any(|p| p == id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base62_is_big_endian_and_padded() {
        assert_eq!(base62(&[0], 3), "000");
        assert_eq!(base62(&[61], 1), "z");
        assert_eq!(base62(&[62], 2), "10");
        assert_eq!(base62(&[1, 0], 2), "48", "256 = 4·62 + 8");
        assert_eq!(base62(&[0xff; 32], BODY_LEN).len(), BODY_LEN, "the largest body still fits");
        assert_eq!(base62(&u32::MAX.to_be_bytes(), CHECK_LEN).len(), CHECK_LEN);
        // CRC-32 as zlib computes it.
        assert_eq!(crc32fast::hash(b"123456789"), 0xcbf4_3926);
    }

    #[test]
    fn a_token_round_trips_and_a_damaged_one_is_told_apart() {
        let token = DeployToken::from_random(&[7; RANDOM_BYTES]);
        let text = token.as_str().to_owned();
        assert!(text.starts_with(PREFIX) && text.len() == LEN, "{text}");
        assert_eq!(token.short(), &text[4..12]);
        let again = DeployToken::parse(&format!("  {text}\n")).unwrap();
        assert_eq!(again.as_str(), text);
        assert_eq!(again.hash(), crate::plugins::signing::sha256_hex(text.as_bytes()));
        assert_eq!(DeployToken::parse("ghp_x").unwrap_err(), NOT_A_TOKEN);
        assert_eq!(DeployToken::parse(&format!("{}-", &text[..LEN - 1])).unwrap_err(), NOT_A_TOKEN);
        assert_eq!(DeployToken::parse(&text[..LEN - 1]).unwrap_err(), DAMAGED);
        // One character changed: the checksum catches it.
        let mut typo = text.clone().into_bytes();
        typo[10] = if typo[10] == b'A' { b'B' } else { b'A' };
        assert_eq!(DeployToken::parse(&String::from_utf8(typo).unwrap()).unwrap_err(), DAMAGED);
        // Never printed.
        assert!(!format!("{token:?}").contains(&text[12..]));
        assert_eq!(token.authorization().as_str(), format!("Bearer {text}"));
    }

    #[test]
    fn a_scope_is_all_or_one_to_fifty_ids_each_once() {
        let two = Scope::plugins(&["git".into(), " focus".into()]).unwrap();
        assert_eq!(two.to_value(), serde_json::json!({ "plugins": ["focus", "git"] }));
        assert!(two.covers("git") && !two.covers("notes") && Scope::All.covers("notes"));
        assert!(Scope::plugins(&[]).is_err() && Scope::plugins(&["Not An Id".into()]).is_err());
        assert_eq!(Scope::plugins(&["git".into(), "focus".into(), " git".into()]), Err("\"git\" is named twice".to_owned()));
        assert!(Scope::plugins(&(0..51).map(|n| format!("p{n:02}")).collect::<Vec<_>>()).is_err());
    }
}
