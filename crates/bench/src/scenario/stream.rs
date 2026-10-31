//! S-stream: K concurrent long chats streaming `zb-replay` scripts (bench.md 3.4).
//!
//! The fixture is `P20` plus K dedicated target chats preloaded to the base
//! size. Every target gets a turn of `turn_ms` at the chosen cadence, with
//! thinking, tool calls and a latency marker on every 20th text delta. With
//! `watch=all` the driver holds the UI's per-chat streams on all K chats, so
//! each one applies deltas and stamps marker arrivals; with `watch=one` only
//! the first chat is watched and the rest are verified after the window.

use std::collections::BTreeSet;
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use serde_json::{Value, json};

use super::content::{Cadence, TurnOpts, build_turn};
use super::driver::{Canary, Ui, Watch};
use super::fixture::{
    self, ChatInfo, FixtureSpec, bad_status, last_turn_text, parse_size, sha256_hex, text_of,
    turn_settled,
};
use super::{TrialCtx, Workload, load_params};
use crate::ab::Tier;
use crate::mono_now;

const CONFIG: &str = include_str!("../../scenarios/stream.toml");

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct StreamParams {
    #[serde(rename = "K", alias = "k")]
    pub k: usize,
    pub base: String,
    pub watch: String,
    pub cadence: String,
    pub turn_ms: u64,
    pub marker_every: usize,
    pub settle_s: u64,
    pub tail_s: u64,
    pub canary_ms: u64,
}

impl Default for StreamParams {
    fn default() -> Self {
        Self {
            k: 4,
            base: "1m".into(),
            watch: "all".into(),
            cadence: "opus".into(),
            turn_ms: 90_000,
            marker_every: 20,
            settle_s: 20,
            tail_s: 5,
            canary_ms: 100,
        }
    }
}

pub struct Stream {
    pub p: StreamParams,
    tier: Tier,
}

impl Stream {
    pub fn from_params(tier: Tier, overrides: &[(String, String)]) -> Result<Self> {
        let p: StreamParams = load_params(CONFIG, tier, overrides)?;
        ensure!(p.k >= 1, "K must be at least 1");
        ensure!(matches!(p.watch.as_str(), "all" | "one"), "watch is all or one");
        ensure!(Cadence::parse(&p.cadence).is_some(), "cadence is opus or haiku");
        parse_size(&p.base)?;
        Ok(Self { p, tier })
    }
}

fn verify(
    chat: &ChatInfo,
    view: &crate::transcript::ChatView,
    expected: &str,
    expected_sha: &str,
) -> (bool, String) {
    if let Some(bad) = bad_status(view) {
        return (false, bad);
    }
    let full = text_of(view);
    if full.len() != chat.text_bytes + expected.len() {
        return (
            false,
            format!(
                "text is {} bytes, expected {} base + {} streamed",
                full.len(),
                chat.text_bytes,
                expected.len()
            ),
        );
    }
    if sha256_hex(&full[..chat.text_bytes]) != chat.text_sha256 {
        return (false, "base transcript differs from the manifest".into());
    }
    if sha256_hex(&full[chat.text_bytes..]) != expected_sha {
        return (false, "streamed text hash differs from the script".into());
    }
    let errors = view.entries.iter().any(|e| {
        e.get("parts")
            .and_then(Value::as_array)
            .is_some_and(|parts| parts.iter().any(|p| p.get("kind").and_then(Value::as_str) == Some("error")))
    });
    if errors {
        return (false, "error part in the transcript".into());
    }
    (true, String::new())
}

impl Workload for Stream {
    const ID: &'static str = "S-stream";

    fn variant(&self) -> Value {
        json!({
            "K": self.p.k, "base": self.p.base, "watch": self.p.watch,
            "cadence": self.p.cadence, "turn_ms": self.p.turn_ms,
            "marker_every": self.p.marker_every, "tier": self.tier.as_str(),
        })
    }

    fn variant_key(&self) -> String {
        format!(
            "k{}-{}-{}-{}-{}",
            self.p.k,
            self.p.base,
            self.p.watch,
            self.p.cadence,
            self.tier.as_str()
        )
    }

    fn fixture_spec(&self) -> FixtureSpec {
        let bytes = parse_size(&self.p.base).unwrap_or(1_000_000);
        fixture::p20(&format!("P20-k{}-{}", self.p.k, self.p.base), 20, self.p.k, bytes)
    }

