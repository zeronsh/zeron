//! Provider routing, storage and caching for the pull request board.
//!
//! The board speaks in scopes (a repository on a host) and change request
//! URLs; an adapter per provider turns those into CLI calls. Routing lives
//! here so RPC handlers never name a provider: the registry reads a wire
//! scope, finds the adapter serving its host, and that adapter alone decides
//! whether the path names one of its repositories. Adapters only fetch;
//! how fresh an answer must be, what is shared between callers, and what
//! is kept on disk is the registry's policy, the same for every provider.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use zeron_proto::{
    ChangeRequestComment, ChangeRequestFilter, ChangeRequestListItem, ChangeRequestPage,
    ChangeRequestPresentation, ChangeRequestScope, ChangeRequestSummary,
};

use crate::change_request_store::ChangeRequestStore;
use crate::source_control::gitlab::GitLabCli;
use crate::source_control::{ChangeRequestError, CheckoutSourceContext, GitHubCli};

/// What the board needs from one provider. Every method validates its own
/// inputs before they reach a CLI, whatever the caller already checked.
#[async_trait]
pub(crate) trait ChangeRequestBoardProvider: Send + Sync {
    /// Stamped on list items and change requests.
    fn provider_id(&self) -> &'static str;

    /// Whether this adapter answers for repositories on `host` (lowercase).
    /// When several do, the one registered first wins.
    fn serves_host(&self, host: &str) -> bool;

    /// `path` on `host` as a scope, when it names one of this provider's
    /// repositories. Only asked for hosts the adapter serves.
    fn scope(&self, host: &str, path: &str) -> Option<ChangeRequestScope>;

    /// The canonical form of `url`, when it names a change request this
    /// adapter can open. Caches key on it, so spellings share an entry.
    fn change_request_url(&self, url: &str) -> Option<String>;

    /// How clients name and show this provider on `host` (lowercase), sent
    /// with its pages and details so no client has to know it. A provider
    /// served from many hosts signs in, and serves images, on each.
    fn presentation(&self, host: &str) -> ChangeRequestPresentation;

    /// Fill a row's presentation fields (its `reference`, avatars) from
    /// what the row already says. Applied to every row served, stored ones
    /// included, so rows stored by an older build read the same.
    fn present(&self, _item: &mut ChangeRequestListItem) {}

    /// The account signed in on `host`, read locally, that stored pages are
    /// kept under. Namespaced by the adapter, so two providers never share
    /// a key. `None` when it can't be told, and then nothing is stored.
    async fn account_key(&self, host: &str) -> Option<String>;

    /// The provider's current name for `scope`, following renames; `None`
    /// when the provider answered that it has no such repository.
    async fn canonical_scope(
        &self,
        scope: &ChangeRequestScope,
    ) -> Result<Option<ChangeRequestScope>, ChangeRequestError>;

    /// One page of open change requests, newest updates first, read from
    /// the provider now.
    async fn list_page(
        &self,
        scope: &ChangeRequestScope,
        filter: ChangeRequestFilter,
        after: Option<&str>,
    ) -> Result<ChangeRequestPage, ChangeRequestError>;

    /// The detail, or with `diff` the patch, as the wire value. `head` and
    /// `base` pin the revision a client observed.
    async fn change_request(
        &self,
        url: &str,
        diff: bool,
        head: Option<&str>,
        base: Option<&str>,
    ) -> Result<serde_json::Value, ChangeRequestError>;

    /// Explicit user submission only. Never retried.
    async fn post_comment(
        &self,
        url: &str,
        body: &str,
    ) -> Result<ChangeRequestComment, ChangeRequestError>;

    /// The change request for `source`'s branch in `scope`, for the
    /// per-chat badge. An adapter that can't look branches up shows none.
    async fn find_for_branch(
        &self,
        _scope: &ChangeRequestScope,
        _source: &CheckoutSourceContext,
    ) -> Result<Option<ChangeRequestSummary>, ChangeRequestError> {
        Ok(None)
    }
}

/// Page cursors are short opaque tokens. Anything else is rejected before
/// it reaches a provider.
pub(crate) fn valid_page_cursor(cursor: &str) -> bool {
    (1..=256).contains(&cursor.len())
        && cursor
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"+/=_-:".contains(&b))
}

type RequestResult = Result<serde_json::Value, ChangeRequestError>;
type RequestFetch = futures::future::Shared<futures::future::BoxFuture<'static, RequestResult>>;

