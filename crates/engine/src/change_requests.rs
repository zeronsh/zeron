//! Demand-driven checkout change-request cache and streams.
//!
//! A watched checkout's Git state (its branch, upstream, remotes) is read
//! again only when the files Git reads it from change: file-system events on
//! its git directories ([`GitWatch`]), gated by a cheap fingerprint of those
//! files ([`GitFingerprint`]), so a checkout left alone runs no Git at all.
//! The provider (GitHub) is asked again on its own schedule (`refresh`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Duration;

use futures::future::{BoxFuture, Shared};
use futures::stream::BoxStream;
use futures::{FutureExt, StreamExt};
use tokio::sync::{Mutex as AsyncMutex, watch};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use zeron_proto::CheckoutChangeRequestStatus;

use crate::fs_watch::{FsWatch, FsWatchHub};
use crate::repos::{CheckoutIdentity, Repos};
use crate::source_control::{
    BranchHeadContext, ChangeRequestError, ChangeRequestResolver, CheckoutChangeRequestLookup,
    CheckoutSourceContext, parse_git_remote,
};

const WITH_CHANGE_REQUEST_TTL: Duration = Duration::from_secs(2 * 60);
const WITHOUT_CHANGE_REQUEST_TTL: Duration = Duration::from_secs(45);
const FAILURE_INITIAL_BACKOFF: Duration = Duration::from_secs(20);
const FAILURE_MAX_BACKOFF: Duration = Duration::from_secs(15 * 60);
/// How often a checkout whose git directories aren't watched (the watch
/// failed, or isn't live yet) is inspected again, as every checkout once was.
const CONTEXT_POLL_INTERVAL: Duration = Duration::from_secs(15);
/// Git changes several of its files at once (a checkout writes HEAD, the
/// index and a reflog): a burst of events is one look, this long after it.
const GIT_EVENT_SETTLE: Duration = Duration::from_millis(250);
/// A checkout on its repository's default branch: a pull request whose head
/// is that branch is rare (a release pull request), so it is asked about
/// seldom. Every chat started in a project's main folder sits there, and each
/// was asking GitHub every 45 s.
const DEFAULT_BRANCH_TTL: Duration = Duration::from_secs(10 * 60);

/// What one provider question is of. Checkouts of one repository on one
/// branch ask GitHub the same thing, so they share the answer: `scope` is
/// empty when the remote names the repository, and the checkout's id only
/// when it doesn't (that lookup fails without asking GitHub anyway).

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ChangeRequestCacheKey {
    pub scope: String,
    pub branch: String,
    pub upstream_ref: String,
    pub remote: String,
}

#[derive(Debug, Clone, Copy)]
struct Timing {
    with_change_request_ttl: Duration,
    without_change_request_ttl: Duration,
    failure_initial_backoff: Duration,
    failure_max_backoff: Duration,
    context_poll_interval: Duration,
    default_branch_ttl: Duration,
    git_event_settle: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            with_change_request_ttl: WITH_CHANGE_REQUEST_TTL,
            without_change_request_ttl: WITHOUT_CHANGE_REQUEST_TTL,
            failure_initial_backoff: FAILURE_INITIAL_BACKOFF,
            failure_max_backoff: FAILURE_MAX_BACKOFF,
            context_poll_interval: CONTEXT_POLL_INTERVAL,
            default_branch_ttl: DEFAULT_BRANCH_TTL,
            git_event_settle: GIT_EVENT_SETTLE,
        }
    }
}

struct CacheEntry {
    state: AsyncMutex<CacheState>,
    demand: AtomicUsize,
    /// When what it holds goes stale (its `next_refresh`, readable without
    /// the async lock): an entry nobody watches is kept until then, so a
    /// watch that goes and comes back (a row scrolled away and back, a
    /// plugin's watch started again) is answered without asking GitHub.
    stale_at: Mutex<Instant>,
}

