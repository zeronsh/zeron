//! Which MCP servers a harness should proxy, leave alone, or omit.
//!
//! Pure functions of one server fact plus the chat's role. Snapshot capture
//! and the launch rewrite that consume this live in later units.

use serde::{Deserialize, Serialize};
use zeron_proto::{McpMode, McpServerChoice};

/// Resolved command basenames that start a language runtime or a container.
const HEAVY_RUNTIMES: &[&str] = &[
    "npx", "npm", "node", "bun", "bunx", "deno", "uvx", "uv", "python", "python3", "pipx",
    "docker", "podman", "java",
];

const REMOTE_TRANSPORTS: &[&str] = &[
    "http",
    "https",
    "sse",
    "ws",
    "wss",
    "claudeai-proxy",
    "sdk",
    "streamable-http",
];

/// What kind of server this row is. The launch mode follows from the class
/// unless the user named one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum McpClass {
    Remote,
    Channel,
    HeavyRuntime,
    NativeSmall,
}

/// What a driver should do with one server. `Untouched` means declare it as
/// configured. A [`McpMode`] is the proxy plan's decision, including `Off`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpDisposition {
    Untouched,
    Mode(McpMode),
}

/// One server, already reduced to the facts classification needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerFact {
    pub name: String,
    /// `stdio`, `http`, `sse`, `claudeai-proxy`, `sdk`, …
    pub transport: String,
    /// The stdio command, when there is one. A path or a bare name.
    pub command: String,
    /// The npx resolver rewrote this command onto a cached install.
    pub npx_resolved: bool,
    /// Cached `initialize` or the plugin manifest says this server is a channel.
    pub channel: bool,
    /// Connector scope (`user`, `claude.ai`, …). Empty when unknown.
    pub scope: String,
}

/// Who is launching, and any per-server choice from Settings.
#[derive(Debug, Clone, Copy)]
pub struct PolicyContext {
    pub worker: bool,
    /// This chat holds the channel lease, or it is the pinned owner.
    /// Ignored for workers: a worker never keeps a channel server.
    pub channel_owner: bool,
    pub choice: Option<McpServerChoice>,
}

/// `name` + NUL + `config_hash`. The NUL keeps the two parts from colliding.
pub fn channel_identity(name: &str, config_hash: &str) -> String {
    format!("{name}\0{config_hash}")
}

/// True for the experimental capability `channel`, `claude/channel`, or any
/// `<namespace>/channel`. `channels` does not match.
pub fn declares_channel_capability(name: &str) -> bool {
    name == "channel" || name == "claude/channel" || name.ends_with("/channel")
}

/// True when any key of an `initialize` experimental map declares a channel.
pub fn experimental_map_declares_channel(
    experimental: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    experimental
        .keys()
        .any(|name| declares_channel_capability(name))
}

pub fn classify(fact: &McpServerFact) -> McpClass {
    if is_remote(fact) {
        return McpClass::Remote;
    }
    if fact.channel {
        return McpClass::Channel;
    }
    if fact.transport.trim().eq_ignore_ascii_case("stdio")
        && !fact.command.trim().is_empty()
        && (fact.npx_resolved || heavy_runtime_command(&fact.command))
    {
        return McpClass::HeavyRuntime;
    }
    McpClass::NativeSmall
}

/// Apply user choices only within the transport and channel safety boundaries.
pub fn disposition(fact: &McpServerFact, ctx: PolicyContext) -> McpDisposition {
    let class = classify(fact);
    if class == McpClass::Remote {
        return McpDisposition::Untouched;
    }
    if class == McpClass::Channel {
        return if ctx.worker
            || !ctx.channel_owner
            || matches!(ctx.choice, Some(McpServerChoice::Off))
        {
            McpDisposition::Mode(McpMode::Off)
        } else {
            McpDisposition::Mode(McpMode::Eager)
        };
    }
    if let Some(choice) = ctx.choice
        && let Some(mode) = choice.explicit_mode(ctx.worker)
    {
        if matches!(mode, McpMode::Lazy | McpMode::LazyKeepAlive)
            && (!fact.transport.trim().eq_ignore_ascii_case("stdio")
                || fact.command.trim().is_empty())
        {
            return McpDisposition::Untouched;
        }
        return McpDisposition::Mode(mode);
    }
    match class {
        McpClass::HeavyRuntime => McpDisposition::Mode(McpMode::Lazy),
        _ => McpDisposition::Untouched,
    }
}

fn is_remote(fact: &McpServerFact) -> bool {
    let transport = fact.transport.trim().to_ascii_lowercase();
    if REMOTE_TRANSPORTS.contains(&transport.as_str()) {
        return true;
    }
    matches!(
        fact.scope.trim().to_ascii_lowercase().as_str(),
        "claudeai" | "claude.ai" | "claude-ai"
    )
}

