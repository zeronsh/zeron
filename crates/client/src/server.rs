//! The server this app signs in to: Zeron's own, or one someone runs
//! themselves (docs/self-hosting.md). The lookup is shared with the desktop
//! app (zeron_proto::server); this is the fetching.

use std::time::Duration;

pub use zeron_proto::server::{
    AddressError, NotFound as ServerLookupError, OFFICIAL_URL, SELF_HOSTING_GUIDE, Server, ServerError,
    host_of, normalize_address,
};
use zeron_proto::server::{Answer, look_up};

/// Longer than a slow server takes to answer, shorter than someone waits on
/// a "Connect" button.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(12);

/// The server at `address` (as typed), read from its discovery document
/// (zeron_proto::server::look_up), on the shared runtime.
pub async fn discover(address: &str) -> Result<Server, ServerLookupError> {
    let typed = address.to_owned();
    let looked_up =
        crate::runtime::run(async move { Ok(look_up(&typed, |url| async move { get(&url).await.ok() }).await) }).await;
    looked_up.unwrap_or_else(|_| {
        // The runtime didn't run it: nothing was asked.
        Err(ServerLookupError { host: host_of(address.trim()), error: ServerError::Unreachable })
    })
}

/// The answer to a GET of `url`.
async fn get(url: &str) -> reqwest::Result<Answer> {
    let response = crate::auth::http()
        .get(url)
        .header(reqwest::header::ACCEPT, "application/json")
        .timeout(LOOKUP_TIMEOUT)
        .send()
        .await?;
    let status = response.status().as_u16();
    let origin = response.url().origin().ascii_serialization();
    let body = response.text().await?;
    Ok(Answer { status, origin, body })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A raw HTTP/1.1 server answering each path from `routes`
    /// (`(path, status, body)`), 404 otherwise.
    async fn serve(routes: Vec<(&'static str, u16, &'static str)>) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let routes = routes.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let n = socket.read(&mut buf).await.unwrap_or(0);
                    let request = String::from_utf8_lossy(&buf[..n]);
                    let path = request.split_whitespace().nth(1).unwrap_or("/").to_owned();
                    let (status, body) = routes
                        .iter()
                        .find(|(p, ..)| *p == path)
                        .map(|(_, s, b)| (*s, *b))
                        .unwrap_or((404, "Not found"));
                    let response = format!(
                        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                });
            }
        });
        url
    }

    #[tokio::test]
    async fn finds_a_self_hosted_server() {
        let url = serve(vec![(
            "/.well-known/zeron",
            200,
            r#"{"zeron":1,"name":"Acme","auth":{"mode":"workos","workosClientId":"client_acme"}}"#,
        )])
        .await;
        let server = discover(&format!(" {url}/ ")).await.unwrap();
        assert_eq!(server.url, url);
        assert_eq!(server.workos_client_id, "client_acme");
        assert_eq!(server.name.as_deref(), Some("Acme"));
        assert!(!server.is_official());
    }

    #[tokio::test]
    async fn tells_an_old_zeron_server_from_a_stranger() {
        let old = serve(vec![("/health", 200, r#"{"ok":true,"auth":"workos"}"#)]).await;
        let error = discover(&old).await.unwrap_err();
        assert_eq!(error.error, ServerError::Outdated);
        assert_eq!(error.host, host_of(&old));

        let stranger = serve(vec![]).await;
        assert_eq!(discover(&stranger).await.unwrap_err().error, ServerError::NotZeron);
    }

    #[tokio::test]
    async fn says_what_a_half_set_up_server_is_missing() {
        let url = serve(vec![(
            "/.well-known/zeron",
            503,
            r#"{"zeron":1,"error":"sign_in_not_configured","missing":["WORKOS_API_KEY"]}"#,
        )])
        .await;
        let error = discover(&url).await.unwrap_err();
        assert!(error.message().ends_with("hasn't finished setting up sign-in (missing WORKOS_API_KEY)."));
    }

    #[tokio::test]
    async fn nothing_listening_is_unreachable() {
        // Bound then dropped: nothing listens there now.
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let error = discover(&format!("http://127.0.0.1:{port}")).await.unwrap_err();
        assert_eq!(error.error, ServerError::Unreachable);
        assert_eq!(error.message(), format!("Couldn't reach 127.0.0.1:{port}. Check the address and your connection."));
    }

    #[tokio::test]
    async fn the_official_address_needs_no_lookup() {
        assert_eq!(discover("edge.zeron.sh").await.unwrap(), Server::official());
    }

    #[tokio::test]
    async fn a_bad_address_is_refused_before_any_request() {
        let error = discover("http://zeron.example.com").await.unwrap_err();
        assert_eq!(error.error, ServerError::Address(AddressError::Insecure));
    }
}
