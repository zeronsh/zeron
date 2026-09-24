//! Browser state shared by the chrome and platform host. No native handles.
use crate::i18n::{self, Locale, MessageId};
use std::net::IpAddr;

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Deserialize)]
pub struct PageState {
    pub url: Option<String>,
    pub title: String,
    pub loading: bool,
    pub can_back: bool,
    pub can_forward: bool,
    pub error: Option<PageFailure>,
}

impl PageState {
    pub fn label(&self, locale: Locale) -> String {
        if !self.title.trim().is_empty() {
            self.title.clone()
        } else {
            self.url
                .as_deref()
                .and_then(|s| url::Url::parse(s).ok())
                .and_then(|u| u.host_str().map(str::to_owned))
                .unwrap_or_else(|| i18n::translate(MessageId::SurfaceBrowser, locale).to_owned())
        }
    }
}

/// Why a page cannot be shown. The pane renders this, so a failure the UI or a
/// platform host authors carries a key and re-renders when the language
/// changes; `Detail` is text a platform or engine layer reported verbatim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PageFailure {
    Copy(MessageId, Vec<(&'static str, String)>),
    Detail(String),
}

impl PageFailure {
    pub fn message(id: MessageId) -> Self {
        Self::Copy(id, Vec::new())
    }
    pub fn with(mut self, placeholder: &'static str, value: impl Into<String>) -> Self {
        if let Self::Copy(_, values) = &mut self {
            values.push((placeholder, value.into()));
        }
        self
    }
    pub fn detail(text: impl Into<String>) -> Self {
        Self::Detail(text.into())
    }
    pub fn text(&self, locale: Locale) -> String {
        match self {
            Self::Copy(id, values) => {
                let values: Vec<(&str, &str)> = values
                    .iter()
                    .map(|(key, value)| (*key, value.as_str()))
                    .collect();
                i18n::fill_many(*id, &values, locale)
            }
            Self::Detail(detail) => detail.clone(),
        }
    }
    /// An open failure states that the page could not be opened and quotes what
    /// the platform reported; copy we authored is already a whole sentence.
    pub fn into_open_failure(self) -> Self {
        match self {
            Self::Detail(detail) => {
                Self::message(MessageId::BrowserOpenFailed).with("{error}", detail)
            }
            copy => copy,
        }
    }
}

/// The platform helper reports an engine's own failure text as a string.
impl<'de> serde::Deserialize<'de> for PageFailure {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        <String as serde::Deserialize>::deserialize(deserializer).map(Self::Detail)
    }
}

pub fn loopback(url: &url::Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(host)) => host == "localhost" || host.ends_with(".localhost"),
        Some(url::Host::Ipv4(ip)) => IpAddr::V4(ip).is_loopback(),
        Some(url::Host::Ipv6(ip)) => IpAddr::V6(ip).is_loopback(),
        None => false,
    }
}

pub fn normalize_address(input: &str) -> Result<String, MessageId> {
    let text = input.trim();
    if text.is_empty() {
        return Err(MessageId::BrowserAddressEmpty);
    }
    if text.chars().any(|c| c.is_control()) {
        return Err(MessageId::BrowserAddressInvalidCharacters);
    }
    // A bare host:port looks like a URI scheme to a URL parser. Only accept
    // that ambiguity when the suffix is an actual numeric port.
    let authority = text.split(['/', '?', '#']).next().unwrap_or(text);
    let host_port = authority
        .rsplit_once(':')
        .is_some_and(|(_, port)| !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()));
    let explicit =
        text.contains("://") || (text.contains(':') && !host_port && !text.starts_with('['));
    let mut parsed = url::Url::parse(&if explicit {
        text.to_owned()
    } else {
        format!("https://{text}")
    })
    .map_err(|_| MessageId::BrowserAddressInvalid)?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(MessageId::BrowserAddressSchemeUnsupported);
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(MessageId::BrowserAddressCredentials);
    }
    if !explicit && loopback(&parsed) {
        let _ = parsed.set_scheme("http");
    }
    Ok(parsed.into())
}

/// Chat links require an explicit web authority; never infer a scheme or
/// silently strip controls, credentials, malformed escapes or backslashes.
pub fn transcript_address(input: &str) -> Result<String, MessageId> {
    let lower = input.to_ascii_lowercase();
    let authority = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))
        .ok_or(MessageId::BrowserLinkUnsupported)?;
    if input.chars().any(|c| c.is_control() || c.is_whitespace())
        || input.contains('\\')
        || authority.is_empty()
        || authority.starts_with(['/', '?', '#'])
    {
        return Err(MessageId::BrowserLinkInvalid);
    }
    for (i, byte) in input.bytes().enumerate() {
        if byte == b'%'
            && !input
                .as_bytes()
                .get(i + 1..i + 3)
                .is_some_and(|hex| hex.iter().all(u8::is_ascii_hexdigit))
        {
            return Err(MessageId::BrowserLinkInvalidEscape);
        }
    }
    normalize_address(input)
}

