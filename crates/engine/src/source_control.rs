//! Pure source-control remote and branch normalization.
//!
//! Process execution and provider calls intentionally live outside this layer so
//! remote parsing and head selector construction remain deterministic and testable.

#[path = "source_control_review_threads.rs"]
mod review_threads;

use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

use zeron_proto::{
    ChangeRequestListItem, ChangeRequestMergeability, ChangeRequestReviewDecision,
    ChangeRequestState, ChangeRequestSummary,
};

const GIT_TIMEOUT: Duration = Duration::from_secs(10);
const GITHUB_TIMEOUT: Duration = Duration::from_secs(20);
const GIT_OUTPUT_LIMIT: usize = 64 * 1024;
/// `ssh -G` only reads configuration; it never connects.
const SSH_CONFIG_TIMEOUT: Duration = Duration::from_secs(3);
/// `repository()` follows renames: the old name resolves to the current one.
const GITHUB_REPOSITORY_QUERY: &str =
    "query($owner: String!, $name: String!) { repository(owner: $owner, name: $name) { nameWithOwner } }";
const GITHUB_OUTPUT_LIMIT: usize = 1024 * 1024;
const GITHUB_RESULT_LIMIT: &str = "20";
const GITHUB_JSON_FIELDS: &str = "number,title,url,state,baseRefName,headRefName,updatedAt,isCrossRepository,headRepositoryOwner";
const GITHUB_SEARCH_QUERY: &str = "query($search: String!, $owner: String!, $name: String!, $after: String) { repository(owner: $owner, name: $name) { nameWithOwner } search(query: $search, type: ISSUE, first: 50, after: $after) { issueCount pageInfo { hasNextPage endCursor } nodes { ... on PullRequest { author { login } headRefOid viewerDidAuthor viewerLatestReviewRequest { id } reviewRequests(first: 100) { nodes { id } pageInfo { hasNextPage } } statusCheckRollup { commit { oid } state contexts { totalCount checkRunCountsByState { state count } statusContextCountsByState { state count } } } number title url state isDraft mergeable reviewDecision createdAt updatedAt additions deletions repository { nameWithOwner } } } } }";

const GITHUB_PR_METADATA_QUERY: &str = "query($owner: String!, $name: String!, $number: Int!) { viewer { login } repository(owner: $owner, name: $name) { pullRequest(number: $number) { headRefOid viewerDidAuthor viewerLatestReviewRequest { id } reviewRequests(first: 100) { nodes { id } pageInfo { hasNextPage } } statusCheckRollup { commit { oid } state contexts(first: 100) { nodes { ... on CheckRun { name status conclusion detailsUrl } ... on StatusContext { context state targetUrl } } totalCount checkRunCountsByState { state count } statusContextCountsByState { state count } } } } } }";

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

/// GitHub connection cursors are short opaque base64 tokens. Anything else is
/// rejected before it reaches the provider.
pub fn valid_page_cursor(cursor: &str) -> bool {
    (1..=256).contains(&cursor.len())
        && cursor
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"+/=_-:".contains(&b))
}

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

type PrResult = Result<serde_json::Value, ChangeRequestError>;
type PrFetch = futures::future::Shared<futures::future::BoxFuture<'static, PrResult>>;

#[derive(Default)]
struct PrRequestCache {
    entries: Vec<(String, Instant, PrResult)>,
    /// Reads awaiting GitHub, joined by identical requests. The id tells a
    /// finishing read whether a write has replaced or dropped it meanwhile.
    in_flight: std::collections::HashMap<String, (u64, PrFetch)>,
    next_fetch: u64,
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

    /// Resolve just the selected checkout's identity. Local only: no provider
    /// lookup, repository enumeration, or remote transport. The slug is the
    /// remote's as written, which may predate a repository rename; the board
    /// adopts GitHub's canonical name once its first page loads.
    pub async fn repository_for_checkout(&self, cwd: &Path) -> Option<String> {
        let remote_url = match self.inspect_checkout(cwd).await {
            Ok(source) => source.branch.remote_url?,
            // An unborn or detached checkout still has a useful origin.
            Err(_) => {
                self.inspector
                    .git_optional(cwd, &["remote", "get-url", "origin"])
                    .await?
            }
        };
        self.github_slug(&remote_url).await
    }

    /// Match a PR's repository against every fetch remote. A fork's origin
    /// stays the board's default identity, while upstream is a valid handoff.
    /// Remote URLs are read with local Git only; a remote is never contacted.
    /// When no remote names the repository as written, `github` resolves each
    /// remote's slug to GitHub's canonical name, so a checkout cloned before a
    /// rename (`acme/old` → `acme/new`) still matches.
    pub async fn matching_repository_for_checkout(
        &self,
        cwd: &Path,
        repository: &str,
        github: &GitHubCli,
    ) -> Option<String> {
        if !valid_pr_repository(repository) {
            return None;
        }
        let remotes = self.inspector.git_optional(cwd, &["remote"]).await?;
        let mut slugs = Vec::new();
        for name in remotes.lines().filter(|name| !name.is_empty()) {
            let Some(urls) = self
                .inspector
                .git_optional(cwd, &["remote", "get-url", "--all", "--", name])
                .await
            else {
                continue;
            };
            for url in urls.lines() {
                if let Some(slug) = self.github_slug(url).await {
                    if slug.eq_ignore_ascii_case(repository) {
                        return Some(slug);
                    }
                    if !slugs
                        .iter()
                        .any(|seen: &String| seen.eq_ignore_ascii_case(&slug))
                    {
                        slugs.push(slug);
                    }
                }
            }
        }
        for slug in slugs {
            if let Some(canonical) = github.canonical_repository(&slug).await
                && canonical.eq_ignore_ascii_case(repository)
            {
                return Some(canonical);
            }
        }
        None
    }

