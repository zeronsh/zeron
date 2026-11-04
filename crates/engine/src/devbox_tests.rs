use super::*;

const BOX: &str = "bx_23456789";

// Failure modes: a snapshotted/forked identity can control the wrong VM, an
// unknown provider state is treated as recoverable, or a peer's readiness is
// accepted without proving the registered execution identity.
#[test]
fn host_binding_requires_current_native_identity() {
    assert_eq!(
        resolve_host_binding(true, false, Some(BOX), Some("boat"), Some(BOX)),
        Ok(Some((DeviceEnvironment::Boat, BOX.into())))
    );
    assert_eq!(
        resolve_host_binding(true, true, None, Some("namespace"), Some("abcdefgh23456")),
        Ok(Some((DeviceEnvironment::Namespace, "abcdefgh23456".into())))
    );
    assert_eq!(
        resolve_host_binding(false, false, None, None, None),
        Ok(None)
    );
    for result in [
        resolve_host_binding(true, false, Some(BOX), None, None),
        resolve_host_binding(true, false, None, Some("boat"), Some(BOX)),
        resolve_host_binding(false, false, Some(BOX), Some("boat"), Some(BOX)),
        resolve_host_binding(true, false, Some("bx_abcdefgh"), Some("boat"), Some(BOX)),
        resolve_host_binding(true, false, Some(BOX), Some("boat"), Some("../../other")),
        resolve_host_binding(true, false, None, None, Some(BOX)),
        resolve_host_binding(true, true, None, None, None),
        resolve_host_binding(true, false, None, Some("namespace"), Some("abcdefgh23456")),
        resolve_host_binding(
            true,
            true,
            Some(BOX),
            Some("namespace"),
            Some("abcdefgh23456"),
        ),
        resolve_host_binding(true, true, Some(BOX), Some("boat"), Some(BOX)),
    ] {
        assert!(result.is_err());
    }
}

#[test]
fn lifecycle_selection_never_recovers_or_replaces_a_machine() {
    for state in ["archived", "stopped"] {
        let bytes =
            serde_json::to_vec(&serde_json::json!({"sandbox":{"id":BOX,"state":state}})).unwrap();
        assert_eq!(wake_action(&bytes, BOX), Ok(WakeAction::Resume));
    }
    for state in [
        "ready",
        "running",
        "idle",
        "provisioning",
        "provisioned",
        "restoring",
        "cloning",
    ] {
        let bytes =
            serde_json::to_vec(&serde_json::json!({"sandbox":{"id":BOX,"state":state}})).unwrap();
        assert_eq!(wake_action(&bytes, BOX), Ok(WakeAction::Connect));
    }
    for value in [
        serde_json::json!({"sandbox":{"id":"bx_abcdefgh","state":"archived"}}),
        serde_json::json!({"sandbox":{"id":BOX,"state":"archiving"}}),
        serde_json::json!({"sandbox":{"id":BOX,"state":"error"}}),
        serde_json::json!({"sandbox":{"id":BOX,"state":"unknown"}}),
        serde_json::json!({"ok":false,"sandbox":{"id":BOX,"state":"ready"}}),
    ] {
        assert!(wake_action(&serde_json::to_vec(&value).unwrap(), BOX).is_err());
    }
    assert!(wake_action(b"not JSON", BOX).is_err());
}

#[test]
fn ready_peer_must_match_exact_provider_device_and_synced_scope() {
    for (provider, id) in [
        (DeviceEnvironment::Boat, BOX),
        (DeviceEnvironment::Namespace, "abcdefgh23456"),
    ] {
        let ready = serde_json::json!({"deviceId":"peer", "devboxProvider":provider, "devboxId":id,
            "workspaceScope":"synced", "capabilities":["native"]});
        assert!(verify_peer(&ready, "peer", provider, id).is_ok());
        for (field, value) in [
            ("deviceId", serde_json::json!("other")),
            ("devboxProvider", serde_json::Value::Null),
            ("devboxId", serde_json::json!("other")),
            ("workspaceScope", serde_json::json!("local")),
            ("capabilities", serde_json::json!([42])),
        ] {
            let mut wrong = ready.clone();
            wrong[field] = value;
            assert!(verify_peer(&wrong, "peer", provider, id).is_err());
        }
    }
}