impl CacheEntry {
    fn is_stale(&self, now: Instant) -> bool {
        now >= *self.stale_at.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

struct CacheState {
    last_success: Option<CheckoutChangeRequestStatus>,
    next_refresh: Instant,
    failures: u32,
    last_error: Option<ChangeRequestError>,
}

type InspectionRun = Shared<BoxFuture<'static, Result<CheckoutSourceContext, ChangeRequestError>>>;

/// The latest Git inspection of one watched path. Every subscription on the
/// path shares it, so a project with many chats runs Git once per change
/// instead of once per chat. Any waiter can drive the shared run, so a
/// subscription that stops being polled mid-inspection blocks no one.
struct Inspection {
    started: Instant,
    /// What the checkout's Git files were as it started: while they stay so
    /// it answers (for a poll interval, if they aren't watched).
    inputs: Option<GitFingerprint>,
    /// The watch of the subscription that started it: kept while that lasts.
    watch: Weak<GitWatch>,
    run: InspectionRun,
}

struct Inner {
    repos: Repos,
    device_id: String,
    lookup: Arc<dyn CheckoutChangeRequestLookup>,
    entries: Mutex<HashMap<ChangeRequestCacheKey, Arc<CacheEntry>>>,
    inspections: Mutex<HashMap<PathBuf, Inspection>>,
    /// Each watched git dir's events, shared by its subscriptions.
    git_watches: Mutex<HashMap<PathBuf, Weak<GitWatch>>>,
    timing: Timing,
    shutdown: CancellationToken,
}

/// File-system events on one checkout's git directories, for as long as a
/// subscription holds it: `changed` ticks on each. Watched: the git dir
/// (HEAD; a worktree's own config), the common dir (config, packed-refs) and
/// its `refs` (an upstream's remote-tracking branch, `<remote>/HEAD`).
struct GitWatch {
    git_dir: PathBuf,
    changed: watch::Receiver<u64>,
    watches: Vec<FsWatch>,
}

impl GitWatch {
    /// Blocking (the hub canonicalizes): from the blocking pool.
    fn start(git_dir: &Path) -> Self {
        let (tx, changed) = watch::channel(0_u64);
        let tx = Arc::new(tx);
        let hub = FsWatchHub::global();
        let common = common_dir(git_dir);
        let mut dirs = vec![(git_dir.to_path_buf(), notify::RecursiveMode::NonRecursive)];
        if common != git_dir {
            dirs.push((common.clone(), notify::RecursiveMode::NonRecursive));
        }
        dirs.push((common.join("refs"), notify::RecursiveMode::Recursive));
        let watches = dirs
            .into_iter()
            .map(|(dir, mode)| {
                let tx = tx.clone();
                hub.watch(&dir, mode, move |_event| {
                    // Any event, and the rescan a watch sends as it goes
                    // live: the fingerprint says whether it mattered.
                    tx.send_modify(|n| *n = n.wrapping_add(1));
                })
            })
            .collect();
        Self {
            git_dir: git_dir.to_path_buf(),
            changed,
            watches,
        }
    }

    /// Whether every directory's watch is installed: events then say when
    /// Git's state can have changed.
    fn live(&self) -> bool {
        self.watches.iter().all(FsWatch::is_live)
    }
}

/// The git dir's common dir (a linked worktree's main `.git`): `commondir`
/// names it, relative to the git dir or absolute.
fn common_dir(git_dir: &Path) -> PathBuf {
    match std::fs::read_to_string(git_dir.join("commondir")) {
        Ok(common) => git_dir.join(common.trim()),
        Err(_) => git_dir.to_path_buf(),
    }
}

/// What a checkout inspection reads from Git, cheaply (a few `stat`s and
/// HEAD): its answer can't change while this doesn't. HEAD's content (the
/// branch), the configs (upstream, remotes, push default; system and global
/// ones too), packed refs, and every remote-tracking ref (an upstream
/// resolves once its branch is pushed; `<remote>/HEAD` names the default
/// branch). A commit, `git status`, an index write: none of these.
#[derive(Debug, Clone, PartialEq, Eq)]
struct GitFingerprint {
    head: Option<Vec<u8>>,
    stamps: Vec<(PathBuf, Option<(u64, i128, u64)>)>,
}

impl GitFingerprint {
    /// Blocking: from the blocking pool.
    fn read(git_dir: &Path) -> Self {
        fn stamp(path: &Path) -> Option<(u64, i128, u64)> {
            #[cfg(unix)]
            use std::os::unix::fs::MetadataExt;
            let meta = std::fs::metadata(path).ok()?;
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_nanos() as i128);
            #[cfg(unix)]
            let inode = meta.ino();
            #[cfg(not(unix))]
            let inode = 0;
            Some((inode, mtime, meta.len()))
        }
        let common = common_dir(git_dir);
        let mut paths = vec![
            common.join("config"),
            git_dir.join("config.worktree"),
            common.join("packed-refs"),
            "/etc/gitconfig".into(),
        ];
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            let xdg = std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".config"));
            paths.push(home.join(".gitconfig"));
            paths.push(xdg.join("git/config"));
        }
        // Remote-tracking refs, walked (a handful of folders; refs a fetch
        // packed are in packed-refs).
        let mut dirs = vec![common.join("refs/remotes")];
        while let Some(dir) = dirs.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if entry.file_type().is_ok_and(|t| t.is_dir()) {
                    dirs.push(path);
                } else {
                    paths.push(path);
                }
            }
        }
        paths.sort();
        Self {
            head: std::fs::read(git_dir.join("HEAD")).ok(),
            stamps: paths
                .into_iter()
                .map(|path| {
                    let stamp = stamp(&path);
                    (path, stamp)
                })
                .collect(),
        }
    }
}

/// Shared host-side cache. It owns no polling task: a stream drives refreshes only
/// while it is actively polled, so dropping the last RPC subscription stops work.
#[derive(Clone)]
pub struct CheckoutChangeRequests {
    inner: Arc<Inner>,
}

impl CheckoutChangeRequests {
    pub fn start(repos: Repos, device_id: &str) -> Self {
        Self::new(repos, device_id, Arc::new(ChangeRequestResolver::new()))
    }

    pub fn new(
        repos: Repos,
        device_id: &str,
        lookup: Arc<dyn CheckoutChangeRequestLookup>,
    ) -> Self {
        Self::with_timing(repos, device_id, lookup, Timing::default())
    }

