//! Bench-side stand-ins for what the UI does: watched chats that apply
//! transcript deltas, drained side streams, the probe-socket collector and
//! the RPC round-trip canary.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::Value;
use tokio::task::JoinHandle;

use crate::probe::{ProbeEvent, ProbeSock};
use crate::rpc::{Checkout, Client, Subscription, UiSet};
use crate::transcript::{ChatView, MarkerScan};

/// Raw bytes and items per RPC method, shared by every drain task.
#[derive(Default)]
pub struct RpcStats {
    inner: Mutex<BTreeMap<String, (u64, u64)>>,
}

impl RpcStats {
    pub fn add(&self, method: &str, bytes: usize) {
        let mut map = self.inner.lock().unwrap();
        let slot = map.entry(method.to_string()).or_default();
        slot.0 += bytes as u64;
        slot.1 += 1;
    }

    pub fn snapshot(&self) -> BTreeMap<String, (u64, u64)> {
        self.inner.lock().unwrap().clone()
    }
}

/// Window delta of two snapshots, as `(bytes_in, items)` maps.
pub fn rpc_delta(
    start: &BTreeMap<String, (u64, u64)>,
    end: &BTreeMap<String, (u64, u64)>,
) -> (BTreeMap<String, u64>, BTreeMap<String, u64>) {
    let mut bytes = BTreeMap::new();
    let mut items = BTreeMap::new();
    for (method, (b1, i1)) in end {
        let (b0, i0) = start.get(method).copied().unwrap_or((0, 0));
        if *i1 > i0 {
            bytes.insert(method.clone(), b1.saturating_sub(b0));
            items.insert(method.clone(), i1 - i0);
        }
    }
    (bytes, items)
}

#[derive(Default)]
pub struct ChatState {
    pub view: ChatView,
    scan: MarkerScan,
    /// `(chat8, seq, t_recv_ns)`, first sight only.
    pub markers: Vec<(String, u64, u64)>,
    pub frames: u64,
    pub resets: u64,
    pub bytes: u64,
    /// `(t_recv_ns, bytes)` of the first frame.
    pub first: Option<(u64, usize)>,
    /// `(t_recv_ns, bytes)` of the first authoritative reset (not the tail preview).
    pub full: Option<(u64, usize)>,
    pub error: Option<String>,
    pub closed: bool,
}

/// A watched chat: a task applies every frame and scans for markers.
pub struct Watch {
    pub chat: String,
    state: Arc<Mutex<ChatState>>,
    task: JoinHandle<()>,
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn scan_value(scan: &mut MarkerScan, value: &Value, t: u64, out: &mut Vec<(String, u64, u64)>) {
    match value {
        Value::String(text) if text.contains("[[zb:") => out.extend(scan.scan(text.as_bytes(), t)),
        Value::Array(items) => items.iter().for_each(|v| scan_value(scan, v, t, out)),
        Value::Object(map) => map.values().for_each(|v| scan_value(scan, v, t, out)),
        _ => {}
    }
}

impl Watch {
    pub fn spawn(chat: String, mut sub: Subscription, stats: Arc<RpcStats>) -> Self {
        let state = Arc::new(Mutex::new(ChatState::default()));
        let shared = Arc::clone(&state);
        let task = tokio::spawn(async move {
            while let Some(item) = sub.recv().await {
                let item = match item {
                    Ok(item) => item,
                    Err(err) => {
                        shared.lock().unwrap().error = Some(format!("{err:#}"));
                        break;
                    }
                };
                stats.add("WatchDocMessages", item.bytes);
                let mut st = shared.lock().unwrap();
                let st = &mut *st;
                st.frames += 1;
                st.bytes += item.bytes as u64;
                if st.first.is_none() {
                    st.first = Some((item.t_recv_ns, item.bytes));
                }
                let is_reset = item.value.get("reset").is_some();
                let pending =
                    item.value.get("historyPending").and_then(Value::as_bool) == Some(true);
                if is_reset {
                    st.resets += 1;
                    if !pending && st.full.is_none() {
                        st.full = Some((item.t_recv_ns, item.bytes));
                    }
                }
                let mut found = Vec::new();
                scan_value(&mut st.scan, &item.value, item.t_recv_ns, &mut found);
                st.markers.extend(found);
                if let Err(err) = st.view.apply(&item.value) {
                    st.error = Some(format!("apply: {err:#}"));
                    break;
                }
            }
            shared.lock().unwrap().closed = true;
        });
        Self { chat, state, task }
    }

