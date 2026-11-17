//! The decision overlay (ds/orch_decision) and the Q&A's (its Answer) read
//! as a transcript does: every word in them selects (a press, a drag, a
//! double click) and copies (Ctrl/Cmd+C), through ds/selection_menu as the
//! transcript's do. A quote has a Copy of its own that copies its words as
//! plain text, and a drag over an option's words selects them rather than
//! choosing it. Mounted headless on ui/control_page_fixture.luau with
//! `fixture.decision_email` (a question whose context quotes an email to
//! send, and a command), and ui/control_qa_fixture.luau.

use zgx_script::{ScriptHost, testing::TestApp};

const QUESTION: &str = "Could you send this support request to mhost.ee";
const OVERLAY: &str = ".orch-decision";

fn fixture(name: &str) -> TestApp {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui").join(name);
    let mut app = TestApp::with_desktop(|runtime| ScriptHost::new(runtime, path));
    app.ui.scene.borrow_mut().resize(1440., 2000.);
    app.frames(30);
    assert_eq!(app.error(), None, "the fixture failed");
    app
}

/// The overlay's body scrolled to its end: what a reader reaches. (Headless,
/// the window's size reads as 1280×800, so the options are past its fold.)
fn to_options(app: &mut TestApp) {
    app.scroll(".orch-decision >> .orch-decision-body", 2000.);
    app.frames(10);
}