    fn with_timing(
        repos: Repos,
        device_id: &str,
        lookup: Arc<dyn CheckoutChangeRequestLookup>,
        timing: Timing,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                repos,
                device_id: device_id.to_owned(),
                lookup,
                entries: Mutex::new(HashMap::new()),
                inspections: Mutex::new(HashMap::new()),
                git_watches: Mutex::new(HashMap::new()),
                timing,
                shutdown: CancellationToken::new(),
            }),
        }
    }

    pub async fn watch(
        &self,
        cwd: &Path,
    ) -> Result<BoxStream<'static, CheckoutChangeRequestStatus>, ChangeRequestError> {
        self.watch_for_branch(cwd, None).await
    }

    /// Resolve a PR for the conversation-owned branch while using `cwd` only
    /// for repository/remote identity. This keeps a later checkout switch from
    /// changing which PR an older conversation displays.
    pub async fn watch_for_branch(
        &self,
        cwd: &Path,
        branch: Option<&str>,
    ) -> Result<BoxStream<'static, CheckoutChangeRequestStatus>, ChangeRequestError> {
        let identity = self
            .inner
            .repos
            .checkout_identity(cwd)
            .await
            .map_err(|_| ChangeRequestError::RepositoryUnavailable)?;
        Ok(self.watch_checkout_for_branch(cwd.to_owned(), identity, branch.map(str::to_owned)))
    }

    #[cfg(test)]
    fn watch_checkout(
        &self,
        cwd: PathBuf,
        identity: CheckoutIdentity,
    ) -> BoxStream<'static, CheckoutChangeRequestStatus> {
        self.watch_checkout_for_branch(cwd, identity, None)
    }

    fn watch_checkout_for_branch(
        &self,
        cwd: PathBuf,
        identity: CheckoutIdentity,
        requested_branch: Option<String>,
    ) -> BoxStream<'static, CheckoutChangeRequestStatus> {
        let state = SubscriptionState {
            service: self.clone(),
            cwd,
            identity,
            requested_branch,
            lease: None,
            last_emitted: None,
            git: None,
        };
        futures::stream::unfold(state, |mut state| async move {
            if state.git.is_none() {
                let git = state.service.git_watch(&state.identity.git_dir).await;
                // Events from here on: the first inspection reads the files
                // as they are now.
                let mut changed = git.changed.clone();
                changed.borrow_and_update();
                state.git = Some((git, changed));
            }
            loop {
                if state.service.inner.shutdown.is_cancelled() {
                    return None;
                }
                let mut source = match state.service.inspect(&state).await {
                    Ok(source) => source,
                    Err(error) => {
                        tracing::debug!(%error, "change request checkout inspection unavailable");
                        // Until Git's files change (a branch checked out
                        // again, a repository made).
                        if !state.wait(None).await {
                            return None;
                        }
                        continue;
                    }
                };
                if let Some(branch) = state.requested_branch.as_deref()
                    && source.branch.local_branch != branch
                {
                    source.branch = BranchHeadContext::resolve(
                        branch,
                        None,
                        source.branch.remote_name.as_deref(),
                        source.branch.remote_url.as_deref(),
                    );
                }
                let key = cache_key(&state.identity.id, &source);
                if state.lease.as_ref().map(|lease| &lease.key) != Some(&key) {
                    state.lease = Some(state.service.acquire(key.clone()));
                }
                let (snapshot, next_refresh) = state.service.refresh(&key, &source).await;
                if let Some(mut snapshot) = snapshot {
                    // (The answer may be another checkout's of the same
                    // repository and branch: told as this one's.)
                    snapshot.checkout_id = state.identity.id.clone();
                    snapshot.cwd = source.checkout_root.to_string_lossy().into_owned();
                    snapshot.branch = source.branch.local_branch.clone();
                    let semantic = SemanticStatus::from(&snapshot);
                    if state.last_emitted.as_ref() != Some(&semantic) {
                        state.last_emitted = Some(semantic);
                        return Some((snapshot, state));
                    }
                }

                if !state.wait(next_refresh).await {
                    return None;
                }
            }
        })
        .boxed()
    }

    /// The watch on `git_dir`, shared with every subscription on it.
    async fn git_watch(&self, git_dir: &Path) -> Arc<GitWatch> {
        let shared = |watches: &HashMap<PathBuf, Weak<GitWatch>>| {
            watches.get(git_dir).and_then(Weak::upgrade)
        };
        if let Some(watch) = shared(
            &self
                .inner
                .git_watches
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        ) {
            return watch;
        }
        let dir = git_dir.to_owned();
        let started = tokio::task::spawn_blocking(move || GitWatch::start(&dir))
            .await
            .unwrap_or_else(|_| GitWatch {
                // (Never live: the subscription polls.)
                git_dir: git_dir.to_owned(),
                changed: watch::channel(0).1,
                watches: Vec::new(),
            });
        let mut watches = self
            .inner
            .git_watches
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        // (Another subscription may have started one meanwhile.)
        if let Some(watch) = shared(&watches) {
            return watch;
        }
        watches.retain(|_, watch| watch.strong_count() > 0);
        let watch = Arc::new(started);
        watches.insert(watch.git_dir.clone(), Arc::downgrade(&watch));
        watch
    }

    /// Inspect the subscription's path, joining the inspection of it that
    /// still answers: one taken with Git's files as they are now (watched,
    /// any time ago; unwatched, less than a poll interval ago). Concurrent
    /// callers for one path share one run instead of each starting their own
    /// Git processes.
    async fn inspect(
        &self,
        state: &SubscriptionState,
    ) -> Result<CheckoutSourceContext, ChangeRequestError> {
        let cwd = state.cwd.as_path();
        let live = state.git.as_ref().is_some_and(|(git, _)| git.live());
        // Read before Git runs: a change made while it runs shows up as a
        // different fingerprint next time.
        let git_dir = state.identity.git_dir.clone();
        let inputs = tokio::task::spawn_blocking(move || GitFingerprint::read(&git_dir))
            .await
            .ok();
        let max_age = self.inner.timing.context_poll_interval;
        let run = {
            let mut inspections = self
                .inner
                .inspections
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            // Forget paths no subscription watches any more, once two
            // intervals old.
            inspections.retain(|_, inspection| {
                inspection.watch.strong_count() > 0 || inspection.started.elapsed() < max_age * 2
            });
            match inspections.get(cwd) {
                Some(inspection)
                    if inputs.is_some()
                        && inspection.inputs == inputs
                        && (live || inspection.started.elapsed() < max_age) =>
                {
                    inspection.run.clone()
                }
                _ => {
                    let lookup = self.inner.lookup.clone();
                    let path = cwd.to_owned();
                    let run = async move { lookup.inspect_checkout(&path).await }
                        .boxed()
                        .shared();
                    inspections.insert(
                        cwd.to_owned(),
                        Inspection {
                            started: Instant::now(),
                            inputs,
                            watch: state
                                .git
                                .as_ref()
                                .map_or_else(Weak::new, |(git, _)| Arc::downgrade(git)),
                            run: run.clone(),
                        },
                    );
                    run
                }
            }
        };
        run.await
    }

    fn acquire(&self, key: ChangeRequestCacheKey) -> DemandLease {
        let entry = {
            let mut entries = self
                .inner
                .entries
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            // Entries nobody watches go once stale.
            let now = Instant::now();
            entries.retain(|_, entry| {
                entry.demand.load(Ordering::Acquire) > 0 || !entry.is_stale(now)
            });
            let entry = entries
                .entry(key.clone())
                .or_insert_with(|| {
                    Arc::new(CacheEntry {
                        state: AsyncMutex::new(CacheState {
                            last_success: None,
                            next_refresh: Instant::now(),
                            failures: 0,
                            last_error: None,
                        }),
                        demand: AtomicUsize::new(0),
                        stale_at: Mutex::new(Instant::now()),
                    })
                })
                .clone();
            // Keep the demand transition under the entries lock. The final
            // lease drop uses that same lock before evicting, so a new watcher
            // cannot resurrect an entry after it was removed from the map.
            entry.demand.fetch_add(1, Ordering::AcqRel);
            entry
        };
        DemandLease {
            key,
            entry,
            inner: Arc::downgrade(&self.inner),
        }
    }

    async fn refresh(
        &self,
        key: &ChangeRequestCacheKey,
        source: &CheckoutSourceContext,
    ) -> (Option<CheckoutChangeRequestStatus>, Option<Instant>) {
        let entry = {
            let entries = self
                .inner
                .entries
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            entries.get(key).cloned().expect("cache entry acquired")
        };
        let mut state = entry.state.lock().await;
        let now = Instant::now();
        if now < state.next_refresh {
            let unsupported = state.last_error == Some(ChangeRequestError::UnsupportedRepository);
            return (
                state.last_success.clone(),
                (!unsupported).then_some(state.next_refresh),
            );
        }

        match self.inner.lookup.resolve_github_source(source).await {
            Ok(change_request) => {
                let on_default_branch = source.default_branch.as_deref()
                    == Some(source.branch.head_branch.as_str());
                let ttl = if on_default_branch {
                    self.inner.timing.default_branch_ttl
                } else if change_request.is_some() {
                    self.inner.timing.with_change_request_ttl
                } else {
                    self.inner.timing.without_change_request_ttl
                };
                let snapshot = CheckoutChangeRequestStatus {
                    checkout_id: String::new(),
                    device_id: self.inner.device_id.clone(),
                    cwd: source.checkout_root.to_string_lossy().into_owned(),
                    branch: source.branch.local_branch.clone(),
                    change_request,
                    updated_at: chrono::Utc::now(),
                };
                state.last_success = Some(snapshot.clone());
                state.next_refresh = Instant::now() + ttl;
                *entry.stale_at.lock().unwrap_or_else(PoisonError::into_inner) = state.next_refresh;
                state.failures = 0;
                state.last_error = None;
                (Some(snapshot), Some(state.next_refresh))
            }
            Err(error) => {
                state.failures = state.failures.saturating_add(1);
                // A remote GitHub can't be asked about (none, or another
                // host's) stays so until the checkout's Git files change,
                // which inspects it again (and keys another entry): nothing
                // to ask meanwhile.
                let unsupported = error == ChangeRequestError::UnsupportedRepository;
                let delay = if unsupported {
                    self.inner.timing.failure_max_backoff
                } else {
                    failure_backoff(
                        self.inner.timing.failure_initial_backoff,
                        self.inner.timing.failure_max_backoff,
                        state.failures,
                    )
                };
                state.next_refresh = Instant::now() + delay;
                *entry.stale_at.lock().unwrap_or_else(PoisonError::into_inner) = state.next_refresh;
                if state.last_error != Some(error) {
                    tracing::warn!(%error, "change request refresh failed; retaining last success");
                    state.last_error = Some(error);
                }
                (
                    state.last_success.clone(),
                    (!unsupported).then_some(state.next_refresh),
                )
            }
        }
    }

    pub fn shutdown(&self) {
        self.inner.shutdown.cancel();
    }
}

