//! Marks for user-added ACP agents. Registry agents publish a monochrome SVG
//! on the ACP CDN; each is fetched once into `{ui data dir}/acp-icons/` and
//! painted like the built-in marks (an alpha mask tinted by the surface, see
//! [`crate::icons::icon`]). Until it lands, and for custom commands, the
//! generic agent glyph stands in.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex, PoisonError};

use gpui::App;
use zeron_engine::registry::HarnessDescriptor;
use zeron_proto::HarnessId;

/// The only host icons are fetched from (the registry's own CDN).
const CDN_PREFIX: &str = "https://cdn.agentclientprotocol.com/";
const MAX_ICON_BYTES: usize = 256 * 1024;

#[derive(Default)]
struct Icons {
    /// Icon url → cached file (interned, so marks stay `&'static str`).
    files: HashMap<String, &'static str>,
    /// Agent → its icon url, from the catalogs seen so far.
    harnesses: HashMap<HarnessId, String>,
    /// Urls already fetched or being fetched this session.
    requested: HashSet<String>,
}

fn icons() -> std::sync::MutexGuard<'static, Icons> {
    static ICONS: LazyLock<Mutex<Icons>> = LazyLock::new(Default::default);
    ICONS.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The mark for a user-added agent: its downloaded icon, else the generic
/// agent glyph.
pub fn mark(harness: HarnessId) -> &'static str {
    let icons = icons();
    icons
        .harnesses
        .get(&harness)
        .and_then(|url| icons.files.get(url).copied())
        .unwrap_or(crate::icons::BOT)
}

/// The mark for a registry icon url (the Add provider list).
pub fn mark_for_url(url: Option<&str>, cx: &mut App) -> &'static str {
    let Some(url) = url else {
        return crate::icons::BOT;
    };
    ensure(url, cx);
    icons().files.get(url).copied().unwrap_or(crate::icons::BOT)
}

/// Remember each added agent's icon and fetch any not cached yet.
pub fn note_catalog(descriptors: &[HarnessDescriptor], cx: &mut App) {
    for descriptor in descriptors {
        let Some(url) = descriptor.acp.as_ref().and_then(|acp| acp.icon.clone()) else {
            continue;
        };
        icons().harnesses.insert(descriptor.id, url.clone());
        ensure(&url, cx);
    }
}

/// `{data dir}/acp-icons/<name>.svg` for a CDN url, `None` for any other.
fn cache_file(url: &str, cx: &App) -> Option<PathBuf> {
    let name = url.strip_prefix(CDN_PREFIX)?.rsplit('/').next()?;
    let stem = name.strip_suffix(".svg")?;
    let safe = !stem.is_empty()
        && stem
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    safe.then(|| crate::settings::data_dir(cx))
        .flatten()
        .map(|dir| dir.join("acp-icons").join(name))
}

fn ensure(url: &str, cx: &mut App) {
    if !icons().requested.insert(url.to_owned()) {
        return;
    }
    let Some(file) = cache_file(url, cx) else {
        return;
    };
    if file.is_file() {
        remember(url, &file);
        return;
    }
    let url = url.to_owned();
    let fetch = gpui_tokio::Tokio::spawn_result(cx, {
        let url = url.clone();
        async move { fetch(&url).await }
    });
    cx.spawn(async move |cx| {
        let bytes = match fetch.await {
            Ok(bytes) => bytes,
            Err(error) => {
                tracing::debug!(%url, %error, "ACP agent icon unavailable");
                return;
            }
        };
        let written = file
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| {
                let tmp = file.with_extension("svg.tmp");
                std::fs::write(&tmp, &bytes).and_then(|()| std::fs::rename(&tmp, &file))
            });
        if let Err(error) = written {
            tracing::debug!(%url, %error, "couldn't cache ACP agent icon");
            return;
        }
        remember(&url, &file);
        cx.update(|cx| cx.refresh_windows());
    })
    .detach();
}

fn remember(url: &str, file: &std::path::Path) {
    let path = zeron_proto::intern(&file.to_string_lossy());
    icons().files.insert(url.to_owned(), path);
}

async fn fetch(url: &str) -> Result<Vec<u8>, anyhow::Error> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let response = client.get(url).send().await?.error_for_status()?;
    let bytes = response.bytes().await?;
    anyhow::ensure!(bytes.len() <= MAX_ICON_BYTES, "icon is too large");
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(1024)]).to_lowercase();
    anyhow::ensure!(head.contains("<svg"), "not an SVG");
    Ok(bytes.to_vec())
}

#[cfg(test)]
mod tests {
    #[gpui::test]
    fn icons_cache_only_cdn_svgs(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            crate::settings::init(Default::default(), dir.path(), cx);
            let file = |url: &str| super::cache_file(url, cx);
            assert_eq!(
                file("https://cdn.agentclientprotocol.com/registry/v1/latest/gemini.svg"),
                Some(dir.path().join("acp-icons").join("gemini.svg"))
            );
            assert_eq!(file("https://example.com/registry/gemini.svg"), None);
            assert_eq!(
                file("https://cdn.agentclientprotocol.com/registry/v1/latest/a%2F..svg"),
                None
            );
            assert_eq!(file("https://cdn.agentclientprotocol.com/x/icon.png"), None);
        });
        assert_eq!(
            super::mark(zeron_proto::HarnessId::Acp("acp.unknown")),
            crate::icons::BOT
        );
    }
}
