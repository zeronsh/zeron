//! The board's GitLab adapter, backed by the host's `glab` installation and
//! its sign-in. Merge requests are listed one project at a time through
//! GraphQL; their detail and comments open in the browser for now.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use zeron_proto::change_request_assessment::{ChangeRequestCi, CiState};
use zeron_proto::{
    ChangeRequestActor, ChangeRequestComment, ChangeRequestFeatures, ChangeRequestFilter,
    ChangeRequestListItem, ChangeRequestMergeability, ChangeRequestPage, ChangeRequestPresentation,
    ChangeRequestReviewDecision, ChangeRequestScope, ChangeRequestState, ChangeRequestSummary,
};

use super::{
    CLI_OUTPUT_LIMIT, CLI_TIMEOUT, ChangeRequestError, CheckoutSourceContext, GITHUB_HOST,
    ProcessRequest, ProcessRunner, SystemProcessRunner, classify_run_error,
};
use crate::change_request_board::{ChangeRequestBoardProvider, valid_page_cursor};

const GITLAB_PROVIDER: &str = "gitlab";
const GITLAB_HOST: &str = "gitlab.com";

/// `currentUser` rides along so each item can say whether it is the
/// viewer's. Names come from variables, never from the query text.
const GITLAB_LIST_QUERY: &str = "query($fullPath: ID!, $after: String, $author: String, $reviewer: String) { currentUser { username } project(fullPath: $fullPath) { fullPath mergeRequests(state: opened, sort: UPDATED_DESC, first: 50, after: $after, authorUsername: $author, reviewerUsername: $reviewer) { count pageInfo { hasNextPage endCursor } nodes { iid title draft webUrl createdAt updatedAt author { username avatarUrl } diffStatsSummary { additions deletions } diffHeadSha headPipeline { status sha } conflicts mergeStatusEnum approved approvalsLeft reviewers { nodes { username mergeRequestInteraction { reviewState } } } } } } }";
const GITLAB_PROJECT_QUERY: &str =
    "query($fullPath: ID!) { project(fullPath: $fullPath) { fullPath } }";
const GITLAB_VIEWER_QUERY: &str = "query { currentUser { username } }";
/// Merge requests from one source branch, in any state. The default branch
/// rides along so a historical one on it is no answer.
const GITLAB_BRANCH_QUERY: &str = "query($fullPath: ID!, $branch: String!) { currentUser { username } project(fullPath: $fullPath) { fullPath repository { rootRef } mergeRequests(sourceBranches: [$branch], sort: UPDATED_DESC, first: 20) { nodes { iid title webUrl state sourceBranch targetBranch updatedAt sourceProject { fullPath } } } } }";
/// Tokens `glab` prefers over its stored sign-in, which may belong to
/// another account than the config's `user`.
const GITLAB_TOKEN_VARIABLES: [&str; 3] = ["GITLAB_TOKEN", "GITLAB_ACCESS_TOKEN", "OAUTH_TOKEN"];

/// GitLab provider backed by `glab`. Hosts are gitlab.com and any host
/// `glab` is signed in to; every call names its host with `--hostname`.
#[derive(Clone)]
pub(crate) struct GitLabCli {
    runner: Arc<dyn ProcessRunner>,
    /// Self-managed hosts from `glab`'s config, read once per engine.
    hosts: Arc<OnceLock<Vec<String>>>,
    /// The configured username per host and when it was read.
    users: Arc<tokio::sync::Mutex<HashMap<String, UserRead>>>,
    /// Whoever the token in use belongs to per host, asked of GitLab when
    /// the config names nobody.
    viewers: Arc<tokio::sync::Mutex<HashMap<String, UserRead>>>,
}

/// A username, if any, and when it was read.
type UserRead = (Instant, Option<String>);

impl GitLabCli {
    pub(crate) fn new() -> Self {
        let gitlab = Self::with_runner(Arc::new(SystemProcessRunner));
        // Tests never read the user's own glab configuration.
        if cfg!(test) {
            gitlab
        } else {
            Self {
                hosts: Default::default(),
                ..gitlab
            }
        }
    }

    fn with_runner(runner: Arc<dyn ProcessRunner>) -> Self {
        Self {
            runner,
            hosts: Arc::new(OnceLock::from(Vec::new())),
            users: Default::default(),
            viewers: Default::default(),
        }
    }

    /// Serve these self-managed hosts, as if `glab` were signed in to them.
    #[cfg(test)]
    fn with_hosts(mut self, hosts: &[&str]) -> Self {
        self.hosts = Arc::new(OnceLock::from(
            hosts
                .iter()
                .map(|host| host.to_string())
                .collect::<Vec<_>>(),
        ));
        self
    }

    fn configured_hosts(&self) -> &[String] {
        self.hosts.get_or_init(|| {
            glab_config_files()
                .into_iter()
                .filter_map(|path| std::fs::read_to_string(path).ok())
                .flat_map(|config| config_hosts(&config))
                .collect()
        })
    }

    /// `scope` when this adapter may pass it to `glab`, checked again
    /// whatever the caller did.
    fn served(&self, scope: &ChangeRequestScope) -> Option<ChangeRequestScope> {
        self.serves_host(scope.host())
            .then(|| self.scope(scope.host(), scope.path()))
            .flatten()
    }

    /// The account in `glab`'s config for `host`, read locally and re-read
    /// every 30 seconds so a new `glab auth login` takes effect.
    async fn configured_user(&self, host: &str) -> Option<String> {
        let mut users = self.users.lock().await;
        if let Some((read_at, user)) = users.get(host)
            && read_at.elapsed() < Duration::from_secs(30)
        {
            return user.clone();
        }
        let user = self
            .runner
            .run(ProcessRequest {
                args: vec![
                    "config".into(),
                    "get".into(),
                    "user".into(),
                    "--host".into(),
                    host.into(),
                ],
                timeout: Duration::from_secs(5),
                ..gitlab_request()
            })
            .await
            .ok()
            .filter(|output| output.success && !output.stdout_truncated)
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .map(|user| user.trim().to_owned())
            .filter(|user| valid_username(user));
        users.insert(host.to_owned(), (Instant::now(), user.clone()));
        user
    }

    /// Who the board's filters mean by "me" on `host`: the configured
    /// account, else whoever the token in use belongs to, kept for five
    /// minutes once known.
    async fn viewer(&self, host: &str) -> Option<String> {
        if !token_in_environment()
            && let Some(user) = self.configured_user(host).await
        {
            return Some(user);
        }
        let mut viewers = self.viewers.lock().await;
        if let Some((read_at, Some(user))) = viewers.get(host)
            && read_at.elapsed() < Duration::from_secs(5 * 60)
        {
            return Some(user.clone());
        }
        let (data, _) = self
            .graphql::<GlViewerData>(host, GITLAB_VIEWER_QUERY, Vec::new(), |_| false)
            .await
            .ok()?;
        let user = data
            .current_user
            .map(|user| user.username)
            .filter(|user| valid_username(user))?;
        viewers.insert(host.to_owned(), (Instant::now(), Some(user.clone())));
        Some(user)
    }

