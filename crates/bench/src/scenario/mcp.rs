//! S-mcp: direct native Claude sessions with a synthetic offline MCP fleet.
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use tokio::process::Command;

use super::{RpcStats, RunEnv, Watch, mcp_process};
use crate::Trial;
use crate::ab::{Check, PlannedTrial, Scenario, Side, TrialBody, TrialMetrics};
use crate::fixture::claude_model::{ClaudeModel, ModelRequest};
use crate::rpc::Client;

struct FleetResult {
    values: BTreeMap<String, f64>,
    sampled: crate::sample::Metrics,
    containment: String,
}

pub struct Mcp {
    env: Arc<RunEnv>,
    variant: Value,
}

impl Mcp {
    pub fn new(env: RunEnv, overrides: &[(String, String)]) -> Result<Self> {
        ensure!(
            overrides.is_empty(),
            "S-mcp currently runs its complete fleet and session matrix"
        );
        Ok(Self {
            env: Arc::new(env),
            variant: json!({"fleets":["none","synthetic"],"sessions":[1,3],"mode":"native-loopback"}),
        })
    }
}

impl Scenario for Mcp {
    fn id(&self) -> &str {
        "S-mcp"
    }
    fn variant(&self) -> &Value {
        &self.variant
    }
    fn variant_key(&self) -> &str {
        "none-synthetic-n1-n3"
    }
    fn fixture_id(&self) -> &str {
        "synthetic-mcp-v1"
    }
    fn run_trial(&mut self, planned: &PlannedTrial) -> Result<TrialBody> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()?;
        let mut last = None;
        for attempt in 0..2 {
            match rt.block_on(run_pair(&self.env, planned, attempt)) {
                Ok(body) => return Ok(body),
                Err(err) => {
                    eprintln!(
                        "zeron-bench: S-mcp trial failed (attempt {}): {err:#}",
                        attempt + 1
                    );
                    last = Some(format!("{err:#}"));
                }
            }
        }
        Ok(super::invalid_body(
            last.unwrap_or_else(|| "S-mcp failed".into()),
        ))
    }
}

async fn run_pair(env: &RunEnv, planned: &PlannedTrial, attempt: u32) -> Result<TrialBody> {
    let started = crate::mono_now();
    let mut metrics = TrialMetrics::default();
    let mut checks = Vec::new();
    let mut noisy = false;
    let mut containment: Option<String> = None;
    for (fleet, slot) in [("none", 0), ("synthetic", 1)] {
        let index = 20_000 + planned.order_index * 10 + attempt * 100_000 + slot;
        eprintln!(
            "zeron-bench: S-mcp side {} trial {} fleet {fleet}",
            planned.side.as_str(),
            planned.order_index
        );
        let result = run_fleet(env, planned.side, index, fleet, 10, 60).await?;
        eprintln!(
            "zeron-bench: S-mcp side {} fleet {fleet}: MCP marginal {:.2} MiB, session-tree marginal {:.2} MiB, readiness upper bound {:.2} ms",
            planned.side.as_str(),
            result.values["mcp_pss_marginal_mib"],
            result.values["session_tree_pss_marginal_mib"],
            result.values["mcp_ready_upper_ms"]
        );
        noisy |= result.values["outside_cpu_pct"] > env.noisy_outside_cpu_pct;
        if let Some(existing) = &containment {
            if existing != &result.containment {
                containment = Some("mixed".into());
            }
        } else {
            containment = Some(result.containment.clone());
        }
        if fleet == "synthetic" {
            metrics.sampled = result.sampled;
        }
        for (name, value) in result.values {
            metrics.scenario.insert(format!("{fleet}_{name}"), value);
        }
        checks.push(Check {
            name: format!("{fleet}_readiness_and_interception"),
            ok: true,
            detail: Some(if fleet == "synthetic" {
                format!("each of three sessions had {} real synthetic servers before a call, with two MCP roots", if planned.side == Side::A { 2 } else { 0 })
            } else { "empty synthetic fleet; three sessions sampled".into() }),
        });
    }
    let cache = crate::fixture::fixture_dir(&env.zb_root, &["cache"])?;
    let calibration = json!({
        "schema":"zeron-bench-mcp-calibration-v1", "side":planned.side.as_str(),
        "fleet":"synthetic", "servers":["synthetic-entry", "plugin:synthetic:synthetic"],
        "ballast_mib_per_server":175, "claude_ready_upper_ms":metrics.scenario["synthetic_mcp_ready_upper_ms"],
        "claude_tree_marginal_mib":metrics.scenario["synthetic_session_tree_pss_marginal_mib"],
        "mcp_tree_marginal_mib":metrics.scenario["synthetic_mcp_pss_marginal_mib"],
        "tool_cold_ms":metrics.scenario["synthetic_tool_cold_ms"], "zero_usage_fixture":true, "noisy":noisy
    });
    let tmp = cache.join(format!(
        "calibration-{}-{}.json.tmp",
        std::process::id(),
        planned.order_index
    ));
    fs::write(&tmp, serde_json::to_vec_pretty(&calibration)?)?;
    fs::rename(tmp, cache.join("calibration.json"))?;
    let mut body = TrialBody::ok(metrics);
    body.checks = checks;
    body.containment = containment.unwrap_or_else(|| "unknown".into());
    body.noisy = noisy;
    body.window_s = (crate::mono_now() - started) as f64 / 1e9;
    Ok(body)
}

