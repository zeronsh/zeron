//! Original content and validated navigation are deliberately separate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkTarget {
    pub label: String,
    pub original: String,
    pub navigation: Result<String, &'static str>,
    pub preview: Option<zeron_markdown::content_reference::HtmlPreview>,
}
impl LinkTarget {
    pub fn new(label: &str, original: &str) -> Self {
        let preview = zeron_markdown::content_reference::html_preview(original);
        Self {
            label: label.into(),
            original: preview
                .as_ref()
                .map_or(original, |preview| &preview.path)
                .into(),
            navigation: crate::browser::model::transcript_address(original),
            preview,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkAction {
    /// Normal click or keyboard activation; the user's persisted preference
    /// decides whether this routes internally or externally.
    Primary,
    /// Explicit "Open in Zeron" context-menu action.
    Internal,
    External,
    Copy,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkOutcome {
    Internal,
    External(String),
    Rejected,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkActivation {
    pub target: LinkTarget,
    pub action: LinkAction,
    pub source_session: Option<String>,
}
impl LinkActivation {
    pub fn web_outcome(&self, embedded: bool) -> LinkOutcome {
        match &self.target.navigation {
            Ok(_) if embedded && self.action == LinkAction::Internal => LinkOutcome::Internal,
            Ok(url) => LinkOutcome::External(url.clone()),
            Err(_) => LinkOutcome::Rejected,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn html_preview_retains_the_file_and_rejects_external_navigation() {
        let target = LinkTarget::new(
            "Preview",
            "\u{e200}visualize\u{e202}{\"path\":\"/tmp/test.html\",\"mode\":\"wide\"}\u{e201}",
        );
        assert_eq!(target.original, "/tmp/test.html");
        assert!(target.preview.as_ref().unwrap().wide);
        let activation = LinkActivation {
            target,
            action: LinkAction::External,
            source_session: Some("owner".into()),
        };
        assert_eq!(activation.web_outcome(true), LinkOutcome::Rejected);
        assert_eq!(activation.web_outcome(false), LinkOutcome::Rejected);
    }
    #[test]
    fn internal_external_and_unsupported_platform_routes() {
        let mut a = LinkActivation {
            target: LinkTarget::new("label", "https://example.com"),
            action: LinkAction::Internal,
            source_session: Some("owner".into()),
        };
        assert_eq!(a.web_outcome(true), LinkOutcome::Internal);
        assert_eq!(
            a.web_outcome(false),
            LinkOutcome::External("https://example.com/".into())
        );
        a.action = LinkAction::External;
        assert_eq!(
            a.web_outcome(true),
            LinkOutcome::External("https://example.com/".into())
        );
        a.action = LinkAction::Primary;
        assert_eq!(
            a.web_outcome(true),
            LinkOutcome::External("https://example.com/".into()),
            "Primary must be resolved by the owning surface"
        );
    }
    #[test]
    fn rejected_targets_never_fall_back() {
        for url in [
            "javascript:alert(1)",
            "data:text/plain,hi",
            "mailto:a@b.com",
            "https://user:pass@example.com",
            "https://example.com/\npath",
            "\nhttps://example.com",
            "https://",
            "example.com",
            "https:///path",
            "https://example.com/%GG",
        ] {
            let a = LinkActivation {
                target: LinkTarget::new("safe label", url),
                action: LinkAction::Internal,
                source_session: Some("chat".into()),
            };
            assert_eq!(a.web_outcome(false), LinkOutcome::Rejected, "{url}");
            assert_eq!(a.web_outcome(true), LinkOutcome::Rejected, "{url}");
        }
    }
    #[test]
    fn labels_and_original_destinations_survive_normalization() {
        let t = LinkTarget::new("Documentation", "HTTPS://EXAMPLE.COM");
        assert_eq!(t.label, "Documentation");
        assert_eq!(t.original, "HTTPS://EXAMPLE.COM");
        assert_eq!(t.navigation, Ok("https://example.com/".into()));
    }
    #[test]
    fn markdown_and_autolinks_share_validation() {
        let tree = super::super::parser::parse_full(
            "[label](https://example.com) and https://example.org/path",
        );
        let super::super::parser::Block::Paragraph { runs } = &tree.blocks[0].block else {
            panic!("paragraph")
        };
        let urls: Vec<_> = runs.iter().filter_map(|r| r.style.link.as_ref()).collect();
        assert_eq!(urls.len(), 2);
        assert!(
            urls.iter()
                .all(|u| LinkTarget::new("", u).navigation.is_ok())
        );
    }
}
