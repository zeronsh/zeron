//! Direct terminal links: the protocol and both ends live in
//! `zeron-term-link` (the phone speaks it too); this is the host's
//! `terminal/{id}` stream service over this engine's PTYs.

pub use zeron_term_link::link::*;

use super::Terminals;

impl HostTerminals for Terminals {
    fn subscribe(
        &self,
        terminal_id: &str,
        after_seq: u64,
    ) -> Result<tokio::sync::mpsc::UnboundedReceiver<zeron_proto::TerminalEvent>, String> {
        Terminals::subscribe(self, terminal_id, Some(after_seq)).map_err(|e| e.to_string())
    }
    fn write_sequenced(
        &self,
        terminal_id: &str,
        client: &str,
        seq: u64,
        bytes: &[u8],
    ) -> Result<bool, String> {
        Terminals::write_sequenced(self, terminal_id, client, seq, bytes).map_err(|e| e.to_string())
    }
    fn resize(&self, terminal_id: &str, cols: u16, rows: u16) -> Result<(), String> {
        Terminals::resize(self, terminal_id, cols, rows).map_err(|e| e.to_string())
    }
}

/// The host's `terminal/{id}` stream service. Only another of the user's own
/// devices may attach: a Cloud machine runs agent code, and a link it opens
/// back over a connection this device made to it must not reach a shell
/// here. (Opening this device's terminals FROM a Cloud machine was never a
/// thing any viewer does; Cloud terminals are reached the other way round.)
pub struct Service {
    terminals: Terminals,
}

impl Service {
    pub fn new(terminals: Terminals) -> Self {
        Self { terminals }
    }
}

/// Whether `peer` may attach to this device's terminals.
fn peer_allowed(peer: Option<&str>) -> bool {
    peer.is_some_and(|peer| !zeron_proto::is_cloud_device(peer, ""))
}

#[async_trait::async_trait]
impl zeron_preview::mux::Connector for Service {
    async fn connect(&self, service: &str) -> anyhow::Result<zeron_preview::mux::BoxIo> {
        self.connect_from(None, service).await
    }
    async fn connect_from(
        &self,
        peer: Option<&str>,
        service: &str,
    ) -> anyhow::Result<zeron_preview::mux::BoxIo> {
        anyhow::ensure!(
            peer_allowed(peer),
            "terminals here are only reachable from the owner's devices"
        );
        let terminal_id = service
            .strip_prefix(SERVICE_PREFIX)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| anyhow::anyhow!("invalid terminal link"))?
            .to_owned();
        let (ours, theirs) = tokio::io::duplex(256 * 1024);
        tokio::spawn(serve(self.terminals.clone(), terminal_id, ours));
        Ok(Box::new(theirs))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD as BASE64;
    use std::time::Duration;
    use zeron_proto::TerminalEvent;