struct DemandLease {
    key: ChangeRequestCacheKey,
    entry: Arc<CacheEntry>,
    inner: Weak<Inner>,
}

impl Drop for DemandLease {
    fn drop(&mut self) {
        if self.entry.demand.fetch_sub(1, Ordering::AcqRel) != 1 {
            return;
        }
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        let mut entries = inner.entries.lock().unwrap_or_else(PoisonError::into_inner);
        let is_cached_entry = entries
            .get(&self.key)
            .is_some_and(|entry| Arc::ptr_eq(entry, &self.entry));
        // A fresh answer is kept for the next watch of it (`acquire` lets it
        // go once stale); a stale one goes now.
        if is_cached_entry
            && self.entry.demand.load(Ordering::Acquire) == 0
            && self.entry.is_stale(Instant::now())
        {
            entries.remove(&self.key);
        }
    }
}

struct SubscriptionState {
    service: CheckoutChangeRequests,
    cwd: PathBuf,
    identity: CheckoutIdentity,
    requested_branch: Option<String>,
    lease: Option<DemandLease>,
    last_emitted: Option<SemanticStatus>,
    /// The checkout's git dirs, watched while this lasts, and the events
    /// this subscription has seen.
    git: Option<(Arc<GitWatch>, watch::Receiver<u64>)>,
}

