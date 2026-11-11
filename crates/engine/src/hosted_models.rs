//! The hosted model list: a small JSON file on Zeron's edge
//! (`<edge>/models/v1.json`) that lets the model pickers learn new models,
//! a better order and better labels without a Zeron release.
//!
//! It only ever adjusts the catalog the harness already built (curated rows
//! plus what the installed CLI reports): it can add a row the installed CLI is
//! new enough for, reorder rows, relabel them and hide them. It never changes
//! what a row launches, its effort ladder or options of a row that exists, and
//! a bad, oversized or missing list leaves the built-in catalog as it is.
//!
//! The engine fetches it (so every client and device it serves gets it),
//! lazily when a catalog is listed and the copy on disk is older than
//! [`TTL`], with `If-None-Match`. The last good list is kept on disk and used
//! offline; a list that fails [`validate`] is dropped and never cached.
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use zeron_proto::{HarnessId, Model, ModelOption, ModelOptionChoice, ReasoningLevel};

/// The schema this engine reads. A breaking change ships at a new path
/// (`/models/v2.json`) with `schema: 2`; this engine keeps reading v1.
pub const SCHEMA: u32 = 1;
pub const PATH: &str = "/models/v1.json";
/// Larger bodies are refused unread past this many bytes.
pub const MAX_BYTES: usize = 64 * 1024;
const MAX_HARNESSES: usize = 16;
const MAX_MODELS: usize = 100;
const MAX_ALIASES: usize = 8;
const MAX_ID: usize = 128;
const MAX_LABEL: usize = 64;
const MAX_DESCRIPTION: usize = 200;
/// How long a fetched list is fresh; after this a listing refetches it.
pub const TTL: Duration = Duration::from_secs(6 * 60 * 60);
/// After a failed fetch, wait this long before the next try.
const RETRY: Duration = Duration::from_secs(30 * 60);
const TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_EDGE: &str = "https://edge.zeron.sh";

