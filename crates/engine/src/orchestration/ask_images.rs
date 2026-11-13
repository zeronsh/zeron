//! An ask's pictures (an option's screenshot, the ask's own), kept in the
//! engine's uploads store when the ask is raised, so they outlive the files
//! they came from and read like any attachment. URLs pass through as they
//! are; a local file must be a picture inside its thread's folder (the
//! attachment jail, [`crate::uploads::Uploads::store_ask_image`]), else it
//! is left out.

use std::path::{Path, PathBuf};

use zeron_proto::orchestration::Ask;

use crate::rpc::EngineRpc;
use crate::uploads::Uploads;

fn is_url(s: &str) -> bool {
    s.starts_with("https://") || s.starts_with("http://")
}

/// `images` as an ask keeps them: URLs as they are, local pictures stored
/// (their stored paths), the rest left out. Each once, in order.
pub(crate) fn store_all(uploads: &Uploads, cwd: Option<&Path>, images: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for image in images {
        let image = image.trim();
        let kept = if is_url(image) {
            Some(image.to_owned())
        } else {
            match uploads.store_ask_image(image, cwd) {
                Ok(stored) => Some(stored),
                Err(error) => {
                    tracing::info!(%error, "ask: a picture left out");
                    None
                }
            }
        };
        if let Some(kept) = kept
            && !out.contains(&kept)
        {
            out.push(kept);
        }
    }
    out
}

/// Store `ask`'s pictures (its own and each option's) for its thread in
/// `cwd`.
pub(crate) fn store_in(uploads: &Uploads, cwd: Option<&Path>, ask: &mut Ask) {
    ask.images = store_all(uploads, cwd, &ask.images);
    for option in &mut ask.options {
        option.images = store_all(uploads, cwd, &option.images);
    }
}

/// Whether `ask` names any picture that isn't stored yet.
pub(crate) fn any(ask: &Ask) -> bool {
    !ask.images.is_empty() || ask.options.iter().any(|o| !o.images.is_empty())
}

/// `chat`'s folder, its pictures' jail (`~` its home's).
pub(crate) fn cwd_of(rpc: &EngineRpc, chat: &str) -> Option<PathBuf> {
    let cwd = rpc.workspace_host().chat(chat).ok().flatten().and_then(|c| c.cwd)?;
    let home = || std::env::var_os("HOME").map(PathBuf::from);
    match cwd.strip_prefix('~') {
        Some("") => home(),
        Some(rest) if rest.starts_with('/') => home().map(|h| h.join(rest.trim_start_matches('/'))),
        _ => Some(PathBuf::from(cwd)),
    }
}
