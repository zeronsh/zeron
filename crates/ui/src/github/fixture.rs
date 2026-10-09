//! Offline preview data, excluded from the application build.
use super::*;
use zeron_proto::{GitHubCheck, GitHubComment};

impl GitHubSurface {
    pub fn demo(
        state: Entity<AppState>,
        files: bool,
        commit: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let target = GitHubTarget::from_url(if commit {
            "https://github.com/acme/project/commit/abcdef1234567890abcdef1234567890abcdef12"
        } else {
            "https://github.com/acme/project/pull/42"
        })
        .unwrap();
        let mut view = Self::new(state, "demo".into(), target.clone(), cx);
        let mut page = GitHubPage {
            target, title: "Add a native GitHub viewer".into(),
            body: "This is an **offline preview with sample data**, rendered using Zeron's own components.\n\nView pull requests inside your workspace. The viewer shares Zeron's markdown renderer, panel controls, and virtualized diff viewer.\n\n### What you can inspect\n\n- **Discussion** shows comments and review summaries.\n- **Checks** shows CI status.\n- **Files** uses Zeron's unified and split diffs, wrapping, syntax colors, and file folding.\n\nUse the Light and Dark buttons to compare appearances.".into(),
            author: "contributor".into(), state: "OPEN".into(), draft: false,
            base_ref: "main".into(), head_ref: "feature/native-github".into(),
            additions: 7, deletions: 2, changed_files: 2, items: vec![],
            comments: vec![
                GitHubComment { author: "reviewer".into(), body: "The published patch stays independent of the local working tree.\n\n```rust\nlet view = Changes::for_published(state, cx);\n```".into(), created_at: "2026-10-09T12:00:00Z".into(), state: "COMMENTED".into(), path: None },
                GitHubComment { author: "maintainer".into(), body: "Looks good. Reusing the existing surface toolbar and diff renderer keeps this consistent with the rest of Zeron.".into(), created_at: "2026-10-09T13:00:00Z".into(), state: "APPROVED".into(), path: None },
            ],
            checks: vec![
                GitHubCheck { name: "Windows build".into(), status: "SUCCESS".into(), url: String::new() },
                GitHubCheck { name: "Rust tests".into(), status: "SUCCESS".into(), url: String::new() },
                GitHubCheck { name: "Linux build".into(), status: "IN_PROGRESS".into(), url: String::new() },
            ],
        };
        if commit {
            page.title = "Keep GitHub links inside the native viewer".into();
            page.state.clear();
            page.base_ref.clear();
            page.head_ref.clear();
            for comment in &mut page.comments {
                comment.state.clear();
            }
        }
        let patch = "diff --git a/crates/ui/src/shell.rs b/crates/ui/src/shell.rs\n--- a/crates/ui/src/shell.rs\n+++ b/crates/ui/src/shell.rs\n@@ -1,4 +1,8 @@\n enum RightSurface {\n     Browser(u64),\n+    GitHub(u64),\n     Diff(u64),\n }\n+\n+// GitHub pages share the existing panel host.\n+// The published patch uses the native Changes renderer.\ndiff --git a/crates/ui/src/links.rs b/crates/ui/src/links.rs\n--- a/crates/ui/src/links.rs\n+++ b/crates/ui/src/links.rs\n@@ -1,4 +1,5 @@\n fn open_link(url: &str) {\n-    open_browser(url);\n-    focus_address();\n+    if let Some(target) = GitHubTarget::from_url(url) {\n+        open_github(target);\n+    }\n }\n";
        view.changes.update(cx, |changes, cx| {
            changes.set_published_diff(
                CheckoutDiff {
                    checkout_id: page.target.url(),
                    device_id: String::new(),
                    cwd: String::new(),
                    patch: patch.into(),
                    files: vec![],
                    additions: 7,
                    deletions: 2,
                    truncated: false,
                    checksum: "native-github-preview".into(),
                    updated_at: chrono::Utc::now(),
                },
                cx,
            )
        });
        view.body = Some(markdown::parse_full(&page.body));
        view.comments = page
            .comments
            .iter()
            .map(|c| markdown::parse_full(&c.body))
            .collect();
        view.page = Some(page);
        view.diff_loaded = true;
        if files {
            view.tab = DetailTab::Files;
        }
        view
    }
}
