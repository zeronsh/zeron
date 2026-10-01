//! Device-to-device file transfer end to end (docs/file-transfer.md): two
//! real engine runtimes on one local edge. The edge carries presence, the
//! WebRTC signaling and the DeviceRoom relay; the bytes go P2P over a
//! DataChannel, or over the relay when forced. Each run sends a folder and
//! a large generated file (1 GiB by default; `ZERON_TRANSFER_TEST_MIB`
//! overrides), cuts the tunnel mid-file, and checks the resumed result
//! byte for byte.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sha2::Digest;
use zeron_engine::{Engine, EngineConfig, EngineRuntime, HarnessId};
use zeron_localedge::{LocalEdge, LocalEdgeConfig};
use zeron_proto::{FileTransfer, FileTransferState, FileTransferTransport, capabilities};
use zeron_rpc::{RpcClient, methods};

const TOKEN: &str = "xfer-0123456789abcdef0123456789abcdef";

async fn start_engine(dir: &Path, edge_url: &str) -> EngineRuntime {
    let config = EngineConfig {
        data_dir: dir.to_path_buf(),
        edge_url: String::new(),
        edge_token: None,
        ipc_port: 0,
        default_harness: HarnessId::Mock,
        org_id: None,
        workos_client_id: None,
        dev_user_id: None,
    }
    .with_local_edge(edge_url, TOKEN);
    let auth = Engine::build_auth(&config).await;
    let scope = Engine::initial_workspace_scope(&auth);
    let profile = Engine::resolve_profile(&config, &auth, scope)
        .unwrap()
        .expect("development profile");
    Engine::assemble_runtime(&config, auth, profile)
        .await
        .expect("engine runtime assembles")
}

struct Device {
    runtime: EngineRuntime,
    rpc: RpcClient,
    id: String,
    inbox: PathBuf,
}

async fn device(root: &Path, name: &str, edge_url: &str) -> Device {
    let dir = root.join(name);
    let runtime = start_engine(&dir, edge_url).await;
    let rpc = zeron_rpc::memory_client(runtime.core().rpc_service());
    let id = runtime.core().device_id.clone();
    // Never the real ~/Zeron Transfers.
    let inbox = dir.join("inbox");
    rpc.call(
        methods::SET_FILE_TRANSFER_SETTINGS,
        serde_json::json!({ "inboxDir": inbox }),
    )
    .await
    .unwrap();
    Device {
        runtime,
        rpc,
        id,
        inbox,
    }
}

