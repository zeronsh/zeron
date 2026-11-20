//! R15: what a plugin's requests from the window reach, through zgx's own
//! transport against a server on this machine. Each URL here is asked as a
//! plugin's `ui.fetch` would, so the transport's reading of the URL and the
//! rule's are the ones that ship. (`ui.socket`: this app has no connector, so
//! a plugin's has nothing to connect through; zgx's own tests hold the hook.)

use std::io::{Read, Write};
use std::net::{IpAddr, TcpListener};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use zgx_script::testing::TestApp;
use zgx_script::{Capability, FetchPolicy, Plugin, ScriptHost, Url};

use super::network::Network;

/// A server on this machine: `/hop` redirects to `location`, the rest is 200
/// with the Host header it was sent. How many requests reached it, and the
/// path each asked for.
struct Server {
    port: u16,
    hits: Arc<AtomicUsize>,
    asked: Arc<std::sync::Mutex<Vec<String>>>,
}

fn serve(location: &str) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let hits = Arc::new(AtomicUsize::new(0));
    let asked_paths: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
    let (counted, location, noted) = (hits.clone(), location.to_owned(), asked_paths.clone());
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            counted.fetch_add(1, Ordering::SeqCst);
            let mut asked = Vec::new();
            let mut buffer = [0; 4096];
            while !asked.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut buffer) {
                    Ok(n) if n > 0 => asked.extend_from_slice(&buffer[..n]),
                    _ => break,
                }
            }
            let head = String::from_utf8_lossy(&asked).into_owned();
            let path = head.split(' ').nth(1).unwrap_or_default().to_owned();
            noted.lock().unwrap().push(path.clone());
            let reply = if path == "/hop" {
                format!("HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            } else {
                "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok".to_owned()
            };
            let _ = stream.write_all(reply.as_bytes());
        }
    });
    Server { port, hits, asked: asked_paths }
}

/// `ui.fetch(url, opts)`, its answer in the text `out`.
fn fetching(url: &str, opts: &str) -> String {
    format!(
        r#"
local out = ui.state("out", "")
ui.fetch("{url}", {opts}):after(
    function(r) out:set(`ok {{r.status}}`) end,
    function(e) out:set(`failed {{e}}`) end
)
local shown = ui.text {{ id = "out", function() return out:get() end }}
"#
    )
}

