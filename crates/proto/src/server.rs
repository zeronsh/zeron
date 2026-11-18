//! Which Zeron server an app talks to — the official one or a self-hosted
//! edge — and how an app learns to sign in to one (docs/self-hosting.md).
//!
//! Someone points an app at a server by typing its address and nothing else.
//! [`normalize_address`] turns what they typed into the server's origin, or
//! says what is wrong with it. The app then fetches the server's discovery
//! document (`GET /.well-known/zeron`, edge/src/discovery.ts) and
//! [`read_discovery`] reads the answer: the server, with the WorkOS client its
//! sign-in goes through, or why the app can't use it.
//!
//! Fetching is the caller's (each app has its own HTTP stack). Reading is
//! here, so the desktop app, the phones and the CLI accept and refuse the same
//! servers, in the same words.

use std::fmt;
use std::future::Future;
use std::net::Ipv4Addr;
use std::path::Path;
use std::task::{Context, Poll, Waker};

use serde::{Deserialize, Serialize};
use url::{Host, Url};

/// The official server (edge/wrangler.jsonc, `production`).
pub const OFFICIAL_URL: &str = "https://edge.zeron.sh";
/// Its WorkOS AuthKit client. Public: it is in every authorize URL.
pub const OFFICIAL_WORKOS_CLIENT_ID: &str = "client_01KWD0EAKZKD50YCQJNYSRE4BY";
/// What the official server is called in the apps.
pub const OFFICIAL_NAME: &str = "Zeron";

/// Where a server describes itself.
pub const DISCOVERY_PATH: &str = "/.well-known/zeron";
/// The discovery format this build reads (edge/src/discovery.ts,
/// `DISCOVERY_VERSION`). A server on a later one needs a newer app.
pub const DISCOVERY_VERSION: u64 = 1;

/// How to run a server of one's own, for the apps' "learn more" links.
pub const SELF_HOSTING_GUIDE: &str =
    "https://github.com/zeronsh/zeron/blob/main/docs/self-hosting.md";

/// The loopback port a desktop sign-in returns to
/// (`http://127.0.0.1:27641/callback`): the one every server's WorkOS client
/// allows, Zeron's and, by the guide, a self-hosted one's.
pub const CALLBACK_PORT: u16 = 27641;

/// The file in an engine's data dir naming the server chosen for it
/// ([`saved`], [`save`]).
pub const SAVED_FILE: &str = "server.json";

/// A server an app can sign in to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Server {
    /// The origin: `https://host[:port]`, no path, no trailing slash.
    pub url: String,
    /// The WorkOS AuthKit client the server's sign-in goes through.
    pub workos_client_id: String,
    /// What its operator calls it (`SERVER_NAME`), when they named it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl Server {
    /// Zeron's own server.
    pub fn official() -> Self {
        Self {
            url: OFFICIAL_URL.to_owned(),
            workos_client_id: OFFICIAL_WORKOS_CLIENT_ID.to_owned(),
            name: Some(OFFICIAL_NAME.to_owned()),
        }
    }

    /// Whether this is Zeron's own server (by address: a client id alone
    /// doesn't make a server official).
    pub fn is_official(&self) -> bool {
        normalize_address(&self.url).is_ok_and(|url| url == OFFICIAL_URL)
    }

    /// `host[:port]`, for showing the server where a name won't do.
    pub fn host(&self) -> String {
        host_of(&self.url)
    }

    /// What to call the server in a sentence: its name, else its host.
    pub fn label(&self) -> String {
        self.name.clone().unwrap_or_else(|| self.host())
    }
}

/// The longest name a server is shown by, in characters (past it, cut with
/// an ellipsis).
pub const NAME_MAX: usize = 64;

/// A server's name as every app shows it: its operator's words on one line.
/// Each run of white space or control characters is one space, and the
/// marks that reorder text around them (bidirectional overrides) are gone,
/// so a name can't break a layout or pass itself off as another; at most
/// [`NAME_MAX`] characters. None when nothing is left.
pub fn tidy_name(raw: &str) -> Option<String> {
    let reorders = |c: char| matches!(c, '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}');
    let kept: String = raw.chars().filter(|&c| !reorders(c)).collect();
    let line = kept
        .split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if line.is_empty() {
        return None;
    }
    Some(match line.char_indices().nth(NAME_MAX) {
        Some((at, _)) => format!("{}…", line[..at].trim_end()),
        None => line,
    })
}

