//! Against a real `muse` (opt-in): `MUSE_LIVE_BIN=/path/to/muse cargo test
//! -p zeron-harness --test muse_account_live -- --ignored --nocapture`.
//! Uses a throwaway, file-backed config root: never the user's own login.
use std::collections::BTreeMap;
use zeron_harness::muse::account;

fn live() -> Option<(
    std::path::PathBuf,
    tempfile::TempDir,
    BTreeMap<String, String>,
)> {
    let bin = std::path::PathBuf::from(std::env::var_os("MUSE_LIVE_BIN")?);
    let root = tempfile::tempdir().unwrap();
    let env = BTreeMap::from([
        (
            "XDG_CONFIG_HOME".to_string(),
            root.path().to_string_lossy().into_owned(),
        ),
        ("TBH_CREDENTIAL_BACKEND".to_string(), "file".to_string()),
        ("MUSE_NO_AUTO_UPDATE".to_string(), "1".to_string()),
    ]);
    Some((bin, root, env))
}

#[tokio::test]
#[ignore]
async fn a_fresh_root_is_signed_out_and_a_device_login_hands_back_its_code() {
    let Some((bin, root, env)) = live() else {
        return;
    };
    let state = account::read(&bin, &env).await.unwrap();
    assert_eq!(state.state, "loggedOut");
    assert!(account::usage(&bin, &env).await.unwrap().is_none());
    let login = account::start_device_login(&bin, &env).await.unwrap();
    println!(
        "verify at {} with {}",
        login.verification_url, login.user_code
    );
    assert!(login.verification_url.starts_with("https://"));
    assert!(!login.user_code.is_empty());
    drop(login);
    // Nothing was stored for an unapproved login.
    assert!(!root.path().join("muse/auth.json").exists());
}

/// Signs in for real (approve the printed code within 15 min), runs a real
/// turn on that account and steers a second one mid-flight, then signs out.
#[tokio::test]
#[ignore]
async fn an_approved_device_login_runs_real_turns_and_signs_out() {
    use futures::StreamExt;
    use zeron_harness::{
        CancellationToken, Harness, MuseHarness, RunControls, SteerMessage, TurnControl,
    };
    use zeron_proto::{AgentEvent, RunRequest, SandboxLevel};
    let Some((bin, root, env)) = live() else {
        return;
    };
    let login = account::start_device_login(&bin, &env).await.unwrap();
    println!(
        "\n>>> Open {} and enter {}\n",
        login.verification_url, login.user_code
    );
    let signed_in = login.finish().await.unwrap();
    println!("signed in: {signed_in:?}");
    assert_eq!(signed_in.state, "accountLogin");
    assert!(
        root.path().join("muse/auth.json").is_file(),
        "the login stays in its own root"
    );
    println!("usage: {:?}", account::usage(&bin, &env).await);

    let work = tempfile::tempdir().unwrap();
    let request = |prompt: &str| RunRequest {
        prompt: prompt.into(),
        harness: None,
        model: None,
        reasoning: None,
        model_options: Default::default(),
        cwd: work.path().display().to_string(),
        sandbox: SandboxLevel::ReadOnly,
        auto_approve: false,
        access: Default::default(),
        plan: false,
        resume: None,
        attachments: vec![],
        worktree: None,
        mcp: None,
        env: env.clone(),
    };
    let (steer, steering) = tokio::sync::mpsc::channel(8);
    let controls = RunControls {
        realtime: None,
        execution_lease: None,
        steering,
        interrupt: CancellationToken::new(),
        request_input: Box::new(|_| {
            let (tx, rx) = tokio::sync::oneshot::channel();
            let _ = tx.send(vec![]);
            rx
        }),
        turn: TurnControl::default(),
    };
    let harness = MuseHarness::new().with_executable(&bin);
    let mut stream = harness
        .run(request("Reply with exactly the word: pong"), controls)
        .await
        .unwrap();
    let mut text = String::new();
    let mut turns = 0;
    let mut steered = false;
    while turns < 2 {
        let event = tokio::time::timeout(std::time::Duration::from_secs(240), stream.next())
            .await
            .expect("Muse went quiet")
            .expect("stream ended")
            .unwrap();
        match &event {
            AgentEvent::TextDelta { text: delta, .. } => text.push_str(delta),
            AgentEvent::Steered { .. } => steered = true,
            AgentEvent::Done { status, error, .. } => {
                turns += 1;
                println!("turn {turns} done: {status:?} {error:?} text={text:?}");
                if turns == 1 {
                    assert!(text.to_lowercase().contains("pong"), "{text}");
                    text.clear();
                    // The next prompt starts a turn; a second one lands in it.
                    steer
                        .send(SteerMessage::text(
                            "Count from 1 to 40, one number per line, slowly.",
                        ))
                        .await
                        .unwrap();
                    tokio::time::sleep(std::time::Duration::from_secs(6)).await;
                    steer
                        .send(SteerMessage::text(
                            "Stop counting now and reply with exactly: STEERED",
                        ))
                        .await
                        .unwrap();
                }
            }
            _ => {}
        }
    }
    println!("steered: {steered}; last text: {text:?}");
    assert!(text.contains("STEERED") || steered, "{text}");
    drop(stream);
    println!("usage after turns: {:?}", account::usage(&bin, &env).await);
    let out = account::logout(&bin, &env).await.unwrap();
    assert!(!out.signed_in());
}

