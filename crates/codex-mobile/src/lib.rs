//! In-process Codex host with a small C ABI. No CLI, localhost server, or
//! subprocess is used. Swift drains JSON-RPC events and resolves mobile tools.
mod config;
mod workspace_git;

use codex_app_server_client::{InProcessAppServerClient, InProcessServerEvent};
use codex_app_server_protocol::{ClientRequest, RequestId};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    ffi::{CStr, CString, c_char},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::sync::mpsc;

struct Host {
    commands: mpsc::Sender<Value>,
    events: Mutex<mpsc::Receiver<Value>>,
}
static HOSTS: OnceLock<Mutex<HashMap<u64, Arc<Host>>>> = OnceLock::new();
static NEXT: AtomicU64 = AtomicU64::new(1);
static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

fn hosts() -> &'static Mutex<HashMap<u64, Arc<Host>>> {
    HOSTS.get_or_init(Default::default)
}
fn runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_stack_size(8 * 1024 * 1024)
            .enable_all()
            .build()
            .expect("Codex runtime")
    })
}

async fn serve(
    options: config::Options,
    mut commands: mpsc::Receiver<Value>,
    events: mpsc::Sender<Value>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args = config::start_args(&options).await?;
    let state_db = args.state_db.clone();
    let mut client = InProcessAppServerClient::start(args).await?;
    events.send(json!({"method":"mobile/ready"})).await?;
    loop {
        tokio::select! {
            command = commands.recv() => {
                let Some(mut command) = command else { break };
                if command.get("method").is_some() {
                    let id = command["id"].clone();
                    let method = command["method"].as_str().unwrap_or_default().to_owned();
                    // Keep the public bridge scoped to native session/auth operations.
                    if !matches!(method.as_str(), "account/read" | "account/login/start" | "account/login/cancel" | "account/logout" | "model/list" | "thread/start" | "thread/resume" | "thread/read" | "thread/list" | "turn/start" | "turn/steer" | "turn/interrupt") {
                        events.send(json!({"id":id,"error":{"code":-32601,"message":"Unsupported mobile method"}})).await?;
                        continue;
                    }
                    // iOS preserves app data while changing the container's absolute path.
                    // Repair only the physical rollout reference before Codex checks it;
                    // paginated history and all logical metadata stay attached to the same id.
                    if method == "thread/resume" {
                        let repair = async {
                            if let (Some(db), Some(id)) = (state_db.as_ref(), command["params"]["threadId"].as_str()) {
                                if let Ok(thread_id) = codex_protocol::ThreadId::from_string(id) {
                                    if let Some(old) = db.find_rollout_path_by_id(thread_id, None).await? {
                                        if let Some(current) = relocated_rollout(&options.home, &old, id)? {
                                            db.replace_rollout_path_if_current(thread_id, &old, &current).await?;
                                        }
                                    }
                                }
                            }
                            Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
                        }.await;
                        if let Err(error) = repair {
                            events.send(json!({"id":id,"error":{"code":-32000,"message":format!("Could not restore saved conversation: {error}")}})).await?;
                            continue;
                        }
                    }
                    if method == "thread/start" || method == "thread/resume" {
                        let params = command["params"].as_object_mut().ok_or("Expected thread parameters")?;
                        params.insert("cwd".into(), json!(options.home.join("context")));
                        params.insert("developerInstructions".into(), json!(config::INSTRUCTIONS));
                        params.insert("approvalPolicy".into(), json!("never"));
                        // Also override restored threads that were created with the old read-only default.
                        params.insert("sandbox".into(), json!("workspace-write"));
                        params.insert("runtimeWorkspaceRoots".into(), json!(["/workspace"]));
                        if method == "thread/start" { params.insert("dynamicTools".into(), config::tools()); }
                    }
                    match serde_json::from_value::<ClientRequest>(command) {
                        Ok(request) => {
                            let handle = client.request_handle();
                            let events = events.clone();
                            // A pending login/request must not prevent interruption or tool replies.
                            tokio::spawn(async move {
                                let response = match handle.request(request).await {
                                    Ok(Ok(result)) => json!({"id":id,"result":result}),
                                    Ok(Err(error)) => json!({"id":id,"error":error}),
                                    Err(error) => json!({"id":id,"error":{"code":-32000,"message":error.to_string()}}),
                                };
                                let _ = events.send(response).await;
                            });
                        }
                        Err(error) => { events.send(json!({"id":id,"error":{"code":-32602,"message":error.to_string()}})).await?; }
                    }
                } else if let Some(id) = command.get("id") {
                    let id: RequestId = serde_json::from_value(id.clone())?;
                    if let Some(result) = command.get("result") { let _ = client.resolve_server_request(id, result.clone()).await; }
                    else if let Some(error) = command.get("error") { let _ = client.reject_server_request(id, serde_json::from_value(error.clone())?).await; }
                }
            }
            event = client.next_event() => {
                let Some(event) = event else { break };
                let value = match event {
                    InProcessServerEvent::ServerNotification(event) => serde_json::to_value(event)?,
                    InProcessServerEvent::ServerRequest(event) => serde_json::to_value(event)?,
                    InProcessServerEvent::Lagged { skipped } => return Err(format!("Codex event stream lost {skipped} events").into()),
                };
                if events.send(value).await.is_err() { break; }
            }
        }
    }
    client.shutdown().await?;
    Ok(())
}

