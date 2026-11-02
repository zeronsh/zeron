//! Droid's native session id survives runtime death through engine dispatch.
#![cfg(unix)]
use std::{os::unix::fs::PermissionsExt, sync::Arc, time::Duration};
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::DroidHarness;
use zeron_proto::{HarnessId, RunRequest, SandboxLevel};

#[tokio::test]
async fn droid_idle_crash_next_dispatch_loads_session_without_replay() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("droid.py");
    // Kept self-contained: engine tests do not depend on another package's
    // optional fixture binary having been built beforehand.
    std::fs::write(&fixture, r#"#!/usr/bin/env python3
import json,sys
settings={"modelId":"fixture","reasoningEffort":"low","specModeModelId":"fixture","specModeReasoningEffort":"low","autonomyLevel":"high","availableAutonomyLevels":["off","low","medium","high"],"interactionMode":"auto","toolExecutionMode":"direct_only"}
loaded=False
def send(m):
 m.update(jsonrpc="2.0",factoryApiVersion="1.0.0")
 print(json.dumps(m),flush=True)
def notify(n):
 send({"type":"notification","method":"droid.session_notification","params":{"notification":n}})
for line in sys.stdin:
 m=json.loads(line);p=m.get("params",{});method=m.get("method")
 assert m["type"]=="request" and isinstance(m["id"],str)
 if method in ["droid.initialize_session","droid.load_session"]:
  loaded=method=="droid.load_session"
  if not loaded:settings.update({k:v for k,v in p.items() if k in settings})
  send({"type":"response","id":m["id"],"result":{"sessionId":"saved-droid","settings":settings,"availableModels":[],"session":{"id":"saved-droid","messages":[{"role":"assistant","content":[{"type":"text","text":"NEVER REPLAY"}]}]}}})
 elif method=="droid.update_session_settings":
  settings.update(p)
  notify({"type":"settings_updated","requestId":m["id"],"settings":settings})
  send({"type":"response","id":m["id"],"result":{}})
 elif method=="droid.add_user_message":
  if p["text"]=="require-resume" and not loaded:sys.exit(43)
  send({"type":"response","id":m["id"],"result":{}})
  notify({"type":"assistant_text_delta","textDelta":"reply:"+p["text"]})
  notify({"type":"agent_turn_completed","reason":"completed"})
  if p["text"]=="idle-crash":sys.exit(0)
 else:send({"type":"response","id":m["id"],"result":{}})
"#).unwrap();
    std::fs::set_permissions(&fixture, std::fs::Permissions::from_mode(0o755)).unwrap();
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(
        DroidHarness::new()
            .with_executable(fixture)
            .with_graces(Duration::from_millis(50), Duration::from_millis(100)),
    ));
    let core =
        EngineCore::assemble(dir.path(), Arc::new(registry), HarnessId::Droid, None).unwrap();
    let chat = "droid-idle-crash";
    let handle = core.doc_host.open(chat).unwrap();
    for prompt in ["idle-crash", "require-resume"] {
        let req = RunRequest {
            prompt: prompt.into(),
            harness: None,
            model: None,
            reasoning: None,
            model_options: Default::default(),
            cwd: dir.path().display().to_string(),
            sandbox: SandboxLevel::WorkspaceWrite,
            auto_approve: true,
            access: Default::default(),
            plan: false,
            attachments: vec![],
            worktree: None,
            resume: None,
            mcp: None,
            env: Default::default(),
            extra_mcp: Vec::new(),
        };
        core.sessions
            .dispatch(chat, HarnessId::Droid, req, None)
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let entries = handle.doc().read_entries().unwrap();
                assert!(!entries.iter().any(|entry| entry.parts.iter().any(|part| matches!(part,zeron_doc::MessagePart::Text {text,..} if text.contains("NEVER REPLAY")))));
                if entries.iter().any(|entry| entry.parts.iter().any(|part| matches!(part,zeron_doc::MessagePart::Text {text,..} if text==&format!("reply:{prompt}")))) { break; }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("Droid must load the engine's saved native session id");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    core.shutdown().await;
}
