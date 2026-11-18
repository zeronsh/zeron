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
    !matches!(
        harness,
        HarnessId::Hermes | HarnessId::Antigravity | HarnessId::Fx
    )
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
/// (engine `agent_accounts::oauth` / `stores` say how): Pi's Claude is the
/// same Claude Pro/Max sign-in Pi's own `/login` runs. Left out: OpenCode's
/// Claude (OpenCode has no Anthropic OAuth — removed upstream, absent from
/// 2.x); Pi's xAI, Kimi, Meta and OpenRouter logins (zeron can't tell their
/// accounts apart yet); and Hermes' Claude, Qwen, MiniMax and OpenRouter
/// (not unattended device codes: a pasted code, another CLI's login, a
/// minted API key, unverified). API keys are [`api_key_options`].
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
            option("anthropic", "Claude"),
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

/// A provider an agent reaches with an API key Zeron can add for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApiKeyOption {
    /// The agent's own id for the provider: its store key (Pi's and
    /// OpenCode's `auth.json`) or Hermes' pool / `hermes auth add` name.
    pub provider: &'static str,
    /// Who issues the key ("OpenRouter", "OpenAI").
    pub label: &'static str,
}

/// The providers whose API key Zeron adds for `harness`, as the agent's own
/// CLI would store it (engine `agent_accounts::api_keys`), each checked
/// with the provider before it's saved. Pure.
pub fn api_key_options(harness: HarnessId) -> Vec<ApiKeyOption> {
    let key = |provider, label| ApiKeyOption { provider, label };
    match harness {
        HarnessId::Pi | HarnessId::Opencode => vec![
            key("anthropic", "Anthropic"),
            key("openai", "OpenAI"),
            key("openrouter", "OpenRouter"),
            key("google", "Google Gemini"),
            key("xai", "xAI"),
            key("deepseek", "DeepSeek"),
        ],
        HarnessId::Hermes => vec![
            key("anthropic", "Anthropic"),
            key("openai-api", "OpenAI"),
            key("openrouter", "OpenRouter"),
            key("gemini", "Google Gemini"),
            key("xai", "xAI"),
            key("deepseek", "DeepSeek"),
        ],
        _ => Vec::new(),
    }
}

/// The API key Zeron adds for `provider` of `harness`, if it adds one.
pub fn api_key_option(harness: HarnessId, provider: &str) -> Option<ApiKeyOption> {
    api_key_options(harness)
        .into_iter()
        .find(|option| option.provider == provider)
}

