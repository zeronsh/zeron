//! Signing in to agents from the ACP registry the ways real ones do
//! (tests/fixtures/fake-registry-signin.py, one shape each, from the wire on
//! Nov 2 2026), each added as the registry adds an agent:
//!
//! - one that starts its sign-in in a session's auth_required answer, its
//!   device code there, and whose `authenticate` then opens the page with
//!   the system's `open` and waits for the pairing (Factory Droid): asked in
//!   ACP's order (a session first), its code and page reported, signed in;
//! - one whose `authenticate` never signs anything in: not taken for signed
//!   in; handed to a terminal, its own CLI run bare (no ACP arguments);
//! - one whose sign-in is its own `command` in the older `_meta` form
//!   (Kimi's `kimi login`): that command, not the ACP launch with `login`
//!   after it; its device page reported;
//! - one that opens its page with the system's `open`, never `$BROWSER`
//!   (Gemini CLI): the page recorded for the app to open (no browser here),
//!   and the callback it waits for finishes it.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use zeron_harness::acp::{NEEDS_TERMINAL, SignInOptions, SignInProgress};
use zeron_harness::{AcpHarness, acp_registry as reg};
use zeron_proto::RegistryAgent;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-registry-signin.py")
}

/// The agents, added: one record each, the fixture in the mode named.
fn agents() -> &'static Path {
    static ROOT: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    ROOT.get_or_init(|| {
        let root = tempfile::tempdir().unwrap();
        for mode in ["droid", "droid-stale", "droid-cli", "kimi", "gemini"] {
            let dir = root.path().join(format!("fake-{mode}"));
            std::fs::create_dir_all(&dir).unwrap();
            let record = serde_json::json!({
                "id": format!("fake-{mode}"), "name": format!("Fake {mode}"), "version": "1.0.0", "kind": "binary",
                "program": fixture(), "args": ["--acp"], "env": { "FAKE_SIGNIN": mode }, "verified": true
            });
            std::fs::write(dir.join("agent.json"), record.to_string()).unwrap();
        }
        // SAFETY: set once, before any test reads it.
        unsafe { std::env::set_var("ZERON_AGENTS_DIR", root.path()) };
        root
    })
    .path()
}

struct Run {
    outcome: Result<(), String>,
    progress: Vec<SignInProgress>,
}

