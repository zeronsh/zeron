//! Loopback reverse proxy that swaps a coding-agent CLI's credential for one
//! of several subscription accounts of the same provider, and moves a request
//! to another account when one hits its usage limit.
//!
//! The engine starts one [`AccountProxy`] lazily and registers a [`Route`]
//! per harness process. The process is pointed at [`Route::base_url`] with a
//! placeholder credential; every request it makes is forwarded to the
//! provider's real host with the credential the engine's
//! [`CredentialSource`] leases for that route. The route token in the URL is
//! the only authentication: 256 random bits, compared in constant time, and
//! never logged.
//!
//! Failover: request bodies up to [`ProxyConfig::max_replay_body`] are
//! buffered. When the provider answers with a quota-exhausted error before any
//! byte went downstream, the account is reported ([`Outcome::LimitReached`]),
//! excluded, and the request is replayed on the next leased account. A 401/403
//! is reported as [`Outcome::Rejected`] and retried once on another account;
//! the CLI itself never sees a 401/403 for a swapped credential (it gets a
//! 503), because CLIs answer 401 by refreshing or wiping their own login.
//! Overload and short throttling pass through untouched. Once the CLI has
//! received response bytes nothing is retried. With no account left, the last
//! upstream error reaches the CLI unchanged.
//!
//! A successful event stream is held back while it carries only lifecycle
//! events (`message_start`, `response.created`...), within a small budget, so
//! a quota error that arrives in-band as the first event still fails over.
//! After the first content event, in-band errors are only reported.
//!
//! WebSocket upgrades are tunneled with the swapped credential on the
//! upstream handshake, except for providers in
//! [`ProxyConfig::refuse_websocket`] (ChatGPT by default): Codex answers a 426
//! by moving the session to HTTP streaming (`core/src/client.rs`, the
//! `UPGRADE_REQUIRED` arms), and a credential fixed at handshake time could
//! not fail over per turn.
//!
//! Which harness uses which [`Provider`], and the override that points it at
//! a route:
//!
//! | Harness | Provider | Override |
//! |---|---|---|
//! | Codex | `ChatGpt` | `thread/start` config `openai_base_url = "<base>/backend-api/codex"` |
//! | OpenCode (ChatGPT) | `ChatGpt` | `OPENCODE_CONFIG_CONTENT` `providers.openai.settings.baseURL = "<base>/backend-api/codex"` |
//! | Pi `openai-codex` | `ChatGpt` | extension `registerProvider("openai-codex", {baseUrl: "<base>/backend-api"})` |
//! | Pi `anthropic` | `Anthropic` | extension `registerProvider("anthropic", {baseUrl: "<base>"})` |
//! | Grok | `Grok` | `GROK_CLI_CHAT_PROXY_BASE_URL=<base>/v1` |
//! | Muse | `Muse` | `serve -c 'endpoint_transport.base_url="<base>"'` (unverified) |

mod forward;
mod provider;
mod server;
mod sse;
mod types;

pub use provider::Provider;
pub use server::{AccountProxy, DEFAULT_MAX_REPLAY_BODY, ProxyConfig, Route};
pub use types::{
    AccountId, BodyEdit, CredentialSource, HistoryRecovery, Lease, LeaseError, Outcome, ProxyEvent,
    RateLimitSnapshot, RateLimitWindow, RouteInfo, SwitchReason,
};
