//! In-memory documents are separate from website navigation and login profiles.
use zeron_markdown::content_reference::{HtmlCompatibility, HtmlPreview};

pub(super) struct HtmlDocument {
    pub path: String,
    pub title: String,
    pub html: String,
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

impl HtmlDocument {
    pub fn new(request: &HtmlPreview, content: &str) -> Result<Self, String> {
        let title = request
            .path
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or("HTML preview")
            .to_owned();
        let runtime = match request.compatibility {
            HtmlCompatibility::None => "",
            HtmlCompatibility::CodexVisualize => include_str!("visualize.js"),
        };
        let controls = match request.compatibility {
            HtmlCompatibility::None => "",
            HtmlCompatibility::CodexVisualize => include_str!("visualize-controls.js"),
        };
        // The iframe has a unique origin, scripts only, no host API. The CSP
        // also applies to srcdoc: content cannot relax it with its own meta tag.
        let child = format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><style>:root{{color-scheme:light dark}}body{{margin:16px;font:14px system-ui}}[hidden]{{display:none!important}}</style><script>{controls}</script><script>{runtime}</script></head><body>{content}</body></html>"
        );
        let width = if request.wide { "100%" } else { "960px" };
        let html = format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"><meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data: blob:; font-src data:; frame-src about:; connect-src 'none'; base-uri 'none'; form-action 'none'\"><title>{}</title><style>:root{{color-scheme:light dark}}html,body{{margin:0;height:100%;overflow:hidden}}iframe{{display:block;width:100%;max-width:{width};height:100%;margin:auto;border:0}}</style></head><body><iframe title=\"{}\" sandbox=\"allow-scripts\" srcdoc=\"{}\"></iframe></body></html>",
            escape(&title),
            escape(&title),
            escape(&child)
        );
        // WebView2 NavigateToString has a 2 MiB limit. Bound the final encoded
        // document, including escaping, consistently on every platform.
        if html.encode_utf16().count() > 900_000 {
            return Err("This HTML preview is too large. Open the source file instead.".into());
        }
        Ok(Self {
            path: request.path.clone(),
            title,
            html,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn documents_escape_the_frame_boundary_and_bound_the_encoded_size() {
        let request = HtmlPreview {
            path: "/tmp/test.html".into(),
            wide: true,
            compatibility: HtmlCompatibility::None,
        };
        let doc = HtmlDocument::new(
            &request,
            "</iframe><script>parent.document.body.remove()</script>",
        )
        .unwrap();
        assert_eq!(doc.html.matches("<iframe ").count(), 1);
        assert!(doc.html.contains("&lt;/iframe&gt;"));
        assert!(doc.html.contains("sandbox=\"allow-scripts\""));
        assert!(HtmlDocument::new(&request, &"&".repeat(250_000)).is_err());
    }
}
