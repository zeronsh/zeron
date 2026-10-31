//! S-switch: round-robin chat switching (bench.md 3.4).
//!
//! The UI's global streams stay open. Each switch closes the previous chat's
//! per-chat streams, then does what opening a chat does: `FocusChat`, then
//! `WatchDocMessages{openingTail}`, `WatchQueue`, `WatchCheckoutDiffs`,
//! `WatchCheckoutChangeRequest` and `WatchPreviews`. The time to the first
//! frame (the tail preview) and to the full authoritative reset is taken from
//! receive stamps, and every full reset is checked against the manifest.

use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::{Value, json};

use super::content::{Cadence, TurnOpts, build_turn};
use super::driver::{Ui, percentile};
use super::fixture::{self, ChatInfo, FixtureSpec, sha256_hex, text_of};
use super::{TrialCtx, Workload, load_params};
use crate::ab::Tier;
use crate::mono_now;

const CONFIG: &str = include_str!("../../scenarios/switch.toml");

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct SwitchParams {
    pub switches: usize,
    pub dwell_ms: u64,
    pub streaming: usize,
    pub settle_s: u64,
}

impl Default for SwitchParams {
    fn default() -> Self {
        Self {
            switches: 60,
            dwell_ms: 2000,
            streaming: 0,
            settle_s: 15,
        }
    }
}

pub struct Switch {
    pub p: SwitchParams,
    tier: Tier,
}

impl Switch {
    pub fn from_params(tier: Tier, overrides: &[(String, String)]) -> Result<Self> {
        let p: SwitchParams = load_params(CONFIG, tier, overrides)?;
        ensure!(p.switches >= 12, "switches must cover the 12 target chats once");
        Ok(Self { p, tier })
    }
}

/// Size class of a target chat, by its configured bytes.
fn class_of(chat: &ChatInfo) -> &'static str {
    match chat.bytes_target {
        0..=200_000 => "100k",
        200_001..=2_000_000 => "1m",
        _ => "5m",
    }
}

/// Size order interleaved: one chat of each class per round.
pub fn interleave(targets: &[ChatInfo]) -> Vec<ChatInfo> {
    let mut by_class: BTreeMap<&'static str, Vec<ChatInfo>> = BTreeMap::new();
    for chat in targets {
        by_class.entry(class_of(chat)).or_default().push(chat.clone());
    }
    let rounds = by_class.values().map(Vec::len).max().unwrap_or(0);
    let mut out = Vec::new();
    for i in 0..rounds {
        for class in ["100k", "1m", "5m"] {
            if let Some(chat) = by_class.get(class).and_then(|v| v.get(i)) {
                out.push(chat.clone());
            }
        }
    }
    out
}

fn dist(ctx: &mut TrialCtx, name: &str, values: &[f64]) {
    if values.is_empty() {
        return;
    }
    ctx.metric(&format!("{name}.p50"), percentile(values, 50.0));
    ctx.metric(&format!("{name}.p90"), percentile(values, 90.0));
    ctx.metric(&format!("{name}.max"), values.iter().copied().fold(0.0, f64::max));
}

impl Workload for Switch {
    const ID: &'static str = "S-switch";

    fn variant(&self) -> Value {
        json!({
            "switches": self.p.switches, "dwell_ms": self.p.dwell_ms,
            "streaming": self.p.streaming, "tier": self.tier.as_str(),
        })
    }

    fn variant_key(&self) -> String {
        format!(
            "n{}{}-{}",
            self.p.switches,
            if self.p.streaming > 0 {
                format!("-stream{}", self.p.streaming)
            } else {
                String::new()
            },
            self.tier.as_str()
        )
    }

    fn fixture_spec(&self) -> FixtureSpec {
        fixture::p20_switch(20)
    }