    /// Run one GraphQL query on `host` with string `variables`. Like `gh`,
    /// `glab` exits non-zero whenever the response carries `errors`; data
    /// that `usable` accepts is still answered, marked partial.
    async fn graphql<T: DeserializeOwned>(
        &self,
        host: &str,
        query: &str,
        variables: Vec<(&str, String)>,
        usable: fn(&T) -> bool,
    ) -> Result<(T, bool), ChangeRequestError> {
        if !self.serves_host(host) {
            return Err(ChangeRequestError::UnsupportedRepository);
        }
        let mut args = vec![
            "api".into(),
            "graphql".into(),
            "--hostname".into(),
            host.into(),
            "-f".into(),
            format!("query={query}"),
        ];
        for (name, value) in variables {
            args.extend(["-f".into(), format!("{name}={value}")]);
        }
        let output = self
            .runner
            .run(ProcessRequest {
                args,
                ..gitlab_request()
            })
            .await
            .map_err(classify_run_error)?;
        let response = (!output.stdout_truncated)
            .then(|| serde_json::from_slice::<GlResponse<T>>(&output.stdout).ok())
            .flatten();
        match response {
            Some(GlResponse {
                data: Some(data),
                errors,
            }) if output.success || usable(&data) => Ok((data, !errors.is_empty())),
            _ if !output.success => Err(classify_gitlab_failure(&output.stderr)),
            _ => Err(ChangeRequestError::Decode),
        }
    }
}

/// Board requests name their host, never prompt, and find `glab` where the
/// user's login shell would.
fn gitlab_request() -> ProcessRequest {
    ProcessRequest {
        program: "glab".into(),
        args: Vec::new(),
        stdin: None,
        cwd: None,
        env: vec![("GLAB_NO_PROMPT".into(), "true".into())],
        timeout: CLI_TIMEOUT,
        output_limit: CLI_OUTPUT_LIMIT,
        login_shell: true,
    }
}

fn token_in_environment() -> bool {
    GITLAB_TOKEN_VARIABLES
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
}

fn valid_username(user: &str) -> bool {
    (1..=255).contains(&user.len())
        && user
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

/// A branch name safe to send as a variable: Git's own rules forbid
/// whitespace and control characters anywhere in one.
fn valid_branch(branch: &str) -> bool {
    (1..=255).contains(&branch.len())
        && !branch.starts_with('-')
        && !branch.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// Where `glab` may keep its config: `GLAB_CONFIG_DIR`, the platform's
/// config directory, and the older `~/.config`.
fn glab_config_files() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(dir) = std::env::var_os("GLAB_CONFIG_DIR").filter(|dir| !dir.is_empty()) {
        dirs.push(PathBuf::from(dir));
    }
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME").filter(|dir| !dir.is_empty()) {
        dirs.push(PathBuf::from(dir).join("glab-cli"));
    }
    let home = crate::repos::home_dir();
    #[cfg(target_os = "macos")]
    dirs.push(home.join("Library/Application Support/glab-cli"));
    #[cfg(windows)]
    if let Some(dir) = std::env::var_os("APPDATA").filter(|dir| !dir.is_empty()) {
        dirs.push(PathBuf::from(dir).join("glab-cli"));
    }
    dirs.push(home.join(".config/glab-cli"));
    dirs.into_iter().map(|dir| dir.join("config.yml")).collect()
}

/// The keys of the top-level `hosts:` map in `glab`'s YAML config, read by
/// indentation alone; anything unexpected is skipped. GitHub's host stays
/// GitHub's.
fn config_hosts(config: &str) -> Vec<String> {
    let mut hosts = Vec::new();
    let mut in_hosts = false;
    let mut indent = None;
    for line in config.lines() {
        let content = line.trim_start();
        if content.is_empty() || content.starts_with('#') {
            continue;
        }
        let depth = line.len() - content.len();
        if depth == 0 {
            in_hosts = content.trim_end() == "hosts:";
            indent = None;
            continue;
        }
        if in_hosts
            && depth == *indent.get_or_insert(depth)
            && let Some(host) = content.trim_end().strip_suffix(':')
        {
            let host = host.trim_matches(['"', '\'']).to_ascii_lowercase();
            // A host the board can scope a repository on.
            let scopable = ChangeRequestScope::new(&host, "group/project").is_some();
            if scopable && host != GITHUB_HOST && !hosts.contains(&host) {
                hosts.push(host);
            }
        }
    }
    hosts
}

/// `glab` prints its own sign-in hints; never expose them, only classify.
fn classify_gitlab_failure(stderr: &[u8]) -> ChangeRequestError {
    let stderr = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    if stderr.contains("rate limit") || stderr.contains("too many requests") {
        ChangeRequestError::RateLimited
    } else if stderr.contains("unauthenticated")
        || stderr.contains("unauthorized")
        || stderr.contains("invalid token")
        || stderr.contains("token is expired")
        || stderr.contains("not logged")
        || stderr.contains("glab auth login")
        || stderr.contains("authentication")
    {
        ChangeRequestError::Authentication
    } else {
        ChangeRequestError::CommandFailed
    }
}

/// A web URL GitLab sent, when it is one.
fn web_url(raw: &str) -> Result<reqwest::Url, ChangeRequestError> {
    let url = reqwest::Url::parse(raw.trim()).map_err(|_| ChangeRequestError::Decode)?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(ChangeRequestError::Decode);
    }
    Ok(url)
}

/// An avatar as GitLab sends it: absolute, or (for uploads on the instance)
/// a path on the host that served `page`.
fn avatar_url(raw: Option<&str>, page: &reqwest::Url) -> Option<String> {
    let raw = raw.map(str::trim).filter(|raw| !raw.is_empty())?;
    let url = page.join(raw).ok()?;
    (matches!(url.scheme(), "http" | "https") && url.host_str().is_some()).then(|| url.to_string())
}

/// How a merge request fills the board's provider-neutral row, so the same
/// assessment groups it as it would a pull request:
///
/// - `iid` is the number, shown as `!iid`; `webUrl` the URL; `draft` makes
///   it a draft; the project's full path, qualified by host, its repository.
/// - CI is the head pipeline, and only when it ran on `diffHeadSha`: a
///   pipeline for an older commit says nothing about the current one, as
///   with GitHub's rollup. `SUCCESS` passed; `FAILED` and `CANCELED` failed;
///   created, waiting, preparing, pending, running, scheduled and manual
///   pipelines are pending; `SKIPPED` skipped; no pipeline is no checks.
/// - Conflicts, or `CANNOT_BE_MERGED`, conflict; `CAN_BE_MERGED` is
///   mergeable; anything else, such as a check GitLab hasn't run yet, is
///   unknown.
/// - Review: any reviewer requesting changes, else approved, else review
///   required while approvals are left or a reviewer hasn't reviewed, else
///   no decision.
/// - The viewer authored it when they are its author, and their review is
///   requested while they are a reviewer who hasn't reviewed yet, their
///   own merge request included: asking for one's own review is deliberate.
fn to_list_item(
    request: GlMergeRequest,
    scope: &ChangeRequestScope,
    viewer: Option<&str>,
) -> Result<ChangeRequestListItem, ChangeRequestError> {
    let number = request
        .iid
        .parse::<u64>()
        .ok()
        .filter(|number| *number > 0)
        .ok_or(ChangeRequestError::Decode)?;
    let title = request
        .title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if title.is_empty() {
        return Err(ChangeRequestError::Decode);
    }
    let url = web_url(&request.web_url)?;
    let head = request.diff_head_sha.unwrap_or_default();
    let ci = match &request.head_pipeline {
        None => ChangeRequestCi {
            state: CiState::NoChecks,
            total_count: 0,
        },
        Some(pipeline) => ChangeRequestCi {
            state: if pipeline.sha.as_deref() == Some(head.as_str()) {
                pipeline_state(&pipeline.status)
            } else {
                CiState::Unknown
            },
            total_count: 1,
        },
    };
    let reviews: Vec<(&str, &str)> = request
        .reviewers
        .iter()
        .flat_map(|reviewers| reviewers.nodes.iter().flatten())
        .map(|reviewer| {
            (
                reviewer.username.as_str(),
                reviewer
                    .merge_request_interaction
                    .as_ref()
                    .and_then(|interaction| interaction.review_state.as_deref())
                    .unwrap_or_default(),
            )
        })
        .collect();
    let review_decision = if reviews
        .iter()
        .any(|(_, state)| *state == "REQUESTED_CHANGES")
    {
        ChangeRequestReviewDecision::ChangesRequested
    } else if request.approved == Some(true) {
        ChangeRequestReviewDecision::Approved
    } else if request.approvals_left.is_some_and(|left| left > 0)
        || reviews.iter().any(|(_, state)| awaiting_review(state))
    {
        ChangeRequestReviewDecision::ReviewRequired
    } else {
        ChangeRequestReviewDecision::Unknown
    };
    let mergeability = match request.merge_status_enum.as_deref() {
        _ if request.conflicts == Some(true) => ChangeRequestMergeability::Conflicting,
        Some("CANNOT_BE_MERGED") => ChangeRequestMergeability::Conflicting,
        Some("CAN_BE_MERGED") => ChangeRequestMergeability::Mergeable,
        _ => ChangeRequestMergeability::Unknown,
    };
    let author = request.author.unwrap_or_default();
    let (additions, deletions) = request
        .diff_stats_summary
        .map_or((0, 0), |stats| (stats.additions, stats.deletions));
    Ok(ChangeRequestListItem {
        provider: GITLAB_PROVIDER.into(),
        viewer_did_author: viewer.map(|viewer| author.username.eq_ignore_ascii_case(viewer)),
        viewer_review_requested: viewer.map(|viewer| {
            reviews
                .iter()
                .any(|(user, state)| user.eq_ignore_ascii_case(viewer) && awaiting_review(state))
        }),
        author: ChangeRequestActor {
            avatar_url: avatar_url(author.avatar_url.as_deref(), &url),
            login: author.username,
        },
        head_ref_oid: head,
        ci,
        repository: scope.to_string(),
        number,
        title,
        url: url.to_string(),
        state: ChangeRequestState::Open,
        is_draft: request.draft,
        review_decision,
        additions,
        deletions,
        mergeability,
        created_at: request.created_at,
        updated_at: request.updated_at,
        reference: None,
    })
}