    /// `owner/name` for a remote on github.com. An SSH remote may name a
    /// host alias from `~/.ssh/config` (`git@github-work:acme/zeron.git`);
    /// `ssh -G` resolves it locally without connecting.
    async fn github_slug(&self, remote_url: &str) -> Option<String> {
        let remote = parse_git_remote(remote_url)?;
        let slug = format!("{}/{}", remote.owner, remote.repository);
        if remote.host.eq_ignore_ascii_case("github.com") {
            return Some(slug);
        }
        if !is_ssh_remote(remote_url) || !valid_ssh_alias(&remote.host) {
            return None;
        }
        let output = self
            .inspector
            .runner
            .run(ProcessRequest {
                program: "ssh".into(),
                args: vec!["-G".into(), remote.host.clone()],
                stdin: None,
                cwd: None,
                env: Vec::new(),
                timeout: SSH_CONFIG_TIMEOUT,
                output_limit: GIT_OUTPUT_LIMIT,
            })
            .await
            .ok()?;
        if !output.success || output.stdout_truncated {
            return None;
        }
        ssh_config_hostname(&String::from_utf8_lossy(&output.stdout))
            .is_some_and(|host| host.eq_ignore_ascii_case("github.com"))
            .then_some(slug)
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
    /// The board's on-disk first pages. `None` keeps everything in memory
    /// (tests, and engines without a data directory).
    store: Option<crate::change_request_store::ChangeRequestStore>,
    /// The active `gh` account and when it was read, keying the store.
    account: Arc<tokio::sync::Mutex<Option<(Instant, Option<String>)>>>,
}

impl GitHubCli {
    /// Shared across RPC clients on this engine. Simultaneous identical reads
    /// share one provider call; distinct reads run side by side. The read runs
    /// detached, so it completes and is cached even when every caller has gone
    /// away. Never retry here.
    async fn cached_pr_request<F>(&self, key: String, refresh: bool, fetch: F) -> PrResult
    where
        F: std::future::Future<Output = PrResult> + Send + 'static,
    {
        use futures::FutureExt;
        let mut cache = self.pr_cache.lock().await;
        if let Some(index) = cache.entries.iter().position(|entry| entry.0 == key) {
            let entry = cache.entries.remove(index);
            let ttl = match &entry.2 {
                Err(ChangeRequestError::RateLimited) => Duration::from_secs(15 * 60),
                // An explicit retry always reaches GitHub again.
                Err(_) if refresh => Duration::ZERO,
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
        let pending = match cache.in_flight.get(&key) {
            Some((_, pending)) => pending.clone(),
            None => {
                let id = cache.next_fetch;
                cache.next_fetch += 1;
                let shared = self.pr_cache.clone();
                let task_key = key.clone();
                let task = tokio::spawn(async move {
                    let result = std::panic::AssertUnwindSafe(fetch)
                        .catch_unwind()
                        .await
                        .unwrap_or(Err(ChangeRequestError::CommandFailed));
                    let mut cache = shared.lock().await;
                    if matches!(result, Err(ChangeRequestError::RateLimited)) {
                        cache.rate_limited_at = Some(Instant::now());
                    }
                    if cache
                        .in_flight
                        .get(&task_key)
                        .is_some_and(|(current, _)| *current == id)
                    {
                        cache.in_flight.remove(&task_key);
                        cache.entries.retain(|entry| entry.0 != task_key);
                        cache
                            .entries
                            .push((task_key, Instant::now(), result.clone()));
                        if cache.entries.len() > 24 {
                            let _ = cache.entries.remove(0);
                        }
                    }
                    result
                });
                let pending =
                    async move { task.await.unwrap_or(Err(ChangeRequestError::CommandFailed)) }
                        .boxed()
                        .shared();
                cache.in_flight.insert(key, (id, pending.clone()));
                pending
            }
        };
        drop(cache);
        pending.await
    }

    pub async fn detail(
        &self,
        url: &str,
        refresh: bool,
    ) -> Result<zeron_proto::ChangeRequestDetail, ChangeRequestError> {
        serde_json::from_value(self.detail_request(url, false, refresh, None, None).await?)
            .map_err(|_| ChangeRequestError::Decode)
    }

    pub async fn diff(&self, url: &str, refresh: bool) -> Result<String, ChangeRequestError> {
        serde_json::from_value(self.detail_request(url, true, refresh, None, None).await?)
            .map_err(|_| ChangeRequestError::Decode)
    }

    pub(crate) async fn detail_request(
        &self,
        url: &str,
        diff: bool,
        refresh: bool,
        head: Option<&str>,
        base: Option<&str>,
    ) -> Result<serde_json::Value, ChangeRequestError> {
        let url = validated_pull_request_url(url)?;
        for oid in [head, base].into_iter().flatten() {
            if !zeron_proto::change_request_assessment::valid_head_oid(oid) {
                return Err(ChangeRequestError::Decode);
            }
        }
        let github = self.clone();
        // A board that observed a new revision must never join an older
        // in-flight read or reuse its five-minute (or refresh) cache entry.
        let key = format!(
            "{diff}:{url}:{}:{}",
            head.unwrap_or_default(),
            base.unwrap_or_default()
        );
        let revisions = head
            .zip(base)
            .map(|(head, base)| (head.to_owned(), base.to_owned()));
        self.cached_pr_request(key, refresh, async move {
            if diff && let Some((head, base)) = revisions {
                return github.fetch_revision_diff(&url, &head, &base).await;
            }
            github.fetch_detail(&url, diff).await
        })
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
                // The body travels on stdin: a long comment would not fit a
                // Windows command line.
                args: vec![
                    "api".into(),
                    "--method".into(),
                    "POST".into(),
                    endpoint,
                    "--input".into(),
                    "-".into(),
                ],
                stdin: Some(
                    serde_json::to_vec(&serde_json::json!({ "body": body }))
                        .map_err(|_| ChangeRequestError::Decode)?,
                ),
                ..github_request()
            })
            .await
            .map_err(classify_run_error)?;
        if !output.success {
            return Err(classify_github_failure(&output.stderr));
        }
        // Evict after the server accepted the write, even if its reply cannot
        // be decoded. A read already in flight may predate it: keep it out too.
        let key = format!("false:{url}:");
        let mut cache = self.pr_cache.lock().await;
        cache.entries.retain(|entry| !entry.0.starts_with(&key));
        cache.in_flight.retain(|entry, _| !entry.starts_with(&key));
        drop(cache);
        if output.stdout_truncated {
            return Err(ChangeRequestError::Decode);
        }
        let value: serde_json::Value =
            serde_json::from_slice(&output.stdout).map_err(|_| ChangeRequestError::Decode)?;
        Ok(zeron_proto::ChangeRequestComment {
            id: value["node_id"].as_str().unwrap_or_default().into(),
            url: value["html_url"].as_str().unwrap_or_default().into(),
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

    async fn fetch_revision_diff(&self, url: &str, head: &str, base: &str) -> PrResult {
        let parsed = reqwest::Url::parse(url).map_err(|_| ChangeRequestError::Decode)?;
        let parts: Vec<_> = parsed.path().trim_matches('/').split('/').collect();
        let output = self
            .runner
            .run(ProcessRequest {
                args: vec![
                    "api".into(),
                    format!("repos/{}/{}/compare/{base}...{head}", parts[0], parts[1]),
                    "-H".into(),
                    "Accept: application/vnd.github.diff".into(),
                ],
                ..github_request()
            })
            .await
            .map_err(classify_run_error)?;
        if !output.success {
            return Err(classify_github_failure(&output.stderr));
        }
        if output.stdout_truncated {
            return Err(ChangeRequestError::Decode);
        }
        Ok(serde_json::Value::String(
            String::from_utf8_lossy(&output.stdout).into_owned(),
        ))
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
            url.clone(),
        ];
        if diff {
            args.push("--color=never".into());
        } else {
            args.extend(["--json".into(), "title,body,url,number,author,baseRefName,headRefName,headRefOid,baseRefOid,state,isDraft,reviewDecision,mergeable,additions,deletions,comments,reviews,files,statusCheckRollup".into()]);
        }
        let output = self
            .runner
            .run(ProcessRequest {
                args,
                ..github_request()
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
            let checks_reported = value.get("statusCheckRollup").is_some();
            remove_nulls(&mut value);
            let mut detail: zeron_proto::ChangeRequestDetail =
                serde_json::from_value(value).map_err(|_| ChangeRequestError::Decode)?;
            if checks_reported {
                detail.ci = zeron_proto::change_request_assessment::ChangeRequestCi::from_checks(
                    &detail.status_check_rollup,
                );
            }
            if zeron_proto::change_request_assessment::valid_head_oid(&detail.head_ref_oid)
                && let Ok(metadata) = self.fetch_pr_metadata(&detail.url, &url).await
            {
                // Who "you" are, so every comment of yours reads as yours,
                // reviews included, before you've commented here.
                detail.viewer_login = metadata.viewer_login.clone();
                let checks = metadata.checks();
                let (head, ci, authored, requested) = metadata.into_fields();
                if head == detail.head_ref_oid {
                    if let Some(checks) = checks {
                        detail.status_check_rollup = checks;
                    } else if ci.state != detail.ci.state {
                        detail.status_check_rollup.clear();
                    }
                    detail.ci = ci;
                    detail.viewer_did_author = authored;
                    detail.viewer_review_requested = requested;
                } else {
                    // Detail/check names belong to an older head: do not attest
                    // to them after the branch moved during the two reads.
                    detail.ci = Default::default();
                    detail.status_check_rollup.clear();
                }
            }
            detail.review_threads = self.fetch_review_threads(&url).await.ok();
            serde_json::to_value(detail).map_err(|_| ChangeRequestError::Decode)
        }
    }

    async fn fetch_pr_metadata(
        &self,
        canonical: &str,
        requested: &str,
    ) -> Result<GhPrMetadata, ChangeRequestError> {
        let url = validated_pull_request_url(if canonical.is_empty() {
            requested
        } else {
            canonical
        })?;
        let parts: Vec<_> = url.split('/').collect();
        let output = self
            .runner
            .run(ProcessRequest {
                args: vec![
                    "api".into(),
                    "graphql".into(),
                    "-f".into(),
                    format!("query={GITHUB_PR_METADATA_QUERY}"),
                    "-f".into(),
                    format!("owner={}", parts[3]),
                    "-f".into(),
                    format!("name={}", parts[4]),
                    "-F".into(),
                    format!("number={}", parts[6]),
                ],
                ..github_request()
            })
            .await
            .map_err(classify_run_error)?;
        if !output.success {
            return Err(classify_github_failure(&output.stderr));
        }
        if output.stdout_truncated {
            return Err(ChangeRequestError::Decode);
        }
        let value: serde_json::Value =
            serde_json::from_slice(&output.stdout).map_err(|_| ChangeRequestError::Decode)?;
        if value["errors"]
            .as_array()
            .is_some_and(|errors| !errors.is_empty())
        {
            return Err(ChangeRequestError::Decode);
        }
        let mut metadata: GhPrMetadata =
            serde_json::from_value(value["data"]["repository"]["pullRequest"].clone())
                .map_err(|_| ChangeRequestError::Decode)?;
        metadata.viewer_login = value["data"]["viewer"]["login"]
            .as_str()
            .filter(|login| !login.is_empty())
            .map(str::to_owned);
        Ok(metadata)
    }

    /// GitHub's current `owner/name` for `repository`, following renames.
    /// `None` when GitHub can't resolve it (missing, private to another
    /// account, offline). Shares the board's cache and rate-limit pause.
    pub async fn canonical_repository(&self, repository: &str) -> Option<String> {
        if !valid_pr_repository(repository) {
            return None;
        }
        let repository = repository.to_ascii_lowercase();
        let key = format!("repository:{repository}");
        let github = self.clone();
        let lookup = self.cached_pr_request(key, false, async move {
            let (owner, name) = repository
                .split_once('/')
                .ok_or(ChangeRequestError::UnsupportedRepository)?;
            let output = github
                .runner
                .run(ProcessRequest {
                    args: vec![
                        "api".into(),
                        "graphql".into(),
                        "-f".into(),
                        format!("query={GITHUB_REPOSITORY_QUERY}"),
                        "-f".into(),
                        format!("owner={owner}"),
                        "-f".into(),
                        format!("name={name}"),
                    ],
                    ..github_request()
                })
                .await
                .map_err(classify_run_error)?;
            if !output.success {
                return Err(classify_github_failure(&output.stderr));
            }
            if output.stdout_truncated {
                return Err(ChangeRequestError::Decode);
            }
            let value: serde_json::Value =
                serde_json::from_slice(&output.stdout).map_err(|_| ChangeRequestError::Decode)?;
            Ok(value["data"]["repository"]["nameWithOwner"].clone())
        });
        let value = tokio::time::timeout(Duration::from_secs(5), lookup)
            .await
            .ok()?
            .ok()?;
        value
            .as_str()
            .filter(|canonical| valid_pr_repository(canonical))
            .map(str::to_owned)
    }

    pub fn new() -> Self {
        Self::with_runner(Arc::new(SystemProcessRunner))
    }

    fn with_runner(runner: Arc<dyn ProcessRunner>) -> Self {
        Self {
            runner,
            pr_cache: Default::default(),
            store: None,
            account: Default::default(),
        }
    }

    /// Keep the board's first pages in `data_dir`, so it opens at once.
    pub(crate) fn with_store(mut self, data_dir: &Path) -> Self {
        self.store = Some(crate::change_request_store::ChangeRequestStore::new(
            data_dir,
        ));
        self
    }

    /// The active github.com account, read from `gh`'s local config (no
    /// network). Re-read every 30 seconds so `gh auth switch` takes effect.
    /// `None` when it can't be told — including a token in the environment,
    /// which may belong to another account — and then nothing is stored.
    async fn store_account(&self) -> Option<String> {
        if ["GH_TOKEN", "GITHUB_TOKEN"]
            .iter()
            .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
        {
            return None;
        }
        let mut account = self.account.lock().await;
        if let Some((read_at, login)) = account.as_ref()
            && read_at.elapsed() < Duration::from_secs(30)
        {
            return login.clone();
        }
        let login = self
            .runner
            .run(ProcessRequest {
                args: vec![
                    "config".into(),
                    "get".into(),
                    "user".into(),
                    "-h".into(),
                    "github.com".into(),
                ],
                timeout: Duration::from_secs(5),
                ..github_request()
            })
            .await
            .ok()
            .filter(|output| output.success && !output.stdout_truncated)
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .map(|login| login.trim().to_owned())
            .filter(|login| {
                !login.is_empty()
                    && login.len() <= 39
                    && login
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            });
        *account = Some((Instant::now(), login.clone()));
        login
    }

    /// One page of open pull requests, 50 at a time, newest updates first.
    ///
    /// `cached` asks for the first page's on-disk copy, whatever its age, so
    /// the board can show it at once and refresh behind it; without one it
    /// reads GitHub as usual. Every first page read from GitHub is stored.
    pub async fn list_page(
        &self,
        repository: &str,
        filter: zeron_proto::ChangeRequestFilter,
        after: Option<&str>,
        refresh: bool,
        cached: bool,
    ) -> Result<zeron_proto::ChangeRequestPage, ChangeRequestError> {
        if !valid_pr_repository(repository) {
            return Err(ChangeRequestError::UnsupportedRepository);
        }
        if after.is_some_and(|cursor| !valid_page_cursor(cursor)) {
            return Err(ChangeRequestError::Decode);
        }
        let first_page = after.is_none();
        let stored = match &self.store {
            Some(store) if first_page => self
                .store_account()
                .await
                .map(|account| (store.clone(), account)),
            _ => None,
        };
        if cached
            && !refresh
            && let Some((store, account)) = &stored
            && let Some(page) = store.load(account, repository, filter).await
        {
            return Ok(page);
        }
        let page = self.fetch_list_page(repository, filter, after, refresh).await?;
        if let Some((store, account)) = stored {
            store.save(&account, repository, filter, &page).await;
        }
        Ok(page)
    }

    async fn fetch_list_page(
        &self,
        repository: &str,
        filter: zeron_proto::ChangeRequestFilter,
        after: Option<&str>,
        refresh: bool,
    ) -> Result<zeron_proto::ChangeRequestPage, ChangeRequestError> {
        let repository = repository.to_ascii_lowercase();
        let key = format!("list:{repository}:{filter:?}:{}", after.unwrap_or_default());
        let github = self.clone();
        let after = after.map(str::to_owned);
        let result = self
            .cached_pr_request(key, refresh, async move {
                let page = github
                    .fetch_filtered_page(&repository, filter, after.as_deref())
                    .await?;
                serde_json::to_value(page).map_err(|_| ChangeRequestError::Decode)
            })
            .await?;
        serde_json::from_value(result).map_err(|_| ChangeRequestError::Decode)
    }

    async fn fetch_filtered_page(
        &self,
        repository: &str,
        filter: zeron_proto::ChangeRequestFilter,
        after: Option<&str>,
    ) -> Result<zeron_proto::ChangeRequestPage, ChangeRequestError> {
        let (page, canonical) = self.fetch_scoped_search(repository, filter, after).await?;
        if after.is_none()
            && page.items.is_empty()
            && let Some(canonical) = canonical
            && !canonical.eq_ignore_ascii_case(repository)
        {
            // Search does not follow repository renames, although repository()
            // does. Follow only that verified canonical name, at most once.
            // Clients adopt the canonical name, so later pages query it directly.
            return self
                .fetch_scoped_search(&canonical, filter, None)
                .await
                .map(|result| result.0);
        }
        Ok(page)
    }

    async fn fetch_scoped_search(
        &self,
        repository: &str,
        filter: zeron_proto::ChangeRequestFilter,
        after: Option<&str>,
    ) -> Result<(zeron_proto::ChangeRequestPage, Option<String>), ChangeRequestError> {
        let (owner, name) = repository
            .split_once('/')
            .ok_or(ChangeRequestError::UnsupportedRepository)?;
        let qualifier = match filter {
            zeron_proto::ChangeRequestFilter::All => "",
            zeron_proto::ChangeRequestFilter::Authored => "author:@me ",
            zeron_proto::ChangeRequestFilter::Reviewing => "review-requested:@me ",
        };
        let request = ProcessRequest {
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
            ]
            .into_iter()
            .chain(
                after
                    .into_iter()
                    .flat_map(|cursor| ["-f".into(), format!("after={cursor}")]),
            )
            .collect(),
            stdin: None,
            ..github_request()
        };
        let output = self.runner.run(request).await.map_err(classify_run_error)?;
        let response = (!output.stdout_truncated)
            .then(|| serde_json::from_slice::<GhSearchResponse>(&output.stdout).ok())
            .flatten();
        // `gh` exits non-zero whenever the response carries `errors`, including
        // the one that accompanies results the viewer cannot read. Those pages
        // are still usable once the repository itself resolved.
        let response = match response {
            Some(response) if output.success || response.data.repository.is_some() => response,
            _ if !output.success => return Err(classify_github_failure(&output.stderr)),
            _ => return Err(ChangeRequestError::Decode),
        };
        let partial_metadata = !response.errors.is_empty();
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
            .flatten()
            .map(to_list_item)
            .collect::<Result<Vec<_>, _>>()?;
        for item in &mut items {
            if partial_metadata {
                item.ci = Default::default();
                item.viewer_did_author = None;
                item.viewer_review_requested = None;
            }
            if filter == zeron_proto::ChangeRequestFilter::Authored {
                item.viewer_did_author = Some(true);
            }
            if filter == zeron_proto::ChangeRequestFilter::Reviewing {
                item.viewer_review_requested = Some(true);
            }
        }
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
        let info = response.data.search.page_info;
        let next_cursor = info
            .filter(|info| info.has_next_page && !items.is_empty())
            .and_then(|info| info.end_cursor)
            .filter(|cursor| valid_page_cursor(cursor));
        Ok((
            zeron_proto::ChangeRequestPage {
                items,
                next_cursor,
                total_count: response.data.search.issue_count,
                fetched_at: Some(Utc::now()),
            },
            canonical,
        ))
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
            stdin: None,
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
            stdin: None,
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

/// Board requests run against GitHub.com without a checkout or interactive prompts.
fn github_request() -> ProcessRequest {
    ProcessRequest {
        program: "gh".into(),
        args: Vec::new(),
        stdin: None,
        cwd: None,
        env: vec![
            ("GH_PROMPT_DISABLED".into(), "1".into()),
            ("GH_HOST".into(), "github.com".into()),
        ],
        timeout: GITHUB_TIMEOUT,
        output_limit: GITHUB_OUTPUT_LIMIT,
    }
}

impl Default for GitHubCli {
    fn default() -> Self {
        Self::new()
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

/// The scp-like (`git@host:owner/repo`) and `ssh://` forms, whose host may
/// be an `~/.ssh/config` alias. HTTP(S) hosts are always literal.
fn is_ssh_remote(remote_url: &str) -> bool {
    match remote_url.trim().split_once("://") {
        Some((scheme, _)) => scheme.eq_ignore_ascii_case("ssh"),
        None => true,
    }
}

/// A host alias safe to hand to `ssh -G` as its destination argument: never
/// an option, a user, or a port.
fn valid_ssh_alias(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && !host.starts_with('-')
        && host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

/// The `hostname` line of `ssh -G` output: the host an alias connects to.
fn ssh_config_hostname(config: &str) -> Option<&str> {
    config.lines().find_map(|line| {
        let (key, value) = line.trim().split_once(char::is_whitespace)?;
        key.eq_ignore_ascii_case("hostname").then(|| value.trim())
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
                stdin: None,
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
    #[serde(flatten)]
    metadata: GhPrMetadata,
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
#[serde(rename_all = "camelCase")]
struct GhPrMetadata {
    #[serde(default)]
    head_ref_oid: Option<String>,
    #[serde(default)]
    viewer_did_author: Option<bool>,
    #[serde(default)]
    viewer_latest_review_request: Option<GhReviewRequestId>,
    #[serde(default)]
    review_requests: Option<GhReviewRequests>,
    // Missing and explicit null have different meanings (unavailable vs no checks).
    #[serde(default)]
    #[serde(deserialize_with = "present_nullable")]
    status_check_rollup: Option<Option<GhCiRollup>>,
    /// The signed-in account, read beside the pull request (`viewer`).
    #[serde(skip)]
    viewer_login: Option<String>,
}

impl GhPrMetadata {
    fn checks(&self) -> Option<Vec<zeron_proto::ChangeRequestCheck>> {
        match self.status_check_rollup.as_ref()? {
            None => Some(Vec::new()),
            Some(rollup) => rollup.contexts.nodes.as_ref().map(|nodes| {
                nodes
                    .iter()
                    .filter(|node| !node.is_null())
                    .map(|node| {
                        // CheckRun and legacy StatusContext have different nullable fields.
                        let text =
                            |field: &str| node[field].as_str().unwrap_or_default().to_owned();
                        zeron_proto::ChangeRequestCheck {
                            name: text("name"),
                            context: text("context"),
                            status: text("status"),
                            conclusion: text("conclusion"),
                            state: text("state"),
                            details_url: text("detailsUrl"),
                            target_url: text("targetUrl"),
                        }
                    })
                    .collect()
            }),
        }
    }

    fn into_fields(
        self,
    ) -> (
        String,
        zeron_proto::change_request_assessment::ChangeRequestCi,
        Option<bool>,
        Option<bool>,
    ) {
        use zeron_proto::change_request_assessment::{ChangeRequestCi, CiState};
        let ci = match self.status_check_rollup {
            None => ChangeRequestCi::default(),
            Some(None) => ChangeRequestCi {
                state: CiState::NoChecks,
                total_count: 0,
            },
            Some(Some(rollup)) => {
                let mut state = match rollup.state.as_str() {
                    "SUCCESS" => CiState::Passed,
                    "FAILURE" | "ERROR" => CiState::Failed,
                    "PENDING" | "EXPECTED" => CiState::Pending,
                    _ => CiState::Unknown,
                };
                let skipped: u64 = rollup
                    .contexts
                    .check_run_counts_by_state
                    .as_ref()
                    .into_iter()
                    .flatten()
                    .filter(|count| matches!(count.state.as_str(), "NEUTRAL" | "SKIPPED"))
                    .map(|count| count.count)
                    .fold(0u64, u64::saturating_add);
                if state == CiState::Passed && rollup.contexts.total_count == 0 {
                    state = CiState::NoChecks;
                }
                if state == CiState::Passed && skipped > 0 && skipped == rollup.contexts.total_count
                {
                    state = CiState::Skipped;
                }
                // Aggregate success must never hide failed/cancelled jobs, even if
                // GitHub does not consider them required for merging.
                if rollup
                    .contexts
                    .check_run_counts_by_state
                    .as_ref()
                    .into_iter()
                    .flatten()
                    .chain(
                        rollup
                            .contexts
                            .status_context_counts_by_state
                            .as_ref()
                            .into_iter()
                            .flatten(),
                    )
                    .any(|count| {
                        count.count > 0
                            && zeron_proto::change_request_assessment::check_failed(&count.state)
                    })
                {
                    state = CiState::Failed;
                }
                if rollup
                    .commit
                    .as_ref()
                    .is_some_and(|commit| Some(commit.oid.as_str()) != self.head_ref_oid.as_deref())
                {
                    state = CiState::Unknown;
                }
                ChangeRequestCi {
                    state,
                    total_count: rollup.contexts.total_count,
                }
            }
        };
        let requested = self.review_requests.and_then(|requests| {
            let found = self
                .viewer_latest_review_request
                .as_ref()
                .is_some_and(|latest| {
                    requests
                        .nodes
                        .iter()
                        .flatten()
                        .any(|request| request.id == latest.id)
                });
            if found {
                Some(true)
            } else if requests.page_info.has_next_page {
                None
            } else {
                Some(false)
            }
        });
        (
            self.head_ref_oid.unwrap_or_default(),
            ci,
            self.viewer_did_author,
            requested,
        )
    }
}

fn present_nullable<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

#[derive(Debug, Deserialize)]
struct GhReviewRequestId {
    id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhReviewRequests {
    nodes: Vec<Option<GhReviewRequestId>>,
    page_info: GhPageInfo,
}

#[derive(Debug, Deserialize)]
struct GhCiRollup {
    state: String,
    contexts: GhCiCount,
    #[serde(default)]
    commit: Option<GhCiCommit>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhCiCount {
    total_count: u64,
    #[serde(default)]
    nodes: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    check_run_counts_by_state: Option<Vec<GhCiStateCount>>,
    #[serde(default)]
    status_context_counts_by_state: Option<Vec<GhCiStateCount>>,
}

#[derive(Debug, Deserialize)]
struct GhCiCommit {
    oid: String,
}

#[derive(Debug, Deserialize)]
struct GhCiStateCount {
    state: String,
    count: u64,
}

#[derive(Debug, Deserialize)]
struct GhSearchResponse {
    data: GhSearchData,
    #[serde(default)]
    errors: Vec<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct GhSearchData {
    #[serde(default)]
    repository: Option<GhSearchRepository>,
    search: GhSearchConnection,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhSearchConnection {
    /// GitHub returns `null` for results the viewer cannot read (for
    /// example SAML-restricted repositories); those are skipped.
    nodes: Vec<Option<GhSearchPullRequest>>,
    #[serde(default)]
    issue_count: Option<u64>,
    #[serde(default)]
    page_info: Option<GhPageInfo>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhPageInfo {
    has_next_page: bool,
    end_cursor: Option<String>,
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
    let (head_ref_oid, ci, authored, requested) = pull_request.metadata.into_fields();
    Ok(ChangeRequestListItem {
        provider: "github".into(),
        author: pull_request.author.unwrap_or_default(),
        head_ref_oid,
        ci,
        viewer_did_author: authored,
        viewer_review_requested: requested,
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
    /// Written to the child's standard input, which is then closed.
    stdin: Option<Vec<u8>>,
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
            .stdin(if request.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .map_err(|error| ProcessRunError::Spawn(error.kind()))?;
        let stdout = child.stdout.take().ok_or(ProcessRunError::Io)?;
        let stderr = child.stderr.take().ok_or(ProcessRunError::Io)?;
        let input = child.stdin.take().zip(request.stdin);
        let completed = tokio::time::timeout(request.timeout, async {
            tokio::try_join!(
                child.wait(),
                read_capped(stdout, request.output_limit),
                read_capped(stderr, request.output_limit),
                async {
                    // A child that exits without reading reports through its
                    // status; a broken pipe here is not the failure.
                    if let Some((mut pipe, bytes)) = input {
                        let _ = pipe.write_all(&bytes).await;
                    }
                    Ok(())
                },
            )
        })
        .await;

        let (status, (stdout, stdout_truncated), (stderr, _stderr_truncated), ()) = match completed
        {
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

    /// Real engine dispatch, wire envelopes and GitHub decoding; only subprocess
    /// execution is scripted. Repository discovery still runs real local Git.
    #[tokio::test]
    async fn pull_request_workflow_crosses_engine_transport_and_github() {
        use serde_json::json;
        use zeron_proto::{ChangeRequestComment, ChangeRequestDetail, ChangeRequestPage};
        use zeron_rpc::{RpcError, capability_errors, methods};

        let dir = tempfile::tempdir().unwrap();
        let checkout = dir.path().join("checkout");
        std::fs::create_dir(&checkout).unwrap();
        run_git(&checkout, &["init", "--quiet"]);
        run_git(
            &checkout,
            &["remote", "add", "origin", "git@github.com:acme/zeron.git"],
        );
        let core = crate::EngineCore::assemble(
            &dir.path().join("engine"),
            Arc::new(crate::HarnessRegistry::new()),
            zeron_proto::HarnessId::Mock,
            None,
        )
        .unwrap();
        core.workspace
            .create_space(
                "space-pr",
                &core.device_id,
                &checkout.to_string_lossy(),
                None,
                true,
            )
            .unwrap();
        let url = "https://github.com/acme/zeron/pull/123";
        let patch = "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-old\n+new\n";
        let body = "Literal `$(touch /tmp/never)`; --flag\n\"quoted\" @octocat";
        let page = |number, cursor: Option<&str>| {
            json!({
                "data": {
                    "repository": {"nameWithOwner": "acme/zeron"},
                    "search": {
                        "nodes": [search_pull_request("acme/zeron", number, "Add dashboard", "OPEN", "MERGEABLE",
                            "2026-08-10T09:30:00Z", "2026-08-19T12:00:00Z", false, Some("CHANGES_REQUESTED"))],
                        "issueCount": 2,
                        "pageInfo": {"hasNextPage": cursor.is_some(), "endCursor": cursor}
                    }
                }
            })
        };
        let head = "a".repeat(40);
        let metadata = json!({"headRefOid": head, "viewerDidAuthor": true,
            "viewerLatestReviewRequest": null,
            "reviewRequests": {"nodes": [], "pageInfo": {"hasNextPage": false}},
            "statusCheckRollup": {"commit": {"oid": head}, "state": "FAILURE", "contexts": {"totalCount": 1,
                "nodes": [{"name": "linux", "status": "COMPLETED", "conclusion": "FAILURE", "detailsUrl": "https://github.com/acme/zeron/actions/runs/1"}]}}
        });
        let metadata_response = json!({"data": {"repository": {"pullRequest": metadata}}});
        let detail = json!({"url": url, "number": 123, "title": "Add dashboard", "author": null,
            "headRefOid": head, "comments": [], "reviewDecision": null,
            "statusCheckRollup": [{"name": "linux", "conclusion": "SUCCESS", "detailsUrl": "https://github.com/acme/zeron/actions/runs/1"}]});
        let mut moved_head = metadata_response.clone();
        moved_head["data"]["repository"]["pullRequest"]["headRefOid"] = "b".repeat(40).into();
        moved_head["data"]["repository"]["pullRequest"]["statusCheckRollup"]["commit"]["oid"] =
            "b".repeat(40).into();
        let mut updated = detail.clone();
        updated["comments"] =
            json!([{"body": body, "author": {"login": "writer"}, "viewerDidAuthor": true}]);
        let connection = |nodes: serde_json::Value, cursor: Option<&str>| {
            json!({
                "nodes": nodes, "pageInfo": {"hasNextPage": cursor.is_some(), "endCursor": cursor}
            })
        };
        let inline = json!({"id":"thread-1","isResolved":false,"isOutdated":false,"path":"a.rs","line":12,
            "comments":connection(json!([{"id":"comment-1","body":"Inline-only change request","author":{"login":"reviewer"},"url":"https://github.com/acme/zeron/pull/123#discussion_r1"}]),Some("comments-next"))});
        let threads_first = json!({"data":{"repository":{"pullRequest":{"reviewThreads":connection(json!([inline]),Some("threads-next"))}}}});
        let replies = json!({"data":{"node":{"comments":connection(json!([{"id":"comment-2","body":"A reply","author":null,"replyTo":{"id":"comment-1"}}]),None)}}});
        let threads_last = json!({"data":{"repository":{"pullRequest":{"reviewThreads":connection(json!([{
            "id":"thread-2","isResolved":true,"isOutdated":true,"path":"old.rs","originalLine":4,
            "comments":connection(json!([]),None)
        }]),None)}}}});
        let threads_empty = json!({"data":{"repository":{"pullRequest":{"reviewThreads":connection(json!([]),None)}}}});
        let renamed = |name: &str| {
            command_success(
                serde_json::to_vec(&json!({"data":{"repository":{"nameWithOwner": name}}}))
                    .unwrap(),
            )
        };
        let runner = FakeProcessRunner::with_responses([
            // Matching a renamed repository resolves origin, then upstream.
            renamed("acme/renamed"),
            renamed("acme/upstream"),
            command_success(serde_json::to_vec(&page(123, Some("Y3Vyc29yOjE="))).unwrap()),
            command_success(serde_json::to_vec(&page(124, None)).unwrap()),
            command_success(serde_json::to_vec(&detail).unwrap()),
            command_success(serde_json::to_vec(&metadata_response).unwrap()),
            command_success(serde_json::to_vec(&threads_first).unwrap()),
            command_success(serde_json::to_vec(&replies).unwrap()),
            command_success(serde_json::to_vec(&threads_last).unwrap()),
            command_success(patch),
            command_success(
                serde_json::to_vec(&json!({"body": body, "user": {"login": "writer"}})).unwrap(),
            ),
            command_success(serde_json::to_vec(&updated).unwrap()),
            command_success(serde_json::to_vec(&moved_head).unwrap()),
            command_success(serde_json::to_vec(&threads_empty).unwrap()),
            command_failure("authentication required: gh auth login; private-provider-diagnostic"),
        ]);
        let github = GitHubCli::with_runner(runner.clone());
        let rpc = Arc::try_unwrap(core.rpc_service())
            .ok()
            .unwrap()
            .with_github(github);
        let client = zeron_rpc::memory_client(Arc::new(rpc));
        let repository: Option<String> = client
            .call_as(
                methods::GET_CHANGE_REQUEST_REPOSITORY,
                json!({"cwd": checkout, "targetDeviceId": core.device_id}),
            )
            .await
            .unwrap();
        assert_eq!(repository.as_deref(), Some("acme/zeron"));
        // A directory that is no chat or project checkout is refused.
        let outside = dir.path().join("elsewhere");
        std::fs::create_dir(&outside).unwrap();
        run_git(&outside, &["init", "--quiet"]);
        assert!(
            client
                .call(
                    methods::GET_CHANGE_REQUEST_REPOSITORY,
                    json!({"cwd": outside, "targetDeviceId": core.device_id}),
                )
                .await
                .is_err()
        );
        // Repository grouping cannot identify a fork's GitHub slug. The
        // selected branch's remote must win over an unrelated origin.
        run_git(
            &checkout,
            &[
                "remote",
                "add",
                "upstream",
                "https://github.com/acme/upstream.git",
            ],
        );
        // Handoffs may use an upstream checkout even when origin is a fork.
        // Default board discovery must continue to select origin.
        // A remote written before a rename matches the repository's current
        // name through GitHub; the lookup is cached, so a miss afterwards
        // only resolves the remotes it hasn't seen.
        for (wanted, expected) in [
            ("ACME/UPSTREAM", Some("acme/upstream")),
            ("acme/zeron", Some("acme/zeron")),
            ("acme/renamed", Some("acme/renamed")),
            ("missing/repository", None),
            ("../invalid", None),
        ] {
            let matched: Option<String> = client.call_as(
                methods::GET_CHANGE_REQUEST_REPOSITORY,
                json!({"cwd": checkout, "repository": wanted, "targetDeviceId": core.device_id}),
            ).await.unwrap();
            assert_eq!(matched.as_deref(), expected);
        }
        run_git(
            &checkout,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "--allow-empty",
                "--quiet",
                "-m",
                "initial",
            ],
        );
        run_git(&checkout, &["branch", "-M", "feature"]);
        run_git(&checkout, &["config", "branch.feature.remote", "upstream"]);
        let tracked: Option<String> = client
            .call_as(
                methods::GET_CHANGE_REQUEST_REPOSITORY,
                json!({"cwd": checkout}),
            )
            .await
            .unwrap();
        assert_eq!(tracked.as_deref(), Some("acme/upstream"));
        run_git(&checkout, &["checkout", "--detach", "--quiet"]);
        let detached: Option<String> = client
            .call_as(
                methods::GET_CHANGE_REQUEST_REPOSITORY,
                json!({"cwd": checkout}),
            )
            .await
            .unwrap();
        assert_eq!(detached.as_deref(), Some("acme/zeron"));
        let first: ChangeRequestPage = client.call_as(methods::LIST_CHANGE_REQUEST_PAGE,
            json!({"repository": repository, "filter": "reviewing", "targetDeviceId": core.device_id})).await.unwrap();
        assert_eq!(first.items[0].number, 123);
        assert_eq!(first.items[0].repository, "acme/zeron");
        assert_eq!(first.total_count, Some(2));
        let second: ChangeRequestPage = client.call_as(methods::LIST_CHANGE_REQUEST_PAGE,
            json!({"repository": repository, "filter": "reviewing", "after": first.next_cursor})).await.unwrap();
        assert_eq!(second.items[0].number, 124);
        assert!(second.next_cursor.is_none());
        let params = json!({"url": url, "targetDeviceId": core.device_id});
        for _ in 0..2 {
            let loaded: ChangeRequestDetail = client
                .call_as(methods::GET_CHANGE_REQUEST, params.clone())
                .await
                .unwrap();
            assert_eq!(loaded.title, "Add dashboard");
            assert_eq!(loaded.number, 123);
            assert!(loaded.author.login.is_empty());
            assert!(loaded.comments.is_empty());
            assert_eq!(loaded.head_ref_oid, head);
            assert_eq!(
                loaded.ci.state,
                zeron_proto::change_request_assessment::CiState::Failed
            );
            assert_eq!(loaded.ci.total_count, 1);
            assert_eq!(
                loaded.status_check_rollup[0].conclusion, "FAILURE",
                "CI and rows use one snapshot even when a job changed on the same head"
            );
            assert_eq!(loaded.viewer_did_author, Some(true));
            let threads = loaded.review_threads.as_ref().unwrap();
            assert_eq!(threads.len(), 2);
            assert_eq!(threads[0].comments.len(), 2);
            assert_eq!(threads[0].comments[0].body, "Inline-only change request");
            assert_eq!(
                threads[0].comments[1].reply_to.as_ref().unwrap().id,
                "comment-1"
            );
            assert!(threads[0].comments[1].author.login.is_empty());
            assert!(!threads[0].is_resolved);
            assert!(threads[1].is_resolved && threads[1].is_outdated);
        }
        assert_eq!(
            runner.requests().len(),
            9,
            "the second detail read uses the real provider cache"
        );
        let diff: String = client
            .call_as(methods::GET_CHANGE_REQUEST_DIFF, params.clone())
            .await
            .unwrap();
        assert_eq!(diff, patch);
        let comment: ChangeRequestComment = client
            .call_as(
                methods::POST_CHANGE_REQUEST_COMMENT,
                json!({"url": url, "body": body, "targetDeviceId": core.device_id}),
            )
            .await
            .unwrap();
        assert_eq!(comment.body, body);
        assert!(comment.viewer_did_author);
        let loaded: ChangeRequestDetail = client
            .call_as(methods::GET_CHANGE_REQUEST, params)
            .await
            .unwrap();
        assert_eq!(
            loaded.comments[0].body, body,
            "posting invalidates the cached thread"
        );
        assert_eq!(
            loaded.ci.state,
            zeron_proto::change_request_assessment::CiState::Unknown,
            "a changed head invalidates the old checks"
        );
        assert!(loaded.status_check_rollup.is_empty());
        let error = client
            .call(
                methods::POST_CHANGE_REQUEST_COMMENT,
                json!({"url": url, "body": "second comment"}),
            )
            .await
            .unwrap_err();
        assert!(matches!(&error, RpcError::Capability(code)
            if code == capability_errors::PULL_REQUESTS_AUTHENTICATION));
        assert!(!error.to_string().contains("private-provider-diagnostic"));

        for (method, params) in [
            (methods::LIST_CHANGE_REQUEST_PAGE, json!({})),
            (
                methods::LIST_CHANGE_REQUEST_PAGE,
                json!({"repository": "acme/zeron", "filter": "unknown"}),
            ),
            (
                methods::LIST_CHANGE_REQUEST_PAGE,
                json!({"repository": "acme/zeron repo:other/repo"}),
            ),
            (
                methods::LIST_CHANGE_REQUEST_PAGE,
                json!({"repository": "acme/zeron", "after": "a b"}),
            ),
            (
                methods::POST_CHANGE_REQUEST_COMMENT,
                json!({"url": url, "body": "  "}),
            ),
            (
                methods::POST_CHANGE_REQUEST_COMMENT,
                json!({"url": url, "body": "x".repeat(60_001)}),
            ),
        ] {
            let error = client.call(method, params).await.unwrap_err();
            assert!(error.to_string().starts_with("bad params"), "{error}");
        }
        let requests = runner.requests();
        assert_eq!(
            requests.len(),
            15,
            "invalid input never reaches gh and writes are not retried"
        );
        // The first two resolve the checkout's remotes to canonical names.
        for (request, name) in requests[..2].iter().zip(["name=zeron", "name=upstream"]) {
            assert!(request.args.contains(&"owner=acme".into()));
            assert!(request.args.contains(&name.into()), "{:?}", request.args);
        }
        let requests = &requests[2..];
        for request in requests.iter().chain(runner.requests()[..2].iter()) {
            assert_eq!(request.program, "gh");
            assert_eq!(
                request.env,
                [
                    ("GH_PROMPT_DISABLED".into(), "1".into()),
                    ("GH_HOST".into(), "github.com".into())
                ]
            );
            assert!(request.cwd.is_none());
            assert_eq!(request.timeout, GITHUB_TIMEOUT);
            assert_eq!(request.output_limit, GITHUB_OUTPUT_LIMIT);
        }
        assert!(requests[0].args.contains(
            &"search=is:pr is:open review-requested:@me repo:acme/zeron sort:updated-desc".into()
        ));
        assert!(requests[1].args.contains(&"after=Y3Vyc29yOjE=".into()));
        assert_eq!(&requests[2].args[..3], ["pr", "view", url]);
        let variables = |index: usize| {
            serde_json::from_slice::<serde_json::Value>(requests[index].stdin.as_ref().unwrap())
                .unwrap()["variables"]
                .clone()
        };
        assert_eq!(variables(5)["after"], "comments-next");
        assert_eq!(variables(5)["id"], "thread-1");
        assert_eq!(variables(6)["after"], "threads-next");
        assert_eq!(requests[7].args, ["pr", "diff", url, "--color=never"]);
        assert_eq!(
            requests[8].args,
            [
                "api",
                "--method",
                "POST",
                "repos/acme/zeron/issues/123/comments",
                "--input",
                "-"
            ]
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(requests[8].stdin.as_ref().unwrap())
                .unwrap(),
            json!({"body": body})
        );
        assert!(!requests[8].args.iter().any(|arg| arg.contains(body)));
    }

    #[tokio::test]
    async fn pr_known_revision_bypasses_recent_detail_cache_and_pins_diff() {
        use serde_json::json;
        let url = "https://github.com/a/b/pull/1";
        let old = "a".repeat(40);
        let head = "b".repeat(40);
        let base = "c".repeat(40);
        let runner = FakeProcessRunner::with_responses([
            command_success(serde_json::to_vec(&json!({"headRefOid":head,"baseRefOid":base,"number":1})).unwrap()),
            command_failure("metadata unavailable"),
            command_success(br#"{"data":{"repository":{"pullRequest":{"reviewThreads":{"nodes":[],"pageInfo":{"hasNextPage":false}}}}}}"#.to_vec()),
            command_success("new patch"),
        ]);
        let github = GitHubCli::with_runner(runner.clone());
        for key in [
            format!("false:{url}::"),
            format!("false:{url}:{old}:"),
            format!("true:{url}::"),
        ] {
            github.pr_cache.lock().await.entries.push((
                key,
                Instant::now(),
                Ok(json!({"headRefOid":old})),
            ));
        }
        let detail = github
            .detail_request(url, false, true, Some(&head), None)
            .await
            .unwrap();
        assert_eq!(detail["headRefOid"], head);
        let diff = github
            .detail_request(url, true, true, Some(&head), Some(&base))
            .await
            .unwrap();
        assert_eq!(diff, "new patch");
        let requests = runner.requests();
        assert_eq!(requests.len(), 4);
        assert_eq!(
            requests[3].args,
            [
                "api",
                &format!("repos/a/b/compare/{base}...{head}"),
                "-H",
                "Accept: application/vnd.github.diff"
            ]
        );
        assert!(
            github
                .detail_request(url, true, false, Some("--help"), None)
                .await
                .is_err()
        );
        assert_eq!(runner.requests().len(), 4);
    }

    #[test]
    fn ssh_host_aliases_resolve_only_from_safe_ssh_remotes() {
        assert!(is_ssh_remote("git@github-work:acme/zeron.git"));
        assert!(is_ssh_remote("ssh://git@github-work/acme/zeron.git"));
        assert!(!is_ssh_remote("https://github-work/acme/zeron.git"));
        assert!(valid_ssh_alias("github-work"));
        assert!(valid_ssh_alias("gh.work_2"));
        for alias in ["", "-oProxyCommand=x", "a b", "user@host", "host:22", "a;b"] {
            assert!(!valid_ssh_alias(alias), "{alias}");
        }
        let config = "user git\nhostname github.com\nport 22\n";
        assert_eq!(ssh_config_hostname(config), Some("github.com"));
        assert_eq!(ssh_config_hostname("user git\n"), None);
    }

    #[tokio::test]
    async fn ssh_alias_remote_matches_through_ssh_config() {
        let runner = FakeProcessRunner::with_responses([command_success(
            "user git\nhostname github.com\n",
        )]);
        let resolver = ChangeRequestResolver {
            inspector: GitCheckoutInspector::new(runner.clone()),
            github: GitHubCli::with_runner(runner.clone()),
        };
        assert_eq!(
            resolver
                .github_slug("git@github-work:acme/zeron.git")
                .await
                .as_deref(),
            Some("acme/zeron")
        );
        let request = &runner.requests()[0];
        assert_eq!(request.program, "ssh");
        assert_eq!(request.args, ["-G", "github-work"]);
        // Literal github.com and HTTPS hosts never consult ssh.
        assert_eq!(
            resolver
                .github_slug("https://github.com/acme/zeron")
                .await
                .as_deref(),
            Some("acme/zeron")
        );
        assert_eq!(resolver.github_slug("https://gitlab.com/acme/zeron").await, None);
        assert_eq!(runner.requests().len(), 1);
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
            "false:https://github.com/a/b/pull/1::".into(),
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
                "--input",
                "-"
            ]
        );
        let sent: serde_json::Value =
            serde_json::from_slice(requests[0].stdin.as_deref().unwrap()).unwrap();
        assert_eq!(sent, serde_json::json!({ "body": body }));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn process_runner_feeds_standard_input_and_closes_it() {
        let body = "x".repeat(100_000);
        let output = SystemProcessRunner
            .run(ProcessRequest {
                program: "cat".into(),
                args: Vec::new(),
                stdin: Some(body.clone().into_bytes()),
                cwd: None,
                env: Vec::new(),
                timeout: Duration::from_secs(5),
                output_limit: GITHUB_OUTPUT_LIMIT,
            })
            .await
            .unwrap();
        assert!(output.success);
        assert_eq!(output.stdout, body.as_bytes());
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
        let runner = FakeProcessRunner::with_responses([command_success(br#"{"number":12,"title":"A PR","body":"Description","author":null,"reviewDecision":null,"comments":[{"viewerDidAuthor":true,"author":{"login":"viewer"},"body":"Own comment"},{"author":{"login":"other"},"body":"Other comment"}],"statusCheckRollup":[{"name":"build","status":"IN_PROGRESS","conclusion":null}]}"#.to_vec()), command_failure("review threads unavailable")]);
        let detail = GitHubCli::with_runner(runner.clone())
            .detail("https://github.com/a/b/pull/12", false)
            .await
            .unwrap();
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
                .diff("https://github.com/a/b/pull/12", false)
                .await
                .unwrap(),
            "diff --git a/a b/a\n"
        );
        assert!(
            github
                .diff("https://github.com/a/b/pull/13", false)
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

    #[tokio::test]
    async fn board_metadata_is_batched_and_only_attests_to_the_current_head() {
        use zeron_proto::change_request_assessment::{CiState, Facts, MissingInformation};
        let head = "a".repeat(40);
        let nodes = (1..=50).map(|number| {
            let mut node = search_pull_request("acme/zeron", number, "A PR", "OPEN", "MERGEABLE",
                "2026-08-10T09:30:00Z", "2026-08-19T12:00:00Z", false, Some("REVIEW_REQUIRED"));
            node["headRefOid"] = head.clone().into();
            node["viewerDidAuthor"] = false.into();
            node["viewerLatestReviewRequest"] = serde_json::json!({"id": "latest"});
            node["reviewRequests"] = serde_json::json!({
                "nodes": [{"id": if number == 1 { "latest" } else { "other" }}],
                "pageInfo": {"hasNextPage": number == 2}
            });
            node["statusCheckRollup"] = match number {
                1 => serde_json::json!({"commit": {"oid": head}, "state": "SUCCESS", "contexts": {
                    "totalCount": 3, "checkRunCountsByState": [{"state": "CANCELLED", "count": 1}]
                }}),
                2 => serde_json::json!({"commit": {"oid": "b".repeat(40)}, "state": "SUCCESS", "contexts": {"totalCount": 3}}),
                3 => serde_json::Value::Null,
                4 => serde_json::json!({"commit": {"oid": head}, "state": "SUCCESS", "contexts": {
                    "totalCount": 3, "checkRunCountsByState": [{"state": "SKIPPED", "count": 3}]
                }}),
                _ => serde_json::json!({"commit": {"oid": head}, "state": "PENDING", "contexts": {"totalCount": 3}}),
            };
            if number == 5 { node.as_object_mut().unwrap().remove("statusCheckRollup"); }
            node
        }).collect();
        let runner = FakeProcessRunner::with_responses([command_success(search_response(nodes))]);
        let github = GitHubCli::with_runner(runner.clone());
        let page = github
            .list_page(
                "acme/zeron",
                zeron_proto::ChangeRequestFilter::All,
                None,
                false,
                false,
            )
            .await
            .unwrap();
        assert_eq!(page.items.len(), 50);
        assert_eq!(
            runner.requests().len(),
            1,
            "no per-PR detail or diff requests"
        );
        let query = &runner.requests()[0].args[3];
        assert!(query.contains("headRefOid"));
        assert!(!query.contains("body"));
        assert!(!query.contains("files"));
        assert!(!query.contains("diff"));
        let item = |number| {
            page.items
                .iter()
                .find(|item| item.number == number)
                .unwrap()
        };
        assert_eq!(item(1).head_ref_oid, head);
        assert_eq!(item(1).ci.state, CiState::Failed);
        assert_eq!(item(1).viewer_review_requested, Some(true));
        assert!(!Facts::from_item(item(1)).assess().attention.is_empty());
        assert_eq!(item(2).ci.state, CiState::Unknown);
        assert_eq!(
            item(2).viewer_review_requested,
            None,
            "unseen review requests remain unknown"
        );
        assert_eq!(item(3).ci.state, CiState::NoChecks);
        assert_eq!(
            item(3).viewer_review_requested,
            Some(false),
            "a completed request is not active"
        );
        assert_eq!(item(4).ci.state, CiState::Skipped);
        assert!(
            Facts::from_item(item(5))
                .assess()
                .missing
                .contains(&MissingInformation::Ci)
        );
    }

    async fn authored(
        github: &GitHubCli,
        repository: &str,
        refresh: bool,
    ) -> Result<Vec<ChangeRequestListItem>, ChangeRequestError> {
        github
            .list_page(
                repository,
                zeron_proto::ChangeRequestFilter::Authored,
                None,
                refresh,
                false,
            )
            .await
            .map(|page| page.items)
    }

    async fn list_with(
        response: Result<ProcessOutput, ProcessRunError>,
    ) -> (
        Result<Vec<ChangeRequestListItem>, ChangeRequestError>,
        Arc<FakeProcessRunner>,
    ) {
        let runner = FakeProcessRunner::with_responses([response]);
        let github = GitHubCli::with_runner(runner.clone());
        let result = authored(&github, "acme/zeron", false).await;
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
    async fn pr_pages_pass_the_cursor_and_report_the_next_one_and_total() {
        let item = |number| {
            search_pull_request(
                "acme/zeron",
                number,
                "PR",
                "OPEN",
                "UNKNOWN",
                "2026-08-10T09:30:00Z",
                "2026-08-19T12:00:00Z",
                false,
                None,
            )
        };
        let page = |items, next: Option<&str>| {
            serde_json::to_vec(&serde_json::json!({"data": {"search": {
                "issueCount": 120,
                "pageInfo": {"hasNextPage": next.is_some(), "endCursor": next},
                "nodes": items,
            }}}))
            .unwrap()
        };
        let runner = FakeProcessRunner::with_responses([
            command_success(page(vec![item(1)], Some("Y3Vyc29yOjUw"))),
            command_success(page(vec![item(2)], None)),
        ]);
        let github = GitHubCli::with_runner(runner.clone());
        let filter = zeron_proto::ChangeRequestFilter::All;
        let first = github
            .list_page("acme/zeron", filter, None, false, false)
            .await
            .unwrap();
        assert_eq!(first.next_cursor.as_deref(), Some("Y3Vyc29yOjUw"));
        assert_eq!(first.total_count, Some(120));
        let second = github
            .list_page("acme/zeron", filter, first.next_cursor.as_deref(), false, false)
            .await
            .unwrap();
        assert_eq!(second.items[0].number, 2);
        assert_eq!(second.next_cursor, None, "the last page has no cursor");
        let requests = runner.requests();
        assert!(!requests[0].args.iter().any(|arg| arg.starts_with("after=")));
        assert!(
            requests[1]
                .args
                .iter()
                .any(|arg| arg == "after=Y3Vyc29yOjUw")
        );
        assert_eq!(
            github
                .list_page("acme/zeron", filter, Some("x repo:other/x"), false, false)
                .await,
            Err(ChangeRequestError::Decode),
            "cursors cannot smuggle search qualifiers"
        );
        assert_eq!(runner.requests().len(), 2);
    }

    #[tokio::test]
    async fn pr_first_page_is_stored_on_disk_and_served_when_cached() {
        let page = |title: &str| {
            command_success(search_response(vec![search_pull_request(
                "acme/zeron",
                1,
                title,
                "OPEN",
                "UNKNOWN",
                "2026-08-10T09:30:00Z",
                "2026-08-19T12:00:00Z",
                false,
                None,
            )]))
        };
        let dir = tempfile::tempdir().unwrap();
        let runner = FakeProcessRunner::with_responses([
            command_success("me\n"),
            page("first"),
            page("refreshed"),
        ]);
        let github = GitHubCli::with_runner(runner.clone()).with_store(dir.path());
        let filter = zeron_proto::ChangeRequestFilter::All;
        // Nothing stored yet: a cached read goes to GitHub and stores it.
        let first = github
            .list_page("acme/zeron", filter, None, false, true)
            .await
            .unwrap();
        assert_eq!(first.items[0].title, "first");
        assert!(first.fetched_at.is_some());
        // A fresh engine (a relaunch) serves the stored page without GitHub.
        let relaunched = GitHubCli::with_runner(runner.clone()).with_store(dir.path());
        *relaunched.account.lock().await = Some((Instant::now(), Some("me".into())));
        let stored = relaunched
            .list_page("acme/zeron", filter, None, false, true)
            .await
            .unwrap();
        assert_eq!(stored.items[0].title, "first");
        // The store keeps fetch times to the millisecond.
        assert_eq!(
            stored.fetched_at.map(|at| at.timestamp_millis()),
            first.fetched_at.map(|at| at.timestamp_millis())
        );
        // Refresh always reaches GitHub and replaces the stored page.
        let refreshed = relaunched
            .list_page("acme/zeron", filter, None, true, false)
            .await
            .unwrap();
        assert_eq!(refreshed.items[0].title, "refreshed");
        let stored = relaunched
            .list_page("acme/zeron", filter, None, false, true)
            .await
            .unwrap();
        assert_eq!(stored.items[0].title, "refreshed");
        let requests = runner.requests();
        assert_eq!(requests.len(), 3, "one account read and two searches");
        assert_eq!(
            requests[0].args,
            ["config", "get", "user", "-h", "github.com"]
        );
        // Another account never sees it.
        assert!(
            relaunched
                .store
                .as_ref()
                .unwrap()
                .load("other", "acme/zeron", filter)
                .await
                .is_none()
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
            let items = authored(&github, "acme/old-name", false).await.unwrap();
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
                .list_page("acme/zeron", filter, None, false, false)
                .await
                .unwrap();
            assert_eq!(runner.requests().len(), index + 1);
            github
                .list_page("ACME/ZERON", filter, None, false, false)
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
            .list_page("acme/zeron", All, None, false, false)
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
            authored(&github, "acme/zeron", false),
            authored(&github, "ACME/ZERON", false)
        );
        assert!(a.unwrap().is_empty() && b.unwrap().is_empty());
        authored(&github, "acme/zeron", true).await.unwrap();
        assert_eq!(
            runner.requests().len(),
            1,
            "concurrent and immediate refresh calls reuse one response"
        );
        github.pr_cache.lock().await.entries[0].1 = Instant::now() - Duration::from_secs(16);
        authored(&github, "acme/zeron", true).await.unwrap();
        assert_eq!(runner.requests().len(), 2);
        github.pr_cache.lock().await.entries[0].1 = Instant::now() - Duration::from_secs(301);
        authored(&github, "acme/zeron", false).await.unwrap();
        assert_eq!(runner.requests().len(), 3);
    }

    /// Answers only once `parties` requests are running at the same time.
    struct GatedProcessRunner {
        gate: tokio::sync::Barrier,
        runs: std::sync::atomic::AtomicUsize,
    }

    #[async_trait]
    impl ProcessRunner for GatedProcessRunner {
        async fn run(&self, request: ProcessRequest) -> Result<ProcessOutput, ProcessRunError> {
            self.runs.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.gate.wait().await;
            if request.args[0] == "pr" {
                command_success("diff --git a/a b/a\n")
            } else {
                command_success(search_response(vec![]))
            }
        }
    }

    #[tokio::test]
    async fn pr_distinct_reads_run_side_by_side_and_identical_reads_share_one_call() {
        let runner = Arc::new(GatedProcessRunner {
            gate: tokio::sync::Barrier::new(2),
            runs: Default::default(),
        });
        let github = GitHubCli::with_runner(runner.clone());
        let (diff, same_diff, list) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(
                github.diff("https://github.com/a/b/pull/1", false),
                github.diff("https://github.com/a/b/pull/1", false),
                authored(&github, "a/b", false),
            )
        })
        .await
        .expect("a slow read must not hold up a different one");
        assert_eq!(diff.unwrap(), "diff --git a/a b/a\n");
        assert_eq!(same_diff.unwrap(), "diff --git a/a b/a\n");
        assert!(list.unwrap().is_empty());
        assert_eq!(runner.runs.load(std::sync::atomic::Ordering::SeqCst), 2);
        let cache = github.pr_cache.lock().await;
        assert_eq!(cache.entries.len(), 2);
        assert!(cache.in_flight.is_empty());
    }

    #[tokio::test]
    async fn pr_read_finishes_and_is_cached_after_its_only_caller_leaves() {
        let runner = Arc::new(GatedProcessRunner {
            gate: tokio::sync::Barrier::new(2),
            runs: Default::default(),
        });
        let github = GitHubCli::with_runner(runner.clone());
        let url = "https://github.com/a/b/pull/1";
        let caller = tokio::spawn({
            let github = github.clone();
            async move { github.diff(url, false).await }
        });
        while runner.runs.load(std::sync::atomic::Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        // GitHub answers once nobody is waiting any more.
        runner.gate.wait().await;
        let diff = tokio::time::timeout(Duration::from_secs(5), github.diff(url, true))
            .await
            .expect("an abandoned read must not strand later ones");
        assert_eq!(diff.unwrap(), "diff --git a/a b/a\n");
        assert_eq!(runner.runs.load(std::sync::atomic::Ordering::SeqCst), 1);
        let cache = github.pr_cache.lock().await;
        assert_eq!(cache.entries.len(), 1);
        assert!(cache.in_flight.is_empty());
    }

    #[tokio::test]
    async fn pr_refresh_retries_a_cached_failure() {
        let runner = FakeProcessRunner::with_responses([
            command_failure("request failed"),
            command_success(search_response(vec![])),
        ]);
        let github = GitHubCli::with_runner(runner.clone());
        for _ in 0..2 {
            assert_eq!(
                authored(&github, "a/b", false).await,
                Err(ChangeRequestError::CommandFailed)
            );
        }
        assert_eq!(runner.requests().len(), 1, "a failure is held briefly");
        assert!(authored(&github, "a/b", true).await.unwrap().is_empty());
        assert_eq!(runner.requests().len(), 2);
    }

    #[tokio::test]
    async fn pr_rate_limit_cooldown_covers_other_repositories_and_details() {
        let runner =
            FakeProcessRunner::with_responses([command_failure("API rate limit exceeded")]);
        let github = GitHubCli::with_runner(runner.clone());
        assert_eq!(
            authored(&github, "a/one", false).await,
            Err(ChangeRequestError::RateLimited)
        );
        assert_eq!(
            authored(&github, "a/two", true).await,
            Err(ChangeRequestError::RateLimited)
        );
        assert_eq!(
            github.diff("https://github.com/a/one/pull/1", true).await,
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
                authored(&github, invalid, false).await,
                Err(ChangeRequestError::UnsupportedRepository)
            );
        }
        assert!(runner.requests().is_empty());
        for index in 0..26 {
            authored(&github, &format!("a/repo-{index}"), false)
                .await
                .unwrap();
        }
        let cache = github.pr_cache.lock().await;
        assert_eq!(cache.entries.len(), 24);
        assert!(
            cache
                .entries
                .iter()
                .all(|entry| entry.0 != "list:a/repo-0:Authored:")
        );
        assert!(
            cache
                .entries
                .iter()
                .any(|entry| entry.0 == "list:a/repo-25:Authored:")
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
    async fn github_search_skips_results_the_viewer_cannot_read() {
        let json = serde_json::to_vec(&serde_json::json!({"data": {"search": {"nodes": [
            null,
            search_pull_request(
                "acme/zeron",
                3,
                "Readable",
                "OPEN",
                "MERGEABLE",
                "2026-08-01T08:00:00Z",
                "2026-08-19T12:00:00Z",
                false,
                None,
            ),
        ]}}}))
        .unwrap();
        let (result, _) = list_with(command_success(json)).await;
        let items = result.unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].number, 3);
    }

    #[tokio::test]
    async fn github_search_keeps_readable_results_when_gh_reports_partial_errors() {
        let readable = search_pull_request(
            "acme/zeron",
            3,
            "Readable",
            "OPEN",
            "MERGEABLE",
            "2026-08-01T08:00:00Z",
            "2026-08-19T12:00:00Z",
            false,
            None,
        );
        let partial = |repository: serde_json::Value| {
            Ok(ProcessOutput {
                success: false,
                stdout: serde_json::to_vec(&serde_json::json!({
                    "data": {"repository": repository, "search": {"nodes": [null, readable]}},
                    "errors": [{"type": "FORBIDDEN"}],
                }))
                .unwrap(),
                stderr: b"gh: Resource protected by organization SAML enforcement".to_vec(),
                stdout_truncated: false,
            })
        };
        let (result, _) = list_with(partial(serde_json::json!({
            "nameWithOwner": "acme/zeron"
        })))
        .await;
        assert_eq!(result.unwrap()[0].number, 3);
        let (result, _) = list_with(partial(serde_json::Value::Null)).await;
        assert_eq!(
            result.unwrap_err(),
            ChangeRequestError::CommandFailed,
            "an unresolved repository is still a failure"
        );
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
            executable: fake_gh.clone(),
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

        // A worktree created in a user-chosen folder must still resolve its
        // repository, branch and PR through the same host-side badge path.
        run_git(
            &checkout,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "--allow-empty",
                "-m",
                "initial",
            ],
        );
        let repos = crate::Repos::with_worktrees_root(
            &temp.path().join("settings"),
            "test-device",
            temp.path().join("default-worktrees"),
        );
        let custom_root = temp.path().join("other disk/worktrees");
        repos
            .set_worktree_settings(zeron_proto::WorktreeSettings {
                use_custom_directory: true,
                custom_directory: Some(custom_root.to_string_lossy().into_owned()),
            })
            .await
            .unwrap();
        let worktree = repos.create_worktree(&checkout, "HEAD").await.unwrap();
        assert!(Path::new(&worktree.path).starts_with(custom_root.canonicalize().unwrap()));
        std::fs::write(
            &fake_gh,
            fake_gh_contents.replace("feature/status", &worktree.branch),
        )
        .unwrap();
        let resolution = resolver
            .resolve_github(Path::new(&worktree.path))
            .await
            .unwrap();
        assert_eq!(
            resolution.source.checkout_root.canonicalize().unwrap(),
            Path::new(&worktree.path).canonicalize().unwrap()
        );
        assert_eq!(resolution.source.branch.local_branch, worktree.branch);
        assert_eq!(resolution.source.branch.owner.as_deref(), Some("acme"));
        let badge = resolution.change_request.unwrap();
        assert_eq!(badge.number, 90);
        assert_eq!(badge.state, ChangeRequestState::Open);
        assert_eq!(badge.head_ref, worktree.branch);
        let identity = repos
            .checkout_identity(Path::new(&worktree.path))
            .await
            .unwrap();
        repos
            .set_worktree_settings(zeron_proto::WorktreeSettings::default())
            .await
            .unwrap();
        let default_worktree = repos.create_worktree(&checkout, "HEAD").await.unwrap();
        assert!(
            Path::new(&default_worktree.path).starts_with(temp.path().join("default-worktrees"))
        );
        assert_eq!(
            repos
                .checkout_identity(Path::new(&worktree.path))
                .await
                .unwrap(),
            identity
        );
        let refs = repos.refs(&checkout).await.unwrap();
        for path in [&worktree.path, &default_worktree.path] {
            // Git reports slash-separated paths on Windows, while settings
            // retain canonical Win32 prefixes. Compare the actual directory.
            assert!(
                refs.iter().any(|entry| entry
                    .worktree_path
                    .as_ref()
                    .is_some_and(|listed| same_file::is_same_file(listed, path).unwrap_or(false))),
                "Git refs must include worktree {path}; got {refs:?}"
            );
        }
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