// ── format ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostedList {
    pub schema: u32,
    /// Keyed by the harness's wire id (`claude-code`, `codex`, …). Unknown
    /// harness ids are kept and ignored (a newer list may name more).
    #[serde(default)]
    pub harnesses: HashMap<String, HostedHarness>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostedHarness {
    #[serde(default)]
    pub models: Vec<HostedModel>,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostedModel {
    /// The id the CLI is launched with. Matches a catalog row exactly.
    pub id: String,
    pub label: Option<String>,
    pub description: Option<String>,
    /// Lower sorts first. Rows the list doesn't order stay after the row
    /// they followed.
    pub order: Option<i32>,
    /// Preselect this row. Only used when the catalog didn't come from the
    /// CLI itself (the CLI's own default always wins).
    #[serde(default)]
    pub default: bool,
    /// The oldest CLI version that runs this model. Required to add a row;
    /// on an existing row, an older (known) CLI hides it.
    pub min_cli: Option<String>,
    #[serde(default)]
    pub hidden: bool,
    /// Other ids the CLI may report for this model (a dated snapshot, a
    /// gateway name). When both are listed, the alias row folds into this one.
    #[serde(default)]
    pub aliases: Vec<String>,
    /// For an added row only: its effort ladder, in the CLI's words
    /// (`low`, `medium`, `high`, `xhigh`, `max`).
    #[serde(default)]
    pub efforts: Vec<String>,
    /// For an added row only: named option sets the engine knows
    /// (`fastMode`, `thinking`, `contextWindow`, `contextWindow1m`). Unknown
    /// names are skipped.
    #[serde(default)]
    pub options: Vec<String>,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:/[]@-".contains(&b))
}

fn valid_text(text: &str, max: usize) -> bool {
    let trimmed = text.trim();
    !trimmed.is_empty() && trimmed.chars().count() <= max && !text.chars().any(char::is_control)
}

/// Parse and check a hosted list. Any error rejects the whole list: a list
/// that is half right is not used at all.
pub fn validate(bytes: &[u8]) -> Result<HostedList, String> {
    if bytes.len() > MAX_BYTES {
        return Err(format!(
            "{} bytes is over the {MAX_BYTES}-byte limit",
            bytes.len()
        ));
    }
    let list: HostedList = serde_json::from_slice(bytes).map_err(|e| format!("bad JSON: {e}"))?;
    if list.schema != SCHEMA {
        return Err(format!("schema {} is not {SCHEMA}", list.schema));
    }
    if list.harnesses.len() > MAX_HARNESSES {
        return Err("too many harnesses".into());
    }
    for (harness, block) in &list.harnesses {
        let fail = |what: String| Err(format!("{harness}: {what}"));
        if block.models.len() > MAX_MODELS {
            return fail("too many models".into());
        }
        let mut ids = HashSet::new();
        let mut defaults = 0;
        for model in &block.models {
            if !valid_id(&model.id) {
                return fail(format!("bad id {:?}", model.id));
            }
            if !ids.insert(model.id.as_str()) {
                return fail(format!("{} is listed twice", model.id));
            }
            if model
                .label
                .as_deref()
                .is_some_and(|l| !valid_text(l, MAX_LABEL))
            {
                return fail(format!("{}: bad label", model.id));
            }
            if model
                .description
                .as_deref()
                .is_some_and(|d| !valid_text(d, MAX_DESCRIPTION))
            {
                return fail(format!("{}: bad description", model.id));
            }
            if let Some(min) = &model.min_cli
                && semver::Version::parse(min).is_err()
            {
                return fail(format!("{}: minCli {min:?} is not a version", model.id));
            }
            if model.aliases.len() > MAX_ALIASES
                || model.aliases.iter().any(|a| !valid_id(a) || *a == model.id)
            {
                return fail(format!("{}: bad aliases", model.id));
            }
            if model.efforts.len() > 8 || model.options.len() > 8 {
                return fail(format!("{}: too many efforts or options", model.id));
            }
            if model.efforts.iter().any(|e| effort(e).is_none()) {
                return fail(format!("{}: unknown effort", model.id));
            }
            if model.default && model.hidden {
                return fail(format!("{}: hidden and default", model.id));
            }
            defaults += usize::from(model.default);
        }
        if defaults > 1 {
            return fail("more than one default".into());
        }
    }
    Ok(list)
}

fn effort(name: &str) -> Option<ReasoningLevel> {
    Some(match name {
        "low" => ReasoningLevel::Low,
        "medium" => ReasoningLevel::Medium,
        "high" => ReasoningLevel::High,
        "xhigh" => ReasoningLevel::XHigh,
        "max" => ReasoningLevel::Max,
        _ => return None,
    })
}

fn select(id: &str, label: &str, choices: [(&str, &str); 2], default: &str) -> ModelOption {
    ModelOption {
        id: id.into(),
        label: label.into(),
        choices: choices
            .into_iter()
            .map(|(id, label)| ModelOptionChoice {
                id: id.into(),
                label: label.into(),
            })
            .collect(),
        default_choice: default.into(),
    }
}

/// The option sets a hosted row may name: the same selects the curated
/// catalogs build, so an added row reads like a curated one.
fn option(name: &str) -> Option<ModelOption> {
    let toggle = |id, label| select(id, label, [("off", "Off"), ("on", "On")], "off");
    let window = |default| {
        select(
            "contextWindow",
            "Context Window",
            [("200k", "200K"), ("1m", "1M")],
            default,
        )
    };
    Some(match name {
        "fastMode" => toggle("fastMode", "Fast Mode"),
        "thinking" => toggle("thinking", "Thinking"),
        "contextWindow" => window("200k"),
        "contextWindow1m" => window("1m"),
        _ => return None,
    })
}

// ── merge ───────────────────────────────────────────────────────────────

/// What the catalog was built from, for the default rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// From the CLI (live, or its saved copy): its default stands.
    Cli,
    /// The built-in rows only: a hosted default may replace the built-in guess.
    BuiltIn,
}

fn too_old(min: Option<&str>, cli: Option<&semver::Version>) -> bool {
    match (min.and_then(|m| semver::Version::parse(m).ok()), cli) {
        (Some(min), Some(cli)) => *cli < min,
        _ => false,
    }
}

