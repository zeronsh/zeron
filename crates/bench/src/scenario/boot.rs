//! S-boot: start the engine on a large profile (bench.md 3.4).
//!
//! The launcher returns at `EngineReady` (`t_ready`). The window starts
//! there, so it covers everything the engine still does after it is ready.
//! `t_chats` is the first time `WatchChats` lists every chat; `t_settle` is
//! the start of the first 3 s stretch in which tree CPU stays under 2% of a
//! core. CPU spent before the launcher returned is read from the cgroup.

use std::collections::VecDeque;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{Value, json};

use super::driver::Watch;
use super::fixture::Profile;
use super::{TrialCtx, Workload, load_params};
use crate::ab::Tier;
use crate::mono_now;

const CONFIG: &str = include_str!("../../scenarios/boot.toml");

/// Quiet means this fraction of one core over the trailing window.
const SETTLE_CORE_FRACTION: f64 = 0.02;
const SETTLE_WINDOW_NS: u64 = 3_000_000_000;
const POLL: Duration = Duration::from_millis(250);

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct BootParams {
    pub max_wait_s: u64,
    pub hold_s: u64,
    pub ui: bool,
}

impl Default for BootParams {
    fn default() -> Self {
        Self {
            max_wait_s: 120,
            hold_s: 30,
            ui: false,
        }
    }
}

pub struct Boot {
    pub p: BootParams,
    tier: Tier,
}

impl Boot {
    pub fn from_params(tier: Tier, overrides: &[(String, String)]) -> Result<Self> {
        let p: BootParams = load_params(CONFIG, tier, overrides)?;
        Ok(Self { p, tier })
    }
}

/// Number of chat rows in one `WatchChats` item, whatever the envelope.
pub fn count_chats(item: &Value) -> usize {
    fn rows(v: &Value) -> Option<usize> {
        match v {
            Value::Array(items) => Some(items.len()),
            Value::Object(map) => {
                for key in ["chats", "items", "rows", "snapshot", "value"] {
                    if let Some(n) = map.get(key).and_then(rows) {
                        return Some(n);
                    }
                }
                None
            }
            _ => None,
        }
    }
    rows(item).unwrap_or(0)
}

fn panic_in_logs(ctx: &TrialCtx) -> Option<String> {
    let mut files = vec![
        ctx.trial.root.join("engine.stdout"),
        ctx.trial.root.join("engine.stderr"),
    ];
    let logs = ctx.trial.data.join("logs");
    if let Ok(rd) = std::fs::read_dir(&logs) {
        files.extend(rd.flatten().map(|e| e.path()));
    }
    for file in files {
        if let Ok(bytes) = std::fs::read(&file)
            && String::from_utf8_lossy(&bytes)
                .to_ascii_lowercase()
                .contains("panicked at")
        {
            return Some(file.display().to_string());
        }
    }
    None
}

impl Workload for Boot {
    const ID: &'static str = "S-boot";

    fn variant(&self) -> Value {
        json!({
            "profile": "P200",
            "max_wait_s": self.p.max_wait_s, "hold_s": self.p.hold_s, "ui": self.p.ui,
            "tier": self.tier.as_str(),
        })
    }

    fn variant_key(&self) -> String {
        format!(
            "p200{}-{}",
            if self.p.ui { "-ui" } else { "" },
            self.tier.as_str()
        )
    }

    fn profile(&self) -> Profile {
        Profile::P200
    }

