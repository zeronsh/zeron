//! A chat2 room's checkpoint, POSTed to the edge (`doc_host`'s
//! `spawn_chat2_checkpoint` makes it): in one request up to the edge's
//! most, past it in parts.

use base64::Engine as _;

use crate::http_error::describe_http_error;

/// The most one request may carry of a chat2 checkpoint: the edge's
/// `MAX_CHECKPOINT_BYTES` (edge/src/chat-room.ts).
pub(crate) const CHAT2_CHECKPOINT_SINGLE_MAX: usize = 32 * 1024 * 1024;
/// A larger one goes in parts of this (the edge's `MAX_CHECKPOINT_PART_BYTES`).
pub(crate) const CHAT2_CHECKPOINT_PART_BYTES: usize = 8 * 1024 * 1024;

/// How many requests a checkpoint of `len` bytes is sent in.
pub(crate) fn chat2_checkpoint_parts(len: usize) -> usize {
    if len <= CHAT2_CHECKPOINT_SINGLE_MAX { 1 } else { len.div_ceil(CHAT2_CHECKPOINT_PART_BYTES) }
}

/// A checkpoint the edge refused as too large (413): when, how much the
/// doc held then (its operations, a measure that costs nothing to read),
/// and how long to leave it. An edge or engine from before checkpoint
/// parts refuses a long chat's every whole post, and it was posted again
/// at the next tick, its whole snapshot each time (35 MB and more, every
/// few seconds). Refused, the chat isn't posted again until the wait is
/// over (a minute, doubling to half an hour) or its doc has grown by a
/// quarter: then it's worth trying again.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Refused {
    pub at: std::time::Instant,
    pub ops: usize,
    pub wait: std::time::Duration,
}

/// The first wait after a refusal.
pub(crate) const REFUSED_FIRST_WAIT: std::time::Duration = std::time::Duration::from_secs(60);
/// The longest.
pub(crate) const REFUSED_MOST_WAIT: std::time::Duration = std::time::Duration::from_secs(30 * 60);

impl Refused {
    /// Refused (again) at `at`, the doc holding `ops`: the wait doubles
    /// from the one before, to its most.
    pub(crate) fn after(before: Option<Refused>, at: std::time::Instant, ops: usize) -> Refused {
        let wait = before.map_or(REFUSED_FIRST_WAIT, |b| (b.wait * 2).min(REFUSED_MOST_WAIT));
        Refused { at, ops, wait }
    }

    /// Whether to post again at `now`, the doc holding `ops`: the wait is
    /// over, or the doc has grown by a quarter since.
    pub(crate) fn allows(&self, now: std::time::Instant, ops: usize) -> bool {
        now.saturating_duration_since(self.at) >= self.wait || ops >= self.ops + self.ops / 4
    }
}

/// Why a checkpoint didn't land: the edge said no (its status), or the
/// request didn't get there.
#[derive(Debug, PartialEq)]
pub(crate) enum CheckpointPost {
    Rejected(u16),
    Failed(String),
}

