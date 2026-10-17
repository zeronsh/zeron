//! `zeron visual check`: run a file's visuals the way Zeron will, without
//! Zeron open. An agent that wrote ```zeron-visual fences into a Markdown
//! document can't see the preview, so this is how it learns that a visual
//! fails, and at which line of the document.

use std::path::Path;

use anyhow::Context as _;
use zeron_visual::document;

/// Check every visual in `path` and print one line per visual. False when
/// any of them fails.
pub fn check(path: &Path) -> anyhow::Result<bool> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let name = path.display().to_string();
    let markdown = path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| matches!(ext.to_ascii_lowercase().as_str(), "md" | "markdown"));
    let (lines, ok) = if markdown {
        check_markdown(&name, &text)
    } else {
        check_script(&name, &text)
    };
    for line in lines {
        println!("{line}");
    }
    Ok(ok)
}

fn check_markdown(name: &str, text: &str) -> (Vec<String>, bool) {
    let fences = document::fences(text);
    if fences.is_empty() {
        return (
            vec![format!("{name}: no ```{} fences", document::FENCE_LANGUAGE)],
            true,
        );
    }
    let mut out = Vec::new();
    let mut ok = true;
    for (ix, fence) in fences.iter().enumerate() {
        // The seed the preview runs this fence with: same random data.
        let seed = zeron_visual::seed_for_document(&fence.code, fence.occurrence);
        ok &= report(
            &mut out,
            name,
            &format!("visual {}", ix + 1),
            fence.line,
            zeron_visual::check(&fence.code, seed),
        );
    }
    (out, ok)
}

fn check_script(name: &str, text: &str) -> (Vec<String>, bool) {
    // A fence's code has no trailing newline; seed the script as that fence.
    let script = text.strip_suffix('\n').unwrap_or(text);
    let script = script.strip_suffix('\r').unwrap_or(script);
    let seed = zeron_visual::seed_for_document(script, 0);
    let mut out = Vec::new();
    let ok = report(
        &mut out,
        name,
        "visual",
        0,
        zeron_visual::check(script, seed),
    );
    (out, ok)
}

fn report(
    out: &mut Vec<String>,
    name: &str,
    label: &str,
    first_line: usize,
    result: Result<zeron_visual::Report, zeron_visual::VisualError>,
) -> bool {
    match result {
        Ok(report) => {
            out.push(format!(
                "{name}:{}: {label} ok ({} interactions)",
                first_line.max(1),
                report.interactions
            ));
            out.extend(report.logs.iter().map(|log| format!("  print: {log}")));
            true
        }
        Err(error) => {
            let (line, message) = document::locate(&error.message, first_line);
            out.push(format!("{name}:{}: {label}: {message}", line.max(1)));
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_errors_point_at_document_lines() {
        let doc = "# Report\n\nIntro.\n\n```zeron-visual\n--! title: Fine\nprint(\"hello\")\nreturn function()\n  return ui.text \"ok\"\nend\n```\n\n- A list\n\n  ~~~zeron-visual\n  local x = 1\n  return function()\n    return ui.nope {}\n  end\n  ~~~\n";
        let (lines, ok) = check_markdown("doc.md", doc);
        assert!(!ok);
        assert_eq!(lines[0], "doc.md:5: visual 1 ok (0 interactions)");
        assert_eq!(lines[1], "  print: hello");
        // `ui.nope` is on script line 3 of the fence opened on line 15.
        assert!(
            lines[2].starts_with("doc.md:18: visual 2: "),
            "{}",
            lines[2]
        );
    }

    #[test]
    fn documents_without_visuals_pass() {
        let (lines, ok) = check_markdown("plain.md", "# Nothing\n\n```lua\nx()\n```\n");
        assert!(ok);
        assert_eq!(lines, ["plain.md: no ```zeron-visual fences"]);
    }

    #[test]
    fn markdown_checks_visuals_in_tab_indented_lists() {
        let (lines, ok) = check_markdown(
            "tabbed.md",
            "- item\n\n\t```zeron-visual\n\treturn ui.nope {}\n\t```\n",
        );
        assert!(!ok);
        assert!(lines[0].starts_with("tabbed.md:4: visual 1: "), "{lines:?}");
    }

    #[test]
    fn markdown_ignores_visual_source_in_html_and_indented_code() {
        for source in [
            "<!--\n```zeron-visual\nreturn ui.nope {}\n```\n-->\n",
            "<div>\n```zeron-visual\nreturn ui.nope {}\n```\n</div>\n",
            "    ```zeron-visual\n    return ui.nope {}\n    ```\n",
        ] {
            let (lines, ok) = check_markdown("plain.md", source);
            assert!(ok, "{lines:?}");
            assert_eq!(lines, ["plain.md: no ```zeron-visual fences"]);
        }
    }

    #[test]
    fn markdown_checks_the_visual_after_an_unclosed_list_fence() {
        let (lines, ok) = check_markdown(
            "list.md",
            "- ```zeron-visual\n  return ui.text \"first\"\n\n```zeron-visual\nreturn ui.nope {}\n```\n",
        );
        assert!(!ok);
        assert!(lines[0].starts_with("list.md:1: visual 1 ok"), "{lines:?}");
        assert!(lines[1].starts_with("list.md:5: visual 2: "), "{lines:?}");
    }

    #[test]
    fn scripts_report_their_own_lines() {
        let (lines, ok) = check_script(
            "v.luau",
            "local a = 1\nreturn function()\n  error(\"boom\")\nend\n",
        );
        assert!(!ok);
        assert!(lines[0].starts_with("v.luau:3: visual: "), "{}", lines[0]);
        assert!(lines[0].contains("boom"), "{}", lines[0]);
    }
}
