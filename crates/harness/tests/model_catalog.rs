#![cfg(unix)]
use std::{path::Path, sync::Arc};
use zeron_harness::{AcpHarness, CodexHarness, Harness};

fn harnesses(binary: &Path) -> Vec<Arc<dyn Harness>> {
    vec![
        Arc::new(CodexHarness::new().with_executable(binary)),
        Arc::new(AcpHarness::grok().with_executable(binary)),
        Arc::new(AcpHarness::hermes().with_executable(binary)),
        Arc::new(zeron_harness::PiHarness::new().with_executable(binary)),
        Arc::new(AcpHarness::antigravity().with_executable(binary)),
        Arc::new(AcpHarness::devin().with_executable(binary)),
    ]
}
fn binary(root: &Path) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = root.join("agent");
    std::fs::write(&path, r#"#!/usr/bin/python3
import json, pathlib, sys, os
root = pathlib.Path(__file__).parent
if '--version' in sys.argv:
    print('agent 1.2.3')
    sys.exit(0)
state = json.loads((root / 'state.json').read_text())
with (root / 'pids').open('a') as pids: pids.write(str(os.getpid()) + '\n')
if 'list' in sys.argv:
    if state['fail']:
        print(state.get('error', 'rate limit'), file=sys.stderr)
        sys.exit(1)
    print(json.dumps({'families':[{'variants':[{'model_uid':state['id'],'label':state['id']}]}]}))
    sys.exit(0)
for line in sys.stdin:
    request = json.loads(line)
    if 'id' not in request: continue
    if 'type' in request:
        data = {}
        if request['type'] == 'get_available_models':
            data = {'models':[] if state.get('empty') else [{'id':state['id'],'provider':'fixture','name':state['id']}]}
        if request['type'] == 'get_available_thinking_levels': data = {'levels':['off']}
        print(json.dumps({'id':request['id'],'type':'response','command':request['type'], 'success':not state['fail'], 'data':data,'error':state.get('error','rate limit')}),flush=True)
        continue
    method = request['method']
    if state['fail'] and method != 'initialize':
        response = {'error':{'code':429,'message':state.get('error', 'rate limit')}}
    else:
        result = {}
        if method == 'model/list': result = {'data':[] if state.get('empty') else [{'model':state['id'],'hidden':False,'isDefault':True}], 'nextCursor':None}
        if method == 'session/new': result = {'sessionId':'fixture','models':{'availableModels':[{'modelId':state['id'],'name':state['id']}]}}
        response = {'result':result}
    response.update({'jsonrpc':'2.0','id':request['id']})
    print(json.dumps(response), flush=True)
"#).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}
#[tokio::test]
async fn every_native_catalog_retains_last_good_and_cold_failure_stays_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let binary = binary(dir.path());
    for harness in harnesses(&binary) {
        let state = dir.path().join("state.json");
        std::fs::write(&state, r#"{"fail":false,"id":"account-model"}"#).unwrap();
        let first = harness.model_catalog(true).await.unwrap();
        assert_eq!(first.source, "live", "{:?}", harness.id());
        assert_eq!(
            first.models[0].id,
            if harness.id() == zeron_proto::HarnessId::Pi {
                "fixture/account-model"
            } else {
                "account-model"
            }
        );
        std::fs::write(&state, r#"{"fail":true,"id":"unused"}"#).unwrap();
        let retained = harness.model_catalog(true).await.unwrap();
        assert_eq!(retained.source, "cache");
        assert_eq!(retained.models, first.models);
        let cold = harnesses(&binary)
            .into_iter()
            .find(|h| h.id() == harness.id())
            .unwrap();
        assert!(
            cold.model_catalog(false).await.is_err(),
            "{:?}",
            harness.id()
        );
        std::fs::write(
            &state,
            r#"{"fail":true,"id":"unused","error":"not logged in"}"#,
        )
        .unwrap();
        let error = harness.model_catalog(true).await.unwrap_err();
        assert_eq!(
            zeron_harness::CatalogFailure::classify(&error),
            zeron_harness::CatalogFailureCode::AuthRequired
        );
        assert!(
            harness.models().await.is_err(),
            "auth failures must surface for {:?}",
            harness.id()
        );
    }
}

#[tokio::test]
async fn codex_empty_catalogs_retire_children_and_next_request_spawns_fresh() {
    let dir = tempfile::tempdir().unwrap();
    let binary = binary(dir.path());
    let harness = CodexHarness::new().with_executable(binary);
    let state = dir.path().join("state.json");
    std::fs::write(&state, r#"{"fail":false,"empty":true,"id":"ignored"}"#).unwrap();
    let error = harness.model_catalog(true).await.unwrap_err();
    assert_eq!(
        zeron_harness::CatalogFailure::classify(&error),
        zeron_harness::CatalogFailureCode::Failed
    );
    let reaped = |expected: usize| {
        let ids: Vec<i32> = std::fs::read_to_string(dir.path().join("pids"))
            .unwrap()
            .lines()
            .map(|line| line.parse().unwrap())
            .collect();
        assert_eq!(ids.len(), expected);
        for pid in ids {
            assert_eq!(
                unsafe { libc::kill(pid, 0) },
                -1,
                "discovery child {pid} still exists"
            );
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::ESRCH)
            );
        }
    };
    reaped(3);
    std::fs::write(&state, r#"{"fail":false,"id":"good"}"#).unwrap();
    let good = harness.model_catalog(true).await.unwrap();
    assert_eq!(good.models[0].id, "good");
    reaped(4);
    std::fs::write(&state, r#"{"fail":false,"empty":true,"id":"ignored"}"#).unwrap();
    let retained = harness.model_catalog(true).await.unwrap();
    assert_eq!(retained.source, "cache");
    assert_eq!(retained.models, good.models);
    reaped(7);
}

#[test]
fn auth_context_child() {
    let Some(root) = std::env::var_os("ZERON_MODEL_CONTEXT_TEST_ROOT") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let binary = binary(&root);
    for (harness, auth) in harnesses(&binary).into_iter().zip([
        ".codex/auth.json",
        ".grok/auth.json",
        ".hermes/auth.json",
        ".pi/agent/auth.json",
        ".gemini/antigravity-acp/oauth_creds.json",
        ".local/share/devin/credentials.toml",
    ]) {
        let before = harness.model_context().unwrap().unwrap();
        let path = root.join(auth);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "first-account").unwrap();
        let first = harness.model_context().unwrap().unwrap();
        assert_ne!(before.hash, first.hash, "{auth}");
        std::fs::write(&path, "other-account").unwrap();
        assert_ne!(
            first.hash,
            harness.model_context().unwrap().unwrap().hash,
            "{auth}"
        );
    }
    let claude = zeron_harness::ClaudeHarness::new().with_executable(&binary);
    let opencode = zeron_harness::OpencodeHarness::new().with_executable(&binary);
    for (harness, file) in [
        (&claude as &dyn Harness, ".claude/settings.json"),
        (&opencode as &dyn Harness, ".local/share/opencode/auth.json"),
        (&opencode as &dyn Harness, "opencode.json"),
    ] {
        let before = harness.model_context().unwrap().unwrap().hash;
        let path = root.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "configured-account").unwrap();
        assert_ne!(
            before,
            harness.model_context().unwrap().unwrap().hash,
            "{file}"
        );
    }
}
#[test]
fn each_spec_hashes_its_auth_file_contents() {
    let dir = tempfile::tempdir().unwrap();
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "auth_context_child", "--nocapture"])
        .current_dir(dir.path())
        .env("HOME", dir.path())
        .env("ZERON_MODEL_CONTEXT_TEST_ROOT", dir.path());
    for key in [
        "CLAUDE_CONFIG_DIR",
        "XDG_CONFIG_HOME",
        "OPENCODE_CONFIG",
        "CODEX_HOME",
        "GROK_HOME",
        "HERMES_HOME",
        "PI_CODING_AGENT_DIR",
        "GEMINI_HOME",
        "XDG_DATA_HOME",
    ] {
        command.env_remove(key);
    }
    let result = command.output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stdout)
    );
}
