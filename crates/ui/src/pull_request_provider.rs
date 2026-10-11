//! What the board and detail view show about a pull request's provider.
//!
//! The engine's adapter sends it as data: a page's or detail's presentation,
//! a row's reference, an actor's avatar, a badge's features. Engines that
//! predate that data send none, and GitHub was then the only provider, so
//! each concern falls back here, and only here, to how GitHub was shown.

use std::borrow::Cow;

use zeron_proto::{
    ChangeRequestActor, ChangeRequestFeatures, ChangeRequestMediaRewrite, ChangeRequestPresentation,
};

/// The host older engines read every change request from.
const FALLBACK_HOST: &str = "github.com";

/// `data`, or GitHub's presentation for engines that send none.
pub(crate) fn presentation(
    data: Option<&ChangeRequestPresentation>,
) -> Cow<'_, ChangeRequestPresentation> {
    match data {
        Some(data) => Cow::Borrowed(data),
        None => Cow::Owned(ChangeRequestPresentation {
            provider_name: "GitHub".into(),
            cli_name: "GitHub CLI".into(),
            cli_command: "gh".into(),
            sign_in_command: "gh auth login".into(),
            noun: "pull request".into(),
            media_hosts: vec![
                FALLBACK_HOST.into(),
                ".githubusercontent.com".into(),
                ".githubassets.com".into(),
            ],
            media_rewrites: vec![ChangeRequestMediaRewrite {
                host: FALLBACK_HOST.into(),
                after_segments: 2,
                segment: "blob".into(),
                raw_host: "raw.githubusercontent.com".into(),
            }],
            features: ChangeRequestFeatures {
                native_detail: true,
                comments: true,
                handoff: true,
            },
        }),
    }
}

/// How a row or badge names its change request: `#12` before engines said.
pub(crate) fn reference(number: u64, reference: Option<&str>) -> String {
    match reference {
        Some(reference) if !reference.is_empty() => reference.to_owned(),
        _ => format!("#{number}"),
    }
}

/// Whether `url` opens in the native detail view. Older engines read only
/// github.com through the native view; anything else opened in the browser.
pub(crate) fn opens_natively(url: &str, features: Option<&ChangeRequestFeatures>) -> bool {
    match features {
        Some(features) => features.native_detail,
        None => url::Url::parse(url).is_ok_and(|url| url.host_str() == Some(FALLBACK_HOST)),
    }
}

/// The actor's avatar image. An engine that sends presentation data sends
/// avatars too, so a missing one means the provider has none. Older engines
/// named GitHub accounts, whose public profile image endpoint redirects to
/// the avatar CDN; stable sizing lets repeated authors share GPUI's image
/// cache.
pub(crate) fn avatar_url(actor: &ChangeRequestActor, presented: bool) -> Option<String> {
    if let Some(url) = actor.avatar_url.as_deref().filter(|url| !url.is_empty()) {
        return Some(url.to_owned());
    }
    if presented || actor.login.is_empty() {
        return None;
    }
    let mut url = url::Url::parse("https://github.com/").ok()?;
    url.path_segments_mut()
        .ok()?
        .push(&format!("{}.png", actor.login));
    url.set_query(Some("size=64"));
    Some(url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_data_wins_and_older_engines_read_as_github() {
        let fallback = presentation(None);
        assert_eq!(fallback.provider_name, "GitHub");
        assert_eq!(fallback.sign_in_command, "gh auth login");
        let forge = ChangeRequestPresentation {
            provider_name: "Forge".into(),
            ..Default::default()
        };
        assert_eq!(presentation(Some(&forge)).provider_name, "Forge");

        assert_eq!(reference(12, None), "#12");
        assert_eq!(reference(7, Some("~7")), "~7");

        let github = "https://github.com/acme/zeron/pull/1";
        let enterprise = "https://github.example.com/acme/zeron/pull/1";
        assert!(opens_natively(github, None));
        assert!(!opens_natively(enterprise, None));
        let browser = ChangeRequestFeatures::default();
        assert!(!opens_natively(github, Some(&browser)));

        let actor = |login: &str, avatar: Option<&str>| ChangeRequestActor {
            login: login.into(),
            avatar_url: avatar.map(str::to_owned),
        };
        assert_eq!(
            avatar_url(&actor("devuser", None), false).as_deref(),
            Some("https://github.com/devuser.png?size=64")
        );
        assert_eq!(avatar_url(&actor("devuser", None), true), None);
        assert_eq!(avatar_url(&actor("", None), false), None);
        assert_eq!(
            avatar_url(&actor("devuser", Some("https://forge.test/a.png")), true).as_deref(),
            Some("https://forge.test/a.png")
        );
    }
}
