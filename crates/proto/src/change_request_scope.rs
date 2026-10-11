//! Which repository the pull request board shows, as it travels on the wire.
//!
//! A scope is a host and the repository's whole path on it. The wire form
//! carries no provider: the engine routes a scope by its host, and the
//! provider serving that host decides whether the path names a repository.
//! A bare `owner/repo` predates other hosts and always means github.com, so
//! older peers keep working; every other scope travels as `host/path`.

use std::fmt;

/// The host a bare `owner/repo` names.
pub const LEGACY_HOST: &str = "github.com";
/// Paths nest (groups within groups), but a scope stays a short key.
const MAX_SEGMENTS: usize = 32;
const MAX_PATH_LEN: usize = 512;

/// One repository on one host. The host is lowercase; the path keeps the
/// case it was given, and compares without it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ChangeRequestScope {
    host: String,
    path: String,
}

impl ChangeRequestScope {
    /// `path` on `host`, when both have a repository's shape: a DNS name, and
    /// at least two path segments safe to hand a CLI. Whether the host's
    /// provider names repositories that way is for its adapter to say.
    pub fn new(host: &str, path: &str) -> Option<Self> {
        (valid_host(host) && valid_path(path)).then(|| Self {
            host: host.to_ascii_lowercase(),
            path: path.to_owned(),
        })
    }

    /// The wire form: a bare `owner/repo` on github.com, else `host/path`.
    /// Two segments are always the bare form, so a host is never mistaken
    /// for an owner.
    pub fn parse(wire: &str) -> Option<Self> {
        if wire.split('/').count() == 2 {
            return Self::new(LEGACY_HOST, wire);
        }
        let (host, path) = wire.split_once('/')?;
        Self::new(host, path)
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    /// Whether this scope travels in the bare form every engine reads. Any
    /// other scope needs an engine that accepts host-qualified scopes.
    pub fn is_legacy(&self) -> bool {
        self.host == LEGACY_HOST && self.path.split('/').count() == 2
    }

    /// Hosts and repository paths resolve without case on every provider.
    pub fn same_repository(&self, other: &Self) -> bool {
        self.host == other.host && self.path.eq_ignore_ascii_case(&other.path)
    }
}

impl fmt::Display for ChangeRequestScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_legacy() {
            f.write_str(&self.path)
        } else {
            write!(f, "{}/{}", self.host, self.path)
        }
    }
}

/// A hostname as remotes and URLs carry it: never an option, a user or a
/// port. A dot is required, so a mistyped path never reads as a host.
fn valid_host(host: &str) -> bool {
    (1..=253).contains(&host.len())
        && host.contains('.')
        && !host.starts_with(['-', '.'])
        && !host.ends_with('.')
        && !host.contains("..")
        && host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
}

fn valid_path(path: &str) -> bool {
    let segments = path.split('/').count();
    path.len() <= MAX_PATH_LEN
        && (2..=MAX_SEGMENTS).contains(&segments)
        && path.split('/').all(|segment| {
            !segment.is_empty()
                && segment != "."
                && segment != ".."
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_owner_repo_is_github_and_round_trips() {
        let scope = ChangeRequestScope::parse("Acme/zeron.app").unwrap();
        assert_eq!(scope.host(), LEGACY_HOST);
        assert_eq!(scope.path(), "Acme/zeron.app");
        assert!(scope.is_legacy());
        assert_eq!(scope.to_string(), "Acme/zeron.app");
        assert!(scope.same_repository(&ChangeRequestScope::parse("acme/ZERON.app").unwrap()));
        // A qualified github.com repository is the same scope, sent bare.
        let qualified = ChangeRequestScope::parse("GitHub.com/acme/zeron").unwrap();
        assert!(qualified.is_legacy());
        assert_eq!(qualified.to_string(), "acme/zeron");
        // Two segments never name a host.
        assert_eq!(
            ChangeRequestScope::parse("forge.test/app").map(|scope| scope.host().to_owned()),
            Some(LEGACY_HOST.to_owned())
        );
    }

    #[test]
    fn other_hosts_travel_qualified_with_whole_paths() {
        let scope = ChangeRequestScope::parse("Forge.test/team/platform/app").unwrap();
        assert_eq!(scope.host(), "forge.test");
        assert_eq!(scope.path(), "team/platform/app");
        assert!(!scope.is_legacy());
        assert_eq!(scope.to_string(), "forge.test/team/platform/app");
        assert_eq!(
            ChangeRequestScope::parse(&scope.to_string()),
            Some(scope.clone())
        );
        assert!(
            !scope.same_repository(
                &ChangeRequestScope::new("other.test", "team/platform/app").unwrap()
            )
        );
        // github.com paths of another shape are scopes too; its adapter
        // refuses them, not the grammar.
        assert!(
            !ChangeRequestScope::parse("github.com/a/b/c")
                .unwrap()
                .is_legacy()
        );
    }

    #[test]
    fn malformed_scopes_are_refused() {
        let deep = format!("forge.test/{}app", "group/".repeat(MAX_SEGMENTS));
        for wire in [
            "",
            "acme",
            "acme/",
            "/zeron",
            "acme/zeron/extra",
            "repo:acme/zeron",
            "../zeron",
            "acme/zeron repo:other/x",
            "forge.test/team//app",
            "forge.test/team/../app",
            "-forge.test/team/app",
            "forge.test./team/app",
            "forge.test:22/team/app",
            "user@forge.test/team/app",
            "forge.test/team/app?x",
            "forge.test/team/a pp",
            deep.as_str(),
        ] {
            assert_eq!(ChangeRequestScope::parse(wire), None, "{wire}");
        }
        assert_eq!(ChangeRequestScope::new("forge.test", "app"), None);
    }
}