pub fn cli_executable() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("ZB_REAL_CLAUDE") {
        return Ok(fs::canonicalize(path).context("resolve ZB_REAL_CLAUDE")?);
    }
    let home = crate::env::passwd_home().context("resolve installed Claude")?;
    let path = home.join(".local/share/mise/installs/claude/latest/claude");
    fs::canonicalize(&path).with_context(|| {
        format!(
            "{} not found; set ZB_REAL_CLAUDE to the native Claude binary",
            path.display()
        )
    })
}

fn copy_fixture(home: &Path, trial: &Trial) -> Result<()> {
    for name in [".claude.json", "settings.json"] {
        fs::copy(home.join(name), trial.home.join(".claude").join(name))?;
    }
    let status = std::process::Command::new("/usr/bin/cp")
        .env_clear()
        .args(["-a", "--"])
        .arg(home.join("plugins"))
        .arg(trial.home.join(".claude/plugins"))
        .status()?;
    ensure!(status.success(), "copy synthetic plugin metadata failed");
    Ok(())
}

async fn run_fleet(
    env: &RunEnv,
    side: Side,
    index: u32,
    fleet: &str,
    settle_s: u64,
    window_s: u64,
) -> Result<FleetResult> {
    let mut trial = Trial::at(&env.zb_root, &env.run_id, index, side.as_str())?;
    trial.bench_bin = env.bench_bin.clone();
    trial.replay_bin = cli_executable()?;
    trial.probe_sock = env.short_sock("mcp", index)?;
    trial.allocate_port()?;
    trial.prepare()?;
    let model = ClaudeModel::start().await?;
    trial.claude_model_port = Some(model.addr.port());
    trial
        .extra
        .push(("ZERON_HARNESS".into(), "claude-code".into()));
    let fixture_home = if fleet == "synthetic" {
        let home = crate::fixture::claude_home::setup(&trial.root)?;
        copy_fixture(&home, &trial)?;
        Some(home)
    } else {
        fs::write(
            trial.home.join(".claude/.claude.json"),
            b"{\"mcpServers\":{}}",
        )?;
        fs::write(
            trial.home.join(".claude/settings.json"),
            b"{\"disableAllHooks\":true}",
        )?;
        None
    };
    let project = trial.root.join("project");
    fs::create_dir(&project)?;
    let status = std::process::Command::new("/usr/bin/git")
        .env_clear()
        .env("HOME", &trial.home)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args(["init", "-q"])
        .arg(&project)
        .status()?;
    ensure!(status.success(), "initialize isolated benchmark project");
    let engine = crate::launch(env.engine(side), &trial).await?;
    let pid = engine.pid;
    let cgroup = engine
        .cgroup
        .as_ref()
        .and_then(|path| crate::sample::Cgroup::open(path).ok());
    let sampler =
        crate::sample::Sampler::start(pid, cgroup, 1.0, &crate::sample::Grouping::default())?;
    let window_start = crate::mono_now();
    let result = drive_fleet(
        &trial,
        &model,
        &project,
        fixture_home.as_deref(),
        side,
        pid,
        env.engine(side),
        settle_s,
        window_s,
    )
    .await;
    let window_end = crate::mono_now();
    let set = tokio::task::spawn_blocking(move || sampler.finish()).await??;
    let sampled = crate::sample::metrics(
        &set,
        crate::sample::Window {
            start_ns: window_start,
            end_ns: window_end,
        },
    );
    let samples: String = set
        .ticks
        .iter()
        .map(|tick| tick.ndjson_line() + "\n")
        .collect();
    fs::write(trial.root.join("samples.ndjson"), samples)?;
    let requests: Vec<_> = model
        .events()
        .into_iter()
        .map(|event| {
            json!({
                "method":event.method, "path":event.path, "t_ns":event.t_ns
            })
        })
        .collect();
    fs::write(
        trial.root.join("model-requests.json"),
        serde_json::to_vec_pretty(&requests)?,
    )?;
    let descendants = mcp_process::descendant_pids(pid)?;
    let stopped = engine.stop().await.context("stop S-mcp engine")?;
    wait_for("MCP descendants to exit", || {
        Ok(descendants
            .iter()
            .all(|pid| !Path::new(&format!("/proc/{pid}")).exists())
            .then_some(()))
    })
    .await?;
    ensure!(
        !matches!(stopped.how, crate::StopHow::Sigkill),
        "S-mcp engine required SIGKILL"
    );
    let mut result = result?;
    ensure!(
        sampled.outside_cpu_pct <= env.invalid_outside_cpu_pct,
        "outside CPU {:.1}% exceeded {:.1}%",
        sampled.outside_cpu_pct,
        env.invalid_outside_cpu_pct
    );
    result.insert("outside_cpu_pct".into(), sampled.outside_cpu_pct);
    result.insert("tree_cpu_s".into(), sampled.cpu_s.tree);
    Ok(FleetResult {
        values: result,
        sampled,
        containment: stopped.containment.to_string(),
    })
}

