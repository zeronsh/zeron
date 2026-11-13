//! `WatchDeviceLoads`: how busy each of the account's devices is, every
//! [`EVERY`] while someone watches. This one from `sysinfo`; the others
//! through `DeviceStats` over their relay (each within its own deadline, all
//! at once), a device that's away or doesn't answer as offline.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use futures::StreamExt;
use futures::stream::BoxStream;
use zeron_proto::orchestration::DeviceLoad;

use crate::device_stats::DeviceStats;
use crate::rpc::EngineRpc;

pub const EVERY: Duration = Duration::from_secs(5);

/// A `DeviceStats` report as a load.
pub(crate) fn load_of(device_id: &str, stats: &DeviceStats) -> DeviceLoad {
    let total = stats.memory.total_bytes;
    DeviceLoad {
        device_id: device_id.to_owned(),
        cpu: stats.cpu.usage.map(|u| u.clamp(0.0, 1.0)),
        memory_used_bytes: total
            .zip(stats.memory.available_bytes)
            .map(|(total, available)| total.saturating_sub(available)),
        memory_total_bytes: total,
        load1: stats.load_average.as_ref().map(|l| l.one as f32),
        cores: stats.cpu.logical_cores.map(|n| n as u32),
        online: true,
        at: stats.collected_at.unwrap_or_else(Utc::now),
    }
}

fn offline(device_id: &str) -> DeviceLoad {
    DeviceLoad {
        device_id: device_id.to_owned(),
        cpu: None,
        memory_used_bytes: None,
        memory_total_bytes: None,
        load1: None,
        cores: None,
        online: false,
        at: Utc::now(),
    }
}

impl EngineRpc {
    /// One sample of every device's load.
    pub(crate) async fn device_loads(&self) -> Vec<DeviceLoad> {
        let me = self.doc_host_ref().device_id().to_owned();
        let devices = self.workspace_host().watch_devices().borrow().clone();
        let asks = devices.iter().map(|device| {
            let me = me.clone();
            let id = device.id.clone();
            async move {
                let params = if id == me {
                    serde_json::json!({})
                } else {
                    if self.workspace_host().peer_liveness(&id) == zeron_rpc::PeerLiveness::Dark {
                        return offline(&id);
                    }
                    serde_json::json!({ "targetDeviceId": id })
                };
                match self.device_stats_value(params).await {
                    Some(stats) => load_of(&id, &stats),
                    None => offline(&id),
                }
            }
        });
        let mut loads = futures::future::join_all(asks).await;
        if !loads.iter().any(|l| l.device_id == me) {
            if let Some(stats) = self.device_stats_value(serde_json::json!({})).await {
                loads.push(load_of(&me, &stats));
            }
        }
        loads
    }
}

/// Every device's load now, then every [`EVERY`], until the watcher goes.
pub(crate) fn stream(rpc: Arc<EngineRpc>) -> BoxStream<'static, serde_json::Value> {
    futures::stream::unfold((rpc, true), |(rpc, first)| async move {
        if !first {
            tokio::time::sleep(EVERY).await;
        }
        let loads = rpc.device_loads().await;
        let value = serde_json::to_value(&loads).ok()?;
        Some((value, (rpc, false)))
    })
    .boxed()
}
