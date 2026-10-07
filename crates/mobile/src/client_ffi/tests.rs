use super::*;

#[test]
fn attachment_limit_matches_the_clients_and_the_24_mb_message() {
    // The composer's picker limit and `stage_attachments`' rejection
    // ("larger than 24 MB") must be the same number.
    assert_eq!(max_attachment_bytes(), 24 * 1024 * 1024);
    assert_eq!(
        max_attachment_bytes(),
        zc::attachments::MAX_ATTACHMENT_BYTES as u64
    );
}

#[test]
fn labels_for_ids_this_build_does_not_know_degrade_to_the_id() {
    // A newer host's harness/model/effort must render as itself, never blank
    // and never a panic, until this build learns a friendlier name.
    assert_eq!(harness_label("quantum-agent".into()), "quantum-agent");
    assert_eq!(
        model_label("quantum-agent".into(), "warp-9".into()),
        "warp-9"
    );
    assert_eq!(reasoning_label("warp".into()), "Warp");
    assert_eq!(reasoning_label(String::new()), "");
    // The static model list has no per-harness entry for it, so it falls
    // back to the Claude Code list rather than an empty picker.
    assert_eq!(
        fallback_models("quantum-agent".into()),
        fallback_models("claude-code".into())
    );
}

#[test]
fn known_ids_get_their_display_labels() {
    assert_eq!(harness_label("claude-code".into()), "Claude Code");
    assert_eq!(reasoning_label("xhigh".into()), "X-High");
}

#[test]
fn fallback_catalog_is_offered_and_leads_with_claude_code() {
    let harnesses = fallback_harnesses();
    assert!(harnesses.iter().all(|h| h.installed && h.offered));
    assert_eq!(harnesses[0].id, "claude-code");
    // First model is the default the picker selects.
    let models = fallback_models("claude-code".into());
    assert!(!models.is_empty());
    assert!(
        models
            .iter()
            .all(|m| !m.id.is_empty() && !m.label.is_empty())
    );
}

#[test]
fn parse_user_message_export_matches_the_client_parser() {
    let content = zc::attachments::with_attachments("hi", &["/uploads/a.png".to_owned()]);
    let parsed = parse_user_message(content);
    assert_eq!(parsed.text, "hi");
    assert_eq!(parsed.images[0].name, "a.png");
    assert_eq!(parse_user_message(String::new()).text, "");
}

#[test]
fn file_mention_link_marks_folders_and_encodes_the_path() {
    assert_eq!(
        file_mention_link("src/lib.rs".into(), false),
        "[lib.rs](zeron-file:src/lib.rs)"
    );
    // A folder link keeps its trailing slash so the host opens it as a dir.
    assert_eq!(
        file_mention_link("src/".into(), true),
        "[src](zeron-file:src/)"
    );
    assert_eq!(
        file_mention_link("my dir/a b.md".into(), false),
        "[a b.md](zeron-file:my%20dir/a%20b.md)"
    );
}