/// One real turn on an account that is already signed in (a Zeron-kept
/// root: `MUSE_LIVE_ROOT=<agent-accounts/muse-homes/<slot>/config>`), with the
/// session opted into Muse's experimental surface: the turn completes and
/// the subscription's usage arrives as `PlanUsage`.
#[tokio::test]
#[ignore]
async fn a_real_turn_reports_the_subscriptions_usage() {
    use futures::StreamExt;
    use zeron_harness::{CancellationToken, Harness, MuseHarness, RunControls, TurnControl};
    use zeron_proto::{AgentEvent, RunRequest, SandboxLevel};
    let (Some(bin), Some(root)) = (
        std::env::var_os("MUSE_LIVE_BIN"),
        std::env::var_os("MUSE_LIVE_ROOT"),
    ) else {
        return;
    };
    let env = BTreeMap::from([
        (
            "XDG_CONFIG_HOME".to_string(),
            root.to_string_lossy().into_owned(),
        ),
        ("TBH_CREDENTIAL_BACKEND".to_string(), "file".to_string()),
    ]);
    let work = tempfile::tempdir().unwrap();
    let (_steer, steering) = tokio::sync::mpsc::channel(1);
    let controls = RunControls {
        realtime: None,
        execution_lease: None,
        steering,
        interrupt: CancellationToken::new(),
        request_input: Box::new(|_| {
            let (tx, rx) = tokio::sync::oneshot::channel();
            let _ = tx.send(vec![]);
            rx
        }),
        turn: TurnControl::default(),
    };
    let request = RunRequest {
        prompt: "Reply with exactly the word: pong".into(),
        harness: None,
        model: None,
        reasoning: None,
        model_options: Default::default(),
        cwd: work.path().display().to_string(),
        sandbox: SandboxLevel::ReadOnly,
        auto_approve: false,
        access: Default::default(),
        plan: false,
        resume: None,
        attachments: vec![],
        worktree: None,
        mcp: None,
        env,
    };
    let mut stream = MuseHarness::new()
        .with_executable(std::path::PathBuf::from(bin))
        .run(request, controls)
        .await
        .unwrap();
    let mut plan = None;
    loop {
        let event = tokio::time::timeout(std::time::Duration::from_secs(180), stream.next())
            .await
            .expect("Muse went quiet")
            .expect("stream ended")
            .unwrap();
        match event {
            AgentEvent::PlanUsage {
                windows,
                plan_label,
            } => {
                println!("plan usage: {plan_label:?} {windows:?}");
                plan = Some(windows);
            }
            AgentEvent::Done { status, error, .. } => {
                println!("done: {status:?} {error:?}");
                break;
            }
            _ => {}
        }
    }
    assert!(plan.is_some(), "no usage/changed during the turn");
}