/// One adapter's recent answers. Simultaneous identical reads share one
/// provider call; distinct reads run side by side. A read runs detached, so
/// it completes and is cached even when every caller has gone away. Never
/// retry here.
#[derive(Clone, Default)]
pub(crate) struct RequestCache(Arc<tokio::sync::Mutex<RequestCacheState>>);

#[derive(Default)]
pub(crate) struct RequestCacheState {
    pub(crate) entries: Vec<(String, Instant, RequestResult)>,
    /// Reads awaiting the provider, joined by identical requests. The id
    /// tells a finishing read whether a write has replaced or dropped it
    /// meanwhile.
    pub(crate) in_flight: std::collections::HashMap<String, (u64, RequestFetch)>,
    next_fetch: u64,
    rate_limited_at: Option<Instant>,
}

impl RequestCache {
    async fn request<F>(&self, key: String, refresh: bool, fetch: F) -> RequestResult
    where
        F: std::future::Future<Output = RequestResult> + Send + 'static,
    {
        use futures::FutureExt;
        let mut cache = self.0.lock().await;
        if let Some(index) = cache.entries.iter().position(|entry| entry.0 == key) {
            let entry = cache.entries.remove(index);
            let ttl = match &entry.2 {
                Err(ChangeRequestError::RateLimited) => Duration::from_secs(15 * 60),
                // An explicit retry always reaches the provider again.
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
                let shared = self.0.clone();
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

    /// Drop every answer, and every read in flight, whose key starts with
    /// `prefix`: a read already in flight may predate a write.
    async fn evict(&self, prefix: &str) {
        let mut cache = self.0.lock().await;
        cache.entries.retain(|entry| !entry.0.starts_with(prefix));
        cache
            .in_flight
            .retain(|entry, _| !entry.starts_with(prefix));
    }

    #[cfg(test)]
    pub(crate) async fn state(&self) -> tokio::sync::MutexGuard<'_, RequestCacheState> {
        self.0.lock().await
    }
}

/// An adapter and its cache: rate limits and answers are per provider.
#[derive(Clone)]
struct Registered {
    adapter: Arc<dyn ChangeRequestBoardProvider>,
    requests: RequestCache,
}

impl Registered {
    fn new(adapter: Arc<dyn ChangeRequestBoardProvider>) -> Self {
        Self {
            adapter,
            requests: RequestCache::default(),
        }
    }
}

/// The board's adapters, picked by host or URL, with the store and caches
/// they share. Trait objects rather than an enum: adapters are injected in
/// tests like the process runner, and a self-hosted provider is data (its
/// host), not a new variant.
#[derive(Clone)]
pub(crate) struct ChangeRequestBoardProviders {
    adapters: Vec<Registered>,
    /// The board's on-disk first pages. `None` keeps everything in memory
    /// (tests).
    store: Option<ChangeRequestStore>,
}

impl ChangeRequestBoardProviders {
    /// The built-in adapters. Order settles hosts more than one adapter
    /// claims.
    fn built_in() -> Vec<Arc<dyn ChangeRequestBoardProvider>> {
        vec![Arc::new(GitHubCli::new()), Arc::new(GitLabCli::new())]
    }

    /// The built-in adapters, keeping first pages in `data_dir`.
    pub(crate) fn new(data_dir: &Path) -> Self {
        Self::with_adapters(Self::built_in(), Some(ChangeRequestStore::new(data_dir)))
    }

    /// The built-in adapters, keeping nothing on disk.
    pub(crate) fn without_store() -> Self {
        Self::with_adapters(Self::built_in(), None)
    }

    fn with_adapters(
        adapters: Vec<Arc<dyn ChangeRequestBoardProvider>>,
        store: Option<ChangeRequestStore>,
    ) -> Self {
        Self {
            adapters: adapters.into_iter().map(Registered::new).collect(),
            store,
        }
    }

    /// `adapters` in this order, storing first pages in `data_dir` if given.
    #[cfg(test)]
    pub(crate) fn for_tests(
        adapters: Vec<Arc<dyn ChangeRequestBoardProvider>>,
        data_dir: Option<&Path>,
    ) -> Self {
        Self::with_adapters(adapters, data_dir.map(ChangeRequestStore::new))
    }

    /// Put `adapter` first, replacing any adapter with its id.
    #[cfg(test)]
    pub(crate) fn with_adapter(mut self, adapter: Arc<dyn ChangeRequestBoardProvider>) -> Self {
        self.adapters
            .retain(|existing| existing.adapter.provider_id() != adapter.provider_id());
        self.adapters.insert(0, Registered::new(adapter));
        self
    }

    /// The answers cached for the adapter `provider_id`.
    #[cfg(test)]
    pub(crate) fn requests(&self, provider_id: &str) -> &RequestCache {
        &self
            .adapters
            .iter()
            .find(|registered| registered.adapter.provider_id() == provider_id)
            .expect("registered adapter")
            .requests
    }

    fn for_host(&self, host: &str) -> Option<&Registered> {
        let host = host.to_ascii_lowercase();
        self.adapters
            .iter()
            .find(|registered| registered.adapter.serves_host(&host))
    }

    /// Whether any adapter answers for `host`.
    pub(crate) fn serves_host(&self, host: &str) -> bool {
        self.for_host(host).is_some()
    }

    /// The adapter serving `scope`'s host, if it names that repository.
    fn for_scope(&self, scope: &ChangeRequestScope) -> Result<&Registered, ChangeRequestError> {
        self.for_host(scope.host())
            .filter(|registered| {
                registered
                    .adapter
                    .scope(scope.host(), scope.path())
                    .is_some()
            })
            .ok_or(ChangeRequestError::UnsupportedRepository)
    }

    /// The adapter that opens `url`, and the URL as it spells it.
    fn for_url(&self, url: &str) -> Result<(&Registered, String), ChangeRequestError> {
        self.adapters
            .iter()
            .find_map(|registered| {
                registered
                    .adapter
                    .change_request_url(url)
                    .map(|url| (registered, url))
            })
            .ok_or(ChangeRequestError::UnsupportedRepository)
    }

    /// `path` on `host` as a scope, when the adapter serving that host names
    /// a repository there.
    pub(crate) fn scope(&self, host: &str, path: &str) -> Option<ChangeRequestScope> {
        let host = host.to_ascii_lowercase();
        self.for_host(&host)?.adapter.scope(&host, path)
    }

    /// A scope as the wire spells it, when an adapter accepts it.
    pub(crate) fn parse(&self, wire: &str) -> Option<ChangeRequestScope> {
        let scope = ChangeRequestScope::parse(wire)?;
        self.scope(scope.host(), scope.path())
    }

    /// The provider's current name for `scope`, following renames. `None`
    /// when it can't be told within a few seconds (missing, private to
    /// another account, offline). Shares the board's cache and rate-limit
    /// pause.
    pub(crate) async fn canonical_scope(
        &self,
        scope: &ChangeRequestScope,
    ) -> Option<ChangeRequestScope> {
        let registered = self.for_scope(scope).ok()?;
        let key = format!("repository:{}", scope.to_string().to_ascii_lowercase());
        let (adapter, wanted) = (registered.adapter.clone(), scope.clone());
        let lookup = registered.requests.request(key, false, async move {
            Ok(match adapter.canonical_scope(&wanted).await? {
                Some(canonical) => serde_json::Value::String(canonical.path().to_owned()),
                None => serde_json::Value::Null,
            })
        });
        let value = tokio::time::timeout(Duration::from_secs(5), lookup)
            .await
            .ok()?
            .ok()?;
        registered.adapter.scope(scope.host(), value.as_str()?)
    }

    /// One page of `scope`'s open change requests. Only first pages are
    /// stored on disk: `cached` asks for the stored copy, whatever its age,
    /// so the board can show it at once and refresh behind it; without one
    /// the provider is read as usual. Every first page read is stored.
    pub(crate) async fn list_page(
        &self,
        scope: &ChangeRequestScope,
        filter: ChangeRequestFilter,
        after: Option<&str>,
        refresh: bool,
        cached: bool,
    ) -> Result<ChangeRequestPage, ChangeRequestError> {
        let registered = self.for_scope(scope)?;
        if after.is_some_and(|cursor| !valid_page_cursor(cursor)) {
            return Err(ChangeRequestError::Decode);
        }
        let repository = scope.to_string();
        let stored = match &self.store {
            Some(store) if after.is_none() => registered
                .adapter
                .account_key(scope.host())
                .await
                .map(|account| (store, account)),
            _ => None,
        };
        if cached
            && !refresh
            && let Some((store, account)) = &stored
            && let Some(page) = store.load(account, &repository, filter).await
        {
            return Ok(present(registered, scope, page));
        }
        let key = format!(
            "list:{}:{filter:?}:{}",
            repository.to_ascii_lowercase(),
            after.unwrap_or_default()
        );
        let (adapter, wanted, after_cursor) = (
            registered.adapter.clone(),
            scope.clone(),
            after.map(str::to_owned),
        );
        let value = registered
            .requests
            .request(key, refresh, async move {
                let page = adapter
                    .list_page(&wanted, filter, after_cursor.as_deref())
                    .await?;
                serde_json::to_value(page).map_err(|_| ChangeRequestError::Decode)
            })
            .await?;
        let page: ChangeRequestPage =
            serde_json::from_value(value).map_err(|_| ChangeRequestError::Decode)?;
        if let Some((store, account)) = stored {
            store.save(&account, &repository, filter, &page).await;
        }
        Ok(present(registered, scope, page))
    }

    /// How clients present `scope`'s provider, for a board that has no page
    /// to read it from (its first load failed).
    pub(crate) fn presentation(
        &self,
        scope: &ChangeRequestScope,
    ) -> Option<ChangeRequestPresentation> {
        self.for_scope(scope)
            .ok()
            .map(|registered| registered.adapter.presentation(scope.host()))
    }

    /// The detail, or with `diff` the patch. A board that observed a new
    /// revision (`head`, `base`) never joins an older read in flight or
    /// reuses its cached answer.
    pub(crate) async fn change_request(
        &self,
        url: &str,
        diff: bool,
        refresh: bool,
        head: Option<&str>,
        base: Option<&str>,
    ) -> Result<serde_json::Value, ChangeRequestError> {
        let (registered, url) = self.for_url(url)?;
        for oid in [head, base].into_iter().flatten() {
            if !zeron_proto::change_request_assessment::valid_head_oid(oid) {
                return Err(ChangeRequestError::Decode);
            }
        }
        let key = format!(
            "{diff}:{url}:{}:{}",
            head.unwrap_or_default(),
            base.unwrap_or_default()
        );
        // The detail is presented for the host its URL names.
        let host = reqwest::Url::parse(&url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
            .unwrap_or_default();
        let adapter = registered.adapter.clone();
        let (head, base) = (head.map(str::to_owned), base.map(str::to_owned));
        let mut value = registered
            .requests
            .request(key, refresh, async move {
                adapter
                    .change_request(&url, diff, head.as_deref(), base.as_deref())
                    .await
            })
            .await?;
        if let Some(detail) = value.as_object_mut().filter(|_| !diff) {
            let presentation = serde_json::to_value(registered.adapter.presentation(&host))
                .map_err(|_| ChangeRequestError::Decode)?;
            detail.insert("presentation".into(), presentation);
        }
        Ok(value)
    }

    /// The change request for `source`'s branch in `scope`, stamped with
    /// its adapter and features. A scope no adapter names has none. Not
    /// cached here: the badge service keeps its own pace.
    pub(crate) async fn find_for_branch(
        &self,
        scope: &ChangeRequestScope,
        source: &CheckoutSourceContext,
    ) -> Result<Option<ChangeRequestSummary>, ChangeRequestError> {
        let Ok(registered) = self.for_scope(scope) else {
            return Ok(None);
        };
        let mut found = registered.adapter.find_for_branch(scope, source).await?;
        if let Some(summary) = &mut found {
            summary.provider = registered.adapter.provider_id().into();
            summary.features = Some(registered.adapter.presentation(scope.host()).features);
        }
        Ok(found)
    }

    /// Post once; never retried. The cached detail is dropped whatever the
    /// outcome: a write may have landed even when its reply failed.
    pub(crate) async fn post_comment(
        &self,
        url: &str,
        body: &str,
    ) -> Result<ChangeRequestComment, ChangeRequestError> {
        let (registered, url) = self.for_url(url)?;
        let comment = registered.adapter.post_comment(&url, body).await;
        registered.requests.evict(&format!("false:{url}:")).await;
        comment
    }
}

/// `page` as clients see it: each row stamped with the adapter that listed
/// it, whatever it wrote, and presented by it, beside its presentation.
fn present(
    registered: &Registered,
    scope: &ChangeRequestScope,
    mut page: ChangeRequestPage,
) -> ChangeRequestPage {
    for item in &mut page.items {
        item.provider = registered.adapter.provider_id().into();
        registered.adapter.present(item);
    }
    page.presentation = Some(registered.adapter.presentation(scope.host()));
    page
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use chrono::DateTime;
    use zeron_proto::{ChangeRequestFeatures, ChangeRequestMergeability, ChangeRequestState};

    use super::*;

    /// A second provider, registered only in tests and answering from
    /// memory: projects nest under groups on `forge.test`, and its change
    /// requests read `~7`. Nothing it does reaches a process.
    pub(crate) struct Forge {
        hosts: Vec<String>,
        repositories: Mutex<HashMap<String, Vec<ChangeRequestListItem>>>,
        pub(crate) calls: AtomicUsize,
    }

    impl Forge {
        pub(crate) fn new() -> Self {
            Self::serving(&["forge.test"])
        }

        pub(crate) fn serving(hosts: &[&str]) -> Self {
            Self {
                hosts: hosts.iter().map(|host| (*host).to_owned()).collect(),
                repositories: Default::default(),
                calls: AtomicUsize::new(0),
            }
        }

        /// Open change requests `numbers` in `path`, newest first.
        pub(crate) fn with_repository(self, path: &str, numbers: &[u64]) -> Self {
            let items = numbers.iter().map(|number| item(path, *number)).collect();
            self.repositories
                .lock()
                .unwrap()
                .insert(path.to_ascii_lowercase(), items);
            self
        }

        pub(crate) fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    pub(crate) fn item(path: &str, number: u64) -> ChangeRequestListItem {
        ChangeRequestListItem {
            provider: "forge".into(),
            author: Default::default(),
            repository: format!("forge.test/{path}"),
            head_ref_oid: String::new(),
            ci: Default::default(),
            viewer_did_author: None,
            viewer_review_requested: None,
            number,
            title: format!("Change {number}"),
            url: format!("https://forge.test/{path}/-/changes/{number}"),
            state: ChangeRequestState::Open,
            is_draft: false,
            review_decision: Default::default(),
            additions: 1,
            deletions: 1,
            mergeability: ChangeRequestMergeability::Unknown,
            created_at: DateTime::UNIX_EPOCH,
            updated_at: DateTime::UNIX_EPOCH,
            reference: None,
        }
    }

    #[async_trait]
    impl ChangeRequestBoardProvider for Forge {
        fn provider_id(&self) -> &'static str {
            "forge"
        }

        fn serves_host(&self, host: &str) -> bool {
            self.hosts.iter().any(|served| served == host)
        }

        /// Groups nest at most four deep here, tighter than the wire allows.
        fn scope(&self, host: &str, path: &str) -> Option<ChangeRequestScope> {
            (path.split('/').count() <= 5)
                .then(|| ChangeRequestScope::new(host, path))
                .flatten()
        }

        fn change_request_url(&self, url: &str) -> Option<String> {
            let (path, number) = url
                .strip_prefix("https://forge.test/")?
                .split_once("/-/changes/")?;
            self.scope("forge.test", path)?;
            let number: u64 = number.parse().ok()?;
            Some(format!("https://forge.test/{path}/-/changes/{number}"))
        }

        fn presentation(&self, host: &str) -> ChangeRequestPresentation {
            ChangeRequestPresentation {
                provider_name: "Forge".into(),
                cli_name: "Forge CLI".into(),
                cli_command: "forge".into(),
                sign_in_command: format!("forge login {host}"),
                noun: "change".into(),
                media_hosts: vec![host.into()],
                media_rewrites: Vec::new(),
                features: ChangeRequestFeatures {
                    native_detail: true,
                    comments: false,
                    handoff: false,
                },
            }
        }

        fn present(&self, item: &mut ChangeRequestListItem) {
            item.reference = Some(format!("~{}", item.number));
        }

        async fn account_key(&self, host: &str) -> Option<String> {
            Some(format!("forge:{host}:devuser"))
        }

        async fn canonical_scope(
            &self,
            scope: &ChangeRequestScope,
        ) -> Result<Option<ChangeRequestScope>, ChangeRequestError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let repositories = self.repositories.lock().unwrap();
            Ok(repositories
                .contains_key(&scope.path().to_ascii_lowercase())
                .then(|| scope.clone()))
        }

        async fn list_page(
            &self,
            scope: &ChangeRequestScope,
            _filter: ChangeRequestFilter,
            _after: Option<&str>,
        ) -> Result<ChangeRequestPage, ChangeRequestError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let items = self
                .repositories
                .lock()
                .unwrap()
                .get(&scope.path().to_ascii_lowercase())
                .cloned()
                .ok_or(ChangeRequestError::CommandFailed)?;
            Ok(ChangeRequestPage {
                total_count: Some(items.len() as u64),
                items,
                next_cursor: None,
                fetched_at: Some(chrono::Utc::now()),
                presentation: None,
            })
        }

        async fn change_request(
            &self,
            url: &str,
            diff: bool,
            _head: Option<&str>,
            _base: Option<&str>,
        ) -> Result<serde_json::Value, ChangeRequestError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(if diff {
                serde_json::Value::String(String::new())
            } else {
                serde_json::json!({ "url": url })
            })
        }

        async fn post_comment(
            &self,
            _url: &str,
            body: &str,
        ) -> Result<ChangeRequestComment, ChangeRequestError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(ChangeRequestComment {
                body: body.into(),
                viewer_did_author: true,
                ..Default::default()
            })
        }