/// What adding one says: "Add OpenRouter API key" (or, inside the
/// provider's own group, "Add API key"). Pure.
pub fn add_key_label(option: ApiKeyOption, in_group: bool) -> String {
    if in_group {
        "Add API key".to_string()
    } else {
        format!("Add {} API key", option.label)
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
/// Settings, the model picker's "<name> account"): the provider its models
/// come from — "ChatGPT" for the ChatGPT-only `openai-codex`, "OpenAI" for
/// `openai` (ChatGPT sign-ins and API keys alike), "Anthropic" for
/// `anthropic` (Claude sign-ins and API keys). An id zeron doesn't know
/// reads as its words. Pure.
pub fn account_group_label(harness: HarnessId, provider: &str) -> String {
    let known = match provider {
        "openai-codex" => "ChatGPT",
        "openai" | "openai-api" => "OpenAI",
        "anthropic" => "Anthropic",
        "github-copilot" | "copilot" => "GitHub Copilot",
        "nous" => "Nous Portal",
        "xai" | "xai-oauth" => "xAI",
        "openrouter" => "OpenRouter",
        "google" | "gemini" => "Google Gemini",
        "google-gemini-cli" => "Gemini",
        "groq" => "Groq",
        "deepseek" => "DeepSeek",
        "mistral" => "Mistral",
        "kimi-coding" => "Kimi",
        "qwen-oauth" => "Qwen",
        "minimax-oauth" => "MiniMax",
        "meta" => "Meta",
        "vercel" | "vercel-ai-gateway" => "Vercel AI Gateway",
        _ => "",
    };
    if !known.is_empty() {
        return known.to_string();
    }
    if let Some(option) = login_option(harness, provider) {
        return option.label.to_string();
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

/// How to add another account to a provider group zeron can neither sign
/// in to nor add a key for (a Pi login zeron can't run, a Hermes provider
/// without a device code or key): the agent's own way, said under the
/// group. `None` where zeron adds one itself. Pure.
pub fn account_group_hint(harness: HarnessId, provider: &str) -> Option<String> {
    if login_option(harness, provider).is_some() || api_key_option(harness, provider).is_some() {
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

// ── Cloud ───────────────────────────────────────────────────────────────────

/// How a Cloud machine hands a vault credential to an agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudDelivery {
    /// These environment variables of the agent's process, set at spawn: a
    /// static key (a new one replaces the session at its next turn).
    Env(&'static [&'static str]),
    /// A file the agent reads, written by the machine's credential broker
    /// and rewritten ahead of the access token's expiry (never with a
    /// refresh token: the vault is the only refresher).
    File,
}

/// One vault credential an agent runs on, on Cloud.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CloudCredential {
    pub vault: crate::VaultProvider,
    /// The account group it fills on the agent's card: the agent's own id
    /// for the model provider (its store key, as [`login_options`] and
    /// [`api_key_options`] name them). Within one group the first credential
    /// the vault grants is the one used (a sign-in before a key); separate
    /// groups are all used at once.
    pub group: &'static str,
    pub delivery: CloudDelivery,
}

const fn cloud(
    vault: crate::VaultProvider,
    group: &'static str,
    delivery: CloudDelivery,
) -> CloudCredential {
    CloudCredential {
        vault,
        group,
        delivery,
    }
}

/// The vault credentials `harness` runs on, on Cloud, in preference order
/// (docs/design/cloud-device.md, "Providers"). Logins appear only where the
/// vault can be their only refresher, or they never expire; anything else
/// reaches Cloud as the vendor's API key. Claude subscription logins go to
/// Claude Code alone. Pure.
pub fn cloud_credentials(harness: HarnessId) -> &'static [CloudCredential] {
    use crate::VaultProvider as V;
    use CloudDelivery::{Env, File};
    const ANTHROPIC: &[&str] = &["ANTHROPIC_API_KEY"];
    const OPENAI: &[&str] = &["OPENAI_API_KEY"];
    const OPENROUTER: &[&str] = &["OPENROUTER_API_KEY"];
    const XAI: &[&str] = &["XAI_API_KEY"];
    const DEEPSEEK: &[&str] = &["DEEPSEEK_API_KEY"];
    const GATEWAY: &[&str] = &["AI_GATEWAY_API_KEY"];
    const CLAUDE_CODE: &[CloudCredential] = &[
        cloud(V::Claude, "anthropic", File),
        cloud(V::AnthropicKey, "anthropic", Env(ANTHROPIC)),
    ];
    const CODEX: &[CloudCredential] = &[
        cloud(V::Codex, "openai", File),
        cloud(V::OpenaiKey, "openai", File),
    ];
    const CURSOR: &[CloudCredential] = &[cloud(V::CursorKey, "cursor", Env(&["CURSOR_API_KEY"]))];
    const DEVIN: &[CloudCredential] = &[cloud(V::Devin, "devin", File)];
    const GROK: &[CloudCredential] = &[
        cloud(V::Grok, "xai", File),
        cloud(V::XaiKey, "xai", Env(XAI)),
    ];
    const MUSE: &[CloudCredential] = &[cloud(V::MetaKey, "meta", Env(&["META_API_KEY"]))];
    const ANTIGRAVITY: &[CloudCredential] = &[cloud(V::GeminiKey, "gemini", File)];
    const FX: &[CloudCredential] = &[
        cloud(V::AiGatewayKey, "vercel", Env(GATEWAY)),
        cloud(V::OpenaiKey, "openai", Env(OPENAI)),
        cloud(V::AnthropicKey, "anthropic", Env(ANTHROPIC)),
    ];
    const PI: &[CloudCredential] = &[
        cloud(V::Codex, "openai-codex", File),
        cloud(V::Copilot, "github-copilot", Env(&["COPILOT_GITHUB_TOKEN"])),
        cloud(V::AnthropicKey, "anthropic", Env(ANTHROPIC)),
        cloud(V::OpenaiKey, "openai", Env(OPENAI)),
        cloud(V::OpenrouterKey, "openrouter", Env(OPENROUTER)),
        cloud(V::GeminiKey, "google", Env(&["GEMINI_API_KEY"])),
        cloud(V::XaiKey, "xai", Env(XAI)),
        cloud(V::DeepseekKey, "deepseek", Env(DEEPSEEK)),
        cloud(V::AiGatewayKey, "vercel-ai-gateway", Env(GATEWAY)),
    ];
    const OPENCODE: &[CloudCredential] = &[
        cloud(V::AnthropicKey, "anthropic", Env(ANTHROPIC)),
        cloud(V::OpenaiKey, "openai", Env(OPENAI)),
        cloud(V::OpenrouterKey, "openrouter", Env(OPENROUTER)),
        cloud(
            V::GeminiKey,
            "google",
            Env(&["GOOGLE_GENERATIVE_AI_API_KEY", "GEMINI_API_KEY"]),
        ),
        cloud(V::XaiKey, "xai", Env(XAI)),
        cloud(V::DeepseekKey, "deepseek", Env(DEEPSEEK)),
        cloud(V::AiGatewayKey, "vercel", Env(GATEWAY)),
    ];
    const HERMES: &[CloudCredential] = &[
        cloud(V::AnthropicKey, "anthropic", Env(ANTHROPIC)),
        cloud(V::OpenaiKey, "openai-api", Env(OPENAI)),
        cloud(V::OpenrouterKey, "openrouter", Env(OPENROUTER)),
        cloud(
            V::GeminiKey,
            "gemini",
            Env(&["GEMINI_API_KEY", "GOOGLE_API_KEY"]),
        ),
        cloud(V::XaiKey, "xai", Env(XAI)),
        cloud(V::DeepseekKey, "deepseek", Env(DEEPSEEK)),
    ];
    match harness {
        HarnessId::ClaudeCode => CLAUDE_CODE,
        HarnessId::Codex => CODEX,
        HarnessId::Cursor => CURSOR,
        HarnessId::Devin => DEVIN,
        HarnessId::Grok => GROK,
        HarnessId::Muse => MUSE,
        HarnessId::Antigravity => ANTIGRAVITY,
        HarnessId::Fx => FX,
        HarnessId::Pi => PI,
        HarnessId::Opencode => OPENCODE,
        HarnessId::Hermes => HERMES,
        _ => &[],
    }
}

/// Whether `harness` runs on Cloud at all. Pure.
pub fn runs_on_cloud(harness: HarnessId) -> bool {
    !cloud_credentials(harness).is_empty()
}

/// The sign-ins Cloud offers for `harness` (Settings → Providers with Cloud
/// shown), in button order: each is captured on the user's own device and
/// goes straight to the vault. The rest of an agent's logins have no place
/// on Cloud (see [`cloud_note`]); their vendors' API keys do. Pure.
pub fn cloud_login_options(harness: HarnessId) -> Vec<LoginOption> {
    let only = |label| {
        vec![LoginOption {
            provider: None,
            label,
        }]
    };
    match harness {
        HarnessId::ClaudeCode => only("Claude Code"),
        HarnessId::Codex => only("Codex"),
        HarnessId::Cursor => only("Cursor"),
        HarnessId::Devin => only("Devin"),
        HarnessId::Grok => only("Grok"),
        HarnessId::Pi => vec![
            LoginOption {
                provider: Some("openai-codex"),
                label: "ChatGPT",
            },
            LoginOption {
                provider: Some("github-copilot"),
                label: "GitHub Copilot",
            },
        ],
        _ => Vec::new(),
    }
}

/// The vault provider a Cloud sign-in of `harness` (for `provider`, on an
/// agent that signs in per model provider) fills. Pure.
pub fn cloud_login_vault(
    harness: HarnessId,
    provider: Option<&str>,
) -> Option<crate::VaultProvider> {
    use crate::VaultProvider as V;
    let option = cloud_login_options(harness)
        .into_iter()
        .find(|o| o.provider == provider.filter(|p| !p.is_empty()))?;
    Some(match (harness, option.provider) {
        (HarnessId::ClaudeCode, _) => V::Claude,
        (HarnessId::Codex, _) | (HarnessId::Pi, Some("openai-codex")) => V::Codex,
        (HarnessId::Cursor, _) => V::CursorKey,
        (HarnessId::Devin, _) => V::Devin,
        (HarnessId::Grok, _) => V::Grok,
        (HarnessId::Pi, Some("github-copilot")) => V::Copilot,
        _ => return None,
    })
}

/// The API keys `harness` takes on Cloud, in its card's order. Pure.
pub fn cloud_key_providers(harness: HarnessId) -> Vec<crate::VaultProvider> {
    cloud_credentials(harness)
        .iter()
        .map(|c| c.vault)
        .filter(|v| v.is_api_key())
        .collect()
}

/// What an agent's Cloud card says about how it signs in there, where that
/// differs from the agent's own sign-ins. Pure.
pub fn cloud_note(harness: HarnessId) -> Option<&'static str> {
    match harness {
        HarnessId::Pi => Some(
            "On Cloud, Pi uses your ChatGPT and GitHub Copilot sign-ins and your API keys. \
             Claude subscriptions run in Claude Code only: add an Anthropic API key for Pi.",
        ),
        HarnessId::Opencode => Some(
            "On Cloud, OpenCode uses your API keys. OpenCode keeps its sign-ins in its own \
             database and refreshes them itself, so they stay on your devices.",
        ),
        HarnessId::Hermes => Some(
            "On Cloud, Hermes uses your API keys. Hermes refreshes the sign-ins in its own \
             pool itself, so they stay on your devices.",
        ),
        HarnessId::Antigravity => Some(
            "On Cloud, Antigravity uses a Gemini API key. Its Google sign-in stays on your \
             devices.",
        ),
        HarnessId::Muse => {
            Some("On Cloud, Muse Code uses a Meta API key. Its own sign-in stays on your devices.")
        }
        HarnessId::Fx => Some("On Cloud, fx uses your Vercel AI Gateway, OpenAI or Anthropic key."),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_built_in_agent_runs_on_cloud_on_credentials_the_vault_holds() {
        use crate::VaultProvider as V;
        for harness in HarnessId::ALL {
            let wire = harness.as_str();
            assert_eq!(
                runs_on_cloud(harness),
                crate::CLOUD_HARNESSES.contains(&wire),
                "{wire}"
            );
        }
        // Claude subscriptions reach Claude Code alone.
        for harness in HarnessId::ALL {
            let claude = cloud_credentials(harness)
                .iter()
                .any(|c| c.vault == V::Claude);
            assert_eq!(claude, harness == HarnessId::ClaudeCode, "{harness:?}");
        }
        // Every sign-in Cloud offers fills a credential the agent runs on.
        for harness in HarnessId::ALL {
            for option in cloud_login_options(harness) {
                let vault = cloud_login_vault(harness, option.provider)
                    .unwrap_or_else(|| panic!("{harness:?} {option:?}"));
                assert!(!vault.is_api_key() || vault == V::CursorKey, "{harness:?}");
                assert!(
                    cloud_credentials(harness).iter().any(|c| c.vault == vault),
                    "{harness:?} {vault:?}"
                );
            }
        }
        // A per-provider agent's groups are its own provider ids.
        for harness in [HarnessId::Pi, HarnessId::Opencode, HarnessId::Hermes] {
            for credential in cloud_credentials(harness) {
                if credential.vault.is_api_key() {
                    assert!(
                        api_key_option(harness, credential.group).is_some()
                            || credential.vault == V::AiGatewayKey,
                        "{harness:?} {}",
                        credential.group
                    );
                }
            }
        }
        assert_eq!(cloud_login_vault(HarnessId::Pi, Some("anthropic")), None);
        assert_eq!(
            cloud_login_vault(HarnessId::Pi, Some("openai-codex")),
            Some(V::Codex)
        );
        assert_eq!(cloud_login_vault(HarnessId::Grok, None), Some(V::Grok));
        assert_eq!(cloud_key_providers(HarnessId::Grok), vec![V::XaiKey]);
    }

    #[test]
    fn per_provider_agents_group_and_offer_a_sign_in_per_provider() {
        for harness in [HarnessId::Opencode, HarnessId::Pi, HarnessId::Hermes] {
            assert!(groups_by_provider(harness), "{harness:?}");
            assert!(
                login_options(harness).iter().all(|o| o.provider.is_some()),
                "{harness:?} names each provider"
            );
            assert!(!api_key_options(harness).is_empty(), "{harness:?}");
        }
        for harness in [HarnessId::ClaudeCode, HarnessId::Codex, HarnessId::Grok] {
            assert!(!groups_by_provider(harness), "{harness:?}");
            assert!(api_key_options(harness).is_empty(), "{harness:?}");
        }
        let pi: Vec<_> = login_options(HarnessId::Pi)
            .iter()
            .map(|o| (o.provider.unwrap(), o.label))
            .collect();
        assert_eq!(
            pi,
            [
                ("openai-codex", "ChatGPT"),
                ("anthropic", "Claude"),
                ("github-copilot", "GitHub Copilot")
            ]
        );
        let hermes: Vec<_> = login_options(HarnessId::Hermes)
            .iter()
            .map(|o| o.provider.unwrap())
            .collect();
        assert_eq!(hermes, ["openai-codex", "nous", "xai-oauth"]);
        // OpenCode has no Claude sign-in of its own: none offered.
        assert!(login_option(HarnessId::Opencode, "anthropic").is_none());
        let copilot = login_option(HarnessId::Pi, "github-copilot").unwrap();
        assert_eq!(
            add_option_label(HarnessId::Pi, copilot, false),
            "Add GitHub Copilot account"
        );
        assert_eq!(
            add_option_label(HarnessId::Pi, copilot, true),
            "Connect GitHub Copilot"
        );
        let openrouter = api_key_option(HarnessId::Hermes, "openrouter").unwrap();
        assert_eq!(add_key_label(openrouter, false), "Add OpenRouter API key");
        assert_eq!(add_key_label(openrouter, true), "Add API key");
        assert!(api_key_option(HarnessId::Hermes, "openai").is_none());
        assert!(api_key_option(HarnessId::Hermes, "openai-api").is_some());
    }

    #[test]
    fn provider_groups_are_named_for_the_provider() {
        assert_eq!(
            account_group_label(HarnessId::Pi, "openai-codex"),
            "ChatGPT"
        );
        assert_eq!(account_group_label(HarnessId::Opencode, "openai"), "OpenAI");
        assert_eq!(account_group_label(HarnessId::Pi, "anthropic"), "Anthropic");
        assert_eq!(
            account_group_label(HarnessId::Hermes, "github-copilot"),
            "GitHub Copilot"
        );
        assert_eq!(account_group_label(HarnessId::Hermes, "xai-oauth"), "xAI");
        assert_eq!(
            account_group_label(HarnessId::Hermes, "gemini"),
            "Google Gemini"
        );
        assert_eq!(
            account_group_label(HarnessId::Hermes, "my-local_llm"),
            "My Local Llm"
        );
    }

    #[test]
    fn a_group_zeron_cant_add_to_says_the_agents_own_way() {
        assert_eq!(
            account_group_hint(HarnessId::Pi, "kimi-coding").as_deref(),
            Some("To add another Kimi account, run /login in Pi.")
        );
        assert_eq!(
            account_group_hint(HarnessId::Hermes, "minimax-oauth").as_deref(),
            Some("To add another MiniMax account, run `hermes auth add minimax-oauth`.")
        );
        // Zeron adds these itself (a sign-in, a key): no hint.
        assert!(account_group_hint(HarnessId::Pi, "anthropic").is_none());
        assert!(account_group_hint(HarnessId::Hermes, "openrouter").is_none());
        assert!(account_group_hint(HarnessId::Opencode, "github-copilot").is_none());
        assert!(account_group_hint(HarnessId::ClaudeCode, "anthropic").is_none());
    }
}
