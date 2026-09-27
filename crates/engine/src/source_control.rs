//! Pure source-control remote and branch normalization.
//!
//! Process execution and provider calls intentionally live outside this layer so
//! remote parsing and head selector construction remain deterministic and testable.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use tokio::io::{AsyncRead, AsyncReadExt};

use zeron_proto::{
    ChangeRequestListItem, ChangeRequestMergeability, ChangeRequestReviewDecision,
    ChangeRequestState, ChangeRequestSummary,
};

const GIT_TIMEOUT: Duration = Duration::from_secs(10);
const GITHUB_TIMEOUT: Duration = Duration::from_secs(20);
const GIT_OUTPUT_LIMIT: usize = 64 * 1024;
const GITHUB_OUTPUT_LIMIT: usize = 1024 * 1024;
const GITHUB_RESULT_LIMIT: &str = "20";
const GITHUB_JSON_FIELDS: &str = "number,title,url,state,baseRefName,headRefName,updatedAt,isCrossRepository,headRepositoryOwner";
const GITHUB_SEARCH_QUERY: &str = "query($search: String!, $owner: String!, $name: String!) { repository(owner: $owner, name: $name) { nameWithOwner } search(query: $search, type: ISSUE, first: 50) { nodes { ... on PullRequest { author { login } number title url state isDraft mergeable reviewDecision createdAt updatedAt additions deletions repository { nameWithOwner } } } } }";

/// Strictly one repository; reject search qualifiers and unscoped requests.
pub fn valid_pr_repository(repository: &str) -> bool {
    let parts: Vec<_> = repository.split('/').collect();
    parts.len() == 2
        && parts.iter().all(|part| {
            !part.is_empty()
                && *part != "."
                && *part != ".."
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        })
}

#[derive(Default)]
struct PrRequestCache {
    entries: Vec<(
        String,
        Instant,
        Result<serde_json::Value, ChangeRequestError>,
    )>,
    rate_limited_at: Option<Instant>,
}

/// Repository identity extracted from a Git remote URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitRemote {
    pub host: String,
    pub owner: String,
    pub repository: String,
}

/// Normalized branch context used to query a change request provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchHeadContext {
    pub local_branch: String,
    pub upstream_ref: Option<String>,
    pub remote_name: Option<String>,
    pub remote_url: Option<String>,
    pub host: Option<String>,
    pub repository: Option<String>,
    pub owner: Option<String>,
    pub head_branch: String,
    pub head_selectors: Vec<String>,
}

/// Git metadata needed by a provider to resolve a branch's change request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutSourceContext {
    pub checkout_root: PathBuf,
    pub branch: BranchHeadContext,
    pub default_branch: Option<String>,
}

/// A provider lookup paired with the exact Git context that produced it.
#[derive(Debug, Clone, PartialEq)]
pub struct ChangeRequestResolution {
    pub source: CheckoutSourceContext,
    pub change_request: Option<ChangeRequestSummary>,
}

/// Safe, provider-facing failure classes. These variants never contain command output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ChangeRequestError {
    #[error("Git repository context is unavailable")]
    RepositoryUnavailable,
    #[error("GitHub is unavailable for this repository")]
    UnsupportedRepository,
    #[error("GitHub CLI is not installed")]
    CliUnavailable,
    #[error("GitHub CLI is not authenticated for this host")]
    Authentication,
    #[error("GitHub API rate limit reached")]
    RateLimited,
    #[error("GitHub request timed out")]
    Timeout,
    #[error("GitHub returned an invalid response")]
    Decode,
    #[error("GitHub request failed")]
    CommandFailed,
}

/// Provider boundary shared by the checkout badge and future source-control views.
#[async_trait]
pub trait ChangeRequestProvider: Send + Sync {
    fn provider(&self) -> &'static str;

    async fn find_for_branch(
        &self,
        source: &CheckoutSourceContext,
    ) -> Result<Option<ChangeRequestSummary>, ChangeRequestError>;
}

/// Host-side boundary for global change request listing and inspection surfaces.
#[async_trait]
pub trait OpenChangeRequestLookup: Send + Sync {
    async fn list_authored_open(
        &self,
        repository: &str,
        refresh: bool,
    ) -> Result<Vec<ChangeRequestListItem>, ChangeRequestError>;
    async fn list_filtered_open(
        &self,
        repository: &str,
        filter: zeron_proto::ChangeRequestFilter,
        refresh: bool,
    ) -> Result<Vec<ChangeRequestListItem>, ChangeRequestError> {
        if filter != zeron_proto::ChangeRequestFilter::Authored {
            return Err(ChangeRequestError::UnsupportedRepository);
        }
        self.list_authored_open(repository, refresh).await
    }
    async fn post_comment(
        &self,
        _url: &str,
        _body: &str,
    ) -> Result<zeron_proto::ChangeRequestComment, ChangeRequestError> {
        Err(ChangeRequestError::UnsupportedRepository)
    }
    async fn detail(
        &self,
        _url: &str,
        _diff: bool,
        _refresh: bool,
    ) -> Result<serde_json::Value, ChangeRequestError> {
        Err(ChangeRequestError::UnsupportedRepository)
    }
}

/// Checkout inspection plus provider resolution, injectable for cache/service tests.
#[async_trait]
pub trait CheckoutChangeRequestLookup: Send + Sync {
    async fn inspect_checkout(
        &self,
        cwd: &Path,
    ) -> Result<CheckoutSourceContext, ChangeRequestError>;

    async fn resolve_github_source(
        &self,
        source: &CheckoutSourceContext,
    ) -> Result<Option<ChangeRequestSummary>, ChangeRequestError>;
}

/// Host-side resolver. All subprocesses run on the device that owns `cwd`.
#[derive(Clone)]
pub struct ChangeRequestResolver {
    inspector: GitCheckoutInspector,
    github: GitHubCli,
}

impl ChangeRequestResolver {
    pub fn new() -> Self {
        let runner: Arc<dyn ProcessRunner> = Arc::new(SystemProcessRunner);
        Self {
            inspector: GitCheckoutInspector::new(runner.clone()),
            github: GitHubCli::with_runner(runner),
        }
    }

    /// Resolve just the selected checkout's identity. Local Git only: no
    /// provider lookup, repository enumeration, or remote transport.
    pub async fn repository_for_checkout(&self, cwd: &Path) -> Option<String> {
        let remote = self
            .inspector
            .git_optional(cwd, &["remote", "get-url", "origin"])
            .await;
        let remote = match remote {
            Some(remote) => parse_git_remote(&remote)?,
            None => {
                let source = self.inspect_checkout(cwd).await.ok()?;
                parse_git_remote(source.branch.remote_url.as_deref()?)?
            }
        };
        (remote.host.eq_ignore_ascii_case("github.com"))
            .then(|| format!("{}/{}", remote.owner, remote.repository))
    }

    pub async fn resolve_github(
        &self,
        cwd: &Path,
    ) -> Result<ChangeRequestResolution, ChangeRequestError> {
        let source = self.inspect_checkout(cwd).await?;
        let change_request = self.resolve_github_source(&source).await?;
        Ok(ChangeRequestResolution {
            source,
            change_request,
        })
    }

    /// Inspect Git separately so callers can key caches before a provider request.
    pub async fn inspect_checkout(
        &self,
        cwd: &Path,
    ) -> Result<CheckoutSourceContext, ChangeRequestError> {
        self.inspector.inspect(cwd).await
    }

    /// Resolve an already-inspected checkout without repeating Git subprocesses.
    pub async fn resolve_github_source(
        &self,
        source: &CheckoutSourceContext,
    ) -> Result<Option<ChangeRequestSummary>, ChangeRequestError> {
        if source.branch.host.is_none()
            || source.branch.owner.is_none()
            || source.branch.repository.is_none()
        {
            return Err(ChangeRequestError::UnsupportedRepository);
        }
        self.github.find_for_branch(source).await
    }
}

impl Default for ChangeRequestResolver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CheckoutChangeRequestLookup for ChangeRequestResolver {
    async fn inspect_checkout(
        &self,
        cwd: &Path,
    ) -> Result<CheckoutSourceContext, ChangeRequestError> {
        ChangeRequestResolver::inspect_checkout(self, cwd).await
    }

    async fn resolve_github_source(
        &self,
        source: &CheckoutSourceContext,
    ) -> Result<Option<ChangeRequestSummary>, ChangeRequestError> {
        ChangeRequestResolver::resolve_github_source(self, source).await
    }
}

/// GitHub provider backed by the host's existing `gh` installation and auth.
#[derive(Clone)]
pub struct GitHubCli {
    runner: Arc<dyn ProcessRunner>,
    pr_cache: Arc<tokio::sync::Mutex<PrRequestCache>>,
}

impl GitHubCli {
    /// Shared across RPC clients on this engine. The lock also serializes distinct
    /// provider reads and coalesces simultaneous identical reads. Never retry here.
    async fn cached_pr_request<F>(
        &self,
        key: String,
        refresh: bool,
        fetch: F,
    ) -> Result<serde_json::Value, ChangeRequestError>
    where
        F: std::future::Future<Output = Result<serde_json::Value, ChangeRequestError>> + Send,
    {
        let mut cache = self.pr_cache.lock().await;
        if let Some(index) = cache.entries.iter().position(|entry| entry.0 == key) {
            let entry = cache.entries.remove(index);
            let ttl = match &entry.2 {
                Err(ChangeRequestError::RateLimited) => Duration::from_secs(15 * 60),
                Err(_) => Duration::from_secs(60),
                Ok(_) if refresh => Duration::from_secs(15),
                Ok(_) => Duration::from_secs(5 * 60),
            };
            let fresh = entry.1.elapsed() < ttl;
            let result = entry.2.clone();
            cache.entries.push(entry);
            if fresh {
                return result;
            }
        }
        if cache
            .rate_limited_at
            .is_some_and(|at| at.elapsed() < Duration::from_secs(15 * 60))
        {
            return Err(ChangeRequestError::RateLimited);
        }
        let result = fetch.await;
        if matches!(result, Err(ChangeRequestError::RateLimited)) {
            cache.rate_limited_at = Some(Instant::now());
        }
        cache.entries.retain(|entry| entry.0 != key);
        cache.entries.push((key, Instant::now(), result.clone()));
        if cache.entries.len() > 24 {
            let _ = cache.entries.remove(0);
        }
        result
    }

    pub async fn detail(
        &self,
        url: &str,
        diff: bool,
        refresh: bool,
    ) -> Result<serde_json::Value, ChangeRequestError> {
        let url = validated_pull_request_url(url)?;
        self.cached_pr_request(
            format!("{diff}:{url}"),
            refresh,
            self.fetch_detail(&url, diff),
        )
        .await
    }