/// `code` as the plugin `plug`'s, naming `hosts` in its manifest.
fn as_plugin(code: &str, hosts: &[&str]) -> TestApp {
    let dir = std::env::temp_dir().join(format!("zgx-plugin-network-{}-{:?}", std::process::id(), std::thread::current().id()));
    std::fs::create_dir_all(&dir).unwrap();
    let entry = dir.join("init.luau");
    std::fs::write(&entry, format!("{code}\nui.contribute(\"bar\", shown)")).unwrap();
    let hosts: Vec<String> = hosts.iter().map(|h| h.to_string()).collect();
    TestApp::new(move |runtime| {
        let host = ScriptHost::inline(runtime, "app", r#"return ui.column { ui.slot("bar") }"#);
        zgx_run::install_fetcher(&host);
        let network = Network::default();
        network.set([("plug".to_string(), hosts)]);
        host.fetch_policy(network);
        host.set_plugins(vec![Plugin { name: "plug".into(), entry, grants: vec![Capability::Fetch] }]);
        host
    })
}

fn answered(app: &mut TestApp) -> String {
    assert!(app.wait_until(|app| !app.text("out").is_empty()), "no answer: {:?}", app.error());
    app.text("out")
}

/// What a plugin naming `hosts` hears asking `url`.
fn asked(url: &str, hosts: &[&str]) -> String {
    answered(&mut as_plugin(&fetching(url, "{}"), hosts))
}

fn hits(server: &Server) -> usize {
    server.hits.load(Ordering::SeqCst)
}

/// An open plugin (its manifest names no host) reaches this device by no
/// address and no spelling of one, nor instance metadata, nor no address at
/// all, and the refusal says which host to add where one can be.
#[test]
fn an_open_plugin_reaches_neither_this_device_nor_metadata() {
    let server = serve("");
    let port = server.port;
    for url in [
        format!("http://127.0.0.1:{port}/"),
        format!("http://localhost:{port}/"),
        format!("http://[::ffff:127.0.0.1]:{port}/"),
        format!("http://[::1]:{port}/"),
        format!("http://0x7f.1:{port}/"),
        format!("http://2130706433:{port}/"),
        "http://169.254.169.254/latest/meta-data/".to_string(),
        "http://[::ffff:169.254.169.254]/".to_string(),
        "http://0.0.0.0/".to_string(),
        "http://[::]/".to_string(),
    ] {
        let out = asked(&url, &[]);
        assert!(out.starts_with("failed"), "{url} was reached: {out}");
    }
    assert_eq!(hits(&server), 0, "a request reached this device");
    let out = asked(&format!("http://localhost:{port}/"), &[]);
    assert!(out.contains(r#""hosts": ["localhost"]"#), "the refusal doesn't say what to add: {out}");
    let out = asked("http://169.254.169.254/", &[]);
    assert!(out.contains("no plugin may reach"), "{out}");
}

/// Named `localhost:<port>`: that host and port, nothing else, and the
/// refusal names what to add. Named without a port: any port on that host.
#[test]
fn a_named_host_is_reached_at_the_port_it_names() {
    let server = serve("");
    let other = serve("");
    let named = format!("localhost:{}", server.port);
    assert_eq!(asked(&format!("http://localhost:{}/", server.port), &[&named]), "ok 200");
    assert_eq!(hits(&server), 1);

    let out = asked(&format!("http://localhost:{}/", other.port), &[&named]);
    assert!(out.starts_with("failed"), "another port was reached: {out}");
    assert!(out.contains(&format!("\"localhost:{}\"", other.port)), "the refusal doesn't say what to add: {out}");
    assert_eq!(hits(&other), 0);

    // The same device by another name is another host.
    let out = asked(&format!("http://127.0.0.1:{}/", server.port), &[&named]);
    assert!(out.starts_with("failed"), "{out}");
    assert_eq!(hits(&server), 1);

    // No port named: a dev server may move.
    assert_eq!(asked(&format!("http://localhost:{}/", other.port), &["localhost"]), "ok 200");
    assert_eq!(hits(&other), 1);

    // Metadata stays out, named or not.
    let out = asked("http://169.254.169.254/", &["169.254.169.254"]);
    assert!(out.contains("no plugin may reach"), "{out}");
}

/// A redirect is asked of as the first URL is: from a host it may reach to a
/// host it may not, the second is never connected to. (The first here is a
/// server named in the manifest: a test has no public server to start from.
/// Below, the rule's own answers for a public first hop.)
#[test]
fn a_redirect_to_this_device_is_never_followed() {
    let target = serve("");
    let first = serve(&format!("http://localhost:{}/", target.port));
    let named = format!("127.0.0.1:{}", first.port);
    let out = asked(&format!("http://127.0.0.1:{}/hop", first.port), &[&named]);
    assert!(out.starts_with("failed"), "{out}");
    assert_eq!(hits(&first), 1);
    assert_eq!(hits(&target), 0, "the redirect was followed");

    // An open plugin: the public first hop is allowed, and the local second
    // is refused as it's about to connect, whichever way it's written.
    let open = Network::default();
    open.set([("plug".to_string(), Vec::new())]);
    let url = |host: &str, port| Url::new(format!("http://{host}/"), "http", host, port);
    assert!(open.url(Some("plug"), &url("example.com", None)).is_ok());
    assert!(open.address(Some("plug"), "example.com", "93.184.216.34".parse().unwrap()).is_ok());
    for (host, address) in [("127.0.0.1", "127.0.0.1"), ("localhost", "::1"), ("rebound.example.com", "10.0.0.7"), ("metadata.google.internal", "169.254.169.254")] {
        let address: IpAddr = address.parse().unwrap();
        assert!(open.address(Some("plug"), host, address).is_err(), "{host} at {address}");
    }
}

/// A name that resolves here is refused where it resolves: the URL's host
/// reads as public, the address doesn't. (`::ffff:` is the same address.)
#[test]
fn a_name_resolving_here_is_refused_by_its_address() {
    let open = Network::default();
    open.set([("plug".to_string(), Vec::new())]);
    for address in ["127.0.0.1", "::1", "::ffff:127.0.0.1", "192.168.1.5", "100.64.0.1", "fd00::1", "0.0.0.0", "::", "169.254.169.254"] {
        let address: IpAddr = address.parse().unwrap();
        assert!(open.address(Some("plug"), "app.example.com", address).is_err(), "{address}");
    }
    // Through the transport: `localhost` is a name, and resolves here.
    let server = serve("");
    let out = asked(&format!("http://localhost:{}/", server.port), &[]);
    assert!(out.contains("this device or your network"), "{out}");
    assert_eq!(hits(&server), 0);
}

/// A plugin never sets Host, whatever it names; credentials go only to a host
/// it names.
#[test]
fn a_plugin_s_headers_are_held_to_the_rule() {
    let server = serve("");
    let named = format!("localhost:{}", server.port);
    let url = format!("http://localhost:{}/", server.port);
    let mut app = as_plugin(&fetching(&url, r#"{ headers = { Host = "example.com" } }"#), &[&named]);
    let out = answered(&mut app);
    assert!(out.starts_with("failed") && out.contains("Host"), "{out}");
    assert_eq!(hits(&server), 0);

    let mut app = as_plugin(&fetching(&url, r#"{ headers = { Authorization = "Bearer x" } }"#), &[&named]);
    assert_eq!(answered(&mut app), "ok 200", "a host it names");
    assert_eq!(hits(&server), 1);

    let open = Network::default();
    open.set([("plug".to_string(), Vec::new())]);
    assert!(open.header(Some("plug"), "example.com", "Authorization").is_err());
    assert!(open.header(Some("plug"), "example.com", "Accept").is_ok());
}

/// The app's own requests aren't held to any of this.
#[test]
fn the_app_s_own_requests_are_unchanged() {
    let server = serve("");
    let code = format!("{}\nreturn ui.column {{ shown }}", fetching(&format!("http://127.0.0.1:{}/", server.port), "{}"));
    let mut app = TestApp::new(move |runtime| {
        let host = ScriptHost::inline(runtime, "app", code);
        zgx_run::install_fetcher(&host);
        host.fetch_policy(Network::default());
        host
    });
    assert_eq!(answered(&mut app), "ok 200");
    assert_eq!(hits(&server), 1);
    let policy = Network::default();
    assert!(policy.url(None, &Url::new("http://127.0.0.1/", "http", "127.0.0.1", None)).is_ok());
    assert!(policy.address(None, "127.0.0.1", "127.0.0.1".parse().unwrap()).is_ok());
    assert!(policy.header(None, "127.0.0.1", "Authorization").is_ok());
}

/// A plugin's remote picture is its request too.
#[test]
fn a_plugin_s_picture_is_held_to_the_rule() {
    let server = serve("");
    let code = format!(r#"local shown = ui.image {{ id = "picture", w = 10, h = 10, src = "http://127.0.0.1:{}/a.png" }}"#, server.port);
    let mut app = as_plugin(&code, &[]);
    for _ in 0..20 {
        app.frames(2);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    // (Its picture's path, not any connection: with the suite's other
    // servers coming and going on this machine's ports, one may yet reach
    // this port that isn't the plugin's.)
    let pictures = |server: &Server| server.asked.lock().unwrap().iter().filter(|p| p.as_str() == "/a.png").count();
    assert_eq!(pictures(&server), 0, "a plugin's picture reached this device");
    // (Named, it goes: the wait above is long enough to have seen it.)
    let named = format!("127.0.0.1:{}", server.port);
    let mut app = as_plugin(&code, &[&named]);
    assert!(app.wait_until(|_| pictures(&server) > 0), "a picture it names never went");
}

/// A plugin not on the list (withdrawn while its request ran) reaches
/// nothing, and a list the engine would refuse reaches nothing.
#[test]
fn what_isn_t_listed_reaches_nothing() {
    let network = Network::default();
    assert!(network.url(Some("gone"), &Url::new("https://example.com/", "https", "example.com", None)).is_err());
    network.set([("bad".to_string(), vec!["http://x.com/path".to_string()])]);
    assert!(network.url(Some("bad"), &Url::new("https://example.com/", "https", "example.com", None)).is_err());
    network.set([("plug".to_string(), Vec::new())]);
    assert!(network.url(Some("plug"), &Url::new("file:///etc/hosts", "file", "", None)).is_err());
}
