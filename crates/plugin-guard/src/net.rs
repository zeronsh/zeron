//! Where a plugin's requests may go (`zeron.fetch`, "network").
//!
//! "network" is the internet: public addresses. Never, unless the manifest
//! names the host: this device (`localhost`, `127.0.0.1`, `::1`), your
//! network (`10.…`, `192.168.…`, `172.16–31.…`, `fc00::/7`, `.local`),
//! carrier and Tailscale space (`100.64.0.0/10`). Never at all: link-local
//! space, where clouds keep instance metadata and credentials
//! (`169.254.169.254`, `fe80::/10`), and addresses no host has (`0.0.0.0`,
//! multicast, reserved). An address is checked as it is connected to (after
//! DNS, so a name that later resolves somewhere private is caught) and
//! again on every redirect.
//!
//! `"network": { "hosts": ["api.example.com", "*.example.org",
//! "localhost:8080"] }` in a manifest narrows a plugin to those hosts
//! (shown at consent), and is the only way to reach a private one: named
//! exactly (no `*.`), with its port when it gives one. [`risky`] says which
//! entries reach this device or your network, for consent to say so.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Hosts a manifest may name, at most.
pub const MAX_HOSTS: usize = 64;

/// Why `ip` is private or unreachable, if it is: `Never` for space no
/// plugin reaches (metadata, unspecified, multicast), `Private` for this
/// device and your network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Public,
    /// This device or a network you're on: only a host named exactly.
    Private,
    /// Link-local (instance metadata), unspecified, multicast, broadcast,
    /// reserved, documentation: never.
    Never,
}

/// Where `ip` is.
pub fn place(ip: IpAddr) -> Place {
    match ip {
        IpAddr::V4(v4) => place_v4(v4),
        IpAddr::V6(v6) => place_v6(v6),
    }
}

fn place_v4(ip: Ipv4Addr) -> Place {
    let [a, b, c, _] = ip.octets();
    if ip.is_link_local() || ip.is_unspecified() || ip.is_broadcast() || ip.is_multicast() || a == 0 {
        return Place::Never;
    }
    // Documentation, benchmarking, IETF protocol, reserved (240/4).
    if (a, b, c) == (192, 0, 2) || (a, b, c) == (198, 51, 100) || (a, b, c) == (203, 0, 113) {
        return Place::Never;
    }
    if (a == 198 && (b & 0xfe) == 18) || (a, b, c) == (192, 0, 0) || a >= 240 {
        return Place::Never;
    }
    // (Alibaba's metadata, inside carrier space.)
    if ip == Ipv4Addr::new(100, 100, 100, 200) {
        return Place::Never;
    }
    if ip.is_loopback() || ip.is_private() || (a == 100 && (b & 0xc0) == 64) {
        return Place::Private;
    }
    Place::Public
}

fn place_v6(ip: Ipv6Addr) -> Place {
    // An IPv4 address inside an IPv6 one is where it really goes.
    if let Some(v4) = ip.to_ipv4_mapped() {
        return place_v4(v4);
    }
    let s = ip.segments();
    // IPv4-compatible (::a.b.c.d), NAT64 (64:ff9b::/96), 6to4 (2002::/16).
    if s[..6] == [0, 0, 0, 0, 0, 0] && !ip.is_loopback() && !ip.is_unspecified() {
        return place_v4(Ipv4Addr::new((s[6] >> 8) as u8, s[6] as u8, (s[7] >> 8) as u8, s[7] as u8));
    }
    if s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        return place_v4(Ipv4Addr::new((s[6] >> 8) as u8, s[6] as u8, (s[7] >> 8) as u8, s[7] as u8));
    }
    if s[0] == 0x2002 {
        return place_v4(Ipv4Addr::new((s[1] >> 8) as u8, s[1] as u8, (s[2] >> 8) as u8, s[2] as u8));
    }
    if ip.is_unspecified() || ip.is_multicast() {
        return Place::Never;
    }
    // Link-local fe80::/10 and the old site-local fec0::/10; documentation.
    if (s[0] & 0xffc0) == 0xfe80 || (s[0] & 0xffc0) == 0xfec0 || (s[0], s[1]) == (0x2001, 0x0db8) {
        return Place::Never;
    }
    // Teredo (2001::/32) tunnels to an address it hides: not reachable here.
    if (s[0], s[1]) == (0x2001, 0) {
        return Place::Never;
    }
    if ip.is_loopback() || (s[0] & 0xfe00) == 0xfc00 {
        return Place::Private;
    }
    Place::Public
}

