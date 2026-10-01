//! Two transfer services wired back to back over in-memory lanes: folders,
//! resume after a severed tunnel, confirmation, cancellation, and a sender
//! that lies in its manifest or its blocks.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use zeron_proto::{FileTransfer, FileTransferState, FileTransferTransport};
use zeron_transfer::manifest::{Entry, EntryKind, Manifest};
use zeron_transfer::wire::{self, Block, Lane, Msg};
use zeron_transfer::{
    BoxIo, Network, SendRequest, Transfers, TransfersConfig, TransportPolicy, Tunnel,
};

/// Lanes to one peer's service. `budget` (bytes, shared by every lane of
/// the first `severed` connections) cuts the tunnel mid-transfer.
struct Net {
    me: String,
    peer: OnceLock<Transfers>,
    budget: Arc<AtomicI64>,
    connects: AtomicUsize,
    severed: usize,
    cancels: Mutex<Vec<String>>,
    /// Throttle lanes (so a test can act mid-transfer).
    slow: std::sync::atomic::AtomicBool,
    /// Bytes written on connections that were not severed (the resume).
    resumed_bytes: Arc<AtomicI64>,
}

struct MemTunnel {
    me: String,
    peer: Transfers,
    budget: Option<Arc<AtomicI64>>,
    slow: bool,
    resumed_bytes: Arc<AtomicI64>,
}

#[async_trait::async_trait]
impl Tunnel for MemTunnel {
    fn transport(&self) -> FileTransferTransport {
        FileTransferTransport::P2p
    }
    fn max_lanes(&self) -> usize {
        4
    }
    async fn open_lane(&self) -> anyhow::Result<BoxIo> {
        let (a, b) = tokio::io::duplex(256 * 1024);
        self.peer
            .accept_lane(self.me.clone(), FileTransferTransport::P2p, Box::new(b));
        Ok(match &self.budget {
            Some(budget) => Box::new(Fuse {
                inner: a,
                budget: budget.clone(),
            }),
            None if self.slow => Box::new(Slow {
                inner: a,
                delay: None,
            }),
            None => Box::new(Counted {
                inner: a,
                written: self.resumed_bytes.clone(),
            }),
        })
    }
    async fn close(&self) {}
}

#[async_trait::async_trait]
impl Network for Net {
    async fn connect(
        &self,
        _device: &str,
        _policy: TransportPolicy,
    ) -> anyhow::Result<Box<dyn Tunnel>> {
        let n = self.connects.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(MemTunnel {
            me: self.me.clone(),
            peer: self.peer.get().expect("wired").clone(),
            budget: (n < self.severed).then(|| self.budget.clone()),
            slow: self.slow.load(Ordering::SeqCst),
            resumed_bytes: self.resumed_bytes.clone(),
        }))
    }
    async fn notify_cancel(&self, _device: &str, transfer_id: &str) {
        self.cancels.lock().unwrap().push(transfer_id.to_owned());
        if let Some(peer) = self.peer.get() {
            let _ = peer.cancel(transfer_id, false);
        }
    }
    fn device_name(&self, device: &str) -> Option<String> {
        Some(format!("{device} name"))
    }
    fn destination_roots(&self) -> Vec<PathBuf> {
        Vec::new()
    }
}

/// Fails every read and write once the shared byte budget is spent — a
/// tunnel dying mid-block.
struct Fuse<T> {
    inner: T,
    budget: Arc<AtomicI64>,
}

