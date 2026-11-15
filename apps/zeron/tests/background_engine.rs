//! The background engine binary end to end: it serves its port at once, says
//! it posts the desktop banners, takes viewport focus, and stops cleanly when
//! asked — what the app relies on to start it, attach, and replace it.

use std::process::Command;
use std::time::{Duration, Instant};

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[test]
fn background_engine_serves_its_port_and_stops_on_request() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let mut child = Command::new(env!("CARGO_BIN_EXE_zeron-engine"))
        .env("ZERON_DATA_DIR", dir.path())
        .env("ZERON_IPC_PORT", port.to_string())
        // Development auth with no bearer: an offline runtime.
        .env("ZERON_WORKOS_CLIENT_ID", "")
        .env_remove("ZERON_EDGE_TOKEN")
        .env("ZERON_EDGE_URL", "http://127.0.0.1:1")
        .env("ZERON_DISABLE_NOTIFICATIONS", "1")
        .env("ZERON_AUTO_UPDATE", "0")
        .spawn()
        .expect("start the background engine");

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let served = runtime.block_on(async {
        let deadline = Instant::now() + Duration::from_secs(60);
        let client = loop {
            match zeron_rpc::connect_ipc(port, Some(dir.path())).await {
                Ok(client) => break client,
                Err(err) if Instant::now() >= deadline => {
                    return Err(format!("never served its port: {err}"));
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
            }
        };
        let info = client
            .call(zeron_rpc::methods::ENGINE_INFO, serde_json::json!({}))
            .await
            .map_err(|err| format!("EngineInfo: {err}"))?;
        let capabilities: Vec<&str> = info["capabilities"]
            .as_array()
            .map(|list| list.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();
        if !capabilities.contains(&"desktop-notifications-v1") {
            return Err(format!("no banner capability in {capabilities:?}"));
        }
        client
            .call(zeron_rpc::methods::ENGINE_READY, serde_json::json!({}))
            .await
            .map_err(|err| format!("EngineReady: {err}"))?;
        client
            .call(
                zeron_rpc::methods::SET_VIEWPORT_FOCUS,
                serde_json::json!({ "viewportId": "test", "focused": true }),
            )
            .await
            .map_err(|err| format!("SetViewportFocus: {err}"))?;
        client
            .call(zeron_rpc::methods::STOP_ENGINE, serde_json::json!({}))
            .await
            .map_err(|err| format!("StopEngine: {err}"))?;
        Ok(())
    });
    if let Err(message) = served {
        let _ = child.kill();
        panic!("{message}");
    }

    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("the engine didn't stop after StopEngine");
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    assert!(status.success(), "engine exited with {status}");
    // It let go of the port: the next engine can take it.
    std::net::TcpListener::bind(("127.0.0.1", port)).expect("port released");
}

#[test]
#[ignore = "spawns zeron-engine against the real graff CLI; needs install + auth + network"]
fn live_background_engine_codegraff_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    let git_status = Command::new("git")
        .args(["init", "-q"])
        .current_dir(repo.path())
        .status()
        .expect("run git init");
    assert!(git_status.success(), "git init failed: {git_status}");

    let skill_dir = repo.path().join(".harness/skills/live-check");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: live-check\ndescription: Live check\n---\nBody\n",
    )
    .unwrap();

    let port = free_port();
    let mut child = Command::new(env!("CARGO_BIN_EXE_zeron-engine"))
        .env("ZERON_DATA_DIR", dir.path())
        .env("ZERON_IPC_PORT", port.to_string())
        .env("ZERON_WORKOS_CLIENT_ID", "")
        .env_remove("ZERON_EDGE_TOKEN")
        .env("ZERON_EDGE_URL", "http://127.0.0.1:1")
        .env("ZERON_DISABLE_NOTIFICATIONS", "1")
        .env("ZERON_AUTO_UPDATE", "0")
        .spawn()
        .expect("start the background engine");

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let result = runtime.block_on(async {
        let deadline = Instant::now() + Duration::from_secs(60);
        let client = loop {
            match zeron_rpc::connect_ipc(port, Some(dir.path())).await {
                Ok(client) => break client,
                Err(err) if Instant::now() >= deadline => {
                    return Err(format!("never served its port: {err}"));
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
            }
        };

        client
            .call(zeron_rpc::methods::ENGINE_READY, serde_json::json!({}))
            .await
            .map_err(|err| format!("EngineReady: {err}"))?;

        client
            .call(
                zeron_rpc::methods::ADD_REPO,
                serde_json::json!({ "path": repo.path().display().to_string() }),
            )
            .await
            .map_err(|err| format!("AddRepo: {err}"))?;

        let device = client
            .call(zeron_rpc::methods::LOCAL_DEVICE, serde_json::json!({}))
            .await
            .map_err(|err| format!("LocalDevice: {err}"))?;
        let device_id = device["deviceId"]
            .as_str()
            .ok_or_else(|| format!("LocalDevice missing deviceId: {device}"))?;
        let space_id = "space-live-codegraff";
        client
            .call(
                zeron_rpc::methods::MUTATE,
                serde_json::json!({
                    "op": "createSpace",
                    "spaceId": space_id,
                    "deviceId": device_id,
                    "path": repo.path().display().to_string(),
                    "gitDetected": true,
                }),
            )
            .await
            .map_err(|err| format!("CreateSpace: {err}"))?;

        let harnesses = client
            .call(zeron_rpc::methods::LIST_HARNESSES, serde_json::json!({}))
            .await
            .map_err(|err| format!("ListHarnesses: {err}"))?;
        let codegraff = harnesses
            .as_array()
            .and_then(|rows| {
                rows.iter()
                    .find(|row| row["id"].as_str() == Some("codegraff"))
            })
            .ok_or_else(|| "ListHarnesses missing codegraff".to_string())?;
        if codegraff["installed"].as_bool() != Some(true) {
            return Err(format!("expected codegraff installed=true, got {codegraff}"));
        }

        let models = match client
            .call(
                zeron_rpc::methods::LIST_MODELS,
                serde_json::json!({ "harness": "codegraff", "force": true }),
            )
            .await
        {
            Err(err)
                if err.to_string().contains("context changed; retry")
                    || err.to_string().contains("credentials changed") =>
            {
                client
                    .call(
                        zeron_rpc::methods::LIST_MODELS,
                        serde_json::json!({ "harness": "codegraff", "force": true }),
                    )
                    .await
                    .map_err(|err| format!("ListModels retry: {err}"))?
            }
            other => other.map_err(|err| format!("ListModels: {err}"))?,
        };
        let model_count = models.as_array().map_or(0, Vec::len);
        if model_count == 0 {
            return Err("ListModels returned empty array for codegraff".to_string());
        }

        let commands = client
            .call(
                zeron_rpc::methods::LIST_COMMANDS,
                serde_json::json!({
                    "harness": "codegraff",
                    "spaceId": space_id,
                }),
            )
            .await
            .map_err(|err| format!("ListCommands: {err}"))?;
        let command_count = commands.as_array().map_or(0, Vec::len);
        if command_count == 0 {
            return Err("ListCommands returned empty array for codegraff".to_string());
        }

        let skills = client
            .call(
                zeron_rpc::methods::LIST_SKILLS,
                serde_json::json!({
                    "harness": "codegraff",
                    "spaceId": space_id,
                }),
            )
            .await
            .map_err(|err| format!("ListSkills: {err}"))?;
        let has_live_check = skills
            .as_array()
            .is_some_and(|rows| rows.iter().any(|row| row["name"].as_str() == Some("live-check")));
        if !has_live_check {
            return Err("ListSkills did not discover live-check under .harness/skills".to_string());
        }

        let updates = client
            .call(
                zeron_rpc::methods::CHECK_HARNESS_UPDATES,
                serde_json::json!({ "harness": "codegraff" }),
            )
            .await
            .map_err(|err| format!("CheckHarnessUpdates: {err}"))?;
        let update_row = updates
            .as_array()
            .and_then(|rows| {
                rows.iter()
                    .find(|row| row["harness"].as_str() == Some("codegraff"))
            })
            .ok_or_else(|| "CheckHarnessUpdates missing codegraff row".to_string())?;
        let installed_ver = update_row["installedVersion"].as_str().unwrap_or("");
        let latest_ver = update_row["latestVersion"].as_str().unwrap_or("");
        if installed_ver.is_empty() || latest_ver.is_empty() || !update_row["error"].is_null() {
            return Err(format!("unexpected update row: {update_row}"));
        }

        let sync_output = Command::new(env!("CARGO_BIN_EXE_zeron"))
            .arg("sync")
            .env("ZERON_DATA_DIR", dir.path())
            .env("ZERON_IPC_PORT", port.to_string())
            .output()
            .map_err(|err| format!("run zeron sync: {err}"))?;
        if !sync_output.status.success() {
            return Err(format!(
                "zeron sync failed ({}): {}",
                sync_output.status,
                String::from_utf8_lossy(&sync_output.stderr)
            ));
        }

        client
            .call(zeron_rpc::methods::STOP_ENGINE, serde_json::json!({}))
            .await
            .map_err(|err| format!("StopEngine: {err}"))?;
        Ok(())
    });
    if let Err(message) = result {
        let _ = child.kill();
        panic!("{message}");
    }

    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("the engine didn't stop after StopEngine");
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    assert!(status.success(), "engine exited with {status}");
}