// Failure modes: only a staging controller can wake, so Crew and Crew Canary
// controllers are refused; or a controller in a deployment the devbox skill
// does not configure starts another deployment's engine on the machine.
#[test]
fn controller_deployment_selects_the_host_channel() {
    let scope = |project: &str, deployment: Option<&str>| -> zeron_proto::CollaborationScope {
        serde_json::from_value(serde_json::json!({"projectId": project, "deploymentId": deployment})).unwrap()
    };
    for (project, deployment, channel) in [
        ("ashler-production", None, HostChannel::Production),
        // Production leaves its deployment to device bootstrap.
        ("ashler-production", Some("ashler-production"), HostChannel::Production),
        ("ashler-staging", Some("ashler-staging"), HostChannel::Staging),
        ("crew-upstream-canary", Some("crew-upstream-canary"), HostChannel::Canary),
    ] {
        assert_eq!(HostChannel::for_controller(&scope(project, deployment)), Ok(channel), "{project} {deployment:?}");
    }
    for (project, deployment) in [
        ("ashler-staging", None),
        ("ashler-staging", Some("preview-deployment")),
        ("crew-upstream-canary", None),
        ("local-development", None),
    ] {
        assert!(HostChannel::for_controller(&scope(project, deployment)).is_err(), "{project} {deployment:?}");
    }
}

