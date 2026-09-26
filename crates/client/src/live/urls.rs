//! Edge endpoint shapes (edge/src — registry-room.ts, chat2 room, device
//! room, auth routes). WebSocket auth rides the URL query (`?token=`) because
//! sockets can't set headers; HTTP endpoints take a Bearer header.

/// `http(s)://host` → `ws(s)://host` (any non-`http` scheme maps to `wss`).
pub(crate) fn ws_base(edge: &str) -> String {
    let edge = edge.trim_end_matches('/');
    if let Some(rest) = edge.strip_prefix("http://") {
        format!("ws://{rest}")
    } else if let Some(rest) = edge.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if edge.starts_with("ws://") || edge.starts_with("wss://") {
        edge.to_owned()
    } else {
        format!("wss://{edge}")
    }
}

pub(crate) fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

pub(crate) fn registry_ws(edge: &str, org_id: &str, token: &str, device_id: &str) -> String {
    format!(
        "{}/registry/{}/ws?token={}&device={}",
        ws_base(edge),
        encode(org_id),
        encode(token),
        encode(device_id)
    )
}

pub(crate) fn registry_rows(edge: &str, org_id: &str, device_id: &str, since: u64) -> String {
    let mut url = format!(
        "{}/registry/{}/rows?device={}&beat=1",
        edge.trim_end_matches('/'),
        encode(org_id),
        encode(device_id)
    );
    if since > 0 {
        url.push_str(&format!("&since={since}"));
    }
    url
}

pub(crate) fn registry_push(edge: &str, org_id: &str, device_id: &str) -> String {
    format!(
        "{}/registry/{}/push?device={}",
        edge.trim_end_matches('/'),
        encode(org_id),
        encode(device_id)
    )
}

pub(crate) fn chat_ws(edge: &str, chat_id: &str, token: &str, device_id: &str) -> String {
    format!(
        "{}/chat2/{}/ws?token={}&device={}",
        ws_base(edge),
        encode(chat_id),
        encode(token),
        encode(device_id)
    )
}

pub(crate) fn chat_checkpoint(edge: &str, chat_id: &str) -> String {
    format!("{}/chat2/{}/checkpoint", edge.trim_end_matches('/'), encode(chat_id))
}

pub(crate) fn chat_rows(edge: &str, chat_id: &str, after: u64, device_id: &str) -> String {
    format!(
        "{}/chat2/{}/rows?after={after}&device={}",
        edge.trim_end_matches('/'),
        encode(chat_id),
        encode(device_id)
    )
}

pub(crate) fn chat_push(edge: &str, chat_id: &str, batch_id: &str, device_id: &str) -> String {
    format!(
        "{}/chat2/{}/rows?batchId={}&device={}",
        edge.trim_end_matches('/'),
        encode(chat_id),
        encode(batch_id),
        encode(device_id)
    )
}

pub(crate) fn nudge(edge: &str, host_device_id: &str) -> String {
    format!("{}/device/{}/nudge", edge.trim_end_matches('/'), encode(host_device_id))
}

pub(crate) fn health(edge: &str) -> String {
    format!("{}/health", edge.trim_end_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_shapes() {
        assert_eq!(ws_base("https://edge.zeron.sh/"), "wss://edge.zeron.sh");
        assert_eq!(ws_base("http://127.0.0.1:8787"), "ws://127.0.0.1:8787");
        assert_eq!(
            registry_ws("https://e.sh", "org_1", "a.b", "ios-1"),
            "wss://e.sh/registry/org_1/ws?token=a.b&device=ios-1"
        );
        assert_eq!(
            chat_ws("https://e.sh", "c1", "t@o", "ios-1"),
            "wss://e.sh/chat2/c1/ws?token=t%40o&device=ios-1"
        );
        assert_eq!(registry_rows("https://e.sh", "o", "d", 0), "https://e.sh/registry/o/rows?device=d&beat=1");
        assert_eq!(nudge("https://e.sh", "dev-mac"), "https://e.sh/device/dev-mac/nudge");
    }
}