/// Apply one harness's hosted entries to the catalog it built. `cli` is the
/// installed CLI's version, when known.
///
/// The rules, in order:
/// 1. Aliases: when a row and its alias are both listed, the alias row goes
///    (its default mark moves to the row).
/// 2. Add: an entry with no row becomes one only if it has `minCli` and the
///    CLI is known to be at least that version, and it isn't hidden.
/// 3. Relabel: `label` / `description` replace the row's text.
/// 4. Hide: `hidden`, or `minCli` above a known CLI, drops the row — never the
///    preselected row, and never every row.
/// 5. Default: on a built-in catalog, a hosted default row is preselected.
/// 6. Order: rows with `order` sort by it; every other row stays right after
///    the row it followed.
pub fn apply(
    mut models: Vec<Model>,
    hosted: &HostedHarness,
    cli: Option<&str>,
    source: Source,
) -> Vec<Model> {
    if hosted.models.is_empty() || models.is_empty() {
        return models;
    }
    let cli = cli.and_then(|v| semver::Version::parse(v.trim()).ok());
    let entry = |id: &str| hosted.models.iter().find(|m| m.id == id);

    // 1. aliases
    for hosted in &hosted.models {
        if !models.iter().any(|m| m.id == hosted.id) {
            continue;
        }
        for alias in &hosted.aliases {
            if let Some(at) = models.iter().position(|m| m.id == *alias) {
                let gone = models.remove(at);
                if gone.default {
                    for model in &mut models {
                        model.default = model.id == hosted.id;
                    }
                }
            }
        }
    }

    // 2. add
    for hosted in &hosted.models {
        let known_alias = hosted
            .aliases
            .iter()
            .any(|alias| models.iter().any(|m| m.id == *alias));
        if hosted.hidden
            || known_alias
            || models.iter().any(|m| m.id == hosted.id)
            || hosted.min_cli.is_none()
            || cli.is_none()
            || too_old(hosted.min_cli.as_deref(), cli.as_ref())
        {
            continue;
        }
        let mut ladder: Vec<ReasoningLevel> = Vec::new();
        for level in hosted.efforts.iter().filter_map(|e| effort(e)) {
            if !ladder.contains(&level) {
                ladder.push(level);
            }
        }
        models.push(Model {
            default: false,
            id: hosted.id.clone(),
            label: hosted.label.clone().unwrap_or_else(|| hosted.id.clone()),
            description: hosted.description.clone(),
            reasoning_levels: ladder,
            options: hosted.options.iter().filter_map(|o| option(o)).collect(),
        });
    }

    // 3. relabel
    for model in &mut models {
        if let Some(hosted) = entry(&model.id) {
            if let Some(label) = &hosted.label {
                model.label = label.trim().to_owned();
            }
            if let Some(description) = &hosted.description {
                model.description = Some(description.trim().to_owned());
            }
        }
    }

    // 4. hide
    let hide = |model: &Model| {
        !model.default
            && entry(&model.id)
                .is_some_and(|h| h.hidden || too_old(h.min_cli.as_deref(), cli.as_ref()))
    };
    if models.iter().any(|m| !hide(m)) {
        models.retain(|m| !hide(m));
    }

    // 5. default
    if source == Source::BuiltIn
        && let Some(default) = hosted.models.iter().find(|h| h.default)
        && models.iter().any(|m| m.id == default.id)
    {
        for model in &mut models {
            model.default = model.id == default.id;
        }
    }

    // 6. order
    let mut key = i64::MIN;
    let mut keyed: Vec<(i64, Model)> = models
        .into_iter()
        .map(|model| {
            if let Some(order) = entry(&model.id).and_then(|h| h.order) {
                key = i64::from(order);
            }
            (key, model)
        })
        .collect();
    keyed.sort_by_key(|(key, _)| *key);
    keyed.into_iter().map(|(_, model)| model).collect()
}

// ── cache + fetch ──────────────────────────────────────────────────────

