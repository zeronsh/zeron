//! Throwaway smoke server for manual browser verification of the web client
//! path: the real remote listener in dev-auth mode, printed as its URL.
//! Seeds one chat with unseen activity so the chat list has a row to show.
use std::sync::Arc;

use zeron_engine::{
    EngineCore, EngineProfile, HarnessId, HarnessRegistry, remote_auth::RemoteAuthorizer,
    serve_engine_remote,
};
use zeron_rpc::RpcService;

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    let dir = tempfile::tempdir().unwrap();
    let core = EngineCore::assemble_with_profile(
        EngineProfile::local(dir.path()).unwrap(),
        Arc::new(HarnessRegistry::new()),
        HarnessId::Mock,
        None,
    )
    .unwrap();
    let remote = serve_engine_remote(
        "127.0.0.1:27699".parse().unwrap(),
        core.rpc_service(),
        Arc::new(RemoteAuthorizer::new(None, None)),
    )
    .await
    .unwrap();
    seed_smoke_chat(core.rpc_service(), dir.path()).await;
    println!("SMOKE READY http://127.0.0.1:27699");
    std::future::pending::<()>().await;
    #[allow(unreachable_code)]
    {
        drop(remote);
        core.shutdown().await;
    }
}

async fn seed_smoke_chat(service: Arc<dyn RpcService>, cwd: &std::path::Path) {
    let device = match service
        .handle(zeron_rpc::methods::LOCAL_DEVICE, serde_json::json!({}))
        .await
    {
        Ok(zeron_rpc::RpcReply::Value(value)) => value["deviceId"].as_str().unwrap().to_string(),
        Ok(_) => panic!("LocalDevice returned a stream"),
        Err(error) => panic!("LocalDevice failed: {error}"),
    };
    for params in [
        serde_json::json!({"op": "createChat", "chatId": "smoke-chat", "deviceId": device}),
        serde_json::json!({"op": "renameChat", "chatId": "smoke-chat", "title": "Browser smoke chat"}),
        // A working directory is what makes the chat runnable: without one the
        // composer refuses to send, so the harness never replies and the
        // transcript stays empty — no use as a visual-parity harness.
        serde_json::json!({"op": "setChatCwd", "chatId": "smoke-chat", "cwd": cwd.to_string_lossy()}),
        serde_json::json!({"op": "setChatActivity", "chatId": "smoke-chat", "lastMessageAt": now_ms() - 5 * 60_000, "createdAt": now_ms() - 3_600_000}),
    ] {
        service
            .handle(zeron_rpc::methods::MUTATE, params)
            .await
            .expect("seed mutate");
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}