/// One `network.hosts` entry: a host (or `*.` and a domain), and a port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostRule {
    /// Lowercase; an IPv6 address without brackets.
    pub host: String,
    /// `*.example.org`: any name under it (never a private one).
    pub wildcard: bool,
    pub port: Option<u16>,
}

impl HostRule {
    /// `api.example.com`, `*.example.org`, `localhost:8080`, `10.0.0.5`,
    /// `[::1]:3000`; no scheme, path or credentials.
    pub fn parse(entry: &str) -> Result<HostRule, String> {
        let bad = || format!("\"{entry}\" isn't a host (api.example.com, *.example.org, localhost:8080)");
        let entry = entry.trim();
        if entry.is_empty() || entry.len() > 253 || entry.contains(['/', '@', '?', '#', ' ']) {
            return Err(bad());
        }
        let (host, port) = if let Some(rest) = entry.strip_prefix('[') {
            let (host, after) = rest.split_once(']').ok_or_else(bad)?;
            host.parse::<Ipv6Addr>().map_err(|_| bad())?;
            let port = match after.strip_prefix(':') {
                Some(p) => Some(p.parse::<u16>().map_err(|_| bad())?),
                None if after.is_empty() => None,
                None => return Err(bad()),
            };
            (host.to_owned(), port)
        } else {
            match entry.rsplit_once(':') {
                Some((host, port)) if !host.contains(':') => (host.to_owned(), Some(port.parse::<u16>().map_err(|_| bad())?)),
                Some(_) => {
                    entry.parse::<Ipv6Addr>().map_err(|_| bad())?;
                    (entry.to_owned(), None)
                }
                None => (entry.to_owned(), None),
            }
        };
        let host = host.to_ascii_lowercase();
        let (wildcard, name) = match host.strip_prefix("*.") {
            Some(name) => (true, name.to_owned()),
            None => (false, host.clone()),
        };
        let label_ok = |l: &str| {
            !l.is_empty()
                && l.len() <= 63
                && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                && !l.starts_with('-')
                && !l.ends_with('-')
        };
        let is_ip = name.parse::<IpAddr>().is_ok();
        if !(is_ip || name.split('.').all(label_ok)) || (wildcard && (is_ip || !name.contains('.'))) {
            return Err(bad());
        }
        Ok(HostRule { host: name, wildcard, port })
    }

    fn matches(&self, host: &str, port: u16) -> bool {
        let host = host.trim_end_matches('.');
        let named = if self.wildcard {
            host.len() > self.host.len() && host.ends_with(&self.host) && host[..host.len() - self.host.len()].ends_with('.')
        } else {
            host == self.host
        };
        named && self.port.is_none_or(|p| p == port)
    }
}

/// Whether a `network.hosts` entry reaches this device or your network
/// (consent words it so): `localhost`, a private address, `.local`,
/// `.internal`, `.lan`, `.home.arpa`, or a one-label name.
pub fn risky(entry: &str) -> bool {
    let Ok(rule) = HostRule::parse(entry) else { return true };
    if let Ok(ip) = rule.host.parse::<IpAddr>() {
        return place(ip) != Place::Public;
    }
    let host = rule.host.as_str();
    host == "localhost"
        || !host.contains('.')
        || [".localhost", ".local", ".internal", ".lan", ".home.arpa", ".intranet", ".corp"]
            .iter()
            .any(|tail| host.ends_with(tail))
}

/// A plugin's network rules: open (any public host) or its manifest's
/// `network.hosts`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Policy {
    hosts: Vec<HostRule>,
}