fn pipeline_state(status: &str) -> CiState {
    match status {
        "SUCCESS" => CiState::Passed,
        "FAILED" | "CANCELED" | "CANCELING" => CiState::Failed,
        "CREATED"
        | "WAITING_FOR_RESOURCE"
        | "WAITING_FOR_CALLBACK"
        | "PREPARING"
        | "PENDING"
        | "RUNNING"
        | "SCHEDULED"
        | "MANUAL" => CiState::Pending,
        "SKIPPED" => CiState::Skipped,
        _ => CiState::Unknown,
    }
}

/// A reviewer who hasn't reviewed yet, or whose approval was withdrawn.
fn awaiting_review(state: &str) -> bool {
    matches!(state, "UNREVIEWED" | "REVIEW_STARTED" | "UNAPPROVED")
}

/// The branch's merge request among `candidates`: from the project itself
/// (not a fork's same-named branch), open ones first, then the newest. On
/// the default branch only an open one counts.
fn select_merge_request(
    project: &str,
    branch: &str,
    default_branch: Option<&str>,
    candidates: Vec<GlBranchMergeRequest>,
) -> Result<Option<ChangeRequestSummary>, ChangeRequestError> {
    let on_default_branch = default_branch == Some(branch);
    let mut matching: Vec<(ChangeRequestState, GlBranchMergeRequest)> = candidates
        .into_iter()
        .filter(|candidate| candidate.source_branch == branch)
        .filter(|candidate| {
            candidate
                .source_project
                .as_ref()
                .is_some_and(|source| source.full_path.eq_ignore_ascii_case(project))
        })
        .filter_map(|candidate| Some((merge_request_state(&candidate.state)?, candidate)))
        .filter(|(state, _)| !on_default_branch || *state == ChangeRequestState::Open)
        .collect();
    matching.sort_by(|(left_state, left), (right_state, right)| {
        let left_open = *left_state == ChangeRequestState::Open;
        let right_open = *right_state == ChangeRequestState::Open;
        right_open
            .cmp(&left_open)
            .then_with(|| right.updated_at.cmp(&left.updated_at))
    });
    let Some((state, request)) = matching.into_iter().next() else {
        return Ok(None);
    };
    let number = request
        .iid
        .parse::<u64>()
        .ok()
        .filter(|number| *number > 0)
        .ok_or(ChangeRequestError::Decode)?;
    let title = request.title.trim();
    if title.is_empty() || request.target_branch.trim().is_empty() {
        return Err(ChangeRequestError::Decode);
    }
    Ok(Some(ChangeRequestSummary {
        provider: GITLAB_PROVIDER.into(),
        number,
        title: title.to_owned(),
        url: web_url(&request.web_url)?.to_string(),
        state,
        base_ref: request.target_branch,
        head_ref: request.source_branch,
        reference: Some(format!("!{number}")),
        features: None,
    }))
}

fn merge_request_state(state: &str) -> Option<ChangeRequestState> {
    match state {
        "opened" => Some(ChangeRequestState::Open),
        "merged" => Some(ChangeRequestState::Merged),
        "closed" | "locked" => Some(ChangeRequestState::Closed),
        _ => None,
    }
}