impl SubscriptionState {
    /// Until Git's files change (events on the git dirs, then a settle),
    /// the provider is due (`refresh_at`), or, while the git dirs aren't
    /// watched, a poll interval. False at shutdown.
    async fn wait(&mut self, refresh_at: Option<Instant>) -> bool {
        let timing = self.service.inner.timing;
        let shutdown = self.service.inner.shutdown.clone();
        let Some((git, changed)) = self.git.as_mut() else {
            return false;
        };
        let poll_at = (!git.live()).then(|| Instant::now() + timing.context_poll_interval);
        let wake_at = match (refresh_at, poll_at) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        let deadline = async {
            match wake_at {
                Some(at) => tokio::time::sleep_until(at).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            _ = shutdown.cancelled() => false,
            () = deadline => true,
            changed_ = changed.changed() => {
                if changed_.is_err() {
                    // (Can't be while the watch is held; were it gone, a
                    // poll interval, as unwatched.)
                    tokio::time::sleep(timing.context_poll_interval).await;
                    return !shutdown.is_cancelled();
                }
                // Let the burst finish: one look after it.
                while let Ok(Ok(())) =
                    tokio::time::timeout(timing.git_event_settle, changed.changed()).await
                {}
                !shutdown.is_cancelled()
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SemanticStatus {
    checkout_id: String,
    device_id: String,
    cwd: String,
    branch: String,
    change_request: Option<zeron_proto::ChangeRequestSummary>,
}

impl From<&CheckoutChangeRequestStatus> for SemanticStatus {
    fn from(status: &CheckoutChangeRequestStatus) -> Self {
        Self {
            checkout_id: status.checkout_id.clone(),
            device_id: status.device_id.clone(),
            cwd: status.cwd.clone(),
            branch: status.branch.clone(),
            change_request: status.change_request.clone(),
        }
    }
}

fn cache_key(checkout_id: &str, source: &CheckoutSourceContext) -> ChangeRequestCacheKey {
    let remote = source
        .branch
        .remote_url
        .as_deref()
        .and_then(parse_git_remote)
        .map(|remote| {
            format!(
                "{}/{}/{}",
                remote.host.to_ascii_lowercase(),
                remote.owner.to_ascii_lowercase(),
                remote.repository.to_ascii_lowercase()
            )
        })
        .unwrap_or_default();
    ChangeRequestCacheKey {
        scope: if remote.is_empty() { checkout_id.to_owned() } else { String::new() },
        branch: source.branch.local_branch.clone(),
        upstream_ref: source.branch.upstream_ref.clone().unwrap_or_default(),
        remote,
    }
}

fn failure_backoff(initial: Duration, maximum: Duration, failures: u32) -> Duration {
    let exponent = failures.saturating_sub(1).min(31);
    initial.saturating_mul(1_u32 << exponent).min(maximum)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::AtomicUsize;

    use async_trait::async_trait;
    use futures::StreamExt;
    use zeron_proto::{ChangeRequestState, ChangeRequestSummary};

    use super::*;
    use crate::source_control::BranchHeadContext;

    struct FakeLookup {
        source: Mutex<CheckoutSourceContext>,
        results: Mutex<VecDeque<Result<Option<ChangeRequestSummary>, ChangeRequestError>>>,
        resolves: AtomicUsize,
        inspects: AtomicUsize,
    }

    impl FakeLookup {
        fn new(
            source: CheckoutSourceContext,
            results: impl IntoIterator<Item = Result<Option<ChangeRequestSummary>, ChangeRequestError>>,
        ) -> Arc<Self> {
            Arc::new(Self {
                source: Mutex::new(source),
                results: Mutex::new(results.into_iter().collect()),
                resolves: AtomicUsize::new(0),
                inspects: AtomicUsize::new(0),
            })
        }

        fn set_source(&self, source: CheckoutSourceContext) {
            *self.source.lock().unwrap() = source;
        }

        fn resolve_count(&self) -> usize {
            self.resolves.load(Ordering::Acquire)
        }

        fn inspect_count(&self) -> usize {
            self.inspects.load(Ordering::Acquire)
        }
    }

    #[async_trait]
    impl CheckoutChangeRequestLookup for FakeLookup {
        async fn inspect_checkout(
            &self,
            _cwd: &Path,
        ) -> Result<CheckoutSourceContext, ChangeRequestError> {
            self.inspects.fetch_add(1, Ordering::AcqRel);
            // Yield so concurrent subscribers overlap the way real Git runs do.
            tokio::task::yield_now().await;
            Ok(self.source.lock().unwrap().clone())
        }

        async fn resolve_github_source(
            &self,
            _source: &CheckoutSourceContext,
        ) -> Result<Option<ChangeRequestSummary>, ChangeRequestError> {
            self.resolves.fetch_add(1, Ordering::AcqRel);
            self.results
                .lock()
                .unwrap()
                .pop_front()
                .expect("missing fake lookup result")
        }
    }

    fn source(branch: &str) -> CheckoutSourceContext {
        CheckoutSourceContext {
            checkout_root: PathBuf::from("/checkout"),
            branch: BranchHeadContext::resolve(
                branch,
                Some(&format!("origin/{branch}")),
                Some("origin"),
                Some("git@github.com:acme/zeron.git"),
            ),
            default_branch: Some("main".into()),
        }
    }

    fn pull_request(number: u64) -> ChangeRequestSummary {
        ChangeRequestSummary {
            provider: "github".into(),
            number,
            title: format!("Pull request {number}"),
            url: format!("https://github.com/acme/zeron/pull/{number}"),
            state: ChangeRequestState::Open,
            base_ref: "main".into(),
            head_ref: "feature/status".into(),
            signals: None,
        }
    }

    fn identity() -> CheckoutIdentity {
        CheckoutIdentity {
            id: "checkout-1".into(),
            root: PathBuf::from("/checkout"),
            git_dir: PathBuf::from("/checkout/.git"),
        }
    }

    fn service(lookup: Arc<FakeLookup>, timing: Timing) -> CheckoutChangeRequests {
        let temp = tempfile::tempdir().unwrap();
        CheckoutChangeRequests::with_timing(
            Repos::new(temp.path(), "device-1"),
            "device-1",
            lookup,
            timing,
        )
    }

    fn fast_timing() -> Timing {
        Timing {
            with_change_request_ttl: Duration::from_millis(15),
            without_change_request_ttl: Duration::from_millis(10),
            failure_initial_backoff: Duration::from_secs(1),
            failure_max_backoff: Duration::from_secs(2),
            context_poll_interval: Duration::from_millis(5),
            default_branch_ttl: Duration::from_secs(60),
            git_event_settle: Duration::from_millis(5),
        }
    }

    #[tokio::test]
    async fn requested_conversation_branch_overrides_live_checkout_branch() {
        let lookup = FakeLookup::new(source("main"), [Ok(Some(pull_request(90)))]);
        let service = service(lookup, Timing::default());
        let mut stream = service.watch_checkout_for_branch(
            PathBuf::from("/checkout"),
            identity(),
            Some("feature/sidebar".into()),
        );
        let snapshot = stream.next().await.expect("opening snapshot");
        assert_eq!(snapshot.branch, "feature/sidebar");
    }

    #[tokio::test]
    async fn two_subscribers_share_one_provider_query() {
        let lookup = FakeLookup::new(source("feature/status"), [Ok(Some(pull_request(90)))]);
        let service = service(lookup.clone(), Timing::default());
        let mut first = service.watch_checkout(PathBuf::from("/chat-a"), identity());
        let mut second = service.watch_checkout(PathBuf::from("/chat-b"), identity());

        let (first, second) = tokio::join!(first.next(), second.next());

        assert_eq!(first.unwrap().change_request.unwrap().number, 90);
        assert_eq!(second.unwrap().change_request.unwrap().number, 90);
        assert_eq!(lookup.resolve_count(), 1);
    }

    #[tokio::test]
    async fn subscribers_on_one_path_share_one_git_inspection() {
        let lookup = FakeLookup::new(source("feature/status"), [Ok(Some(pull_request(90)))]);
        let service = service(lookup.clone(), Timing::default());
        let mut first = service.watch_checkout(PathBuf::from("/checkout"), identity());
        let mut second = service.watch_checkout(PathBuf::from("/checkout"), identity());
        let mut third = service.watch_checkout(PathBuf::from("/checkout"), identity());

        let (first, second, third) = tokio::join!(first.next(), second.next(), third.next());

        assert!(first.is_some() && second.is_some() && third.is_some());
        assert_eq!(lookup.inspect_count(), 1);

        // Another path is a different checkout as far as Git knows.
        let mut other = service.watch_checkout(PathBuf::from("/checkout/sub"), identity());
        other.next().await.unwrap();
        assert_eq!(lookup.inspect_count(), 2);
    }

    #[tokio::test]
    async fn a_stalled_subscriber_does_not_hold_up_its_path() {
        let lookup = FakeLookup::new(source("feature/status"), [Ok(Some(pull_request(90)))]);
        let service = service(lookup.clone(), Timing::default());
        let mut stalled = service.watch_checkout(PathBuf::from("/checkout"), identity());
        // Start an inspection, then stop polling it partway through.
        assert!(futures::poll!(stalled.next()).is_pending());

        let mut live = service.watch_checkout(PathBuf::from("/checkout"), identity());
        let status = tokio::time::timeout(Duration::from_secs(1), live.next())
            .await
            .expect("the live subscriber finishes the shared inspection")
            .unwrap();

        assert_eq!(status.change_request.unwrap().number, 90);
        assert_eq!(lookup.inspect_count(), 1);
    }

    #[tokio::test]
    async fn branch_change_moves_to_a_new_cache_key() {
        let lookup = FakeLookup::new(
            source("feature/one"),
            [Ok(Some(pull_request(1))), Ok(Some(pull_request(2)))],
        );
        let service = service(lookup.clone(), fast_timing());
        let mut stream = service.watch_checkout(PathBuf::from("/checkout"), identity());
        let first = stream.next().await.unwrap();
        lookup.set_source(source("feature/two"));

        let second = tokio::time::timeout(Duration::from_millis(100), stream.next())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(first.branch, "feature/one");
        assert_eq!(second.branch, "feature/two");
        assert_eq!(second.change_request.unwrap().number, 2);
        assert_eq!(lookup.resolve_count(), 2);
    }

    #[tokio::test]
    async fn successful_none_clears_a_previous_pull_request() {
        let lookup = FakeLookup::new(
            source("feature/status"),
            [Ok(Some(pull_request(90))), Ok(None)],
        );
        let service = service(lookup, fast_timing());
        let mut stream = service.watch_checkout(PathBuf::from("/checkout"), identity());
        assert!(stream.next().await.unwrap().change_request.is_some());

        let cleared = tokio::time::timeout(Duration::from_millis(100), stream.next())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(cleared.change_request, None);
    }

    #[tokio::test]
    async fn unchanged_success_is_not_reemitted() {
        let lookup = FakeLookup::new(
            source("feature/status"),
            [
                Ok(Some(pull_request(90))),
                Ok(Some(pull_request(90))),
                Ok(Some(pull_request(91))),
            ],
        );
        let service = service(lookup.clone(), fast_timing());
        let mut stream = service.watch_checkout(PathBuf::from("/checkout"), identity());
        assert_eq!(
            stream.next().await.unwrap().change_request.unwrap().number,
            90
        );

        let changed = tokio::time::timeout(Duration::from_millis(100), stream.next())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(changed.change_request.unwrap().number, 91);
        assert_eq!(lookup.resolve_count(), 3);
    }

    #[tokio::test]
    async fn failure_keeps_the_last_success() {
        let lookup = FakeLookup::new(
            source("feature/status"),
            [
                Ok(Some(pull_request(90))),
                Err(ChangeRequestError::RateLimited),
            ],
        );
        let service = service(lookup.clone(), fast_timing());
        let mut stream = service.watch_checkout(PathBuf::from("/checkout"), identity());
        let first = stream.next().await.unwrap();

        assert!(
            tokio::time::timeout(Duration::from_millis(50), stream.next())
                .await
                .is_err()
        );
        assert_eq!(lookup.resolve_count(), 2);

        let mut later = service.watch_checkout(PathBuf::from("/checkout"), identity());
        let cached = later.next().await.unwrap();
        assert_eq!(cached.change_request, first.change_request);
        assert_eq!(lookup.resolve_count(), 2);
    }

    #[tokio::test]
    async fn authentication_and_rate_limit_keep_the_last_success() {
        let source = source("feature/status");
        let lookup = FakeLookup::new(
            source.clone(),
            [
                Ok(Some(pull_request(90))),
                Err(ChangeRequestError::Authentication),
                Err(ChangeRequestError::RateLimited),
            ],
        );
        let timing = Timing {
            with_change_request_ttl: Duration::from_millis(1),
            without_change_request_ttl: Duration::from_millis(1),
            failure_initial_backoff: Duration::from_millis(1),
            failure_max_backoff: Duration::from_millis(1),
            context_poll_interval: Duration::from_millis(1),
            default_branch_ttl: Duration::from_millis(1),
            git_event_settle: Duration::from_millis(1),
        };
        let service = service(lookup.clone(), timing);
        let key = cache_key("checkout-1", &source);
        let _lease = service.acquire(key.clone());

        let (first, _) = service.refresh(&key, &source).await;
        tokio::time::sleep(Duration::from_millis(2)).await;
        let (after_auth, _) = service.refresh(&key, &source).await;
        tokio::time::sleep(Duration::from_millis(2)).await;
        let (after_rate_limit, _) = service.refresh(&key, &source).await;

        assert_eq!(after_auth, first);
        assert_eq!(after_rate_limit, first);
        assert_eq!(lookup.resolve_count(), 3);
    }

    #[test]
    fn failure_backoff_grows_and_caps() {
        let initial = Duration::from_secs(20);
        let maximum = Duration::from_secs(160);
        assert_eq!(
            failure_backoff(initial, maximum, 1),
            Duration::from_secs(20)
        );
        assert_eq!(
            failure_backoff(initial, maximum, 2),
            Duration::from_secs(40)
        );
        assert_eq!(
            failure_backoff(initial, maximum, 3),
            Duration::from_secs(80)
        );
        assert_eq!(
            failure_backoff(initial, maximum, 4),
            Duration::from_secs(160)
        );
        assert_eq!(
            failure_backoff(initial, maximum, 30),
            Duration::from_secs(160)
        );
    }

    #[tokio::test]
    async fn dropping_last_subscriber_stops_refresh_work_and_a_stale_entry_goes() {
        let lookup = FakeLookup::new(
            source("feature/status"),
            [Ok(Some(pull_request(90))), Ok(Some(pull_request(91)))],
        );
        let service = service(lookup.clone(), fast_timing());
        let mut stream = service.watch_checkout(PathBuf::from("/checkout"), identity());
        stream.next().await.unwrap();
        drop(stream);

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(lookup.resolve_count(), 1, "nobody watching: nothing asked");

        // Stale by now: a later observer asks again (and the stale entry
        // nobody watched is gone).
        let mut renewed = service.watch_checkout(PathBuf::from("/checkout"), identity());
        let renewed_status = renewed.next().await.unwrap();
        assert_eq!(renewed_status.change_request.unwrap().number, 91);
        assert_eq!(lookup.resolve_count(), 2);
        assert_eq!(service.inner.entries.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_watch_that_goes_and_comes_back_within_its_ttl_asks_nothing() {
        let lookup = FakeLookup::new(source("feature/status"), [Ok(Some(pull_request(90)))]);
        let service = service(lookup.clone(), Timing::default());
        for _ in 0..20 {
            let mut stream = service.watch_checkout(PathBuf::from("/checkout"), identity());
            assert_eq!(stream.next().await.unwrap().change_request.unwrap().number, 90);
        }
        assert_eq!(lookup.resolve_count(), 1);
    }

    #[tokio::test]
    async fn checkouts_of_one_repository_and_branch_share_one_provider_query() {
        let lookup = FakeLookup::new(source("feature/status"), [Ok(Some(pull_request(90)))]);
        let service = service(lookup.clone(), Timing::default());
        let other = CheckoutIdentity {
            id: "checkout-2".into(),
            root: PathBuf::from("/clone"),
            git_dir: PathBuf::from("/clone/.git"),
        };
        let mut first = service.watch_checkout(PathBuf::from("/checkout"), identity());
        let first = first.next().await.unwrap();
        let mut second = service.watch_checkout(PathBuf::from("/clone"), other);
        let second = second.next().await.unwrap();
        assert_eq!(lookup.resolve_count(), 1);
        assert_eq!(second.change_request, first.change_request);
        // Each is told as its own checkout.
        assert_eq!((first.checkout_id.as_str(), second.checkout_id.as_str()), ("checkout-1", "checkout-2"));
    }

    #[tokio::test]
    async fn the_default_branch_is_asked_about_seldom() {
        let lookup = FakeLookup::new(source("main"), [Ok(None), Ok(None)]);
        let service = service(lookup.clone(), fast_timing());
        let mut stream = service.watch_checkout(PathBuf::from("/checkout"), identity());
        assert_eq!(stream.next().await.unwrap().change_request, None);
        // Many of the 10 ms no-PR TTLs later, still the one question.
        assert!(
            tokio::time::timeout(Duration::from_millis(80), stream.next())
                .await
                .is_err()
        );
        assert_eq!(lookup.resolve_count(), 1);
    }

    #[tokio::test]
    async fn shutdown_ends_an_active_stream() {
        let lookup = FakeLookup::new(source("feature/status"), [Ok(Some(pull_request(90)))]);
        let service = service(lookup, Timing::default());
        let mut stream = service.watch_checkout(PathBuf::from("/checkout"), identity());
        stream.next().await.unwrap();

        service.shutdown();

        assert!(stream.next().await.is_none());
    }

    /// A checkout on disk: its git dir with HEAD on `branch` and a config.
    fn checkout_on_disk(branch: &str) -> (tempfile::TempDir, CheckoutIdentity) {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let git_dir = root.join(".git");
        std::fs::create_dir_all(git_dir.join("refs/heads")).unwrap();
        std::fs::write(git_dir.join("HEAD"), format!("ref: refs/heads/{branch}\n")).unwrap();
        std::fs::write(git_dir.join("config"), "[core]\n").unwrap();
        let identity = CheckoutIdentity {
            id: "checkout-1".into(),
            root,
            git_dir,
        };
        (dir, identity)
    }

    /// Until the subscription's git dir watch is installed.
    async fn until_watched(service: &CheckoutChangeRequests) {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let live = service
                    .inner
                    .git_watches
                    .lock()
                    .unwrap()
                    .values()
                    .filter_map(Weak::upgrade)
                    .any(|git| git.live());
                if live {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the git dir's watch goes live");
    }

    /// A checkout left alone runs no Git: it is inspected again when its
    /// git files change (a branch checked out), not on a timer, however
    /// short the poll interval, and not for files that don't decide the
    /// answer (the index).
    #[tokio::test]
    async fn a_watched_checkout_runs_git_again_only_when_its_git_files_change() {
        let (_dir, identity) = checkout_on_disk("feature/one");
        let git_dir = identity.git_dir.clone();
        let lookup = FakeLookup::new(
            source("feature/one"),
            [Ok(Some(pull_request(1))), Ok(Some(pull_request(2)))],
        );
        let timing = Timing {
            with_change_request_ttl: Duration::from_secs(60),
            without_change_request_ttl: Duration::from_secs(60),
            git_event_settle: Duration::from_millis(20),
            ..fast_timing()
        };
        let service = service(lookup.clone(), timing);
        let mut stream = service.watch_checkout(identity.root.clone(), identity);
        assert_eq!(stream.next().await.unwrap().branch, "feature/one");
        until_watched(&service).await;
        // (The watch's own rescan as it went live settles first.)
        tokio::time::sleep(Duration::from_millis(200)).await;

        let inspected = lookup.inspect_count();
        assert!(
            tokio::time::timeout(Duration::from_millis(300), stream.next())
                .await
                .is_err()
        );
        assert_eq!(
            lookup.inspect_count(),
            inspected,
            "no Git while nothing changed"
        );

        std::fs::write(git_dir.join("index"), "staged").unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(500), stream.next())
                .await
                .is_err()
        );
        assert_eq!(
            lookup.inspect_count(),
            inspected,
            "the index decides nothing here"
        );

        lookup.set_source(source("feature/two"));
        std::fs::write(git_dir.join("HEAD"), "ref: refs/heads/feature/two\n").unwrap();
        let switched = tokio::time::timeout(Duration::from_secs(10), stream.next())
            .await
            .expect("inspected again as HEAD changed")
            .unwrap();
        assert_eq!(switched.branch, "feature/two");
        assert_eq!(switched.change_request.unwrap().number, 2);
    }

    /// A remote GitHub can't be asked about is asked nothing more until the
    /// checkout's Git files change: no retry timer.
    #[tokio::test]
    async fn an_unsupported_repository_is_not_asked_again_on_a_timer() {
        let (_dir, identity) = checkout_on_disk("feature/one");
        let lookup = FakeLookup::new(
            source("feature/one"),
            [Err(ChangeRequestError::UnsupportedRepository)],
        );
        let timing = Timing {
            // (A retry would come at the first backoff.)
            failure_initial_backoff: Duration::from_millis(10),
            failure_max_backoff: Duration::from_secs(60),
            ..fast_timing()
        };
        let service = service(lookup.clone(), timing);
        let mut stream = service.watch_checkout(identity.root.clone(), identity);
        // (Nothing to emit: driven by a task of its own.)
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Some(status) = stream.next().await {
                let _ = tx.send(status);
            }
        });
        until_watched(&service).await;
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(rx.try_recv().is_err());
        assert_eq!(lookup.resolve_count(), 1);
    }
}
