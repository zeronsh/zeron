//! Conformance engine for the `@zeron/engine-client` test suite: the real
//! remote listener on an ephemeral port in dev-auth mode, announced on
//! stdout as one `CONFORMANCE {json}` line and kept alive until the test
//! teardown kills the process.
use std::sync::Arc;

use zeron_engine::{
    EngineCore, EngineProfile, HarnessId, HarnessRegistry, remote_auth::RemoteAuthorizer,
    serve_engine_remote,
};

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
        "127.0.0.1:0".parse().unwrap(),
        core.rpc_service(),
        Arc::new(RemoteAuthorizer::new(None, None)),
    )
    .await
    .unwrap();
    println!(
        "CONFORMANCE {}",
        serde_json::json!({
            "endpoint": format!("http://{}", remote.address),
            "deviceId": core.device_id,
        })
    );
    std::future::pending::<()>().await;
    #[allow(unreachable_code)]
    {
        drop(remote);
        core.shutdown().await;
    }
}