/// The email question, its overlay open.
fn email() -> TestApp {
    let mut app = fixture("control_page_fixture.luau");
    assert!(app.host.run_command("fixture.decision_email"));
    app.frames(30);
    app.click(&format!(r#"#control-widget-needs >> role:button ~"{QUESTION}""#));
    app.frames(40);
    assert!(app.shown(OVERLAY), "the overlay opened");
    app
}

/// The word in the middle of `target` (a press there twice), selected:
/// what's selected, as Ctrl/Cmd+C copies it: one word of `words`.
fn word_at(app: &mut TestApp, target: &str, words: &str) -> String {
    let r = app.bounds(target);
    app.ui.clear_text_selection();
    assert!(app.ui.select_word_at(r.x + r.width / 2., r.y + r.height / 2.), "{target}: its words select");
    let mut word = app.ui.selected_text();
    // (The middle a space between words: the first word, then.)
    if word.trim().is_empty() {
        app.ui.clear_text_selection();
        assert!(app.ui.select_word_at(r.x + 4., r.y + r.height / 2.), "{target}: its words select");
        word = app.ui.selected_text();
    }
    assert!(!word.trim().is_empty() && words.contains(word.trim()), "{target}: {word:?} is one of {words:?}");
    word
}

/// What Ctrl/Cmd+C copies now: the desktop copies the selected text when no
/// field has focus (zgx-desktop `clipboard_command`), as the selection's
/// Copy does (ds/selection_menu, the host's `copy_selection`).
fn copies(app: &TestApp) -> String {
    assert!(app.ui.focused_editor().is_none(), "a press in the words takes focus from the reply, so the copy is theirs");
    app.ui.selected_text()
}

fn clipboard(app: &TestApp) -> String {
    app.desktop.as_ref().expect("a desktop").clipboard()
}

#[test]
fn every_part_of_the_decision_overlay_selects() {
    let mut app = email();
    let parts = [
        (r#".orch-decision >> .orch-decision-question >> ~"support request""#, "Could you send this support request to mhost.ee about the German server's outbound connections dropping every 20–30 minutes?"),
        (r#".orch-decision >> .orch-decision-meta >> ~"asked""#, "asked now ago"),
        (r#".orch-decision >> .orch-decision-thread >> "Exnomic VPN""#, "Exnomic VPN"),
        (r#".orch-decision >> .orch-decision-context >> ~"Root cause""#, "Root cause of tonight's outages: de-mhost loses the ability to open new outbound TCP connections for about 3 minutes, every 20–30 minutes. Text to send (from your mhost account):"),
        (r#".orch-decision >> .md-quote >> ~"Hello. Our VPS""#, "Hello. Our VPS 103.227.84.140 periodically loses the ability to open new outbound TCP connections for about 2–3 minutes, roughly every 20–30 minutes."),
        (r#".orch-decision >> .md-quote >> ~"Do you apply""#, "Do you apply any rate limit or anti-abuse rule to outbound TCP connections from VMs?"),
        (r#".orch-decision >> .md-code >> ~"ss -s""#, "ss -s | head -3"),
        (r#".orch-decision >> .orch-decision-option >> ~"Later""#, "2 Later"),
        (r#".orch-decision >> .orch-decision-option >> ~"paste their answer""#, "I'll paste their answer here when it comes"),
    ];
    for (target, words) in parts {
        if target.contains("orch-decision-option") {
            to_options(&mut app);
        }
        word_at(&mut app, target, words);
    }
}

#[test]
fn a_double_click_and_copy_puts_the_word_on_the_clipboard() {
    let mut app = email();
    let target = r#".orch-decision >> .md-quote >> ~"Hello. Our VPS""#;
    let words = "Hello. Our VPS 103.227.84.140 periodically loses the ability to open new outbound TCP connections for about 2–3 minutes, roughly every 20–30 minutes.";
    // (With the reply focused first: a press in the words moves it.)
    app.click(".orch-decision >> role:textbox");
    app.frames(5);
    app.double_click(target);
    app.frames(5);
    let word = copies(&app);
    assert!(!word.is_empty() && !word.contains(' ') && words.contains(&word), "a word of it: {word:?}");
    // Still open: a press in its words is the reader's, not a close.
    assert!(app.shown(OVERLAY));
}

#[test]
fn a_drag_across_the_context_copies_it_as_written() {
    let mut app = email();
    app.drag(
        r#".orch-decision >> .orch-decision-context >> ~"Root cause""#,
        r#".orch-decision >> .md-quote >> ~"Thank you.""#,
    );
    app.frames(5);
    let picked = app.ui.selected_text();
    assert!(picked.contains("of tonight's outages:"), "the bold words, without their marks: {picked:?}");
    assert!(picked.contains("Do you apply any rate limit"), "the quote's list too: {picked:?}");
    assert!(!picked.contains("**") && !picked.contains("> "), "no markdown marks: {picked:?}");
    assert_eq!(copies(&app), picked);
    assert!(app.shown(OVERLAY));
}

#[test]
fn a_quotes_copy_gives_the_email_without_its_markers() {
    let mut app = email();
    app.click(".orch-decision >> .md-quote-copy");
    app.frames(5);
    let email = clipboard(&app);
    assert_eq!(
        email,
        "Hello. Our VPS 103.227.84.140 periodically loses the ability to open new outbound TCP connections for about 2–3 minutes, roughly every 20–30 minutes.\n\n1. Do you apply any rate limit or anti-abuse rule to outbound TCP connections from VMs?\n2. If so, what are the thresholds, and can it be raised for this VM?\n\nThank you."
    );
    assert!(app.find(r#".orch-decision >> .md-quote-copy >> "Copied""#).is_some(), "it says so");
    assert!(app.shown(OVERLAY));
}

#[test]
fn a_drag_over_an_options_words_selects_them_and_chooses_nothing() {
    let mut app = email();
    to_options(&mut app);
    app.drag(
        r#".orch-decision >> .orch-decision-option >> ~"paste their answer""#,
        r#".orch-decision >> .orch-decision-option >> ~"failover work continues""#,
    );
    app.frames(10);
    assert!(!app.ui.selected_text().is_empty(), "the words selected");
    assert!(app.find(".orch-decision >> .orch-decision-chosen").is_none(), "no option chosen: {}", app.tree());
    // A click (no drag) on one still chooses it.
    app.ui.clear_text_selection();
    app.click(r#".orch-decision >> .orch-decision-option >> ~"paste their answer""#);
    app.frames(10);
    assert!(app.find(".orch-decision >> .orch-decision-chosen").is_some(), "chosen by a click");
}

#[test]
fn the_qa_answer_overlay_selects_and_copies_too() {
    let mut app = fixture("control_qa_fixture.luau");
    app.click(r#"#control-widget-qa >> role:button ~"Are Inbox and Next sections""#);
    app.frames(40);
    assert!(app.shown(".orch-qa-dialog"), "the answer opened");
    word_at(
        &mut app,
        r#".orch-qa-dialog >> .orch-decision-question >> ~"Inbox and Next""#,
        "Are Inbox and Next sections in the board ever used? Under what conditions does that happen? Because I've never seen a card in Inbox or Next.",
    );
    word_at(
        &mut app,
        r#".orch-qa-dialog >> .orch-qa-dialog-answer >> ~"In today's flow""#,
        "In today's flow, essentially nothing reaches Inbox or Next. I checked the code.",
    );
    app.double_click(r#".orch-qa-dialog >> .orch-qa-dialog-answer >> ~"In today's flow""#);
    app.frames(5);
    assert!(!copies(&app).is_empty());
    // Its quotes copy too.
    assert!(app.find(".orch-qa-dialog >> .orch-qa-dialog-answer").is_some());
    assert!(app.shown(".orch-qa-dialog"));
}