fn heavy_runtime_command(command: &str) -> bool {
    let base = command_basename(command);
    HEAVY_RUNTIMES.contains(&base.as_str())
}

fn command_basename(command: &str) -> String {
    let name = command.rsplit(['/', '\\']).next().unwrap_or(command);
    let lower = name.to_ascii_lowercase();
    lower
        .strip_suffix(".exe")
        .or_else(|| lower.strip_suffix(".cmd"))
        .or_else(|| lower.strip_suffix(".bat"))
        .unwrap_or(&lower)
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fact(
        name: &str,
        transport: &str,
        command: &str,
        npx_resolved: bool,
        channel: bool,
        scope: &str,
    ) -> McpServerFact {
        McpServerFact {
            name: name.into(),
            transport: transport.into(),
            command: command.into(),
            npx_resolved,
            channel,
            scope: scope.into(),
        }
    }

    fn ctx(worker: bool, channel_owner: bool, choice: Option<McpServerChoice>) -> PolicyContext {
        PolicyContext {
            worker,
            channel_owner,
            choice,
        }
    }

    #[test]
    fn mcp_policy_heavy_stdio_runtimes_are_lazy() {
        for command in [
            "node",
            "npx",
            "bun",
            "uvx",
            "docker",
            "npm",
            "bunx",
            "deno",
            "uv",
            "python",
            "python3",
            "pipx",
            "podman",
            "java",
            "/usr/local/bin/npx",
            r"C:\Program Files\nodejs\node.EXE",
            "bun.cmd",
        ] {
            let row = fact("srv", "stdio", command, false, false, "user");
            assert_eq!(classify(&row), McpClass::HeavyRuntime, "{command}");
            assert_eq!(
                disposition(&row, ctx(false, false, None)),
                McpDisposition::Mode(McpMode::Lazy),
                "{command}"
            );
        }
        let resolved = fact("srv", "stdio", "reins", true, false, "user");
        assert_eq!(classify(&resolved), McpClass::HeavyRuntime);
    }

    #[test]
    fn mcp_policy_remote_and_claude_ai_connectors_stay_untouched() {
        for (transport, scope) in [
            ("http", "user"),
            ("https", ""),
            ("sse", "user"),
            ("ws", "user"),
            ("wss", "user"),
            ("streamable-http", "user"),
            ("HTTP", "user"),
            ("claudeai-proxy", ""),
            ("sdk", "project"),
            ("stdio", "claude.ai"),
            ("stdio", "claudeai"),
            ("stdio", "Claude-AI"),
        ] {
            let row = fact("connector", transport, "node", false, false, scope);
            assert_eq!(classify(&row), McpClass::Remote, "{transport}/{scope}");
            assert_eq!(
                disposition(&row, ctx(false, true, None)),
                McpDisposition::Untouched,
                "{transport}/{scope}"
            );
            assert_eq!(
                disposition(&row, ctx(true, false, None)),
                McpDisposition::Untouched,
                "worker {transport}/{scope}"
            );
        }
    }

    #[test]
    fn mcp_policy_small_native_servers_stay_untouched() {
        for command in ["reins", "zeron", "my-server", "docker-compose", ""] {
            let row = fact("srv", "stdio", command, false, false, "user");
            assert_eq!(classify(&row), McpClass::NativeSmall, "{command}");
            assert_eq!(
                disposition(&row, ctx(false, false, None)),
                McpDisposition::Untouched,
                "{command}"
            );
        }
    }

    #[test]
    fn mcp_policy_channel_servers_are_eager_for_the_owner_only() {
        let row = fact("fake-channel", "stdio", "fake-channel", false, true, "user");
        assert_eq!(classify(&row), McpClass::Channel);
        assert_eq!(
            disposition(&row, ctx(false, true, None)),
            McpDisposition::Mode(McpMode::Eager)
        );
        assert_eq!(
            disposition(&row, ctx(false, false, None)),
            McpDisposition::Mode(McpMode::Off)
        );
        // Remote transports remain untouched, even if they report a channel.
        let remote = fact("fake-channel", "http", "", false, true, "claude.ai");
        assert_eq!(classify(&remote), McpClass::Remote);
        assert_eq!(
            disposition(&remote, ctx(false, false, None)),
            McpDisposition::Untouched
        );
    }

    #[test]
    fn mcp_policy_workers_get_the_reduced_set() {
        let heavy = fact("playwright", "stdio", "npx", false, false, "user");
        let native = fact("reins", "stdio", "reins", false, false, "user");
        let channel = fact("fake-channel", "stdio", "fake", false, true, "user");
        let worker = ctx(true, true, None);
        assert_eq!(
            disposition(&heavy, worker),
            McpDisposition::Mode(McpMode::Lazy)
        );
        assert_eq!(disposition(&native, worker), McpDisposition::Untouched);
        assert_eq!(
            disposition(&channel, worker),
            McpDisposition::Mode(McpMode::Off)
        );
        assert_eq!(
            disposition(&heavy, ctx(true, false, Some(McpServerChoice::MainOnly))),
            McpDisposition::Mode(McpMode::Off)
        );
        assert_eq!(
            disposition(&channel, ctx(true, true, Some(McpServerChoice::MainOnly))),
            McpDisposition::Mode(McpMode::Off)
        );
        // Main-only on a main chat keeps the class default.
        assert_eq!(
            disposition(&heavy, ctx(false, false, Some(McpServerChoice::MainOnly))),
            McpDisposition::Mode(McpMode::Lazy)
        );
        assert_eq!(
            disposition(&native, ctx(false, false, Some(McpServerChoice::MainOnly))),
            McpDisposition::Untouched
        );
    }

    #[test]
    fn mcp_policy_user_overrides_win() {
        let heavy = fact("playwright", "stdio", "npx", false, false, "user");
        let remote = fact("docs", "https", "", false, false, "");
        let channel = fact("fake-channel", "stdio", "fake", false, true, "user");
        assert_eq!(
            disposition(&heavy, ctx(false, false, Some(McpServerChoice::Eager))),
            McpDisposition::Mode(McpMode::Eager)
        );
        assert_eq!(
            disposition(&heavy, ctx(false, false, Some(McpServerChoice::Off))),
            McpDisposition::Mode(McpMode::Off)
        );
        assert_eq!(
            disposition(&remote, ctx(false, false, Some(McpServerChoice::Lazy))),
            McpDisposition::Untouched
        );
        // A worker cannot acquire a channel through an override.
        assert_eq!(
            disposition(&channel, ctx(true, false, Some(McpServerChoice::Eager))),
            McpDisposition::Mode(McpMode::Off)
        );
        assert_eq!(
            disposition(
                &native_small(),
                ctx(false, false, Some(McpServerChoice::LazyKeepAlive))
            ),
            McpDisposition::Mode(McpMode::LazyKeepAlive)
        );
    }

    #[test]
    fn mcp_policy_unknown_transport_and_channel_overrides_are_safe() {
        for transport in ["", "unknown"] {
            let row = fact("srv", transport, "node", true, false, "");
            assert_eq!(classify(&row), McpClass::NativeSmall);
            assert_eq!(
                disposition(&row, ctx(false, true, Some(McpServerChoice::Lazy))),
                McpDisposition::Untouched
            );
        }
        let row = fact("srv", "stdio", "", true, false, "");
        assert_eq!(classify(&row), McpClass::NativeSmall);
        let channel = fact("channel", "stdio", "node", false, true, "");
        for choice in [
            McpServerChoice::Lazy,
            McpServerChoice::LazyKeepAlive,
            McpServerChoice::Eager,
        ] {
            assert_eq!(
                disposition(&channel, ctx(false, true, Some(choice))),
                McpDisposition::Mode(McpMode::Eager)
            );
            assert_eq!(
                disposition(&channel, ctx(false, false, Some(choice))),
                McpDisposition::Mode(McpMode::Off)
            );
        }
    }

    fn native_small() -> McpServerFact {
        fact("reins", "stdio", "reins", false, false, "user")
    }

    #[test]
    fn mcp_policy_channel_capability_names_and_identity() {
        assert!(declares_channel_capability("channel"));
        assert!(declares_channel_capability("claude/channel"));
        assert!(declares_channel_capability("acme/channel"));
        assert!(!declares_channel_capability("channels"));
        assert!(!declares_channel_capability("claude/channels"));
        assert!(!declares_channel_capability("channel/extra"));
        assert!(!declares_channel_capability("Channel"));
        assert!(!declares_channel_capability(""));
        let mut map = serde_json::Map::new();
        assert!(!experimental_map_declares_channel(&map));
        map.insert("tools".into(), serde_json::json!({}));
        assert!(!experimental_map_declares_channel(&map));
        map.insert("claude/channel".into(), serde_json::Value::Null);
        assert!(experimental_map_declares_channel(&map));
        assert_eq!(
            channel_identity("fake-channel", "hash"),
            "fake-channel\0hash"
        );
        assert_ne!(channel_identity("ab", "c"), channel_identity("a", "bc"));
    }
}