/// POST `snapshot` as chat `chat_id`'s checkpoint covering rows up to
/// `seq_covered`. Up to one request's most, in one request as always; past
/// it (a long chat's state alone outgrew it: 35 and 39 MB, refused with 413
/// on every post), in parts to `/checkpoint-part`, then committed whole by
/// its size (`/checkpoint?staged=`). An edge from before parts answers the
/// first part 404: it stays without a checkpoint, as it was.
pub(crate) async fn post_chat2_checkpoint(
    http: &reqwest::Client,
    edge_url: &str,
    chat_id: &str,
    bearer: &str,
    seq_covered: u64,
    frontier: &[u8],
    snapshot: Vec<u8>,
) -> Result<(), CheckpointPost> {
    let base = format!("{}/chat2/{}", edge_url.trim_end_matches('/'), chat_id);
    let frontier = base64::engine::general_purpose::STANDARD.encode(frontier);
    let send = |request: reqwest::RequestBuilder| async move {
        match request.timeout(std::time::Duration::from_secs(300)).bearer_auth(bearer).send().await {
            Ok(res) if res.status().is_success() => Ok(()),
            Ok(res) => Err(CheckpointPost::Rejected(res.status().as_u16())),
            Err(err) => Err(CheckpointPost::Failed(describe_http_error(err))),
        }
    };
    let len = snapshot.len();
    if chat2_checkpoint_parts(len) == 1 {
        let url = format!("{base}/checkpoint?seqCovered={seq_covered}");
        return send(http.post(url).header("x-chat2-frontier", frontier).body(snapshot)).await;
    }
    // (One buffer, sliced: no part is copied.)
    let snapshot = bytes::Bytes::from(snapshot);
    for offset in (0..len).step_by(CHAT2_CHECKPOINT_PART_BYTES) {
        let part = snapshot.slice(offset..(offset + CHAT2_CHECKPOINT_PART_BYTES).min(len));
        send(http.post(format!("{base}/checkpoint-part?offset={offset}")).body(part)).await?;
    }
    let url = format!("{base}/checkpoint?seqCovered={seq_covered}&staged={len}");
    send(http.post(url).header("x-chat2-frontier", frontier)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Refused, a chat waits a minute, then two, four… to half an hour
    /// before its checkpoint is posted again, unless its doc grows by a
    /// quarter first: an old edge's 413 isn't re-earned every few seconds
    /// with the whole snapshot.
    #[test]
    fn a_refused_checkpoint_waits_longer_each_time_unless_the_doc_grows() {
        use std::time::{Duration, Instant};
        let t0 = Instant::now();
        let first = Refused::after(None, t0, 1000);
        assert_eq!(first.wait, Duration::from_secs(60));
        assert!(!first.allows(t0 + Duration::from_secs(5), 1000), "the next tick: not again");
        assert!(!first.allows(t0 + Duration::from_secs(59), 1240), "grown, but not by a quarter");
        assert!(first.allows(t0 + Duration::from_secs(5), 1250), "grown by a quarter: again");
        assert!(first.allows(t0 + Duration::from_secs(60), 1000), "the wait over: again");
        let mut r = first;
        let mut waits = vec![r.wait.as_secs()];
        for _ in 0..8 {
            r = Refused::after(Some(r), r.at + r.wait, 1000);
            waits.push(r.wait.as_secs());
        }
        assert_eq!(waits, vec![60, 120, 240, 480, 960, 1800, 1800, 1800, 1800]);
        // An hour after a first refusal, a still doc is posted 6 times, not
        // ~600 (a tick every few seconds).
        let (mut r, mut posts, mut at) = (Refused::after(None, t0, 1000), 1, t0);
        while at < t0 + Duration::from_secs(3600) {
            at += Duration::from_secs(6);
            if r.allows(at, 1000) {
                posts += 1;
                r = Refused::after(Some(r), at, 1000);
            }
        }
        assert!(posts <= 7, "{posts} posts in an hour");
    }
    use std::sync::{Arc, Mutex};

    fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    /// What a fake edge heard: each request's path and query, and its body.
    type Heard = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

    /// An edge on a local port answering every request 200, or 404 to
    /// `/checkpoint-part` (an edge from before parts) when `old`.
    async fn fake_edge(old: bool) -> (String, Heard) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let heard: Heard = Arc::default();
        let log = heard.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else { return };
                let log = log.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    loop {
                        // A request: its head, then content-length bytes.
                        let head_end = loop {
                            if let Some(at) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                                break at + 4;
                            }
                            let mut chunk = vec![0; 1 << 20];
                            match socket.read(&mut chunk).await {
                                Ok(0) | Err(_) => return,
                                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                            }
                        };
                        let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                        let target = head.split_whitespace().nth(1).unwrap_or_default().to_owned();
                        let len: usize = head
                            .lines()
                            .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap_or(0)))
                            .unwrap_or(0);
                        while buf.len() < head_end + len {
                            let mut chunk = vec![0; 1 << 20];
                            match socket.read(&mut chunk).await {
                                Ok(0) | Err(_) => return,
                                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                            }
                        }
                        let body = buf[head_end..head_end + len].to_vec();
                        buf.drain(..head_end + len);
                        let refuse = old && target.contains("/checkpoint-part");
                        lock(&log).push((target, body));
                        let status = if refuse { "404 Not Found" } else { "200 OK" };
                        let _ = socket.write_all(format!("HTTP/1.1 {status}\r\ncontent-length: 2\r\n\r\n{{}}").as_bytes()).await;
                    }
                });
            }
        });
        (url, heard)
    }

    /// A long chat's doc, past what one request may carry: a transcript of
    /// text that doesn't compress (as a real one's snapshot hardly does),
    /// committed a turn at a time, exported once.
    fn whale() -> (loro::LoroDoc, Vec<u8>) {
        let doc = loro::LoroDoc::new();
        let turns = doc.get_list("turns");
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut written = 0usize;
        while written < CHAT2_CHECKPOINT_SINGLE_MAX + 4 * 1024 * 1024 {
            let turn: String = (0..256 * 1024)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    char::from(b'!' + (seed % 90) as u8)
                })
                .collect();
            written += turn.len();
            turns.push(turn.as_str()).unwrap();
            doc.commit();
        }
        let snapshot = doc.export(loro::ExportMode::Snapshot).unwrap();
        (doc, snapshot)
    }

    /// A long chat's checkpoint, past one request's most (refused 413 with
    /// it all in one), goes in parts and lands whole: put back together, it's
    /// the very snapshot, and it imports to the very doc.
    #[tokio::test]
    async fn a_checkpoint_past_one_request_goes_in_parts_and_lands_whole() {
        let (doc, snapshot) = whale();
        assert!(snapshot.len() > CHAT2_CHECKPOINT_SINGLE_MAX);
        let (url, heard) = fake_edge(false).await;
        let http = reqwest::Client::new();
        let posted = post_chat2_checkpoint(&http, &url, "chat-1", "bearer", 42, b"\x01", snapshot.clone()).await;
        assert_eq!(posted, Ok(()));
        let heard = lock(&heard).clone();
        let parts = chat2_checkpoint_parts(snapshot.len());
        assert_eq!(heard.len(), parts + 1, "its parts, then the commit");
        let mut whole = Vec::new();
        for (i, (target, body)) in heard[..parts].iter().enumerate() {
            assert_eq!(target, &format!("/chat2/chat-1/checkpoint-part?offset={}", i * CHAT2_CHECKPOINT_PART_BYTES));
            assert!(body.len() <= CHAT2_CHECKPOINT_PART_BYTES);
            whole.extend_from_slice(body);
        }
        assert_eq!(heard[parts].0, format!("/chat2/chat-1/checkpoint?seqCovered=42&staged={}", snapshot.len()));
        assert!(heard[parts].1.is_empty());
        assert!(whole == snapshot, "the parts make the snapshot");
        let reader = loro::LoroDoc::new();
        reader.import(&whole).unwrap();
        assert_eq!(reader.get_deep_value(), doc.get_deep_value());
    }

    /// Up to one request's most it goes as it always has: one POST with it
    /// all. Past it, an edge from before parts (404) leaves it rejected, as
    /// the 413 did, and nothing is committed.
    #[tokio::test]
    async fn a_small_one_is_one_request_and_an_old_edge_refuses_parts() {
        let (url, heard) = fake_edge(true).await;
        let http = reqwest::Client::new();
        let small = vec![7u8; 1024];
        assert_eq!(post_chat2_checkpoint(&http, &url, "c", "b", 3, b"\x01", small.clone()).await, Ok(()));
        assert_eq!(lock(&heard).clone(), vec![("/chat2/c/checkpoint?seqCovered=3".to_owned(), small)]);
        lock(&heard).clear();
        let large = vec![9u8; CHAT2_CHECKPOINT_SINGLE_MAX + 1];
        assert_eq!(
            post_chat2_checkpoint(&http, &url, "c", "b", 3, b"\x01", large).await,
            Err(CheckpointPost::Rejected(404))
        );
        let heard = lock(&heard).clone();
        assert_eq!(heard.len(), 1, "stops at the first part refused");
        assert_eq!(heard[0].0, "/chat2/c/checkpoint-part?offset=0");
    }
}
