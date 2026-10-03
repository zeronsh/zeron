//! Real native process startup on every platform, with no installed CLI or login.
#![cfg(feature = "native-fixture")]
use futures::StreamExt;
use serde_json::{Value, json};
use std::{path::Path, time::Duration};
use zeron_harness::{CancellationToken, Harness, OpencodeHarness, RunControls};
use zeron_proto::{AgentEvent, DoneStatus, RunRequest, SandboxLevel};

#[test]
fn native_startup_isolated() {
    if std::env::var_os("ZERON_OPENCODE_NATIVE_CASE").is_some() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(worker());
        return;
    }
    let mut cases = vec![
        "v1",
        "v2",
        "v2-slow",
        "v2-unknown",
        "v2-disabled-mcp",
        "v2-crash",
        "invalid-override",
    ];
    if cfg!(windows) {
        cases.push("v2-cmd");
    }
    for case in cases {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("O'Brien 日本語 &! profile");
        std::fs::create_dir_all(&root).unwrap();
        let exe = root.join(format!("opencode-{case}{}", std::env::consts::EXE_SUFFIX));
        std::fs::copy(env!("CARGO_BIN_EXE_harness-opencode-fixture"), &exe).unwrap();
        let launcher = if case == "v2-cmd" {
            let shim = root.join("opencode.cmd");
            std::fs::write(
                &shim,
                format!(
                    "@echo off\r\n\"%~dp0{}\" %*\r\n",
                    exe.file_name().unwrap().to_string_lossy()
                ),
            )
            .unwrap();
            shim
        } else {
            exe
        };
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "native_startup_isolated", "--nocapture"])
            .env("ZERON_OPENCODE_NATIVE_CASE", case)
            .env("OPENCODE_EXECUTABLE", &launcher)
            .env("USERPROFILE", &root)
            .env("HOME", root.join("git-home"))
            .env("XDG_DATA_HOME", root.join("data"))
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("OPENCODE_SERVER_USERNAME", "inherited-wrong-user")
            .env("OPENCODE_PASSWORD", "inherited-wrong-password")
            .env("OPENCODE_SERVER_PASSWORD", "inherited-wrong-password")
            .env(
                "OPENCODE_CONFIG_CONTENT",
                if case == "v2-disabled-mcp" {
                    r#"{"mcp":false}"#
                } else {
                    "{}"
                },
            )
            .env("ZERON_OPENCODE_STARTUP_TIMEOUT_SECS", "8")
            .env_remove("OPENCODE_CONFIG")
            .env_remove("OPENCODE_CONFIG_DIR");
        if !cfg!(windows) {
            command.env("HOME", &root);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{case}: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

async fn worker() {
    let case = std::env::var("ZERON_OPENCODE_NATIVE_CASE").unwrap();
    if case == "invalid-override" {
        std::fs::remove_file(std::env::var_os("OPENCODE_EXECUTABLE").unwrap()).unwrap();
        let error = OpencodeHarness::new().models().await.unwrap_err();
        assert!(error.to_string().contains("does not exist"), "{error}");
        return;
    }
    let harness = OpencodeHarness::new();
    if case == "v2-crash" {
        let error = harness.models().await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("fixture plugin initialization failed"),
            "{error}"
        );
        return;
    }
    if case == "v1" || case == "v2" {
        let models = harness.models().await.unwrap();
        assert_eq!(models[0].id, "test/model");
    } else {
        let root = zeron_harness::opencode::paths::Paths::detect().home;
        let (_tx, steering) = tokio::sync::mpsc::channel(1);
        let controls = RunControls {
            execution_lease: None,
            steering,
            interrupt: CancellationToken::new(),
            request_input: Box::new(|_| tokio::sync::oneshot::channel().1),
        };
        let request = RunRequest {
            cwd: root.to_string_lossy().into_owned(),
            model: Some("missing/model".into()),
            mcp: Some(zeron_proto::McpServer {
                name: "zeron".into(),
                command: "fixture-mcp".into(),
                args: vec![],
                env: Default::default(),
            }),
            prompt: "hello".into(),
            harness: None,
            reasoning: None,
            model_options: Default::default(),
            sandbox: SandboxLevel::WorkspaceWrite,
            auto_approve: true,
            attachments: vec![],
            resume: None,
            worktree: None,
        };
        let mut events = harness.run(request, controls).await.unwrap();
        let error = tokio::time::timeout(Duration::from_secs(15), async {
            while let Some(event) = events.next().await {
                if let AgentEvent::Done {
                    status: DoneStatus::Errored,
                    error: Some(error),
                    ..
                } = event.unwrap()
                {
                    return error;
                }
            }
            panic!("missing preflight error");
        })
        .await
        .unwrap();
        assert!(error.contains("unavailable in this project"), "{error}");
    }
    let home = zeron_harness::opencode::paths::Paths::detect().home;
    let record: Value =
        serde_json::from_slice(&std::fs::read(home.join("fixture-launch.json")).unwrap()).unwrap();
    assert_eq!(Path::new(record["cwd"].as_str().unwrap()), home);
    assert_eq!(record["username"], "opencode");
    assert_eq!(record["passwordsMatch"], true);
    let config: Value = serde_json::from_str(record["config"].as_str().unwrap()).unwrap();
    if case == "v2-slow" {
        assert!(config["mcp"]["servers"]["zeron"].is_object());
    }
    if case == "v2-unknown" {
        assert_eq!(config, json!({}));
    }
    if case == "v2-disabled-mcp" {
        assert_eq!(config, json!({"mcp":false}));
    }
}