    /// Explicit user submission only. Never retry a write after an ambiguous failure.
    pub async fn post_comment(
        &self,
        url: &str,
        body: &str,
    ) -> Result<zeron_proto::ChangeRequestComment, ChangeRequestError> {
        let url = validated_pull_request_url(url)?;
        if body.trim().is_empty() || body.len() > 60_000 {
            return Err(ChangeRequestError::Decode);
        }
        let parsed = reqwest::Url::parse(&url).map_err(|_| ChangeRequestError::Decode)?;
        let parts: Vec<_> = parsed.path().trim_matches('/').split('/').collect();
        let endpoint = format!(
            "repos/{}/{}/issues/{}/comments",
            parts[0], parts[1], parts[3]
        );
        let output = self
            .runner
            .run(ProcessRequest {
                program: "gh".into(),
                args: vec![
                    "api".into(),
                    "--method".into(),
                    "POST".into(),
                    endpoint,
                    "-f".into(),
                    format!("body={body}"),
                ],
                cwd: None,
                env: vec![
                    ("GH_PROMPT_DISABLED".into(), "1".into()),
                    ("GH_HOST".into(), "github.com".into()),
                ],
                timeout: GITHUB_TIMEOUT,
                output_limit: GITHUB_OUTPUT_LIMIT,
            })
            .await
            .map_err(classify_run_error)?;
        if !output.success {
            return Err(classify_github_failure(&output.stderr));
        }
        // Evict after the server accepted the write, even if its reply cannot be decoded.
        self.pr_cache
            .lock()
            .await
            .entries
            .retain(|entry| entry.0 != format!("false:{url}"));
        if output.stdout_truncated {
            return Err(ChangeRequestError::Decode);
        }
        let value: serde_json::Value =
            serde_json::from_slice(&output.stdout).map_err(|_| ChangeRequestError::Decode)?;
        Ok(zeron_proto::ChangeRequestComment {
            viewer_did_author: true,
            body: value["body"]
                .as_str()
                .ok_or(ChangeRequestError::Decode)?
                .into(),
            author: zeron_proto::ChangeRequestActor {
                login: value["user"]["login"].as_str().unwrap_or_default().into(),
            },
            created_at: value["created_at"].as_str().unwrap_or_default().into(),
            ..Default::default()
        })
    }

    /// Fetch one PR without requiring a local checkout or executing shell text.
    async fn fetch_detail(
        &self,
        url: &str,
        diff: bool,
    ) -> Result<serde_json::Value, ChangeRequestError> {
        let url = validated_pull_request_url(url)?;
        let mut args = vec![
            "pr".into(),
            if diff { "diff".into() } else { "view".into() },
            url,
        ];
        if diff {
            args.push("--color=never".into());
        } else {
            args.extend(["--json".into(), "title,body,url,number,author,baseRefName,headRefName,state,isDraft,reviewDecision,mergeable,additions,deletions,comments,reviews,files,statusCheckRollup".into()]);
        }
        let output = self
            .runner
            .run(ProcessRequest {
                program: "gh".into(),
                args,
                cwd: None,
                env: vec![
                    ("GH_PROMPT_DISABLED".into(), "1".into()),
                    ("GH_HOST".into(), "github.com".into()),
                ],
                timeout: GITHUB_TIMEOUT,
                output_limit: GITHUB_OUTPUT_LIMIT,
            })
            .await
            .map_err(classify_run_error)?;
        if !output.success {
            return Err(classify_github_failure(&output.stderr));
        }
        if output.stdout_truncated {
            return Err(ChangeRequestError::Decode);
        }
        if diff {
            Ok(serde_json::Value::String(
                String::from_utf8_lossy(&output.stdout).into_owned(),
            ))
        } else {
            let mut value: serde_json::Value =
                serde_json::from_slice(&output.stdout).map_err(|_| ChangeRequestError::Decode)?;
            // GitHub uses null for absent check/review data; the wire model uses defaults.
            fn remove_nulls(value: &mut serde_json::Value) {
                match value {
                    serde_json::Value::Object(map) => {
                        map.retain(|_, v| !v.is_null());
                        for v in map.values_mut() {
                            remove_nulls(v);
                        }
                    }
                    serde_json::Value::Array(items) => {
                        for v in items {
                            remove_nulls(v);
                        }
                    }
                    _ => {}
                }
            }
            remove_nulls(&mut value);
            let detail: zeron_proto::ChangeRequestDetail =
                serde_json::from_value(value).map_err(|_| ChangeRequestError::Decode)?;
            serde_json::to_value(detail).map_err(|_| ChangeRequestError::Decode)
        }
    }

    pub fn new() -> Self {
        Self::with_runner(Arc::new(SystemProcessRunner))
    }

    fn with_runner(runner: Arc<dyn ProcessRunner>) -> Self {
        Self {
            runner,
            pr_cache: Default::default(),
        }
    }

    /// List open pull requests authored by the active GitHub CLI account.
    pub async fn list_authored_open(
        &self,
        repository: &str,
        refresh: bool,
    ) -> Result<Vec<ChangeRequestListItem>, ChangeRequestError> {
        self.list_filtered_open(
            repository,
            zeron_proto::ChangeRequestFilter::Authored,
            refresh,
        )
        .await
    }

    pub async fn list_filtered_open(
        &self,
        repository: &str,
        filter: zeron_proto::ChangeRequestFilter,
        refresh: bool,
    ) -> Result<Vec<ChangeRequestListItem>, ChangeRequestError> {
        if !valid_pr_repository(repository) {
            return Err(ChangeRequestError::UnsupportedRepository);
        }
        let repository = repository.to_ascii_lowercase();
        let result = self
            .cached_pr_request(format!("list:{repository}:{filter:?}"), refresh, async {
                serde_json::to_value(self.fetch_filtered_open(&repository, filter).await?)
                    .map_err(|_| ChangeRequestError::Decode)
            })
            .await?;
        serde_json::from_value(result).map_err(|_| ChangeRequestError::Decode)
    }

    async fn fetch_filtered_open(
        &self,
        repository: &str,
        filter: zeron_proto::ChangeRequestFilter,
    ) -> Result<Vec<ChangeRequestListItem>, ChangeRequestError> {
        let (items, canonical) = self.fetch_scoped_search(repository, filter).await?;
        if items.is_empty()
            && let Some(canonical) = canonical
            && !canonical.eq_ignore_ascii_case(repository)
        {
            // Search does not follow repository renames, although repository()
            // does. Follow only that verified canonical name, at most once.
            return self
                .fetch_scoped_search(&canonical, filter)
                .await
                .map(|result| result.0);
        }
        Ok(items)
    }

    async fn fetch_scoped_search(
        &self,
        repository: &str,
        filter: zeron_proto::ChangeRequestFilter,
    ) -> Result<(Vec<ChangeRequestListItem>, Option<String>), ChangeRequestError> {
        let (owner, name) = repository
            .split_once('/')
            .ok_or(ChangeRequestError::UnsupportedRepository)?;
        let qualifier = match filter {
            zeron_proto::ChangeRequestFilter::All => "",
            zeron_proto::ChangeRequestFilter::Authored => "author:@me ",
            zeron_proto::ChangeRequestFilter::Reviewing => "review-requested:@me ",
        };
        let request = ProcessRequest {
            program: "gh".into(),
            args: vec![
                "api".into(),
                "graphql".into(),
                "-f".into(),
                format!("query={GITHUB_SEARCH_QUERY}"),
                "-f".into(),
                format!("owner={owner}"),
                "-f".into(),
                format!("name={name}"),
                "-f".into(),
                format!("search=is:pr is:open {qualifier}repo:{repository} sort:updated-desc"),
            ],
            cwd: None,
            env: vec![
                ("GH_PROMPT_DISABLED".into(), "1".into()),
                ("GH_HOST".into(), "github.com".into()),
            ],
            timeout: GITHUB_TIMEOUT,
            output_limit: GITHUB_OUTPUT_LIMIT,
        };
        let output = self.runner.run(request).await.map_err(classify_run_error)?;
        if !output.success {
            return Err(classify_github_failure(&output.stderr));
        }
        if output.stdout_truncated {
            return Err(ChangeRequestError::Decode);
        }

        let response: GhSearchResponse =
            serde_json::from_slice(&output.stdout).map_err(|_| ChangeRequestError::Decode)?;
        let canonical = response.data.repository.map(|repo| repo.name_with_owner);
        if canonical
            .as_deref()
            .is_some_and(|repo| !valid_pr_repository(repo))
        {
            return Err(ChangeRequestError::Decode);
        }
        let mut items = response
            .data
            .search
            .nodes
            .into_iter()
            .map(to_list_item)
            .collect::<Result<Vec<_>, _>>()?;
        // GitHub can return the canonical name of a renamed repository.
        // Scope is enforced by the query, not by matching an old remote name.
        items.truncate(50);
        items.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| left.repository.cmp(&right.repository))
                .then_with(|| left.number.cmp(&right.number))
        });
        Ok((items, canonical))
    }

    async fn list_for_selector(
        &self,
        source: &CheckoutSourceContext,
        selector: &str,
    ) -> Result<Vec<GhPullRequest>, ChangeRequestError> {
        let request = ProcessRequest {
            program: "gh".into(),
            args: vec![
                "pr".into(),
                "list".into(),
                "--head".into(),
                selector.into(),
                "--state".into(),
                "all".into(),
                "--limit".into(),
                GITHUB_RESULT_LIMIT.into(),
                "--json".into(),
                GITHUB_JSON_FIELDS.into(),
            ],
            cwd: Some(source.checkout_root.clone()),
            env: vec![("GH_PROMPT_DISABLED".into(), "1".into())],
            timeout: GITHUB_TIMEOUT,
            output_limit: GITHUB_OUTPUT_LIMIT,
        };
        let output = self.runner.run(request).await.map_err(classify_run_error)?;
        if !output.success {
            return Err(classify_github_failure(&output.stderr));
        }
        if output.stdout_truncated {
            return Err(ChangeRequestError::Decode);
        }
        serde_json::from_slice(&output.stdout).map_err(|_| ChangeRequestError::Decode)
    }

    /// Resolve the remote's default branch through the provider API.
    ///
    /// This deliberately never uses Git remote transport (`git ls-remote`)
    /// against the watched checkout, which would honor repository-controlled
    /// transport and credential configuration. `gh` queries the
    /// already-sanitized host/owner/repository identity with its own
    /// credentials. Failures degrade to "unknown", matching the pre-existing
    /// behavior when a default branch cannot be determined.
    async fn default_branch(&self, source: &CheckoutSourceContext) -> Option<String> {
        let host = source.branch.host.as_deref()?;
        let owner = source.branch.owner.as_deref()?;
        let repository = source.branch.repository.as_deref()?;
        let request = ProcessRequest {
            program: "gh".into(),
            args: vec![
                "repo".into(),
                "view".into(),
                format!("{host}/{owner}/{repository}"),
                "--json".into(),
                "defaultBranchRef".into(),
            ],
            cwd: Some(source.checkout_root.clone()),
            env: vec![("GH_PROMPT_DISABLED".into(), "1".into())],
            timeout: GITHUB_TIMEOUT,
            output_limit: GITHUB_OUTPUT_LIMIT,
        };
        let output = self.runner.run(request).await.ok()?;
        if !output.success || output.stdout_truncated {
            return None;
        }
        let view: GhRepoView = serde_json::from_slice(&output.stdout).ok()?;
        view.default_branch_ref
            .map(|reference| reference.name)
            .filter(|name| !name.is_empty())
    }
}