#[derive(Serialize, Deserialize)]
struct Saved {
    schema_version: u32,
    fetched_at: u64,
    url: String,
    etag: Option<String>,
    /// The list as served (re-validated on every read).
    body: String,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// The list's URL on `edge`, or `None` when that edge may not serve it: only
/// HTTPS on Zeron's own domain (`zeron.sh` and its subdomains).
pub fn list_url(edge: &str) -> Option<String> {
    let url = reqwest::Url::parse(edge.trim()).ok()?;
    let host = url.host_str()?;
    let ours = host == "zeron.sh" || host.ends_with(".zeron.sh");
    (url.scheme() == "https" && ours && url.port().is_none() && url.username().is_empty())
        .then(|| format!("https://{host}{PATH}"))
}

struct State {
    list: Option<HostedList>,
    fetched_at: u64,
    etag: Option<String>,
    /// No fetch before this (unix seconds): backoff after a failure.
    retry_at: u64,
    fetching: bool,
}

/// The engine's copy of the hosted list: read from disk once, refreshed in
/// the background, applied to every catalog the engine lists.
pub struct HostedModels {
    path: PathBuf,
    url: Option<String>,
    client: Option<reqwest::Client>,
    state: Mutex<State>,
}

impl HostedModels {
    /// For the engine's own edge (`ZERON_EDGE_URL`, production's when unset).
    pub fn for_engine(data_dir: &Path) -> Arc<Self> {
        let edge = std::env::var("ZERON_EDGE_URL")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_EDGE.into());
        let url = list_url(&edge);
        if url.is_none() {
            tracing::info!(%edge, "Hosted model list off: edge is not HTTPS on zeron.sh");
        }
        Self::new(data_dir, url, false)
    }

    /// The one copy per data dir, shared by every catalog listing. Unit
    /// tests get one that never fetches.
    pub fn shared(data_dir: &Path) -> Arc<Self> {
        static SHARED: std::sync::OnceLock<Mutex<HashMap<PathBuf, Arc<HostedModels>>>> =
            std::sync::OnceLock::new();
        let mut shared = SHARED.get_or_init(Default::default).lock().unwrap();
        shared
            .entry(data_dir.to_path_buf())
            .or_insert_with(|| {
                if cfg!(test) {
                    Self::new(data_dir, None, false)
                } else {
                    Self::for_engine(data_dir)
                }
            })
            .clone()
    }