    async fn run(&self, ctx: &mut TrialCtx) -> Result<()> {
        let p = &self.p;
        let fx = ctx.fixture.clone();
        let want = fx.chats.len();
        // CPU burned before the launcher returned (the cgroup counts since scope start).
        if let Some(us) = ctx.boot_cpu_us {
            ctx.metric("cpu_s_to_ready", us as f64 / 1e6);
        }
        ctx.metric("t_listen_ms", ctx.ms_since_spawn(ctx.t_listen));
        ctx.metric("t_ready_ms", ctx.ms_since_spawn(ctx.t_ready));
        ctx.open_window().await?;
        let client = ctx.connect().await?;

        // t_chats: the first WatchChats item that lists every chat.
        let mut sub = client.subscribe("WatchChats", json!({})).await?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
        let mut t_chats = None;
        let mut listed = 0;
        while t_chats.is_none() {
            let item = tokio::time::timeout_at(deadline, sub.recv())
                .await
                .context("WatchChats never listed every chat")?
                .context("WatchChats closed")??;
            listed = count_chats(&item.value);
            if listed >= want {
                t_chats = Some(item.t_recv_ns);
            }
        }
        if let Some(t) = t_chats {
            ctx.metric("t_chats_ms", ctx.ms_since_spawn(t));
        }
        ctx.check(
            "chats_listed",
            listed == want,
            format!("{listed} of {want} chats"),
        );
        drop(sub);

        let mut ui = None;
        if p.ui {
            let mut u = super::driver::Ui::new(client.clone(), ctx.stats.clone());
            u.open_global().await?;
            if let Some(chat) = fx.class("100k", true).first() {
                let t = u.open_chat(&chat.id, &fx.space_path(chat)).await?;
                u.watch(&chat.id)?
                    .wait_for(Duration::from_secs(120), "first frame", |s| {
                        s.first.is_some()
                    })
                    .await?;
                if let Some((t_first, _)) = u.watch(&chat.id)?.with(|s| s.first) {
                    ctx.metric(
                        "t_first_frame_ms",
                        t_first.saturating_sub(t.t_start) as f64 / 1e6,
                    );
                }
            }
            ui = Some(u);
        }

        // t_settle: poll tree CPU until a 3 s stretch stays under 2% of a core.
        let mut samples: VecDeque<(u64, u64)> = VecDeque::new();
        let give_up = tokio::time::Instant::now() + Duration::from_secs(p.max_wait_s);
        let mut settled: Option<(u64, u64)> = None;
        loop {
            if let Some(cpu) = ctx.tree_cpu_us() {
                let now = mono_now();
                samples.push_back((now, cpu));
                while samples.len() > 2 && now - samples[1].0 >= SETTLE_WINDOW_NS {
                    samples.pop_front();
                }
                let (t0, c0) = samples[0];
                if now - t0 >= SETTLE_WINDOW_NS {
                    let used_s = (cpu - c0) as f64 / 1e6;
                    let wall_s = (now - t0) as f64 / 1e9;
                    if used_s / wall_s < SETTLE_CORE_FRACTION {
                        settled = Some((t0, c0));
                        break;
                    }
                }
            }
            if tokio::time::Instant::now() >= give_up {
                break;
            }
            tokio::time::sleep(POLL).await;
        }
        match settled {
            Some((t, cpu)) => {
                ctx.metric("t_settle_ms", ctx.ms_since_spawn(t));
                ctx.metric("cpu_s_to_settle", cpu as f64 / 1e6);
                ctx.metric("settled", 1.0);
            }
            None => {
                // Never quiet: report the give-up time so the metric still compares.
                ctx.metric("t_settle_ms", ctx.ms_since_spawn(mono_now()));
                ctx.metric("settled", 0.0);
            }
        }
        // Late sweeps (previews, repair ticks) show up after the first quiet spell.
        tokio::time::sleep(Duration::from_secs(p.hold_s)).await;
        ctx.close_window().await?;

        // Recovery: residue chats come up aborted, never still streaming.
        let victims: Vec<_> = fx
            .chats
            .iter()
            .filter(|c| c.crash_residue)
            .take(3)
            .cloned()
            .collect();
        let mut aborted = 0;
        for chat in &victims {
            let sub = client
                .subscribe(
                    "WatchDocMessages",
                    json!({"chatId": chat.id, "openingTail": true}),
                )
                .await?;
            let watch = Watch::spawn(chat.id.clone(), sub, ctx.stats.clone());
            watch
                .wait_for(Duration::from_secs(60), "the full reset", |s| {
                    s.full.is_some()
                })
                .await?;
            let (streaming, stamped) = watch.with(|s| {
                let mut streaming = false;
                let mut stamped = false;
                for e in &s.view.entries {
                    if e.get("role").and_then(Value::as_str) != Some("assistant") {
                        continue;
                    }
                    match e.get("status").and_then(Value::as_str) {
                        Some("streaming") => streaming = true,
                        Some("aborted" | "failed" | "interrupted") => stamped = true,
                        _ => {}
                    }
                }
                (streaming, stamped)
            });
            if stamped {
                aborted += 1;
            }
            ctx.check(
                &format!("residue_not_streaming:{}", chat.n),
                !streaming,
                format!("streaming={streaming}"),
            );
        }
        if !victims.is_empty() {
            ctx.check(
                "residue_stamped_aborted",
                aborted == victims.len(),
                format!(
                    "{aborted} of {} sampled residue chats are stamped aborted",
                    victims.len()
                ),
            );
        }
        ctx.metric("residue_chats_aborted", aborted as f64);
        ctx.metric("chats_listed", listed as f64);
        let panic = panic_in_logs(ctx);
        ctx.check(
            "no_panic_in_logs",
            panic.is_none(),
            panic.unwrap_or_default(),
        );
        drop(ui);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_rows_are_counted_through_envelopes() {
        assert_eq!(count_chats(&json!([{"id":1},{"id":2}])), 2);
        assert_eq!(count_chats(&json!({"chats":[{"id":1}]})), 1);
        assert_eq!(count_chats(&json!({"snapshot":{"items":[1,2,3]}})), 3);
        assert_eq!(count_chats(&json!({"other":1})), 0);
    }

    #[test]
    fn variant_key_names_profile_and_tier() {
        let b = Boot::from_params(Tier::Quick, &[]).unwrap();
        assert_eq!(b.variant_key(), "p200-quick");
        assert_eq!(b.p.hold_s, 10);
        assert_eq!(b.profile(), Profile::P200);
    }
}