impl Default for GitHubCli {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl OpenChangeRequestLookup for GitHubCli {
    async fn list_authored_open(
        &self,
        repository: &str,
        refresh: bool,
    ) -> Result<Vec<ChangeRequestListItem>, ChangeRequestError> {
        GitHubCli::list_authored_open(self, repository, refresh).await
    }
    async fn list_filtered_open(
        &self,
        repository: &str,
        filter: zeron_proto::ChangeRequestFilter,
        refresh: bool,
    ) -> Result<Vec<ChangeRequestListItem>, ChangeRequestError> {
        GitHubCli::list_filtered_open(self, repository, filter, refresh).await
    }
    async fn post_comment(
        &self,
        url: &str,
        body: &str,
    ) -> Result<zeron_proto::ChangeRequestComment, ChangeRequestError> {
        GitHubCli::post_comment(self, url, body).await
    }
    async fn detail(
        &self,
        url: &str,
        diff: bool,
        refresh: bool,
    ) -> Result<serde_json::Value, ChangeRequestError> {
        GitHubCli::detail(self, url, diff, refresh).await
    }
}

#[async_trait]
impl ChangeRequestProvider for GitHubCli {
    fn provider(&self) -> &'static str {
        "github"
    }

    async fn find_for_branch(
        &self,
        source: &CheckoutSourceContext,
    ) -> Result<Option<ChangeRequestSummary>, ChangeRequestError> {
        if source.branch.host.is_none()
            || source.branch.owner.is_none()
            || source.branch.repository.is_none()
        {
            return Err(ChangeRequestError::UnsupportedRepository);
        }
        if source.branch.head_selectors.is_empty() {
            return Ok(None);
        }

        for selector in &source.branch.head_selectors {
            let candidates = self.list_for_selector(source, selector).await?;
            if candidates.is_empty() {
                continue;
            }
            let default_branch = match source.default_branch.clone() {
                Some(branch) => Some(branch),
                None if needs_default_branch(source, &candidates) => {
                    self.default_branch(source).await
                }
                None => None,
            };
            return select_pull_request(
                self.provider(),
                source,
                default_branch.as_deref(),
                candidates,
            );
        }
        Ok(None)
    }
}

impl BranchHeadContext {
    /// Build provider head selectors from already-inspected Git branch metadata.
    pub fn resolve(
        local_branch: impl Into<String>,
        upstream_ref: Option<&str>,
        remote_name: Option<&str>,
        remote_url: Option<&str>,
    ) -> Self {
        let local_branch = local_branch.into();
        let upstream_ref = non_empty(upstream_ref).map(str::to_owned);
        let remote_name = non_empty(remote_name)
            .map(str::to_owned)
            .or_else(|| upstream_ref.as_deref().and_then(remote_from_upstream));
        let remote_url = non_empty(remote_url).map(str::to_owned);
        let parsed_remote = remote_url.as_deref().and_then(parse_git_remote);
        let upstream_branch = upstream_ref
            .as_deref()
            .and_then(|upstream| branch_from_upstream(upstream, remote_name.as_deref()));
        let head_branch = upstream_branch.unwrap_or(&local_branch).to_owned();

        let mut head_selectors = Vec::new();
        if let Some(remote) = parsed_remote.as_ref() {
            push_unique(
                &mut head_selectors,
                format!("{}:{head_branch}", remote.owner),
            );
        }
        if upstream_ref.is_some() {
            push_unique(&mut head_selectors, head_branch.clone());
        }
        if upstream_ref.is_none() || local_branch == head_branch {
            push_unique(&mut head_selectors, local_branch.clone());
        }

        Self {
            local_branch,
            upstream_ref,
            remote_name,
            remote_url,
            host: parsed_remote.as_ref().map(|remote| remote.host.clone()),
            repository: parsed_remote
                .as_ref()
                .map(|remote| remote.repository.clone()),
            owner: parsed_remote.map(|remote| remote.owner),
            head_branch,
            head_selectors,
        }
    }
}

/// Parse the SSH scp-like, SSH URL, and HTTP(S) forms commonly used by GitHub.
///
/// The host is intentionally not restricted to `github.com`: GitHub Enterprise
/// installations use arbitrary hostnames and are resolved later by the provider.
pub fn parse_git_remote(remote_url: &str) -> Option<GitRemote> {
    let remote_url = remote_url.trim();
    if remote_url.is_empty() || remote_url.chars().any(char::is_whitespace) {
        return None;
    }

    let (host, path) = if let Some((scheme, remainder)) = remote_url.split_once("://") {
        if !matches!(
            scheme.to_ascii_lowercase().as_str(),
            "ssh" | "http" | "https"
        ) {
            return None;
        }
        let (authority, path) = remainder.split_once('/')?;
        (host_from_authority(authority)?, path)
    } else {
        let (authority, path) = remote_url.split_once(':')?;
        if authority.contains('/') || path.starts_with('/') {
            return None;
        }
        (host_from_authority(authority)?, path)
    };

    let path = path.trim_matches('/');
    let mut segments = path.split('/');
    let owner = segments.next()?;
    let repository = segments.next()?;
    if segments.next().is_some() || owner.is_empty() || repository.is_empty() {
        return None;
    }
    let repository = repository.strip_suffix(".git").unwrap_or(repository);
    if repository.is_empty() {
        return None;
    }

    Some(GitRemote {
        host: host.to_ascii_lowercase(),
        owner: owner.to_owned(),
        repository: repository.to_owned(),
    })
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn host_from_authority(authority: &str) -> Option<&str> {
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = host.split_once(':').map_or(host, |(host, _)| host);
    (!host.is_empty()).then_some(host)
}

fn remote_from_upstream(upstream: &str) -> Option<String> {
    normalized_upstream(upstream)
        .split_once('/')
        .map(|(remote, _)| remote.to_owned())
}

fn branch_from_upstream<'a>(upstream: &'a str, remote_name: Option<&str>) -> Option<&'a str> {
    let upstream = normalized_upstream(upstream);
    if let Some(remote_name) = remote_name {
        let prefix = format!("{remote_name}/");
        if let Some(branch) = upstream.strip_prefix(&prefix) {
            return (!branch.is_empty()).then_some(branch);
        }
    }
    upstream
        .split_once('/')
        .and_then(|(_, branch)| (!branch.is_empty()).then_some(branch))
}

fn normalized_upstream(upstream: &str) -> &str {
    upstream.strip_prefix("refs/remotes/").unwrap_or(upstream)
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.contains(&value) {
        values.push(value);
    }
}

#[derive(Clone)]
struct GitCheckoutInspector {
    runner: Arc<dyn ProcessRunner>,
}

impl GitCheckoutInspector {
    fn new(runner: Arc<dyn ProcessRunner>) -> Self {
        Self { runner }
    }

    async fn inspect(&self, cwd: &Path) -> Result<CheckoutSourceContext, ChangeRequestError> {
        let checkout_root_value = self
            .git_required(cwd, &["rev-parse", "--show-toplevel"])
            .await?;
        if checkout_root_value.is_empty() {
            return Err(ChangeRequestError::RepositoryUnavailable);
        }
        let checkout_root = PathBuf::from(checkout_root_value);
        let local_branch = self
            .git_required(&checkout_root, &["branch", "--show-current"])
            .await?;
        if local_branch.is_empty() || local_branch == "HEAD" {
            return Err(ChangeRequestError::RepositoryUnavailable);
        }

        let upstream_ref = self
            .git_optional(
                &checkout_root,
                &[
                    "rev-parse",
                    "--abbrev-ref",
                    "--symbolic-full-name",
                    "@{upstream}",
                ],
            )
            .await;
        let branch_remote_key = format!("branch.{local_branch}.remote");
        let mut remote_name = self
            .git_optional(&checkout_root, &["config", "--get", &branch_remote_key])
            .await
            .filter(|remote| remote != ".");
        if remote_name.is_none() {
            remote_name = upstream_ref.as_deref().and_then(remote_from_upstream);
        }
        if remote_name.is_none() {
            remote_name = self
                .git_optional(&checkout_root, &["config", "--get", "remote.pushDefault"])
                .await;
        }
        if remote_name.is_none()
            && let Some(remotes) = self.git_optional(&checkout_root, &["remote"]).await
        {
            let remotes: Vec<_> = remotes
                .lines()
                .filter(|remote| !remote.is_empty())
                .collect();
            // `origin` is Git's conventional default destination when no
            // branch/upstream or push-default configuration is present. It is
            // still the useful PR source for a local branch in a fork +
            // upstream checkout.
            remote_name = remotes
                .iter()
                .find(|remote| **remote == "origin")
                .map(|remote| (*remote).to_owned())
                .or_else(|| (remotes.len() == 1).then(|| remotes[0].to_owned()));
        }

        let remote_url = if let Some(remote) = remote_name.as_deref() {
            self.git_optional(&checkout_root, &["remote", "get-url", "--push", remote])
                .await
        } else {
            None
        };
        let remote_url = if remote_url.is_none() {
            if let Some(remote) = remote_name.as_deref() {
                self.git_optional(&checkout_root, &["remote", "get-url", remote])
                    .await
            } else {
                None
            }
        } else {
            remote_url
        };

        let branch = BranchHeadContext::resolve(
            local_branch,
            upstream_ref.as_deref(),
            remote_name.as_deref(),
            remote_url.as_deref(),
        );
        // The default branch is read from local refs only. `refs/remotes/
        // <remote>/HEAD` is a fetch-time cache and may be absent; the provider
        // resolves the remote's default branch itself when needed. Never fall
        // back to `git ls-remote` here: it executes repository-controlled
        // transport and credential configuration (for example
        // `core.sshCommand`), which would let any watched checkout run
        // arbitrary commands on the host.
        let default_branch = if let Some(remote) = branch.remote_name.as_deref() {
            let remote_head = format!("refs/remotes/{remote}/HEAD");
            self.git_optional(
                &checkout_root,
                &["symbolic-ref", "--quiet", "--short", &remote_head],
            )
            .await
            .and_then(|reference| branch_from_upstream(&reference, Some(remote)).map(str::to_owned))
        } else {
            None
        };

        Ok(CheckoutSourceContext {
            checkout_root,
            branch,
            default_branch,
        })
    }

    async fn git_required(&self, cwd: &Path, args: &[&str]) -> Result<String, ChangeRequestError> {
        self.git(cwd, args)
            .await
            .ok_or(ChangeRequestError::RepositoryUnavailable)
    }

    async fn git_optional(&self, cwd: &Path, args: &[&str]) -> Option<String> {
        self.git(cwd, args).await
    }

