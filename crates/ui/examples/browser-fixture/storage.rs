//! Invoked repeatedly by test-browser-storage.py with one stable HTTP origin.
use gpui::{AsyncApp, Entity, WindowHandle};
use std::{
    path::Path,
    time::{Duration, Instant},
};
use zeron_ui::{
    browser::{BrowserContext, BrowserSurface},
    shell::Shell,
};

async fn loaded(browser: &Entity<BrowserSurface>, cx: &mut AsyncApp) -> anyhow::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let page = browser.read_with(cx, |browser, _| browser.page.clone());
        anyhow::ensure!(
            page.error.is_none(),
            "storage page failed: {:?}",
            page.error
        );
        anyhow::ensure!(!page.title.starts_with("storage:fail:"), "{}", page.title);
        if page.title == "storage:pass" {
            return Ok(());
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "storage page timed out: {:?}",
            page
        );
        super::pause(cx, 50).await;
    }
}

pub async fn exercise(
    window: WindowHandle<Shell>,
    state: Entity<zeron_ui::state::AppState>,
    root: &Path,
    profile: &str,
    url: &str,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    if url == "cleanup" {
        let (tx, rx) = tokio::sync::oneshot::channel();
        cx.update(|_| {
            BrowserContext::fixture_remove_profile(root, profile, move |result| {
                let _ = tx.send(result);
            })
        });
        rx.await?.map_err(anyhow::Error::msg)?;
        return Ok(());
    }
    anyhow::ensure!(
        BrowserContext::fixture_persistence_supported(),
        "persistent profiles require macOS 14+ or Linux"
    );
    let (id, browser) = window.update(cx, |shell, window, cx| {
        shell.fixture_open_browser(Some(url.to_owned()), window, cx)
    })?;
    let loaded_result = loaded(&browser, cx).await;
    #[cfg(target_os = "macos")]
    if std::env::var_os("ZERON_BROWSER_STORAGE_DIAGNOSTICS").is_some() {
        let (tx, rx) = tokio::sync::oneshot::channel();
        browser.read_with(cx, |browser, _| browser.fixture_cookies(move |cookies| {
            let _ = tx.send(cookies);
        }));
        eprintln!("Storage native cookies before close ({url}): {:?}", rx.await?);
    }
    loaded_result?;
    window.update(cx, |shell, window, cx| {
        shell.fixture_close_browser(id, window, cx)
    })?;
    drop(browser);
    eprintln!("Storage lifecycle: closed page and dropped browser ({url})");
    if url.ends_with("/delete") && std::env::var_os("ZERON_BROWSER_STORAGE_REOPEN").is_some() {
        let empty = format!("{}/empty", url.strip_suffix("/delete").unwrap());
        let (id, reopened) = window.update(cx, |shell, window, cx| {
            shell.fixture_open_browser(Some(empty), window, cx)
        })?;
        loaded(&reopened, cx).await?;
        window.update(cx, |shell, window, cx| shell.fixture_close_browser(id, window, cx))?;
        eprintln!("Storage diagnostic: empty after closing and reopening the deleted page");
    }
    if url.ends_with("/write") {
        super::pause(cx, 2200).await; // Wait for the intentionally short-lived cookie.
        let url = format!("{}/read", url.strip_suffix("/write").unwrap());
        let (_, browser) = window.update(cx, |shell, window, cx| {
            shell.fixture_open_browser(Some(url.clone()), window, cx)
        })?;
        loaded(&browser, cx).await?;
        // Switch with a live page. The shell must close it before opening the
        // other identity, then recover the first identity's durable state.
        state.update(cx, |state, cx| {
            state.local_device_id = Some(format!("{profile}-other"));
            cx.notify();
        });
        let empty = format!("{}/empty", url.strip_suffix("/read").unwrap());
        let (_, other) = window.update(cx, |shell, window, cx| {
            shell.fixture_open_browser(Some(empty), window, cx)
        })?;
        anyhow::ensure!(
            !browser.read_with(cx, |b, _| b.fixture_native_visible()),
            "old profile remained visible"
        );
        loaded(&other, cx).await?;
        state.update(cx, |state, cx| {
            state.local_device_id = Some(profile.to_owned());
            cx.notify();
        });
        let (id, restored) = window.update(cx, |shell, window, cx| {
            shell.fixture_open_browser(Some(url.clone()), window, cx)
        })?;
        loaded(&restored, cx).await?;
        window.update(cx, |shell, window, cx| {
            shell.fixture_close_browser(id, window, cx)
        })?;
    }
    Ok(())
}