impl Policy {
    /// From a manifest's `network.hosts` (each already valid; see
    /// [`HostRule::parse`]).
    pub fn new(hosts: &[String]) -> Result<Policy, String> {
        if hosts.len() > MAX_HOSTS {
            return Err(format!("A plugin names {MAX_HOSTS} hosts at most"));
        }
        Ok(Policy { hosts: hosts.iter().map(|h| HostRule::parse(h)).collect::<Result<_, _>>()? })
    }

    /// Whether `host:port` (a URL's) may be asked at all: any, or one the
    /// manifest names.
    pub fn host(&self, host: &str, port: u16) -> Result<(), String> {
        let host = unbracket(host);
        if self.hosts.is_empty() || self.hosts.iter().any(|r| r.matches(&host, port)) {
            Ok(())
        } else {
            Err(format!("This plugin reaches only the hosts it names (not {host})"))
        }
    }

    /// Whether `ip`, which `host:port` resolved to (or is), may be
    /// connected to.
    pub fn address(&self, host: &str, port: u16, ip: IpAddr) -> Result<(), String> {
        let host = unbracket(host);
        match place(ip) {
            Place::Public => Ok(()),
            Place::Never => Err(format!("{host} is at {ip}, which no plugin may reach")),
            Place::Private if self.hosts.iter().any(|r| !r.wildcard && r.matches(&host, port)) => Ok(()),
            Place::Private => Err(format!(
                "{host} is at {ip}, on this device or your network: a plugin reaches one only if its manifest names it (\"network\": {{ \"hosts\": [\"{host}\"] }})"
            )),
        }
    }

    /// Whether `ip`, which the name `host` resolved to, may be connected to
    /// on any port [`Policy::host`] allowed (what a resolver checks: it
    /// knows the name, not the port).
    pub fn resolved(&self, host: &str, ip: IpAddr) -> Result<(), String> {
        let host = unbracket(host);
        match place(ip) {
            Place::Public => Ok(()),
            Place::Never => Err(format!("{host} is at {ip}, which no plugin may reach")),
            Place::Private if self.hosts.iter().any(|r| !r.wildcard && r.host == host.trim_end_matches('.')) => Ok(()),
            Place::Private => Err(format!(
                "{host} is at {ip}, on this device or your network: a plugin reaches one only if its manifest names it (\"network\": {{ \"hosts\": [\"{host}\"] }})"
            )),
        }
    }

    /// Whether a header may be sent to `host:port`: never `Host` or what
    /// shapes the connection; credentials (`Authorization`, `Cookie`) only
    /// to a host the manifest names.
    pub fn header(&self, host: &str, port: u16, name: &str) -> Result<(), String> {
        let lower = name.to_ascii_lowercase();
        if matches!(
            lower.as_str(),
            "host" | "connection" | "content-length" | "transfer-encoding" | "te" | "trailer" | "upgrade" | "keep-alive"
                | "expect"
        ) || lower.starts_with("proxy-")
        {
            return Err(format!("A plugin can't set the {name} header"));
        }
        if matches!(lower.as_str(), "authorization" | "cookie") {
            let host = unbracket(host);
            if !self.hosts.iter().any(|r| r.matches(&host, port)) {
                return Err(format!(
                    "A plugin sends {name} only to a host its manifest names (\"network\": {{ \"hosts\": [\"{host}\"] }})"
                ));
            }
        }
        Ok(())
    }
}