/// Signs `mode`'s agent in as the engine does: a recording `$BROWSER` and
/// the file it records to. `callback`: what to do with a recorded page.
async fn sign_in(mode: &str, on_page: impl FnOnce(String, PathBuf) + Send + 'static) -> (Run, PathBuf) {
    agents();
    let scratch = tempfile::tempdir().unwrap().keep();
    let url_file = scratch.join("browser-url");
    std::fs::write(&url_file, "").unwrap();
    let browser = scratch.join("browser");
    std::fs::write(&browser, format!("#!/bin/sh\nprintf '%s\\n' \"$1\" >> '{}'\n", url_file.display())).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&browser, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let agent = RegistryAgent::new(&format!("fake-{mode}")).unwrap();
    assert!(reg::installed(agent));
    let harness = AcpHarness::registry(agent, &format!("Fake {mode}"));
    let options = SignInOptions {
        browser: Some(browser),
        method: None,
        env: vec![
            ("ZERON_LOGIN_URL_FILE".into(), url_file.clone().into()),
            ("HOME".into(), scratch.clone().into()),
        ],
        url_filter: None,
    };
    // The page, once recorded, to whoever answers it.
    let watch = url_file.clone();
    let home = scratch.clone();
    tokio::spawn(async move {
        for _ in 0..200 {
            if let Some(url) = std::fs::read_to_string(&watch)
                .ok()
                .and_then(|t| t.lines().map(str::trim).find(|l| l.starts_with("http")).map(str::to_owned))
            {
                on_page(url, home);
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    });
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let outcome = tokio::time::timeout(
        Duration::from_secs(30),
        harness.sign_in_registry(options, move |p| log.lock().unwrap().push(p)),
    )
    .await
    .expect("the sign-in ends")
    .map_err(|e| e.to_string());
    let progress = seen.lock().unwrap().clone();
    (Run { outcome, progress }, url_file)
}

#[tokio::test]
async fn a_sign_in_started_by_the_session_reports_its_code_and_page_and_finishes() {
    let (run, url_file) = sign_in("droid", |url, home| {
        // The person pairs the device on the page (the code in it).
        assert!(url.contains("user_code=WXYZ-9876"), "{url}");
        std::fs::write(home.join("paired"), "").unwrap();
    })
    .await;
    run.outcome.expect("paired: signed in");
    assert!(
        run.progress.iter().any(|p| matches!(p, SignInProgress::Output(l) if l.contains("Your code: WXYZ-9876"))),
        "its code, to show: {:?}",
        run.progress
    );
    assert!(std::fs::read_to_string(url_file).unwrap().contains("https://auth.example.com/device?user_code=WXYZ-9876"));
}

#[tokio::test]
async fn credentials_it_rejects_are_logged_out_and_the_sign_in_starts_over() {
    // (What the user hit: stale credentials, no code, an instant no-op.)
    let (run, url_file) = sign_in("droid-stale", |url, home| {
        assert!(url.contains("user_code=WXYZ-9876"), "{url}");
        std::fs::write(home.join("paired"), "").unwrap();
    })
    .await;
    run.outcome.expect("logged out, paired: signed in");
    assert!(
        run.progress.iter().any(|p| matches!(p, SignInProgress::Output(l) if l.contains("Your code: WXYZ-9876"))),
        "{:?}",
        run.progress
    );
    assert!(std::fs::read_to_string(url_file).unwrap().contains("user_code=WXYZ-9876"));
}

#[tokio::test]
async fn an_authenticate_that_signs_nothing_in_goes_to_a_terminal_with_the_agents_own_cli() {
    let (run, _) = sign_in("droid-cli", |_, _| {}).await;
    let error = run.outcome.expect_err("not signed in");
    let command = error.split(NEEDS_TERMINAL).nth(1).unwrap_or_else(|| panic!("a terminal: {error}"));
    assert!(command.contains("fake-registry-signin.py"), "{command}");
    assert!(!command.contains("--acp"), "its own CLI, without the ACP arguments: {command}");
}

#[tokio::test]
async fn an_older_terminal_method_runs_its_own_command_and_reports_its_page() {
    let (run, url_file) = sign_in("kimi", |_, _| {}).await;
    run.outcome.expect("its own `login` signs in");
    assert!(
        run.progress.iter().any(|p| matches!(p, SignInProgress::OpenBrowser(u) if u.contains("user_code=ABCD-1234"))),
        "{:?}",
        run.progress
    );
    // (Not `--acp login`: that would have been the ACP launch, refused.)
    assert!(
        !run.progress.iter().any(|p| matches!(p, SignInProgress::Output(l) if l.contains("Welcome"))),
        "{:?}",
        run.progress
    );
    let _ = url_file;
}

#[tokio::test]
async fn a_page_opened_with_the_systems_opener_is_recorded_and_its_callback_finishes_it() {
    let (run, url_file) = sign_in("gemini", |url, _| {
        // The app opens the page on the device the person is at; the
        // provider redirects to the loopback callback (the tunnel's end).
        let parsed = reqwest::Url::parse(&url).unwrap();
        let redirect = parsed
            .query_pairs()
            .find(|(k, _)| k == "redirect_uri")
            .map(|(_, v)| v.into_owned())
            .unwrap();
        let port = reqwest::Url::parse(&redirect).unwrap().port().unwrap();
        tokio::spawn(async move {
            let mut socket = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
            socket.write_all(b"GET /oauth2callback?code=x HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").await.unwrap();
            let mut reply = String::new();
            let _ = socket.read_to_string(&mut reply).await;
        });
    })
    .await;
    run.outcome.expect("its callback finishes it");
    let recorded = std::fs::read_to_string(url_file).unwrap();
    assert!(recorded.contains("https://accounts.example.com/auth?redirect_uri="), "{recorded}");
}