#[async_trait]
impl ChangeRequestBoardProvider for GitLabCli {
    fn provider_id(&self) -> &'static str {
        GITLAB_PROVIDER
    }

    fn serves_host(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        host != GITHUB_HOST && (host == GITLAB_HOST || self.configured_hosts().contains(&host))
    }

    /// Projects nest under groups (`group/subgroup/project`). `-` separates
    /// a project from its pages in GitLab's URLs, so it names none.
    fn scope(&self, host: &str, path: &str) -> Option<ChangeRequestScope> {
        path.split('/')
            .all(|segment| segment != "-")
            .then(|| ChangeRequestScope::new(host, path))
            .flatten()
    }

    /// `https://<host>/<group>/.../<project>/-/merge_requests/<iid>`.
    fn change_request_url(&self, url: &str) -> Option<String> {
        let url = reqwest::Url::parse(url).ok()?;
        if url.scheme() != "https"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some()
        {
            return None;
        }
        let host = url.host_str()?.to_ascii_lowercase();
        if !self.serves_host(&host) {
            return None;
        }
        let (path, iid) = url
            .path()
            .trim_matches('/')
            .split_once("/-/merge_requests/")?;
        let iid = iid.parse::<u64>().ok().filter(|iid| *iid > 0)?;
        let scope = self.scope(&host, path)?;
        Some(format!(
            "https://{}/{}/-/merge_requests/{iid}",
            scope.host(),
            scope.path()
        ))
    }

    fn presentation(&self, host: &str) -> ChangeRequestPresentation {
        let host = if host.is_empty() { GITLAB_HOST } else { host };
        ChangeRequestPresentation {
            provider_name: "GitLab".into(),
            cli_name: "GitLab CLI".into(),
            cli_command: "glab".into(),
            sign_in_command: format!("glab auth login --hostname {host}"),
            noun: "merge request".into(),
            media_hosts: vec![host.into()],
            media_rewrites: Vec::new(),
            // Merge requests open in the browser.
            features: ChangeRequestFeatures {
                native_detail: false,
                comments: false,
                handoff: false,
            },
        }
    }

    /// `!iid`. Avatars come with the page, so stored rows keep theirs.
    fn present(&self, item: &mut ChangeRequestListItem) {
        item.reference = Some(format!("!{}", item.number));
    }

    /// `gitlab:<host>:<username>`, from `glab`'s config. A token in the
    /// environment may belong to anyone, so then nothing is stored.
    async fn account_key(&self, host: &str) -> Option<String> {
        if token_in_environment() || !self.serves_host(host) {
            return None;
        }
        let host = host.to_ascii_lowercase();
        let user = self.configured_user(&host).await?;
        Some(format!("{GITLAB_PROVIDER}:{host}:{user}"))
    }

    /// GitLab's own path for the project, through `project(fullPath:)`.
    async fn canonical_scope(
        &self,
        scope: &ChangeRequestScope,
    ) -> Result<Option<ChangeRequestScope>, ChangeRequestError> {
        let scope = self
            .served(scope)
            .ok_or(ChangeRequestError::UnsupportedRepository)?;
        let (data, _) = self
            .graphql::<GlProjectData>(
                scope.host(),
                GITLAB_PROJECT_QUERY,
                vec![("fullPath", scope.path().to_owned())],
                |_| false,
            )
            .await?;
        Ok(data
            .project
            .and_then(|project| self.scope(scope.host(), &project.full_path)))
    }

    /// One page of open merge requests, 50 at a time, newest updates first.
    async fn list_page(
        &self,
        scope: &ChangeRequestScope,
        filter: ChangeRequestFilter,
        after: Option<&str>,
    ) -> Result<ChangeRequestPage, ChangeRequestError> {
        let scope = self
            .served(scope)
            .ok_or(ChangeRequestError::UnsupportedRepository)?;
        if after.is_some_and(|cursor| !valid_page_cursor(cursor)) {
            return Err(ChangeRequestError::Decode);
        }
        let mut variables = vec![("fullPath", scope.path().to_owned())];
        let person = match filter {
            ChangeRequestFilter::All => None,
            ChangeRequestFilter::Authored => Some("author"),
            ChangeRequestFilter::Reviewing => Some("reviewer"),
        };
        if let Some(person) = person {
            // Without a known account, "mine" can't be asked for.
            let viewer = self
                .viewer(scope.host())
                .await
                .ok_or(ChangeRequestError::Authentication)?;
            variables.push((person, viewer));
        }
        if let Some(after) = after {
            variables.push(("after", after.to_owned()));
        }
        let (data, partial_metadata) = self
            .graphql::<GlListData>(scope.host(), GITLAB_LIST_QUERY, variables, |data| {
                data.project.is_some()
            })
            .await?;
        let viewer = data
            .current_user
            .map(|user| user.username)
            .filter(|user| valid_username(user));
        // GitLab answers a project that doesn't exist, or that the viewer
        // can't see, with null. Signed out, every private project looks so.
        let Some(project) = data.project else {
            return Err(match viewer {
                None => ChangeRequestError::Authentication,
                Some(_) => ChangeRequestError::UnsupportedRepository,
            });
        };
        // GitLab's name for the project, which the board adopts.
        let canonical = self
            .scope(scope.host(), &project.full_path)
            .ok_or(ChangeRequestError::Decode)?;
        let requests = project.merge_requests.ok_or(ChangeRequestError::Decode)?;
        let mut items = requests
            .nodes
            .into_iter()
            .flatten()
            .map(|request| to_list_item(request, &canonical, viewer.as_deref()))
            .collect::<Result<Vec<_>, _>>()?;
        for item in &mut items {
            if partial_metadata {
                item.ci = Default::default();
                item.viewer_did_author = None;
                item.viewer_review_requested = None;
            }
            if filter == ChangeRequestFilter::Authored {
                item.viewer_did_author = Some(true);
            }
        }
        items.truncate(50);
        items.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| left.number.cmp(&right.number))
        });
        let next_cursor = requests
            .page_info
            .filter(|info| info.has_next_page && !items.is_empty())
            .and_then(|info| info.end_cursor)
            .filter(|cursor| valid_page_cursor(cursor));
        Ok(ChangeRequestPage {
            items,
            next_cursor,
            total_count: requests.count,
            fetched_at: Some(Utc::now()),
            presentation: None,
        })
    }

    /// Not yet: GitLab merge requests open in the browser.
    async fn change_request(
        &self,
        _url: &str,
        _diff: bool,
        _head: Option<&str>,
        _base: Option<&str>,
    ) -> Result<serde_json::Value, ChangeRequestError> {
        Err(ChangeRequestError::UnsupportedRepository)
    }

    /// Not yet: comment on GitLab merge requests in the browser.
    async fn post_comment(
        &self,
        _url: &str,
        _body: &str,
    ) -> Result<ChangeRequestComment, ChangeRequestError> {
        Err(ChangeRequestError::UnsupportedRepository)
    }

    /// The merge request from the checkout's branch into its project, by
    /// `mergeRequests(sourceBranches:)`.
    async fn find_for_branch(
        &self,
        scope: &ChangeRequestScope,
        source: &CheckoutSourceContext,
    ) -> Result<Option<ChangeRequestSummary>, ChangeRequestError> {
        let scope = self
            .served(scope)
            .ok_or(ChangeRequestError::UnsupportedRepository)?;
        let branch = source.branch.head_branch.as_str();
        if !valid_branch(branch) {
            return Ok(None);
        }
        let (data, _) = self
            .graphql::<GlBranchData>(
                scope.host(),
                GITLAB_BRANCH_QUERY,
                vec![
                    ("fullPath", scope.path().to_owned()),
                    ("branch", branch.to_owned()),
                ],
                |data| data.project.is_some(),
            )
            .await?;
        let Some(project) = data.project else {
            return match data.current_user {
                None => Err(ChangeRequestError::Authentication),
                Some(_) => Ok(None),
            };
        };
        let default_branch = source.default_branch.clone().or_else(|| {
            project
                .repository
                .and_then(|repository| repository.root_ref)
                .filter(|root| !root.is_empty())
        });
        let candidates = project
            .merge_requests
            .map(|requests| requests.nodes.into_iter().flatten().collect())
            .unwrap_or_default();
        select_merge_request(
            &project.full_path,
            branch,
            default_branch.as_deref(),
            candidates,
        )
    }
}

