//! Finding the server at an address someone typed (the sign-in screen,
//! Settings › Account). zeron_proto::server looks it up, as it does for the
//! phones through zeron-client; this fetches.

use std::time::Duration;

pub use zeron_proto::server::NotFound;
use zeron_proto::server::{Answer, Server, origin_of};

/// Longer than a slow server takes to answer, shorter than someone waits on
/// "Connect".
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(12);

/// The server at `address` (zeron_proto::server::look_up). Blocking: for a
/// worker thread.
pub fn look_up(address: &str) -> Result<Server, NotFound> {
    let agent = ureq::AgentBuilder::new()
        .timeout(LOOKUP_TIMEOUT)
        .user_agent("zeron-zgx")
        .build();
    zeron_proto::server::look_up_blocking(address, |url| get(&agent, url))
}

/// The answer to a GET of `url`; None when nothing answered.
fn get(agent: &ureq::Agent, url: &str) -> Option<Answer> {
    let response = match agent.get(url).set("accept", "application/json").call() {
        Ok(response) | Err(ureq::Error::Status(_, response)) => response,
        Err(ureq::Error::Transport(_)) => return None,
    };
    let status = response.status();
    let origin = origin_of(response.get_url()).unwrap_or_else(|| url.to_owned());
    Some(Answer { status, origin, body: response.into_string().unwrap_or_default() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_proto::server::{ServerError, host_of};
    use std::io::{Read, Write};

    /// A raw HTTP/1.1 server answering each path from `routes`
    /// (`(path, status, body)`), 404 otherwise.
    fn serve(routes: &'static [(&'static str, u16, &'static str)]) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for mut socket in listener.incoming().flatten() {
                let mut buf = [0u8; 4096];
                let n = socket.read(&mut buf).unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]);
                let path = request.split_whitespace().nth(1).unwrap_or("/");
                let (status, body) =
                    routes.iter().find(|(p, ..)| *p == path).map_or((404, "Not found"), |(_, s, b)| (*s, *b));
                let _ = write!(
                    socket,
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        url
    }

    #[test]
    fn finds_a_self_hosted_server() {
        let url = serve(&[(
            "/.well-known/zeron",
            200,
            r#"{"zeron":1,"name":"Acme","auth":{"mode":"workos","workosClientId":"client_acme"}}"#,
        )]);
        let server = look_up(&format!(" {url}/ ")).unwrap();
        assert_eq!((server.url, server.workos_client_id, server.name), (url, "client_acme".into(), Some("Acme".into())));
    }

    #[test]
    fn tells_an_old_zeron_server_from_a_stranger() {
        let old = serve(&[("/health", 200, r#"{"ok":true,"auth":"workos"}"#)]);
        assert_eq!(look_up(&old).unwrap_err(), NotFound { host: host_of(&old), error: ServerError::Outdated });
        let stranger = serve(&[]);
        assert_eq!(look_up(&stranger).unwrap_err().error, ServerError::NotZeron);
    }

    #[test]
    fn nothing_listening_is_unreachable_and_zeron_needs_no_lookup() {
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        assert_eq!(look_up(&format!("http://127.0.0.1:{port}")).unwrap_err().error, ServerError::Unreachable);
        assert_eq!(look_up("edge.zeron.sh").unwrap(), Server::official());
        assert_eq!(
            look_up("http://zeron.example.com").unwrap_err().error,
            ServerError::Address(zeron_proto::server::AddressError::Insecure)
        );
    }
}