    async fn git(&self, cwd: &Path, args: &[&str]) -> Option<String> {
        let output = self
            .runner
            .run(ProcessRequest {
                program: "git".into(),
                args: args.iter().map(|arg| (*arg).to_owned()).collect(),
                cwd: Some(cwd.to_owned()),
                env: Vec::new(),
                timeout: GIT_TIMEOUT,
                output_limit: GIT_OUTPUT_LIMIT,
            })
            .await
            .ok()?;
        if !output.success || output.stdout_truncated {
            return None;
        }
        String::from_utf8(output.stdout)
            .ok()
            .map(|value| value.trim().to_owned())
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhRepoView {
    default_branch_ref: Option<GhBranchRef>,
}

#[derive(Debug, Deserialize)]
struct GhBranchRef {
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhPullRequest {
    number: u64,
    title: String,
    url: String,
    state: GhPullRequestState,
    base_ref_name: String,
    head_ref_name: String,
    updated_at: DateTime<Utc>,
    is_cross_repository: bool,
    head_repository_owner: GhRepositoryOwner,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhSearchPullRequest {
    #[serde(default)]
    author: Option<zeron_proto::ChangeRequestActor>,
    number: u64,
    title: String,
    url: String,
    state: GhPullRequestState,
    repository: GhSearchRepository,
    mergeable: GhMergeability,
    review_decision: Option<GhReviewDecision>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    is_draft: bool,
    additions: u64,
    deletions: u64,
}

#[derive(Debug, Deserialize)]
struct GhSearchResponse {
    data: GhSearchData,
}

#[derive(Debug, Deserialize)]
struct GhSearchData {
    #[serde(default)]
    repository: Option<GhSearchRepository>,
    search: GhSearchConnection,
}

#[derive(Debug, Deserialize)]
struct GhSearchConnection {
    nodes: Vec<GhSearchPullRequest>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhSearchRepository {
    name_with_owner: String,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum GhPullRequestState {
    Open,
    Closed,
    Merged,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum GhMergeability {
    Mergeable,
    Conflicting,
    Unknown,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum GhReviewDecision {
    Approved,
    ChangesRequested,
    ReviewRequired,
}

impl From<GhPullRequestState> for ChangeRequestState {
    fn from(state: GhPullRequestState) -> Self {
        match state {
            GhPullRequestState::Open => Self::Open,
            GhPullRequestState::Closed => Self::Closed,
            GhPullRequestState::Merged => Self::Merged,
        }
    }
}

impl From<GhMergeability> for ChangeRequestMergeability {
    fn from(mergeability: GhMergeability) -> Self {
        match mergeability {
            GhMergeability::Mergeable => Self::Mergeable,
            GhMergeability::Conflicting => Self::Conflicting,
            GhMergeability::Unknown => Self::Unknown,
        }
    }
}

impl From<GhReviewDecision> for ChangeRequestReviewDecision {
    fn from(decision: GhReviewDecision) -> Self {
        match decision {
            GhReviewDecision::Approved => Self::Approved,
            GhReviewDecision::ChangesRequested => Self::ChangesRequested,
            GhReviewDecision::ReviewRequired => Self::ReviewRequired,
        }
    }
}

#[derive(Debug, Deserialize)]
struct GhRepositoryOwner {
    login: String,
}

/// The default branch only matters to suppress historical terminal pull
/// requests on the default branch itself; skip the extra provider request
/// unless a terminal candidate could actually be suppressed.
fn needs_default_branch(source: &CheckoutSourceContext, candidates: &[GhPullRequest]) -> bool {
    candidates.iter().any(|candidate| {
        candidate.head_ref_name == source.branch.head_branch
            && !matches!(candidate.state, GhPullRequestState::Open)
    })
}

fn select_pull_request(
    provider: &str,
    source: &CheckoutSourceContext,
    default_branch: Option<&str>,
    candidates: Vec<GhPullRequest>,
) -> Result<Option<ChangeRequestSummary>, ChangeRequestError> {
    let expected_owner = source.branch.owner.as_deref();
    let on_default_branch = default_branch == Some(&source.branch.head_branch);
    let mut matching: Vec<GhPullRequest> = candidates
        .into_iter()
        .filter(|candidate| candidate.head_ref_name == source.branch.head_branch)
        .filter(|candidate| {
            expected_owner.is_some_and(|owner| {
                candidate
                    .head_repository_owner
                    .login
                    .eq_ignore_ascii_case(owner)
            }) || (expected_owner.is_none() && !candidate.is_cross_repository)
        })
        .filter(|candidate| {
            !on_default_branch || matches!(candidate.state, GhPullRequestState::Open)
        })
        .collect();

    matching.sort_by(|left, right| {
        let left_open = matches!(left.state, GhPullRequestState::Open);
        let right_open = matches!(right.state, GhPullRequestState::Open);
        right_open
            .cmp(&left_open)
            .then_with(|| right.updated_at.cmp(&left.updated_at))
            .then_with(|| right.number.cmp(&left.number))
    });

    matching
        .into_iter()
        .next()
        .map(|pull_request| to_summary(provider, pull_request))
        .transpose()
}

fn to_summary(
    provider: &str,
    pull_request: GhPullRequest,
) -> Result<ChangeRequestSummary, ChangeRequestError> {
    if pull_request.number == 0
        || pull_request.title.trim().is_empty()
        || pull_request.base_ref_name.trim().is_empty()
        || pull_request.head_ref_name.trim().is_empty()
    {
        return Err(ChangeRequestError::Decode);
    }
    let url =
        reqwest::Url::parse(pull_request.url.trim()).map_err(|_| ChangeRequestError::Decode)?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(ChangeRequestError::Decode);
    }

    Ok(ChangeRequestSummary {
        provider: provider.to_owned(),
        number: pull_request.number,
        title: pull_request.title,
        url: url.to_string(),
        state: pull_request.state.into(),
        base_ref: pull_request.base_ref_name,
        head_ref: pull_request.head_ref_name,
    })
}

fn to_list_item(
    pull_request: GhSearchPullRequest,
) -> Result<ChangeRequestListItem, ChangeRequestError> {
    let repository = pull_request.repository.name_with_owner.trim();
    let mut repository_parts = repository.split('/');
    let owner = repository_parts.next().unwrap_or_default();
    let name = repository_parts.next().unwrap_or_default();
    if pull_request.number == 0
        || owner.is_empty()
        || name.is_empty()
        || repository_parts.next().is_some()
        || repository.chars().any(char::is_whitespace)
        || !matches!(pull_request.state, GhPullRequestState::Open)
    {
        return Err(ChangeRequestError::Decode);
    }

    let title = pull_request
        .title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if title.is_empty() {
        return Err(ChangeRequestError::Decode);
    }
    let url =
        reqwest::Url::parse(pull_request.url.trim()).map_err(|_| ChangeRequestError::Decode)?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(ChangeRequestError::Decode);
    }
    Ok(ChangeRequestListItem {
        provider: "github".into(),
        author: pull_request.author.unwrap_or_default(),
        repository: repository.into(),
        number: pull_request.number,
        title,
        url: url.to_string(),
        state: pull_request.state.into(),
        is_draft: pull_request.is_draft,
        review_decision: pull_request
            .review_decision
            .map(Into::into)
            .unwrap_or_default(),
        additions: pull_request.additions,
        deletions: pull_request.deletions,
        mergeability: pull_request.mergeable.into(),
        created_at: pull_request.created_at,
        updated_at: pull_request.updated_at,
    })
}

fn validated_pull_request_url(raw: &str) -> Result<String, ChangeRequestError> {
    let url = reqwest::Url::parse(raw).map_err(|_| ChangeRequestError::UnsupportedRepository)?;
    let parts: Vec<_> = url.path().trim_matches('/').split('/').collect();
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || parts.len() != 4
        || parts[2] != "pull"
        || parts[0].is_empty()
        || parts[1].is_empty()
        || !parts[0..2].iter().all(|part| {
            part.chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        })
        || parts[3]
            .parse::<u64>()
            .ok()
            .is_none_or(|number| number == 0)
    {
        return Err(ChangeRequestError::UnsupportedRepository);
    }
    Ok(format!(
        "https://github.com/{}/{}/pull/{}",
        parts[0], parts[1], parts[3]
    ))
}

fn classify_github_failure(stderr: &[u8]) -> ChangeRequestError {
    let stderr = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    if stderr.contains("rate limit") || stderr.contains("secondary rate") {
        ChangeRequestError::RateLimited
    } else if stderr.contains("not logged")
        || stderr.contains("authentication")
        || stderr.contains("authenticate")
        || stderr.contains("gh auth login")
        || stderr.contains("bad credentials")
    {
        ChangeRequestError::Authentication
    } else {
        ChangeRequestError::CommandFailed
    }
}

fn classify_run_error(error: ProcessRunError) -> ChangeRequestError {
    match error {
        ProcessRunError::Spawn(io::ErrorKind::NotFound) => ChangeRequestError::CliUnavailable,
        ProcessRunError::Timeout => ChangeRequestError::Timeout,
        ProcessRunError::Spawn(_) | ProcessRunError::Io => ChangeRequestError::CommandFailed,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProcessRequest {
    program: String,
    args: Vec<String>,
    cwd: Option<PathBuf>,
    env: Vec<(String, String)>,
    timeout: Duration,
    output_limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProcessOutput {
    success: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    stdout_truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProcessRunError {
    Spawn(io::ErrorKind),
    Timeout,
    Io,
}

#[async_trait]
trait ProcessRunner: Send + Sync {
    async fn run(&self, request: ProcessRequest) -> Result<ProcessOutput, ProcessRunError>;
}

struct SystemProcessRunner;

#[async_trait]
impl ProcessRunner for SystemProcessRunner {
    async fn run(&self, request: ProcessRequest) -> Result<ProcessOutput, ProcessRunError> {
        let mut command = tokio::process::Command::new(&request.program);
        if request.program == "gh" {
            zeron_harness::compose_login_shell_path(&mut command);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.as_std_mut().creation_flags(0x08000000);
        }
        command.args(&request.args);
        if let Some(cwd) = &request.cwd {
            command.current_dir(cwd);
        }
        command
            .envs(request.env)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .map_err(|error| ProcessRunError::Spawn(error.kind()))?;
        let stdout = child.stdout.take().ok_or(ProcessRunError::Io)?;
        let stderr = child.stderr.take().ok_or(ProcessRunError::Io)?;
        let completed = tokio::time::timeout(request.timeout, async {
            tokio::try_join!(
                child.wait(),
                read_capped(stdout, request.output_limit),
                read_capped(stderr, request.output_limit),
            )
        })
        .await;

        let (status, (stdout, stdout_truncated), (stderr, _stderr_truncated)) = match completed {
            Ok(Ok(output)) => output,
            Ok(Err(_)) => return Err(ProcessRunError::Io),
            Err(_) => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Err(ProcessRunError::Timeout);
            }
        };
        Ok(ProcessOutput {
            success: status.success(),
            stdout,
            stderr,
            stdout_truncated,
        })
    }
}

async fn read_capped(
    mut reader: impl AsyncRead + Unpin,
    limit: usize,
) -> io::Result<(Vec<u8>, bool)> {
    let mut output = Vec::with_capacity(limit.min(16 * 1024));
    let mut buffer = [0_u8; 8 * 1024];
    let mut truncated = false;
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        let remaining = limit.saturating_sub(output.len());
        let keep = read.min(remaining);
        output.extend_from_slice(&buffer[..keep]);
        truncated |= keep < read;
    }
    Ok((output, truncated))
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    struct FakeProcessRunner {
        requests: Mutex<Vec<ProcessRequest>>,
        responses: Mutex<VecDeque<Result<ProcessOutput, ProcessRunError>>>,
    }

    impl FakeProcessRunner {
        fn with_responses(
            responses: impl IntoIterator<Item = Result<ProcessOutput, ProcessRunError>>,
        ) -> Arc<Self> {
            Arc::new(Self {
                requests: Mutex::new(Vec::new()),
                responses: Mutex::new(responses.into_iter().collect()),
            })
        }

        fn requests(&self) -> Vec<ProcessRequest> {
            self.requests.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl ProcessRunner for FakeProcessRunner {
        async fn run(&self, request: ProcessRequest) -> Result<ProcessOutput, ProcessRunError> {
            self.requests.lock().unwrap().push(request);
            // Let competing callers reach the in-flight cache before completion.
            tokio::task::yield_now().await;
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("missing fake process response")
        }
    }

    struct ExecutableGhRunner {
        executable: PathBuf,
    }

    #[async_trait]
    impl ProcessRunner for ExecutableGhRunner {
        async fn run(&self, mut request: ProcessRequest) -> Result<ProcessOutput, ProcessRunError> {
            if request.program == "gh" {
                #[cfg(unix)]
                {
                    request.program = self.executable.to_string_lossy().into_owned();
                }
                #[cfg(windows)]
                {
                    request.args.splice(
                        ..0,
                        [
                            "-NoLogo".into(),
                            "-NoProfile".into(),
                            "-NonInteractive".into(),
                            "-ExecutionPolicy".into(),
                            "Bypass".into(),
                            "-File".into(),
                            self.executable.to_string_lossy().into_owned(),
                        ],
                    );
                    request.program = "powershell.exe".into();
                }
            }
            SystemProcessRunner.run(request).await
        }
    }

    fn run_git(cwd: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .expect("git fixture command starts");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn command_success(stdout: impl Into<Vec<u8>>) -> Result<ProcessOutput, ProcessRunError> {
        Ok(ProcessOutput {
            success: true,
            stdout: stdout.into(),
            stderr: Vec::new(),
            stdout_truncated: false,
        })
    }

    #[test]
    fn pr_detail_rejects_unsafe_or_non_pr_targets() {
        for url in [
            "--help",
            "https://example.com/a/b/pull/1",
            "http://github.com/a/b/pull/1",
            "https://github.com/a/b/issues/1",
            "https://github.com/a/b/pull/0",
            "https://user@github.com/a/b/pull/1",
            "https://github.com/a/b/pull/1/files",
        ] {
            assert!(validated_pull_request_url(url).is_err(), "{url}");
        }
        assert_eq!(
            validated_pull_request_url("https://github.com/a/b/pull/12#discussion").unwrap(),
            "https://github.com/a/b/pull/12"
        );
    }

    #[tokio::test]
    async fn pr_comment_posts_literal_body_once_and_invalidates_detail() {
        let body = "Hello @octocat\n\n`$(do-not-run)` **Markdown**";
        let runner = FakeProcessRunner::with_responses([command_success(
            serde_json::to_vec(&serde_json::json!({
                "body": body, "user": {"login": "octocat"}, "created_at": "2026-09-20T20:00:00Z"
            }))
            .unwrap(),
        )]);
        let github = GitHubCli::with_runner(runner.clone());
        github.pr_cache.lock().await.entries.push((
            "false:https://github.com/a/b/pull/1".into(),
            Instant::now(),
            Ok(serde_json::json!({})),
        ));
        let comment = github
            .post_comment("https://github.com/a/b/pull/1", body)
            .await
            .unwrap();
        assert_eq!(comment.body, body);
        assert_eq!(comment.author.login, "octocat");
        assert!(comment.viewer_did_author);
        assert!(github.pr_cache.lock().await.entries.is_empty());
        let requests = runner.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].args,
            [
                "api",
                "--method",
                "POST",
                "repos/a/b/issues/1/comments",
                "-f",
                &format!("body={body}")
            ]
        );
    }

    #[tokio::test]
    async fn pr_comment_rejects_invalid_inputs_and_never_retries_failure() {
        let runner = FakeProcessRunner::with_responses([command_failure("request failed")]);
        let github = GitHubCli::with_runner(runner.clone());
        assert!(
            github
                .post_comment("https://evil.test/a/b/pull/1", "hello")
                .await
                .is_err()
        );
        assert!(
            github
                .post_comment("https://github.com/a/b/pull/1", "  ")
                .await
                .is_err()
        );
        assert!(
            github
                .post_comment("https://github.com/a/b/pull/1", &"x".repeat(60_001))
                .await
                .is_err()
        );
        assert!(runner.requests().is_empty());
        assert!(
            github
                .post_comment("https://github.com/a/b/pull/1", "hello")
                .await
                .is_err()
        );
        assert_eq!(runner.requests().len(), 1);
    }

    #[tokio::test]
    async fn pr_detail_normalizes_absent_github_fields_and_uses_bounded_process() {
        let runner = FakeProcessRunner::with_responses([command_success(br#"{"number":12,"title":"A PR","body":"Description","author":null,"reviewDecision":null,"comments":[{"viewerDidAuthor":true,"author":{"login":"viewer"},"body":"Own comment"},{"author":{"login":"other"},"body":"Other comment"}],"statusCheckRollup":[{"name":"build","status":"IN_PROGRESS","conclusion":null}]}"#.to_vec())]);
        let result = GitHubCli::with_runner(runner.clone())
            .detail("https://github.com/a/b/pull/12", false, false)
            .await
            .unwrap();
        let detail: zeron_proto::ChangeRequestDetail = serde_json::from_value(result).unwrap();
        assert_eq!(detail.status_check_rollup[0].status, "IN_PROGRESS");
        assert_eq!(detail.status_check_rollup[0].conclusion, "");
        assert!(detail.author.login.is_empty());
        assert!(detail.comments[0].viewer_did_author);
        assert!(!detail.comments[1].viewer_did_author);
        let request = &runner.requests()[0];
        assert_eq!(
            &request.args[..3],
            ["pr", "view", "https://github.com/a/b/pull/12"]
        );
        assert_eq!(request.timeout, GITHUB_TIMEOUT);
        assert_eq!(request.output_limit, GITHUB_OUTPUT_LIMIT);
        assert!(request.cwd.is_none());
    }

    #[tokio::test]
    async fn pr_diff_returns_patch_and_rejects_truncated_output() {
        let mut truncated = command_success("partial").unwrap();
        truncated.stdout_truncated = true;
        let runner = FakeProcessRunner::with_responses([
            command_success("diff --git a/a b/a\n"),
            Ok(truncated),
        ]);
        let github = GitHubCli::with_runner(runner.clone());
        assert_eq!(
            github
                .detail("https://github.com/a/b/pull/12", true, false)
                .await
                .unwrap(),
            "diff --git a/a b/a\n"
        );
        assert!(
            github
                .detail("https://github.com/a/b/pull/13", true, false)
                .await
                .is_err()
        );
        assert_eq!(runner.requests()[0].args.last().unwrap(), "--color=never");
    }

    fn command_failure(stderr: &str) -> Result<ProcessOutput, ProcessRunError> {
        Ok(ProcessOutput {
            success: false,
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
            stdout_truncated: false,
        })
    }

    fn source(branch: &str, owner: &str, default_branch: Option<&str>) -> CheckoutSourceContext {
        CheckoutSourceContext {
            checkout_root: PathBuf::from("/checkout"),
            branch: BranchHeadContext::resolve(
                branch,
                Some(&format!("origin/{branch}")),
                Some("origin"),
                Some(&format!("https://github.com/{owner}/zeron.git")),
            ),
            default_branch: default_branch.map(str::to_owned),
        }
    }

    fn pull_request(
        number: u64,
        state: &str,
        owner: &str,
        branch: &str,
        updated_at: &str,
    ) -> serde_json::Value {
        serde_json::json!({
            "number": number,
            "title": format!("Pull request {number}"),
            "url": format!("https://github.com/acme/zeron/pull/{number}"),
            "state": state,
            "baseRefName": "main",
            "headRefName": branch,
            "updatedAt": updated_at,
            "isCrossRepository": owner != "acme",
            "headRepositoryOwner": { "login": owner }
        })
    }

    fn search_pull_request(
        repository: &str,
        number: u64,
        title: &str,
        state: &str,
        mergeable: &str,
        created_at: &str,
        updated_at: &str,
        is_draft: bool,
        review_decision: Option<&str>,
    ) -> serde_json::Value {
        serde_json::json!({
            "number": number,
            "author": { "login": "octocat" },
            "title": title,
            "url": format!("https://github.com/{repository}/pull/{number}"),
            "state": state,
            "mergeable": mergeable,
            "repository": {
                "nameWithOwner": repository,
            },
            "createdAt": created_at,
            "updatedAt": updated_at,
            "isDraft": is_draft,
            "reviewDecision": review_decision,
            "additions": 42,
            "deletions": 7,
        })
    }

    fn search_response(items: Vec<serde_json::Value>) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "data": { "search": { "nodes": items } }
        }))
        .unwrap()
    }

    async fn list_with(
        response: Result<ProcessOutput, ProcessRunError>,
    ) -> (
        Result<Vec<ChangeRequestListItem>, ChangeRequestError>,
        Arc<FakeProcessRunner>,
    ) {
        let runner = FakeProcessRunner::with_responses([response]);
        let github = GitHubCli::with_runner(runner.clone());
        let result = github.list_authored_open("acme/zeron", false).await;
        (result, runner)
    }

    async fn resolve_with(
        source: &CheckoutSourceContext,
        response: Result<ProcessOutput, ProcessRunError>,
    ) -> (
        Result<Option<ChangeRequestSummary>, ChangeRequestError>,
        Arc<FakeProcessRunner>,
    ) {
        let runner = FakeProcessRunner::with_responses([response]);
        let github = GitHubCli::with_runner(runner.clone());
        let result = github.find_for_branch(source).await;
        (result, runner)
    }

    #[tokio::test]
    async fn github_cli_uses_exact_arguments_and_checkout_cwd() {
        let source = source("feature/status", "acme", Some("main"));
        let json = serde_json::to_vec(&vec![pull_request(
            90,
            "OPEN",
            "acme",
            "feature/status",
            "2026-08-15T12:00:00Z",
        )])
        .unwrap();
        let (result, runner) = resolve_with(&source, command_success(json)).await;

        let summary = result.unwrap().unwrap();
        assert_eq!(summary.number, 90);
        assert_eq!(summary.state, ChangeRequestState::Open);
        assert_eq!(summary.provider, "github");
        let requests = runner.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].program, "gh");
        assert_eq!(requests[0].cwd.as_deref(), Some(Path::new("/checkout")));
        assert_eq!(
            requests[0].args,
            [
                "pr",
                "list",
                "--head",
                "acme:feature/status",
                "--state",
                "all",
                "--limit",
                "20",
                "--json",
                GITHUB_JSON_FIELDS,
            ]
        );
        assert_eq!(requests[0].env, [("GH_PROMPT_DISABLED".into(), "1".into())]);
        assert_eq!(requests[0].timeout, GITHUB_TIMEOUT);
        assert_eq!(requests[0].output_limit, GITHUB_OUTPUT_LIMIT);
    }

    #[tokio::test]
    async fn pr_default_repository_uses_only_local_origin_metadata() {
        let runner =
            FakeProcessRunner::with_responses([command_success("git@github.com:acme/zeron.git\n")]);
        let resolver = ChangeRequestResolver {
            inspector: GitCheckoutInspector::new(runner.clone()),
            github: GitHubCli::with_runner(runner.clone()),
        };
        assert_eq!(
            resolver
                .repository_for_checkout(Path::new("/checkout"))
                .await
                .as_deref(),
            Some("acme/zeron")
        );
        let requests = runner.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].program, "git");
        assert_eq!(requests[0].args, ["remote", "get-url", "origin"]);
        assert_eq!(requests[0].cwd.as_deref(), Some(Path::new("/checkout")));
    }

    #[tokio::test]
    async fn pr_scoped_search_accepts_canonical_names_after_repository_rename() {
        let json = search_response(vec![search_pull_request(
            "acme/new-name",
            1,
            "PR",
            "OPEN",
            "UNKNOWN",
            "2026-08-10T09:30:00Z",
            "2026-08-19T12:00:00Z",
            false,
            None,
        )]);
        let (result, runner) = list_with(command_success(json)).await;
        let items = result.unwrap();
        assert_eq!(items[0].repository, "acme/new-name");
        assert_eq!(items[0].author.login, "octocat");
        assert!(
            runner.requests()[0]
                .args
                .iter()
                .any(|arg| arg.contains("repo:acme/zeron"))
        );
    }

    #[tokio::test]
    async fn pr_renamed_empty_search_follows_once_and_caches_result() {
        let redirect = serde_json::json!({"data": {
            "repository": {"nameWithOwner": "acme/new-name"},
            "search": {"nodes": []}
        }});
        let items = search_response(vec![search_pull_request(
            "acme/new-name",
            1,
            "PR",
            "OPEN",
            "UNKNOWN",
            "2026-08-10T09:30:00Z",
            "2026-08-19T12:00:00Z",
            false,
            None,
        )]);
        let runner = FakeProcessRunner::with_responses([
            command_success(serde_json::to_vec(&redirect).unwrap()),
            command_success(items),
        ]);
        let github = GitHubCli::with_runner(runner.clone());
        for _ in 0..2 {
            let items = github
                .list_authored_open("acme/old-name", false)
                .await
                .unwrap();
            assert_eq!(items[0].repository, "acme/new-name");
        }
        let requests = runner.requests();
        assert_eq!(requests.len(), 2);
        assert!(requests[1].args.iter().any(|arg| arg == "name=new-name"));
        assert_eq!(
            requests[1].args.last().unwrap(),
            "search=is:pr is:open author:@me repo:acme/new-name sort:updated-desc"
        );
    }

    #[tokio::test]
    async fn pr_empty_canonical_search_does_not_retry() {
        let response = serde_json::json!({"data": {
            "repository": {"nameWithOwner": "acme/zeron"},
            "search": {"nodes": []}
        }});
        let (items, runner) =
            list_with(command_success(serde_json::to_vec(&response).unwrap())).await;
        assert!(items.unwrap().is_empty());
        assert_eq!(runner.requests().len(), 1);
    }

    #[tokio::test]
    async fn pr_filters_are_scoped_cached_and_never_prefetched() {
        use zeron_proto::ChangeRequestFilter::{All, Authored, Reviewing};
        let runner = FakeProcessRunner::with_responses(
            (0..3).map(|_| command_success(search_response(vec![]))),
        );
        let github = GitHubCli::with_runner(runner.clone());
        for (index, filter) in [All, Authored, Reviewing].into_iter().enumerate() {
            github
                .list_filtered_open("acme/zeron", filter, false)
                .await
                .unwrap();
            assert_eq!(runner.requests().len(), index + 1);
            github
                .list_filtered_open("ACME/ZERON", filter, false)
                .await
                .unwrap();
            assert_eq!(runner.requests().len(), index + 1);
        }
        let requests = runner.requests();
        for (request, qualifier) in
            requests
                .iter()
                .zip(["", "author:@me ", "review-requested:@me "])
        {
            assert_eq!(
                request.args.last().unwrap(),
                &format!("search=is:pr is:open {qualifier}repo:acme/zeron sort:updated-desc")
            );
        }
        github
            .list_filtered_open("acme/zeron", All, false)
            .await
            .unwrap();
        assert_eq!(runner.requests().len(), 3);
    }

    #[tokio::test]
    async fn pr_cache_coalesces_reads_throttles_refresh_and_expires() {
        let runner = FakeProcessRunner::with_responses([
            command_success(search_response(vec![])),
            command_success(search_response(vec![])),
            command_success(search_response(vec![])),
        ]);
        let github = GitHubCli::with_runner(runner.clone());
        let (a, b) = tokio::join!(
            github.list_authored_open("acme/zeron", false),
            github.list_authored_open("ACME/ZERON", false)
        );
        assert!(a.unwrap().is_empty() && b.unwrap().is_empty());
        github.list_authored_open("acme/zeron", true).await.unwrap();
        assert_eq!(
            runner.requests().len(),
            1,
            "concurrent and immediate refresh calls reuse one response"
        );
        github.pr_cache.lock().await.entries[0].1 = Instant::now() - Duration::from_secs(16);
        github.list_authored_open("acme/zeron", true).await.unwrap();
        assert_eq!(runner.requests().len(), 2);
        github.pr_cache.lock().await.entries[0].1 = Instant::now() - Duration::from_secs(301);
        github
            .list_authored_open("acme/zeron", false)
            .await
            .unwrap();
        assert_eq!(runner.requests().len(), 3);
    }

    #[tokio::test]
    async fn pr_rate_limit_cooldown_covers_other_repositories_and_details() {
        let runner =
            FakeProcessRunner::with_responses([command_failure("API rate limit exceeded")]);
        let github = GitHubCli::with_runner(runner.clone());
        assert_eq!(
            github.list_authored_open("a/one", false).await,
            Err(ChangeRequestError::RateLimited)
        );
        assert_eq!(
            github.list_authored_open("a/two", true).await,
            Err(ChangeRequestError::RateLimited)
        );
        assert_eq!(
            github
                .detail("https://github.com/a/one/pull/1", true, true)
                .await,
            Err(ChangeRequestError::RateLimited)
        );
        assert_eq!(
            runner.requests().len(),
            1,
            "refresh and a different key must not bypass backoff"
        );
    }

    #[tokio::test]
    async fn pr_cache_is_bounded_and_repository_filter_cannot_be_broadened() {
        let runner = FakeProcessRunner::with_responses(
            (0..26).map(|_| command_success(search_response(vec![]))),
        );
        let github = GitHubCli::with_runner(runner.clone());
        for invalid in ["", "acme", "a/b repo:c/d", "a/*", "a/b/c", "a/.."] {
            assert_eq!(
                github.list_authored_open(invalid, false).await,
                Err(ChangeRequestError::UnsupportedRepository)
            );
        }
        assert!(runner.requests().is_empty());
        for index in 0..26 {
            github
                .list_authored_open(&format!("a/repo-{index}"), false)
                .await
                .unwrap();
        }
        let cache = github.pr_cache.lock().await;
        assert_eq!(cache.entries.len(), 24);
        assert!(
            cache
                .entries
                .iter()
                .all(|entry| entry.0 != "list:a/repo-0:Authored")
        );
    }

    #[tokio::test]
    async fn github_search_uses_exact_repository_arguments_and_environment() {
        let json = search_response(vec![search_pull_request(
            "acme/zeron",
            123,
            "Dashboard",
            "OPEN",
            "CONFLICTING",
            "2026-08-10T09:30:00Z",
            "2026-08-19T12:00:00Z",
            true,
            Some("CHANGES_REQUESTED"),
        )]);
        let (result, runner) = list_with(command_success(json)).await;

        let item = &result.unwrap()[0];
        assert_eq!(item.repository, "acme/zeron");
        assert!(item.is_draft);
        assert_eq!(
            item.review_decision,
            ChangeRequestReviewDecision::ChangesRequested
        );
        assert_eq!(item.additions, 42);
        assert_eq!(item.deletions, 7);
        assert_eq!(item.mergeability, ChangeRequestMergeability::Conflicting);
        assert_eq!(
            item.created_at,
            DateTime::parse_from_rfc3339("2026-08-10T09:30:00Z")
                .unwrap()
                .with_timezone(&Utc)
        );
        let requests = runner.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].program, "gh");
        assert_eq!(requests[0].cwd, None);
        assert_eq!(
            requests[0].args,
            [
                "api",
                "graphql",
                "-f",
                &format!("query={GITHUB_SEARCH_QUERY}"),
                "-f",
                "owner=acme",
                "-f",
                "name=zeron",
                "-f",
                "search=is:pr is:open author:@me repo:acme/zeron sort:updated-desc"
            ]
        );
        assert_eq!(
            requests[0].env,
            [
                ("GH_PROMPT_DISABLED".into(), "1".into()),
                ("GH_HOST".into(), "github.com".into()),
            ]
        );
        assert_eq!(requests[0].timeout, GITHUB_TIMEOUT);
        assert_eq!(requests[0].output_limit, GITHUB_OUTPUT_LIMIT);
    }

    #[tokio::test]
    async fn github_search_normalizes_titles_and_sorts_results_stably() {
        let json = search_response(vec![
            search_pull_request(
                "acme/zeron",
                8,
                "  A title\nwith\tspacing  ",
                "OPEN",
                "MERGEABLE",
                "2026-08-01T08:00:00Z",
                "2026-08-19T11:00:00Z",
                false,
                Some("APPROVED"),
            ),
            search_pull_request(
                "acme/zeron",
                4,
                "Alpha",
                "OPEN",
                "UNKNOWN",
                "2026-08-02T08:00:00Z",
                "2026-08-19T12:00:00Z",
                true,
                Some("REVIEW_REQUIRED"),
            ),
            search_pull_request(
                "acme/zeron",
                2,
                "Earlier number",
                "OPEN",
                "MERGEABLE",
                "2026-08-03T08:00:00Z",
                "2026-08-19T12:00:00Z",
                false,
                None,
            ),
        ]);

        let (result, _) = list_with(command_success(json)).await;
        let items = result.unwrap();
        assert_eq!(
            items
                .iter()
                .map(|item| (item.repository.as_str(), item.number))
                .collect::<Vec<_>>(),
            [("acme/zeron", 2), ("acme/zeron", 4), ("acme/zeron", 8)]
        );
        assert_eq!(items[2].title, "A title with spacing");
        assert_eq!(items[0].state, ChangeRequestState::Open);
    }

    #[tokio::test]
    async fn github_search_rejects_invalid_structural_fields() {
        let base = search_pull_request(
            "acme/zeron",
            1,
            "Valid",
            "OPEN",
            "MERGEABLE",
            "2026-08-01T08:00:00Z",
            "2026-08-19T12:00:00Z",
            false,
            None,
        );
        let mut invalid_items = Vec::new();
        for (pointer, value) in [
            ("/number", serde_json::json!(0)),
            ("/title", serde_json::json!(" \n\t ")),
            ("/repository/nameWithOwner", serde_json::json!("zeron")),
            ("/repository/nameWithOwner", serde_json::json!("a/b/c")),
            ("/url", serde_json::json!("file:///tmp/pr")),
            ("/state", serde_json::json!("CLOSED")),
            ("/mergeable", serde_json::json!("BLOCKED")),
        ] {
            let mut item = base.clone();
            *item.pointer_mut(pointer).unwrap() = value;
            invalid_items.push(item);
        }

        for item in invalid_items {
            let json = search_response(vec![item]);
            let (result, _) = list_with(command_success(json)).await;
            assert_eq!(result.unwrap_err(), ChangeRequestError::Decode);
        }
    }

    #[tokio::test]
    async fn github_search_classifies_process_and_output_failures() {
        for (response, expected) in [
            (
                Err(ProcessRunError::Spawn(io::ErrorKind::NotFound)),
                ChangeRequestError::CliUnavailable,
            ),
            (Err(ProcessRunError::Timeout), ChangeRequestError::Timeout),
            (
                command_failure("not logged in; token secret-value"),
                ChangeRequestError::Authentication,
            ),
            (
                command_failure("API rate limit exceeded"),
                ChangeRequestError::RateLimited,
            ),
        ] {
            let (result, _) = list_with(response).await;
            let error = result.unwrap_err();
            assert_eq!(error, expected);
            assert!(!error.to_string().contains("secret-value"));
        }

        for output in [
            command_success(b"not-json".to_vec()),
            Ok(ProcessOutput {
                success: true,
                stdout: search_response(Vec::new()),
                stderr: Vec::new(),
                stdout_truncated: true,
            }),
        ] {
            let (result, _) = list_with(output).await;
            assert_eq!(result.unwrap_err(), ChangeRequestError::Decode);
        }
    }

    #[tokio::test]
    async fn real_git_checkout_resolves_through_host_fake_gh() {
        let temp = tempfile::tempdir().expect("fixture tempdir");
        let checkout = temp.path().join("checkout");
        std::fs::create_dir_all(&checkout).expect("checkout directory");
        run_git(&checkout, &["init", "-q", "-b", "main"]);
        run_git(&checkout, &["checkout", "-q", "-b", "feature/status"]);
        run_git(
            &checkout,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/acme/zeron.git",
            ],
        );

        #[cfg(unix)]
        let fake_gh = temp.path().join("gh");
        #[cfg(windows)]
        let fake_gh = temp.path().join("gh.ps1");
        #[cfg(unix)]
        let fake_gh_contents = r##"#!/bin/sh
if [ "$GH_PROMPT_DISABLED" != "1" ]; then
  echo "interactive auth was not disabled" >&2
  exit 2
fi
printf '%s\n' '[{"number":90,"title":"Host-resolved pull request","url":"https://github.com/acme/zeron/pull/90","state":"OPEN","baseRefName":"main","headRefName":"feature/status","updatedAt":"2026-08-15T12:00:00Z","isCrossRepository":false,"headRepositoryOwner":{"login":"acme"}}]'
"##;
        #[cfg(windows)]
        let fake_gh_contents = r##"if ($env:GH_PROMPT_DISABLED -ne '1') {
  [Console]::Error.WriteLine('interactive auth was not disabled')
  exit 2
}
[Console]::Out.WriteLine('[{"number":90,"title":"Host-resolved pull request","url":"https://github.com/acme/zeron/pull/90","state":"OPEN","baseRefName":"main","headRefName":"feature/status","updatedAt":"2026-08-15T12:00:00Z","isCrossRepository":false,"headRepositoryOwner":{"login":"acme"}}]')
"##;
        std::fs::write(&fake_gh, fake_gh_contents).expect("write fake gh");
        #[cfg(unix)]
        {
            let mut permissions = std::fs::metadata(&fake_gh)
                .expect("fake gh metadata")
                .permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&fake_gh, permissions).expect("make fake gh executable");
        }