#[derive(Debug, Deserialize)]
struct GlResponse<T> {
    #[serde(default = "Option::default")]
    data: Option<T>,
    #[serde(default)]
    errors: Vec<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GlViewerData {
    #[serde(default)]
    current_user: Option<GlUser>,
}

#[derive(Debug, Deserialize)]
struct GlProjectData {
    #[serde(default)]
    project: Option<GlProjectPath>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GlProjectPath {
    full_path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GlListData {
    #[serde(default)]
    current_user: Option<GlUser>,
    #[serde(default)]
    project: Option<GlProject>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GlUser {
    username: String,
    #[serde(default)]
    avatar_url: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GlProject {
    full_path: String,
    #[serde(default)]
    merge_requests: Option<GlMergeRequests>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GlMergeRequests {
    #[serde(default)]
    count: Option<u64>,
    #[serde(default)]
    page_info: Option<GlPageInfo>,
    nodes: Vec<Option<GlMergeRequest>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GlPageInfo {
    has_next_page: bool,
    end_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GlMergeRequest {
    iid: String,
    title: String,
    #[serde(default)]
    draft: bool,
    web_url: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    #[serde(default)]
    author: Option<GlUser>,
    #[serde(default)]
    diff_stats_summary: Option<GlDiffStats>,
    #[serde(default)]
    diff_head_sha: Option<String>,
    #[serde(default)]
    head_pipeline: Option<GlPipeline>,
    #[serde(default)]
    conflicts: Option<bool>,
    #[serde(default)]
    merge_status_enum: Option<String>,
    #[serde(default)]
    approved: Option<bool>,
    #[serde(default)]
    approvals_left: Option<i64>,
    #[serde(default)]
    reviewers: Option<GlReviewers>,
}

#[derive(Debug, Deserialize)]
struct GlDiffStats {
    additions: u64,
    deletions: u64,
}

#[derive(Debug, Deserialize)]
struct GlPipeline {
    status: String,
    #[serde(default)]
    sha: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GlReviewers {
    nodes: Vec<Option<GlReviewer>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GlReviewer {
    username: String,
    #[serde(default)]
    merge_request_interaction: Option<GlInteraction>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GlInteraction {
    #[serde(default)]
    review_state: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GlBranchData {
    #[serde(default)]
    current_user: Option<GlUser>,
    #[serde(default)]
    project: Option<GlBranchProject>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GlBranchProject {
    full_path: String,
    #[serde(default)]
    repository: Option<GlRepository>,
    #[serde(default)]
    merge_requests: Option<GlBranchMergeRequests>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GlRepository {
    #[serde(default)]
    root_ref: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GlBranchMergeRequests {
    nodes: Vec<Option<GlBranchMergeRequest>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GlBranchMergeRequest {
    iid: String,
    title: String,
    web_url: String,
    state: String,
    source_branch: String,
    target_branch: String,
    updated_at: DateTime<Utc>,
    #[serde(default)]
    source_project: Option<GlProjectPath>,
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use serde_json::json;
    use zeron_proto::change_request_assessment::{AttentionReason, Blocker, Facts};

    use super::*;
    use crate::change_request_board::ChangeRequestBoardProviders;
    use crate::source_control::{BranchHeadContext, GitHubCli, ProcessOutput, ProcessRunError};

    const PROJECT: &str = "gitlab.com/acme/platform/team/api";

    /// Answers `glab` from a queue and records what it was asked. Never
    /// starts a process.
    #[derive(Default)]
    struct FakeGlab {
        requests: Mutex<Vec<ProcessRequest>>,
        responses: Mutex<VecDeque<Result<ProcessOutput, ProcessRunError>>>,
    }

    impl FakeGlab {
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
    impl ProcessRunner for FakeGlab {
        async fn run(&self, request: ProcessRequest) -> Result<ProcessOutput, ProcessRunError> {
            self.requests.lock().unwrap().push(request);
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("missing fake glab response")
        }
    }

    fn success(stdout: impl Into<Vec<u8>>) -> Result<ProcessOutput, ProcessRunError> {
        Ok(ProcessOutput {
            success: true,
            stdout: stdout.into(),
            stderr: Vec::new(),
            stdout_truncated: false,
        })
    }

    fn failure(stderr: &str) -> Result<ProcessOutput, ProcessRunError> {
        Ok(ProcessOutput {
            success: false,
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
            stdout_truncated: false,
        })
    }

    fn scope() -> ChangeRequestScope {
        ChangeRequestScope::parse(PROJECT).unwrap()
    }

    fn head(c: char) -> String {
        c.to_string().repeat(40)
    }

    /// GitLab alone behind the registry, as the board reaches it.
    fn board(gitlab: GitLabCli) -> ChangeRequestBoardProviders {
        ChangeRequestBoardProviders::for_tests(vec![Arc::new(gitlab)], None)
    }

    fn merge_request(iid: u64, author: &str) -> serde_json::Value {
        json!({
            "iid": iid.to_string(),
            "title": format!("  Change\n{iid} "),
            "draft": false,
            "webUrl": format!("https://gitlab.com/acme/platform/team/api/-/merge_requests/{iid}"),
            "createdAt": "2026-08-10T09:30:00Z",
            "updatedAt": format!("2026-08-19T{:02}:00:00Z", iid % 24),
            "author": {"username": author, "avatarUrl": "https://avatars.example/a.png"},
            "diffStatsSummary": {"additions": iid, "deletions": 1},
            "diffHeadSha": head('a'),
            "headPipeline": {"status": "SUCCESS", "sha": head('a')},
            "conflicts": false,
            "mergeStatusEnum": "CAN_BE_MERGED",
            "approved": false,
            "approvalsLeft": 0,
            "reviewers": {"nodes": []},
        })
    }

    fn reviewer(username: &str, state: &str) -> serde_json::Value {
        json!({"username": username, "mergeRequestInteraction": {"reviewState": state}})
    }

    fn list_response(
        nodes: Vec<serde_json::Value>,
        cursor: Option<&str>,
        viewer: Option<&str>,
    ) -> Result<ProcessOutput, ProcessRunError> {
        let count = nodes.len();
        success(
            serde_json::to_vec(&json!({"data": {
                "currentUser": viewer.map(|username| json!({"username": username})),
                "project": {
                    "fullPath": "acme/platform/team/api",
                    "mergeRequests": {
                        "count": count + 10,
                        "pageInfo": {"hasNextPage": cursor.is_some(), "endCursor": cursor},
                        "nodes": nodes,
                    }
                }
            }}))
            .unwrap(),
        )
    }

    fn graphql_args(query: &str, variables: &[(&str, &str)]) -> Vec<String> {
        let mut args: Vec<String> = ["api", "graphql", "--hostname", "gitlab.com", "-f"]
            .map(String::from)
            .into();
        args.push(format!("query={query}"));
        for (name, value) in variables {
            args.extend(["-f".into(), format!("{name}={value}")]);
        }
        args
    }

    #[tokio::test]
    async fn merge_requests_map_onto_board_rows() {
        let mut draft = merge_request(1, "devuser");
        draft["draft"] = true.into();
        let mut failing = merge_request(2, "colleague");
        failing["headPipeline"]["status"] = "FAILED".into();
        failing["conflicts"] = true.into();
        failing["reviewers"]["nodes"] = json!([reviewer("DeVuSeR", "UNREVIEWED")]);
        // Uploaded avatars come as paths on the instance.
        failing["author"]["avatarUrl"] = "/uploads/-/system/user/avatar/7/avatar.png".into();
        let mut stale = merge_request(3, "colleague");
        stale["headPipeline"]["sha"] = head('b').into();
        stale["approved"] = true.into();
        stale["reviewers"]["nodes"] = json!([reviewer("devuser", "APPROVED")]);
        stale["author"]["avatarUrl"] = serde_json::Value::Null;
        let mut feedback = merge_request(4, "devuser");
        feedback["headPipeline"]["status"] = "RUNNING".into();
        feedback["mergeStatusEnum"] = "UNCHECKED".into();
        feedback["approved"] = true.into();
        feedback["reviewers"]["nodes"] = json!([
            reviewer("lead", "APPROVED"),
            reviewer("other", "REQUESTED_CHANGES")
        ]);
        let mut waiting = merge_request(5, "colleague");
        waiting["headPipeline"] = serde_json::Value::Null;
        waiting["mergeStatusEnum"] = "CANNOT_BE_MERGED".into();
        waiting["approvalsLeft"] = 1.into();
        waiting["diffStatsSummary"] = serde_json::Value::Null;
        let mut canceled = merge_request(6, "colleague");
        canceled["headPipeline"]["status"] = "CANCELED".into();
        canceled["reviewers"]["nodes"] = json!([reviewer("devuser", "REVIEWED")]);
        // Asking for one's own review is deliberate: it still needs you.
        let mut own_review = merge_request(7, "devuser");
        own_review["reviewers"]["nodes"] = json!([reviewer("devuser", "UNREVIEWED")]);
        let runner = FakeGlab::with_responses([list_response(
            vec![
                draft, failing, stale, feedback, waiting, canceled, own_review,
            ],
            None,
            Some("devuser"),
        )]);
        let board = board(GitLabCli::with_runner(runner.clone()));
        // The board may ask with any casing; GitLab's own path comes back.
        let asked = board.parse("GitLab.com/Acme/Platform/team/api").unwrap();
        let page = board
            .list_page(&asked, ChangeRequestFilter::All, None, false, false)
            .await
            .unwrap();
        let request = &runner.requests()[0];
        assert_eq!(request.program, "glab");
        assert!(request.login_shell);
        assert_eq!(
            request.args,
            graphql_args(GITLAB_LIST_QUERY, &[("fullPath", "Acme/Platform/team/api")])
        );
        assert_eq!(
            request.env,
            [("GLAB_NO_PROMPT".to_string(), "true".to_string())]
        );
        assert_eq!(page.total_count, Some(17));
        assert_eq!(page.next_cursor, None);
        let presentation = page.presentation.as_ref().unwrap();
        assert_eq!(presentation.provider_name, "GitLab");
        assert_eq!(
            presentation.sign_in_command,
            "glab auth login --hostname gitlab.com"
        );
        assert_eq!(presentation.media_hosts, ["gitlab.com"]);
        assert_eq!(presentation.features, ChangeRequestFeatures::default());
        let items: HashMap<u64, &ChangeRequestListItem> =
            page.items.iter().map(|item| (item.number, item)).collect();
        assert_eq!(page.items[0].number, 7, "newest updates first");
        for item in &page.items {
            assert_eq!(item.provider, "gitlab");
            assert_eq!(item.repository, PROJECT);
            assert_eq!(item.state, ChangeRequestState::Open);
            assert_eq!(item.title, format!("Change {}", item.number));
            assert_eq!(item.head_ref_oid, head('a'));
            assert_eq!(item.reference, Some(format!("!{}", item.number)));
        }
        let first = items[&1];
        assert!(first.is_draft);
        assert_eq!(first.author.login, "devuser");
        assert_eq!(
            first.author.avatar_url.as_deref(),
            Some("https://avatars.example/a.png")
        );
        assert_eq!(
            first.url,
            "https://gitlab.com/acme/platform/team/api/-/merge_requests/1"
        );
        assert_eq!((first.additions, first.deletions), (1, 1));
        assert_eq!(first.ci.state, CiState::Passed);
        assert_eq!(first.mergeability, ChangeRequestMergeability::Mergeable);
        assert_eq!(first.review_decision, ChangeRequestReviewDecision::Unknown);
        assert_eq!(first.viewer_did_author, Some(true));
        assert_eq!(first.viewer_review_requested, Some(false));

        let failing = items[&2];
        assert_eq!(
            failing.author.avatar_url.as_deref(),
            Some("https://gitlab.com/uploads/-/system/user/avatar/7/avatar.png")
        );
        assert_eq!(failing.ci.state, CiState::Failed);
        assert_eq!(failing.mergeability, ChangeRequestMergeability::Conflicting);
        assert_eq!(
            failing.review_decision,
            ChangeRequestReviewDecision::ReviewRequired
        );
        assert_eq!(failing.viewer_did_author, Some(false));
        assert_eq!(failing.viewer_review_requested, Some(true));
        let assessment = Facts::from_item(failing).assess();
        assert_eq!(
            assessment.blockers,
            [Blocker::FailingCi, Blocker::MergeConflicts]
        );
        assert_eq!(
            assessment.attention,
            [AttentionReason::ReviewRequested, AttentionReason::FailingCi],
            "needs you"
        );

        let stale = items[&3];
        assert_eq!(stale.author.avatar_url, None);
        assert_eq!(
            stale.ci.state,
            CiState::Unknown,
            "pipeline for an older head"
        );
        assert_eq!(stale.review_decision, ChangeRequestReviewDecision::Approved);
        assert_eq!(
            stale.viewer_review_requested,
            Some(false),
            "already approved"
        );

        let feedback = items[&4];
        assert_eq!(feedback.ci.state, CiState::Pending);
        assert_eq!(feedback.mergeability, ChangeRequestMergeability::Unknown);
        assert_eq!(
            feedback.review_decision,
            ChangeRequestReviewDecision::ChangesRequested,
            "changes requested outweighs an approval"
        );
        assert_eq!(
            Facts::from_item(feedback).assess().attention,
            [AttentionReason::FeedbackOnAuthored]
        );

        let waiting = items[&5];
        assert_eq!(waiting.ci.state, CiState::NoChecks);
        assert_eq!(waiting.mergeability, ChangeRequestMergeability::Conflicting);
        assert_eq!(
            waiting.review_decision,
            ChangeRequestReviewDecision::ReviewRequired
        );
        assert_eq!((waiting.additions, waiting.deletions), (0, 0));

        let canceled = items[&6];
        assert_eq!(canceled.ci.state, CiState::Failed);
        assert_eq!(
            canceled.review_decision,
            ChangeRequestReviewDecision::Unknown
        );
        assert_eq!(canceled.viewer_review_requested, Some(false));

        let own_review = items[&7];
        assert_eq!(own_review.viewer_did_author, Some(true));
        assert_eq!(own_review.viewer_review_requested, Some(true));
        assert_eq!(
            Facts::from_item(own_review).assess().attention,
            [AttentionReason::ReviewRequested]
        );
    }

    #[test]
    fn pipeline_states_map_onto_ci() {
        for (status, state) in [
            ("SUCCESS", CiState::Passed),
            ("FAILED", CiState::Failed),
            ("CANCELED", CiState::Failed),
            ("CANCELING", CiState::Failed),
            ("CREATED", CiState::Pending),
            ("WAITING_FOR_RESOURCE", CiState::Pending),
            ("WAITING_FOR_CALLBACK", CiState::Pending),
            ("PREPARING", CiState::Pending),
            ("PENDING", CiState::Pending),
            ("RUNNING", CiState::Pending),
            ("SCHEDULED", CiState::Pending),
            ("MANUAL", CiState::Pending),
            ("SKIPPED", CiState::Skipped),
            ("SOMETHING_NEW", CiState::Unknown),
        ] {
            assert_eq!(pipeline_state(status), state, "{status}");
        }
    }

    #[tokio::test]
    async fn filters_ask_for_the_viewer_and_pages_follow_the_cursor() {
        let cursor = "eyJpZCI6IjQ0MSJ9";
        let runner = FakeGlab::with_responses([
            success("devuser\n"),
            list_response(
                vec![merge_request(9, "devuser")],
                Some(cursor),
                Some("devuser"),
            ),
            list_response(vec![merge_request(8, "devuser")], None, Some("devuser")),
            list_response(vec![merge_request(7, "colleague")], None, Some("devuser")),
            list_response(vec![merge_request(6, "colleague")], None, Some("devuser")),
        ]);
        let gitlab = GitLabCli::with_runner(runner.clone());
        let path = "acme/platform/team/api";
        let first = gitlab
            .list_page(&scope(), ChangeRequestFilter::Authored, None)
            .await
            .unwrap();
        assert_eq!(first.next_cursor.as_deref(), Some(cursor));
        assert_eq!(first.items[0].viewer_did_author, Some(true));
        let second = gitlab
            .list_page(&scope(), ChangeRequestFilter::Authored, Some(cursor))
            .await
            .unwrap();
        assert_eq!(second.items[0].number, 8);
        let reviewing = gitlab
            .list_page(&scope(), ChangeRequestFilter::Reviewing, None)
            .await
            .unwrap();
        // Being a reviewer lists it; only an unreviewed request needs you.
        assert_eq!(reviewing.items[0].viewer_review_requested, Some(false));
        gitlab
            .list_page(&scope(), ChangeRequestFilter::All, None)
            .await
            .unwrap();
        let requests = runner.requests();
        assert_eq!(
            requests[0].args,
            ["config", "get", "user", "--host", "gitlab.com"]
        );
        assert_eq!(
            requests[1].args,
            graphql_args(
                GITLAB_LIST_QUERY,
                &[("fullPath", path), ("author", "devuser")]
            )
        );
        assert_eq!(
            requests[2].args,
            graphql_args(
                GITLAB_LIST_QUERY,
                &[("fullPath", path), ("author", "devuser"), ("after", cursor)]
            )
        );
        // The account is read once, not per page.
        assert_eq!(
            requests[3].args,
            graphql_args(
                GITLAB_LIST_QUERY,
                &[("fullPath", path), ("reviewer", "devuser")]
            )
        );
        assert_eq!(
            requests[4].args,
            graphql_args(GITLAB_LIST_QUERY, &[("fullPath", path)])
        );
        assert_eq!(requests.len(), 5);
        // Cursors are checked before they reach glab.
        assert_eq!(
            gitlab
                .list_page(&scope(), ChangeRequestFilter::All, Some("$(nope)"))
                .await
                .err(),
            Some(ChangeRequestError::Decode)
        );
        assert_eq!(runner.requests().len(), 5);
    }

    #[tokio::test]
    async fn without_a_configured_account_filters_ask_gitlab_who_is_signed_in() {
        let runner = FakeGlab::with_responses([
            success(""),
            success(r#"{"data":{"currentUser":{"username":"token-owner"}}}"#),
            list_response(Vec::new(), None, Some("token-owner")),
            list_response(Vec::new(), None, Some("token-owner")),
        ]);
        let gitlab = GitLabCli::with_runner(runner.clone());
        for _ in 0..2 {
            gitlab
                .list_page(&scope(), ChangeRequestFilter::Reviewing, None)
                .await
                .unwrap();
        }
        let requests = runner.requests();
        assert_eq!(requests[1].args, graphql_args(GITLAB_VIEWER_QUERY, &[]));
        assert_eq!(
            requests[2].args,
            graphql_args(
                GITLAB_LIST_QUERY,
                &[
                    ("fullPath", "acme/platform/team/api"),
                    ("reviewer", "token-owner")
                ]
            )
        );
        // Both answers are remembered: the next page asks for nothing else.
        assert_eq!(requests[3].args, requests[2].args);
        assert_eq!(requests.len(), 4);
    }

    #[tokio::test]
    async fn first_pages_are_stored_per_gitlab_account() {
        let dir = tempfile::tempdir().unwrap();
        let runner = FakeGlab::with_responses([
            success("devuser\n"),
            list_response(vec![merge_request(1, "devuser")], None, Some("devuser")),
            success("devuser\n"),
        ]);
        let gitlab = GitLabCli::with_runner(runner.clone()).with_hosts(&["git.example.org"]);
        assert_eq!(
            gitlab.account_key("GitLab.com").await.as_deref(),
            Some("gitlab:gitlab.com:devuser")
        );
        // Unserved hosts have no account to read.
        assert_eq!(gitlab.account_key("github.com").await, None);
        assert_eq!(gitlab.account_key("gitlab.example.com").await, None);
        let board =
            ChangeRequestBoardProviders::for_tests(vec![Arc::new(gitlab)], Some(dir.path()));
        let fetched = board
            .list_page(&scope(), ChangeRequestFilter::All, None, false, false)
            .await
            .unwrap();
        // A relaunch reads the account locally and serves the stored copy,
        // avatars included, without asking GitLab again.
        let relaunched = ChangeRequestBoardProviders::for_tests(
            vec![Arc::new(GitLabCli::with_runner(runner.clone()))],
            Some(dir.path()),
        );
        let stored = relaunched
            .list_page(&scope(), ChangeRequestFilter::All, None, false, true)
            .await
            .unwrap();
        assert_eq!(stored.items, fetched.items);
        assert!(stored.items[0].author.avatar_url.is_some());
        let requests = runner.requests();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[2].args[..3], ["config", "get", "user"]);
    }

    #[tokio::test]
    async fn failures_are_classified_without_exposing_output() {
        let not_found = || {
            success(
                serde_json::to_vec(&json!({"data": {
                    "currentUser": {"username": "devuser"}, "project": null
                }}))
                .unwrap(),
            )
        };
        let signed_out = || {
            success(
                serde_json::to_vec(&json!({"data": {"currentUser": null, "project": null}}))
                    .unwrap(),
            )
        };
        let truncated = || {
            Ok(ProcessOutput {
                success: true,
                stdout: b"{\"data\":".to_vec(),
                stderr: Vec::new(),
                stdout_truncated: true,
            })
        };
        for (response, expected) in [
            (
                Err(ProcessRunError::Spawn(std::io::ErrorKind::NotFound)),
                ChangeRequestError::CliUnavailable,
            ),
            (Err(ProcessRunError::Timeout), ChangeRequestError::Timeout),
            (
                failure("glab: Invalid token"),
                ChangeRequestError::Authentication,
            ),
            (
                failure("ERROR\n  Unauthenticated.\n"),
                ChangeRequestError::Authentication,
            ),
            (
                failure("429 Too Many Requests"),
                ChangeRequestError::RateLimited,
            ),
            (
                failure("private-provider-diagnostic"),
                ChangeRequestError::CommandFailed,
            ),
            (signed_out(), ChangeRequestError::Authentication),
            (not_found(), ChangeRequestError::UnsupportedRepository),
            (truncated(), ChangeRequestError::Decode),
            (success("not json"), ChangeRequestError::Decode),
        ] {
            let runner = FakeGlab::with_responses([response]);
            let result = GitLabCli::with_runner(runner)
                .list_page(&scope(), ChangeRequestFilter::All, None)
                .await;
            assert_eq!(result.err(), Some(expected));
        }
    }

    #[tokio::test]
    async fn partial_errors_keep_rows_but_drop_their_metadata() {
        let mut response: serde_json::Value = serde_json::from_slice(
            &list_response(vec![merge_request(1, "devuser")], None, Some("devuser"))
                .unwrap()
                .stdout,
        )
        .unwrap();
        response["errors"] = json!([{"message": "approvalsLeft is unavailable"}]);
        let runner = FakeGlab::with_responses([Ok(ProcessOutput {
            success: false,
            stdout: serde_json::to_vec(&response).unwrap(),
            stderr: b"glab: approvalsLeft is unavailable".to_vec(),
            stdout_truncated: false,
        })]);
        let page = GitLabCli::with_runner(runner)
            .list_page(&scope(), ChangeRequestFilter::All, None)
            .await
            .unwrap();
        assert_eq!(page.items[0].ci.state, CiState::Unknown);
        assert_eq!(page.items[0].viewer_did_author, None);
        assert_eq!(page.items[0].viewer_review_requested, None);
    }

    #[tokio::test]
    async fn canonical_scopes_come_from_gitlab_and_detail_stays_in_the_browser() {
        let runner = FakeGlab::with_responses([
            success(r#"{"data":{"project":{"fullPath":"acme/platform/team/renamed-api"}}}"#),
            success(r#"{"data":{"project":null}}"#),
        ]);
        let gitlab = GitLabCli::with_runner(runner.clone());
        assert_eq!(
            gitlab.canonical_scope(&scope()).await,
            Ok(ChangeRequestScope::parse(
                "gitlab.com/acme/platform/team/renamed-api"
            ))
        );
        assert_eq!(
            runner.requests()[0].args,
            graphql_args(
                GITLAB_PROJECT_QUERY,
                &[("fullPath", "acme/platform/team/api")]
            )
        );
        assert_eq!(gitlab.canonical_scope(&scope()).await, Ok(None));
        // Neither a GitHub scope nor an unserved host reaches glab.
        for unserved in [
            ChangeRequestScope::parse("acme/zeron").unwrap(),
            ChangeRequestScope::parse("gitlab.example.com/group/project").unwrap(),
        ] {
            assert_eq!(
                gitlab.canonical_scope(&unserved).await,
                Err(ChangeRequestError::UnsupportedRepository)
            );
            assert_eq!(
                gitlab
                    .list_page(&unserved, ChangeRequestFilter::All, None)
                    .await
                    .err(),
                Some(ChangeRequestError::UnsupportedRepository)
            );
        }
        let board = board(gitlab);
        let url = "https://GitLab.com/acme/platform/team/api/-/merge_requests/441#note_1";
        assert_eq!(
            board.change_request(url, false, false, None, None).await,
            Err(ChangeRequestError::UnsupportedRepository)
        );
        assert_eq!(
            board.post_comment(url, "hello").await.err(),
            Some(ChangeRequestError::UnsupportedRepository)
        );
        assert_eq!(runner.requests().len(), 2);
    }

    #[test]
    fn urls_and_scopes_name_projects_on_served_hosts() {
        let gitlab =
            GitLabCli::with_runner(FakeGlab::with_responses([])).with_hosts(&["git.example.org"]);
        assert_eq!(
            gitlab
                .change_request_url(
                    "https://GitLab.com/acme/platform/team/api/-/merge_requests/0441?tab=diffs"
                )
                .as_deref(),
            Some("https://gitlab.com/acme/platform/team/api/-/merge_requests/441")
        );
        assert!(
            gitlab
                .change_request_url("https://git.example.org/group/project/-/merge_requests/2")
                .is_some()
        );
        for url in [
            "http://gitlab.com/group/project/-/merge_requests/1",
            "https://user@gitlab.com/group/project/-/merge_requests/1",
            "https://gitlab.com:8443/group/project/-/merge_requests/1",
            "https://gitlab.com/group/project/-/merge_requests/0",
            "https://gitlab.com/group/project/-/merge_requests/1/diffs",
            "https://gitlab.com/project/-/merge_requests/1",
            "https://gitlab.com/group/project/-/issues/1",
            "https://gitlab.example.com/group/project/-/merge_requests/1",
            "https://github.com/acme/zeron/pull/1",
        ] {
            assert_eq!(gitlab.change_request_url(url), None, "{url}");
        }
        assert!(gitlab.serves_host("gitlab.com") && gitlab.serves_host("git.example.org"));
        assert!(!gitlab.serves_host("github.com") && !gitlab.serves_host("other.test"));
        // Nested groups are projects; `-` never names one.
        assert!(
            gitlab
                .scope("gitlab.com", "acme/platform/team/api")
                .is_some()
        );
        assert!(gitlab.scope("gitlab.com", "acme/-/api").is_none());
        assert_eq!(
            gitlab.presentation("git.example.org").sign_in_command,
            "glab auth login --hostname git.example.org"
        );
    }

    #[test]
    fn configured_hosts_come_from_the_hosts_map() {
        let config = "\
# What protocol to use
git_protocol: ssh
host: gitlab.com
hosts:
    # Configuration specific for GitLab instances.
    gitlab.com:
        api_protocol: https
        api_host: gitlab.com
        user: devuser
    git.example.org:
        user: someone
        custom_headers: []
    \"Gitlab.Corp.Example\":
        user: someone
    github.com:
        user: wrong
    -bad:
        user: x
aliases:
    ci.example.com:
";
        assert_eq!(
            config_hosts(config),
            ["gitlab.com", "git.example.org", "gitlab.corp.example"]
        );
        assert!(config_hosts("hosts: {}\n").is_empty());
        assert!(config_hosts("").is_empty());
    }

    fn source(remote: &str, branch: &str, default_branch: Option<&str>) -> CheckoutSourceContext {
        CheckoutSourceContext {
            checkout_root: "/checkout".into(),
            branch: BranchHeadContext::resolve(
                branch,
                Some(&format!("origin/{branch}")),
                Some("origin"),
                Some(remote),
            ),
            default_branch: default_branch.map(str::to_owned),
        }
    }

    fn branch_request(
        iid: u64,
        state: &str,
        source_project: &str,
        updated: &str,
    ) -> serde_json::Value {
        json!({
            "iid": iid.to_string(),
            "title": format!("Change {iid}"),
            "webUrl": format!("https://gitlab.com/acme/platform/team/api/-/merge_requests/{iid}"),
            "state": state,
            "sourceBranch": "feature/change",
            "targetBranch": "main",
            "updatedAt": updated,
            "sourceProject": {"fullPath": source_project},
        })
    }

    fn branch_response(nodes: Vec<serde_json::Value>) -> Result<ProcessOutput, ProcessRunError> {
        success(
            serde_json::to_vec(&json!({"data": {
                "currentUser": {"username": "devuser"},
                "project": {
                    "fullPath": "acme/platform/team/api",
                    "repository": {"rootRef": "main"},
                    "mergeRequests": {"nodes": nodes},
                }
            }}))
            .unwrap(),
        )
    }

    #[tokio::test]
    async fn branch_badges_find_the_branchs_merge_request_through_the_registry() {
        let runner = FakeGlab::with_responses([
            branch_response(vec![
                // A fork's branch of the same name is another change.
                branch_request(5, "opened", "someone/api", "2026-08-20T00:00:00Z"),
                branch_request(
                    4,
                    "merged",
                    "acme/platform/team/api",
                    "2026-08-19T00:00:00Z",
                ),
                branch_request(
                    3,
                    "opened",
                    "Acme/Platform/team/api",
                    "2026-08-01T00:00:00Z",
                ),
            ]),
            branch_response(vec![branch_request(
                4,
                "merged",
                "acme/platform/team/api",
                "2026-08-19T00:00:00Z",
            )]),
            branch_response(vec![branch_request(
                4,
                "merged",
                "acme/platform/team/api",
                "2026-08-19T00:00:00Z",
            )]),
            success(r#"{"data":{"currentUser":null,"project":null}}"#),
        ]);
        let resolver = crate::source_control::ChangeRequestResolver::with_providers(
            ChangeRequestBoardProviders::for_tests(
                vec![
                    Arc::new(GitHubCli::offline()),
                    Arc::new(GitLabCli::with_runner(runner.clone())),
                ],
                None,
            ),
        );
        let nested = source(
            "git@gitlab.com:acme/platform/team/api.git",
            "feature/change",
            Some("main"),
        );
        let summary = resolver.resolve_source(&nested).await.unwrap().unwrap();
        assert_eq!(summary.provider, "gitlab");
        assert_eq!(summary.number, 3, "open beats a newer merged one");
        assert_eq!(summary.reference.as_deref(), Some("!3"));
        assert_eq!(summary.state, ChangeRequestState::Open);
        assert_eq!(
            (summary.base_ref.as_str(), summary.head_ref.as_str()),
            ("main", "feature/change")
        );
        assert!(!summary.features.unwrap().native_detail);
        assert_eq!(
            runner.requests()[0].args,
            graphql_args(
                GITLAB_BRANCH_QUERY,
                &[
                    ("fullPath", "acme/platform/team/api"),
                    ("branch", "feature/change")
                ]
            )
        );
        // A merged one is the branch's answer when nothing is open.
        let merged = resolver.resolve_source(&nested).await.unwrap().unwrap();
        assert_eq!(merged.state, ChangeRequestState::Merged);
        // On the default branch (here told by GitLab), only an open one.
        let on_default = source(
            "https://gitlab.com/acme/platform/team/api.git",
            "feature/change",
            None,
        );
        let mut on_default_branch = on_default.clone();
        on_default_branch.default_branch = Some("feature/change".into());
        assert_eq!(resolver.resolve_source(&on_default_branch).await, Ok(None));
        // Signed out, every project looks missing: that's a sign-in error.
        assert_eq!(
            resolver.resolve_source(&on_default).await,
            Err(ChangeRequestError::Authentication)
        );
        assert_eq!(runner.requests().len(), 4);
    }
}