/// The origin of a full URL (an answer's, after redirects), to read as an
/// address again; None when it doesn't parse.
pub fn origin_of(url: &str) -> Option<String> {
    Url::parse(url).ok().map(|url| url.origin().ascii_serialization())
}

/// `host[:port]` of an address (the address itself when it doesn't parse).
pub fn host_of(address: &str) -> String {
    let Ok(url) = Url::parse(address) else {
        return address.to_owned();
    };
    match (url.host_str(), url.port()) {
        (Some(host), Some(port)) => format!("{host}:{port}"),
        (Some(host), None) => host.to_owned(),
        _ => address.to_owned(),
    }
}

/// What's wrong with an address someone typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressError {
    Empty,
    /// Not a web address at all.
    Invalid,
    /// A scheme other than `https://` (or `http://` nearby).
    Scheme,
    /// `user:password@` in it.
    Credentials,
    /// A path, query or fragment after the host.
    Path,
    /// `http://` to a host outside the local network.
    Insecure,
}

/// The server's origin for an address someone typed: trimmed, `https://`
/// when no scheme is given, lowercased, no trailing slash.
///
/// Plain `http://` only reaches hosts on this device or its network (a server
/// in the next room, a Tailscale peer): a sign-in elsewhere goes over TLS.
/// A server lives at the root of its host, as every route the apps call does.
pub fn normalize_address(input: &str) -> Result<String, AddressError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(AddressError::Empty);
    }
    if input.chars().any(char::is_whitespace) {
        return Err(AddressError::Invalid);
    }
    // `zeron.example.com:8443` would otherwise parse as the scheme
    // `zeron.example.com`; and a `://` further on (`example.com/?next=https://…`)
    // isn't one.
    let has_scheme = input.split_once("://").is_some_and(|(scheme, _)| {
        scheme.starts_with(|c: char| c.is_ascii_alphabetic())
            && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    });
    let full = if has_scheme {
        input.to_owned()
    } else {
        format!("https://{input}")
    };
    let url = Url::parse(&full).map_err(|_| AddressError::Invalid)?;
    if !matches!(url.scheme(), "https" | "http") {
        return Err(AddressError::Scheme);
    }
    let Some(host) = url.host() else {
        return Err(AddressError::Invalid);
    };
    // url takes empty labels (`zeron..example.com`, `.example.com`); no
    // resolver does.
    if let Host::Domain(domain) = &host
        && (domain.starts_with('.') || domain.contains(".."))
    {
        return Err(AddressError::Invalid);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(AddressError::Credentials);
    }
    if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
        return Err(AddressError::Path);
    }
    if url.scheme() == "http" && !is_local(&host) {
        return Err(AddressError::Insecure);
    }
    Ok(url.origin().ascii_serialization())
}

/// On this device or its network: loopback, private and link-local ranges,
/// Tailscale's CGNAT range, and names only a local resolver answers.
fn is_local(host: &Host<&str>) -> bool {
    match host {
        Host::Domain(domain) => {
            let domain = domain.trim_end_matches('.');
            !domain.contains('.')
                || [".localhost", ".local", ".internal", ".lan", ".home.arpa"]
                    .iter()
                    .any(|suffix| domain.ends_with(suffix))
        }
        Host::Ipv4(ip) => is_local_v4(*ip),
        Host::Ipv6(ip) => {
            ip.is_loopback()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
                || ip.to_ipv4_mapped().is_some_and(is_local_v4)
        }
    }
}

fn is_local_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    ip.is_loopback() || ip.is_private() || ip.is_link_local() || (a == 100 && (64..128).contains(&b))
}

