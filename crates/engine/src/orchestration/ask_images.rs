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
    let l = s.get(..8).unwrap_or(s).to_ascii_lowercase();
    l.starts_with("https://") || l.starts_with("http://")
}

/// Whether the person's interface may fetch `url` for an ask: https only,
/// to a public name (never an IP literal, a bare or local name, loopback,
/// private or link-local), no credentials in it.
pub(crate) fn safe_url(url: &str) -> bool {
    let Some(rest) = url.get(..8).filter(|p| p.eq_ignore_ascii_case("https://")).map(|_| &url[8..]) else { return false };
    if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.contains('@') || authority.starts_with('[') {
        return false;
    }
    let host = authority.rsplit_once(':').map_or(authority, |(h, port)| {
        if port.chars().all(|c| c.is_ascii_digit()) { h } else { authority }
    });
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() || host.contains(':') || host.parse::<std::net::IpAddr>().is_ok() {
        return false;
    }
    let labels: Vec<&str> = host.split('.').collect();
    // A name with a real (non-numeric) top level: "0x7f.1", "2130706433"
    // and "intranet" aren't.
    let Some(top) = labels.last() else { return false };
    if labels.len() < 2 || labels.iter().any(|l| l.is_empty()) || !top.chars().any(|c| c.is_ascii_alphabetic()) || top.starts_with("0x") {
        return false;
    }
    if !host.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.') {
        return false;
    }
    const LOCAL: &[&str] = &["localhost", "local", "internal", "lan", "home", "corp", "intranet", "localdomain", "arpa", "test", "invalid"];
    !LOCAL.contains(top)
}

/// `images` as an ask keeps them: URLs as they are, local pictures stored
/// (their stored paths), the rest left out. Each once, in order.
pub(crate) fn store_all(uploads: &Uploads, cwd: Option<&Path>, images: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for image in images {
        let image = image.trim();
        let kept = if is_url(image) {
            // (Remote pictures: https to a public name only.)
            safe_url(image).then(|| image.to_owned())
        } else if image.contains("://") {
            None
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

#[cfg(test)]
mod tests {
    use super::safe_url;

    #[test]
    fn remote_pictures_are_https_to_public_names_only() {
        for ok in ["https://x.dev/a.png", "https://cdn.example.com:8443/a/b.webp?raw=1", "HTTPS://Example.org/a.png"] {
            assert!(safe_url(ok), "{ok}");
        }
        for bad in [
            "http://x.dev/a.png",
            "file:///etc/a.png",
            "https://localhost/a.png",
            "https://app.localhost/a.png",
            "https://127.0.0.1/a.png",
            "https://10.0.0.5/a.png",
            "https://169.254.169.254/latest.png",
            "https://[::1]/a.png",
            "https://2130706433/a.png",
            "https://0x7f.1/a.png",
            "https://printer.local/a.png",
            "https://intranet/a.png",
            "https://user:pw@x.dev/a.png",
            "https://x.dev@127.0.0.1/a.png",
            "https://x .dev/a.png",
        ] {
            assert!(!safe_url(bad), "{bad}");
        }
    }
}