    fn text(events: &[TerminalEvent]) -> String {
        let mut out = Vec::new();
        for event in events {
            if let TerminalEvent::Data { data, .. } = event {
                out.extend(BASE64.decode(data).unwrap());
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    async fn until(
        link: &mut Link,
        seen: &mut Vec<TerminalEvent>,
        done: impl Fn(&[TerminalEvent]) -> bool,
    ) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while !done(seen) {
                seen.push(link.events.recv().await.expect("link ended early"));
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out; saw {:?}", text(seen)));
    }

    #[tokio::test]
    async fn input_output_resize_resume_and_exit_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let terminals = Terminals::new();
        let session = terminals
            .open_with_shell(dir.path().to_str().unwrap(), 80, 24, Some("/bin/sh"))
            .unwrap();
        let (viewer, host) = tokio::io::duplex(64 * 1024);
        tokio::spawn(serve(terminals.clone(), session.id.clone(), host));
        let mut link = attach(viewer, 0, "viewer").await.unwrap();
        let mut seen = Vec::new();

        link.outgoing
            .send(Outgoing::Input {
                seq: 1,
                bytes: b"echo li''nk-$((6*7))\n".to_vec(),
            })
            .unwrap();
        until(&mut link, &mut seen, |s| text(s).contains("link-42")).await;
        link.outgoing
            .send(Outgoing::Resize {
                cols: 101,
                rows: 33,
            })
            .unwrap();
        link.outgoing
            .send(Outgoing::Input {
                seq: 2,
                bytes: b"stty size\n".to_vec(),
            })
            .unwrap();
        until(&mut link, &mut seen, |s| text(s).contains("33 101")).await;

        // A second link resuming after what we saw replays nothing old.
        let last = seen
            .iter()
            .map(|e| match e {
                TerminalEvent::Data { seq, .. } | TerminalEvent::Exit { seq, .. } => *seq,
            })
            .max()
            .unwrap();
        drop(link);
        let (viewer, host) = tokio::io::duplex(64 * 1024);
        tokio::spawn(serve(terminals.clone(), session.id.clone(), host));
        let mut resumed = attach(viewer, last, "viewer").await.unwrap();
        resumed
            .outgoing
            .send(Outgoing::Input {
                seq: 3,
                bytes: b"echo af''ter; exit 3\n".to_vec(),
            })
            .unwrap();
        let mut after = Vec::new();
        until(&mut resumed, &mut after, |s| {
            s.iter().any(|e| matches!(e, TerminalEvent::Exit { .. }))
        })
        .await;
        assert!(text(&after).contains("after"));
        assert!(
            !text(&after).contains("link-42"),
            "resume replayed old output"
        );
        assert!(matches!(
            after.last(),
            Some(TerminalEvent::Exit { exit_code: 3, .. })
        ));
        terminals.close(&session.id).unwrap();
    }

    /// Frames must stay intact while output floods one way and keys stream
    /// the other: every select branch races the half-read frames.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn heavy_output_and_typing_never_desynchronize_the_stream() {
        let dir = tempfile::tempdir().unwrap();
        let terminals = Terminals::new();
        let session = terminals
            .open_with_shell(dir.path().to_str().unwrap(), 80, 24, Some("/bin/sh"))
            .unwrap();
        // A small pipe forces partial reads of every large frame.
        let (viewer, host) = tokio::io::duplex(4 * 1024);
        tokio::spawn(serve(terminals.clone(), session.id.clone(), host));
        let mut link = attach(viewer, 0, "viewer").await.unwrap();
        link.outgoing
            .send(Outgoing::Input {
                seq: 1,
                bytes: b"head -c 3000000 /dev/zero | tr '\\0' x; echo; echo FL''OOD-DONE\n"
                    .to_vec(),
            })
            .unwrap();
        let typer = {
            let outgoing = link.outgoing.clone();
            tokio::spawn(async move {
                // Keys the shell will see after the flood (harmless spaces).
                for seq in 2..400u64 {
                    if outgoing
                        .send(Outgoing::Input {
                            seq,
                            bytes: b" ".to_vec(),
                        })
                        .is_err()
                    {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
            })
        };
        let mut xs = 0usize;
        let mut tail = String::new();
        let mut last_seq = 0;
        tokio::time::timeout(Duration::from_secs(60), async {
            while !tail.contains("FLOOD-DONE") {
                match link
                    .events
                    .recv()
                    .await
                    .expect("link must survive the flood")
                {
                    TerminalEvent::Data { seq, data } => {
                        assert_eq!(seq, last_seq + 1, "events arrive whole and in order");
                        last_seq = seq;
                        let bytes = BASE64.decode(data).unwrap();
                        xs += bytes.iter().filter(|b| **b == b'x').count();
                        tail.push_str(&String::from_utf8_lossy(&bytes));
                        if tail.len() > 256 {
                            tail.drain(..tail.len() - 256);
                        }
                    }
                    TerminalEvent::Exit { .. } => panic!("shell exited"),
                }
            }
        })
        .await
        .expect("flood finished in time");
        assert!(xs >= 3_000_000, "lost output: {xs} x's");
        let mut acked = 0;
        while let Ok(seq) = link.acks.try_recv() {
            acked = acked.max(seq);
        }
        assert!(acked >= 1, "input was acknowledged");
        typer.abort();
        terminals.close(&session.id).unwrap();
    }

    #[tokio::test]
    async fn replayed_input_is_applied_once_per_client() {
        let dir = tempfile::tempdir().unwrap();
        let terminals = Terminals::new();
        let session = terminals
            .open_with_shell(dir.path().to_str().unwrap(), 80, 24, Some("/bin/sh"))
            .unwrap();
        let id = &session.id;
        assert!(terminals.write_sequenced(id, "a", 1, b"").unwrap());
        assert!(terminals.write_sequenced(id, "a", 2, b"").unwrap());
        assert!(
            !terminals.write_sequenced(id, "a", 1, b"").unwrap(),
            "replay"
        );
        assert!(
            !terminals.write_sequenced(id, "a", 2, b"").unwrap(),
            "replay"
        );
        assert!(terminals.write_sequenced(id, "a", 3, b"").unwrap());
        assert!(
            terminals.write_sequenced(id, "b", 1, b"").unwrap(),
            "each viewer numbers its own input"
        );
        terminals.close(id).unwrap();
    }

    #[test]
    fn only_the_owners_own_devices_may_attach() {
        assert!(peer_allowed(Some("laptop-device")));
        assert!(!peer_allowed(None), "no anonymous links");
        assert!(
            !peer_allowed(Some(&format!("{}abc", zeron_proto::CLOUD_DEVICE_PREFIX))),
            "a Cloud machine must not reach a shell here"
        );
    }

    #[tokio::test]
    async fn unknown_terminal_is_refused() {
        let terminals = Terminals::new();
        let (viewer, host) = tokio::io::duplex(1024);
        tokio::spawn(serve(terminals, "nope".into(), host));
        let err = attach(viewer, 0, "viewer").await.err().expect("refused");
        assert!(err.to_string().contains("Terminal not found"), "{err}");
    }
}
