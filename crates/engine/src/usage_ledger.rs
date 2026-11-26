//! Engine-owned token usage ledger.
//!
//! Providers' transcripts are their own formats and often outlive the account
//! or model that wrote them; scanning them backwards can only ever be a guess.
//! Zeron is already on the event path, so every `AgentEvent::Usage` is written
//! here as it happens, one JSON object per line in `{data_dir}/usage-events.jsonl`.
//! The analytics RPC reads this file alone. A line carries only metadata and
//! counters — no prompts, responses, access tokens or credential material.
//!
//! Token fields keep the provider's meaning: `input_tokens` is uncached input
//! where the provider separates cache traffic, cache channels are additional,
//! and output is provider-reported output (Anthropic includes thinking there).
//! Cost is present only when the provider itself reported it; nothing here
//! estimates subscription usage from a price table.

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageRecord {
    /// Unix seconds.
    pub at: i64,
    pub device_id: String,
    pub chat_id: String,
    pub harness: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Stable opaque key of the account/credential the run started with, when
    /// the host knows one. It is a digest, never the credential itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_key: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

impl UsageRecord {
    fn total_tokens(&self) -> u64 {
        self.input_tokens
            .saturating_add(self.cache_read_input_tokens.unwrap_or(0))
            .saturating_add(self.cache_creation_input_tokens.unwrap_or(0))
            .saturating_add(self.output_tokens)
    }
}

#[derive(Default, Clone, Copy)]
struct Totals {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_creation: u64,
    reasoning: u64,
    total: u64,
    cost: f64,
    cost_rows: u64,
    rows: u64,
}

impl Totals {
    fn add(&mut self, row: &UsageRecord) {
        self.input = self.input.saturating_add(row.input_tokens);
        self.output = self.output.saturating_add(row.output_tokens);
        self.cache_read = self
            .cache_read
            .saturating_add(row.cache_read_input_tokens.unwrap_or(0));
        self.cache_creation = self
            .cache_creation
            .saturating_add(row.cache_creation_input_tokens.unwrap_or(0));
        self.reasoning = self
            .reasoning
            .saturating_add(row.reasoning_tokens.unwrap_or(0));
        self.total = self.total.saturating_add(row.total_tokens());
        if let Some(cost) = row.cost_usd {
            self.cost += cost;
            self.cost_rows += 1;
        }
        self.rows += 1;
    }

    fn value(&self) -> Value {
        let mut out = json!({
            "inputTokens": self.input,
            "outputTokens": self.output,
            "cacheReadInputTokens": self.cache_read,
            "cacheCreationInputTokens": self.cache_creation,
            "reasoningTokens": self.reasoning,
            "totalTokens": self.total,
            "events": self.rows,
        });
        if self.cost_rows > 0 {
            out["costUsd"] = json!(self.cost);
            out["costEvents"] = json!(self.cost_rows);
        }
        out
    }
}

pub struct UsageLedger {
    path: PathBuf,
}

impl UsageLedger {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    pub fn append(&self, row: &UsageRecord) -> Result<(), std::io::Error> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        serde_json::to_writer(&mut file, row)?;
        file.write_all(b"\n")
    }

    fn rows(&self) -> Vec<UsageRecord> {
        let Ok(file) = std::fs::File::open(&self.path) else {
            return Vec::new();
        };
        BufReader::new(file)
            .lines()
            .map_while(Result::ok)
            .filter_map(|line| serde_json::from_str(&line).ok())
            .collect()
    }

    /// Aggregate rows in `[from, to)` (Unix seconds; either end optional).
    /// `bucket_seconds` aligns bucket starts to the Unix epoch; the app's day
    /// ranges are UTC, matching Cloud's usage days.
    pub fn summary(&self, from: Option<i64>, to: Option<i64>, bucket_seconds: i64) -> Value {
        let bucket_seconds = bucket_seconds.max(60);
        let mut totals = Totals::default();
        let mut buckets: BTreeMap<i64, Totals> = BTreeMap::new();
        let mut by_harness: BTreeMap<String, Totals> = BTreeMap::new();
        let mut by_model: BTreeMap<String, Totals> = BTreeMap::new();
        let mut by_account: BTreeMap<String, Totals> = BTreeMap::new();
        let mut started: Option<i64> = None;
        let mut rows_in_range = 0u64;
        for row in self.rows() {
            started = Some(started.map_or(row.at, |at| at.min(row.at)));
            if from.is_some_and(|at| row.at < at) || to.is_some_and(|at| row.at >= at) {
                continue;
            }
            rows_in_range += 1;
            totals.add(&row);
            let start = row.at.div_euclid(bucket_seconds) * bucket_seconds;
            buckets.entry(start).or_default().add(&row);
            by_harness.entry(row.harness.clone()).or_default().add(&row);
            let model = row.model.clone().unwrap_or_else(|| "unknown".into());
            by_model.entry(model).or_default().add(&row);
            if let Some(account) = &row.account_key {
                by_account.entry(account.clone()).or_default().add(&row);
            }
        }
        let named = |rows: BTreeMap<String, Totals>, key: &str| {
            rows.into_iter()
                .map(|(name, totals)| {
                    let mut row = totals.value();
                    row[key] = json!(name);
                    row
                })
                .collect::<Vec<_>>()
        };
        json!({
            "totals": totals.value(),
            "buckets": buckets.into_iter().map(|(start, totals)| {
                let mut row = totals.value();
                row["start"] = json!(start);
                row
            }).collect::<Vec<_>>(),
            "byHarness": named(by_harness, "harness"),
            "byModel": named(by_model, "model"),
            "byAccount": named(by_account, "accountKey"),
            "coverage": {
                // The ledger starts with this build: nothing can be said about
                // turns before its first row, and a provider that emits no usage
                // event is absent rather than zero.
                "complete": false,
                "missingCost": totals.cost_rows < rows_in_range,
                "startedCollectingAt": started,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(at: i64, harness: &str, input: u64, output: u64) -> UsageRecord {
        UsageRecord {
            at,
            device_id: "dev".into(),
            chat_id: "chat".into(),
            harness: harness.into(),
            model: Some("model".into()),
            account_key: Some("account".into()),
            input_tokens: input,
            output_tokens: output,
            cache_read_input_tokens: None,
            cache_creation_input_tokens: None,
            reasoning_tokens: None,
            cost_usd: None,
        }
    }

    #[test]
    fn summary_aggregates_cache_cost_buckets_and_groups() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = UsageLedger::new(dir.path().join("usage-events.jsonl"));
        let mut first = row(86_400 + 10, "claude-code", 10, 2);
        first.cache_read_input_tokens = Some(40);
        first.cache_creation_input_tokens = Some(5);
        first.cost_usd = Some(0.25);
        let mut second = row(86_400 + 20, "codex", 4, 1);
        second.reasoning_tokens = Some(3);
        ledger.append(&first).unwrap();
        ledger.append(&second).unwrap();
        ledger.append(&row(2 * 86_400 + 10, "codex", 8, 2)).unwrap();

        let summary = ledger.summary(Some(86_400), Some(2 * 86_400), 86_400);
        assert_eq!(summary["totals"]["inputTokens"], 14);
        assert_eq!(summary["totals"]["outputTokens"], 3);
        assert_eq!(summary["totals"]["cacheReadInputTokens"], 40);
        assert_eq!(summary["totals"]["cacheCreationInputTokens"], 5);
        assert_eq!(summary["totals"]["reasoningTokens"], 3);
        assert_eq!(summary["totals"]["totalTokens"], 62);
        assert_eq!(summary["totals"]["costUsd"], 0.25);
        assert_eq!(summary["buckets"].as_array().unwrap().len(), 1);
        assert_eq!(summary["buckets"][0]["start"], 86_400);
        assert_eq!(summary["byHarness"].as_array().unwrap().len(), 2);
        assert_eq!(summary["coverage"]["missingCost"], true);
    }

    #[test]
    fn summary_filters_the_range_and_tolerates_a_torn_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage-events.jsonl");
        let ledger = UsageLedger::new(path.clone());
        ledger.append(&row(100, "pi", 1, 1)).unwrap();
        ledger.append(&row(200, "pi", 2, 2)).unwrap();
        std::fs::write(
            &path,
            format!("{}{{broken\n", std::fs::read_to_string(&path).unwrap()),
        )
        .unwrap();
        let summary = ledger.summary(Some(150), None, 60);
        assert_eq!(summary["totals"]["events"], 1);
        assert_eq!(summary["totals"]["inputTokens"], 2);
        assert_eq!(summary["coverage"]["startedCollectingAt"], 100);
    }
}