    pub fn with<R>(&self, f: impl FnOnce(&ChatState) -> R) -> R {
        f(&self.state.lock().unwrap())
    }

    /// Poll until `cond` holds. A watch error or a closed stream fails the wait.
    pub async fn wait_for(
        &self,
        timeout: Duration,
        what: &str,
        cond: impl Fn(&ChatState) -> bool,
    ) -> Result<()> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            {
                let st = self.state.lock().unwrap();
                if let Some(err) = &st.error {
                    bail!("chat {}: {err}", self.chat);
                }
                if cond(&st) {
                    return Ok(());
                }
                if st.closed {
                    bail!("chat {}: stream closed while waiting for {what}", self.chat);
                }
            }
            if tokio::time::Instant::now() >= deadline {
                bail!(
                    "chat {}: timed out after {timeout:?} waiting for {what}",
                    self.chat
                );
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    }
}

/// Count and discard a stream's items.
pub fn drain(method: &'static str, mut sub: Subscription, stats: Arc<RpcStats>) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(item) = sub.recv().await {
            match item {
                Ok(item) => stats.add(method, item.bytes),
                Err(_) => break,
            }
        }
    })
}

/// The UI's subscription set (bench.md 3.5) with every stream drained.
pub struct Ui {
    pub client: Client,
    pub stats: Arc<RpcStats>,
    set: UiSet,
    tasks: Vec<JoinHandle<()>>,
    pub watches: BTreeMap<String, Watch>,
    chat_tasks: BTreeMap<String, Vec<JoinHandle<()>>>,
}

/// Monotonic stamps around one chat open.
#[derive(Clone, Copy, Debug)]
pub struct OpenTimes {
    pub t_start: u64,
    pub t_done: u64,
}

impl Ui {
    pub fn new(client: Client, stats: Arc<RpcStats>) -> Self {
        let set = UiSet::new(
            client.clone(),
            Checkout {
                cwd: String::new(),
                branch: None,
                target_device_id: None,
            },
        );
        Self {
            client,
            stats,
            set,
            tasks: Vec::new(),
            watches: BTreeMap::new(),
            chat_tasks: BTreeMap::new(),
        }
    }

    pub async fn open_global(&mut self) -> Result<()> {
        self.set.open_global().await?;
        for (method, sub) in std::mem::take(&mut self.set.global) {
            self.tasks.push(drain(method, sub, Arc::clone(&self.stats)));
        }
        Ok(())
    }

    /// `FocusChat` plus the four per-chat streams (and previews), as the UI does.
    pub async fn open_chat(&mut self, id: &str, cwd: &Path) -> Result<OpenTimes> {
        self.set.checkout.cwd = cwd.to_string_lossy().into_owned();
        let t_start = crate::mono_now();
        self.set.open_chat(id).await?;
        let t_done = crate::mono_now();
        let mut subs = self.set.chats.remove(id).context("chat streams missing")?;
        let doc = subs
            .remove("WatchDocMessages")
            .context("WatchDocMessages missing")?;
        self.watches.insert(
            id.to_string(),
            Watch::spawn(id.to_string(), doc, Arc::clone(&self.stats)),
        );
        let tasks = subs
            .into_iter()
            .map(|(method, sub)| drain(method, sub, Arc::clone(&self.stats)))
            .collect();
        self.chat_tasks.insert(id.to_string(), tasks);
        Ok(OpenTimes { t_start, t_done })
    }

    pub fn close_chat(&mut self, id: &str) {
        self.watches.remove(id);
        for task in self.chat_tasks.remove(id).unwrap_or_default() {
            task.abort();
        }
    }

    pub fn watch(&self, id: &str) -> Result<&Watch> {
        self.watches
            .get(id)
            .with_context(|| format!("chat {id} is not open"))
    }
}

impl Drop for Ui {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
        for tasks in self.chat_tasks.values() {
            for task in tasks {
                task.abort();
            }
        }
    }
}