    async fn run(&self, ctx: &mut TrialCtx) -> Result<()> {
        let p = &self.p;
        let fx = ctx.fixture.clone();
        let targets: Vec<ChatInfo> = fx.chats_tagged("target").cloned().collect();
        ensure!(targets.len() == p.k, "fixture has {} target chats, want {}", targets.len(), p.k);
        let client = ctx.connect().await?;
        let mut ui = Ui::new(client.clone(), ctx.stats.clone());
        ui.open_global().await.context("open the global streams")?;
        let watched_n = if p.watch == "all" { p.k } else { 1 };
        for chat in &targets[..watched_n] {
            ui.open_chat(&chat.id, fx.space_path(chat))
                .await
                .with_context(|| format!("open chat {}", chat.n))?;
        }
        for chat in &targets[..watched_n] {
            ui.watch(&chat.id)?
                .wait_for(Duration::from_secs(120), "the full reset", |s| s.full.is_some())
                .await?;
            let (ok, detail) = ui.watch(&chat.id)?.with(|s| {
                let text = text_of(&s.view);
                let ok = sha256_hex(&text) == chat.text_sha256 && text.len() == chat.text_bytes;
                (ok, format!("{} bytes, {} entries (manifest {} / {})", text.len(), s.view.entries.len(), chat.text_bytes, chat.entries))
            });
            ctx.check(&format!("base_matches_manifest:{}", chat.n), ok, if ok { String::new() } else { detail });
        }

        // One script per chat, identical across trials and sides.
        let cadence = Cadence::parse(&p.cadence).unwrap_or_else(Cadence::opus);
        let scripts_dir = ctx.trial.root.join("scripts");
        std::fs::create_dir_all(&scripts_dir)?;
        let mut turns = Vec::new();
        for chat in &targets {
            let built = build_turn(&TurnOpts {
                seed: 0x5700_0000 + chat.n as u64,
                chat8: chat.chat8.clone(),
                payload_bytes: 1,
                min_ms: p.turn_ms,
                cadence,
                marker_every: p.marker_every,
                tools: true,
                thinking: true,
            });
            let path = scripts_dir.join(format!("s{}.zbr.jsonl", chat.n));
            std::fs::write(&path, &built.source)?;
            turns.push((path, built));
        }
        let text_total: usize = turns.iter().map(|(_, t)| t.text.len()).sum();
        let payload_total: usize = turns.iter().map(|(_, t)| t.payload_bytes).sum();
        let duration_ms = turns.iter().map(|(_, t)| t.duration_ms).max().unwrap_or(0);

        tokio::time::sleep(Duration::from_secs(p.settle_s)).await;
        ctx.open_window().await?;
        let canary = Canary::start(client.clone(), Duration::from_millis(p.canary_ms));
        let t0 = mono_now();
        let queued = targets.iter().zip(&turns).map(|(chat, (path, _))| {
            let client = client.clone();
            let cwd = fx.space_path(chat).to_path_buf();
            let id = chat.id.clone();
            let msg = fixture::message_uuid(chat.n, 7000);
            let path = path.clone();
            async move { fixture::queue_play(&client, &id, &cwd, &msg, &path, 1.0).await }
        });
        futures::future::try_join_all(queued).await.context("queue the turns")?;
        let budget = Duration::from_millis(duration_ms * 2) + Duration::from_secs(90);
        for (chat, (_, built)) in targets.iter().zip(&turns).take(watched_n) {
            let expected = built.text.len();
            ui.watch(&chat.id)?
                .wait_for(budget, "the turn to complete", |s| {
                    turn_settled(&s.view) && last_turn_text(&s.view).len() >= expected
                })
                .await?;
        }
        // Unwatched chats run the same timeline; give them the scripted time.
        let elapsed = (mono_now() - t0) / 1_000_000;
        if elapsed < duration_ms + 2_000 {
            tokio::time::sleep(Duration::from_millis(duration_ms + 2_000 - elapsed)).await;
        }
        tokio::time::sleep(Duration::from_secs(p.tail_s)).await;
        ctx.close_window().await?;
        let (w0, w1) = ctx.window_ns();
        ctx.rtt_ms = canary.between(w0, w1).into_iter().map(|n| n as f64 / 1e6).collect();
        drop(canary);
        tokio::time::sleep(Duration::from_millis(300)).await;

        // Correctness: every chat's transcript is the base plus exactly the scripted turn.
        let marks = ctx.probe.marks();
        let mut expected_markers = 0usize;
        let mut seen_markers = 0usize;
        let mut lat = Vec::new();
        let mut complete = 0;
        for (chat, (_, built)) in targets.iter().zip(&turns).take(watched_n) {
            let watch = ui.watch(&chat.id)?;
            let (ok, detail, seqs, stamped) = watch.with(|s| {
                let (ok, detail) = verify(chat, &s.view, &built.text, &built.text_sha256);
                let seqs: BTreeSet<u64> = s
                    .markers
                    .iter()
                    .filter(|(c8, _, _)| *c8 == chat.chat8)
                    .map(|(_, seq, _)| *seq)
                    .collect();
                (ok, detail, seqs, s.markers.clone())
            });
            ctx.check(&format!("transcript_sha:{}", chat.n), ok, detail);
            let want: BTreeSet<u64> = (1..=built.markers as u64).collect();
            ctx.check(
                &format!("markers_seen:{}", chat.n),
                seqs == want,
                format!("saw {} of {} markers", seqs.len(), want.len()),
            );
            expected_markers += want.len();
            seen_markers += seqs.len();
            if ok {
                complete += 1;
            }
            for (c8, seq, t_recv) in stamped {
                if let Some(t_w) = marks.get(&(c8, seq))
                    && t_recv >= *t_w
                {
                    lat.push((t_recv - t_w) as f64 / 1e6);
                }
            }
        }
        // Chats nobody watched are verified from a fresh stream, after the window.
        for (chat, (_, built)) in targets.iter().zip(&turns).skip(watched_n) {
            let sub = client
                .subscribe("WatchDocMessages", json!({"chatId": chat.id, "openingTail": true}))
                .await?;
            let watch = Watch::spawn(chat.id.clone(), sub, ctx.stats.clone());
            watch
                .wait_for(Duration::from_secs(120), "the full reset", |s| s.full.is_some())
                .await?;
            let (ok, detail) = watch.with(|s| verify(chat, &s.view, &built.text, &built.text_sha256));
            ctx.check(&format!("transcript_sha:{}", chat.n), ok, detail);
            if ok {
                complete += 1;
            }
        }
        ctx.check("status_complete", complete == p.k, format!("{complete} of {} chats verified", p.k));
        if lat.is_empty() && watched_n > 0 {
            bail!("no latency samples: probe saw {} marks, watchers saw {} markers", marks.len(), seen_markers);
        }
        ctx.stream_lat_ms = lat;
        ctx.metric("streamed_text_bytes", text_total as f64);
        ctx.metric("streamed_payload_bytes", payload_total as f64);
        ctx.metric("markers_expected", expected_markers as f64);
        ctx.metric("markers_seen", seen_markers as f64);
        ctx.metric("chats_streamed", p.k as f64);
        ctx.metric("chats_watched", watched_n as f64);
        drop(ui);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_merge_tier_then_overrides() {
        let s = Stream::from_params(Tier::Quick, &[("K".into(), "2".into()), ("watch".into(), "one".into())]).unwrap();
        assert_eq!(s.p.k, 2);
        assert_eq!(s.p.watch, "one");
        assert_eq!(s.p.turn_ms, 45_000);
        assert_eq!(s.variant_key(), "k2-1m-one-opus-quick");
        let std = Stream::from_params(Tier::Standard, &[]).unwrap();
        assert_eq!(std.p.turn_ms, 90_000);
        assert!(Stream::from_params(Tier::Quick, &[("watch".into(), "some".into())]).is_err());
    }

    #[test]
    fn fixture_has_k_targets_of_the_base_size() {
        let s = Stream::from_params(Tier::Quick, &[("base".into(), "5m".into())]).unwrap();
        let spec = s.fixture_spec();
        let targets: Vec<_> = spec.chats.iter().filter(|c| c.tag == "target").collect();
        assert_eq!(targets.len(), 4);
        assert!(targets.iter().all(|c| c.bytes == 5_000_000));
    }
}
