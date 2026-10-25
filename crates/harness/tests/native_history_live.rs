//! Experimental transcript editing against the real CLIs: write a session,
//! edit what it was told, resume the edited copy and ask what it remembers.
//! `#[ignore]`d (they spend a few cents on a small model):
//! `cargo test -p zeron-harness --test native_history_live -- --ignored`.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::process::Command;

use zeron_harness::{ClaudeHarness, CodexHarness, Harness};
use zeron_proto::native_history::{NativeEdit, NativeItemKind};

fn scratch() -> tempfile::TempDir {
    let base = std::path::PathBuf::from(std::env::var("HOME").unwrap())
        .join("Documents/dev/tmp/native-history");
    std::fs::create_dir_all(&base).unwrap();
    tempfile::tempdir_in(base).unwrap()
}

fn text_item<'a>(
    items: &'a [zeron_proto::native_history::NativeHistoryItem],
    needle: &str,
) -> &'a str {
    items
        .iter()
        .find(|item| matches!(&item.kind, NativeItemKind::Text { text } if text.contains(needle)))
        .unwrap_or_else(|| panic!("no item with {needle}: {items:#?}"))
        .id
        .as_str()
}

fn run(command: &mut Command) -> String {
    let out = command.output().unwrap();
    assert!(
        out.status.success(),
        "{command:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[tokio::test]
#[ignore = "runs the real claude CLI"]
async fn claude_resumes_the_edited_copy() {
    let dir = scratch();
    let started = run(Command::new("claude")
        .current_dir(dir.path())
        .args(["-p", "--model", "haiku", "--output-format", "json"])
        .arg("The secret word is MANGO. Reply with just: noted"));
    let session = serde_json::from_str::<serde_json::Value>(&started).unwrap()["session_id"]
        .as_str()
        .unwrap()
        .to_owned();

    let harness = ClaudeHarness::new();
    let env = BTreeMap::new();
    let read = harness
        .read_native_history(&session, dir.path(), &env)
        .await
        .unwrap();
    let prompt = text_item(&read.items, "MANGO").to_owned();
    let new_session = harness
        .write_edited_session(
            &session,
            dir.path(),
            &env,
            &read.revision,
            &[NativeEdit::Replace {
                id: prompt,
                text: "The secret word is KIWI. Reply with just: noted".into(),
            }],
        )
        .await
        .unwrap();
    assert_ne!(new_session, session);

    // The original is untouched; the copy lists the edit.
    let original = harness
        .read_native_history(&session, dir.path(), &env)
        .await
        .unwrap();
    assert_eq!(original.revision, read.revision);
    let edited = harness
        .read_native_history(&new_session, dir.path(), &env)
        .await
        .unwrap();
    text_item(&edited.items, "KIWI");

    let answer = run(Command::new("claude")
        .current_dir(dir.path())
        .args(["-p", "--model", "haiku", "--resume", &new_session])
        .arg("What was the secret word? Reply with just the word."));
    assert!(answer.to_uppercase().contains("KIWI"), "{answer}");
}

#[tokio::test]
#[ignore = "runs the real codex CLI"]
async fn codex_resumes_the_edited_thread() {
    let dir = scratch();
    let started = run(Command::new("codex")
        .current_dir(dir.path())
        .args(["exec", "--skip-git-repo-check", "--json"])
        .args(["-c", "model_reasoning_effort=\"low\""])
        .arg("The secret word is MANGO. Reply with just: noted"));
    let session = started
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find_map(|event| event["thread_id"].as_str().map(str::to_owned))
        .expect("no thread id in codex output");

    let harness = CodexHarness::new();
    let env = BTreeMap::new();
    let read = harness
        .read_native_history(&session, dir.path(), &env)
        .await
        .unwrap();
    let prompt = text_item(&read.items, "MANGO").to_owned();
    let new_session = harness
        .write_edited_session(
            &session,
            dir.path(),
            &env,
            &read.revision,
            &[NativeEdit::Replace {
                id: prompt,
                text: "The secret word is KIWI. Reply with just: noted".into(),
            }],
        )
        .await
        .unwrap();
    assert_ne!(new_session, session);

    let edited = harness
        .read_native_history(&new_session, dir.path(), &env)
        .await
        .unwrap();
    text_item(&edited.items, "KIWI");

    let answer = run(Command::new("codex")
        .current_dir(dir.path())
        .args(["exec", "resume", "--skip-git-repo-check"])
        .args(["-c", "model_reasoning_effort=\"low\""])
        .arg(&new_session)
        .arg("What was the secret word? Reply with just the word."));
    assert!(answer.to_uppercase().contains("KIWI"), "{answer}");
}