    /// `insecure_for_tests` lets a test serve plain HTTP from loopback.
    fn new(data_dir: &Path, url: Option<String>, insecure_for_tests: bool) -> Arc<Self> {
        let path = data_dir.join("model-catalogs").join("hosted-v1.json");
        let saved = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Saved>(&bytes).ok())
            .filter(|saved| saved.schema_version == 1 && Some(&saved.url) == url.as_ref());
        let (list, fetched_at, etag) = match saved {
            Some(saved) => match validate(saved.body.as_bytes()) {
                Ok(list) => (Some(list), saved.fetched_at, saved.etag),
                Err(error) => {
                    tracing::warn!(%error, "Saved hosted model list is invalid; ignored");
                    (None, 0, None)
                }
            },
            None => (None, 0, None),
        };
        let client = url.as_ref().and_then(|_| {
            reqwest::Client::builder()
                .https_only(!insecure_for_tests)
                // A redirect could leave our domain: none are followed.
                .redirect(reqwest::redirect::Policy::none())
                .timeout(TIMEOUT)
                .user_agent(concat!("zeron-engine/", env!("CARGO_PKG_VERSION")))
                .build()
                .ok()
        });
        Arc::new(Self {
            path,
            url,
            client,
            state: Mutex::new(State {
                list,
                fetched_at,
                etag,
                retry_at: 0,
                fetching: false,
            }),
        })
    }

    /// The current list's entries for `harness`, if any.
    pub fn for_harness(&self, harness: HarnessId) -> Option<HostedHarness> {
        let state = self.state.lock().unwrap();
        state
            .list
            .as_ref()?
            .harnesses
            .get(harness.as_str())
            .cloned()
    }

    /// Apply the current list to a catalog; also starts a background refresh
    /// when the list is stale (the refreshed list applies from the next call).
    pub fn apply(
        self: &Arc<Self>,
        harness: HarnessId,
        models: Vec<Model>,
        cli: Option<&str>,
        source: Source,
    ) -> Vec<Model> {
        self.refresh_if_stale();
        match self.for_harness(harness) {
            Some(hosted) => apply(models, &hosted, cli, source),
            None => models,
        }
    }

    fn refresh_if_stale(self: &Arc<Self>) {
        if self.client.is_none() {
            return;
        }
        {
            let mut state = self.state.lock().unwrap();
            let now = now();
            if state.fetching
                || now < state.retry_at
                || now.saturating_sub(state.fetched_at) < TTL.as_secs()
            {
                return;
            }
            state.fetching = true;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            self.state.lock().unwrap().fetching = false;
            return;
        };
        let this = self.clone();
        runtime.spawn(async move {
            let result = this.fetch().await;
            let mut state = this.state.lock().unwrap();
            state.fetching = false;
            if let Err(error) = result {
                tracing::info!(%error, "Hosted model list not refreshed; keeping the last good one");
                state.retry_at = now() + RETRY.as_secs();
            }
        });
    }

    /// Fetch once (conditional on the saved ETag), validate, then save and
    /// use it. On any failure the list in use is kept.
    pub async fn fetch(&self) -> Result<(), String> {
        let (Some(client), Some(url)) = (&self.client, &self.url) else {
            return Err("no list URL".into());
        };
        let etag = self.state.lock().unwrap().etag.clone();
        let mut request = client.get(url);
        if let Some(etag) = &etag {
            request = request.header(reqwest::header::IF_NONE_MATCH, etag);
        }
        let mut response = request.send().await.map_err(|e| e.to_string())?;
        let status = response.status();
        if status == reqwest::StatusCode::NOT_MODIFIED && etag.is_some() {
            let body = {
                let mut state = self.state.lock().unwrap();
                state.fetched_at = now();
                state
                    .list
                    .as_ref()
                    .and_then(|l| serde_json::to_string(l).ok())
            };
            if let Some(body) = body {
                self.save(etag, body)?;
            }
            return Ok(());
        }
        if status == reqwest::StatusCode::NOT_FOUND {
            // No list published here: the built-in catalog alone, from now on.
            let mut state = self.state.lock().unwrap();
            state.list = None;
            state.etag = None;
            state.fetched_at = now();
            let _ = std::fs::remove_file(&self.path);
            return Ok(());
        }
        if !status.is_success() {
            return Err(format!("HTTP {status}"));
        }
        if response
            .content_length()
            .is_some_and(|len| len > MAX_BYTES as u64)
        {
            return Err("list is over the size limit".into());
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
            body.extend_from_slice(&chunk);
            if body.len() > MAX_BYTES {
                return Err("list is over the size limit".into());
            }
        }
        let list = validate(&body)?;
        let etag = response
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let text = String::from_utf8(body).map_err(|e| e.to_string())?;
        {
            let mut state = self.state.lock().unwrap();
            state.list = Some(list);
            state.etag = etag.clone();
            state.fetched_at = now();
        }
        self.save(etag, text)
    }

    fn save(&self, etag: Option<String>, body: String) -> Result<(), String> {
        let saved = Saved {
            schema_version: 1,
            fetched_at: now(),
            url: self.url.clone().unwrap_or_default(),
            etag,
            body,
        };
        let bytes = serde_json::to_vec(&saved).map_err(|e| e.to_string())?;
        let write = || -> std::io::Result<()> {
            std::fs::create_dir_all(self.path.parent().unwrap())?;
            let staging = self
                .path
                .with_extension(format!("{}.partial", uuid::Uuid::new_v4()));
            if let Err(error) = std::fs::write(&staging, &bytes)
                .and_then(|()| std::fs::rename(&staging, &self.path))
            {
                let _ = std::fs::remove_file(&staging);
                return Err(error);
            }
            Ok(())
        };
        write().map_err(|e| e.to_string())
    }
}

#[cfg(test)]
#[path = "hosted_models_tests.rs"]
mod tests;