async fn wait_until(what: &str, timeout: Duration, mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(start.elapsed() < timeout, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn transfers(device: &Device) -> Vec<FileTransfer> {
    serde_json::from_value(
        device
            .rpc
            .call(methods::LIST_FILE_TRANSFERS, serde_json::json!({}))
            .await
            .unwrap(),
    )
    .unwrap()
}

async fn wait_row(
    device: &Device,
    id: &str,
    timeout: Duration,
    mut what: impl FnMut(&FileTransfer) -> bool,
) -> FileTransfer {
    let start = Instant::now();
    loop {
        let rows = transfers(device).await;
        if let Some(row) = rows.iter().find(|r| r.id == id) {
            if what(row) {
                return row.clone();
            }
            assert!(
                !(row.state.is_terminal() && row.state != FileTransferState::Completed),
                "transfer ended: {row:#?}"
            );
        }
        assert!(start.elapsed() < timeout, "timed out; rows: {rows:#?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Position-dependent bytes, generated and hashed in 1 MiB pieces.
fn generate(path: &Path, size: u64) -> String {
    use std::io::Write;
    let mut file = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    let mut hasher = sha2::Sha256::new();
    let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut chunk = vec![0u8; 1024 * 1024];
    let mut left = size;
    while left > 0 {
        let n = left.min(chunk.len() as u64) as usize;
        for word in chunk[..n].chunks_mut(8) {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let bytes = x.to_le_bytes();
            word.copy_from_slice(&bytes[..word.len()]);
        }
        file.write_all(&chunk[..n]).unwrap();
        hasher.update(&chunk[..n]);
        left -= n as u64;
    }
    file.flush().unwrap();
    hex(&hasher.finalize())
}

fn sha256_file(path: &Path) -> String {
    let mut hasher = sha2::Sha256::new();
    let mut file = std::fs::File::open(path).unwrap();
    std::io::copy(&mut file, &mut hasher).unwrap();
    hex(&hasher.finalize())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn make_folder(root: &Path) -> PathBuf {
    let folder = root.join("build-output");
    std::fs::create_dir_all(folder.join("apk/debug")).unwrap();
    std::fs::create_dir_all(folder.join("logs")).unwrap();
    std::fs::write(folder.join("apk/debug/output-metadata.json"), b"{\"v\":1}").unwrap();
    std::fs::write(folder.join("logs/build.log"), "line\n".repeat(50_000)).unwrap();
    std::fs::write(folder.join("empty"), b"").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("apk/debug", folder.join("latest")).unwrap();
    folder
}

fn same_tree(a: &Path, b: &Path) {
    for entry in std::fs::read_dir(a).unwrap() {
        let entry = entry.unwrap();
        let other = b.join(entry.file_name());
        let meta = std::fs::symlink_metadata(entry.path()).unwrap();
        if meta.file_type().is_symlink() {
            assert_eq!(
                std::fs::read_link(entry.path()).unwrap(),
                std::fs::read_link(&other).unwrap()
            );
        } else if meta.is_dir() {
            same_tree(&entry.path(), &other);
        } else {
            assert_eq!(
                std::fs::read(entry.path()).unwrap(),
                std::fs::read(&other).unwrap()
            );
        }
    }
}

async fn send(from: &Device, to: &Device, paths: &[&Path], transport: &str) -> String {
    let reply = from
        .rpc
        .call(
            methods::SEND_FILES,
            serde_json::json!({
                "toDeviceId": to.id,
                "paths": paths,
                "transport": transport,
            }),
        )
        .await
        .unwrap();
    reply["transferId"].as_str().unwrap().to_owned()
}

/// Send a folder and the large file from `from` to `to`, cut the tunnel
/// once the large file is partway, and verify the resumed result.
/// What every round sends.
struct Payload {
    folder: PathBuf,
    big: PathBuf,
    big_sha: String,
}

async fn round(
    from: &Device,
    to: &Device,
    payload: &Payload,
    transport: &str,
    expect: FileTransferTransport,
    cut: bool,
) {
    let Payload {
        folder,
        big,
        big_sha,
    } = payload;
    let size = std::fs::metadata(big).unwrap().len();
    let started = Instant::now();
    let id = send(from, to, &[folder.as_path(), big.as_path()], transport).await;
    let first = wait_row(to, &id, Duration::from_secs(120), |r| {
        r.state == FileTransferState::Transferring && r.done_bytes > size / 4
    })
    .await;
    assert_eq!(first.transport, Some(expect));
    let cut_at = first.done_bytes;
    if cut {
        // Cut the tunnel mid-file: the sender redials and resumes.
        match expect {
            FileTransferTransport::P2p => from.runtime.core().previews.drop_peer(&to.id).await,
            FileTransferTransport::Relay => from.runtime.core().links().unwrap().invalidate(&to.id),
        }
        wait_row(from, &id, Duration::from_secs(60), |r| {
            r.state == FileTransferState::Reconnecting
        })
        .await;
    }
    let done = wait_row(to, &id, Duration::from_secs(1800), |r| {
        r.state == FileTransferState::Completed
    })
    .await;
    let elapsed = started.elapsed();
    let sent = wait_row(from, &id, Duration::from_secs(60), |r| {
        r.state == FileTransferState::Completed
    })
    .await;
    assert_eq!(sent.done_bytes, sent.total_bytes);
    assert_eq!(done.transport, Some(expect));
    let dest = PathBuf::from(done.destination.unwrap());
    assert!(
        dest.starts_with(&to.inbox),
        "{} lands in the inbox",
        dest.display()
    );
    same_tree(
        folder,
        &PathBuf::from(done.items[0].path.as_deref().unwrap()),
    );
    let landed = PathBuf::from(done.items[1].path.as_deref().unwrap());
    assert_eq!(&sha256_file(&landed), big_sha, "large file identical");
    eprintln!(
        "{transport}: {} MiB + folder in {:.1}s ({:.1} MiB/s overall{})",
        size >> 20,
        elapsed.as_secs_f64(),
        (done.total_bytes as f64 / (1 << 20) as f64) / elapsed.as_secs_f64(),
        if cut {
            format!(", cut at {} MiB and resumed", cut_at >> 20)
        } else {
            String::new()
        },
    );
    std::fs::remove_file(&landed).unwrap();
}

#[test]
fn engines_transfer_folders_and_large_files_p2p_and_over_the_relay() {
    if std::env::var_os("RUST_LOG").is_some() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_test_writer()
            .try_init();
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(8)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(scenario());
}

async fn scenario() {
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let edge = LocalEdge::start(LocalEdgeConfig::loopback(dir.path().join("edge"), 0, TOKEN))
        .await
        .unwrap();
    let laptop = device(dir.path(), "laptop", &edge.url()).await;
    let phone = device(dir.path(), "phone", &edge.url()).await;
    // Both engines see each other (registry presence + capability).
    for (a, b) in [(&laptop, &phone), (&phone, &laptop)] {
        let workspace = a.runtime.core().workspace.clone();
        let other = b.id.clone();
        wait_until("peer presence", Duration::from_secs(60), || {
            workspace
                .read_devices()
                .unwrap_or_default()
                .iter()
                .any(|d| d.id == other && d.supports(capabilities::FILE_TRANSFER_V1))
                && workspace.peer_liveness(&other) == zeron_rpc::PeerLiveness::Live
        })
        .await;
    }

    let mib: u64 = std::env::var("ZERON_TRANSFER_TEST_MIB")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1024);
    let source = dir.path().join("source");
    std::fs::create_dir_all(&source).unwrap();
    let folder = make_folder(&source);
    let big = source.join("large.bin");
    let big_sha = generate(&big, mib * 1024 * 1024 + 12345);
    let payload = Payload {
        folder,
        big,
        big_sha,
    };

    let rounds = std::env::var("ZERON_TRANSFER_TEST_ROUNDS").unwrap_or_else(|_| "p2p,relay".into());
    // Each transport: one clean run (throughput), one cut mid-file.
    let throughput = std::env::var_os("ZERON_TRANSFER_TEST_THROUGHPUT").is_some();
    if rounds.contains("p2p") {
        if throughput {
            round(
                &laptop,
                &phone,
                &payload,
                "p2p",
                FileTransferTransport::P2p,
                false,
            )
            .await;
        }
        round(
            &laptop,
            &phone,
            &payload,
            "p2p",
            FileTransferTransport::P2p,
            true,
        )
        .await;
    }
    if rounds.contains("relay") {
        if throughput {
            round(
                &phone,
                &laptop,
                &payload,
                "relay",
                FileTransferTransport::Relay,
                false,
            )
            .await;
        }
        round(
            &phone,
            &laptop,
            &payload,
            "relay",
            FileTransferTransport::Relay,
            true,
        )
        .await;
    }

    // An agent's "send it to me" resolves nothing without a typed message.
    let error = laptop
        .rpc
        .call(
            methods::SEND_FILES,
            serde_json::json!({ "chatId": "no-such-chat", "paths": [&payload.big] }),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("say which device"), "{error}");

    laptop.runtime.shutdown().await;
    phone.runtime.shutdown().await;
    edge.shutdown().await;
}
