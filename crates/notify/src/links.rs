//! The `zeron://` conversation links Zeron's apps hand around (a chat's
//! "Copy link", a link opened from elsewhere), for every interface: the
//! GPUI app (crates/ui/src/links.rs) and the zgx app.
//!
//!   zeron://open/chat/<chat id>?workspace=<locator>
//!
//! The locator is opaque: enough to reject a link for another local or
//! synced workspace without putting device, user or organization ids in the
//! URL ([`workspace_locator_of`]).

use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationDeepLink {
    pub chat_id: String,
    pub workspace: String,
}

/// The locator of a workspace: the first 16 hex of SHA-256 over its scope,
/// as Rust names it (`Local`, `Synced`, `Development`), and who it belongs to
/// (`device:<id>` local; `user:<id>:org:<id or "personal">` synced).
pub fn workspace_locator_of(scope: &str, identity: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(format!("{scope}\0{identity}"));
    format!("{:x}", hash.finalize())[..16].to_string()
}

pub fn conversation_link(chat_id: &str, workspace: &str) -> String {
    format!(
        "zeron://open/chat/{}?workspace={}",
        encode_component(chat_id),
        encode_component(workspace)
    )
}

pub fn parse_conversation_link(url: &str) -> Result<ConversationDeepLink, &'static str> {
    let rest = url
        .strip_prefix("zeron://open/chat/")
        .ok_or("not a Zeron conversation link")?;
    let (chat_id, query) = rest.split_once('?').ok_or("missing workspace locator")?;
    if chat_id.is_empty() || chat_id.contains('/') {
        return Err("invalid conversation id");
    }
    let workspace = query
        .split('&')
        .find_map(|part| part.strip_prefix("workspace="))
        .ok_or("missing workspace locator")?;
    Ok(ConversationDeepLink {
        chat_id: decode_component(chat_id)?,
        workspace: decode_component(workspace)?,
    })
}

/// The longest address an invite carries (a URL with a port and a path to
/// spare); a longer one is no server's.
pub const SERVER_INVITE_MAX: usize = 512;

/// A self-hosted server's invite, as its own page hands it out ("Open in
/// Zeron", edge/src/discovery.ts): the address to look up, which the apps
/// offer and never switch to without a click.
///
///   zeron://server?address=<address>
pub fn server_invite_link(address: &str) -> String {
    format!("zeron://server?address={}", encode_component(address))
}

/// The address an invite carries, trimmed: one line, no longer than
/// [`SERVER_INVITE_MAX`]. What it names is the lookup's to judge.
pub fn parse_server_invite(url: &str) -> Result<String, &'static str> {
    let rest = url.strip_prefix("zeron://server").ok_or("not a Zeron server invite")?;
    let query = rest
        .strip_prefix("/?")
        .or_else(|| rest.strip_prefix('?'))
        .ok_or("missing server address")?;
    let encoded = query
        .split('&')
        .find_map(|part| part.strip_prefix("address="))
        .ok_or("missing server address")?;
    let address = decode_component(encoded)?;
    let address = address.trim();
    if address.is_empty() {
        return Err("missing server address");
    }
    if address.len() > SERVER_INVITE_MAX || address.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("invalid server address");
    }
    Ok(address.to_owned())
}

/// A string as one part of a link (RFC 3986 unreserved characters kept).
pub fn encode_component(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            use std::fmt::Write as _;
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

fn decode_component(value: &str) -> Result<String, &'static str> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let encoded = bytes
                .get(index + 1..index + 3)
                .ok_or("invalid URL escape")?;
            let text = std::str::from_utf8(encoded).map_err(|_| "invalid URL escape")?;
            out.push(u8::from_str_radix(text, 16).map_err(|_| "invalid URL escape")?);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).map_err(|_| "invalid UTF-8 in URL")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zeron_link_round_trips_reserved_characters() {
        let link = conversation_link("chat/with space", "workspace:one");
        assert_eq!(
            parse_conversation_link(&link).unwrap(),
            ConversationDeepLink {
                chat_id: "chat/with space".into(),
                workspace: "workspace:one".into(),
            }
        );
    }

    #[test]
    fn malformed_or_foreign_links_are_rejected() {
        assert!(parse_conversation_link("https://example.com").is_err());
        assert!(parse_conversation_link("zeron://open/chat/id").is_err());
        assert!(parse_conversation_link("zeron://open/chat/%GG?workspace=x").is_err());
    }

    #[test]
    fn a_server_invite_round_trips_its_address() {
        for address in ["zeron.acme.com", "https://zeron.acme.com", "http://192.168.1.20:8787/edge"] {
            assert_eq!(parse_server_invite(&server_invite_link(address)).as_deref(), Ok(address));
        }
        assert_eq!(
            parse_server_invite("zeron://server/?from=web&address=%20zeron.acme.com%20").as_deref(),
            Ok("zeron.acme.com")
        );
    }

    #[test]
    fn a_server_invite_without_one_line_of_address_is_refused() {
        assert!(parse_server_invite("zeron://server").is_err());
        assert!(parse_server_invite("zeron://server?address=").is_err());
        assert!(parse_server_invite("zeron://server?address=%20%20").is_err());
        assert!(parse_server_invite("zeron://server?address=a%0Ab").is_err());
        assert!(parse_server_invite("zeron://server?address=a%20b").is_err());
        assert!(parse_server_invite("zeron://server?address=%GG").is_err());
        assert!(parse_server_invite(&server_invite_link(&"a".repeat(SERVER_INVITE_MAX + 1))).is_err());
        assert!(parse_server_invite("zeron://serverx?address=a").is_err());
        assert!(parse_server_invite("https://zeron.acme.com").is_err());
    }

    #[test]
    fn a_locator_is_sixteen_hex_of_its_scope_and_identity() {
        let one = workspace_locator_of("Local", "device:a");
        assert_eq!(one.len(), 16);
        assert!(one.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(one, workspace_locator_of("Local", "device:b"));
        assert_ne!(one, workspace_locator_of("Synced", "device:a"));
    }
}