/// Rebase a missing rollout onto this installation's private Codex home.
/// Never redirect an existing path, accept traversal, or follow links outside home.
fn relocated_rollout(home: &std::path::Path, old: &std::path::Path, id: &str) -> std::io::Result<Option<std::path::PathBuf>> {
    use std::path::Component;
    // An old iOS container may be inaccessible as well as absent. Only probe it;
    // validate any replacement against the current private home below.
    if old.exists() { return Ok(None); }
    let components: Vec<_> = old.components().collect();
    let Some(start) = components.iter().rposition(|part| matches!(part, Component::Normal(name) if *name == "sessions" || *name == "archived_sessions")) else { return Ok(None) };
    if components[start..].iter().any(|part| !matches!(part, Component::Normal(_))) { return Ok(None); }
    let Some(name) = old.file_name().and_then(|name| name.to_str()) else { return Ok(None) };
    if !name.starts_with("rollout-") || !name.ends_with(&format!("-{id}.jsonl")) { return Ok(None); }
    let root = home.canonicalize()?;
    let candidate = components[start..].iter().fold(root.clone(), |path, part| path.join(part.as_os_str()));
    if !candidate.try_exists()? { return Ok(None); }
    let candidate = candidate.canonicalize()?;
    if candidate.starts_with(&root) && candidate.is_file() { Ok(Some(candidate)) } else { Ok(None) }
}

/// Starts an isolated host. Returns zero for malformed options. The caller owns
/// UTF-8 input for the duration of this call; no pointers are retained.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn zeron_codex_open(options: *const c_char) -> u64 {
    std::panic::catch_unwind(|| {
        if options.is_null() {
            return 0;
        }
        let Ok(options) = serde_json::from_slice::<config::Options>(
            unsafe { CStr::from_ptr(options) }.to_bytes(),
        ) else {
            return 0;
        };
        let (commands, receiver) = mpsc::channel(128);
        let (events, output) = mpsc::channel(4096);
        let host = Arc::new(Host {
            commands,
            events: Mutex::new(output),
        });
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        hosts().lock().unwrap().insert(id, host);
        runtime().spawn(async move {
            if let Err(error) = serve(options, receiver, events.clone()).await {
                let _ = events
                    .send(json!({"method":"mobile/error","params":{"message":error.to_string()}}))
                    .await;
            }
        });
        id
    })
    .unwrap_or(0)
}

/// Enqueue a JSON request or server-request reply. Returns 0 on acceptance.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn zeron_codex_send(id: u64, message: *const c_char) -> i32 {
    std::panic::catch_unwind(|| {
        if message.is_null() {
            return -1;
        }
        let bytes = unsafe { CStr::from_ptr(message) }.to_bytes();
        if bytes.len() > 16 * 1024 * 1024 {
            return -1;
        }
        let Ok(value) = serde_json::from_slice(bytes) else {
            return -1;
        };
        let Some(host) = hosts().lock().unwrap().get(&id).cloned() else {
            return -1;
        };
        if host.commands.try_send(value).is_ok() {
            0
        } else {
            -1
        }
    })
    .unwrap_or(-1)
}

/// Nonblocking drain of up to 128 events as a JSON array. Free the returned
/// allocation exactly once with zeron_codex_free, on any thread.
#[unsafe(no_mangle)]
pub extern "C" fn zeron_codex_poll(id: u64) -> *mut c_char {
    std::panic::catch_unwind(|| {
        let Some(host) = hosts().lock().unwrap().get(&id).cloned() else {
            return std::ptr::null_mut();
        };
        let mut receiver = host.events.lock().unwrap();
        let mut events = Vec::new();
        for _ in 0..128 {
            match receiver.try_recv() {
                Ok(event) => events.push(event),
                Err(_) => break,
            }
        }
        CString::new(serde_json::to_string(&events).unwrap())
            .unwrap()
            .into_raw()
    })
    .unwrap_or(std::ptr::null_mut())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn zeron_codex_free(value: *mut c_char) {
    if !value.is_null() {
        drop(unsafe { CString::from_raw(value) });
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn zeron_codex_close(id: u64) {
    if let Ok(mut hosts) = hosts().lock() {
        hosts.remove(&id);
    }
}
