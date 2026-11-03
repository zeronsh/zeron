//! The desktop app's own updates, for whichever interface it has (the GPUI
//! app's `crates/ui/src/app_update.rs`, the zgx interface's `src/about.rs`):
//! what both do the same, apart from how they draw it.
//!
//! The lifecycle around these (Sparkle's, VS Code's): a found release
//! downloads in the background ([`stage_offer`]), "Restart to update" installs
//! it and relaunches ([`InstallKind::apply_desktop`] with `relaunch`), and one
//! nobody restarted for installs when the app quits (without).

use std::path::{Path, PathBuf};

use crate::{Channel, InstallKind, Offer, app_version, bundle_engine_version, channel, fetch_latest, offer};

/// What the selected channel offers this app now, with the release itself:
/// the feed asked again, so a long-lived status still lands on the newest
/// release (and that release's checksums).
pub async fn current_offer(edge_url: &str, install: &InstallKind) -> anyhow::Result<(Offer, crate::Manifest)> {
    let manifest = fetch_latest(edge_url).await?;
    let running_engine = match install {
        InstallKind::MacApp { bundle } => bundle_engine_version(bundle),
        _ => None,
    };
    let offer = offer(
        &manifest,
        app_version(),
        manifest.channel,
        running_engine.as_deref(),
        channel::macos_version(),
    );
    Ok((offer, manifest))
}

/// Download, verify and stage what the selected channel offers this app (a
/// newer release, or the other channel's after a switch): its version and
/// the staged bundle. An offer that's gone or held is an error, in words for
/// the person.
pub async fn stage_offer(
    edge_url: &str,
    data_dir: &Path,
    install: &InstallKind,
) -> anyhow::Result<(String, PathBuf)> {
    let (offer, manifest) = current_offer(edge_url, install).await?;
    match offer {
        Offer::Newer | Offer::Switch => {}
        Offer::Held(why) => anyhow::bail!(why),
        Offer::Current => anyhow::bail!("the release feed no longer offers a newer version"),
    }
    let staged = install.stage_desktop(edge_url, &manifest, data_dir).await?;
    Ok((manifest.version, staged))
}

/// "Get beta updates" on or off: the beta channel needs a macOS app install
/// on [`channel::BETA_MINIMUM_MACOS`] or later (the refusal says why, for the
/// person); leaving it is always allowed. The channel chosen.
pub fn choose_channel(data_dir: &Path, beta: bool) -> Result<Channel, String> {
    if beta && let Some(why) = channel::beta_unavailable() {
        return Err(why);
    }
    let chosen = if beta { Channel::Beta } else { Channel::Stable };
    channel::select(data_dir, chosen).map_err(|err| format!("Couldn't save the choice: {err}"))?;
    tracing::info!(channel = chosen.as_str(), "update channel chosen");
    Ok(chosen)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaving_beta_is_always_allowed() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(choose_channel(dir.path(), false), Ok(Channel::Stable));
        assert_eq!(channel::read(dir.path()).channel, Channel::Stable);
    }

    /// A test binary isn't a macOS app install: beta is refused, in words.
    #[test]
    fn beta_needs_an_app_install() {
        let dir = tempfile::tempdir().unwrap();
        let refused = choose_channel(dir.path(), true).unwrap_err();
        assert!(refused.contains("Zeron app"), "{refused}");
        assert_eq!(channel::read(dir.path()).channel, Channel::Stable);
    }
}
