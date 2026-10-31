//! API-price spend from the agents' own local logs (after t3code's
//! Usage page). Claude Code (`~/.claude/projects`), Codex (`~/.codex/sessions`)
//! and Pi (`~/.pi/agent/sessions`) each write every request's token counts to
//! JSONL transcripts; this reads them, buckets tokens per (local day,
//! provider, model), and prices them with LiteLLM's public rate table. Pi
//! records its own cost, which wins over the table.
//!
//! A scan keeps a per-file cache (`<data dir>/usage-scan-cache.json`, keyed by
//! path, size and mtime), so only new or grown transcripts are read again.
//! Everything here is blocking and runs on a background thread.
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Local, NaiveDate};
use serde::{Deserialize, Serialize};
use zeron_proto::HarnessId;

const CACHE_FILE: &str = "usage-scan-cache.json";
const CACHE_VERSION: u32 = 1;
const PRICES_FILE: &str = "usage-prices.json";
const PRICES_URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";
const PRICES_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const SCAN_THREADS: usize = 4;
/// Pi forks copy the parent's history into the new file, stamped before the
/// fork's own header; anything older than that is billed in the parent.
const FORK_SLACK_MS: i64 = 1000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tokens {
    pub input: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub output: u64,
}

impl Tokens {
    pub fn total(self) -> u64 {
        self.input + self.cache_read + self.cache_write + self.output
    }

    fn add(&mut self, other: Tokens) {
        self.input += other.input;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
        self.output += other.output;
    }
}

/// One (day, provider, model) cell.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bucket {
    pub day: NaiveDate,
    pub provider: HarnessId,
    /// Shown name.
    pub model: String,
    /// Name looked up in the rate table.
    pub rate_model: String,
    /// Every token, for display.
    pub tokens: Tokens,
    /// Tokens without a provider-reported cost; priced from the table.
    pub unreported: Tokens,
    /// Cost the provider wrote into the log (Pi).
    pub reported_usd: f64,
    pub requests: u64,
}

/// A transcript's usage before bucketing.
struct Record {
    day: NaiveDate,
    provider: HarnessId,
    model: String,
    rate_model: String,
    tokens: Tokens,
    reported_usd: Option<f64>,
}

fn bucketize(records: Vec<Record>) -> Vec<Bucket> {
    let mut map: BTreeMap<(NaiveDate, String, String), Bucket> = BTreeMap::new();
    for record in records {
        let key = (record.day, record.model.clone(), record.rate_model.clone());
        let bucket = map.entry(key).or_insert_with(|| Bucket {
            day: record.day,
            provider: record.provider,
            model: record.model.clone(),
            rate_model: record.rate_model.clone(),
            tokens: Tokens::default(),
            unreported: Tokens::default(),
            reported_usd: 0.0,
            requests: 0,
        });
        bucket.tokens.add(record.tokens);
        bucket.requests += 1;
        match record.reported_usd {
            Some(usd) => bucket.reported_usd += usd,
            None => bucket.unreported.add(record.tokens),
        }
    }
    map.into_values().collect()
}

fn local_day(timestamp: &str) -> Option<(NaiveDate, i64)> {
    let at = DateTime::parse_from_rfc3339(timestamp).ok()?;
    Some((at.with_timezone(&Local).date_naive(), at.timestamp_millis()))
}

fn count(value: Option<f64>) -> u64 {
    value
        .filter(|v| v.is_finite() && *v > 0.0)
        .map_or(0, |v| v as u64)
}

// ---------------------------------------------------------------- Claude Code

#[derive(Deserialize)]
struct ClaudeLine {
    #[serde(rename = "type")]
    kind: Option<String>,
    timestamp: Option<String>,
    #[serde(rename = "requestId")]
    request_id: Option<String>,
    #[serde(rename = "costUSD")]
    cost_usd: Option<f64>,
    message: Option<ClaudeMessage>,
}

#[derive(Deserialize)]
struct ClaudeMessage {
    id: Option<String>,
    model: Option<String>,
    usage: Option<ClaudeUsage>,
}

#[derive(Deserialize)]
struct ClaudeUsage {
    input_tokens: Option<f64>,
    output_tokens: Option<f64>,
    cache_creation_input_tokens: Option<f64>,
    cache_read_input_tokens: Option<f64>,
}