/// Why an app can't use a server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerError {
    /// What was typed isn't a usable address.
    Address(AddressError),
    /// No answer: wrong address, server down, no network, a bad certificate.
    Unreachable,
    /// Something answered, but not a Zeron server.
    NotZeron,
    /// A Zeron server from before discovery: its operator needs to update it.
    Outdated,
    /// A server on a discovery format (or sign-in) this build doesn't know.
    NeedsNewerApp,
    /// A development server (`AUTH_MODE=dev`): anyone could sign in as anyone.
    DevServer,
    /// Sign-in isn't set up on the server; the settings it is missing.
    NotConfigured { missing: Vec<String> },
    /// The server answered with an error of its own.
    Unavailable { status: u16 },
}

impl ServerError {
    /// A stable name for the case, for apps that word it themselves
    /// (translations).
    pub fn code(&self) -> &'static str {
        match self {
            Self::Address(AddressError::Empty) => "empty",
            Self::Address(AddressError::Invalid) => "invalid",
            Self::Address(AddressError::Scheme) => "scheme",
            Self::Address(AddressError::Credentials) => "credentials",
            Self::Address(AddressError::Path) => "path",
            Self::Address(AddressError::Insecure) => "insecure",
            Self::Unreachable => "unreachable",
            Self::NotZeron => "not_zeron",
            Self::Outdated => "outdated",
            Self::NeedsNewerApp => "needs_newer_app",
            Self::DevServer => "dev_server",
            Self::NotConfigured { .. } => "not_configured",
            Self::Unavailable { .. } => "unavailable",
        }
    }

    /// What to tell the person who typed `host`, in a sentence or two.
    pub fn message(&self, host: &str) -> String {
        match self {
            Self::Address(AddressError::Empty) => "Enter your server's address.".into(),
            Self::Address(AddressError::Invalid) => {
                "That isn't a server address. It looks like zeron.example.com.".into()
            }
            Self::Address(AddressError::Scheme) => "Use an https:// address.".into(),
            Self::Address(AddressError::Credentials) => {
                "Leave the user name and password out of the address.".into()
            }
            Self::Address(AddressError::Path) => {
                "Enter the server's address alone, like zeron.example.com, with nothing after it."
                    .into()
            }
            Self::Address(AddressError::Insecure) => {
                "Use https://. Zeron connects over http:// only to servers on your own network."
                    .into()
            }
            Self::Unreachable => {
                format!("Couldn't reach {host}. Check the address and your connection.")
            }
            Self::NotZeron => format!("{host} isn't a Zeron server."),
            Self::Outdated => {
                format!("{host} runs an older Zeron server. Its owner needs to update it.")
            }
            Self::NeedsNewerApp => {
                format!("{host} needs a newer version of Zeron. Update the app to connect.")
            }
            Self::DevServer => format!(
                "{host} is a development server, which apps can't sign in to. It needs WorkOS sign-in set up."
            ),
            Self::NotConfigured { missing } if missing.is_empty() => {
                format!("{host} hasn't finished setting up sign-in.")
            }
            Self::NotConfigured { missing } => format!(
                "{host} hasn't finished setting up sign-in (missing {}).",
                missing.join(", ")
            ),
            Self::Unavailable { status } => {
                format!("{host} isn't answering right now (error {status}). Try again in a moment.")
            }
        }
    }
}

impl From<AddressError> for ServerError {
    fn from(error: AddressError) -> Self {
        Self::Address(error)
    }
}

impl fmt::Display for AddressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&ServerError::Address(*self).message(""))
    }
}

impl std::error::Error for AddressError {}

/// What a discovery answer says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Discovered {
    Server(Server),
    Failed(ServerError),
    /// No discovery document: an older Zeron server, or not Zeron at all.
    /// Fetch `/health` and let [`read_health`] tell.
    AskHealth,
}

