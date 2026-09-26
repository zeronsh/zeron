//! Keep transport causes when HTTP errors cross string-based RPC/sync seams.

use std::error::Error;

/// reqwest's Display omits its source chain, including DNS/TLS/socket errors.
/// Retain that chain and the destination origin, excluding URL credentials,
/// paths and queries that may contain tokens or other private values.
pub(crate) fn describe_http_error(error: reqwest::Error) -> String {
    zeron_sync::budget::shared().observe_error(&error);
    let origin = error.url().map(|url| url.origin().ascii_serialization());
    let error = error.without_url();
    let mut message = match origin {
        Some(origin) => format!("{origin}: {error}"),
        None => error.to_string(),
    };
    let mut source = error.source();
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    pub(crate) struct FailingDns {
        pub(crate) calls: AtomicUsize,
    }

    impl reqwest::dns::Resolve for FailingDns {
        fn resolve(&self, _: reqwest::dns::Name) -> reqwest::dns::Resolving {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Err(std::io::Error::other("injected DNS lookup failure").into()) })
        }
    }

    impl FailingDns {
        pub(crate) fn client(self: &Arc<Self>) -> reqwest::Client {
            reqwest::Client::builder()
                .no_proxy()
                .dns_resolver(self.clone())
                .build()
                .unwrap()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reports_dns_cause_without_url_secrets() {
        let dns = std::sync::Arc::new(test_support::FailingDns::default());
        let error = dns.client()
            .get("https://login-user:password-secret@edge.invalid/private-path?token=query-secret#fragment-secret")
            .send().await.unwrap_err();
        assert!(!error.to_string().contains("injected DNS lookup failure"));

        let message = describe_http_error(error);
        assert!(message.contains("https://edge.invalid"), "{message}");
        assert!(message.contains("injected DNS lookup failure"), "{message}");
        for secret in [
            "login-user",
            "password-secret",
            "private-path",
            "query-secret",
            "fragment-secret",
        ] {
            assert!(!message.contains(secret), "URL secret leaked: {message}");
        }
    }
}