async fn wait_for<F, T>(what: &str, mut f: F) -> Result<T>
where
    F: FnMut() -> Result<Option<T>>,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
    loop {
        if let Some(value) = f()? {
            return Ok(value);
        }
        if tokio::time::Instant::now() >= deadline {
            bail!("timed out waiting for {what}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn model_messages(model: &ClaudeModel) -> Vec<ModelRequest> {
    model
        .events()
        .into_iter()
        .filter(|e| e.path.split('?').next() == Some("/v1/messages"))
        .collect()
}

async fn queue_session(
    client: &Client,
    trial: &Trial,
    model: &ClaudeModel,
    project: &Path,
    engine_pid: u32,
    chat: &str,
    seq: usize,
    watches: &mut Vec<Watch>,
) -> Result<(u32, u64, u64)> {
    let before: BTreeSet<_> = mcp_process::claude_descendants(engine_pid, &trial.replay_bin)?
        .into_iter()
        .collect();
    let old = model_messages(model).len();
    let stream = client
        .subscribe(
            "WatchDocMessages",
            json!({"chatId":chat,"openingTail":true}),
        )
        .await?;
    let watch = Watch::spawn(chat.to_owned(), stream, Arc::new(RpcStats::default()));
    watch
        .wait_for(Duration::from_secs(30), "chat reset", |s| s.full.is_some())
        .await?;
    client
        .call(
            "QueueCommand",
            json!({"chatId":chat,"command":{"kind":"run",
        "messageId":format!("b8000003-0000-4000-8000-{seq:012x}"), "request":{
            "prompt":"offline benchmark", "model":"haiku", "reasoning":null,
            "modelOptions":{}, "cwd":project, "sandbox":"workspace-write",
            "autoApprove":false, "resume":null}}}),
        )
        .await?;
    let event = wait_for("Claude model request", || {
        Ok(model_messages(model).get(old).cloned())
    })
    .await?;
    let pid = wait_for("new Claude session", || {
        let new: Vec<_> = mcp_process::claude_descendants(engine_pid, &trial.replay_bin)?
            .into_iter()
            .filter(|p| !before.contains(p))
            .collect();
        ensure!(new.len() <= 1, "ambiguous extra Claude processes: {new:?}");
        Ok(new.first().copied())
    })
    .await?;
    let start = mcp_process::started_mono_ns(pid)?;
    ensure!(event.t_ns >= start, "model request predates Claude process");
    watches.push(watch);
    Ok((pid, start, event.t_ns))
}

async fn drive_fleet(
    trial: &Trial,
    model: &ClaudeModel,
    project: &Path,
    home: Option<&Path>,
    side: Side,
    engine_pid: u32,
    engine_path: &Path,
    settle_s: u64,
    window_s: u64,
) -> Result<BTreeMap<String, f64>> {
    let client = Client::connect(trial.port, &trial.data).await?;
    let (device, _) = client.call("LocalDevice", json!({})).await?;
    let space = "b8000001-0000-4000-8000-000000000001";
    client.call("Mutate", json!({"op":"createSpace", "spaceId":space, "deviceId":device["deviceId"], "path":project})).await?;
    let chats: Vec<_> = (0..4)
        .map(|n| format!("b8000002-0000-4000-8000-{n:012x}"))
        .collect();
    for chat in &chats {
        client.call("Mutate", json!({"op":"createChat", "chatId":chat, "spaceId":space, "config":{"harness":"claude-code","model":"haiku","reasoning":null,"sandbox":"workspace-write"}})).await?;
        client
            .call(
                "Mutate",
                json!({"op":"renameChat", "chatId":chat, "title":"MCP benchmark"}),
            )
            .await?;
    }
    if home.is_some() && side == Side::B {
        client
            .call(
                "ListCommands",
                json!({"harness":"claude-code", "spaceId":space}),
            )
            .await?;
        wait_for("MCP capture", || {
            let path = trial.data.join("claude-mcp");
            let captured = fs::read_dir(path).ok().is_some_and(|entries| {
                entries.flatten().any(|entry| {
                    let Ok(bytes) = fs::read(entry.path()) else {
                        return false;
                    };
                    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
                        return false;
                    };
                    let Some(rows) = value["servers"].as_array() else {
                        return false;
                    };
                    let names: BTreeSet<_> = rows
                        .iter()
                        .filter(|row| {
                            row["status"] == "connected" && row["config"]["command"] == "node"
                        })
                        .filter_map(|row| row["name"].as_str())
                        .collect();
                    names.contains("synthetic-entry")
                        && names.contains("plugin:synthetic:synthetic")
                })
            });
            Ok(captured.then_some(()))
        })
        .await?;
    }
    // Warm cache in a real session, then stop its in-flight turn so it is not
    // included in the n=1 and n=3 populations.
    model.hold_responses(true);
    let mut watches = Vec::new();
    let warm_log = proxy_log_lines(trial);
    let (warm_pid, _, _) = queue_session(
        &client,
        trial,
        model,
        project,
        engine_pid,
        &chats[0],
        0,
        &mut watches,
    )
    .await?;
    let proxy = (side == Side::B).then(|| engine_path.parent().unwrap().join("zeron-mcp-proxy"));
    if let Some(home) = home {
        wait_ready(
            trial,
            home,
            side,
            warm_pid,
            proxy.as_deref(),
            warm_log,
            true,
        )
        .await?;
    }
    client
        .call(
            "QueueCommand",
            json!({"chatId":chats[0],"command":{"kind":"interrupt"}}),
        )
        .await?;
    wait_for("warm session to terminate", || {
        Ok(
            (!mcp_process::claude_descendants(engine_pid, &trial.replay_bin)?.contains(&warm_pid))
                .then_some(()),
        )
    })
    .await?;
    model.hold_responses(false);
    wait_for("all warmup Claude processes to exit", || {
        Ok(
            mcp_process::claude_descendants(engine_pid, &trial.replay_bin)?
                .is_empty()
                .then_some(()),
        )
    })
    .await?;
    let warm_marker_count = home.map(server_event_lines).unwrap_or(0);
    let mut pids = Vec::new();
    let mut times = Vec::new();
    let mut pss1 = 0.0;
    let mut pss3 = 0.0;
    let mut tree1 = 0.0;
    let mut tree3 = 0.0;
    for n in 1..=3 {
        if n > 1 {
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        let log_start = proxy_log_lines(trial);
        let (pid, start, request_time) = queue_session(
            &client,
            trial,
            model,
            project,
            engine_pid,
            &chats[n],
            n,
            &mut watches,
        )
        .await?;
        let observed = if let Some(home) = home {
            wait_ready(trial, home, side, pid, proxy.as_deref(), log_start, false).await?
        } else {
            request_time
        };
        ensure!(
            observed >= start,
            "readiness observation predates process start"
        );
        if side == Side::B
            && let Some(home) = home
        {
            ensure!(
                server_event_lines(home) == warm_marker_count,
                "lazy measured session launched a real server before its first call"
            );
        }
        watches
            .last()
            .context("missing session watch")?
            .wait_for(
                Duration::from_secs(30),
                "successful zero-usage fixture turn",
                |state| {
                    super::fixture::turn_settled(&state.view)
                        && super::fixture::last_turn_text(&state.view).contains("fixture complete")
                },
            )
            .await?;
        pids.push(pid);
        wait_for("exact session count", || {
            let actual: BTreeSet<_> =
                mcp_process::claude_descendants(engine_pid, &trial.replay_bin)?
                    .into_iter()
                    .collect();
            let expected: BTreeSet<_> = pids.iter().copied().collect();
            ensure!(
                expected.is_subset(&actual),
                "a measured Claude session exited"
            );
            Ok((actual == expected).then_some(()))
        })
        .await?;
        times.push((observed - start) as f64 / 1e6);
        tokio::time::sleep(Duration::from_millis(250)).await;
        if n == 1 || n == 3 {
            tokio::time::sleep(Duration::from_secs(settle_s)).await;
            let mut mcp_samples = Vec::new();
            let mut tree_samples = Vec::new();
            for sample_index in 0..7 {
                let mut mcp = 0u64;
                let mut tree = 0u64;
                for pid in &pids {
                    tree += mcp_process::tree_pss_kib(*pid)?;
                    if let Some(home) = home {
                        let sample = mcp_process::measure(
                            *pid,
                            &home.join("synthetic-mcp.js"),
                            Path::new("/usr/bin/node"),
                            proxy.as_deref(),
                        )?;
                        let expected = if side == Side::A { 2 } else { 0 };
                        ensure!(
                            sample.servers == expected && sample.roots.len() == 2,
                            "session failed eager/lazy interception: {sample:?}"
                        );
                        mcp += sample.pss_kib;
                    }
                }
                mcp_samples.push(mcp);
                tree_samples.push(tree);
                if sample_index < 6 {
                    tokio::time::sleep(Duration::from_secs_f64(window_s as f64 / 6.0)).await;
                }
            }
            if side == Side::B
                && let Some(home) = home
            {
                ensure!(
                    server_event_lines(home) == warm_marker_count,
                    "lazy measured session launched a real server during the idle window"
                );
            }
            mcp_samples.sort_unstable();
            tree_samples.sort_unstable();
            let mcp = mcp_samples[3] as f64 / 1024.0;
            let tree = tree_samples[3] as f64 / 1024.0;
            if n == 1 {
                pss1 = mcp;
                tree1 = tree;
            } else {
                pss3 = mcp;
                tree3 = tree;
            }
        }
    }
    let mut out = BTreeMap::from([
        ("mcp_pss_n1_mib".into(), pss1),
        ("mcp_pss_n3_mib".into(), pss3),
        ("mcp_pss_marginal_mib".into(), (pss3 - pss1) / 2.0),
        ("session_tree_pss_n1_mib".into(), tree1),
        ("session_tree_pss_n3_mib".into(), tree3),
        (
            "session_tree_pss_marginal_mib".into(),
            (tree3 - tree1) / 2.0,
        ),
        (
            "mcp_ready_upper_ms".into(),
            times.iter().sum::<f64>() / times.len() as f64,
        ),
    ]);
    if let Some(home) = home {
        out.insert(
            "real_servers_before_call_per_session".into(),
            if side == Side::A { 2.0 } else { 0.0 },
        );
        out.insert("mcp_roots_per_session".into(), 2.0);
        let (cold, warm) = call_tool(trial, home, side, proxy.as_deref()).await?;
        out.insert("tool_cold_ms".into(), cold);
        out.insert("tool_warm_ms".into(), warm);
    }
    ensure!(
        model_messages(model).len() >= 4,
        "not all sessions reached the loopback model fixture"
    );
    ensure!(
        model.events().iter().all(|event| event.method == "POST"),
        "unexpected model HTTP method"
    );
    ensure!(
        model.requests().iter().all(|p| matches!(
            p.split('?').next(),
            Some("/v1/messages" | "/v1/messages/count_tokens")
        )),
        "unexpected model endpoint"
    );
    Ok(out)
}

fn server_event_lines(home: &Path) -> usize {
    fs::read_to_string(home.join("server-events.jsonl"))
        .map(|text| text.lines().count())
        .unwrap_or(0)
}

fn proxy_log_lines(trial: &Trial) -> usize {
    fs::read_to_string(trial.data.join("logs/mcp-proxy.jsonl"))
        .map(|text| text.lines().count())
        .unwrap_or(0)
}

/// Polling timestamps bound the observed readiness of both MCP roots. Direct
/// servers must have handled tools/list; lazy servers must show two fresh
/// catalog hits and no live backend.
async fn wait_ready(
    trial: &Trial,
    home: &Path,
    side: Side,
    pid: u32,
    proxy: Option<&Path>,
    from: usize,
    warm: bool,
) -> Result<u64> {
    wait_for("synthetic MCP readiness", || {
        let sample = mcp_process::measure(
            pid,
            &home.join("synthetic-mcp.js"),
            Path::new("/usr/bin/node"),
            proxy,
        )?;
        if sample.roots.len() != 2 {
            return Ok(None);
        }
        if side == Side::A {
            if sample.servers != 2 {
                return Ok(None);
            }
            let file = fs::read_to_string(home.join("server-events.jsonl")).unwrap_or_default();
            let ready: BTreeSet<_> = file
                .lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .filter(|event| event["event"] == "tools/list")
                .filter_map(|event| event["pid"].as_u64().map(|v| v as u32))
                .collect();
            if !sample.server_pids.iter().all(|pid| ready.contains(pid)) {
                return Ok(None);
            }
        } else {
            if sample.servers != 0 {
                return Ok(None);
            }
            let file =
                fs::read_to_string(trial.data.join("logs/mcp-proxy.jsonl")).unwrap_or_default();
            let names: BTreeSet<_> = file
                .lines()
                .skip(from)
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .filter(|event| {
                    event["event"] == "cache"
                        && (event["status"] == "hit" || (warm && event["status"] == "refresh"))
                })
                .filter_map(|event| event["server"].as_str().map(str::to_owned))
                .collect();
            if !(names.contains("synthetic-entry") && names.contains("plugin:synthetic:synthetic"))
            {
                return Ok(None);
            }
        }
        Ok(Some(crate::mono_now()))
    })
    .await
}

async fn call_tool(
    trial: &Trial,
    home: &Path,
    side: Side,
    proxy: Option<&Path>,
) -> Result<(f64, f64)> {
    let script = home.join("synthetic-mcp.js");
    let data = trial.root.join("mcp-call-data");
    fs::create_dir(&data)?;
    let mut cmd = if side == Side::B {
        let mut cmd = Command::new(proxy.context("missing candidate proxy")?);
        cmd.args([
            "--server",
            "synthetic-entry",
            "--mode",
            "lazy",
            "--data-dir",
        ])
        .arg(&data)
        .arg("--run-dir")
        .arg(data.join("run"))
        .args(["--", "node"])
        .arg(&script)
        .arg("entry");
        cmd
    } else {
        let mut cmd = Command::new("/usr/bin/node");
        cmd.arg(&script).arg("entry");
        cmd
    };
    cmd.env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", &trial.home)
        .env("ZERON_DATA_DIR", &data)
        .env("CLAUDE_CONFIG_DIR", trial.home.join(".claude"))
        .env("CODEX_HOME", trial.home.join("codex"))
        .stdin(Stdio::piped());
    let mut client = crate::mcp_client::Client::start(&mut cmd, Duration::from_secs(30))?;
    client.initialize().await?;
    client.request("tools/list", json!({})).await?;
    let (_, cold) = client.timed_call("echo", json!({})).await?;
    let (_, warm) = client.timed_call("echo", json!({})).await?;
    client.stop().await?;
    Ok((cold, warm))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ab::Tier;

    #[tokio::test]
    #[ignore = "requires real Claude, an upstream engine, and the isolated native namespace"]
    async fn upstream_fleets_start_and_expose_their_marginal_pss() {
        let scratch = PathBuf::from(std::env::var_os("TMPDIR").expect("scratch TMPDIR"));
        let root = scratch.parent().unwrap().to_path_buf();
        let engine =
            PathBuf::from(std::env::var_os("ZB_TEST_ENGINE").expect("explicit upstream engine"));
        let bench =
            PathBuf::from(std::env::var_os("ZB_TEST_BENCH").expect("explicit bench executable"));
        let env = RunEnv::new(
            root,
            format!("mcp-native-{}-{}", std::process::id(), crate::mono_now()),
            Tier::Quick,
            1,
            engine.clone(),
            engine,
            bench,
            "fixture".into(),
        )
        .unwrap();
        let none = run_fleet(&env, Side::A, 1, "none", 0, 1).await.unwrap();
        assert_eq!(none.values["mcp_pss_marginal_mib"], 0.0);
        let synthetic = run_fleet(&env, Side::A, 2, "synthetic", 0, 1)
            .await
            .unwrap();
        assert!(
            synthetic.values["mcp_pss_marginal_mib"] > 300.0,
            "{:?}",
            synthetic.values
        );
        assert!(synthetic.values["tool_cold_ms"] >= 0.0);
    }

    #[tokio::test]
    #[ignore = "requires the built candidate engine and proxy in the isolated native namespace"]
    async fn candidate_proves_lazy_interception() {
        let scratch = PathBuf::from(std::env::var_os("TMPDIR").expect("scratch TMPDIR"));
        let root = scratch.parent().unwrap().to_path_buf();
        let engine =
            PathBuf::from(std::env::var_os("ZB_TEST_ENGINE").expect("explicit candidate engine"));
        let bench =
            PathBuf::from(std::env::var_os("ZB_TEST_BENCH").expect("explicit bench executable"));
        let env = RunEnv::new(
            root,
            format!("mcp-candidate-{}-{}", std::process::id(), crate::mono_now()),
            Tier::Quick,
            1,
            engine.clone(),
            engine,
            bench,
            "fixture".into(),
        )
        .unwrap();
        let synthetic = run_fleet(&env, Side::B, 1, "synthetic", 0, 1)
            .await
            .unwrap();
        assert!(
            synthetic.values["mcp_pss_marginal_mib"] < 30.0,
            "{:?}",
            synthetic.values
        );
        assert!(synthetic.values["tool_cold_ms"] >= 0.0);
    }
}