pub fn allowed_navigation(address: &str) -> bool {
    url::Url::parse(address).is_ok_and(|u| {
        matches!(u.scheme(), "http" | "https")
            && u.host_str().is_some()
            && u.username().is_empty()
            && u.password().is_none()
    })
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Presentation {
    #[default]
    Hidden,
    Live,
    /// Keep rendering while GPUI owns drag input.
    Passthrough,
}

pub fn presentation(active: bool, dragging: bool) -> Presentation {
    if !active {
        Presentation::Hidden
    } else if dragging {
        Presentation::Passthrough
    } else {
        Presentation::Live
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalizes_web_addresses_and_loopback_ports() {
        for (input, expected) in [
            ("localhost:3000", "http://localhost:3000/"),
            ("127.0.0.1:5173/a?x=1", "http://127.0.0.1:5173/a?x=1"),
            ("[::1]:8080", "http://[::1]:8080/"),
            ("[::1]", "http://[::1]/"),
            (" app.localhost:3000 ", "http://app.localhost:3000/"),
            ("example.com/path", "https://example.com/path"),
            ("https://localhost:3000", "https://localhost:3000/"),
            ("http://example.com", "http://example.com/"),
        ] {
            assert_eq!(normalize_address(input), Ok(expected.into()), "{input}");
        }
    }
    #[test]
    fn rejects_non_web_schemes_credentials_and_bad_input() {
        for input in [
            "",
            "javascript:alert(1)",
            "file:///tmp/a",
            "data:text/html,hi",
            "zeron://open/chat/a",
            "https://user:pass@example.com",
            "https://",
            "two words",
            "https://example.com/\nsecret",
        ] {
            assert!(normalize_address(input).is_err(), "{input}");
        }
        assert!(!allowed_navigation("javascript:alert(1)"));
        assert!(!allowed_navigation("https://user@example.com/"));
    }
    #[test]
    fn native_visibility_never_outlives_its_surface() {
        assert_eq!(presentation(false, false), Presentation::Hidden);
        assert_eq!(presentation(false, true), Presentation::Hidden);
        assert_eq!(presentation(true, true), Presentation::Passthrough);
        assert_eq!(presentation(true, false), Presentation::Live);
    }
    #[test]
    fn tab_labels_fall_back_to_host_then_browser() {
        let mut page = PageState::default();
        assert_eq!(page.label(Locale::En), "Browser");
        assert_eq!(page.label(Locale::ZhCn), "浏览器");
        page.url = Some("http://localhost:3000/path".into());
        assert_eq!(page.label(Locale::En), "localhost");
        page.title = "Local preview".into();
        assert_eq!(
            page.label(Locale::ZhCn),
            "Local preview",
            "titles stay verbatim"
        );
    }

    #[test]
    fn page_failures_render_in_the_active_locale() {
        let authored = PageFailure::message(MessageId::BrowserHelperStopped);
        assert_eq!(
            authored.text(Locale::En),
            MessageId::BrowserHelperStopped.english()
        );
        assert_ne!(authored.text(Locale::ZhCn), authored.text(Locale::En));

        let templated = PageFailure::message(MessageId::BrowserWebkitStartFailed)
            .with("{error}", "no /usr/lib");
        assert!(templated.text(Locale::En).contains("no /usr/lib"));
        assert!(templated.text(Locale::ZhCn).contains("no /usr/lib"));

        let engine = PageFailure::detail("net::ERR_CONNECTION_REFUSED");
        assert_eq!(engine.text(Locale::ZhCn), "net::ERR_CONNECTION_REFUSED");
        assert_eq!(
            engine.into_open_failure().text(Locale::En),
            "Could not open this page: net::ERR_CONNECTION_REFUSED"
        );
        assert_eq!(
            PageFailure::detail("net::ERR_FAILED")
                .into_open_failure()
                .text(Locale::ZhCn),
            "无法打开此页面：net::ERR_FAILED"
        );
    }

    #[test]
    fn helper_error_text_deserializes_as_a_detail() {
        let page: PageState = serde_json::from_str(
            r#"{"url":null,"title":"","loading":false,"can_back":false,"can_forward":false,
                "error":"Could not connect: Connection refused"}"#,
        )
        .expect("snapshot");
        assert_eq!(
            page.error,
            Some(PageFailure::detail("Could not connect: Connection refused"))
        );
        let cleared: PageState = serde_json::from_str(
            r#"{"url":null,"title":"","loading":false,"can_back":false,"can_forward":false,
                "error":null}"#,
        )
        .expect("snapshot");
        assert_eq!(cleared.error, None);
    }
}