/// Collects probe-socket datagrams on a thread so the queue never fills.
pub struct Probe {
    events: Arc<Mutex<Vec<(u64, ProbeEvent)>>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Probe {
    pub fn start(path: &Path) -> Result<Self> {
        let sock = ProbeSock::bind(path).with_context(|| format!("bind {}", path.display()))?;
        sock.set_read_timeout(Some(Duration::from_millis(200)))?;
        let events = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (ev, st) = (Arc::clone(&events), Arc::clone(&stop));
        let thread = std::thread::Builder::new()
            .name("zb-probe".into())
            .spawn(move || {
                while !st.load(Ordering::Relaxed) {
                    match sock.recv() {
                        Ok(event) => ev.lock().unwrap().push((crate::mono_now(), event)),
                        Err(_) => continue,
                    }
                }
                // Drain what is already queued.
                let _ = sock.set_nonblocking(true);
                while let Ok(event) = sock.recv() {
                    ev.lock().unwrap().push((crate::mono_now(), event));
                }
            })
            .context("spawn probe thread")?;
        Ok(Self {
            events,
            stop,
            thread: Some(thread),
        })
    }

    pub fn finish(&mut self) -> Vec<(u64, ProbeEvent)> {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        std::mem::take(&mut *self.events.lock().unwrap())
    }

    /// `(chat8, seq)` to the replay's write time.
    pub fn marks(&self) -> HashMap<(String, u64), u64> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(_, e)| match e {
                ProbeEvent::Mark { chat, seq, t } => Some(((chat.clone(), *seq), *t)),
                _ => None,
            })
            .collect()
    }

    /// Shim spawn events with `lo <= t <= hi`, counted by program name.
    pub fn spawns_between(&self, lo: u64, hi: u64) -> BTreeMap<String, u64> {
        let mut out = BTreeMap::new();
        for (_, event) in self.events.lock().unwrap().iter() {
            if let ProbeEvent::Spawn { prog, t, .. } = event
                && *t >= lo
                && *t <= hi
            {
                *out.entry(prog.clone()).or_default() += 1;
            }
        }
        out
    }

    pub fn event_count(&self) -> usize {
        self.events.lock().unwrap().len()
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// `LocalDevice` round trips every `period`, as `(t_start_ns, rtt_ns)`.
pub struct Canary {
    samples: Arc<Mutex<Vec<(u64, u64)>>>,
    task: JoinHandle<()>,
}

impl Canary {
    pub fn start(client: Client, period: Duration) -> Self {
        let samples = Arc::new(Mutex::new(Vec::new()));
        let out = Arc::clone(&samples);
        let task = tokio::spawn(async move {
            let mut tick = tokio::time::interval(period);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                let t0 = crate::mono_now();
                match client.call("LocalDevice", serde_json::json!({})).await {
                    Ok((_, rtt)) => out.lock().unwrap().push((t0, rtt)),
                    Err(_) => break,
                }
            }
        });
        Self { samples, task }
    }

    pub fn between(&self, lo: u64, hi: u64) -> Vec<u64> {
        self.samples
            .lock()
            .unwrap()
            .iter()
            .filter(|(t, _)| *t >= lo && *t <= hi)
            .map(|(_, rtt)| *rtt)
            .collect()
    }
}

impl Drop for Canary {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Nearest-rank percentile of `values` (sorted copy).
pub fn percentile(values: &[f64], p: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let rank = ((p / 100.0) * v.len() as f64).ceil().max(1.0) as usize;
    v[rank.min(v.len()) - 1]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_is_nearest_rank() {
        let v = [5.0, 1.0, 3.0, 2.0, 4.0];
        assert_eq!(percentile(&v, 50.0), 3.0);
        assert_eq!(percentile(&v, 99.0), 5.0);
        assert_eq!(percentile(&[], 50.0), 0.0);
    }

    #[test]
    fn rpc_delta_reports_only_methods_that_moved() {
        let mut a = BTreeMap::new();
        a.insert("A".to_string(), (10, 1));
        a.insert("B".to_string(), (5, 1));
        let mut b = a.clone();
        b.insert("A".to_string(), (30, 3));
        let (bytes, items) = rpc_delta(&a, &b);
        assert_eq!(bytes["A"], 20);
        assert_eq!(items["A"], 2);
        assert!(!bytes.contains_key("B"));
    }

    #[test]
    fn marker_values_are_found_in_nested_deltas() {
        let mut scan = MarkerScan::default();
        let mut out = Vec::new();
        let frame =
            serde_json::json!({"append":[{"entry":"e","part":"p","text":"x [[zb:b7000001:4]]"}]});
        scan_value(&mut scan, &frame, 9, &mut out);
        scan_value(&mut scan, &frame, 10, &mut out);
        assert_eq!(out, vec![("b7000001".to_string(), 4, 9)]);
    }
}
