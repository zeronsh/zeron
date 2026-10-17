//! Actual native load_html + opaque-origin script/carousel/CSP regression.
use gpui::{AsyncApp, Entity, WindowHandle};
use std::{
    path::Path,
    time::{Duration, Instant},
};
use zeron_markdown::content_reference::{HtmlCompatibility, HtmlPreview};
use zeron_ui::{browser::BrowserSurface, shell::Shell, state::AppState};

async fn capture(
    window: WindowHandle<Shell>,
    browser: &Entity<BrowserSurface>,
    output: &Path,
    name: &str,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    {
        let _ = window;
        let (tx, rx) = tokio::sync::oneshot::channel();
        browser.read_with(cx, |browser, _| {
            browser.fixture_snapshot(move |result| {
                let _ = tx.send(result);
            })
        });
        let png = rx.await?.map_err(anyhow::Error::msg)?;
        std::fs::write(output.join(format!("{name}.png")), png)?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = browser;
        super::capture_frame(window, output, name, cx).await
    }
}

const SAMPLE: &str = r#"
<style>body{background:#eff7f5;color:#164f46}.card{padding:24px;border:1px solid #89b7ac;border-radius:12px}button{padding:8px 14px;border-radius:6px;border:1px solid #89b7ac;background:#fff;color:#164f46}</style>
<h1>Sentence practice</h1><p>Compare two compact layouts.</p>
<div class="viz-carousel" aria-label="Practice layouts">
<section data-variant="Single column"><article class="card"><h2>Single column</h2><p>Listen, write, and compare your answer.</p><button id="answer" onclick="this.textContent='Submitted'">Submit</button></article></section>
<section data-variant="Two columns" hidden><article class="card"><h2>Two columns</h2><p>Answer and feedback side by side.</p></article></section>
</div>
<script>
addEventListener('DOMContentLoaded', () => setTimeout(async () => {
  try {
    const assert = (condition, message) => { if (!condition) throw Error(message); };
    let isolated = false;
    try { void parent.document.body; } catch { isolated = true; }
    assert(isolated, 'child can access parent');
    isolated = false;
    try { localStorage.setItem('preview', 'bad'); } catch { isolated = true; }
    assert(isolated, 'child has origin storage');
    let blocked = false;
    try { await fetch('data:text/plain,network'); } catch { blocked = true; }
    assert(blocked, 'connect-src escaped');
    document.querySelector('#answer').click();
    assert(document.querySelector('#answer').textContent === 'Submitted', 'inline script did not run');
    const slides = [...document.querySelectorAll('section[data-variant]')];
    assert(!slides[0].hidden && slides[1].hidden, 'initial variant');
    document.querySelector('nav button:last-child').click();
    assert(slides[0].hidden && !slides[1].hidden, 'next variant');
    document.querySelector('nav button:first-child').click();
    assert(!slides[0].hidden && slides[1].hidden, 'previous variant');
    parent.postMessage('preview:pass', '*');
  } catch (error) { parent.postMessage('preview:fail:' + error.message, '*'); }
}, 100));
</script>
"#;

pub async fn exercise(
    window: WindowHandle<Shell>,
    state: Entity<AppState>,
    output: &Path,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    state.update(cx, |state, cx| {
        let entries = serde_json::from_value(serde_json::json!([
            {"id":"fixture-html","role":"assistant","parts":[{"id":"text","kind":"text","text":"Compare the two compact layouts.\n\n:codex-file-citation{path=\"/tmp/practice-source.pdf\" purpose=\"source\"}\n\n\u{e200}visualize\u{e202}{\"path\":\"/tmp/layout-preview.html\",\"mode\":\"wide\"}\u{e201}"}],"createdAt":1788900001000_i64,"deviceId":"local","status":"complete"}
        ])).unwrap();
        state.receive_transcript_frame(zeron_doc::TranscriptFrame::Reset { reset: entries }, cx).unwrap();
    });
    let request = HtmlPreview {
        path: "/tmp/layout-preview.html".into(),
        wide: true,
        compatibility: HtmlCompatibility::CodexVisualize,
    };
    let (_, browser) = window.update(cx, |shell, window, cx| {
        shell.fixture_open_browser(None, window, cx)
    })?;
    window
        .update(cx, |_, window, cx| {
            browser.update(cx, |browser, cx| {
                browser.fixture_document(&request, SAMPLE, window, cx)
            })
        })?
        .map_err(anyhow::Error::msg)?;
    let deadline = Instant::now() + Duration::from_secs(25);
    loop {
        let (status, page) = browser.read_with(cx, |browser, _| {
            (browser.fixture_document_status(), browser.page.clone())
        });
        anyhow::ensure!(page.error.is_none(), "HTML load failed: {page:?}");
        anyhow::ensure!(
            !status
                .as_deref()
                .is_some_and(|s| s.starts_with("preview:fail:")),
            "{status:?}"
        );
        if status.as_deref() == Some("preview:pass") {
            break;
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "HTML preview timed out: {status:?} {page:?}"
        );
        super::pause(cx, 50).await;
    }
    capture(window, &browser, output, "html-preview", cx).await?;
    if let Some(path) = std::env::var_os("ZERON_HTML_PREVIEW_FILE") {
        let content = std::fs::read_to_string(path)?;
        window.update(cx, |shell, _, cx| shell.fixture_expand_browser(cx))?;
        window
            .update(cx, |_, window, cx| {
                browser.update(cx, |browser, cx| {
                    browser.fixture_document(&request, &content, window, cx)
                })
            })?
            .map_err(anyhow::Error::msg)?;
        super::pause(cx, 1200).await;
        capture(window, &browser, output, "html-preview-user-file", cx).await?;
    }
    std::fs::write(
        output.join("html-preview-result.json"),
        "{\"status\":\"pass\"}\n",
    )?;
    Ok(())
}
