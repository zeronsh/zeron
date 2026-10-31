//! S-idle: 20 chats, minutes of idle (bench.md 3.4).
//!
//! Variants: `headless` (no client connected), `ui` (the UI's global streams
//! plus one open chat, as the app idles) and `parked` (`ui` plus chats that
//! finished a replay turn and keep their session alive with resident
//! ballast and children, like real Claude sessions). Nothing talks to the
//! engine inside the window: counters are read on its edges only, and the
//! sampler runs at 1 Hz on `/proc` and cgroup files.

use std::time::Duration;

use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::{Value, json};

use super::driver::{Ui, Watch};
use super::fixture::{self, Profile, base_matches, last_turn_text, turn_settled};
use super::{TrialCtx, Workload, load_params};
use crate::ab::Tier;

const CONFIG: &str = include_str!("../../scenarios/idle.toml");

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct IdleParams {
    pub variant: String,
    pub settle_s: u64,
    pub window_s: u64,
    pub parked_chats: usize,
    pub ballast_mib: u64,
    pub children: String,
}

impl Default for IdleParams {
    fn default() -> Self {
        Self {
            variant: "ui".into(),
            settle_s: 60,
            window_s: 300,
            parked_chats: 5,
            ballast_mib: 300,
            children: "2:250".into(),
        }
    }
}

pub struct Idle {
    pub p: IdleParams,
    tier: Tier,
}

impl Idle {
    pub fn from_params(tier: Tier, overrides: &[(String, String)]) -> Result<Self> {
        let p: IdleParams = load_params(CONFIG, tier, overrides)?;
        ensure!(
            matches!(p.variant.as_str(), "headless" | "ui" | "parked"),
            "variant is headless, ui or parked"
        );
        Ok(Self { p, tier })
    }
}

impl Workload for Idle {
    const ID: &'static str = "S-idle";

    fn variant(&self) -> Value {
        json!({
            "variant": self.p.variant, "settle_s": self.p.settle_s, "window_s": self.p.window_s,
            "parked_chats": self.p.parked_chats, "ballast_mib": self.p.ballast_mib,
            "children": self.p.children, "tier": self.tier.as_str(),
        })
    }

    fn variant_key(&self) -> String {
        format!("{}-{}", self.p.variant, self.tier.as_str())
    }

    fn profile(&self) -> Profile {
        Profile::P20
    }

    fn sampler_hz(&self) -> f64 {
        1.0
    }

    fn engine_env(&self) -> Vec<(String, String)> {
        if self.p.variant != "parked" {
            return Vec::new();
        }
        vec![
            ("ZB_REPLAY_MODE".into(), "ballast".into()),
            (
                "ZB_REPLAY_BALLAST_MIB".into(),
                self.p.ballast_mib.to_string(),
            ),
            ("ZB_REPLAY_CHILDREN".into(), self.p.children.clone()),
        ]
    }

    async fn run(&self, ctx: &mut TrialCtx) -> Result<()> {
        let p = &self.p;
        let fx = ctx.fixture.clone();
        let mut ui: Option<Ui> = None;
        let mut open_chat = None;
        if p.variant != "headless" {
            let client = ctx.connect().await?;
            let mut u = Ui::new(client, ctx.stats.clone());
            u.open_global().await?;
            // The chat a user would have open: a small background chat.
            let chat = fx
                .class("100k", true)
                .first()
                .context("fixture has no chat")?
                .clone();
            u.open_chat(&chat.id, &fx.space_path(&chat)).await?;
            u.watch(&chat.id)?
                .wait_for(Duration::from_secs(60), "the full reset", |s| {
                    s.full.is_some()
                })
                .await?;
            open_chat = Some(chat);
            ui = Some(u);
        }
        if p.variant == "parked" {
            // Sessions that finished a turn stay alive (the idle reaper is 30 min).
            let client = ctx.connect().await?;
            let parked: Vec<_> = fx
                .class("100k", true)
                .into_iter()
                .skip(1)
                .chain(fx.class("1m", true))
                .take(p.parked_chats)
                .collect();
            ensure!(parked.len() == p.parked_chats, "not enough chats to park");
            for chat in &parked {
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
                let dir = ctx.trial.root.join("scripts");
                std::fs::create_dir_all(&dir)?;
                let built = super::content::build_turn(&super::content::TurnOpts {
                    seed: 0x1d1e + chat.n as u64,
                    chat8: chat.chat8.clone(),
                    payload_bytes: 4_000,
                    min_ms: 0,
                    cadence: super::content::Cadence::burst(),
                    marker_every: 0,
                    tools: false,
                    thinking: false,
                });
                let path = dir.join(format!("park{}.zbr.jsonl", chat.n));
                std::fs::write(&path, &built.source)?;
                fixture::queue_play(
                    &client,
                    &chat.id,
                    &fx.space_path(chat),
                    &fixture::message_uuid(chat.n, 7100),
                    &path,
                    0.0,
                )
                .await?;
                let expected = built.text.len();
                watch
                    .wait_for(Duration::from_secs(90), "the parking turn", |s| {
                        turn_settled(&s.view) && last_turn_text(&s.view).len() >= expected
                    })
                    .await?;
            }
        }
        if p.variant == "headless" {
            ctx.disconnect();
        }
        tokio::time::sleep(Duration::from_secs(p.settle_s)).await;

        ctx.open_window().await?;
        let frames_before = ui
            .as_ref()
            .and_then(|u| {
                open_chat
                    .as_ref()
                    .map(|c| u.watch(&c.id).map(|w| w.with(|s| s.frames)))
            })
            .transpose()?;
        tokio::time::sleep(Duration::from_secs(p.window_s)).await;
        ctx.close_window().await?;

        if let (Some(u), Some(chat), Some(before)) = (&ui, &open_chat, frames_before) {
            let w = u.watch(&chat.id)?;
            let (frames, (ok, detail), err) =
                w.with(|s| (s.frames, base_matches(chat, &s.view), s.error.clone()));
            ctx.check(
                "open_chat_unchanged",
                ok && err.is_none(),
                err.unwrap_or(detail),
            );
            ctx.check(
                "no_transcript_frames_while_idle",
                frames == before,
                format!(
                    "{} transcript frames arrived during the window",
                    frames - before
                ),
            );
        }
        let minutes = ctx.window_s() / 60.0;
        ctx.metric("window_min", minutes);
        ctx.metric(
            "parked_sessions",
            if p.variant == "parked" {
                p.parked_chats as f64
            } else {
                0.0
            },
        );
        drop(ui);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quick_tier_is_two_minutes_and_variants_are_validated() {
        let idle = Idle::from_params(Tier::Quick, &[]).unwrap();
        assert_eq!(idle.p.window_s, 120);
        assert_eq!(idle.p.window_s % 60, 0);
        assert_eq!(idle.variant_key(), "ui-quick");
        assert!(Idle::from_params(Tier::Quick, &[("variant".into(), "nope".into())]).is_err());
        let parked =
            Idle::from_params(Tier::Standard, &[("variant".into(), "parked".into())]).unwrap();
        assert!(
            parked
                .engine_env()
                .iter()
                .any(|(k, v)| k == "ZB_REPLAY_BALLAST_MIB" && v == "300")
        );
        assert_eq!(parked.p.window_s, 300);
    }
}
