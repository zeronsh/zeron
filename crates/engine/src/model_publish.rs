//! Cloud runner only: keeps the account's model catalogs current on the
//! edge (docs/design/cloud-device.md, "Engine changes"), so every device's
//! model picker lists each Cloud agent's real models while all of Cloud's
//! machines sleep.
//!
//! An agent's models depend on the credentials it runs on (Pi lists the
//! models of every provider it's signed in to), and on Cloud those are the
//! vault's, handed to each run by the credential broker. So after a run's
//! environment is prepared, the agent's credential variables become its
//! discovery environment ([`zeron_harness::catalog_env`]), the catalog is
//! discovered with them, and it is sent to the edge (`POST /runner/models`)
//! with the run's credential fingerprint. At most once per (agent,
//! fingerprint) per [`REPUBLISH_AFTER`]; a new fingerprint (another account,
//! a new key) publishes at once. Claude Code's and Codex's lists are kept on
//! the edge by hand, so they are never published.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::{Duration, Instant};

use zeron_proto::providers::{CloudDelivery, cloud_credentials, runs_on_cloud};
use zeron_proto::{HarnessId, Model};

use crate::registry::{CatalogPublisher, HarnessRegistry};

/// The same catalog again after this, so a model the provider adds shows up.
pub const REPUBLISH_AFTER: Duration = Duration::from_secs(10 * 60);

/// Where a discovered catalog goes (the edge; a recorder in tests).
#[async_trait::async_trait]
pub trait CatalogSink: Send + Sync {
    async fn publish(
        &self,
        harness: HarnessId,
        models: &[Model],
        credential: &str,
    ) -> Result<(), String>;
}

/// `POST {edge}/runner/models` with the runner bearer.
pub struct EdgeSink {
    url: String,
    tokens: Arc<dyn zeron_rpc::TokenSource>,
    http: reqwest::Client,
}

impl EdgeSink {
    pub fn new(edge_url: &str, tokens: Arc<dyn zeron_rpc::TokenSource>) -> Self {
        Self {
            url: format!("{}/runner/models", edge_url.trim_end_matches('/')),
            tokens,
            http: crate::runner::http_client(),
        }
    }
}

#[async_trait::async_trait]
impl CatalogSink for EdgeSink {
    async fn publish(
        &self,
        harness: HarnessId,
        models: &[Model],
        credential: &str,
    ) -> Result<(), String> {
        let token = self.tokens.token().await.map_err(|e| e.to_string())?;
        let mut body = serde_json::json!({
            "harness": harness.as_str(),
            "models": models,
            "credential": credential,
        });
        // Whose catalog it is: the edge keeps one per set of accounts, so a
        // switch of account shows that account's models.
        if let Some(accounts) = crate::credentials::credential_accounts(credential) {
            body["accounts"] = serde_json::Value::String(accounts);
        }
        let res = self
            .http
            .post(&self.url)
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .map_err(crate::http_error::describe_http_error)?;
        match res.status().as_u16() {
            200..=299 => Ok(()),
            // An edge that predates the route: nothing to keep it in.
            404 => {
                tracing::debug!("model catalogs: the edge doesn't keep them yet");
                Ok(())
            }
            status => Err(format!("the edge refused the catalog ({status})")),
        }
    }
}

struct Inner {
    registry: Weak<HarnessRegistry>,
    sink: Arc<dyn CatalogSink>,
    /// The fingerprint each agent's catalog was last published with, and when.
    published: Mutex<HashMap<HarnessId, (String, Instant)>>,
    /// Agents whose catalog is being discovered now.
    busy: Mutex<HashSet<HarnessId>>,
}

