//! The relay layer's shared names (docs/design/relay-layer.md).

/// Advertised by an engine that runs the relay layer: a chat hosted on it can read
/// the room's offline queue.
pub const CHAT_RELAY_CAPABILITY: &str = "chat-relay-v1";
/// Room generation of a chat whose host relays instead of storing.
pub const RELAY_ROOM_GEN: u32 = 3;
/// Generation a host returns a chat to when it can't be relayed.
pub const STORED_ROOM_GEN: u32 = 2;
