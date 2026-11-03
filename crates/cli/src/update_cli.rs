//! `zeron update` — check for and apply a newer release, natively (the same
//! flow `edge/src/install.sh` performs: download → verify → symlink swap →
//! service restart). macOS app bundles swap the bundle instead; source builds
//! are report-only.

use anyhow::bail;
use zeron_update::{InstallKind, Offer};

/// `--check` prints the verdict and exits (nonzero when an update is available,
/// so scripts can gate on it). A macOS app follows its release channel
/// (`update-channel.json` in the data dir), as the app's own updater does.
pub async fn update(edge_url: &str, check_only: bool) -> anyhow::Result<()> {
    zeron_update::channel::set_data_dir(&super::paths::data_dir());
    let manifest = zeron_update::fetch_latest(edge_url).await?;
    let current = zeron_update::app_version();
    let running_engine = match zeron_update::detect_install() {
        InstallKind::MacApp { bundle } => zeron_update::bundle_engine_version(&bundle),
        _ => None,
    };
    match zeron_update::offer(
        &manifest,
        current,
        manifest.channel,
        running_engine.as_deref(),
        zeron_update::channel::macos_version(),
    ) {
        Offer::Newer | Offer::Switch => {}
        Offer::Held(why) => {
            println!("zeron {current}: {} isn't installable here. {why}", manifest.version);
            return Ok(());
        }
        Offer::Current => {
            println!(
                "zeron {current} is up to date (latest: {}).",
                manifest.version
            );
            return Ok(());
        }
    }
    println!("zeron {current} → {} available", manifest.version);
    if check_only {
        std::process::exit(1);
    }

    let install = zeron_update::detect_install();
    if let Some(blocker) = install.desktop_update_blocker() {
        bail!("{blocker}");
    }
    match install {
        InstallKind::Managed { app_root } => {
            println!(
                "downloading {}…",
                zeron_update::headless_artifact(&manifest.version)
            );
            zeron_update::stage_headless(edge_url, &manifest, &app_root).await?;
            zeron_update::apply_headless(&app_root, &manifest.version)?;
            println!(
                "installed {} (current → {})",
                app_root.join(&manifest.version).display(),
                manifest.version
            );
            match zeron_update::restart_service() {
                Ok(()) => println!("engine service restarted."),
                Err(err) => println!(
                    "note: service restart failed ({err:#}) — restart the engine manually to finish."
                ),
            }
            Ok(())
        }
        InstallKind::MacApp { bundle } => {
            println!(
                "downloading {}…",
                zeron_update::mac_app_artifact(&manifest.version)
            );
            let data_dir = super::paths::data_dir();
            let staged = zeron_update::stage_mac_app(edge_url, &manifest, &data_dir).await?;
            zeron_update::apply_mac_app(&staged, &bundle)?;
            println!("updated {} — relaunch Zeron to finish.", bundle.display());
            Ok(())
        }
        #[cfg(windows)]
        InstallKind::WindowsPortable { directory } => {
            let staged = zeron_update::windows::stage(edge_url, &manifest, &directory).await?;
            zeron_update::windows::apply(&staged, &directory, false)?;
            println!(
                "updated to {} — relaunch Zeron to finish.",
                manifest.version
            );
            Ok(())
        }
        InstallKind::Unmanaged => {
            bail!(
                "this binary is not update-managed (source build or hand-copied).\n\
                 Linux: curl -fsSL https://zeron.sh/install.sh | sh, or run install.sh from the release tarball\n\
                 macOS: download the new Zeron.app dmg, or rebuild from source.\n\
                 Windows: install with the Zeron setup .exe from {}, or rebuild from source.",
                zeron_update::LATEST_RELEASE_PAGE
            )
        }
    }
}