fn unbracket(host: &str) -> String {
    host.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn places() {
        for (addr, want) in [
            ("8.8.8.8", Place::Public),
            ("1.1.1.1", Place::Public),
            ("2606:4700:4700::1111", Place::Public),
            ("127.0.0.1", Place::Private),
            ("127.8.9.10", Place::Private),
            ("10.1.2.3", Place::Private),
            ("172.16.0.1", Place::Private),
            ("172.31.255.255", Place::Private),
            ("192.168.1.1", Place::Private),
            ("100.64.0.1", Place::Private),
            ("100.101.102.103", Place::Private),
            ("::1", Place::Private),
            ("fd00::1", Place::Private),
            ("fd00:ec2::254", Place::Private),
            ("169.254.169.254", Place::Never),
            ("169.254.170.2", Place::Never),
            ("100.100.100.200", Place::Never),
            ("0.0.0.0", Place::Never),
            ("0.1.2.3", Place::Never),
            ("255.255.255.255", Place::Never),
            ("224.0.0.1", Place::Never),
            ("240.0.0.1", Place::Never),
            ("::", Place::Never),
            ("fe80::1", Place::Never),
            ("::ffff:127.0.0.1", Place::Private),
            ("::ffff:169.254.169.254", Place::Never),
            ("::127.0.0.1", Place::Private),
            ("64:ff9b::a9fe:a9fe", Place::Never),
            ("2002:7f00:1::", Place::Private),
            ("2001:0:4136:e378:8000:63bf:3fff:fdd2", Place::Never),
            ("172.32.0.1", Place::Public),
        ] {
            assert_eq!(place(ip(addr)), want, "{addr}");
        }
    }

    /// R15: an open plugin (no hosts named) reaches the internet, never
    /// this device, your network or instance metadata, by name or address.
    #[test]
    fn an_open_plugin_reaches_public_addresses_only() {
        let open = Policy::default();
        assert!(open.host("example.com", 443).is_ok());
        assert!(open.address("example.com", 443, ip("93.184.216.34")).is_ok());
        for (host, addr) in [
            ("127.0.0.1", "127.0.0.1"),
            ("localhost", "127.0.0.1"),
            ("169.254.169.254", "169.254.169.254"),
            ("metadata.google.internal", "169.254.169.254"),
            ("rebound.example.com", "10.0.0.7"),
            ("[::1]", "::1"),
            ("[::ffff:7f00:1]", "::ffff:127.0.0.1"),
        ] {
            assert!(open.address(host, 80, ip(addr)).is_err(), "{host} → {addr}");
        }
    }

    #[test]
    fn named_hosts_narrow_it_and_open_private_ones_exactly() {
        let p = Policy::new(&["api.example.com".into(), "*.example.org".into(), "localhost:8080".into(), "10.0.0.5".into()])
            .unwrap();
        assert!(p.host("api.example.com", 443).is_ok());
        assert!(p.host("a.b.example.org", 443).is_ok());
        assert!(p.host("example.org", 443).is_err(), "a wildcard is under the domain");
        assert!(p.host("evil.com", 443).is_err());
        assert!(p.host("localhost", 8080).is_ok());
        assert!(p.host("localhost", 22).is_err(), "only the port named");
        assert!(p.address("localhost", 8080, ip("127.0.0.1")).is_ok());
        assert!(p.address("10.0.0.5", 80, ip("10.0.0.5")).is_ok());
        // A wildcard never reaches a private address (rebinding).
        assert!(p.address("x.example.org", 443, ip("192.168.0.1")).is_err());
        // Metadata, even named.
        let named = Policy::new(&["169.254.169.254".into()]).unwrap();
        assert!(named.address("169.254.169.254", 80, ip("169.254.169.254")).is_err());
    }

    #[test]
    fn headers() {
        let open = Policy::default();
        for refused in ["Host", "host", "Proxy-Authorization", "Transfer-Encoding", "Connection"] {
            assert!(open.header("example.com", 443, refused).is_err(), "{refused}");
        }
        assert!(open.header("example.com", 443, "Authorization").is_err());
        assert!(open.header("example.com", 443, "Accept").is_ok());
        let named = Policy::new(&["api.example.com".into()]).unwrap();
        assert!(named.header("api.example.com", 443, "Authorization").is_ok());
    }

    #[test]
    fn entries() {
        for ok in ["api.example.com", "*.example.org", "localhost", "localhost:8080", "10.0.0.5", "[::1]:3000", "::1", "Example.COM"] {
            assert!(HostRule::parse(ok).is_ok(), "{ok}");
        }
        for bad in ["", "http://x.com", "x.com/path", "user@x.com", "*.com", "*", "*.10.0.0.1", "x..com", "-x.com", "x.com:99999", "a b"] {
            assert!(HostRule::parse(bad).is_err(), "{bad}");
        }
        assert!(risky("localhost:8080") && risky("10.0.0.5") && risky("nas.local") && risky("printer") && risky("[::1]"));
        assert!(!risky("api.example.com") && !risky("*.example.org"));
    }
}
