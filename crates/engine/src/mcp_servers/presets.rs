//! Built-in catalog of official remote MCP server presets.

use zeron_proto::{McpAuthKind, McpPreset, McpTransport};

/// Predefined catalog of popular official remote MCP servers.
///
/// Every one signs in with no app of the person's own: its OAuth server
/// registers Zeron's client itself (RFC 7591), each checked on 2026-11-23 by
/// registering a client as `oauth::register_dynamic_client` does. Servers
/// that need an app of your own (Slack, GitHub, Figma) are added as custom
/// servers with one.
pub fn default_presets() -> Vec<McpPreset> {
    vec![
        McpPreset {
            id: "linear".into(),
            name: "Linear".into(),
            description: "Inspect and manage Linear issues, cycles, projects, and team workflows."
                .into(),
            category: "Project Management".into(),
            url: "https://mcp.linear.app/mcp".into(),
            transport: McpTransport::Http,
            auth_kind: McpAuthKind::OAuth,
            docs_url: "https://linear.app/docs/mcp".into(),
            icon: "linear".into(),
            header_name: None,
            client_required: false,
        },
        McpPreset {
            id: "notion".into(),
            name: "Notion".into(),
            description: "Search, read, and update Notion pages, databases, and comments.".into(),
            category: "Docs & Knowledge".into(),
            url: "https://mcp.notion.com/mcp".into(),
            transport: McpTransport::Http,
            auth_kind: McpAuthKind::OAuth,
            docs_url: "https://developers.notion.com/docs/mcp".into(),
            icon: "notion".into(),
            header_name: None,
            client_required: false,
        },
        McpPreset {
            id: "sentry".into(),
            name: "Sentry".into(),
            description: "Investigate Sentry issues, stack traces, releases, and performance data."
                .into(),
            category: "Observability".into(),
            url: "https://mcp.sentry.dev/mcp".into(),
            transport: McpTransport::Http,
            auth_kind: McpAuthKind::OAuth,
            docs_url: "https://docs.sentry.io/product/sentry-mcp/".into(),
            icon: "sentry".into(),
            header_name: None,
            client_required: false,
        },
        McpPreset {
            id: "atlassian".into(),
            name: "Atlassian".into(),
            description: "Search and manage Jira issues and Confluence pages across Atlassian."
                .into(),
            category: "Project Management".into(),
            url: "https://mcp.atlassian.com/v1/mcp".into(),
            transport: McpTransport::Http,
            auth_kind: McpAuthKind::OAuth,
            docs_url: "https://support.atlassian.com/rovo/docs/setting-up-ides/".into(),
            icon: "atlassian".into(),
            header_name: None,
            client_required: false,
        },
        McpPreset {
            id: "stripe".into(),
            name: "Stripe".into(),
            description:
                "Inspect Stripe customers, payments, subscriptions, and API documentation.".into(),
            category: "Payments".into(),
            url: "https://mcp.stripe.com".into(),
            transport: McpTransport::Http,
            auth_kind: McpAuthKind::OAuth,
            docs_url: "https://docs.stripe.com/mcp".into(),
            icon: "stripe".into(),
            header_name: None,
            client_required: false,
        },
        McpPreset {
            id: "supabase".into(),
            name: "Supabase".into(),
            description:
                "Manage Supabase projects, Postgres schemas, Edge Functions, and logs.".into(),
            category: "Database".into(),
            url: "https://mcp.supabase.com/mcp".into(),
            transport: McpTransport::Http,
            auth_kind: McpAuthKind::OAuth,
            docs_url: "https://supabase.com/docs/guides/getting-started/mcp".into(),
            icon: "supabase".into(),
            header_name: None,
            client_required: false,
        },
        McpPreset {
            id: "vercel".into(),
            name: "Vercel".into(),
            description:
                "Inspect Vercel deployments, build logs, domains, and project settings.".into(),
            category: "Infrastructure".into(),
            url: "https://mcp.vercel.com".into(),
            transport: McpTransport::Http,
            auth_kind: McpAuthKind::OAuth,
            docs_url: "https://vercel.com/docs/mcp/vercel-mcp".into(),
            icon: "vercel".into(),
            header_name: None,
            client_required: false,
        },
        McpPreset {
            id: "cloudflare".into(),
            name: "Cloudflare".into(),
            description:
                "Manage Cloudflare Workers, KV, D1, R2, observability, and documentation.".into(),
            category: "Infrastructure".into(),
            url: "https://mcp.cloudflare.com/mcp".into(),
            transport: McpTransport::Http,
            auth_kind: McpAuthKind::OAuth,
            docs_url: "https://developers.cloudflare.com/agents/model-context-protocol/mcp-servers-for-cloudflare/".into(),
            icon: "cloudflare".into(),
            header_name: None,
            client_required: false,
        },
        McpPreset {
            id: "posthog".into(),
            name: "PostHog".into(),
            description:
                "Query PostHog analytics, feature flags, experiments, and error tracking.".into(),
            category: "Analytics".into(),
            url: "https://mcp.posthog.com/mcp".into(),
            transport: McpTransport::Http,
            auth_kind: McpAuthKind::OAuth,
            docs_url: "https://posthog.com/docs/model-context-protocol".into(),
            icon: "posthog".into(),
            header_name: None,
            client_required: false,
        },
        McpPreset {
            id: "context7".into(),
            name: "Context7".into(),
            description: "Fetch up-to-date library documentation and version-specific code examples."
                .into(),
            category: "Documentation".into(),
            url: "https://mcp.context7.com/mcp".into(),
            transport: McpTransport::Http,
            auth_kind: McpAuthKind::None,
            docs_url: "https://context7.com".into(),
            icon: "context7".into(),
            header_name: None,
            client_required: false,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn default_presets_have_unique_ids_and_valid_urls() {
        let presets = default_presets();
        assert_eq!(presets.len(), 10);
        let mut ids = HashSet::new();
        for p in &presets {
            assert!(ids.insert(p.id.clone()), "duplicate preset id: {}", p.id);
            assert_ne!(p.id, "zeron");
            assert!(!p.name.trim().is_empty());
            assert!(!p.category.trim().is_empty());
            assert!(p.url.starts_with("https://"), "bad url for {}: {}", p.id, p.url);
            assert!(
                p.docs_url.starts_with("https://"),
                "bad docs_url for {}: {}",
                p.id,
                p.docs_url
            );
            assert_eq!(p.transport, McpTransport::Http);
        }
        let context7 = presets.iter().find(|p| p.id == "context7").unwrap();
        assert_eq!(context7.auth_kind, McpAuthKind::None);
        // (Slack's MCP takes only a Marketplace-published or internal Slack
        // app's own client: not offered here, added as a custom server with
        // one's own app instead. Every preset signs in without one.)
        // Nor GitHub's (its OAuth has no client registration) or Figma's
        // (registration refused to all but its approved clients).
        for gone in ["slack", "github", "figma"] {
            assert!(presets.iter().all(|p| p.id != gone), "{gone} needs its own app");
        }
        assert!(presets.iter().all(|p| !p.client_required));
    }
}
