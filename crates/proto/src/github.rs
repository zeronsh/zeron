//! Read-only GitHub surfaces. Credentials and execution stay on the host.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GitHubResource {
    Repository,
    PullRequests,
    Issues,
    PullRequest(u64),
    Issue(u64),
    Commit(String),
    Compare(String),
    /// Other GitHub web routes stay in the native surface, with the full URL preserved.
    Page(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubTarget {
    pub owner: String,
    pub repository: String,
    pub resource: GitHubResource,
}

impl GitHubTarget {
    /// Every genuine GitHub web link enters the native viewer. Lookalike domains,
    /// credentials in URLs, and non-web schemes retain ordinary link handling.
    pub fn from_url(url: &str) -> Option<Self> {
        if url
            .bytes()
            .any(|b| b.is_ascii_control() || b.is_ascii_whitespace())
        {
            return None;
        }
        let parsed = url::Url::parse(url).ok()?;
        if !matches!(parsed.scheme(), "http" | "https")
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.port().is_some()
            || !matches!(parsed.host_str(), Some("github.com" | "www.github.com"))
            || url.contains('\\')
        {
            return None;
        }
        let parts: Vec<_> = parsed
            .path()
            .trim_end_matches('/')
            .split('/')
            .skip(1)
            .collect();
        let valid = |s: &str| {
            !s.is_empty()
                && s != "."
                && s != ".."
                && !s.starts_with('-')
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        };
        // These are global GitHub routes, not repositories.
        let global = [
            "login",
            "settings",
            "orgs",
            "users",
            "marketplace",
            "features",
            "topics",
            "collections",
            "sponsors",
            "search",
            "notifications",
            "new",
            "explore",
            "dashboard",
            "pulls",
            "issues",
            "discussions",
            "codespaces",
            "sessions",
            "join",
            "logout",
            "organizations",
            "account",
            "contact",
            "about",
            "pricing",
            "security",
        ];
        let owner = parts
            .first()
            .copied()
            .filter(|s| valid(s) && !global.contains(s))
            .unwrap_or_default();
        let repository = parts
            .get(1)
            .copied()
            .filter(|s| !owner.is_empty() && valid(s))
            .unwrap_or_default();
        let mut canonical = parsed.clone();
        canonical.set_scheme("https").ok()?;
        canonical.set_host(Some("github.com")).ok()?;
        let fallback = || GitHubResource::Page(canonical.to_string());
        let number = |s: &str| s.parse::<u64>().ok().filter(|n| *n > 0);
        let resource = if repository.is_empty() {
            fallback()
        } else {
            match &parts[2..] {
                [] => GitHubResource::Repository,
                ["pulls"] => GitHubResource::PullRequests,
                ["issues"] => GitHubResource::Issues,
                ["pull", n] | ["pull", n, "files" | "commits" | "checks"] => number(n)
                    .map(GitHubResource::PullRequest)
                    .unwrap_or_else(fallback),
                ["issues", n] => number(n)
                    .map(GitHubResource::Issue)
                    .unwrap_or_else(fallback),
                ["commit", sha] | ["pull", _, "commits", sha]
                    if (4..=64).contains(&sha.len())
                        && sha.bytes().all(|b| b.is_ascii_hexdigit()) =>
                {
                    GitHubResource::Commit(sha.to_lowercase())
                }
                ["compare", range] if range.contains("...") => {
                    GitHubResource::Compare(range.to_string())
                }
                _ => fallback(),
            }
        };
        Some(Self {
            owner: owner.to_string(),
            repository: repository.to_string(),
            resource,
        })
    }

    pub fn repository_name(&self) -> String {
        if self.repository.is_empty() {
            if self.owner.is_empty() {
                "GitHub".into()
            } else {
                self.owner.clone()
            }
        } else {
            format!("{}/{}", self.owner, self.repository)
        }
    }

    pub fn resolves_checkout(&self) -> bool {
        self.owner.is_empty()
            && self.repository.is_empty()
            && self.resource == GitHubResource::Repository
    }

    pub fn has_diff(&self) -> bool {
        matches!(
            self.resource,
            GitHubResource::PullRequest(_) | GitHubResource::Commit(_) | GitHubResource::Compare(_)
        )
    }

    pub fn url(&self) -> String {
        let suffix = match &self.resource {
            GitHubResource::Repository => String::new(),
            GitHubResource::PullRequests => "/pulls".into(),
            GitHubResource::Issues => "/issues".into(),
            GitHubResource::PullRequest(n) => format!("/pull/{n}"),
            GitHubResource::Issue(n) => format!("/issues/{n}"),
            GitHubResource::Commit(sha) => format!("/commit/{sha}"),
            GitHubResource::Compare(range) => format!("/compare/{range}"),
            GitHubResource::Page(url) => return url.clone(),
        };
        format!("https://github.com/{}{suffix}", self.repository_name())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubListItem {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub state: String,
    pub author: String,
    pub updated_at: String,
    pub draft: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubComment {
    pub author: String,
    pub body: String,
    pub created_at: String,
    pub state: String,
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubCheck {
    pub name: String,
    pub status: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubPage {
    pub target: GitHubTarget,
    pub title: String,
    pub body: String,
    pub author: String,
    pub state: String,
    pub draft: bool,
    pub base_ref: String,
    pub head_ref: String,
    pub additions: u32,
    pub deletions: u32,
    pub changed_files: u32,
    pub items: Vec<GitHubListItem>,
    pub comments: Vec<GitHubComment>,
    pub checks: Vec<GitHubCheck>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubDiff {
    pub patch: String,
    pub truncated: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recognizes_supported_pages_and_canonicalizes_subpages() {
        for (suffix, resource) in [
            ("", GitHubResource::Repository),
            ("/pulls", GitHubResource::PullRequests),
            ("/issues", GitHubResource::Issues),
            (
                "/pull/42/files?diff=split#discussion",
                GitHubResource::PullRequest(42),
            ),
            ("/issues/7#issuecomment-1", GitHubResource::Issue(7)),
            ("/commit/42926c8", GitHubResource::Commit("42926c8".into())),
            (
                "/pull/42/commits/42926c8",
                GitHubResource::Commit("42926c8".into()),
            ),
            (
                "/compare/main...feature",
                GitHubResource::Compare("main...feature".into()),
            ),
        ] {
            let target =
                GitHubTarget::from_url(&format!("https://github.com/acme/project{suffix}"))
                    .unwrap();
            assert_eq!(target.resource, resource);
            assert_eq!(GitHubTarget::from_url(&target.url()), Some(target));
        }
    }
    #[test]
    fn all_github_web_routes_stay_native_and_preserve_unmapped_urls() {
        for url in [
            "https://github.com",
            "https://github.com/zeronsh",
            "https://github.com/acme/project/blob/main/file.rs#L12",
            "https://github.com/acme/project/tree/main/folder",
            "https://github.com/acme/project/actions/runs/123?check_suite_focus=true",
            "https://github.com/acme/project/releases/tag/v1.0",
            "https://github.com/login/device",
            "https://github.com/acme/project/settings",
            "https://github.com/acme/project/pull/0",
            "https://github.com/acme/project/pull/-1",
        ] {
            let target = GitHubTarget::from_url(url).expect(url);
            assert!(matches!(target.resource, GitHubResource::Page(_)), "{url}");
            assert_eq!(GitHubTarget::from_url(&target.url()), Some(target));
        }
        let canonical =
            GitHubTarget::from_url("http://www.github.com/acme/project/commit/ABCDEF").unwrap();
        assert_eq!(
            canonical.url(),
            "https://github.com/acme/project/commit/abcdef"
        );
        let homepage = GitHubTarget::from_url("https://github.com/").unwrap();
        assert!(!homepage.resolves_checkout());
    }
    #[test]
    fn unsafe_urls_keep_ordinary_link_handling() {
        for url in [
            "https://github.com.evil.test/acme/project/pull/1",
            "https://user@github.com/acme/project/pull/1",
            "https://github.com:444/acme/project/pull/1",
            "ftp://github.com/acme/project",
            "https://github.com\\evil.test/acme/project",
            "https://github.com/acme/project/pull/1\n",
        ] {
            assert!(GitHubTarget::from_url(url).is_none(), "{url}");
        }
    }
}
