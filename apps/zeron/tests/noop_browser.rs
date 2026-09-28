#[test]
fn browser_suppression_exits_without_starting_the_app() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_zeron"))
        .args([
            "--noop-browser",
            "https://accounts.google.com/o/oauth2/auth?client_id=fake&state=test",
        ])
        .output()
        .expect("suppression process");
    assert!(output.status.success(), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
}
