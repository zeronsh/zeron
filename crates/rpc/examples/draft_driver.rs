//! Two-device synced-draft e2e driver (`scripts/e2e-draft.sh` runs it).
//!
//! Connects to two running headless engines (same user, different devices, real edge) over
//! their localhost IPC ports, each holding a `DraftDoc` replica like a composer window, and
//! proves the draft plane end to end:
//!
//! 1. both `WatchDraft` streams open with an empty `reset` snapshot;
//! 2. typing on A shows up live on B;
//! 3. concurrent typing on A and B merges: both replicas converge and keep both edits;
//! 4. `ClearDraft` (send) discards the draft on both devices and bumps the epoch;
//! 5. typing on B after the discard reaches A (the new epoch works and the old text is gone).
//!
//! Prints `PASS`/`FAIL` lines; exits nonzero on failure.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use zeron_doc::DraftDoc;
use zeron_proto::DraftFrame;
use zeron_rpc::{RpcClient, connect_ws, methods};

const STEP_TIMEOUT: Duration = Duration::from_secs(45);
const CHAT: &str = "e2e-draft-chat";

fn fail(message: &str) -> ! {
    eprintln!("FAIL: {message}");
    std::process::exit(1);
}

fn pass(message: &str) {
    println!("PASS: {message}");
}

/// One composer-window stand-in: an engine connection plus a replica fed by `WatchDraft`.
struct Window {
    name: &'static str,
    client: RpcClient,
    doc: Arc<Mutex<DraftDoc>>,
    epoch: Arc<AtomicU64>,
    resets: Arc<AtomicU64>,
}

impl Window {
    async fn open(name: &'static str, port: u16) -> Self {
        let client = connect_ws(&format!("ws://127.0.0.1:{port}"))
            .await
            .unwrap_or_else(|err| fail(&format!("{name}: connect ipc :{port}: {err}")));
        let doc = Arc::new(Mutex::new(DraftDoc::new()));
        let epoch = Arc::new(AtomicU64::new(0));
        let resets = Arc::new(AtomicU64::new(0));
        let mut rx = client
            .subscribe(methods::WATCH_DRAFT, serde_json::json!({ "chatId": CHAT }))
            .await
            .unwrap_or_else(|err| fail(&format!("{name}: WatchDraft: {err}")));
        let (task_doc, task_epoch, task_resets) = (doc.clone(), epoch.clone(), resets.clone());
        tokio::spawn(async move {
            while let Some(item) = rx.recv().await {
                let Ok(frame) = serde_json::from_value::<DraftFrame>(item) else {
                    continue;
                };
                let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(&frame.update)
                else {
                    continue;
                };
                if frame.reset {
                    let Ok(fresh) = DraftDoc::from_snapshot(&bytes) else {
                        continue;
                    };
                    *task_doc.lock().unwrap() = fresh;
                    task_resets.fetch_add(1, Ordering::SeqCst);
                } else {
                    let _ = task_doc.lock().unwrap().import(&bytes);
                }
                task_epoch.store(frame.epoch, Ordering::SeqCst);
            }
        });
        Self {
            name,
            client,
            doc,
            epoch,
            resets,
        }
    }

    fn text(&self) -> String {
        self.doc.lock().unwrap().text()
    }

    /// Type into the local replica and push the resulting update, like the composer does.
    async fn type_text(&self, new_text: &str) {
        let update = {
            let doc = self.doc.lock().unwrap();
            let before = doc.version();
            doc.set_text(new_text).expect("set_text");
            doc.export_since(&before).expect("export")
        };
        let update = base64::engine::general_purpose::STANDARD.encode(update);
        self.client
            .call(
                methods::EDIT_DRAFT,
                serde_json::json!({ "chatId": CHAT, "update": update }),
            )
            .await
            .unwrap_or_else(|err| fail(&format!("{}: EditDraft: {err}", self.name)));
    }

    async fn clear(&self) {
        self.client
            .call(methods::CLEAR_DRAFT, serde_json::json!({ "chatId": CHAT }))
            .await
            .unwrap_or_else(|err| fail(&format!("{}: ClearDraft: {err}", self.name)));
        *self.doc.lock().unwrap() = DraftDoc::new();
    }
}

async fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + STEP_TIMEOUT;
    while !condition() {
        if Instant::now() >= deadline {
            fail(&format!(
                "{what}: timed out after {}s",
                STEP_TIMEOUT.as_secs()
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let a_port: u16 = args
        .next()
        .unwrap_or_else(|| "27801".into())
        .parse()
        .expect("A port");
    let b_port: u16 = args
        .next()
        .unwrap_or_else(|| "27802".into())
        .parse()
        .expect("B port");

    let a = Window::open("A", a_port).await;
    let b = Window::open("B", b_port).await;
    wait_until("initial snapshots", || {
        a.resets.load(Ordering::SeqCst) >= 1 && b.resets.load(Ordering::SeqCst) >= 1
    })
    .await;
    if !a.text().is_empty() || !b.text().is_empty() {
        fail("a fresh draft should start empty on both devices");
    }
    pass("both devices opened an empty draft");

    // 2. A types, B sees it live.
    a.type_text("Refactor the parser").await;
    wait_until("A's typing on B", || b.text() == "Refactor the parser").await;
    pass("typing on A appeared live on B");

    // 3. Concurrent typing merges, nothing is lost.
    a.type_text("Refactor the parser carefully").await;
    b.type_text("Please: Refactor the parser").await;
    wait_until("concurrent edits to converge", || {
        let (x, y) = (a.text(), b.text());
        x == y && x.contains("carefully") && x.contains("Please: ")
    })
    .await;
    pass(&format!(
        "concurrent typing merged on both devices: {:?}",
        a.text()
    ));

    // 4. Send on A: discard everywhere.
    let epoch_before = a
        .epoch
        .load(Ordering::SeqCst)
        .max(b.epoch.load(Ordering::SeqCst));
    let resets_b = b.resets.load(Ordering::SeqCst);
    a.clear().await;
    wait_until("discard to reach B", || {
        b.resets.load(Ordering::SeqCst) > resets_b && b.text().is_empty()
    })
    .await;
    wait_until("epoch bump on B", || {
        b.epoch.load(Ordering::SeqCst) > epoch_before
    })
    .await;
    pass("ClearDraft on A emptied the draft on B and bumped the epoch");

    // 5. The new epoch works and the old text does not come back.
    b.type_text("next prompt").await;
    wait_until("post-discard typing on A", || a.text() == "next prompt").await;
    pass("typing after the discard synced under the new epoch, old text stayed gone");

    println!("PASS: two-device draft e2e complete");
}