        let runner: Arc<dyn ProcessRunner> = Arc::new(ExecutableGhRunner {
            executable: fake_gh,
        });
        let resolver = ChangeRequestResolver {
            inspector: GitCheckoutInspector::new(runner.clone()),
            github: GitHubCli::with_runner(runner),
        };

        let resolution = resolver
            .resolve_github(&checkout)
            .await
            .expect("host resolves fake GitHub pull request");

        assert_eq!(
            std::fs::canonicalize(&resolution.source.checkout_root).expect("resolved root"),
            std::fs::canonicalize(&checkout).expect("fixture root")
        );
        assert_eq!(resolution.source.branch.local_branch, "feature/status");
        assert_eq!(resolution.source.branch.owner.as_deref(), Some("acme"));
        let pull_request = resolution.change_request.expect("pull request");
        assert_eq!(pull_request.number, 90);
        assert_eq!(pull_request.state, ChangeRequestState::Open);
        assert_eq!(pull_request.head_ref, "feature/status");
    }

    #[tokio::test]
    async fn git_inspector_uses_origin_without_upstream_in_a_multi_remote_checkout() {
        let temp = tempfile::tempdir().expect("fixture tempdir");
        let checkout = temp.path().join("checkout");
        std::fs::create_dir_all(&checkout).expect("checkout directory");
        run_git(&checkout, &["init", "-q", "-b", "main"]);
        run_git(&checkout, &["checkout", "-q", "-b", "feature/no-upstream"]);
        run_git(
            &checkout,
            &[
                "remote",
                "add",
                "origin",
                "git@github.com:contributor/zeron.git",
            ],
        );
        run_git(
            &checkout,
            &[
                "remote",
                "add",
                "upstream",
                "https://github.com/acme/zeron.git",
            ],
        );

        let source = ChangeRequestResolver::new()
            .inspect_checkout(&checkout)
            .await
            .expect("inspect checkout without branch upstream");

        assert_eq!(source.branch.upstream_ref, None);
        assert_eq!(source.branch.remote_name.as_deref(), Some("origin"));
        assert_eq!(
            source.branch.remote_url.as_deref(),
            Some("git@github.com:contributor/zeron.git")
        );
        assert_eq!(source.branch.owner.as_deref(), Some("contributor"));
        assert_eq!(
            source.branch.head_selectors,
            ["contributor:feature/no-upstream", "feature/no-upstream"]
        );
    }

    #[tokio::test]
    async fn git_inspector_never_runs_remote_transport_for_an_absent_default_branch() {
        let temp = tempfile::tempdir().expect("fixture tempdir");
        let checkout = temp.path().join("checkout");
        std::fs::create_dir_all(&checkout).expect("checkout directory");
        run_git(&checkout, &["init", "-q", "-b", "main"]);
        run_git(
            &checkout,
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.com",
                "commit",
                "--allow-empty",
                "-qm",
                "fixture",
            ],
        );
        run_git(
            &checkout,
            &["remote", "add", "origin", "git@github.com:acme/zeron.git"],
        );
        // A repository-controlled transport command: any Git subcommand that
        // touches the remote (such as the former `ls-remote` default-branch
        // fallback) would execute it and create the marker.
        let marker = temp.path().join("transport-ran");
        #[cfg(unix)]
        let transport_command = format!("sh -c 'touch {}; exit 1'", marker.display());
        #[cfg(windows)]
        let transport_command = format!(
            "powershell.exe -NoLogo -NoProfile -NonInteractive -Command \"New-Item -ItemType File -Force -LiteralPath '{}'; exit 1\"",
            marker.to_string_lossy().replace('\'', "''")
        );
        run_git(
            &checkout,
            &["config", "core.sshCommand", &transport_command],
        );

        let source = ChangeRequestResolver::new()
            .inspect_checkout(&checkout)
            .await
            .expect("inspect checkout without a cached remote HEAD");

        assert_eq!(source.branch.remote_name.as_deref(), Some("origin"));
        assert_eq!(
            source.default_branch, None,
            "an unfetched default branch stays unknown until the provider resolves it"
        );
        assert!(
            !marker.exists(),
            "checkout inspection must never invoke Git remote transport"
        );
    }

    #[tokio::test]
    async fn merged_pull_request_is_returned_when_no_open_exists() {
        let source = source("feature/status", "acme", Some("main"));
        let json = serde_json::to_vec(&vec![pull_request(
            91,
            "MERGED",
            "acme",
            "feature/status",
            "2026-08-15T12:00:00Z",
        )])
        .unwrap();

        let (result, _) = resolve_with(&source, command_success(json)).await;
        let summary = result.unwrap().unwrap();
        assert_eq!(summary.number, 91);
        assert_eq!(summary.state, ChangeRequestState::Merged);
    }

    #[tokio::test]
    async fn closed_pull_request_is_returned_when_no_open_exists() {
        let source = source("feature/status", "acme", Some("main"));
        let json = serde_json::to_vec(&vec![pull_request(
            92,
            "CLOSED",
            "acme",
            "feature/status",
            "2026-08-15T12:00:00Z",
        )])
        .unwrap();

        let (result, _) = resolve_with(&source, command_success(json)).await;
        assert_eq!(result.unwrap().unwrap().state, ChangeRequestState::Closed);
    }

    #[tokio::test]
    async fn open_pull_request_wins_over_newer_terminal_candidates() {
        let source = source("feature/status", "acme", Some("main"));
        let json = serde_json::to_vec(&vec![
            pull_request(
                93,
                "MERGED",
                "acme",
                "feature/status",
                "2026-08-15T14:00:00Z",
            ),
            pull_request(94, "OPEN", "acme", "feature/status", "2026-08-15T10:00:00Z"),
            pull_request(
                95,
                "CLOSED",
                "acme",
                "feature/status",
                "2026-08-15T15:00:00Z",
            ),
        ])
        .unwrap();

        let (result, _) = resolve_with(&source, command_success(json)).await;
        assert_eq!(result.unwrap().unwrap().number, 94);
    }

    #[tokio::test]
    async fn newest_terminal_pull_request_wins_without_an_open_candidate() {
        let source = source("feature/status", "acme", Some("main"));
        let json = serde_json::to_vec(&vec![
            pull_request(
                96,
                "MERGED",
                "acme",
                "feature/status",
                "2026-08-15T12:00:00Z",
            ),
            pull_request(
                97,
                "CLOSED",
                "acme",
                "feature/status",
                "2026-08-15T13:00:00Z",
            ),
        ])
        .unwrap();

        let (result, _) = resolve_with(&source, command_success(json)).await;
        assert_eq!(result.unwrap().unwrap().number, 97);
    }

    #[tokio::test]
    async fn empty_specific_lookup_uses_the_next_safe_selector() {
        let source = source("feature/status", "acme", Some("main"));
        let json = serde_json::to_vec(&vec![pull_request(
            100,
            "OPEN",
            "acme",
            "feature/status",
            "2026-08-15T12:00:00Z",
        )])
        .unwrap();
        let runner = FakeProcessRunner::with_responses([
            command_success(b"[]".to_vec()),
            command_success(json),
        ]);
        let github = GitHubCli::with_runner(runner.clone());

        let result = github.find_for_branch(&source).await.unwrap().unwrap();

        assert_eq!(result.number, 100);
        let requests = runner.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].args[3], "acme:feature/status");
        assert_eq!(requests[1].args[3], "feature/status");
    }

    #[tokio::test]
    async fn pull_request_from_another_fork_is_rejected() {
        let source = source("feature/status", "contributor", Some("main"));
        let json = serde_json::to_vec(&vec![pull_request(
            98,
            "OPEN",
            "someone-else",
            "feature/status",
            "2026-08-15T12:00:00Z",
        )])
        .unwrap();

        let (result, _) = resolve_with(&source, command_success(json)).await;
        assert_eq!(result.unwrap(), None);
    }

    #[tokio::test]
    async fn partial_or_invalid_json_is_classified_as_decode() {
        let source = source("feature/status", "acme", Some("main"));
        for json in [br#"[{"number":90}]"#.to_vec(), b"not-json".to_vec()] {
            let (result, _) = resolve_with(&source, command_success(json)).await;
            assert_eq!(result.unwrap_err(), ChangeRequestError::Decode);
        }
    }

    #[tokio::test]
    async fn truncated_json_is_classified_as_decode() {
        let source = source("feature/status", "acme", Some("main"));
        let runner = FakeProcessRunner::with_responses([Ok(ProcessOutput {
            success: true,
            stdout: b"[]".to_vec(),
            stderr: Vec::new(),
            stdout_truncated: true,
        })]);
        let github = GitHubCli::with_runner(runner);

        let result = github.find_for_branch(&source).await;

        assert_eq!(result.unwrap_err(), ChangeRequestError::Decode);
    }

    #[tokio::test]
    async fn missing_github_executable_is_classified() {
        let source = source("feature/status", "acme", Some("main"));
        let (result, _) = resolve_with(
            &source,
            Err(ProcessRunError::Spawn(io::ErrorKind::NotFound)),
        )
        .await;
        assert_eq!(result.unwrap_err(), ChangeRequestError::CliUnavailable);
    }

    #[tokio::test]
    async fn authentication_failure_is_classified_without_exposing_stderr() {
        let source = source("feature/status", "acme", Some("main"));
        let secret_stderr = "not logged into github.example.com; token secret-value";
        let (result, _) = resolve_with(&source, command_failure(secret_stderr)).await;
        let error = result.unwrap_err();
        assert_eq!(error, ChangeRequestError::Authentication);
        assert!(!error.to_string().contains("secret-value"));
    }

    #[tokio::test]
    async fn rate_limit_failure_is_classified() {
        let source = source("feature/status", "acme", Some("main"));
        let (result, _) =
            resolve_with(&source, command_failure("GraphQL: API rate limit exceeded")).await;
        assert_eq!(result.unwrap_err(), ChangeRequestError::RateLimited);
    }

    #[tokio::test]
    async fn generic_github_failure_is_classified_without_exposing_stderr() {
        let source = source("feature/status", "acme", Some("main"));
        let (result, _) = resolve_with(
            &source,
            command_failure("provider failed with sensitive details"),
        )
        .await;
        let error = result.unwrap_err();
        assert_eq!(error, ChangeRequestError::CommandFailed);
        assert!(!error.to_string().contains("sensitive details"));
    }

    #[tokio::test]
    async fn timeout_is_classified() {
        let source = source("feature/status", "acme", Some("main"));
        let (result, _) = resolve_with(&source, Err(ProcessRunError::Timeout)).await;
        assert_eq!(result.unwrap_err(), ChangeRequestError::Timeout);
    }

    #[tokio::test]
    async fn terminal_pull_request_is_suppressed_on_default_branch() {
        let source = source("main", "acme", Some("main"));
        let json = serde_json::to_vec(&vec![pull_request(
            99,
            "MERGED",
            "acme",
            "main",
            "2026-08-15T12:00:00Z",
        )])
        .unwrap();

        let (result, _) = resolve_with(&source, command_success(json)).await;
        assert_eq!(result.unwrap(), None);
    }

    #[tokio::test]
    async fn unknown_default_branch_resolves_through_the_provider_before_suppression() {
        let source = source("main", "acme", None);
        let list = serde_json::to_vec(&vec![pull_request(
            99,
            "MERGED",
            "acme",
            "main",
            "2026-08-15T12:00:00Z",
        )])
        .unwrap();
        let runner = FakeProcessRunner::with_responses([
            command_success(list),
            command_success(br#"{"defaultBranchRef":{"name":"main"}}"#.to_vec()),
        ]);
        let github = GitHubCli::with_runner(runner.clone());

        let result = github.find_for_branch(&source).await;

        assert_eq!(result.unwrap(), None, "historical PR on main is suppressed");
        let requests = runner.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[1].program, "gh");
        assert_eq!(
            requests[1].args,
            [
                "repo",
                "view",
                "github.com/acme/zeron",
                "--json",
                "defaultBranchRef"
            ]
        );
        assert_eq!(requests[1].env, [("GH_PROMPT_DISABLED".into(), "1".into())]);
    }

    #[tokio::test]
    async fn open_candidates_skip_the_provider_default_branch_lookup() {
        let source = source("feature/status", "acme", None);
        let json = serde_json::to_vec(&vec![pull_request(
            90,
            "OPEN",
            "acme",
            "feature/status",
            "2026-08-15T12:00:00Z",
        )])
        .unwrap();

        let (result, runner) = resolve_with(&source, command_success(json)).await;

        assert_eq!(result.unwrap().unwrap().number, 90);
        assert_eq!(runner.requests().len(), 1);
    }

    #[tokio::test]
    async fn provider_default_branch_failure_keeps_the_terminal_pull_request() {
        let source = source("main", "acme", None);
        let list = serde_json::to_vec(&vec![pull_request(
            99,
            "MERGED",
            "acme",
            "main",
            "2026-08-15T12:00:00Z",
        )])
        .unwrap();
        let runner = FakeProcessRunner::with_responses([
            command_success(list),
            command_failure("could not resolve repository"),
        ]);
        let github = GitHubCli::with_runner(runner);

        let result = github.find_for_branch(&source).await;

        assert_eq!(result.unwrap().unwrap().number, 99);
    }

    #[tokio::test]
    async fn git_inspector_reads_branch_upstream_remote_and_default_branch() {
        let runner = FakeProcessRunner::with_responses([
            command_success("/checkout\n"),
            command_success("feature/status\n"),
            command_success("fork/published-status\n"),
            command_success("fork\n"),
            command_success("git@github.com:contributor/zeron.git\n"),
            command_success("fork/main\n"),
        ]);
        let inspector = GitCheckoutInspector::new(runner.clone());

        let source = inspector.inspect(Path::new("/nested/path")).await.unwrap();

        assert_eq!(source.checkout_root, Path::new("/checkout"));
        assert_eq!(source.branch.local_branch, "feature/status");
        assert_eq!(source.branch.head_branch, "published-status");
        assert_eq!(source.branch.owner.as_deref(), Some("contributor"));
        assert_eq!(source.default_branch.as_deref(), Some("main"));
        let requests = runner.requests();
        assert_eq!(requests[0].cwd.as_deref(), Some(Path::new("/nested/path")));
        assert_eq!(requests[0].args, ["rev-parse", "--show-toplevel"]);
        assert_eq!(requests[4].args, ["remote", "get-url", "--push", "fork"]);
    }

    #[test]
    fn parses_supported_git_remote_forms() {
        for (url, expected) in [
            (
                "git@github.com:owner/repo.git",
                GitRemote {
                    host: "github.com".into(),
                    owner: "owner".into(),
                    repository: "repo".into(),
                },
            ),
            (
                "ssh://git@github.com/owner/repo.git",
                GitRemote {
                    host: "github.com".into(),
                    owner: "owner".into(),
                    repository: "repo".into(),
                },
            ),
            (
                "https://github.com/owner/repo",
                GitRemote {
                    host: "github.com".into(),
                    owner: "owner".into(),
                    repository: "repo".into(),
                },
            ),
            (
                "https://user@github.example.com/owner/repo.git/",
                GitRemote {
                    host: "github.example.com".into(),
                    owner: "owner".into(),
                    repository: "repo".into(),
                },
            ),
        ] {
            assert_eq!(parse_git_remote(url), Some(expected), "remote: {url}");
        }
    }

    #[test]
    fn rejects_unsupported_or_malformed_remotes() {
        for url in [
            "file:///tmp/repo",
            "/tmp/repo",
            "git://github.com/owner/repo.git",
            "https://github.com/owner",
            "https://github.com/owner/repo/extra",
            "",
        ] {
            assert_eq!(parse_git_remote(url), None, "remote: {url}");
        }
    }

    #[test]
    fn fork_remote_owner_selector_has_priority() {
        let context = BranchHeadContext::resolve(
            "local-name",
            Some("fork/published-name"),
            Some("fork"),
            Some("git@github.com:contributor/zeron.git"),
        );

        assert_eq!(context.host.as_deref(), Some("github.com"));
        assert_eq!(context.owner.as_deref(), Some("contributor"));
        assert_eq!(context.repository.as_deref(), Some("zeron"));
        assert_eq!(context.head_branch, "published-name");
        assert_eq!(
            context.head_selectors,
            ["contributor:published-name", "published-name"]
        );
    }

    #[test]
    fn branch_without_upstream_keeps_safe_local_fallback() {
        let context = BranchHeadContext::resolve(
            "feature/local",
            None,
            Some("origin"),
            Some("https://github.com/acme/zeron.git"),
        );

        assert_eq!(context.head_branch, "feature/local");
        assert_eq!(
            context.head_selectors,
            ["acme:feature/local", "feature/local"]
        );
    }

    #[test]
    fn matching_local_and_upstream_branches_are_deduplicated() {
        let context = BranchHeadContext::resolve(
            "feature/shared",
            Some("refs/remotes/origin/feature/shared"),
            Some("origin"),
            Some("https://github.com/acme/zeron"),
        );

        assert_eq!(context.remote_name.as_deref(), Some("origin"));
        assert_eq!(
            context.head_selectors,
            ["acme:feature/shared", "feature/shared"]
        );
    }
}
