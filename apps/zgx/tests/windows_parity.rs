//! Run the existing Luau browser, terminal, annotation and path regressions
//! with the same pinned host that builds the Windows app.
use std::path::PathBuf;

#[test]
fn browser_terminal_annotation_and_windows_paths() {
    let ui = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui");
    for filter in ["annotation flow:", "browser", "terminal", "Windows paths:"] {
        let report = zgx_script::test_runner::run(&ui, Some(filter), |outcome| {
            if let Some(failure) = &outcome.failure {
                eprintln!("{}: {failure}", outcome.name);
            }
        })
        .expect("the UI test runner must load");
        assert!(!report.outcomes.is_empty(), "no tests matched {filter}");
        assert_eq!(report.failed(), 0, "{filter}: {report:#?}");
    }
}
