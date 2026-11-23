//! A dump's receipt (ds/orch_receipt) copies as it was written: its words
//! are one paragraph (its parts spans in it), not a box a word, so a
//! selection across it gives the dump's own text, spaces and all, never
//! the tabs zgx puts between boxes side by side. Mounted headless on
//! orch_receipt_fixture.luau (the sample's newest dump; crate::fixture_tests
//! finds it).

use zgx_script::testing::TestApp;

#[test]
fn a_receipt_copies_its_dump_as_written() {
    let mut app = TestApp::new(|runtime| crate::fixture_tests::host(runtime, "orch_receipt_fixture.luau"));
    app.ui.scene.borrow_mut().resize(900., 500.);
    app.frames(30);
    assert_eq!(app.error(), None, "the fixture failed");
    let words = app.target(".orch-receipt-words").expect("the receipt's words");
    assert!(app.ui.select_all_text(words), "its text selects");
    let copied = app.ui.selected_text();
    let written = "ok so the orchestrator page should be side by side not tabs, also remind me to rerun omarchy update it needs sudo. and ask the vpn guy whether moscow rebooted fine. the CI thing can wait till next week, and is the engine state thread still on the registry part?";
    assert_eq!(copied, written, "the dump as written");
    assert!(!copied.contains('\t'), "no tabs: {copied:?}");
}
