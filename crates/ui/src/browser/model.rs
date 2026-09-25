//! Browser state shared by the chrome and platform host. No native handles.
use std::net::IpAddr;

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Deserialize)]
pub struct PageState {
    pub url: Option<String>,
    pub title: String,
    pub loading: bool,
    pub can_back: bool,
    pub can_forward: bool,
    pub error: Option<String>,
}

impl PageState {
    pub fn label(&self) -> String {
        if !self.title.trim().is_empty() {
            self.title.clone()
        } else {
            self.url
                .as_deref()
                .and_then(|s| url::Url::parse(s).ok())
                .and_then(|u| u.host_str().map(str::to_owned))
                .unwrap_or_else(|| "Browser".into())
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct InspectedElement {
    pub tag: String,
    pub id: String,
    pub classes: String,
    pub selector: String,
    pub text: String,
    #[serde(default)]
    pub dom_path: Option<String>,
    #[serde(default)]
    pub bounds: Option<String>,
    #[serde(default)]
    pub attributes: Vec<String>,
    #[serde(default)]
    pub screenshot: Option<Vec<u8>>,
    #[serde(default)]
    pub user_prompt: Option<String>,
    #[serde(default)]
    pub elements: Vec<InspectedElement>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ConsoleLogEntry {
    pub level: String,
    pub text: String,
    pub timestamp: u64,
}

impl InspectedElement {
    pub fn single_prompt_context(&self) -> String {
        let tag = if self.tag.is_empty() { "element" } else { &self.tag };
        let mut lines = vec![
            "@".to_string(),
            "```browser_element".to_string(),
            "The user selected this node in the browser preview (blue outline in the screenshot).".to_string(),
            String::new(),
            format!("tag: {tag}"),
        ];
        let dom_path = self
            .dom_path
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(if !self.selector.is_empty() { &self.selector } else { "" });
        if !dom_path.is_empty() {
            lines.push(format!("dom_path: {dom_path}"));
        }
        if !self.classes.is_empty() {
            lines.push(format!("class: {}", self.classes));
        }
        if !self.text.is_empty() {
            lines.push(format!("visible_text: {}", self.text));
        }
        if let Some(bounds) = &self.bounds {
            if !bounds.is_empty() {
                lines.push(format!("bounds_css_px: {bounds}"));
            }
        }
        if !self.attributes.is_empty() {
            lines.push("attributes:".to_string());
            for attr in &self.attributes {
                lines.push(format!("  {attr}"));
            }
        }
        lines.push("```".to_string());
        lines.join("\n")
    }

    pub fn to_prompt_context(&self) -> String {
        let mut result = if self.elements.len() > 1 {
            self.elements
                .iter()
                .map(|elem| elem.single_prompt_context())
                .collect::<Vec<_>>()
                .join("\n\n")
        } else {
            self.single_prompt_context()
        };
        if let Some(user_prompt) = &self.user_prompt {
            let trimmed = user_prompt.trim();
            if !trimmed.is_empty() {
                result.push_str("\n\n");
                result.push_str(trimmed);
            }
        }
        result
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

pub fn normalize_address(input: &str) -> Result<String, &'static str> {
    let text = input.trim();
    if text.is_empty() {
        return Err("Enter a website or localhost address.");
    }
    if text.chars().any(|c| c.is_control()) {
        return Err("This address contains invalid characters.");
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
    .map_err(|_| "Enter a valid website or localhost address.")?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err("Only http and https addresses are supported.");
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("Use an address without an embedded username or password.");
    }
    if !explicit && loopback(&parsed) {
        let _ = parsed.set_scheme("http");
    }
    Ok(parsed.into())
}

/// Chat links require an explicit web authority; never infer a scheme or
/// silently strip controls, credentials, malformed escapes or backslashes.
pub fn transcript_address(input: &str) -> Result<String, &'static str> {
    let lower = input.to_ascii_lowercase();
    let authority = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))
        .ok_or("Only explicit http and https links are supported.")?;
    if input.chars().any(|c| c.is_control() || c.is_whitespace())
        || input.contains('\\')
        || authority.is_empty()
        || authority.starts_with(['/', '?', '#'])
    {
        return Err("This link contains an invalid address.");
    }
    for (i, byte) in input.bytes().enumerate() {
        if byte == b'%'
            && !input
                .as_bytes()
                .get(i + 1..i + 3)
                .is_some_and(|hex| hex.iter().all(u8::is_ascii_hexdigit))
        {
            return Err("This link contains an invalid escape.");
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

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DesignElementColor {
    pub border: String,
    pub bg: String,
    pub text: String,
    pub shadow: String,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct DesignPopupTheme {
    pub bg: String,
    pub backdrop_filter: String,
    pub border: String,
    pub text: String,
    pub text_muted: String,
    pub tag_bg: String,
    pub tag_text: String,
    pub submit_bg: String,
    pub submit_color: String,
    pub box_shadow: String,
    pub palette: Vec<DesignElementColor>,
}

pub fn hsla_to_css(color: gpui::Hsla) -> String {
    let [r, g, b] = crate::theme::hsl_to_rgb(color.h, color.s, color.l);
    format!(
        "rgba({}, {}, {}, {:.3})",
        (r * 255.0).round() as u8,
        (g * 255.0).round() as u8,
        (b * 255.0).round() as u8,
        color.a
    )
}

impl DesignPopupTheme {
    pub fn from_theme(theme: &crate::theme::Theme) -> Self {
        let is_glass = theme.is_glass();
        let bg = hsla_to_css(theme.glass());
        let border = hsla_to_css(theme.border);
        let text = hsla_to_css(theme.text);
        let text_muted = hsla_to_css(theme.text_muted);
        let first_color = DesignElementColor {
            border: hsla_to_css(theme.accent),
            bg: hsla_to_css(theme.accent_wash),
            text: hsla_to_css(theme.accent),
            shadow: format!("0 0 0 1px {}", hsla_to_css(theme.accent.opacity(0.4))),
        };
        let is_dark = theme.appearance.is_dark();
        let rest = if is_dark {
            vec![
                DesignElementColor {
                    border: "rgba(129, 140, 248, 1.0)".into(),
                    bg: "rgba(129, 140, 248, 0.18)".into(),
                    text: "rgba(165, 180, 252, 1.0)".into(),
                    shadow: "0 0 0 1px rgba(129, 140, 248, 0.4)".into(),
                },
                DesignElementColor {
                    border: "rgba(52, 211, 153, 1.0)".into(),
                    bg: "rgba(52, 211, 153, 0.18)".into(),
                    text: "rgba(110, 231, 183, 1.0)".into(),
                    shadow: "0 0 0 1px rgba(52, 211, 153, 0.4)".into(),
                },
                DesignElementColor {
                    border: "rgba(251, 146, 60, 1.0)".into(),
                    bg: "rgba(251, 146, 60, 0.18)".into(),
                    text: "rgba(253, 186, 116, 1.0)".into(),
                    shadow: "0 0 0 1px rgba(251, 146, 60, 0.4)".into(),
                },
                DesignElementColor {
                    border: "rgba(244, 114, 182, 1.0)".into(),
                    bg: "rgba(244, 114, 182, 0.18)".into(),
                    text: "rgba(249, 168, 212, 1.0)".into(),
                    shadow: "0 0 0 1px rgba(244, 114, 182, 0.4)".into(),
                },
                DesignElementColor {
                    border: "rgba(56, 189, 248, 1.0)".into(),
                    bg: "rgba(56, 189, 248, 0.18)".into(),
                    text: "rgba(125, 211, 252, 1.0)".into(),
                    shadow: "0 0 0 1px rgba(56, 189, 248, 0.4)".into(),
                },
            ]
        } else {
            vec![
                DesignElementColor {
                    border: "rgba(99, 102, 241, 1.0)".into(),
                    bg: "rgba(99, 102, 241, 0.14)".into(),
                    text: "rgba(79, 70, 229, 1.0)".into(),
                    shadow: "0 0 0 1px rgba(99, 102, 241, 0.35)".into(),
                },
                DesignElementColor {
                    border: "rgba(16, 185, 129, 1.0)".into(),
                    bg: "rgba(16, 185, 129, 0.14)".into(),
                    text: "rgba(4, 120, 87, 1.0)".into(),
                    shadow: "0 0 0 1px rgba(16, 185, 129, 0.35)".into(),
                },
                DesignElementColor {
                    border: "rgba(249, 115, 22, 1.0)".into(),
                    bg: "rgba(249, 115, 22, 0.14)".into(),
                    text: "rgba(194, 65, 12, 1.0)".into(),
                    shadow: "0 0 0 1px rgba(249, 115, 22, 0.35)".into(),
                },
                DesignElementColor {
                    border: "rgba(236, 72, 153, 1.0)".into(),
                    bg: "rgba(236, 72, 153, 0.14)".into(),
                    text: "rgba(190, 24, 93, 1.0)".into(),
                    shadow: "0 0 0 1px rgba(236, 72, 153, 0.35)".into(),
                },
                DesignElementColor {
                    border: "rgba(2, 132, 199, 1.0)".into(),
                    bg: "rgba(2, 132, 199, 0.14)".into(),
                    text: "rgba(3, 105, 161, 1.0)".into(),
                    shadow: "0 0 0 1px rgba(2, 132, 199, 0.35)".into(),
                },
            ]
        };
        let tag_bg = first_color.bg.clone();
        let tag_text = first_color.text.clone();
        let mut palette = vec![first_color];
        palette.extend(rest);
        let submit_bg = hsla_to_css(theme.text);
        let submit_color = hsla_to_css(theme.bg);
        let (backdrop_filter, box_shadow) = if is_glass {
            (
                "blur(16px) saturate(180%)".to_string(),
                "0 8px 32px rgba(0, 0, 0, 0.35), 0 2px 8px rgba(0, 0, 0, 0.2)".to_string(),
            )
        } else {
            (
                "none".to_string(),
                "0 8px 24px rgba(0, 0, 0, 0.4)".to_string(),
            )
        };

        Self {
            bg,
            backdrop_filter,
            border,
            text,
            text_muted,
            tag_bg,
            tag_text,
            submit_bg,
            submit_color,
            box_shadow,
            palette,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_inspected_element_prompt_context() {
        let el = InspectedElement {
            tag: "button".into(),
            id: "submit".into(),
            classes: "btn btn-primary".into(),
            selector: "form > button#submit".into(),
            text: "Submit Form".into(),
            ..Default::default()
        };
        let ctx = el.to_prompt_context();
        assert!(ctx.starts_with("@\n```browser_element\n"));
        assert!(ctx.contains("tag: button"));
        assert!(ctx.contains("dom_path: form > button#submit"));
        assert!(ctx.contains("class: btn btn-primary"));
        assert!(ctx.contains("visible_text: Submit Form"));
        assert!(ctx.ends_with("```"));

        // Selector fallback when no id or classes
        let el_selector = InspectedElement {
            tag: "div".into(),
            id: "".into(),
            classes: "".into(),
            selector: "main > section:nth-of-type(2)".into(),
            text: "Hero Content".into(),
            ..Default::default()
        };
        let ctx_sel = el_selector.to_prompt_context();
        assert!(ctx_sel.contains("tag: div"));
        assert!(ctx_sel.contains("dom_path: main > section:nth-of-type(2)"));
        assert!(ctx_sel.contains("visible_text: Hero Content"));

        // Element without text
        let el_no_text = InspectedElement {
            tag: "input".into(),
            id: "search".into(),
            classes: "".into(),
            selector: "input#search".into(),
            text: "".into(),
            ..Default::default()
        };
        let ctx_no_text = el_no_text.to_prompt_context();
        assert!(ctx_no_text.contains("tag: input"));
        assert!(ctx_no_text.contains("dom_path: input#search"));
        assert!(!ctx_no_text.contains("visible_text"));

        // Serde roundtrip for IPC payload compatibility
        let json = r#"{"tag":"span","id":"badge","classes":"pill active","selector":"span#badge","text":"5","user_prompt":"change color"}"#;
        let parsed: InspectedElement = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.tag, "span");
        assert_eq!(parsed.id, "badge");
        assert_eq!(parsed.text, "5");
        assert_eq!(parsed.user_prompt.as_deref(), Some("change color"));
    }
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
        assert_eq!(page.label(), "Browser");
        page.url = Some("http://localhost:3000/path".into());
        assert_eq!(page.label(), "localhost");
        page.title = "Local preview".into();
        assert_eq!(page.label(), "Local preview");
    }

    #[test]
    fn console_log_entry_serde_and_formatting() {
        let entry = ConsoleLogEntry {
            level: "error".into(),
            text: "Uncaught TypeError: Cannot read property of undefined".into(),
            timestamp: 123456789,
        };
        let serialized = serde_json::to_string(&entry).unwrap();
        let deserialized: ConsoleLogEntry = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized.level, "error");
        assert_eq!(deserialized.text, "Uncaught TypeError: Cannot read property of undefined");
        assert_eq!(deserialized.timestamp, 123456789);
    }

    #[test]
    fn design_popup_theme_glass_and_opaque() {
        let mut theme = crate::theme::Theme::dark();
        theme.surface_treatment = zeron_theme::SurfaceTreatment::Frosted;
        let frosted = DesignPopupTheme::from_theme(&theme);
        assert!(frosted.backdrop_filter.contains("blur"));
        assert!(frosted.bg.contains("rgba"));
        assert!(!frosted.palette.is_empty());
        // First color matches theme accent
        assert_eq!(frosted.palette[0].border, hsla_to_css(theme.accent));
        assert_eq!(frosted.palette[0].text, hsla_to_css(theme.accent));
        assert_eq!(frosted.palette[0].bg, hsla_to_css(theme.accent_wash));

        theme.surface_treatment = zeron_theme::SurfaceTreatment::Opaque;
        let opaque = DesignPopupTheme::from_theme(&theme);
        assert_eq!(opaque.backdrop_filter, "none");
        assert_eq!(opaque.palette[0].border, hsla_to_css(theme.accent));
    }

    #[test]
    fn multi_element_prompt_context() {
        let el1 = InspectedElement {
            tag: "h1".into(),
            id: "title".into(),
            classes: "".into(),
            selector: "h1#title".into(),
            text: "Main Heading".into(),
            ..Default::default()
        };
        let el2 = InspectedElement {
            tag: "a".into(),
            id: "".into(),
            classes: "nav-link".into(),
            selector: "nav > a.nav-link".into(),
            text: "Documentation".into(),
            ..Default::default()
        };
        let multi = InspectedElement {
            tag: el1.tag.clone(),
            id: el1.id.clone(),
            classes: el1.classes.clone(),
            selector: el1.selector.clone(),
            text: el1.text.clone(),
            user_prompt: Some("adjust styling".into()),
            elements: vec![el1, el2],
            ..Default::default()
        };
        let ctx = multi.to_prompt_context();
        assert!(ctx.contains("tag: h1"));
        assert!(ctx.contains("dom_path: h1#title"));
        assert!(ctx.contains("visible_text: Main Heading"));
        assert!(ctx.contains("tag: a"));
        assert!(ctx.contains("dom_path: nav > a.nav-link"));
        assert!(ctx.contains("class: nav-link"));
        assert!(ctx.contains("visible_text: Documentation"));
    }
}