// Failure mode: wake starts one fixed channel's helper, which a machine set up
// for another channel refuses, so it never comes online. The names are the
// devbox skill's `crew-devbox-autostart` channels.
#[cfg(unix)]
#[tokio::test]
async fn host_start_runs_the_controller_channel_helper() {
    use std::os::unix::fs::PermissionsExt;
    for (channel, name) in [
        (HostChannel::Production, "production"),
        (HostChannel::Staging, "staging"),
        (HostChannel::Canary, "canary"),
    ] {
        for (provider, id) in [(DeviceEnvironment::Boat, BOX), (DeviceEnvironment::Namespace, "abcdefgh23456")] {
            let temp = tempfile::tempdir().unwrap();
            let helper = temp.path().join(".local/bin/crew-devbox-autostart");
            std::fs::create_dir_all(helper.parent().unwrap()).unwrap();
            let seen = temp.path().join("seen");
            std::fs::write(&helper, format!("#!/bin/sh\nprintf '%s' \"$*\" > '{}'\n", seen.display())).unwrap();
            std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700)).unwrap();
            let home = temp.path().display();
            // Boat: `--no-update --json info ID`, then `--no-update --json exec ID --timeout 180 COMMAND`.
            // Namespace: `exec ID -- sh -c COMMAND`.
            let script = if provider == DeviceEnvironment::Boat {
                format!("#!/bin/sh\nif [ \"$3\" = info ]; then\n  printf '%s' '{{\"sandbox\":{{\"id\":\"{BOX}\",\"state\":\"ready\"}}}}'\nelse\n  shift 6\n  exec env HOME='{home}' /bin/sh -c \"$1\"\nfi\n")
            } else {
                format!("#!/bin/sh\nshift 3\nexec env HOME='{home}' \"$@\"\n")
            };
            let executable = temp.path().join("provider");
            std::fs::write(&executable, script).unwrap();
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
            let stop = CancellationToken::new();
            prepare_host(&executable, provider, id, channel, &stop, &|| Ok(())).await.unwrap();
            assert_eq!(std::fs::read_to_string(&seen).unwrap(), name, "{provider:?}");
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn bounded_cli_capture_surfaces_excess_output_without_retaining_it() {
    let stop = CancellationToken::new();
    let result = run_cli(
        Path::new("/bin/sh"),
        &["-c", "dd if=/dev/zero bs=1024 count=80 2>/dev/null"],
        Duration::from_secs(5),
        &stop,
    )
    .await;
    assert!(matches!(result, Err(error) if error.contains("output exceeded")));
}

#[cfg(unix)]
#[tokio::test]
async fn cancelled_cli_cannot_leave_its_child_running() {
    let temp = tempfile::tempdir().unwrap();
    let pid_file = temp.path().join("owned-child");
    let script = format!("sleep 60 & echo $! > '{}'; wait", pid_file.display());
    let stop = CancellationToken::new();
    let cancel = stop.clone();
    let pid_path = pid_file.clone();
    let watcher = tokio::spawn(async move {
        // Wait on the fixture's own process start, not a provider/entity poll.
        for _ in 0..100 {
            if pid_path.exists() {
                cancel.cancel();
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("fixture child did not start");
    });
    let result = run_cli(
        Path::new("/bin/sh"),
        &["-c", &script],
        Duration::from_secs(5),
        &stop,
    )
    .await;
    watcher.await.unwrap();
    assert!(matches!(result, Err(error) if error.contains("cancelled")));
    let child: libc::pid_t = std::fs::read_to_string(pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    // Killed descendants can briefly remain as zombies; no live workload may survive.
    let output = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &child.to_string()])
        .output()
        .unwrap();
    let state = String::from_utf8_lossy(&output.stdout);
    assert!(
        state.trim().is_empty() || state.trim().starts_with('Z'),
        "child remains live: {state}"
    );
}

#[tokio::test]
async fn pre_cancelled_cli_never_attempts_to_spawn() {
    let stop = CancellationToken::new();
    stop.cancel();
    let missing = tempfile::tempdir().unwrap();
    let result = run_cli(&missing.path().join("must-not-be-spawned"), &[], Duration::from_secs(5), &stop).await;
    assert!(matches!(result, Err(error) if error.contains("cancelled")));
}

#[tokio::test]
async fn reconnect_burst_still_rechecks_peer_readiness() {
    let (_devices_tx, mut devices) = tokio::sync::watch::channel(Vec::new());
    let (online_tx, mut online) = tokio::sync::broadcast::channel(1);
    online_tx.send(()).unwrap();
    online_tx.send(()).unwrap();
    wait_for_readiness_change(&mut devices, &mut online).await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn dropped_cli_future_kills_descendants_without_an_explicit_cancel() {
    let temp = tempfile::tempdir().unwrap();
    let pid_file = temp.path().join("dropped-child");
    let script = format!("sleep 60 & echo $! > '{}'; wait", pid_file.display());
    let stop = CancellationToken::new();
    let args = ["-c", script.as_str()];
    let mut operation = Box::pin(run_cli(Path::new("/bin/sh"), &args, Duration::from_secs(60), &stop));
    tokio::select! {
        result = &mut operation => panic!("fixture exited before drop: {result:?}"),
        _ = async {
            for _ in 0..100 {
                if pid_file.exists() { return; }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            panic!("fixture child did not start");
        } => {}
    }
    // The total wake deadline drops the provider future, not its token.
    drop(operation);
    let child: libc::pid_t = std::fs::read_to_string(pid_file).unwrap().trim().parse().unwrap();
    let output = std::process::Command::new("ps").args(["-o", "stat=", "-p", &child.to_string()]).output().unwrap();
    let state = String::from_utf8_lossy(&output.stdout);
    assert!(state.trim().is_empty() || state.trim().starts_with('Z'), "child remains live after drop: {state}");
}

#[cfg(unix)]
#[tokio::test(start_paused = true)]
async fn wake_deadline_includes_waiting_for_another_wake() {
    let temp = tempfile::tempdir().unwrap();
    let core = crate::EngineCore::assemble_with_profile(
        crate::EngineProfile::local(temp.path()).unwrap(),
        Arc::new(crate::HarnessRegistry::new()), zeron_proto::HarnessId::Omp, None,
    ).unwrap();
    let links = LinkCache::new(zeron_rpc::LinkCacheConfig::new(
        "https://unused.invalid", Arc::new(zeron_rpc::StaticToken("unused".into())),
    ));
    let device: Device = serde_json::from_value(serde_json::json!({
        "id": "remote", "name": "Boat", "platform": "linux",
        "devboxProvider": "boat", "devboxId": BOX,
    })).unwrap();
    core.workspace.upsert_device_row(&device);
    let _gate = GATE.lock().await;
    let authorize = || Ok(());
    let operation = wake(&core.workspace, &links, &device.id, HostChannel::Staging, &authorize);
    tokio::pin!(operation);
    tokio::select! {
        result = &mut operation => panic!("wake escaped its occupied gate: {result:?}"),
        _ = tokio::task::yield_now() => {}
    }
    // Failure mode: the whole wake gets less than its steps' combined limits, so a
    // cold Boat wake (167 s on 2026-11-03) times out while each step is in bounds.
    let budget = Duration::from_secs(zeron_proto::DEVBOX_WAKE_TIMEOUT_SECS);
    tokio::time::advance(budget - Duration::from_secs(1)).await;
    tokio::select! {
        result = &mut operation => panic!("wake gave up before its budget: {result:?}"),
        _ = tokio::task::yield_now() => {}
    }
    tokio::time::advance(Duration::from_secs(1)).await;
    assert!(matches!(operation.await, Err(error) if error.contains("timed out")));
    core.sessions.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn queued_wake_rejects_a_changed_binding_before_provider_operations() {
    let temp = tempfile::tempdir().unwrap();
    let core = crate::EngineCore::assemble_with_profile(
        crate::EngineProfile::local(temp.path()).unwrap(),
        Arc::new(crate::HarnessRegistry::new()), zeron_proto::HarnessId::Omp, None,
    ).unwrap();
    let links = LinkCache::new(zeron_rpc::LinkCacheConfig::new(
        "https://unused.invalid", Arc::new(zeron_rpc::StaticToken("unused".into())),
    ));
    let mut device: Device = serde_json::from_value(serde_json::json!({
        "id": "remote", "name": "Boat", "platform": "linux",
        "devboxProvider": "boat", "devboxId": BOX,
    })).unwrap();
    core.workspace.upsert_device_row(&device);
    let gate = GATE.lock().await;
    let authorize = || Ok(());
    let operation = wake(&core.workspace, &links, &device.id, HostChannel::Staging, &authorize);
    tokio::pin!(operation);
    assert!(futures::poll!(operation.as_mut()).is_pending());
    // Removing the binding also keeps the unfixed path provider-free.
    device.devbox_id = None;
    core.workspace.upsert_device_row(&device);
    drop(gate);
    assert!(matches!(operation.await, Err(error) if error.contains("binding changed")));
    core.sessions.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn queued_wake_rpc_rejects_sign_out_revocation_and_scope_change() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use zeron_rpc::{RpcService, TokenSource, methods};
    use zeron_sync::scaffold_auth::{save_session, ScaffoldConfig, ScaffoldIdentity, ScaffoldSession, DEFAULT_OAUTH_SCOPES};
    use std::sync::atomic::{AtomicBool, Ordering};

    // A Canary controller must reach the wake gate like a staging one; only
    // staging controllers could wake before.
    for (project, outcome) in ["ashler-staging", "crew-upstream-canary"].into_iter()
        .flat_map(|project| ["signed-out", "revoked", "scope-changed"].map(|outcome| (project, outcome)))
    {
        let temp = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let moved = Arc::new(AtomicBool::new(false));
        let server = tokio::spawn({
            let resource = origin.clone();
            let moved = moved.clone();
            async move {
                loop {
                    let (mut stream, _) = listener.accept().await.unwrap();
                    let mut bytes = [0_u8; 8192];
                    let size = stream.read(&mut bytes).await.unwrap();
                    let request = String::from_utf8_lossy(&bytes[..size]);
                    let body = if request.starts_with("GET /auth/session ") {
                        serde_json::json!({"userId":"owner", "email":"owner@example.com",
                            "projectScope":project, "deploymentId": if moved.load(Ordering::SeqCst) { "other-deployment" } else { project },
                            "credential":"scaffold", "capabilities":["session.read"]})
                    } else {
                        assert!(request.starts_with("GET /api/code-sandboxes/auth/session "), "unexpected request: {request}");
                        serde_json::json!({"ok":true, "resource":resource, "actor":{"sub":"owner", "auth":"iap"},
                            "scopes":DEFAULT_OAUTH_SCOPES.split_whitespace().collect::<Vec<_>>()})
                    }.to_string();
                    stream.write_all(format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
                }
            }
        });
        let auth_dir = temp.path().join("auth");
        std::fs::create_dir_all(&auth_dir).unwrap();
        save_session(&auth_dir.join("scaffold-session.json"), &ScaffoldSession {
            access_token: "sc_rc_test".into(), resource: origin.clone(), edge_origin: origin.clone(), device: None,
            identity: ScaffoldIdentity { user_id: "owner".into(), email: "owner@example.com".into(), name: None,
                project_scope: project.into(), deployment_id: Some(project.into()), capabilities: vec!["session.read".into()] },
        }).unwrap();
        let mut config = crate::AuthConfig::new(&origin, &auth_dir);
        config.scaffold = Some(ScaffoldConfig::new(&origin, &origin, project));
        let auth = crate::Auth::detect(config).await;
        assert!(auth.state().is_signed_in());
        let token = TokenSource::token(&auth).await.unwrap();
        let runtime = crate::scaffold::ScaffoldRuntime::new(
            crate::scaffold::ScaffoldClient::new(&origin, project, auth.for_profile_tokens()).unwrap(),
            &origin, Arc::new(crate::scaffold::UnavailableDeviceJoinGrantProvider),
        ).with_deployment_id(project.into());
        let crew = crate::crew::CrewServices::new(auth.clone(), runtime,
            Arc::new(zeron_sync::DocsStore::open(&temp.path().join("ledger")).unwrap())).unwrap();
        let core = crate::EngineCore::assemble_with_profile(
            crate::EngineProfile::scaffold(&temp.path().join("engine"), &auth).unwrap(),
            Arc::new(crate::HarnessRegistry::new()), zeron_proto::HarnessId::Omp, None,
        ).unwrap();
        let links = LinkCache::new(zeron_rpc::LinkCacheConfig::new(
            "https://unused.invalid", Arc::new(zeron_rpc::StaticToken("unused".into())),
        ));
        let rpc = core.build_rpc().with_auth(auth.clone()).with_crew(crew).with_links(links);
        let mut device: Device = serde_json::from_value(serde_json::json!({
            "id": "remote", "name": "Boat", "platform": "linux",
            "devboxProvider": "boat", "devboxId": BOX,
        })).unwrap();
        core.workspace.upsert_device_row(&device);
        let gate = GATE.lock().await;
        let operation = rpc.handle(methods::WAKE_DEVICE, serde_json::json!({"deviceId": device.id}));
        tokio::pin!(operation);
        assert!(futures::poll!(operation.as_mut()).is_pending(), "{project} wake did not reach the gate");
        match outcome {
            "signed-out" => auth.sign_out(),
            "revoked" => auth.invalidate(token.generation),
            "scope-changed" => {
                moved.store(true, Ordering::SeqCst);
                auth.invalidate(token.generation);
                assert!(auth.access_token().await.is_err());
            }
            _ => unreachable!(),
        }
        // Even a regressed authorization guard must not invoke a real CLI.
        device.devbox_id = None;
        core.workspace.upsert_device_row(&device);
        drop(gate);
        let error = match operation.await {
            Err(error) => error,
            Ok(_) => panic!("queued {project} {outcome} wake must be rejected"),
        };
        assert!(error.to_string().contains("Crew"), "{project} {outcome}: {error}");
        core.sessions.shutdown().await;
        server.abort();
    }
}

// Failure modes: after a Boat agent restart its exec sessions lack BOAT_ID, so
// the guarded start helper refuses the machine Boat just routed the exec to;
// a fallback that overrides Boat's own value hides a copied binding; an ID
// that is not a valid Boat ID reaches the remote shell.
#[cfg(unix)]
#[tokio::test]
async fn boat_start_supplies_only_a_missing_native_identity() {
    use std::os::unix::fs::PermissionsExt;
    for (native, expected) in [(None, BOX), (Some("bx_abcdefgh"), "bx_abcdefgh")] {
        let temp = tempfile::tempdir().unwrap();
        let helper = temp.path().join(".local/bin/crew-devbox-autostart");
        std::fs::create_dir_all(helper.parent().unwrap()).unwrap();
        let seen = temp.path().join("seen");
        std::fs::write(&helper, format!(
            "#!/bin/sh\nprintf '%s %s' \"${{BOAT_ID-unset}}\" \"$1\" > '{}'\n", seen.display(),
        )).unwrap();
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700)).unwrap();
        let identity = native.map_or("-u BOAT_ID".to_owned(), |id| format!("BOAT_ID={id}"));
        let executable = temp.path().join("boat");
        std::fs::write(&executable, format!(
            "#!/bin/sh\nif [ \"$3\" = info ]; then\n  printf '%s' '{{\"sandbox\":{{\"id\":\"{BOX}\",\"state\":\"ready\"}}}}'\nelse\n  shift 6\n  exec env {identity} HOME='{}' /bin/sh -c \"$1\"\nfi\n",
            temp.path().display(),
        )).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let stop = CancellationToken::new();
        prepare_host(&executable, DeviceEnvironment::Boat, BOX, HostChannel::Staging, &stop, &|| Ok(())).await.unwrap();
        assert_eq!(std::fs::read_to_string(&seen).unwrap(), format!("{expected} staging"));
    }
    let temp = tempfile::tempdir().unwrap();
    let executable = temp.path().join("boat");
    let invoked = temp.path().join("invoked");
    std::fs::write(&executable, format!("#!/bin/sh\ntouch '{}'\n", invoked.display())).unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let stop = CancellationToken::new();
    let forged = format!("{BOX}\"; touch pwned; \"");
    assert!(prepare_host(&executable, DeviceEnvironment::Boat, &forged, HostChannel::Staging, &stop, &|| Ok(())).await.is_err());
    assert!(!invoked.exists(), "an invalid Boat ID must not reach the provider CLI");
}

#[cfg(unix)]
#[tokio::test]
async fn provider_preparation_rechecks_authority_between_commands() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let executable = temp.path().join("boat");
    let inspected = temp.path().join("inspected");
    let mutated = temp.path().join("mutated");
    std::fs::write(&executable, format!(
        "#!/bin/sh\nif [ \"$3\" = info ]; then\n  touch '{}'\n  printf '%s' '{{\"sandbox\":{{\"id\":\"{BOX}\",\"state\":\"archived\"}}}}'\nelse\n  touch '{}'\nfi\n",
        inspected.display(), mutated.display(),
    )).unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let authorize = || {
        if inspected.exists() { Err("Controller scope changed".to_owned()) } else { Ok(()) }
    };
    let stop = CancellationToken::new();
    let result = prepare_host(&executable, DeviceEnvironment::Boat, BOX, HostChannel::Staging, &stop, &authorize).await;
    assert!(matches!(result, Err(error) if error == "Controller scope changed"));
    assert!(inspected.exists());
    assert!(!mutated.exists(), "revoked authority must not resume or exec the machine");
}