/// Read the answer to `GET {url}/.well-known/zeron` (`url` normalized).
pub fn read_discovery(url: &str, status: u16, body: &str) -> Discovered {
    let document = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .filter(|value| value.get("zeron").is_some());
    let Some(document) = document else {
        return match status {
            // Not found, or a router that only knows other methods.
            404 | 405 | 410 => Discovered::AskHealth,
            // A proxy or platform page in front of the server.
            500..=599 => Discovered::Failed(ServerError::Unavailable { status }),
            _ => Discovered::Failed(ServerError::NotZeron),
        };
    };
    let Some(version) = document["zeron"].as_u64().filter(|v| *v >= 1) else {
        return Discovered::Failed(ServerError::NotZeron);
    };
    if version > DISCOVERY_VERSION {
        return Discovered::Failed(ServerError::NeedsNewerApp);
    }
    if document["error"].as_str() == Some("sign_in_not_configured") {
        let missing = document["missing"]
            .as_array()
            .map(|names| {
                names
                    .iter()
                    .filter_map(|name| name.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        return Discovered::Failed(ServerError::NotConfigured { missing });
    }
    if !(200..300).contains(&status) {
        return Discovered::Failed(ServerError::Unavailable { status });
    }
    let auth = &document["auth"];
    match auth["mode"].as_str() {
        Some("workos") => match auth["workosClientId"].as_str().map(str::trim) {
            Some(client) if !client.is_empty() => Discovered::Server(Server {
                url: url.to_owned(),
                workos_client_id: client.to_owned(),
                name: document["name"].as_str().and_then(tidy_name),
            }),
            _ => Discovered::Failed(ServerError::NotZeron),
        },
        Some("dev") => Discovered::Failed(ServerError::DevServer),
        // A sign-in this build doesn't know.
        Some(_) => Discovered::Failed(ServerError::NeedsNewerApp),
        None => Discovered::Failed(ServerError::NotZeron),
    }
}

/// Read the answer to `GET {url}/health` after a server had no discovery
/// document: a Zeron edge answers `{"ok":true,"auth":…}`.
pub fn read_health(status: u16, body: &str) -> ServerError {
    let zeron = (200..300).contains(&status)
        && serde_json::from_str::<serde_json::Value>(body).is_ok_and(|health| {
            health["ok"] == serde_json::Value::Bool(true) && health["auth"].is_string()
        });
    if zeron {
        ServerError::Outdated
    } else {
        ServerError::NotZeron
    }
}

/// The URL that answers whether `url` (normalized) is a Zeron server.
pub fn discovery_url(url: &str) -> String {
    format!("{url}{DISCOVERY_PATH}")
}

/// `url`'s health route, asked when it has no discovery document.
pub fn health_url(url: &str) -> String {
    format!("{url}/health")
}

/// Why the server at an address can't be used, and the host to name in
/// saying so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotFound {
    pub host: String,
    pub error: ServerError,
}

impl NotFound {
    /// What to tell the person who typed the address.
    pub fn message(&self) -> String {
        self.error.message(&self.host)
    }
}

impl fmt::Display for NotFound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for NotFound {}

/// One GET's answer: its status, the origin that answered (after any
/// redirect) and its body.
#[derive(Debug, Clone)]
pub struct Answer {
    pub status: u16,
    pub origin: String,
    pub body: String,
}

/// The server at `address` (as someone typed it), read from its discovery
/// document. `fetch` GETs a URL (`accept: application/json`) the app's own
/// way: None when nothing answered. Every app looks up here, so all of them
/// take the same servers.
///
/// Zeron's own address answers without a request (its client is built in),
/// so pointing back at it works offline.
pub async fn look_up<F, A>(address: &str, mut fetch: F) -> Result<Server, NotFound>
where
    F: FnMut(String) -> A,
    A: Future<Output = Option<Answer>>,
{
    let fail = |url: &str, error: ServerError| NotFound { host: host_of(url), error };
    let url = normalize_address(address).map_err(|error| NotFound {
        host: address.trim().to_owned(),
        error: error.into(),
    })?;
    if url == OFFICIAL_URL {
        return Ok(Server::official());
    }
    let answer = fetch(discovery_url(&url)).await.ok_or_else(|| fail(&url, ServerError::Unreachable))?;
    // A server that moved (http → https, a bare host → its www) is where the
    // answer came from, held to the same rules as a typed one.
    let origin = normalize_address(&answer.origin).map_err(|error| fail(&answer.origin, error.into()))?;
    match read_discovery(&origin, answer.status, &answer.body) {
        Discovered::Server(server) => Ok(server),
        Discovered::Failed(error) => Err(fail(&origin, error)),
        Discovered::AskHealth => {
            let error = match fetch(health_url(&origin)).await {
                Some(answer) => read_health(answer.status, &answer.body),
                None => ServerError::NotZeron,
            };
            Err(fail(&origin, error))
        }
    }
}

/// [`look_up`] with a `fetch` that blocks until it's answered (for a
/// worker thread): every fetch is ready as it's made, so one poll finishes it.
pub fn look_up_blocking(address: &str, mut fetch: impl FnMut(&str) -> Option<Answer>) -> Result<Server, NotFound> {
    let lookup = std::pin::pin!(look_up(address, |url| std::future::ready(fetch(&url))));
    match lookup.poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(found) => found,
        Poll::Pending => unreachable!("a blocking fetch is never pending"),
    }
}

/// Whether an engine's environment says which server it uses: an edge
/// (`ZERON_EDGE_URL`), a sign-in client (`ZERON_WORKOS_CLIENT_ID`, even
/// empty: that asks for dev auth) or a dev bearer (`ZERON_EDGE_TOKEN`). It
/// wins over the server saved for the data dir.
pub fn environment_names_a_server(var: impl Fn(&str) -> Option<String>) -> bool {
    var("ZERON_EDGE_TOKEN").is_some()
        || var("ZERON_EDGE_URL").is_some_and(|url| !url.trim().is_empty())
        || var("ZERON_WORKOS_CLIENT_ID").is_some()
}

/// The server an engine on `data_dir` signs in to in place of its
/// environment's: the one saved there, unless the environment (`var`)
/// names one. Every engine resolves it here, so the edge it syncs with and
/// the one it asks for anything else agree.
pub fn chosen(data_dir: &Path, var: impl Fn(&str) -> Option<String>) -> Option<Server> {
    if environment_names_a_server(var) {
        return None;
    }
    saved(data_dir)
}

/// The server saved for the engine on `data_dir` (the desktop app's
/// Settings): the one every engine there signs in to unless its environment
/// names another ([`chosen`]), so a session kept there is that server's.
/// None when none was chosen, or the file isn't one this build can use.
pub fn saved(data_dir: &Path) -> Option<Server> {
    let bytes = std::fs::read(data_dir.join(SAVED_FILE)).ok()?;
    let server: Server = serde_json::from_slice(&bytes).ok()?;
    let workos_client_id = server.workos_client_id.trim();
    if workos_client_id.is_empty() {
        return None;
    }
    Some(Server {
        url: normalize_address(&server.url).ok()?,
        workos_client_id: workos_client_id.to_owned(),
        name: server.name.as_deref().and_then(tidy_name),
    })
}

/// Chooses `server` for the engine on `data_dir` from its next start (none:
/// the one its environment names, Zeron's by default). Written whole or not
/// at all: an engine starting meanwhile reads the old choice or the new.
pub fn save(data_dir: &Path, server: Option<&Server>) -> std::io::Result<()> {
    let path = data_dir.join(SAVED_FILE);
    let Some(server) = server else {
        return match std::fs::remove_file(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            other => other,
        };
    };
    std::fs::create_dir_all(data_dir)?;
    let json = serde_json::to_vec_pretty(server).map_err(std::io::Error::other)?;
    let partial = data_dir.join(format!("{SAVED_FILE}.partial"));
    std::fs::write(&partial, json)?;
    std::fs::rename(partial, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_become_origins() {
        let ok = |input: &str| normalize_address(input).unwrap();
        assert_eq!(ok("zeron.example.com"), "https://zeron.example.com");
        assert_eq!(ok("  Zeron.Example.COM/  "), "https://zeron.example.com");
        assert_eq!(ok("https://zeron.example.com:8443/"), "https://zeron.example.com:8443");
        assert_eq!(ok("zeron.example.com:8443"), "https://zeron.example.com:8443");
        assert_eq!(ok("HTTPS://edge.zeron.sh"), OFFICIAL_URL);
        assert_eq!(ok("https://edge.zeron.sh:443"), OFFICIAL_URL);
        assert_eq!(ok("http://localhost:8787"), "http://localhost:8787");
        assert_eq!(ok("http://192.168.1.20:8787"), "http://192.168.1.20:8787");
        assert_eq!(ok("http://[::1]:8787"), "http://[::1]:8787");
        assert_eq!(ok("http://studio.local"), "http://studio.local");
        assert_eq!(ok("http://nas"), "http://nas");
        assert_eq!(ok("http://100.101.102.103"), "http://100.101.102.103");
    }

    #[test]
    fn addresses_that_cant_be_used_say_why() {
        let err = |input: &str| normalize_address(input).unwrap_err();
        assert_eq!(err("   "), AddressError::Empty);
        assert_eq!(err("zeron example"), AddressError::Invalid);
        assert_eq!(err("https://"), AddressError::Invalid);
        assert_eq!(err("zeron..example.com"), AddressError::Invalid);
        assert_eq!(err("ftp://zeron.example.com"), AddressError::Scheme);
        assert_eq!(err("wss://zeron.example.com"), AddressError::Scheme);
        assert_eq!(err("https://me:pw@zeron.example.com"), AddressError::Credentials);
        assert_eq!(err("https://example.com/zeron"), AddressError::Path);
        assert_eq!(err("https://example.com/?x=1"), AddressError::Path);
        assert_eq!(err("https://example.com/#top"), AddressError::Path);
        // A URL further on isn't the address's scheme.
        assert_eq!(err("zeron.example.com/login?next=https://x"), AddressError::Path);
        assert_eq!(err("http://zeron.example.com"), AddressError::Insecure);
        assert_eq!(err("http://8.8.8.8"), AddressError::Insecure);
        assert_eq!(err("http://100.128.0.1"), AddressError::Insecure);
    }

    #[test]
    fn the_official_server_is_known_by_its_address() {
        assert!(Server::official().is_official());
        let elsewhere = Server {
            url: "https://zeron.example.com".into(),
            workos_client_id: OFFICIAL_WORKOS_CLIENT_ID.into(),
            name: None,
        };
        assert!(!elsewhere.is_official());
        assert_eq!(elsewhere.label(), "zeron.example.com");
        assert_eq!(host_of("http://192.168.1.20:8787"), "192.168.1.20:8787");
        assert_eq!(Server::official().label(), "Zeron");
    }

    #[test]
    fn a_server_round_trips_through_json() {
        let server = Server {
            url: "https://zeron.example.com".into(),
            workos_client_id: "client_1".into(),
            name: None,
        };
        let json = serde_json::to_string(&server).unwrap();
        assert_eq!(json, r#"{"url":"https://zeron.example.com","workosClientId":"client_1"}"#);
        assert_eq!(serde_json::from_str::<Server>(&json).unwrap(), server);
    }

    const URL: &str = "https://zeron.example.com";

    #[test]
    fn a_workos_server_is_one_to_sign_in_to() {
        let found = read_discovery(
            URL,
            200,
            r#"{"zeron":1,"name":" Acme ","auth":{"mode":"workos","workosClientId":"client_9"}}"#,
        );
        assert_eq!(
            found,
            Discovered::Server(Server {
                url: URL.into(),
                workos_client_id: "client_9".into(),
                name: Some("Acme".into()),
            })
        );
        // Unknown fields are a later server's additions, not a problem.
        let found = read_discovery(
            URL,
            200,
            r#"{"zeron":1,"extra":true,"auth":{"mode":"workos","workosClientId":"c","more":1}}"#,
        );
        assert!(matches!(found, Discovered::Server(Server { name: None, .. })));
    }

    #[test]
    fn a_name_is_shown_on_one_line() {
        assert_eq!(tidy_name(" Acme\n\tLabs \u{0007} "), Some("Acme Labs".into()));
        assert_eq!(tidy_name("\u{202E}emcA"), Some("emcA".into()));
        assert_eq!(tidy_name(" \n "), None);
        let long = tidy_name(&"a".repeat(NAME_MAX + 10)).unwrap();
        assert_eq!(long.chars().count(), NAME_MAX + 1);
        assert!(long.ends_with('…'));
        assert_eq!(tidy_name(&"é".repeat(NAME_MAX)), Some("é".repeat(NAME_MAX)));
    }

    #[test]
    fn discovery_refusals() {
        let failed = |status, body| match read_discovery(URL, status, body) {
            Discovered::Failed(error) => error,
            other => panic!("expected a failure, got {other:?}"),
        };
        assert_eq!(failed(200, r#"{"zeron":1,"auth":{"mode":"dev"}}"#), ServerError::DevServer);
        assert_eq!(
            failed(200, r#"{"zeron":2,"auth":{"mode":"workos","workosClientId":"c"}}"#),
            ServerError::NeedsNewerApp
        );
        assert_eq!(failed(200, r#"{"zeron":1,"auth":{"mode":"passkeys"}}"#), ServerError::NeedsNewerApp);
        assert_eq!(failed(200, r#"{"zeron":1,"auth":{"mode":"workos"}}"#), ServerError::NotZeron);
        assert_eq!(failed(200, r#"{"zeron":"yes"}"#), ServerError::NotZeron);
        assert_eq!(failed(200, "<html>hello</html>"), ServerError::NotZeron);
        assert_eq!(failed(200, r#"{"ok":true}"#), ServerError::NotZeron);
        assert_eq!(failed(401, "nope"), ServerError::NotZeron);
        assert_eq!(failed(502, "<html>bad gateway</html>"), ServerError::Unavailable { status: 502 });
        assert_eq!(
            failed(503, r#"{"zeron":1,"error":"sign_in_not_configured","missing":["WORKOS_API_KEY"]}"#),
            ServerError::NotConfigured { missing: vec!["WORKOS_API_KEY".into()] }
        );
        assert_eq!(read_discovery(URL, 404, "Not found"), Discovered::AskHealth);
    }

    #[test]
    fn health_tells_an_old_zeron_server_from_anything_else() {
        assert_eq!(read_health(200, r#"{"ok":true,"auth":"workos"}"#), ServerError::Outdated);
        assert_eq!(read_health(200, r#"{"ok":true}"#), ServerError::NotZeron);
        assert_eq!(read_health(404, ""), ServerError::NotZeron);
        assert_eq!(read_health(200, "ok"), ServerError::NotZeron);
    }

    #[test]
    fn messages_name_the_server() {
        let host = "zeron.example.com";
        assert_eq!(ServerError::NotZeron.message(host), "zeron.example.com isn't a Zeron server.");
        assert_eq!(
            ServerError::NotConfigured { missing: vec!["WORKOS_API_KEY".into()] }.message(host),
            "zeron.example.com hasn't finished setting up sign-in (missing WORKOS_API_KEY)."
        );
        assert_eq!(ServerError::Unreachable.code(), "unreachable");
        assert_eq!(AddressError::Insecure.to_string(), ServerError::Address(AddressError::Insecure).message(host));
    }

    #[test]
    fn routes_hang_off_the_origin() {
        assert_eq!(discovery_url(URL), "https://zeron.example.com/.well-known/zeron");
        assert_eq!(health_url(URL), "https://zeron.example.com/health");
        assert_eq!(origin_of("https://zeron.example.com:8443/.well-known/zeron?x=1").as_deref(), Some("https://zeron.example.com:8443"));
        assert_eq!(origin_of("not a url"), None);
    }

    #[test]
    fn a_chosen_server_is_kept_in_the_data_dir() {
        let dir = std::env::temp_dir().join(format!("zeron-server-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(saved(&dir), None, "none chosen");
        save(&dir, None).unwrap();

        let acme = Server { url: URL.into(), workos_client_id: "client_acme".into(), name: Some("Acme".into()) };
        save(&dir, Some(&acme)).unwrap();
        assert_eq!(saved(&dir), Some(acme));
        assert!(!dir.join(format!("{SAVED_FILE}.partial")).exists());

        // Read as it would be typed: an origin, trimmed; or not at all.
        std::fs::write(dir.join(SAVED_FILE), r#"{"url":"zeron.example.com/","workosClientId":" client_acme ","name":" "}"#).unwrap();
        assert_eq!(saved(&dir), Some(Server { url: URL.into(), workos_client_id: "client_acme".into(), name: None }));
        for unusable in [r#"{"url":"http://zeron.example.com","workosClientId":"c"}"#, r#"{"url":"zeron.example.com","workosClientId":""}"#, "{"] {
            std::fs::write(dir.join(SAVED_FILE), unusable).unwrap();
            assert_eq!(saved(&dir), None, "{unusable}");
        }

        save(&dir, None).unwrap();
        assert!(!dir.join(SAVED_FILE).exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A lookup answered from `routes` (`(url, status, origin, body)`),
    /// nothing answering elsewhere; and the URLs it asked, in order.
    fn look_up_in(address: &str, routes: &[(&str, u16, &str, &str)]) -> (Result<Server, NotFound>, Vec<String>) {
        let mut asked = Vec::new();
        let found = look_up_blocking(address, |url| {
            asked.push(url.to_owned());
            routes.iter().find(|(u, ..)| *u == url).map(|(_, status, origin, body)| Answer {
                status: *status,
                origin: (*origin).into(),
                body: (*body).into(),
            })
        });
        (found, asked)
    }

    #[test]
    fn a_lookup_reads_the_discovery_document_where_it_was_answered() {
        const ACME: &str = r#"{"zeron":1,"name":"Acme","auth":{"mode":"workos","workosClientId":"client_acme"}}"#;
        let (found, asked) = look_up_in(" Zeron.Example.com/ ", &[(&discovery_url(URL), 200, URL, ACME)]);
        let acme = Server { url: URL.into(), workos_client_id: "client_acme".into(), name: Some("Acme".into()) };
        assert_eq!((found, asked), (Ok(acme), vec![discovery_url(URL)]));

        // Moved: the server is where it answered from, by the same rules.
        let moved = look_up_in("acme.example.com", &[(&discovery_url("https://acme.example.com"), 200, URL, ACME)]).0;
        assert_eq!(moved.unwrap().url, URL);
        let insecure = look_up_in("acme.example.com", &[(&discovery_url("https://acme.example.com"), 200, "http://acme.example.com", ACME)]).0;
        assert_eq!(insecure, Err(NotFound { host: "acme.example.com".into(), error: ServerError::Address(AddressError::Insecure) }));
    }

    #[test]
    fn a_lookup_without_a_document_asks_health_and_zeron_asks_nothing() {
        let old = look_up_in(URL, &[(&discovery_url(URL), 404, URL, "Not found"), (&health_url(URL), 200, URL, r#"{"ok":true,"auth":"workos"}"#)]);
        assert_eq!(old.0.unwrap_err().error, ServerError::Outdated);
        assert_eq!(old.1, vec![discovery_url(URL), health_url(URL)]);
        let stranger = look_up_in(URL, &[(&discovery_url(URL), 404, URL, "Not found")]).0;
        assert_eq!(stranger.unwrap_err().error, ServerError::NotZeron);

        let silent = look_up_in(URL, &[]).0;
        assert_eq!(silent, Err(NotFound { host: "zeron.example.com".into(), error: ServerError::Unreachable }));
        assert_eq!(look_up_in("edge.zeron.sh", &[]), (Ok(Server::official()), vec![]));
        assert_eq!(look_up_in("ftp://x", &[]).0.unwrap_err().error, ServerError::Address(AddressError::Scheme));
    }

    #[test]
    fn the_environment_wins_over_the_chosen_server() {
        let with = |pairs: &'static [(&str, &str)]| {
            move |key: &str| pairs.iter().find(|(k, _)| *k == key).map(|(_, v)| v.to_string())
        };
        assert!(!environment_names_a_server(with(&[])));
        assert!(!environment_names_a_server(with(&[("ZERON_EDGE_URL", " ")])));
        assert!(environment_names_a_server(with(&[("ZERON_EDGE_URL", "http://localhost:8787")])));
        assert!(environment_names_a_server(with(&[("ZERON_WORKOS_CLIENT_ID", "")])));
        assert!(environment_names_a_server(with(&[("ZERON_EDGE_TOKEN", "dev-user")])));

        let dir = std::env::temp_dir().join(format!("zeron-server-env-{}", std::process::id()));
        let acme = Server { url: URL.into(), workos_client_id: "client_acme".into(), name: None };
        save(&dir, Some(&acme)).unwrap();
        assert_eq!(chosen(&dir, with(&[])), Some(acme));
        assert_eq!(chosen(&dir, with(&[("ZERON_EDGE_TOKEN", "dev-user")])), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
