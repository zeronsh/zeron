//! The coordinator receives presence, catalogs and SDP only. Reconnects obtain
//! fresh credentials and clear the previous connection's remote routes/peers.
use crate::{
    catalog::Catalog,
    peer::{OutgoingSignal, Peers, Signal},
};
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use std::{sync::Arc, time::Duration};
use tokio::sync::{Notify, mpsc};
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};
use tokio_util::sync::CancellationToken;
use zeron_proto::PreviewService;
#[async_trait::async_trait]
pub trait TokenSource: Send + Sync {
    async fn token(&self) -> anyhow::Result<String>;
}
pub struct Config {
    pub edge_url: String,
    pub org_id: String,
    pub tokens: Arc<dyn TokenSource>,
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum Incoming {
    Catalog {
        device: String,
        services: Vec<PreviewService>,
    },
    Gone {
        device: String,
    },
    Signal {
        from: String,
        signal: Signal,
    },
}
pub async fn run(
    config: Config,
    catalog: Catalog,
    peers: Peers,
    outgoing: mpsc::Receiver<OutgoingSignal>,
    stop: CancellationToken,
) {
    run_with_wake(
        config,
        catalog,
        peers,
        outgoing,
        stop,
        Arc::new(Notify::new()),
    )
    .await
}

/// [`run`], wakeable: a device whose network changed or that comes back
/// from suspension (a phone) notifies `wake`. Waiting to reconnect, it
/// redials at once; connected, it proves the socket with a ping and redials
/// if nothing answers within [`WAKE_PROBE`] — rather than leaning on the
/// 40s lease while the socket may be dead.
pub async fn run_with_wake(
    config: Config,
    catalog: Catalog,
    peers: Peers,
    outgoing: mpsc::Receiver<OutgoingSignal>,
    stop: CancellationToken,
    wake: Arc<Notify>,
) {
    run_signing_in(
        config,
        catalog,
        peers,
        outgoing,
        stop,
        wake,
        Arc::new(|| false),
    )
    .await
}

/// Whether this device is in the middle of a provider sign-in whose
/// callback rides the direct connection.
pub type SigningIn = Arc<dyn Fn() -> bool + Send + Sync>;

/// [`run_with_wake`] for an engine: while `signing_in` holds, it joins as a
/// sign-in, which the coordinator admits past its per-user device limit (a
/// user's idle Cloud machines and phones can fill it). Notify `wake` when a
/// sign-in starts so a refused device redials at once.
pub async fn run_signing_in(
    config: Config,
    catalog: Catalog,
    peers: Peers,
    mut outgoing: mpsc::Receiver<OutgoingSignal>,
    stop: CancellationToken,
    wake: Arc<Notify>,
    signing_in: SigningIn,
) {
    loop {
        let connected = connect(
            &config,
            &catalog,
            &peers,
            &mut outgoing,
            &wake,
            signing_in(),
        );
        tokio::select! { _ = stop.cancelled() => break, result = connected => {
            if let Err(error) = result { tracing::debug!(%error, "preview coordinator disconnected"); }
        } }
        catalog.clear_remote();
        peers.clear().await;
        while outgoing.try_recv().is_ok() {} // signaling from the old lease is stale
        tokio::select! {
            _ = stop.cancelled() => break,
            _ = tokio::time::sleep(Duration::from_secs(3)) => {}
            _ = wake.notified() => {}
        }
    }
    catalog.clear_remote();
    peers.clear().await;
}

fn coordinator_url(
    config: &Config,
    device: &str,
    signing_in: bool,
) -> anyhow::Result<reqwest::Url> {
    let mut url = reqwest::Url::parse(&config.edge_url)?;
    let scheme = if url.scheme() == "https" { "wss" } else { "ws" };
    url.set_scheme(scheme)
        .map_err(|_| anyhow::anyhow!("invalid preview coordinator URL"))?;
    url.set_path(&format!("/preview/{}/ws", config.org_id));
    let mut query = url.query_pairs_mut();
    query.clear().append_pair("device", device);
    if signing_in {
        query.append_pair("signIn", "1");
    }
    drop(query);
    Ok(url)
}

/// How long a woken connection has to answer its probe.
const WAKE_PROBE: Duration = Duration::from_millis(2500);
async fn connect(
    config: &Config,
    catalog: &Catalog,
    peers: &Peers,
    outgoing: &mut mpsc::Receiver<OutgoingSignal>,
    wake: &Notify,
    signing_in: bool,
) -> anyhow::Result<()> {
    let token = config.tokens.token().await?;
    let url = coordinator_url(config, catalog.device_id(), signing_in)?;
    let mut request = url.as_str().into_client_request()?;
    request
        .headers_mut()
        .insert("authorization", format!("Bearer {token}").parse()?);
    let mut websocket_config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default();
    websocket_config.max_message_size = Some(1024 * 1024);
    websocket_config.max_frame_size = Some(1024 * 1024);
    let (socket, _) = tokio::time::timeout(
        Duration::from_secs(10),
        tokio_tungstenite::connect_async_with_config(request, Some(websocket_config), false),
    )
    .await??;
    let (mut write, mut read) = socket.split();
    let mut changes = catalog.subscribe();
    let mut advertised = catalog.local_services();
    write
        .send(Message::Text(
            serde_json::json!({"type":"catalog", "services":advertised}).to_string(),
        ))
        .await?;
    let mut ping = tokio::time::interval(Duration::from_secs(10));
    let mut last_message = tokio::time::Instant::now();
    // A woken socket's probe: it must hear something by this deadline.
    let mut probe: Option<tokio::time::Instant> = None;
    // SDP gathering must not block the socket heartbeat or other devices.
    let mut negotiations = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            _ = ping.tick() => {
                anyhow::ensure!(last_message.elapsed() < Duration::from_secs(40), "preview coordinator lease expired");
                write.send(Message::Text("ping".into())).await?;
            }
            _ = wake.notified() => {
                write.send(Message::Text("ping".into())).await?;
                probe = Some(tokio::time::Instant::now() + WAKE_PROBE);
            }
            _ = async { tokio::time::sleep_until(probe.unwrap_or_else(tokio::time::Instant::now)).await }, if probe.is_some() => {
                anyhow::bail!("preview coordinator didn't answer after a wake");
            }
            _ = changes.changed() => {
                let next = catalog.local_services();
                if next != advertised { write.send(Message::Text(serde_json::json!({"type":"catalog", "services":next}).to_string())).await?; advertised = next; }
            }
            Some(message) = outgoing.recv() => {
                write.send(Message::Text(serde_json::json!({"type":"signal", "to":message.to, "signal":message.signal}).to_string())).await?;
            }
            Some(_) = negotiations.join_next(), if !negotiations.is_empty() => {}
            message = read.next() => {
                let message = message.ok_or_else(|| anyhow::anyhow!("preview coordinator closed"))??;
                last_message = tokio::time::Instant::now();
                probe = None;
                match message {
                    Message::Text(text) if text == "pong" => {},
                    Message::Text(text) => {
                        anyhow::ensure!(text.len() <= 1024*1024, "oversized preview coordinator message");
                        match serde_json::from_str::<Incoming>(&text)? {
                            Incoming::Catalog { device, services } => catalog.set_remote(&device, services)?,
                            Incoming::Gone { device } => { catalog.remove_remote(&device); peers.remove(&device).await; }
                            Incoming::Signal { from, signal } => {
                                anyhow::ensure!(negotiations.len() < 16, "too many concurrent preview negotiations");
                                let peers = peers.clone();
                                negotiations.spawn(async move { if let Err(error) = peers.signal(&from, signal).await { tracing::debug!(%error, "preview negotiation failed"); } });
                            }
                        }
                    }
                    Message::Ping(bytes) => write.send(Message::Pong(bytes)).await?,
                    Message::Close(_) => anyhow::bail!("preview coordinator closed"),
                    Message::Binary(_) => anyhow::bail!("application traffic is not allowed on preview signaling"),
                    _ => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoToken;
    #[async_trait::async_trait]
    impl TokenSource for NoToken {
        async fn token(&self) -> anyhow::Result<String> {
            anyhow::bail!("unused")
        }
    }

    #[test]
    fn a_sign_in_joins_flagged_so_the_device_limit_lets_it_in() {
        let config = Config {
            edge_url: "https://edge.example".into(),
            org_id: "org".into(),
            tokens: Arc::new(NoToken),
        };
        assert_eq!(
            coordinator_url(&config, "laptop", false).unwrap().as_str(),
            "wss://edge.example/preview/org/ws?device=laptop"
        );
        assert_eq!(
            coordinator_url(&config, "laptop", true).unwrap().as_str(),
            "wss://edge.example/preview/org/ws?device=laptop&signIn=1"
        );
    }
}