    async fn run(&self, ctx: &mut TrialCtx) -> Result<()> {
        let p = &self.p;
        let fx = ctx.fixture.clone();
        let targets: Vec<ChatInfo> = fx.chats_tagged("target").cloned().collect();
        let order = interleave(&targets);
        ensure!(order.len() == 12, "fixture has {} target chats, want 12", order.len());
        let client = ctx.connect().await?;
        let mut ui = Ui::new(client.clone(), ctx.stats.clone());
        ui.open_global().await?;

        // while-streaming variant: other chats stream at opus cadence, unwatched.
        let mut streamed = 0usize;
        if p.streaming > 0 {
            let dir = ctx.trial.root.join("scripts");
            std::fs::create_dir_all(&dir)?;
            let total_ms = p.switches as u64 * (p.dwell_ms + 1500) + 30_000;
            let bg: Vec<ChatInfo> = fx.chats_tagged("bg").take(p.streaming).cloned().collect();
            ensure!(bg.len() == p.streaming, "not enough background chats");
            for chat in &bg {
                let built = build_turn(&TurnOpts {
                    seed: 0x5e00_0000 + chat.n as u64,
                    chat8: chat.chat8.clone(),
                    payload_bytes: 1,
                    min_ms: total_ms,
                    cadence: Cadence::opus(),
                    marker_every: 0,
                    tools: true,
                    thinking: true,
                });
                let path = dir.join(format!("bg{}.zbr.jsonl", chat.n));
                std::fs::write(&path, &built.source)?;
                fixture::queue_play(
                    &client,
                    &chat.id,
                    fx.space_path(chat),
                    &fixture::message_uuid(chat.n, 7200),
                    &path,
                    1.0,
                )
                .await?;
                streamed += built.text.len();
            }
        }
        tokio::time::sleep(Duration::from_secs(p.settle_s)).await;

        ctx.open_window().await?;
        let mut first_ms: BTreeMap<&'static str, Vec<f64>> = BTreeMap::new();
        let mut full_ms: BTreeMap<&'static str, Vec<f64>> = BTreeMap::new();
        let mut first_bytes: BTreeMap<&'static str, Vec<f64>> = BTreeMap::new();
        let mut full_bytes: BTreeMap<&'static str, Vec<f64>> = BTreeMap::new();
        let mut open_all_ms = Vec::new();
        let mut mismatches = Vec::new();
        let mut previous: Option<String> = None;
        for s in 0..p.switches {
            let chat = &order[s % order.len()];
            if let Some(prev) = previous.take() {
                ui.close_chat(&prev);
            }
            let t0 = mono_now();
            let opened = ui
                .open_chat(&chat.id, fx.space_path(chat))
                .await
                .with_context(|| format!("switch {s} to chat {}", chat.n))?;
            open_all_ms.push(opened.t_done.saturating_sub(t0) as f64 / 1e6);
            let watch = ui.watch(&chat.id)?;
            watch
                .wait_for(Duration::from_secs(120), "the full reset", |st| st.full.is_some())
                .await?;
            let (first, full, entries, text) = watch.with(|st| {
                (st.first, st.full, st.view.entries.len(), text_of(&st.view))
            });
            let class = class_of(chat);
            if let (Some((t1, b1)), Some((t2, b2))) = (first, full) {
                first_ms.entry(class).or_default().push(t1.saturating_sub(t0) as f64 / 1e6);
                full_ms.entry(class).or_default().push(t2.saturating_sub(t0) as f64 / 1e6);
                first_bytes.entry(class).or_default().push(b1 as f64);
                full_bytes.entry(class).or_default().push(b2 as f64);
            }
            if entries != chat.entries || sha256_hex(&text) != chat.text_sha256 {
                mismatches.push(format!(
                    "switch {s} chat {}: {entries} entries (want {}), text {} B (want {})",
                    chat.n,
                    chat.entries,
                    text.len(),
                    chat.text_bytes
                ));
            }
            previous = Some(chat.id.clone());
            tokio::time::sleep(Duration::from_millis(p.dwell_ms)).await;
            if s + 1 == 12 {
                ctx.marks.insert("after12".into(), mono_now());
            }
        }
        ctx.marks.insert("last".into(), mono_now());
        ctx.close_window().await?;

        ctx.check(
            "resets_match_manifest",
            mismatches.is_empty(),
            mismatches.iter().take(3).cloned().collect::<Vec<_>>().join("; "),
        );
        let mut all_first = Vec::new();
        let mut all_full = Vec::new();
        for (class, values) in &first_ms {
            all_first.extend(values.iter().copied());
            dist(ctx, &format!("t_first_frame_ms.{class}"), values);
        }
        for (class, values) in &full_ms {
            all_full.extend(values.iter().copied());
            dist(ctx, &format!("t_full_reset_ms.{class}"), values);
        }
        for (class, values) in &first_bytes {
            ctx.metric(&format!("first_bytes.{class}"), values.iter().sum::<f64>() / values.len() as f64);
        }
        for (class, values) in &full_bytes {
            ctx.metric(&format!("full_bytes.{class}"), values.iter().sum::<f64>() / values.len() as f64);
        }
        dist(ctx, "t_first_frame_ms", &all_first);
        dist(ctx, "t_full_reset_ms", &all_full);
        dist(ctx, "t_open_all_ms", &open_all_ms);
        ctx.metric("switches", p.switches as f64);
        ctx.metric("streamed_text_bytes_background", streamed as f64);
        drop(ui);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_interleave_by_size_class() {
        let spec = fixture::p20_switch(1);
        let chats: Vec<ChatInfo> = spec
            .chats
            .iter()
            .enumerate()
            .filter(|(_, c)| c.tag == "target")
            .map(|(n, c)| ChatInfo {
                n,
                id: fixture::chat_uuid(n),
                chat8: fixture::chat_uuid(n)[..8].into(),
                space: c.space,
                tag: c.tag.clone(),
                bytes_target: c.bytes,
                turns: 1,
                entries: 2,
                transcript_bytes: 0,
                text_bytes: 0,
                text_sha256: String::new(),
                crash_residue: false,
            })
            .collect();
        let order = interleave(&chats);
        assert_eq!(order.len(), 12);
        let classes: Vec<_> = order.iter().take(6).map(class_of).collect();
        assert_eq!(classes, ["100k", "1m", "5m", "100k", "1m", "5m"]);
    }

    #[test]
    fn quick_tier_switches_twice_around() {
        let s = Switch::from_params(Tier::Quick, &[]).unwrap();
        assert_eq!(s.p.switches, 24);
        assert_eq!(s.variant_key(), "n24-quick");
        assert!(Switch::from_params(Tier::Quick, &[("switches".into(), "3".into())]).is_err());
    }
}