/// The runner's [`CatalogPublisher`]. Cheap to clone.
#[derive(Clone)]
pub struct ModelPublisher {
    inner: Arc<Inner>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The variables of `env` that hold `harness`'s Cloud credentials: what
/// its discovery needs, nothing else of the run's (its chat, git identity).
pub fn credential_env(
    harness: HarnessId,
    env: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut names: HashSet<&str> = cloud_credentials(harness)
        .iter()
        .filter_map(|c| match c.delivery {
            CloudDelivery::Env(names) => Some(names.to_vec()),
            // Logins handed to an agent's own credential API (OpenCode's)
            // ride one variable its harness installs on any server it boots,
            // discovery's included.
            CloudDelivery::Api => Some(vec![zeron_harness::opencode::CREDENTIALS_ENV]),
            // A file is already where discovery reads it.
            CloudDelivery::File => None,
        })
        .flatten()
        .collect();
    // Antigravity's Gemini key is a file setting AND this variable.
    if harness == HarnessId::Antigravity {
        names.insert("GEMINI_API_KEY");
    }
    env.iter()
        .filter(|(key, _)| names.contains(key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// The models of `harness`'s catalog its Cloud credentials can run. OpenCode
/// lists every OpenAI model in `-flex` and `-fast` variants (OpenAI's flex
/// and priority service tiers); a ChatGPT sign-in runs `-fast` but refuses
/// `-flex` ("Unsupported service_tier: flex"), which only an API key takes.
/// So with OpenAI reached through the sign-in alone, those aren't offered.
fn usable_with(harness: HarnessId, models: Vec<Model>, env: &BTreeMap<String, String>) -> Vec<Model> {
    if harness != HarnessId::Opencode || env.contains_key("OPENAI_API_KEY") {
        return models;
    }
    let chatgpt = env
        .get(zeron_harness::opencode::CREDENTIALS_ENV)
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .and_then(|list| list.as_array().cloned())
        .is_some_and(|list| list.iter().any(|c| c["integrationID"] == "openai"));
    if !chatgpt {
        return models;
    }
    models
        .into_iter()
        .filter(|m| !(m.id.starts_with("openai/") && m.id.ends_with("-flex")))
        .collect()
}

/// Whether this agent's catalog is published (the edge keeps Claude Code's
/// and Codex's by hand; registry agents don't run on Cloud).
fn publishes(harness: HarnessId) -> bool {
    runs_on_cloud(harness) && !matches!(harness, HarnessId::ClaudeCode | HarnessId::Codex)
}

impl ModelPublisher {
    pub fn new(registry: &Arc<HarnessRegistry>, sink: Arc<dyn CatalogSink>) -> Self {
        Self {
            inner: Arc::new(Inner {
                registry: Arc::downgrade(registry),
                sink,
                published: Mutex::new(HashMap::new()),
                busy: Mutex::new(HashSet::new()),
            }),
        }
    }

    /// Discover `harness`'s catalog and publish it, unless the same
    /// fingerprint was published within [`REPUBLISH_AFTER`]. `true` when it
    /// published.
    pub async fn refresh(&self, harness: HarnessId) -> bool {
        if !lock(&self.inner.busy).insert(harness) {
            return false;
        }
        let published = self.refresh_now(harness).await;
        lock(&self.inner.busy).remove(&harness);
        published
    }

    async fn refresh_now(&self, harness: HarnessId) -> bool {
        let Some(registry) = self.inner.registry.upgrade() else {
            return false;
        };
        // Unknown (the vault out of reach) is no fingerprint to file it under.
        let Some(credential) = registry.run_credential(harness, None).await else {
            return false;
        };
        if credential == "none" {
            return false;
        }
        if let Some((last, at)) = lock(&self.inner.published).get(&harness)
            && *last == credential
            && at.elapsed() < REPUBLISH_AFTER
        {
            return false;
        }
        // Only a list the agent gave now, with these credentials: a cached
        // or built-in stand-in (an agent's fallback when discovery fails)
        // would replace its real models everywhere.
        // A run preparing the same agent meanwhile can change its discovery
        // environment under the probe ("credentials changed"): once more.
        let mut discovered = registry.discover_catalog(harness).await;
        if discovered
            .as_ref()
            .is_err_and(|error| error.to_string().contains("credentials changed"))
        {
            discovered = registry.discover_catalog(harness).await;
        }
        let models = match discovered {
            Ok(catalog) if catalog.source == "live" && !catalog.models.is_empty() => {
                usable_with(harness, catalog.models, &zeron_harness::catalog_env::get(harness))
            }
            Ok(catalog) => {
                tracing::info!(harness = harness.as_str(), source = catalog.source,
                    "model catalogs: not live; not published");
                return false;
            }
            Err(error) => {
                tracing::info!(harness = harness.as_str(), %error, "model catalogs: discovery failed; not published");
                return false;
            }
        };
        match self.inner.sink.publish(harness, &models, &credential).await {
            Ok(()) => {
                tracing::info!(
                    harness = harness.as_str(),
                    models = models.len(),
                    "model catalogs: published"
                );
                lock(&self.inner.published).insert(harness, (credential, Instant::now()));
                true
            }
            Err(error) => {
                tracing::warn!(harness = harness.as_str(), %error, "model catalogs: publish failed");
                false
            }
        }
    }
}

/// How long after boot the first agents' catalogs are primed: the run that
/// woke the machine goes first.
const PRIME_AFTER: Duration = Duration::from_secs(20);

/// Whether a machine can list `harness`'s models without installing
/// anything for it first: its CLI is there, or (Cursor) it runs on an SDK its
/// own discovery installs.
fn discoverable_here(harness: HarnessId) -> bool {
    harness == HarnessId::Cursor || zeron_harness::install::installed(harness)
}

impl ModelPublisher {
    /// After a boot, publish the catalog of every agent this account has
    /// connected that the machine can list without installing anything: so
    /// the picker shows an agent's real models before its first Cloud run,
    /// once any machine has woken. One agent at a time; an agent with no
    /// credential is skipped (its preparation refuses). Preparing writes the
    /// agent's credential files and its discovery environment — what its
    /// first run would do anyway.
    pub fn prime(&self) {
        self.prime_with(PRIME_AFTER, discoverable_here);
    }

    fn prime_with(&self, after: Duration, discoverable: fn(HarnessId) -> bool) -> Option<tokio::task::JoinHandle<()>> {
        tokio::runtime::Handle::try_current().ok()?;
        let weak = Arc::downgrade(&self.inner);
        Some(tokio::spawn(async move {
            tokio::time::sleep(after).await;
            for harness in HarnessId::ALL.into_iter().filter(|h| publishes(*h)) {
                let Some(inner) = weak.upgrade() else { return };
                let Some(registry) = inner.registry.upgrade() else {
                    return;
                };
                if !discoverable(harness) {
                    continue;
                }
                let mut request = zeron_proto::RunRequest {
                    mcp: None,
                    prompt: String::new(),
                    harness: Some(harness),
                    model: None,
                    reasoning: None,
                    model_options: serde_json::Map::new(),
                    cwd: std::env::temp_dir().to_string_lossy().into_owned(),
                    sandbox: zeron_proto::SandboxLevel::ReadOnly,
                    auto_approve: false,
                    access: Default::default(),
                    plan: false,
                    code_mode: false,
                    attachments: Vec::new(),
                    resume: None,
                    worktree: None,
                    env: Default::default(),
                    extra_mcp: Vec::new(),
                };
                // Preparing hands the publisher this agent's credentials and
                // starts its discovery ([`CatalogPublisher::prepared`]); the
                // next agent waits for it (a bounded wait: discovery has its
                // own timeouts).
                drop(registry);
                let prepared = match inner.registry.upgrade() {
                    Some(registry) => registry.prepare_run(harness, None, &mut request).await.is_ok(),
                    None => return,
                };
                if prepared {
                    let deadline = Instant::now() + Duration::from_secs(120);
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    while lock(&inner.busy).contains(&harness) && Instant::now() < deadline {
                        tokio::time::sleep(Duration::from_millis(250)).await;
                    }
                }
            }
        }))
    }
}

impl CatalogPublisher for ModelPublisher {
    fn prepared(&self, harness: HarnessId, env: &BTreeMap<String, String>) {
        if !publishes(harness) {
            return;
        }
        // Discovery runs with this run's keys from now on, whoever asks.
        zeron_harness::catalog_env::set(harness, credential_env(harness, env));
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let this = self.clone();
        tokio::spawn(async move {
            this.refresh(harness).await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::RunEnvironment;
    use futures::stream::BoxStream;
    use zeron_harness::{Harness, HarnessError, RunControls};
    use zeron_proto::{AgentEvent, ReasoningLevel, RunRequest, SteeringMode};

    /// The catalog environment is process-wide: tests that set Muse's take
    /// turns.
    static CATALOG_ENV: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// An agent whose catalog names the key discovery ran with.
    struct KeyedAgent;

    #[async_trait::async_trait]
    impl Harness for KeyedAgent {
        fn id(&self) -> HarnessId {
            HarnessId::Muse
        }
        fn display_name(&self) -> &str {
            "keyed"
        }
        fn supports_steering(&self) -> bool {
            false
        }
        fn steering_mode(&self) -> SteeringMode {
            SteeringMode::TurnBoundary
        }
        fn reasoning_levels(&self) -> &[ReasoningLevel] {
            &[]
        }
        async fn models(&self) -> Result<Vec<Model>, HarnessError> {
            let key = zeron_harness::catalog_env::get(HarnessId::Muse)
                .remove("META_API_KEY")
                .unwrap_or_default();
            Ok(vec![Model {
                id: format!("model-for-{key}"),
                label: "M".into(),
                description: None,
                provider: None,
                reasoning_levels: Vec::new(),
                options: Vec::new(),
                default: false,
            }])
        }
        async fn run(
            &self,
            _request: RunRequest,
            _controls: RunControls,
        ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
            Err(HarnessError::Protocol("not a runner".into()))
        }
    }

    /// The run credential is the key's own generation.
    struct Fingerprint(Arc<Mutex<String>>);

    #[async_trait::async_trait]
    impl RunEnvironment for Fingerprint {
        async fn prepare(
            &self,
            _harness: HarnessId,
            _chat_id: Option<&str>,
            request: &mut RunRequest,
        ) -> Result<(), String> {
            let key = lock(&self.0).clone();
            request.env.insert("META_API_KEY".into(), key);
            request.env.insert(zeron_proto::ORIGIN_ENV.into(), "chat".into());
            Ok(())
        }
        async fn credential(&self, _harness: HarnessId, _chat_id: Option<&str>) -> Option<String> {
            Some(format!("meta-key:{}", lock(&self.0)))
        }
    }

    #[derive(Default)]
    struct Recorder(Mutex<Vec<(HarnessId, Vec<String>, String)>>);

    #[async_trait::async_trait]
    impl CatalogSink for Recorder {
        async fn publish(
            &self,
            harness: HarnessId,
            models: &[Model],
            credential: &str,
        ) -> Result<(), String> {
            lock(&self.0).push((
                harness,
                models.iter().map(|m| m.id.clone()).collect(),
                credential.into(),
            ));
            Ok(())
        }
    }

    fn request() -> RunRequest {
        serde_json::from_value(serde_json::json!({
            "prompt": "hi",
            "cwd": "/tmp",
            "sandbox": "workspace-write",
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn publishes_what_the_runs_credentials_discover_once_per_fingerprint() {
        let _turn = CATALOG_ENV.lock().await;
        let registry = Arc::new(HarnessRegistry::new());
        registry.register(Arc::new(KeyedAgent));
        let key = Arc::new(Mutex::new("k1".to_string()));
        registry.set_run_environment(Arc::new(Fingerprint(key.clone())));
        let sink = Arc::new(Recorder::default());
        let publisher = ModelPublisher::new(&registry, sink.clone());

        let mut run = request();
        registry
            .prepare_run(HarnessId::Muse, None, &mut run)
            .await
            .unwrap();
        // Only the credential variables reach discovery, never the chat's.
        let discovery = credential_env(HarnessId::Muse, &run.env);
        assert_eq!(discovery.keys().collect::<Vec<_>>(), ["META_API_KEY"]);
        // Preparing a run publishes in the background.
        publisher.prepared(HarnessId::Muse, &run.env);
        assert_eq!(zeron_harness::catalog_env::get(HarnessId::Muse), discovery);
        for _ in 0..200 {
            if !lock(&sink.0).is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            lock(&sink.0).clone(),
            vec![(HarnessId::Muse, vec!["model-for-k1".to_string()], "meta-key:k1".to_string())]
        );
        // The same fingerprint again: not published.
        assert!(!publisher.refresh(HarnessId::Muse).await);

        // A new key: a new catalog, at once.
        *lock(&key) = "k2".into();
        let mut run = request();
        registry
            .prepare_run(HarnessId::Muse, None, &mut run)
            .await
            .unwrap();
        zeron_harness::catalog_env::set(HarnessId::Muse, credential_env(HarnessId::Muse, &run.env));
        assert!(publisher.refresh(HarnessId::Muse).await);
        assert_eq!(
            lock(&sink.0).last().cloned(),
            Some((HarnessId::Muse, vec!["model-for-k2".to_string()], "meta-key:k2".to_string()))
        );
        zeron_harness::catalog_env::set(HarnessId::Muse, BTreeMap::new());
    }

    #[tokio::test]
    async fn a_boot_publishes_connected_agents_it_can_list_without_a_run() {
        let _turn = CATALOG_ENV.lock().await;
        let registry = Arc::new(HarnessRegistry::new());
        registry.register(Arc::new(KeyedAgent));
        registry.set_run_environment(Arc::new(Fingerprint(Arc::new(Mutex::new("boot".into())))));
        let sink = Arc::new(Recorder::default());
        let publisher = ModelPublisher::new(&registry, sink.clone());
        registry.set_catalog_publisher(Arc::new(publisher.clone()));
        // Nothing here can be listed: nothing is prepared or published.
        publisher.prime_with(Duration::ZERO, |_| false).unwrap().await.unwrap();
        assert!(lock(&sink.0).is_empty());
        // Muse can: its catalog, with the credentials a run would have.
        publisher.prime_with(Duration::ZERO, |h| h == HarnessId::Muse).unwrap().await.unwrap();
        assert_eq!(
            lock(&sink.0).clone(),
            vec![(HarnessId::Muse, vec!["model-for-boot".to_string()], "meta-key:boot".to_string())]
        );
        zeron_harness::catalog_env::set(HarnessId::Muse, BTreeMap::new());
    }

    #[test]
    fn opencode_on_a_chatgpt_sign_in_lists_no_flex_tier() {
        let model = |id: &str| Model {
            id: id.into(),
            label: id.into(),
            description: None,
            provider: None,
            reasoning_levels: Vec::new(),
            options: Vec::new(),
            default: false,
        };
        let all = || {
            vec![
                model("openai/gpt-5.6-sol"),
                model("openai/gpt-5.6-sol-fast"),
                model("openai/gpt-5.6-sol-flex"),
                model("openrouter/x-flex"),
            ]
        };
        let ids = |models: Vec<Model>| models.into_iter().map(|m| m.id).collect::<Vec<_>>();
        let signed_in = BTreeMap::from([(
            zeron_harness::opencode::CREDENTIALS_ENV.to_string(),
            r#"[{"integrationID":"openai","value":{"type":"oauth"}}]"#.to_string(),
        )]);
        assert_eq!(
            ids(usable_with(HarnessId::Opencode, all(), &signed_in)),
            ["openai/gpt-5.6-sol", "openai/gpt-5.6-sol-fast", "openrouter/x-flex"]
        );
        // An OpenAI key takes the flex tier; other agents are as listed.
        let mut keyed = signed_in.clone();
        keyed.insert("OPENAI_API_KEY".into(), "sk".into());
        assert_eq!(usable_with(HarnessId::Opencode, all(), &keyed).len(), 4);
        assert_eq!(usable_with(HarnessId::Pi, all(), &signed_in).len(), 4);
        assert_eq!(usable_with(HarnessId::Opencode, all(), &BTreeMap::new()).len(), 4);
    }

    #[test]
    fn claude_code_and_codex_are_never_published() {
        assert!(!publishes(HarnessId::ClaudeCode));
        assert!(!publishes(HarnessId::Codex));
        assert!(!publishes(HarnessId::Mock));
        assert!(publishes(HarnessId::Pi));
        let env = BTreeMap::from([
            ("OPENROUTER_API_KEY".to_string(), "or".to_string()),
            ("GIT_AUTHOR_NAME".to_string(), "Ada".to_string()),
            ("GEMINI_API_KEY".to_string(), "g".to_string()),
        ]);
        assert_eq!(
            credential_env(HarnessId::Pi, &env).keys().collect::<Vec<_>>(),
            ["GEMINI_API_KEY", "OPENROUTER_API_KEY"]
        );
        assert_eq!(
            credential_env(HarnessId::Antigravity, &env).keys().collect::<Vec<_>>(),
            ["GEMINI_API_KEY"]
        );
        // OpenCode's sign-ins ride its handoff variable into discovery too.
        let mut env = env;
        env.insert(
            zeron_harness::opencode::CREDENTIALS_ENV.to_string(),
            "[]".to_string(),
        );
        assert!(
            credential_env(HarnessId::Opencode, &env)
                .contains_key(zeron_harness::opencode::CREDENTIALS_ENV)
        );
        assert!(
            !credential_env(HarnessId::Pi, &env)
                .contains_key(zeron_harness::opencode::CREDENTIALS_ENV)
        );
    }
}
