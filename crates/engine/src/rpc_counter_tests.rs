use super::*;

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn engine_counters_memory_fake_claude() {
    use std::time::Duration;
    let temp = tempfile::tempdir().unwrap();
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../harness/tests/fixtures/fake-claude.sh");
    let registry = std::sync::Arc::new(HarnessRegistry::new());
    registry.register(std::sync::Arc::new(
        zeron_harness::ClaudeHarness::new().with_executable(fixture),
    ));
    let profile = crate::EngineProfile::development(temp.path(), "test-org", "test-user");
    let core =
        crate::EngineCore::assemble_with_profile(profile, registry, HarnessId::ClaudeCode, None)
            .unwrap();
    let client = zeron_rpc::memory_client(core.rpc_service());
    let before = client
        .call(methods::ENGINE_COUNTERS, serde_json::json!({}))
        .await
        .unwrap();
    let request = zeron_proto::RunRequest {
        mcp: None,
        prompt: "scenario:happy".into(),
        harness: None,
        model: None,
        reasoning: None,
        model_options: Default::default(),
        cwd: temp.path().display().to_string(),
        sandbox: zeron_proto::SandboxLevel::ReadOnly,
        auto_approve: false,
        access: Default::default(),
        plan: false,
        attachments: vec![],
        worktree: None,
        resume: None,
        env: [(
            "CLAUDE_CONFIG_DIR".to_owned(),
            temp.path().join("claude").display().to_string(),
        )]
        .into(),
    };
    core.sessions
        .dispatch(
            "counter-chat",
            HarnessId::ClaudeCode,
            request,
            Some("message".into()),
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let after = client
                .call(methods::ENGINE_COUNTERS, serde_json::json!({}))
                .await
                .unwrap();
            if after["counters"]["ClaudeLines"].as_u64().unwrap()
                > before["counters"]["ClaudeLines"].as_u64().unwrap()
                && core
                    .sessions
                    .session_status("counter-chat")
                    .is_some_and(|s| s.status == zeron_proto::SessionStatus::Idle)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        client
            .call(
                methods::ENGINE_COUNTERS,
                serde_json::json!({"targetDeviceId":"other"})
            )
            .await
            .is_err()
    );
    assert!(!forwardable(methods::ENGINE_COUNTERS));
    core.sessions.shutdown().await;
}
