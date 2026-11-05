//! Which agents Zeron signs in to and how: the rules every app's provider
//! settings follow (desktop Settings → Providers, the phone's Providers), so
//! the platforms offer the same sign-ins, labels and switching. Pure.

use crate::HarnessId;

/// The providers zeron can sign into, in display order: (harness, name, CLI
/// command — named in the empty-state copy, zeron settings.agents.tsx
/// `PROVIDERS`). Every agent with a login of its own is here; what each one
/// supports is documented engine-side (`agent_accounts` module docs).
pub const PROVIDERS: [(HarnessId, &str, &str); 10] = [
    (HarnessId::ClaudeCode, "Claude Code", "claude"),
    (HarnessId::Codex, "Codex", "codex"),
    (HarnessId::Cursor, "Cursor", "cursor-agent"),
    (HarnessId::Antigravity, "Antigravity", "Antigravity"),
    (HarnessId::Grok, "Grok", "grok login"),
    (HarnessId::Devin, "Devin", "devin auth login"),
    (HarnessId::Opencode, "OpenCode", "opencode auth login"),
    (HarnessId::Pi, "Pi", "pi"),
    (HarnessId::Hermes, "Hermes", "hermes auth add"),
    (HarnessId::Muse, "Muse Code", "muse login"),
];

/// Whether `harness` has an Accounts section (and sign-in flow). Pure.
pub fn signs_in(harness: HarnessId) -> bool {
    PROVIDERS
        .iter()
        .any(|(provider, _, _)| *provider == harness)
}

pub fn provider_name(harness: HarnessId) -> &'static str {
    if harness == HarnessId::Fx {
        return "fx";
    }
    PROVIDERS
        .iter()
        .find(|(provider, _, _)| *provider == harness)
        .map_or("this provider", |(_, name, _)| name)
}

/// Whether the provider reports plan usage. One that doesn't shows no meters
/// and no "usage unavailable" note — there is nothing missing. Pure.
pub fn reports_usage(harness: HarnessId) -> bool {
    !matches!(harness, HarnessId::Antigravity | HarnessId::Fx)
}

/// Providers whose agent holds exactly ONE login (Antigravity): once it is
/// connected there is nothing to add — signing in again only re-confirms it.
pub fn keeps_one_login(harness: HarnessId) -> bool {
    harness == HarnessId::Antigravity
}

/// Whether zeron switches this agent's logins. Hermes rotates through its
/// own credential pool (listed, never reordered); Antigravity keeps one.
pub fn switches_accounts(harness: HarnessId) -> bool {
    !matches!(harness, HarnessId::Hermes | HarnessId::Antigravity | HarnessId::Fx)
}

/// A standing note under a provider's Accounts label, for an agent whose
/// accounts work differently. Hermes owns its credential pool: zeron lists
/// it and adds to it through Hermes' own CLI, but never switches or removes
/// its entries. Pure.
pub fn provider_note(harness: HarnessId) -> Option<&'static str> {
    match harness {
        HarnessId::Hermes => Some(
            "Hermes manages its own credential pool and rotates through it. Accounts added \
             here go through `hermes auth add`; remove one with `hermes auth remove`.",
        ),
        _ => None,
    }
}

/// One way to add an account: agents that keep a login PER model provider
/// (OpenCode, Pi, Hermes) sign in to a named provider; the rest have one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoginOption {
    /// The engine's `provider` param (`None` = the agent's only login).
    pub provider: Option<&'static str>,
    /// Who the user signs in to.
    pub label: &'static str,
}

/// The sign-ins zeron offers for `harness`, in button order. Pure.
pub fn login_options(harness: HarnessId) -> Vec<LoginOption> {
    let option = |provider, label| LoginOption {
        provider: Some(provider),
        label,
    };
    match harness {
        HarnessId::Opencode => vec![
            option("openai", "ChatGPT"),
            option("github-copilot", "GitHub Copilot"),
        ],
        HarnessId::Pi => vec![option("openai-codex", "ChatGPT")],
        HarnessId::Hermes => vec![
            option("openai-codex", "ChatGPT"),
            option("nous", "Nous Portal"),
        ],
        _ => vec![LoginOption {
            provider: None,
            label: provider_name(harness),
        }],
    }
}

/// The list's last row: connect the first account, or add another. Pure.
pub fn add_account_label(harness: HarnessId, empty: bool) -> String {
    if empty {
        format!("Connect a {} account", provider_name(harness))
    } else {
        "Add account".to_string()
    }
}

/// [`add_account_label`] for one [`LoginOption`]: a per-provider sign-in
/// names its provider ("Connect ChatGPT", "Add GitHub Copilot account"). Pure.
pub fn add_option_label(harness: HarnessId, option: LoginOption, empty: bool) -> String {
    match option.provider {
        None => add_account_label(harness, empty),
        Some(_) if empty => format!("Connect {}", option.label),
        Some(_) => format!("Add {} account", option.label),
    }
}