        /// The newest change request in the scope, whatever the branch.
        async fn find_for_branch(
            &self,
            scope: &ChangeRequestScope,
            source: &CheckoutSourceContext,
        ) -> Result<Option<ChangeRequestSummary>, ChangeRequestError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let repositories = self.repositories.lock().unwrap();
            Ok(repositories
                .get(&scope.path().to_ascii_lowercase())
                .and_then(|items| items.first())
                .map(|item| ChangeRequestSummary {
                    provider: String::new(),
                    number: item.number,
                    title: item.title.clone(),
                    url: item.url.clone(),
                    state: item.state,
                    base_ref: "main".into(),
                    head_ref: source.branch.head_branch.clone(),
                    reference: Some(format!("~{}", item.number)),
                    features: None,
                }))
        }
    }

    /// GitHub, unable to start a process, then the forge.
    fn registry(forge: Forge) -> (ChangeRequestBoardProviders, Arc<Forge>) {
        let forge = Arc::new(forge);
        let providers = ChangeRequestBoardProviders::for_tests(
            vec![Arc::new(GitHubCli::offline()), forge.clone()],
            None,
        );
        (providers, forge)
    }

    fn adapter_for(providers: &ChangeRequestBoardProviders, wire: &str) -> Option<&'static str> {
        let scope = providers.parse(wire)?;
        providers
            .for_scope(&scope)
            .ok()
            .map(|registered| registered.adapter.provider_id())
    }

    #[test]
    fn wire_scopes_route_by_host_and_each_adapter_names_its_paths() {
        let (providers, _) = registry(Forge::new());
        for (wire, provider, canonical) in [
            ("acme/zeron", Some("github"), "acme/zeron"),
            ("GitHub.com/acme/zeron", Some("github"), "acme/zeron"),
            (
                "Forge.test/team/platform/app",
                Some("forge"),
                "forge.test/team/platform/app",
            ),
            (
                "forge.test/a/b/c/d/e",
                Some("forge"),
                "forge.test/a/b/c/d/e",
            ),
            // GitHub repositories are never nested; the forge stops at four
            // groups, tighter than the wire.
            ("github.com/acme/zeron/extra", None, ""),
            ("forge.test/a/b/c/d/e/f", None, ""),
            // No adapter serves this host.
            ("other.test/team/app", None, ""),
            ("not a scope", None, ""),
        ] {
            assert_eq!(adapter_for(&providers, wire), provider, "{wire}");
            if provider.is_some() {
                assert_eq!(providers.parse(wire).unwrap().to_string(), canonical);
            }
        }
        assert_eq!(
            providers.scope("FORGE.test", "team/app"),
            ChangeRequestScope::parse("forge.test/team/app")
        );
        assert!(providers.serves_host("forge.test") && providers.serves_host("GitHub.com"));
        assert!(!providers.serves_host("other.test"));
    }

    #[test]
    fn hosts_more_than_one_adapter_claims_go_to_the_first_registered() {
        let claiming = || Arc::new(Forge::serving(&["forge.test", "github.com"]));
        let github_first = ChangeRequestBoardProviders::for_tests(
            vec![Arc::new(GitHubCli::offline()), claiming()],
            None,
        );
        assert_eq!(adapter_for(&github_first, "acme/zeron"), Some("github"));
        let forge_first = ChangeRequestBoardProviders::for_tests(
            vec![claiming(), Arc::new(GitHubCli::offline())],
            None,
        );
        assert_eq!(adapter_for(&forge_first, "acme/zeron"), Some("forge"));
        assert_eq!(
            adapter_for(&forge_first, "forge.test/team/app"),
            Some("forge")
        );
    }

    #[tokio::test]
    async fn unserved_scopes_and_urls_fail_before_any_adapter_call() {
        let (providers, forge) = registry(Forge::new().with_repository("team/app", &[7]));
        let unserved = ChangeRequestScope::new("other.test", "team/app").unwrap();
        let nested_github = ChangeRequestScope::new("github.com", "acme/zeron/extra").unwrap();
        for scope in [&unserved, &nested_github] {
            assert_eq!(
                providers
                    .list_page(scope, ChangeRequestFilter::All, None, false, false)
                    .await
                    .err(),
                Some(ChangeRequestError::UnsupportedRepository),
                "{scope}"
            );
            assert_eq!(providers.canonical_scope(scope).await, None);
        }
        for url in [
            "https://other.test/team/app/-/changes/7",
            "https://forge.test/a/b/c/d/e/f/-/changes/7",
            "not a url",
        ] {
            assert_eq!(
                providers
                    .change_request(url, false, false, None, None)
                    .await
                    .err(),
                Some(ChangeRequestError::UnsupportedRepository),
                "{url}"
            );
            assert_eq!(
                providers.post_comment(url, "hello").await.err(),
                Some(ChangeRequestError::UnsupportedRepository)
            );
        }
        assert_eq!(forge.calls(), 0);
        // A served scope reaches its adapter.
        let scope = providers.parse("forge.test/team/app").unwrap();
        let page = providers
            .list_page(&scope, ChangeRequestFilter::All, None, false, false)
            .await
            .unwrap();
        assert_eq!(page.items[0].url, "https://forge.test/team/app/-/changes/7");
        assert_eq!(forge.calls(), 1);
    }

    #[tokio::test]
    async fn pages_and_details_carry_their_adapters_presentation() {
        let (providers, _) = registry(Forge::new().with_repository("team/platform/app", &[7]));
        let scope = providers.parse("forge.test/team/platform/app").unwrap();
        let page = providers
            .list_page(&scope, ChangeRequestFilter::All, None, false, false)
            .await
            .unwrap();
        assert_eq!(page.items[0].reference.as_deref(), Some("~7"));
        assert_eq!(page.items[0].provider, "forge");
        let presentation = page.presentation.unwrap();
        assert_eq!(presentation.provider_name, "Forge");
        assert!(presentation.features.native_detail && !presentation.features.comments);
        // Presented for the scope's host, as pages, details and the
        // registry's own answer all are.
        assert_eq!(presentation.sign_in_command, "forge login forge.test");
        assert_eq!(providers.presentation(&scope), Some(presentation.clone()));
        let github = providers.parse("acme/zeron").unwrap();
        assert_eq!(
            providers
                .presentation(&github)
                .map(|presentation| presentation.provider_name),
            Some("GitHub".into())
        );
        let url = "https://forge.test/team/platform/app/-/changes/7";
        let detail: zeron_proto::ChangeRequestDetail = serde_json::from_value(
            providers
                .change_request(url, false, false, None, None)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(detail.presentation, Some(presentation));
        // A patch is text; it carries nothing else.
        assert_eq!(
            providers.change_request(url, true, false, None, None).await,
            Ok(serde_json::Value::String(String::new()))
        );
    }

    #[tokio::test]
    async fn the_registry_stores_and_caches_pages_for_every_adapter_alike() {
        use ChangeRequestFilter::All;
        let dir = tempfile::tempdir().unwrap();
        let forge = Arc::new(Forge::new().with_repository("team/platform/app", &[7]));
        let providers = ChangeRequestBoardProviders::for_tests(
            vec![Arc::new(GitHubCli::offline()), forge.clone()],
            Some(dir.path()),
        );
        let scope = providers.parse("forge.test/team/platform/app").unwrap();
        // Nothing stored yet: a cached read reaches the adapter and stores.
        let first = providers
            .list_page(&scope, All, None, false, true)
            .await
            .unwrap();
        assert_eq!(first.items[0].number, 7);
        // Identical reads share the answer; the adapter is asked once.
        providers
            .list_page(&scope, All, None, false, false)
            .await
            .unwrap();
        assert_eq!(forge.calls(), 1);
        // Stored under the adapter's own account key and the wire scope.
        let store = ChangeRequestStore::new(dir.path());
        let repository = "forge.test/team/platform/app";
        assert!(
            store
                .load("forge:forge.test:devuser", repository, All)
                .await
                .is_some()
        );
        assert!(store.load("devuser", repository, All).await.is_none());

        // A relaunch serves the stored page, presented, without the adapter
        // (which has since lost the repository).
        let relaunched_forge = Arc::new(Forge::new());
        let relaunched = ChangeRequestBoardProviders::for_tests(
            vec![relaunched_forge.clone()],
            Some(dir.path()),
        );
        let stored = relaunched
            .list_page(&scope, All, None, false, true)
            .await
            .unwrap();
        assert_eq!(stored.items[0].reference.as_deref(), Some("~7"));
        assert_eq!(stored.presentation.unwrap().provider_name, "Forge");
        assert_eq!(
            stored.fetched_at.map(|at| at.timestamp_millis()),
            first.fetched_at.map(|at| at.timestamp_millis())
        );
        assert_eq!(relaunched_forge.calls(), 0);
        // Refresh always reaches the adapter.
        assert_eq!(
            relaunched
                .list_page(&scope, All, None, true, false)
                .await
                .err(),
            Some(ChangeRequestError::CommandFailed)
        );
        assert_eq!(relaunched_forge.calls(), 1);
        // Cursors are checked before any adapter sees them.
        assert_eq!(
            providers
                .list_page(&scope, All, Some("x repo:other/x"), false, false)
                .await
                .err(),
            Some(ChangeRequestError::Decode)
        );

        // Canonical names share the cache too.
        for _ in 0..2 {
            assert_eq!(providers.canonical_scope(&scope).await, Some(scope.clone()));
        }
        assert_eq!(forge.calls(), 2);
        let missing = providers.parse("forge.test/team/missing").unwrap();
        assert_eq!(providers.canonical_scope(&missing).await, None);

        // Details are cached per canonical URL; a comment drops the detail.
        let url = "https://forge.test/team/platform/app/-/changes/07";
        for _ in 0..2 {
            providers
                .change_request(url, false, false, None, None)
                .await
                .unwrap();
        }
        assert_eq!(forge.calls(), 4);
        providers.post_comment(url, "hello").await.unwrap();
        providers
            .change_request(url, false, false, None, None)
            .await
            .unwrap();
        assert_eq!(forge.calls(), 6);
        assert!(
            providers
                .requests("forge")
                .state()
                .await
                .entries
                .iter()
                .any(|entry| entry.0 == "false:https://forge.test/team/platform/app/-/changes/7::")
        );
    }

    #[tokio::test]
    async fn branch_badges_route_through_the_registry_like_the_board() {
        use crate::source_control::{BranchHeadContext, ChangeRequestResolver};
        let (providers, forge) = registry(Forge::new().with_repository("team/platform/app", &[7]));
        let resolver = ChangeRequestResolver::with_providers(providers);
        let source = |remote: &str| CheckoutSourceContext {
            checkout_root: "/checkout".into(),
            branch: BranchHeadContext::resolve(
                "feature/change",
                Some("origin/feature/change"),
                Some("origin"),
                Some(remote),
            ),
            default_branch: Some("main".into()),
        };
        // A nested project's remote keeps its whole path and reaches the
        // forge, which stamps nothing itself: the registry names it.
        let nested = source("git@forge.test:team/platform/app.git");
        assert_eq!(nested.branch.repository.as_deref(), Some("platform/app"));
        let summary = resolver.resolve_source(&nested).await.unwrap().unwrap();
        assert_eq!(summary.provider, "forge");
        assert_eq!(summary.reference.as_deref(), Some("~7"));
        assert_eq!(summary.head_ref, "feature/change");
        assert!(summary.features.unwrap().native_detail);
        assert_eq!(forge.calls(), 1);
        // A host no adapter serves, or a path the forge refuses, has no
        // badge: not an error to back off from.
        for remote in [
            "https://other.test/team/app.git",
            "https://forge.test/a/b/c/d/e/f.git",
        ] {
            assert_eq!(resolver.resolve_source(&source(remote)).await, Ok(None));
        }
        assert_eq!(forge.calls(), 1);
        // A served provider's failure still is one.
        assert_eq!(
            resolver
                .resolve_source(&source("https://github.com/acme/zeron.git"))
                .await,
            Err(ChangeRequestError::CliUnavailable)
        );
    }
}
