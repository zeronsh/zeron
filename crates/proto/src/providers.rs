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
///
/// For an agent that keeps a login per model provider, these are the
/// providers its own auth supports AND zeron can sign in to unattended
/// (engine `agent_accounts::oauth` / `stores` say how). Left out on
/// purpose: Pi's Claude (`anthropic`) — zeron doesn't mint Claude Code
/// subscription logins for a third-party agent (Pi's own `/login` still
/// does, and those logins are listed and switched); OpenCode's Anthropic
/// OAuth (removed upstream); Pi's xAI, Kimi, Meta and OpenRouter logins
/// (zeron can't tell their accounts apart yet); and Hermes' Claude, Qwen,
/// MiniMax and OpenRouter (not unattended device codes: a pasted code,
/// another CLI's login, a minted API key, unverified).
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
        HarnessId::Pi => vec![
            option("openai-codex", "ChatGPT"),
            option("github-copilot", "GitHub Copilot"),
        ],
        HarnessId::Hermes => vec![
            option("openai-codex", "ChatGPT"),
            option("nous", "Nous Portal"),
            option("xai-oauth", "xAI"),
        ],
        _ => vec![LoginOption {
            provider: None,
            label: provider_name(harness),
        }],
    }
}

/// Whether `harness` keeps one login PER model provider (OpenCode, Pi,
/// Hermes): its accounts show grouped by provider, each group with its own
/// account in use, and a run uses the account of its model's provider.
/// Every other agent has one group: its own. Pure.
pub fn groups_by_provider(harness: HarnessId) -> bool {
    matches!(
        harness,
        HarnessId::Opencode | HarnessId::Pi | HarnessId::Hermes
    )
}

/// The sign-in zeron offers for `provider` of `harness`, if it offers one.
pub fn login_option(harness: HarnessId, provider: &str) -> Option<LoginOption> {
    login_options(harness)
        .into_iter()
        .find(|option| option.provider == Some(provider))
}

/// What a provider group of a per-provider agent is called (its heading in
/// Settings, the model picker's "<name> account"): who the user signs in
/// to — "ChatGPT" for OpenCode's `openai` and Pi's `openai-codex`, "Claude"
/// for `anthropic`. An id zeron doesn't know reads as its words. Pure.
pub fn account_group_label(harness: HarnessId, provider: &str) -> String {
    if let Some(option) = login_option(harness, provider) {
        return option.label.to_string();
    }
    let known = match provider {
        "openai" | "openai-codex" => "ChatGPT",
        "anthropic" => "Claude",
        "github-copilot" | "copilot" => "GitHub Copilot",
        "nous" => "Nous Portal",
        "xai" | "xai-oauth" => "xAI",
        "openrouter" => "OpenRouter",
        "google-gemini-cli" => "Gemini",
        "kimi-coding" => "Kimi",
        "qwen-oauth" => "Qwen",
        "minimax-oauth" => "MiniMax",
        "meta" => "Meta",
        _ => "",
    };
    if !known.is_empty() {
        return known.to_string();
    }
    provider
        .split(['-', '_'])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next()
                .map(|f| f.to_uppercase().chain(c).collect::<String>())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// How to add another account to a provider group zeron can't sign in to
/// itself (Pi's Claude logins, a Hermes provider without a device code):
/// the agent's own way, said under the group. `None` where zeron offers
/// the sign-in ([`login_option`]). Pure.
pub fn account_group_hint(harness: HarnessId, provider: &str) -> Option<String> {
    if login_option(harness, provider).is_some() {
        return None;
    }
    let label = account_group_label(harness, provider);
    match harness {
        HarnessId::Pi => Some(format!("To add another {label} account, run /login in Pi.")),
        HarnessId::Opencode => Some(format!(
            "To add another {label} account, run `opencode auth login`."
        )),
        HarnessId::Hermes => Some(format!(
            "To add another {label} account, run `hermes auth add {provider}`."
        )),
        _ => None,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_provider_agents_group_and_offer_a_sign_in_per_provider() {
        for harness in [HarnessId::Opencode, HarnessId::Pi, HarnessId::Hermes] {
            assert!(groups_by_provider(harness), "{harness:?}");
            assert!(
                login_options(harness).iter().all(|o| o.provider.is_some()),
                "{harness:?} names each provider"
            );
        }
        for harness in [HarnessId::ClaudeCode, HarnessId::Codex, HarnessId::Grok] {
            assert!(!groups_by_provider(harness), "{harness:?}");
        }
        let pi: Vec<_> = login_options(HarnessId::Pi)
            .iter()
            .map(|o| (o.provider.unwrap(), o.label))
            .collect();
        assert_eq!(
            pi,
            [
                ("openai-codex", "ChatGPT"),
                ("github-copilot", "GitHub Copilot")
            ]
        );
        let hermes: Vec<_> = login_options(HarnessId::Hermes)
            .iter()
            .map(|o| o.provider.unwrap())
            .collect();
        assert_eq!(hermes, ["openai-codex", "nous", "xai-oauth"]);
        // Pi's Claude logins are listed and switched, never signed in to here.
        assert!(login_option(HarnessId::Pi, "anthropic").is_none());
        let copilot = login_option(HarnessId::Pi, "github-copilot").unwrap();
        assert_eq!(
            add_option_label(HarnessId::Pi, copilot, false),
            "Add GitHub Copilot account"
        );
        assert_eq!(
            add_option_label(HarnessId::Pi, copilot, true),
            "Connect GitHub Copilot"
        );
    }

    #[test]
    fn provider_groups_are_named_for_who_you_sign_in_to() {
        assert_eq!(
            account_group_label(HarnessId::Pi, "openai-codex"),
            "ChatGPT"
        );
        assert_eq!(
            account_group_label(HarnessId::Opencode, "openai"),
            "ChatGPT"
        );
        assert_eq!(account_group_label(HarnessId::Pi, "anthropic"), "Claude");
        assert_eq!(
            account_group_label(HarnessId::Hermes, "github-copilot"),
            "GitHub Copilot"
        );
        assert_eq!(account_group_label(HarnessId::Hermes, "xai-oauth"), "xAI");
        assert_eq!(
            account_group_label(HarnessId::Hermes, "my-local_llm"),
            "My Local Llm"
        );
    }

    #[test]
    fn a_group_zeron_cant_add_to_says_the_agents_own_way() {
        assert_eq!(
            account_group_hint(HarnessId::Pi, "anthropic").as_deref(),
            Some("To add another Claude account, run /login in Pi.")
        );
        assert_eq!(
            account_group_hint(HarnessId::Hermes, "anthropic").as_deref(),
            Some("To add another Claude account, run `hermes auth add anthropic`.")
        );
        assert!(account_group_hint(HarnessId::Pi, "openai-codex").is_none());
        assert!(account_group_hint(HarnessId::Opencode, "github-copilot").is_none());
        assert!(account_group_hint(HarnessId::ClaudeCode, "anthropic").is_none());
    }
}