impl<T: AsyncRead + Unpin> AsyncRead for Fuse<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.budget.load(Ordering::SeqCst) <= 0 {
            return Poll::Ready(Err(std::io::ErrorKind::ConnectionReset.into()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for Fuse<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if self.budget.fetch_sub(data.len() as i64, Ordering::SeqCst) <= 0 {
            return Poll::Ready(Err(std::io::ErrorKind::ConnectionReset.into()));
        }
        Pin::new(&mut self.inner).poll_write(cx, data)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// Counts the bytes written through it.
struct Counted<T> {
    inner: T,
    written: Arc<AtomicI64>,
}

impl<T: AsyncRead + Unpin> AsyncRead for Counted<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for Counted<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let result = std::task::ready!(Pin::new(&mut self.inner).poll_write(cx, data));
        if let Ok(n) = &result {
            self.written.fetch_add(*n as i64, Ordering::SeqCst);
        }
        Poll::Ready(result)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// ~12 MB/s per lane: 64 KiB writes 5 ms apart.
struct Slow<T> {
    inner: T,
    delay: Option<Pin<Box<tokio::time::Sleep>>>,
}

impl<T: AsyncRead + Unpin> AsyncRead for Slow<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for Slow<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if let Some(delay) = self.delay.as_mut() {
            std::task::ready!(delay.as_mut().poll(cx));
            self.delay = None;
        }
        let n = data.len().min(64 * 1024);
        let result = std::task::ready!(Pin::new(&mut self.inner).poll_write(cx, &data[..n]));
        self.delay = Some(Box::pin(tokio::time::sleep(Duration::from_millis(5))));
        Poll::Ready(result)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

struct Pair {
    sender: Transfers,
    receiver: Transfers,
    sender_net: Arc<Net>,
    home: PathBuf,
    _dir: tempfile::TempDir,
}

fn net(me: &str, severed: usize, budget: i64) -> Arc<Net> {
    Arc::new(Net {
        me: me.into(),
        peer: OnceLock::new(),
        budget: Arc::new(AtomicI64::new(budget)),
        connects: AtomicUsize::new(0),
        severed,
        cancels: Mutex::new(Vec::new()),
        slow: Default::default(),
        resumed_bytes: Arc::new(AtomicI64::new(0)),
    })
}

fn service(dir: &Path, id: &str, home: Option<PathBuf>, net: Arc<Net>) -> Transfers {
    Transfers::new(
        TransfersConfig {
            device_id: id.into(),
            device_name: format!("{id} name"),
            state_dir: dir.join(id).join("state"),
            settings_file: dir.join(id).join("settings.json"),
            home_dir: home,
        },
        net,
    )
}

fn pair(severed: usize, budget: i64) -> Pair {
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let sender_net = net("laptop", severed, budget);
    let receiver_net = net("phone", 0, 0);
    let sender = service(dir.path(), "laptop", None, sender_net.clone());
    let receiver = service(
        dir.path(),
        "phone",
        Some(home.clone()),
        receiver_net.clone(),
    );
    let _ = sender_net.peer.set(receiver.clone());
    let _ = receiver_net.peer.set(sender.clone());
    Pair {
        sender,
        receiver,
        sender_net,
        home,
        _dir: dir,
    }
}

async fn wait_for(
    transfers: &Transfers,
    id: &str,
    what: impl Fn(&FileTransfer) -> bool,
) -> FileTransfer {
    let mut watch = transfers.watch();
    let found = tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            if let Some(row) = watch.borrow_and_update().iter().find(|r| r.id == id)
                && what(row)
            {
                return row.clone();
            }
            watch.changed().await.unwrap();
        }
    })
    .await;
    found.unwrap_or_else(|_| panic!("timed out; rows: {:#?}", transfers.list()))
}

fn state_is(state: FileTransferState) -> impl Fn(&FileTransfer) -> bool {
    move |row| row.state == state
}

