//! A terminal link over a real WebRTC pair (host candidates on this machine):
//! the keystroke → PTY echo round trip must cost transport time only, with
//! nothing in the DataChannel/mux path holding small frames back.
#![cfg(unix)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use tokio_util::sync::CancellationToken;
use zeron_engine::Terminals;
use zeron_engine::terminals::link::{self, Outgoing};
use zeron_preview::peer::Peers;
use zeron_proto::TerminalEvent;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn keystroke_echo_over_a_webrtc_link_is_transport_bound() {
    let dir = tempfile::tempdir().unwrap();
    let terminals = Terminals::new();
    let session = terminals
        .open_with_shell(dir.path().to_str().unwrap(), 80, 24, Some("/bin/sh"))
        .unwrap();

    let stop = CancellationToken::new();
    let service = Arc::new(link::Service::new(terminals.clone()));
    let (mut host, mut host_out) = Peers::new("device-a".into(), service.clone(), stop.clone());
    let (mut viewer, mut viewer_out) = Peers::new("device-b".into(), service, stop.clone());
    host.set_ice_servers(Vec::new()).unwrap();
    viewer.set_ice_servers(Vec::new()).unwrap();
    let to_viewer = viewer.clone();
    let forward_host = tokio::spawn(async move {
        while let Some(message) = host_out.recv().await {
            let _ = to_viewer.signal("device-a", message.signal).await;
        }
    });
    let to_host = host.clone();
    let forward_viewer = tokio::spawn(async move {
        while let Some(message) = viewer_out.recv().await {
            let _ = to_host.signal("device-b", message.signal).await;
        }
    });

    let stream = tokio::time::timeout(
        Duration::from_secs(20),
        viewer.open("device-a", &link::service_id(&session.id), false),
    )
    .await
    .expect("pairing in time")
    .expect("direct stream");
    let mut link = link::attach(stream, 0, "viewer").await.expect("attach");

    // `cat` in raw mode echoes each byte the moment the PTY sees it.
    link.outgoing
        .send(Outgoing::Input {
            seq: 1,
            bytes: b"stty raw -echo; echo RE''ADY; cat\n".to_vec(),
        })
        .unwrap();
    let mut seen = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), async {
        // The quotes keep the typed command's own echo from matching.
        while !String::from_utf8_lossy(&seen).contains("READY") {
            if let Some(TerminalEvent::Data { data, .. }) = link.events.recv().await {
                seen.extend(
                    base64::engine::general_purpose::STANDARD
                        .decode(data)
                        .unwrap(),
                );
            }
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "shell never got ready: {:?}",
            String::from_utf8_lossy(&seen)
        )
    });
    // Let the shell settle into cat before timing.
    tokio::time::sleep(Duration::from_millis(200)).await;
    while link.events.try_recv().is_ok() {}

    let mut samples = Vec::new();
    for i in 0..40u8 {
        let key = b'a' + (i % 26);
        let sent = Instant::now();
        link.outgoing
            .send(Outgoing::Input {
                seq: 2 + u64::from(i),
                bytes: vec![key],
            })
            .unwrap();
        loop {
            let event = tokio::time::timeout(Duration::from_secs(5), link.events.recv())
                .await
                .expect("echo in time")
                .expect("link alive");
            if let TerminalEvent::Data { data, .. } = event
                && base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .unwrap()
                    .contains(&key)
            {
                break;
            }
        }
        samples.push(sent.elapsed());
        // Typing cadence, so each sample is an idle-path round trip.
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    samples.sort();
    let median = samples[samples.len() / 2];
    let p90 = samples[samples.len() * 9 / 10];
    eprintln!("keystroke echo over WebRTC: median {median:?}, p90 {p90:?}");
    assert!(
        median < Duration::from_millis(15) && p90 < Duration::from_millis(40),
        "echo round trip should be transport-bound on loopback: median {median:?}, p90 {p90:?}"
    );

    stop.cancel();
    host.clear().await;
    viewer.clear().await;
    forward_host.abort();
    forward_viewer.abort();
    terminals.close(&session.id).unwrap();
}