/// Claude writes one line per content block, each repeating the message's
/// full usage: keep the first per message and request.
pub fn parse_claude(text: &str) -> Vec<Bucket> {
    let mut seen = HashSet::new();
    let mut records = Vec::new();
    for line in text.lines() {
        if !line.contains("\"usage\"") {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<ClaudeLine>(line) else {
            continue;
        };
        if entry.kind.as_deref() != Some("assistant") {
            continue;
        }
        let Some(message) = entry.message else { continue };
        let (Some(usage), Some(model)) = (message.usage, message.model) else {
            continue;
        };
        if model.is_empty() || model == "<synthetic>" {
            continue;
        }
        let Some((day, _)) = entry.timestamp.as_deref().and_then(local_day) else {
            continue;
        };
        if (message.id.is_some() || entry.request_id.is_some())
            && !seen.insert((message.id.clone(), entry.request_id.clone()))
        {
            continue;
        }
        let tokens = Tokens {
            input: count(usage.input_tokens),
            cache_read: count(usage.cache_read_input_tokens),
            cache_write: count(usage.cache_creation_input_tokens),
            output: count(usage.output_tokens),
        };
        if tokens.total() == 0 {
            continue;
        }
        records.push(Record {
            day,
            provider: HarnessId::ClaudeCode,
            rate_model: model.clone(),
            model,
            tokens,
            reported_usd: entry.cost_usd.filter(|usd| usd.is_finite() && *usd > 0.0),
        });
    }
    bucketize(records)
}

// ---------------------------------------------------------------------- Codex

#[derive(Deserialize)]
struct CodexLine {
    #[serde(rename = "type")]
    kind: Option<String>,
    timestamp: Option<String>,
    payload: Option<CodexPayload>,
}

#[derive(Deserialize)]
struct CodexPayload {
    #[serde(rename = "type")]
    kind: Option<String>,
    model: Option<String>,
    info: Option<CodexInfo>,
    forked_from_id: Option<String>,
    source: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct CodexInfo {
    last_token_usage: Option<CodexTokens>,
    total_token_usage: Option<CodexTokens>,
}

#[derive(Clone, Copy, Deserialize, PartialEq)]
struct CodexTokens {
    input_tokens: Option<f64>,
    cached_input_tokens: Option<f64>,
    cache_write_input_tokens: Option<f64>,
    output_tokens: Option<f64>,
}

impl CodexTokens {
    fn tokens(self) -> Tokens {
        let input = count(self.input_tokens);
        let cached = count(self.cached_input_tokens);
        let write = count(self.cache_write_input_tokens);
        Tokens {
            // Codex counts the cached part inside input_tokens.
            input: input.saturating_sub(cached + write),
            cache_read: cached,
            cache_write: write,
            output: count(self.output_tokens),
        }
    }
}

/// Codex `token_count` events carry the last request and the running total;
/// the model comes from the latest `turn_context`. Re-emitted counters (same
/// total) and a fork's copied history (a burst right after a forked
/// `session_meta`) are skipped.
pub fn parse_codex(text: &str) -> Vec<Bucket> {
    let mut model = String::new();
    let mut previous_total: Option<CodexTokens> = None;
    let mut previous_last: Option<CodexTokens> = None;
    let mut fork_anchor: Option<i64> = None;
    let mut saw_meta = false;
    let mut records = Vec::new();
    for line in text.lines() {
        if !(line.contains("\"token_count\"")
            || line.contains("\"turn_context\"")
            || line.contains("\"session_meta\""))
        {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<CodexLine>(line) else {
            continue;
        };
        let Some(payload) = entry.payload else { continue };
        let at = entry.timestamp.as_deref().and_then(local_day);
        match entry.kind.as_deref() {
            Some("session_meta") => {
                if !saw_meta {
                    saw_meta = true;
                    let forked = payload.forked_from_id.is_some()
                        || payload
                            .source
                            .as_ref()
                            .and_then(|s| s.pointer("/subagent/thread_spawn/parent_thread_id"))
                            .is_some_and(|v| v.is_string());
                    if forked {
                        fork_anchor = at.map(|(_, ms)| ms);
                    }
                }
                continue;
            }
            Some("turn_context") => {
                if let Some(next) = payload.model.filter(|m| !m.is_empty()) {
                    model = next;
                }
                continue;
            }
            _ => {}
        }
        if payload.kind.as_deref() != Some("token_count") {
            continue;
        }
        let Some(info) = payload.info else { continue };
        let Some((day, ms)) = at else { continue };
        if let Some(anchor) = fork_anchor {
            if ms - anchor < FORK_SLACK_MS {
                fork_anchor = Some(ms);
                if info.total_token_usage.is_some() {
                    previous_total = info.total_token_usage;
                }
                continue;
            }
            fork_anchor = None;
        }
        let total = info.total_token_usage;
        if total.is_some() && total == previous_total {
            continue;
        }
        if total.is_some() {
            previous_total = total;
        }
        let Some(last) = info.last_token_usage else { continue };
        if total.is_none() && previous_last == Some(last) {
            continue;
        }
        previous_last = Some(last);
        let tokens = last.tokens();
        if count(last.input_tokens) + tokens.output == 0 || model.is_empty() {
            continue;
        }
        records.push(Record {
            day,
            provider: HarnessId::Codex,
            model: model.clone(),
            rate_model: model.clone(),
            tokens,
            reported_usd: None,
        });
    }
    bucketize(records)
}

// ------------------------------------------------------------------------- Pi

#[derive(Deserialize)]
struct PiLine {
    #[serde(rename = "type")]
    kind: Option<String>,
    timestamp: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    usage: Option<PiUsage>,
    message: Option<PiMessage>,
}

#[derive(Deserialize)]
struct PiMessage {
    role: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    #[serde(rename = "responseModel")]
    response_model: Option<String>,
    usage: Option<PiUsage>,
}

#[derive(Deserialize)]
struct PiUsage {
    input: Option<f64>,
    output: Option<f64>,
    #[serde(rename = "cacheRead")]
    cache_read: Option<f64>,
    #[serde(rename = "cacheWrite")]
    cache_write: Option<f64>,
    cost: Option<PiCost>,
}

#[derive(Deserialize)]
struct PiCost {
    total: Option<f64>,
}

/// Pi's assistant messages carry usage and (when it knows the rates) cost.
pub fn parse_pi(text: &str) -> Vec<Bucket> {
    let mut header_ms: Option<i64> = None;
    let mut records = Vec::new();
    for line in text.lines() {
        let header = header_ms.is_none() && line.contains("\"session\"");
        if !header && !line.contains("\"usage\"") {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<PiLine>(line) else {
            continue;
        };
        let at = entry.timestamp.as_deref().and_then(local_day);
        if entry.kind.as_deref() == Some("session") {
            header_ms = at.map(|(_, ms)| ms);
            continue;
        }
        let Some((day, ms)) = at else { continue };
        if header_ms.is_some_and(|start| ms + FORK_SLACK_MS < start) {
            continue;
        }
        let (provider, model, response_model, usage) = match entry.kind.as_deref() {
            Some("message") => {
                let Some(message) = entry.message else { continue };
                if message.role.as_deref() != Some("assistant") {
                    continue;
                }
                (message.provider, message.model, message.response_model, message.usage)
            }
            Some("usage") => (entry.provider, entry.model, None, entry.usage),
            _ => continue,
        };
        let (Some(usage), Some(model)) = (usage, model) else { continue };
        let tokens = Tokens {
            input: count(usage.input),
            cache_read: count(usage.cache_read),
            cache_write: count(usage.cache_write),
            output: count(usage.output),
        };
        let cost = usage
            .cost
            .and_then(|cost| cost.total)
            .filter(|usd| usd.is_finite() && *usd > 0.0);
        if tokens.total() == 0 && cost.is_none() {
            continue;
        }
        let rate_model = response_model.unwrap_or_else(|| model.clone());
        let shown = match provider {
            Some(provider) if !provider.is_empty() => format!("{provider}/{rate_model}"),
            _ => rate_model.clone(),
        };
        records.push(Record {
            day,
            provider: HarnessId::Pi,
            model: shown,
            rate_model,
            tokens,
            reported_usd: cost,
        });
    }
    bucketize(records)
}

// -------------------------------------------------------------------- pricing

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rate {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

pub type Rates = HashMap<String, Rate>;

/// LiteLLM's table, per token. Cache rates fall back to the input rate;
/// models missing input or output rates are dropped. Bare names (after the
/// last `/`) alias the full key when unambiguous.
pub fn parse_rates(json: &[u8]) -> Rates {
    let Ok(document) = serde_json::from_slice::<HashMap<String, serde_json::Value>>(json) else {
        return Rates::new();
    };
    let number = |entry: &serde_json::Value, key: &str| {
        entry.get(key).and_then(|v| v.as_f64()).filter(|v| v.is_finite())
    };
    let mut rates = Rates::new();
    for (name, entry) in &document {
        let (Some(input), Some(output)) = (
            number(entry, "input_cost_per_token"),
            number(entry, "output_cost_per_token"),
        ) else {
            continue;
        };
        rates.insert(
            name.trim().to_lowercase(),
            Rate {
                input,
                output,
                cache_read: number(entry, "cache_read_input_token_cost").unwrap_or(input),
                cache_write: number(entry, "cache_creation_input_token_cost").unwrap_or(input),
            },
        );
    }
    let mut aliases: HashMap<String, Option<Rate>> = HashMap::new();
    for (key, rate) in &rates {
        let Some((_, bare)) = key.rsplit_once('/') else { continue };
        if bare.is_empty() || rates.contains_key(bare) {
            continue;
        }
        aliases
            .entry(bare.to_owned())
            .and_modify(|held| {
                if *held != Some(*rate) {
                    *held = None;
                }
            })
            .or_insert(Some(*rate));
    }
    rates.extend(
        aliases
            .into_iter()
            .filter_map(|(alias, rate)| Some((alias, rate?))),
    );
    rates
}

/// The rate for a logged model name. Bare family names ("opus") are
/// ambiguous and stay unpriced.
pub fn lookup<'a>(rates: &'a Rates, model: &str) -> Option<&'a Rate> {
    let key = model.trim().to_lowercase();
    let key = key.split('[').next().unwrap_or_default();
    let bare = key.rsplit('/').next().unwrap_or(key);
    if bare.is_empty() || matches!(bare, "synthetic" | "opus" | "sonnet" | "haiku") {
        return None;
    }
    rates.get(key).or_else(|| rates.get(bare))
}

fn price(tokens: Tokens, rate: &Rate) -> f64 {
    tokens.input as f64 * rate.input
        + tokens.cache_read as f64 * rate.cache_read
        + tokens.cache_write as f64 * rate.cache_write
        + tokens.output as f64 * rate.output
}

// -------------------------------------------------------------------- summary

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Period {
    Today,
    Week,
    Month,
    All,
}

impl Period {
    pub const ALL: [Period; 4] = [Period::Today, Period::Week, Period::Month, Period::All];

    pub fn label(self) -> &'static str {
        match self {
            Period::Today => "Today",
            Period::Week => "7 days",
            Period::Month => "30 days",
            Period::All => "All",
        }
    }

    /// The first day inside the period, `None` for all time.
    pub fn since(self, today: NaiveDate) -> Option<NaiveDate> {
        let back = match self {
            Period::Today => 0,
            Period::Week => 6,
            Period::Month => 29,
            Period::All => return None,
        };
        today.checked_sub_days(chrono::Days::new(back))
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Line {
    pub cost_usd: f64,
    pub tokens: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Summary {
    pub cost_usd: f64,
    pub tokens: Tokens,
    /// Tokens neither reported nor in the rate table.
    pub unpriced_tokens: u64,
    pub requests: u64,
    pub by_provider: Vec<(HarnessId, Line)>,
    /// (shown model, provider), most expensive first.
    pub by_model: Vec<(String, HarnessId, Line)>,
    /// Spend per day across the chart window, oldest first.
    pub daily: Vec<(NaiveDate, f64)>,
}

/// Pure: everything the card shows for one period.
pub fn summarize(buckets: &[Bucket], rates: &Rates, period: Period, today: NaiveDate) -> Summary {
    let since = period.since(today);
    // The chart covers the period (all time charts the last 30 days).
    let chart_days: u64 = match period {
        Period::Today => 1,
        Period::Week => 7,
        Period::Month | Period::All => 30,
    };
    let chart_since = today
        .checked_sub_days(chrono::Days::new(chart_days - 1))
        .unwrap_or(today);
    let mut summary = Summary::default();
    let mut providers: HashMap<HarnessId, Line> = HashMap::new();
    let mut models: HashMap<(String, HarnessId), Line> = HashMap::new();
    let mut daily: BTreeMap<NaiveDate, f64> = BTreeMap::new();
    for bucket in buckets {
        let rate = lookup(rates, &bucket.rate_model);
        let priced = rate.map_or(0.0, |rate| price(bucket.unreported, rate));
        let cost = bucket.reported_usd + priced;
        if bucket.day >= chart_since && bucket.day <= today {
            *daily.entry(bucket.day).or_default() += cost;
        }
        if since.is_some_and(|since| bucket.day < since) {
            continue;
        }
        summary.cost_usd += cost;
        summary.tokens.add(bucket.tokens);
        summary.requests += bucket.requests;
        if rate.is_none() {
            summary.unpriced_tokens += bucket.unreported.total();
        }
        let total = bucket.tokens.total();
        let line = providers.entry(bucket.provider).or_default();
        line.cost_usd += cost;
        line.tokens += total;
        let line = models
            .entry((bucket.model.clone(), bucket.provider))
            .or_default();
        line.cost_usd += cost;
        line.tokens += total;
    }
    let order = |a: &Line, b: &Line| {
        b.cost_usd
            .total_cmp(&a.cost_usd)
            .then(b.tokens.cmp(&a.tokens))
    };
    summary.by_provider = providers.into_iter().collect();
    summary.by_provider.sort_by(|a, b| order(&a.1, &b.1));
    summary.by_model = models
        .into_iter()
        .map(|((model, provider), line)| (model, provider, line))
        .collect();
    summary
        .by_model
        .sort_by(|a, b| order(&a.2, &b.2).then_with(|| a.0.cmp(&b.0)));
    let mut day = chart_since;
    while day <= today {
        summary
            .daily
            .push((day, daily.get(&day).copied().unwrap_or_default()));
        let Some(next) = day.succ_opt() else { break };
        day = next;
    }
    summary
}

/// `$12.34`, `<$0.01`, `$1.2k`.
pub fn format_usd(usd: f64) -> String {
    if usd <= 0.0 {
        "$0".into()
    } else if usd < 0.01 {
        "<$0.01".into()
    } else if usd < 1000.0 {
        format!("${usd:.2}")
    } else if usd < 100_000.0 {
        format!("${:.1}k", usd / 1000.0)
    } else {
        format!("${:.0}k", usd / 1000.0)
    }
}

/// `950`, `12.3K`, `4.5M`, `1.2B`.
pub fn format_tokens(tokens: u64) -> String {
    let t = tokens as f64;
    if tokens < 1000 {
        tokens.to_string()
    } else if t < 1e6 {
        format!("{:.1}K", t / 1e3)
    } else if t < 1e9 {
        format!("{:.1}M", t / 1e6)
    } else {
        format!("{:.1}B", t / 1e9)
    }
}

// ----------------------------------------------------------------------- scan

#[derive(Default, Serialize, Deserialize)]
struct ScanCache {
    version: u32,
    files: HashMap<String, CachedFile>,
}

#[derive(Clone, Serialize, Deserialize)]
struct CachedFile {
    size: u64,
    mtime_ms: u128,
    buckets: Vec<Bucket>,
}

/// One scan's result.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub buckets: Vec<Bucket>,
    pub rates: Rates,
    /// Whether a rate table loaded (fresh or cached).
    pub has_rates: bool,
    pub files: usize,
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Where each agent keeps its transcripts.
fn roots() -> Vec<(HarnessId, PathBuf)> {
    let Some(home) = home() else { return Vec::new() };
    let claude = env_dir("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home.join(".claude"));
    let codex = env_dir("CODEX_HOME").unwrap_or_else(|| home.join(".codex"));
    let pi = env_dir("PI_CODING_AGENT_DIR").unwrap_or_else(|| home.join(".pi/agent"));
    vec![
        (HarnessId::ClaudeCode, claude.join("projects")),
        (HarnessId::Codex, codex.join("sessions")),
        (HarnessId::Codex, codex.join("archived_sessions")),
        (HarnessId::Pi, pi.join("sessions")),
    ]
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else { continue };
        let path = entry.path();
        if kind.is_dir() {
            walk(&path, out);
        } else if kind.is_file() && path.extension().is_some_and(|ext| ext == "jsonl") {
            out.push(path);
        }
    }
}

fn stamp(path: &Path) -> Option<(u64, u128)> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()?
        .as_millis();
    Some((meta.len(), mtime))
}

fn parse_file(provider: HarnessId, path: &Path) -> Vec<Bucket> {
    let Ok(bytes) = std::fs::read(path) else { return Vec::new() };
    let text = String::from_utf8_lossy(&bytes);
    match provider {
        HarnessId::ClaudeCode => parse_claude(&text),
        HarnessId::Codex => parse_codex(&text),
        HarnessId::Pi => parse_pi(&text),
        _ => Vec::new(),
    }
}

/// The rate table: refreshed from LiteLLM once a day, else the last copy.
fn load_rates(data_dir: &Path) -> Option<Rates> {
    let path = data_dir.join(PRICES_FILE);
    let fresh = std::fs::metadata(&path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|at| at.elapsed().ok())
        .is_some_and(|age| age < PRICES_TTL);
    if !fresh {
        let staging = data_dir.join(format!("{PRICES_FILE}.download"));
        let ok = std::process::Command::new("/usr/bin/curl")
            .args(["-sSfL", "--max-time", "20", "-o"])
            .arg(&staging)
            .arg(PRICES_URL)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        let valid = ok
            && std::fs::read(&staging)
                .ok()
                .is_some_and(|bytes| !parse_rates(&bytes).is_empty());
        if valid {
            let _ = std::fs::rename(&staging, &path);
        } else {
            let _ = std::fs::remove_file(&staging);
        }
    }
    let rates = parse_rates(&std::fs::read(&path).ok()?);
    (!rates.is_empty()).then_some(rates)
}

/// Read every transcript (re-reading only changed ones) and the rate table.
/// Blocking: call from a background thread.
pub fn scan(data_dir: &Path) -> Report {
    static SCAN_LOCK: Mutex<()> = Mutex::new(());
    let _guard = SCAN_LOCK.lock();
    let cache_path = data_dir.join(CACHE_FILE);
    let mut cache: ScanCache = std::fs::read(&cache_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .filter(|cache: &ScanCache| cache.version == CACHE_VERSION)
        .unwrap_or_default();
    let mut files: Vec<(HarnessId, PathBuf)> = Vec::new();
    for (provider, root) in roots() {
        let mut found = Vec::new();
        walk(&root, &mut found);
        files.extend(found.into_iter().map(|path| (provider, path)));
    }
    let mut stale: Vec<(HarnessId, PathBuf, u64, u128)> = Vec::new();
    let mut fresh: HashMap<String, CachedFile> = HashMap::new();
    for (provider, path) in &files {
        let Some((size, mtime_ms)) = stamp(path) else { continue };
        let key = path.to_string_lossy().into_owned();
        match cache.files.remove(&key) {
            Some(hit) if hit.size == size && hit.mtime_ms == mtime_ms => {
                fresh.insert(key, hit);
            }
            _ => stale.push((*provider, path.clone(), size, mtime_ms)),
        }
    }
    let changed = !stale.is_empty() || !cache.files.is_empty();
    let queue = Mutex::new(stale);
    let done = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..SCAN_THREADS {
            scope.spawn(|| {
                loop {
                    let Some((provider, path, size, mtime_ms)) =
                        queue.lock().ok().and_then(|mut queue| queue.pop())
                    else {
                        break;
                    };
                    let buckets = parse_file(provider, &path);
                    if let Ok(mut done) = done.lock() {
                        done.push((
                            path.to_string_lossy().into_owned(),
                            CachedFile {
                                size,
                                mtime_ms,
                                buckets,
                            },
                        ));
                    }
                }
            });
        }
    });
    fresh.extend(done.into_inner().unwrap_or_default());
    let buckets: Vec<Bucket> = fresh
        .values()
        .flat_map(|file| file.buckets.iter().cloned())
        .collect();
    let file_count = fresh.len();
    if changed {
        let cache = ScanCache {
            version: CACHE_VERSION,
            files: fresh,
        };
        if let Ok(bytes) = serde_json::to_vec(&cache) {
            let staging = cache_path.with_extension("json.tmp");
            if std::fs::write(&staging, bytes).is_ok() {
                let _ = std::fs::rename(&staging, &cache_path);
            }
        }
    }
    let rates = load_rates(data_dir);
    Report {
        buckets,
        has_rates: rates.is_some(),
        rates: rates.unwrap_or_default(),
        files: file_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    // Noon UTC stays on the same local day in every real timezone.
    const NOON: &str = "2026-03-10T12:00:00Z";

    #[test]
    fn claude_counts_each_message_once_and_skips_synthetic() {
        let line = |id: &str, model: &str| {
            format!(
                r#"{{"type":"assistant","timestamp":"{NOON}","requestId":"r-{id}","message":{{"id":"{id}","model":"{model}","usage":{{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":100,"cache_creation_input_tokens":20}}}}}}"#
            )
        };
        let text = [
            line("a", "claude-sonnet-4-5"),
            line("a", "claude-sonnet-4-5"),
            line("b", "claude-sonnet-4-5"),
            line("c", "<synthetic>"),
            r#"{"type":"user","message":{"content":"hi"}}"#.into(),
        ]
        .join("\n");
        let buckets = parse_claude(&text);
        assert_eq!(buckets.len(), 1);
        let bucket = &buckets[0];
        assert_eq!(bucket.day, day("2026-03-10"));
        assert_eq!(bucket.requests, 2);
        assert_eq!(
            bucket.tokens,
            Tokens {
                input: 20,
                cache_read: 200,
                cache_write: 40,
                output: 10
            }
        );
        assert_eq!(bucket.unreported, bucket.tokens);
    }

    #[test]
    fn codex_bills_new_requests_with_the_current_model() {
        let count = |last: u64, total: u64| {
            format!(
                r#"{{"timestamp":"{NOON}","type":"event_msg","payload":{{"type":"token_count","info":{{"last_token_usage":{{"input_tokens":{last},"cached_input_tokens":40,"output_tokens":7}},"total_token_usage":{{"input_tokens":{total},"cached_input_tokens":40,"output_tokens":7}}}}}}}}"#
            )
        };
        let text = [
            format!(r#"{{"timestamp":"{NOON}","type":"session_meta","payload":{{"id":"s"}}}}"#),
            format!(r#"{{"timestamp":"{NOON}","type":"turn_context","payload":{{"model":"gpt-5"}}}}"#),
            count(100, 100),
            // Re-emitted counter: same total.
            count(100, 100),
            count(50, 150),
        ]
        .join("\n");
        let buckets = parse_codex(&text);
        assert_eq!(buckets.len(), 1);
        assert_eq!(buckets[0].model, "gpt-5");
        assert_eq!(buckets[0].requests, 2);
        // 100-40 + 50-40 uncached input, 40+40 cached.
        assert_eq!(buckets[0].tokens.input, 70);
        assert_eq!(buckets[0].tokens.cache_read, 80);
        assert_eq!(buckets[0].tokens.output, 14);
    }

    #[test]
    fn codex_skips_a_forks_copied_history() {
        let text = [
            r#"{"timestamp":"2026-03-10T12:00:00.000Z","type":"session_meta","payload":{"id":"s","forked_from_id":"p"}}"#,
            r#"{"timestamp":"2026-03-10T12:00:00.000Z","type":"turn_context","payload":{"model":"gpt-5"}}"#,
            r#"{"timestamp":"2026-03-10T12:00:00.020Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":900,"output_tokens":9},"total_token_usage":{"input_tokens":900,"output_tokens":9}}}}"#,
            r#"{"timestamp":"2026-03-10T12:00:09.000Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":10,"output_tokens":1},"total_token_usage":{"input_tokens":910,"output_tokens":10}}}}"#,
        ]
        .join("\n");
        let buckets = parse_codex(&text);
        assert_eq!(buckets.len(), 1);
        assert_eq!(buckets[0].requests, 1);
        assert_eq!(buckets[0].tokens.input, 10);
    }

    #[test]
    fn pi_keeps_reported_cost_and_skips_fork_copies() {
        let text = [
            r#"{"type":"session","id":"s","timestamp":"2026-03-10T12:00:00Z"}"#,
            // Copied from the parent before the fork's header.
            r#"{"type":"message","id":"old","timestamp":"2026-03-09T12:00:00Z","message":{"role":"assistant","provider":"anthropic","model":"claude-opus","usage":{"input":1,"output":1,"cost":{"total":5.0}}}}"#,
            r#"{"type":"message","id":"a","timestamp":"2026-03-10T12:00:05Z","message":{"role":"assistant","provider":"larp","model":"group/opus","responseModel":"claude-opus-4-1","usage":{"input":100,"output":10,"cacheRead":1000,"cacheWrite":0,"cost":{"total":0.25}}}}"#,
            r#"{"type":"message","id":"b","timestamp":"2026-03-10T12:00:06Z","message":{"role":"assistant","provider":"larp","model":"group/opus","responseModel":"claude-opus-4-1","usage":{"input":5,"output":5,"cost":{"total":0}}}}"#,
            r#"{"type":"message","id":"u","timestamp":"2026-03-10T12:00:07Z","message":{"role":"user","content":"x"}}"#,
        ]
        .join("\n");
        let buckets = parse_pi(&text);
        assert_eq!(buckets.len(), 1);
        let bucket = &buckets[0];
        assert_eq!(bucket.model, "larp/claude-opus-4-1");
        assert_eq!(bucket.rate_model, "claude-opus-4-1");
        assert_eq!(bucket.requests, 2);
        assert!((bucket.reported_usd - 0.25).abs() < 1e-9);
        // Only the zero-cost message needs the table.
        assert_eq!(bucket.unreported.total(), 10);
    }

    #[test]
    fn rates_parse_with_cache_fallbacks_and_bare_aliases() {
        let json = br#"{
            "anthropic/claude-x": {"input_cost_per_token": 0.000003, "output_cost_per_token": 0.000015, "cache_read_input_token_cost": 0.0000003},
            "gpt-y": {"input_cost_per_token": 0.000001, "output_cost_per_token": 0.000002},
            "broken": {"input_cost_per_token": 0.1},
            "sample_spec": {"max_tokens": 1}
        }"#;
        let rates = parse_rates(json);
        let claude = lookup(&rates, "Claude-X[1m]").expect("bare alias, variant stripped");
        assert!((claude.cache_read - 0.0000003).abs() < 1e-12);
        assert!((claude.cache_write - 0.000003).abs() < 1e-12);
        assert!(lookup(&rates, "gpt-y").is_some());
        assert!(lookup(&rates, "broken").is_none());
        assert!(lookup(&rates, "opus").is_none());
    }

    #[test]
    fn summary_prices_unreported_tokens_and_windows_by_period() {
        let today = day("2026-03-10");
        let bucket = |d: &str, model: &str, provider, reported: f64, unreported: u64| Bucket {
            day: day(d),
            provider,
            model: model.into(),
            rate_model: model.into(),
            tokens: Tokens {
                input: unreported + 10,
                ..Tokens::default()
            },
            unreported: Tokens {
                input: unreported,
                ..Tokens::default()
            },
            reported_usd: reported,
            requests: 1,
        };
        let buckets = vec![
            bucket("2026-03-10", "gpt-y", HarnessId::Codex, 0.0, 1_000_000),
            bucket("2026-03-09", "pi-model", HarnessId::Pi, 2.0, 0),
            bucket("2026-01-01", "mystery", HarnessId::ClaudeCode, 0.0, 500),
        ];
        let mut rates = Rates::new();
        rates.insert(
            "gpt-y".into(),
            Rate {
                input: 0.000001,
                output: 0.0,
                cache_read: 0.0,
                cache_write: 0.0,
            },
        );
        let today_only = summarize(&buckets, &rates, Period::Today, today);
        assert!((today_only.cost_usd - 1.0).abs() < 1e-9);
        assert_eq!(today_only.daily.len(), 1);
        let week = summarize(&buckets, &rates, Period::Week, today);
        assert!((week.cost_usd - 3.0).abs() < 1e-9);
        assert_eq!(week.by_provider[0].0, HarnessId::Pi);
        assert_eq!(week.daily.len(), 7);
        assert_eq!(week.unpriced_tokens, 0);
        let all = summarize(&buckets, &rates, Period::All, today);
        assert_eq!(all.unpriced_tokens, 500);
        assert_eq!(all.by_model.len(), 3);
        assert_eq!(all.daily.len(), 30);
    }

    #[test]
    fn money_and_tokens_format_compactly() {
        assert_eq!(format_usd(0.0), "$0");
        assert_eq!(format_usd(0.004), "<$0.01");
        assert_eq!(format_usd(12.345), "$12.35");
        assert_eq!(format_usd(1234.0), "$1.2k");
        assert_eq!(format_tokens(950), "950");
        assert_eq!(format_tokens(12_300), "12.3K");
        assert_eq!(format_tokens(4_500_000), "4.5M");
    }
}