/// Deterministic, position-dependent bytes: a misplaced block shows.
fn pattern(size: usize, seed: u64) -> Vec<u8> {
    let mut x = seed | 1;
    (0..size)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

fn project(root: &Path) -> PathBuf {
    let project = root.join("src-project");
    std::fs::create_dir_all(project.join("app/src")).unwrap();
    std::fs::create_dir_all(project.join("empty-dir")).unwrap();
    std::fs::write(project.join("README.md"), b"# hello\n").unwrap();
    std::fs::write(
        project.join("app/src/main.rs"),
        pattern(3 * 1024 * 1024 + 17, 1),
    )
    .unwrap();
    std::fs::write(project.join("app/empty.txt"), b"").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(project.join("run.sh"), b"#!/bin/sh\necho hi\n").unwrap();
        std::fs::set_permissions(
            project.join("run.sh"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        std::os::unix::fs::symlink("app/src/main.rs", project.join("main-link")).unwrap();
    }
    project
}

fn assert_same_tree(a: &Path, b: &Path) {
    for entry in std::fs::read_dir(a).unwrap() {
        let entry = entry.unwrap();
        let other = b.join(entry.file_name());
        let meta = std::fs::symlink_metadata(entry.path()).unwrap();
        let other_meta = std::fs::symlink_metadata(&other)
            .unwrap_or_else(|_| panic!("{} missing", other.display()));
        if meta.file_type().is_symlink() {
            assert!(
                other_meta.file_type().is_symlink(),
                "{} must stay a symlink",
                other.display()
            );
            assert_eq!(
                std::fs::read_link(entry.path()).unwrap(),
                std::fs::read_link(&other).unwrap()
            );
        } else if meta.is_dir() {
            assert!(other_meta.is_dir());
            assert_same_tree(&entry.path(), &other);
        } else {
            assert_eq!(
                std::fs::read(entry.path()).unwrap(),
                std::fs::read(&other).unwrap(),
                "{}",
                other.display()
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    meta.permissions().mode() & 0o777,
                    other_meta.permissions().mode() & 0o777
                );
            }
        }
    }
}

fn no_parts(dir: &Path) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        assert!(!name.ends_with(".part"), "leftover {name}");
        if entry.file_type().unwrap().is_dir() {
            no_parts(&entry.path());
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_folder_and_a_file_arrive_intact_in_the_inbox() {
    let pair = pair(0, 0);
    let project = project(pair._dir.path());
    let apk = pair._dir.path().join("app-debug.apk");
    std::fs::write(&apk, pattern(2 * 1024 * 1024, 7)).unwrap();
    let reply = pair
        .sender
        .send(SendRequest {
            to: "phone".into(),
            paths: vec![project.clone(), apk.clone()],
            destination: None,
            policy: TransportPolicy::Auto,
        })
        .await
        .unwrap();
    assert_eq!(reply.to_device_name, "phone name");
    let sent = wait_for(
        &pair.sender,
        &reply.transfer_id,
        state_is(FileTransferState::Completed),
    )
    .await;
    assert_eq!(sent.done_bytes, sent.total_bytes);
    let received = wait_for(
        &pair.receiver,
        &reply.transfer_id,
        state_is(FileTransferState::Completed),
    )
    .await;
    let inbox = pair.home.join("Zeron Transfers").join("laptop name");
    assert_eq!(
        received.destination.as_deref(),
        Some(inbox.to_str().unwrap())
    );
    assert_eq!(
        received.items[0].path.as_deref(),
        Some(inbox.join("src-project").to_str().unwrap())
    );
    assert_same_tree(&project, &inbox.join("src-project"));
    assert!(inbox.join("src-project/empty-dir").is_dir());
    assert_eq!(
        std::fs::read(inbox.join("app-debug.apk")).unwrap(),
        std::fs::read(&apk).unwrap()
    );
    no_parts(&inbox);

    // The same items again land beside the first copy, never over it.
    let again = pair
        .sender
        .send(SendRequest {
            to: "phone".into(),
            paths: vec![project.clone(), apk.clone()],
            destination: None,
            policy: TransportPolicy::Auto,
        })
        .await
        .unwrap();
    wait_for(
        &pair.receiver,
        &again.transfer_id,
        state_is(FileTransferState::Completed),
    )
    .await;
    assert!(inbox.join("src-project (2)/README.md").is_file());
    assert!(inbox.join("app-debug (2).apk").is_file());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_severed_tunnel_resumes_from_verified_blocks() {
    // The first connection dies after ~20 MiB of a 48 MiB file.
    let pair = pair(1, 20 * 1024 * 1024);
    let big = pair._dir.path().join("big.bin");
    let bytes = pattern(48 * 1024 * 1024 + 5, 3);
    std::fs::write(&big, &bytes).unwrap();
    let reply = pair
        .sender
        .send(SendRequest {
            to: "phone".into(),
            paths: vec![big.clone()],
            destination: None,
            policy: TransportPolicy::Auto,
        })
        .await
        .unwrap();
    let interrupted = wait_for(
        &pair.sender,
        &reply.transfer_id,
        state_is(FileTransferState::Reconnecting),
    )
    .await;
    assert!(interrupted.done_bytes < interrupted.total_bytes);
    let receiving = wait_for(
        &pair.receiver,
        &reply.transfer_id,
        state_is(FileTransferState::Reconnecting),
    )
    .await;
    let before = receiving.done_bytes;
    assert!(before > 0, "some blocks were verified before the cut");
    let done = wait_for(
        &pair.receiver,
        &reply.transfer_id,
        state_is(FileTransferState::Completed),
    )
    .await;
    assert_eq!(done.done_bytes, bytes.len() as u64);
    let landed = pair.home.join("Zeron Transfers/laptop name/big.bin");
    assert!(std::fs::read(&landed).unwrap() == bytes);
    assert_eq!(
        pair.sender_net.connects.load(Ordering::SeqCst),
        2,
        "one resume"
    );
    wait_for(
        &pair.sender,
        &reply.transfer_id,
        state_is(FileTransferState::Completed),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn confirmation_decline_and_cancel_reach_both_sides() {
    let pair = pair(0, 0);
    let mut settings = pair.receiver.settings();
    settings.require_confirmation = true;
    pair.receiver.set_settings(settings).unwrap();
    let file = pair._dir.path().join("notes.txt");
    std::fs::write(&file, b"notes").unwrap();
    let send = |paths: Vec<PathBuf>| {
        pair.sender.send(SendRequest {
            to: "phone".into(),
            paths,
            destination: None,
            policy: TransportPolicy::Auto,
        })
    };

    // Declined.
    let declined = send(vec![file.clone()]).await.unwrap();
    wait_for(
        &pair.receiver,
        &declined.transfer_id,
        state_is(FileTransferState::AwaitingAcceptance),
    )
    .await;
    wait_for(
        &pair.sender,
        &declined.transfer_id,
        state_is(FileTransferState::AwaitingAcceptance),
    )
    .await;
    pair.receiver.decline(&declined.transfer_id).unwrap();
    wait_for(
        &pair.sender,
        &declined.transfer_id,
        state_is(FileTransferState::Declined),
    )
    .await;
    assert!(
        !pair
            .home
            .join("Zeron Transfers/laptop name/notes.txt")
            .exists()
    );

    // Accepted.
    let accepted = send(vec![file.clone()]).await.unwrap();
    wait_for(
        &pair.receiver,
        &accepted.transfer_id,
        state_is(FileTransferState::AwaitingAcceptance),
    )
    .await;
    pair.receiver.accept(&accepted.transfer_id).unwrap();
    wait_for(
        &pair.sender,
        &accepted.transfer_id,
        state_is(FileTransferState::Completed),
    )
    .await;
    assert_eq!(
        std::fs::read(pair.home.join("Zeron Transfers/laptop name/notes.txt")).unwrap(),
        b"notes"
    );

    // Cancelled by the sender while waiting.
    let cancelled = send(vec![file.clone()]).await.unwrap();
    wait_for(
        &pair.receiver,
        &cancelled.transfer_id,
        state_is(FileTransferState::AwaitingAcceptance),
    )
    .await;
    pair.sender.cancel(&cancelled.transfer_id, true).unwrap();
    wait_for(
        &pair.receiver,
        &cancelled.transfer_id,
        state_is(FileTransferState::Cancelled),
    )
    .await;
    wait_for(
        &pair.sender,
        &cancelled.transfer_id,
        state_is(FileTransferState::Cancelled),
    )
    .await;

    // Cancelled by the receiver mid-transfer: partial data is removed.
    let mut settings = pair.receiver.settings();
    settings.require_confirmation = false;
    pair.receiver.set_settings(settings).unwrap();
    let big = pair._dir.path().join("large.bin");
    std::fs::write(&big, pattern(256 * 1024 * 1024, 9)).unwrap();
    pair.sender_net.slow.store(true, Ordering::SeqCst);
    let midway = send(vec![big]).await.unwrap();
    wait_for(&pair.receiver, &midway.transfer_id, |r| r.done_bytes > 0).await;
    pair.receiver.cancel(&midway.transfer_id, true).unwrap();
    wait_for(
        &pair.sender,
        &midway.transfer_id,
        state_is(FileTransferState::Cancelled),
    )
    .await;
    wait_for(
        &pair.receiver,
        &midway.transfer_id,
        state_is(FileTransferState::Cancelled),
    )
    .await;
    no_parts(&pair.home.join("Zeron Transfers/laptop name"));
    assert!(
        !pair
            .home
            .join("Zeron Transfers/laptop name/large.bin")
            .exists()
    );
}

/// A sender that cancels and hangs up before the receiver even asked its
/// user: the queued `Cancel` wins over the broken lane.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cancel_racing_the_confirmation_prompt_is_not_lost() {
    let pair = pair(0, 0);
    let mut settings = pair.receiver.settings();
    settings.require_confirmation = true;
    pair.receiver.set_settings(settings).unwrap();
    for _ in 0..20 {
        let (mut ours, theirs) = tokio::io::duplex(1024 * 1024);
        pair.receiver.accept_lane(
            "laptop".into(),
            FileTransferTransport::P2p,
            Box::new(theirs),
        );
        let manifest = Manifest {
            entries: vec![Entry {
                path: "f.txt".into(),
                kind: EntryKind::File,
                size: 4,
                mode: 0o644,
                target: None,
            }],
        };
        let transfer_id = uuid::Uuid::new_v4().to_string();
        for msg in [
            Msg::Hello {
                v: wire::PROTOCOL_VERSION,
                transfer_id: transfer_id.clone(),
                session_id: "s1".into(),
                lane: Lane::Control,
                sender_name: "laptop".into(),
            },
            Msg::Offer {
                entry_count: 1,
                file_count: 1,
                total_bytes: 4,
                digest: manifest.digest(),
                destination: None,
                skipped: 0,
            },
            Msg::Manifest {
                entries: manifest.entries.clone(),
            },
            Msg::ManifestEnd,
            Msg::Cancel { reason: None },
        ] {
            wire::write_msg(&mut ours, &msg).await.unwrap();
        }
        drop(ours);
        wait_for(
            &pair.receiver,
            &transfer_id,
            state_is(FileTransferState::Cancelled),
        )
        .await;
    }
}

/// Speak the wire protocol by hand, as a hostile or buggy sender would.
async fn raw_offer(receiver: &Transfers, entries: Vec<Entry>) -> (BoxIo, Msg) {
    let (mut ours, theirs) = tokio::io::duplex(1024 * 1024);
    receiver.accept_lane(
        "laptop".into(),
        FileTransferTransport::Relay,
        Box::new(theirs),
    );
    let manifest = Manifest { entries };
    let transfer_id = uuid::Uuid::new_v4().to_string();
    for msg in [
        Msg::Hello {
            v: wire::PROTOCOL_VERSION,
            transfer_id,
            session_id: "s1".into(),
            lane: Lane::Control,
            sender_name: "laptop".into(),
        },
        Msg::Offer {
            entry_count: manifest.entries.len() as u64,
            file_count: manifest.file_count(),
            total_bytes: manifest.total_bytes(),
            digest: manifest.digest(),
            destination: None,
            skipped: 0,
        },
        Msg::Manifest {
            entries: manifest.entries.clone(),
        },
        Msg::ManifestEnd,
    ] {
        wire::write_msg(&mut ours, &msg).await.unwrap();
    }
    let reply = tokio::time::timeout(Duration::from_secs(10), wire::read_msg(&mut ours))
        .await
        .unwrap()
        .unwrap();
    (Box::new(ours), reply)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn traversal_in_a_manifest_is_refused_and_nothing_is_written() {
    let pair = pair(0, 0);
    let evil = |path: &str, kind| Entry {
        path: path.into(),
        kind,
        size: 4,
        mode: 0,
        target: (kind == EntryKind::Symlink).then(|| "/".into()),
    };
    for entries in [
        vec![evil("../../escape.txt", EntryKind::File)],
        vec![evil("/abs.txt", EntryKind::File)],
        vec![
            evil("ok", EntryKind::Dir),
            evil("ok/../../x", EntryKind::File),
        ],
        vec![
            evil("link", EntryKind::Symlink),
            evil("link/etc-passwd", EntryKind::File),
        ],
    ] {
        let (_io, reply) = raw_offer(&pair.receiver, entries).await;
        match reply {
            Msg::Error { message } => assert!(message.starts_with("Refused"), "{message}"),
            other => panic!("a traversal was answered with {other:?}"),
        }
    }
    assert!(!pair._dir.path().join("escape.txt").exists());
    assert!(
        !pair.home.join("Zeron Transfers").exists()
            || std::fs::read_dir(pair.home.join("Zeron Transfers"))
                .unwrap()
                .next()
                .is_none()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn corrupt_blocks_are_rejected_and_requested_again() {
    let pair = pair(0, 0);
    let data = b"four".to_vec();
    let (mut control, reply) = raw_offer(
        &pair.receiver,
        vec![Entry {
            path: "f.txt".into(),
            kind: EntryKind::File,
            size: 4,
            mode: 0o644,
            target: None,
        }],
    )
    .await;
    assert!(
        matches!(reply, Msg::Accept { ref have } if have.is_empty()),
        "{reply:?}"
    );
    let id = pair.receiver.list()[0].id.clone();
    let (mut lane, theirs) = tokio::io::duplex(1024 * 1024);
    pair.receiver.accept_lane(
        "laptop".into(),
        FileTransferTransport::Relay,
        Box::new(theirs),
    );
    wire::write_msg(
        &mut lane,
        &Msg::Hello {
            v: wire::PROTOCOL_VERSION,
            transfer_id: id.clone(),
            session_id: "s1".into(),
            lane: Lane::Data,
            sender_name: "laptop".into(),
        },
    )
    .await
    .unwrap();
    // A block whose bytes don't match its hash.
    let mut bad = Block::new(0, 0, data.clone());
    bad.data[0] ^= 0xff;
    wire::write_block(&mut lane, &bad).await.unwrap();
    loop {
        match wire::read_msg(&mut control).await.unwrap() {
            Msg::Resend {
                file: 0,
                blocks: Some(blocks),
            } => {
                assert_eq!(blocks, vec![0]);
                break;
            }
            Msg::Progress { done_bytes } => assert_eq!(done_bytes, 0),
            other => panic!("unexpected {other:?}"),
        }
    }
    // The good block plus a digest that doesn't match: the whole file is
    // requested again rather than renamed into place.
    wire::write_block(&mut lane, &Block::new(0, 0, data.clone()))
        .await
        .unwrap();
    wire::write_msg(
        &mut control,
        &Msg::Digest {
            file: 0,
            sha256: "0".repeat(64),
        },
    )
    .await
    .unwrap();
    loop {
        match wire::read_msg(&mut control).await.unwrap() {
            Msg::Resend {
                file: 0,
                blocks: None,
            } => break,
            Msg::Progress { .. } => {}
            other => panic!("unexpected {other:?}"),
        }
    }
    assert!(!pair.home.join("Zeron Transfers/laptop name/f.txt").exists());
    // Correct bytes and digest complete it.
    wire::write_block(&mut lane, &Block::new(0, 0, data.clone()))
        .await
        .unwrap();
    let digest = zeron_transfer::manifest::hex(&<sha2::Sha256 as sha2::Digest>::digest(&data));
    wire::write_msg(
        &mut control,
        &Msg::Digest {
            file: 0,
            sha256: digest,
        },
    )
    .await
    .unwrap();
    loop {
        match wire::read_msg(&mut control).await.unwrap() {
            Msg::Complete => break,
            Msg::Progress { .. } => {}
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(
        std::fs::read(pair.home.join("Zeron Transfers/laptop name/f.txt")).unwrap(),
        data
    );
    wait_for(&pair.receiver, &id, state_is(FileTransferState::Completed)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_explicit_destination_must_be_inside_home() {
    let pair = pair(0, 0);
    let file = pair._dir.path().join("a.txt");
    std::fs::write(&file, b"a").unwrap();
    let project = pair.home.join("projects/app");
    std::fs::create_dir_all(&project).unwrap();
    let inside = pair
        .sender
        .send(SendRequest {
            to: "phone".into(),
            paths: vec![file.clone()],
            destination: Some(project.to_string_lossy().into_owned()),
            policy: TransportPolicy::Auto,
        })
        .await
        .unwrap();
    wait_for(
        &pair.sender,
        &inside.transfer_id,
        state_is(FileTransferState::Completed),
    )
    .await;
    assert_eq!(std::fs::read(project.join("a.txt")).unwrap(), b"a");
    let outside = pair
        .sender
        .send(SendRequest {
            to: "phone".into(),
            paths: vec![file],
            destination: Some(pair._dir.path().to_string_lossy().into_owned()),
            policy: TransportPolicy::Auto,
        })
        .await
        .unwrap();
    let failed = wait_for(
        &pair.sender,
        &outside.transfer_id,
        state_is(FileTransferState::Failed),
    )
    .await;
    assert!(failed.error.unwrap().contains("home folder"));
}

/// `~` means the sending engine's home for sources and the receiving
/// engine's home for a destination (a remote viewer or an agent in `~`
/// can't know either absolute path).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tilde_paths_resolve_against_each_engines_home() {
    let pair = pair(0, 0);
    // The phone engine has a home; `~/notes.txt` is its file.
    std::fs::write(pair.home.join("notes.txt"), b"from home").unwrap();
    let sent = pair
        .receiver
        .send(SendRequest {
            to: "laptop".into(),
            paths: vec!["~/notes.txt".into()],
            destination: None,
            policy: TransportPolicy::Auto,
        })
        .await
        .unwrap();
    let done = wait_for(
        &pair.sender,
        &sent.transfer_id,
        state_is(FileTransferState::Completed),
    )
    .await;
    let landed = PathBuf::from(done.items[0].path.as_deref().unwrap());
    assert_eq!(std::fs::read(landed).unwrap(), b"from home");

    // A `~/…` destination is the receiving engine's home.
    let project = pair.home.join("projects/app");
    std::fs::create_dir_all(&project).unwrap();
    let file = pair._dir.path().join("b.txt");
    std::fs::write(&file, b"b").unwrap();
    let into = pair
        .sender
        .send(SendRequest {
            to: "phone".into(),
            paths: vec![file],
            destination: Some("~/projects/app".into()),
            policy: TransportPolicy::Auto,
        })
        .await
        .unwrap();
    wait_for(
        &pair.receiver,
        &into.transfer_id,
        state_is(FileTransferState::Completed),
    )
    .await;
    assert_eq!(std::fs::read(project.join("b.txt")).unwrap(), b"b");

    // Without a home, `~` is a clear error rather than a relative path.
    let error = pair
        .sender
        .send(SendRequest {
            to: "phone".into(),
            paths: vec!["~/x".into()],
            destination: None,
            policy: TransportPolicy::Auto,
        })
        .await
        .unwrap_err();
    assert!(error.to_string().contains("home folder"), "{error}");
}
