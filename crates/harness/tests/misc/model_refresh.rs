//! Catalog refresh must survive large responses, account changes, and failed
//! probes without pinning a stale catalog for the lifetime of the engine.
#![cfg(unix)]
use std::{os::unix::fs::PermissionsExt, path::Path};
use zeron_harness::{AcpHarness, Harness};

fn fixture(dir: &Path) -> std::path::PathBuf {
    let script = dir.join("agent.py");
    std::fs::write(&script, r#"#!/usr/bin/env python3
import json, pathlib, sys, time
if sys.argv[1:] == ['--version']:
    print('agent 1.0.0')
    sys.exit(0)
root = pathlib.Path(__file__).parent
with (root / 'calls').open('a') as f: f.write('probe\n')
time.sleep(0.15)
state = json.loads((root / 'state').read_text())
items = [{'id': state['id'] + str(i), 'name': 'Model ' + str(i), 'description': 'x' * 2048} for i in range(256)]
if sys.argv[1:] == ['models']:
    if state.get('broken'):
        print('{"ev":"models","items":[')
    else:
        print(json.dumps({'ev': 'models', 'items': items}))
    sys.exit(state.get('exit', 0))
for line in sys.stdin:
    req = json.loads(line)
    if 'id' not in req: continue
    if state.get('broken'):
        print(json.dumps({'jsonrpc': '2.0', 'id': req['id'], 'error': {'code': -32603, 'message': 'offline'}}), flush=True)
        continue
    result = {} if req['method'] == 'initialize' else {'sessionId': 'test', 'models': {'currentModelId': items[0]['id'], 'availableModels': [dict(item, modelId=item['id']) for item in items]}}
    print(json.dumps({'jsonrpc': '2.0', 'id': req['id'], 'result': result}), flush=True)
"#).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    script
}

async fn check_refresh(harness: &dyn Harness, dir: &Path) {
    let state = dir.join("state");
    std::fs::write(&state, r#"{"id":"first-"}"#).unwrap();
    let (first, overlap) = tokio::join!(harness.models(), harness.models());
    let first = first.unwrap();
    assert_eq!(first.len(), 256);
    assert_eq!(first[255].id, "first-255");
    assert_eq!(first, overlap.unwrap());
    assert_eq!(
        std::fs::read_to_string(dir.join("calls"))
            .unwrap()
            .lines()
            .count(),
        1
    );

    std::fs::write(&state, r#"{"id":"second-"}"#).unwrap();
    assert_eq!(harness.models().await.unwrap()[255].id, "second-255");
    std::fs::write(&state, r#"{"id":"broken-","broken":true}"#).unwrap();
    let fallback = harness.models().await.unwrap();
    assert_eq!(fallback.len(), 256);
    assert_eq!(fallback[255].id, "second-255");
    std::fs::write(&state, r#"{"id":"recovered-"}"#).unwrap();
    assert_eq!(harness.models().await.unwrap()[255].id, "recovered-255");
}

#[tokio::test]
async fn acp_refreshes_large_catalogs_coalesces_and_recovers() {
    let dir = tempfile::tempdir().unwrap();
    let harness = AcpHarness::grok().with_executable(fixture(dir.path()));
    check_refresh(&harness, dir.path()).await;
}
