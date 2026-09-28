//! A failed refresh is not a new catalog. Keep the last successful response
//! for the same credential context, coalesce callers, and respect rate limits.
use crate::{CatalogFailure, CatalogFailureCode, HarnessError, ModelCatalog};
use std::{
    future::Future,
    time::{Duration, Instant},
};
use zeron_proto::Model;

#[derive(Default)]
pub(crate) struct Catalog {
    pub(crate) state: tokio::sync::Mutex<State>,
}

#[derive(Default)]
pub(crate) struct State {
    context: Option<[u8; 32]>,
    models: Option<Vec<Model>>,
    pub(crate) retry_at: Option<Instant>,
    error: Option<CatalogFailure>,
    completed_at: Option<Instant>,
}

impl Catalog {
    #[cfg(test)]
    pub(crate) async fn get<F, Fut, K>(
        &self,
        context: K,
        discover: F,
    ) -> Result<Vec<Model>, HarnessError>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<Vec<Model>, HarnessError>>,
        K: Fn() -> Result<[u8; 32], HarnessError>,
    {
        self.get_with(false, context, discover)
            .await
            .map(|r| r.models)
    }

    pub(crate) async fn get_with<F, Fut, K>(
        &self,
        force: bool,
        context: K,
        discover: F,
    ) -> Result<ModelCatalog, HarnessError>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<Vec<Model>, HarnessError>>,
        K: Fn() -> Result<[u8; 32], HarnessError>,
    {
        self.get_with_timeout(force, Duration::from_secs(20), context, discover)
            .await
    }

    pub(crate) async fn get_with_timeout<F, Fut, K>(
        &self,
        force: bool,
        timeout: Duration,
        context: K,
        mut discover: F,
    ) -> Result<ModelCatalog, HarnessError>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<Vec<Model>, HarnessError>>,
        K: Fn() -> Result<[u8; 32], HarnessError>,
    {
        let requested_at = Instant::now();
        let mut state = self.state.lock().await;
        let key = context()?;
        if state.context != Some(key) {
            *state = State {
                context: Some(key),
                ..State::default()
            };
        }
        if state.completed_at.is_some_and(|at| at >= requested_at)
            || (!force && state.retry_at.is_some_and(|at| Instant::now() < at))
        {
            return cached(&state).map(|models| ModelCatalog {
                models,
                source: "cache",
            });
        }
        let refresh = async {
            let mut error = CatalogFailure {
                code: CatalogFailureCode::Failed,
                message: "model catalog unavailable".into(),
            };
            for attempt in 0..3 {
                if attempt > 0 {
                    tokio::time::sleep(Duration::from_millis(250 * attempt)).await;
                }
                match discover().await {
                    Ok(models) if !models.is_empty() => return Ok(models),
                    Ok(_) => {
                        error = CatalogFailure {
                            code: CatalogFailureCode::Failed,
                            message: "Harness returned an empty model catalog".into(),
                        }
                    }
                    Err(e) => error = e.into(),
                }
                // Repeating a rate-limited or unauthenticated call cannot heal
                // it. A subsequent login changes the context and bypasses cooldown.
                if !error.code.allows_stale() || needs_cooldown(&error.message) {
                    break;
                }
            }
            Err(error)
        };
        let result = tokio::time::timeout(timeout, refresh)
            .await
            .unwrap_or_else(|_| {
                Err(CatalogFailure {
                    code: CatalogFailureCode::Timeout,
                    message: "Harness model discovery timed out".into(),
                })
            });
        // Never publish a response from a login that changed during the request.
        if context()? != key {
            *state = State::default();
            return Err(HarnessError::Protocol(
                "Harness credentials changed during model discovery; retry".into(),
            ));
        }
        let source = if result.is_ok() { "live" } else { "cache" };
        state.completed_at = Some(Instant::now());
        match result {
            Ok(models) => {
                state.models = Some(models);
                state.error = None;
                state.retry_at = Some(Instant::now() + Duration::from_secs(60));
            }
            Err(error) => {
                if !error.code.allows_stale() {
                    state.models = None;
                }
                tracing::warn!(code = %error.code, %error, retaining_catalog = state.models.is_some(), "Harness model refresh failed");
                let cooldown = if !error.code.allows_stale() || needs_cooldown(&error.message) {
                    60
                } else {
                    10
                };
                state.error = Some(error);
                state.retry_at = Some(Instant::now() + Duration::from_secs(cooldown));
            }
        }
        cached(&state).map(|models| ModelCatalog { models, source })
    }
}

fn needs_cooldown(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    [
        "rate limit",
        "rate-limit",
        "429",
        "unauthenticated",
        "unauthorized",
        "not authenticated",
        "isn't signed in",
        "auth_required",
        "401",
        "api key",
    ]
    .iter()
    .any(|pattern| lower.contains(pattern))
}

fn cached(state: &State) -> Result<Vec<Model>, HarnessError> {
    state.models.clone().ok_or_else(|| {
        state
            .error
            .clone()
            .unwrap_or_else(|| CatalogFailure {
                code: CatalogFailureCode::Failed,
                message: "Harness model catalog is unavailable; retry".into(),
            })
            .into()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    fn models(id: &str) -> Vec<Model> {
        vec![Model {
            id: id.into(),
            label: id.into(),
            description: None,
            reasoning_levels: vec![],
            options: vec![],
        }]
    }
    async fn expire(catalog: &Catalog) {
        catalog.state.lock().await.retry_at = None;
    }

    #[tokio::test]
    async fn empty_catalogs_are_failed_and_cannot_replace_last_good() {
        let cache = Catalog::default();
        let cold = cache
            .get(|| Ok([1; 32]), || async { Ok(vec![]) })
            .await
            .unwrap_err();
        assert_eq!(CatalogFailure::classify(&cold), CatalogFailureCode::Failed);
        cache
            .get_with(true, || Ok([1; 32]), || async { Ok(models("good")) })
            .await
            .unwrap();
        let stale = cache
            .get_with(true, || Ok([1; 32]), || async { Ok(vec![]) })
            .await
            .unwrap();
        assert_eq!(stale.source, "cache");
        assert_eq!(stale.models, models("good"));
        assert_eq!(
            cache.state.lock().await.error.as_ref().unwrap().code,
            CatalogFailureCode::Failed
        );
    }

    #[tokio::test]
    async fn stale_catalog_is_only_served_for_transient_failure_codes() {
        for (message, code) in [
            ("timed out", CatalogFailureCode::Timeout),
            ("offline", CatalogFailureCode::Failed),
            ("not logged in", CatalogFailureCode::AuthRequired),
            ("spawn ENOENT", CatalogFailureCode::MissingExecutable),
        ] {
            let cache = Catalog::default();
            cache
                .get(|| Ok([1; 32]), || async { Ok(models("good")) })
                .await
                .unwrap();
            let result = cache
                .get_with(
                    true,
                    || Ok([1; 32]),
                    || async { Err(HarnessError::Protocol(message.into())) },
                )
                .await;
            if code.allows_stale() {
                assert_eq!(result.unwrap().models, models("good"));
            } else {
                assert_eq!(CatalogFailure::classify(&result.unwrap_err()), code);
                let cached = cache
                    .get(|| Ok([1; 32]), || async { panic!("cooldown") })
                    .await;
                assert_eq!(CatalogFailure::classify(&cached.unwrap_err()), code);
            }
        }
    }

    #[tokio::test]
    async fn authentication_failures_cool_down_until_force_or_context_change() {
        let cache = Catalog::default();
        let calls = AtomicUsize::new(0);
        let discover = || async {
            calls.fetch_add(1, Ordering::Relaxed);
            Err(HarnessError::Protocol("401 unauthorized".into()))
        };
        assert!(
            cache
                .get_with(false, || Ok([1; 32]), discover)
                .await
                .is_err()
        );
        assert!(
            cache
                .get_with(false, || Ok([1; 32]), discover)
                .await
                .is_err()
        );
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert!(
            cache
                .get_with(true, || Ok([1; 32]), discover)
                .await
                .is_err()
        );
        assert!(
            cache
                .get_with(false, || Ok([2; 32]), discover)
                .await
                .is_err()
        );
        assert_eq!(calls.load(Ordering::Relaxed), 3);
    }

    #[tokio::test]
    async fn force_bypasses_cooldown_and_overlapping_force_calls_coalesce() {
        let cache = Catalog::default();
        let calls = AtomicUsize::new(0);
        let probe = || async {
            calls.fetch_add(1, Ordering::Relaxed);
            tokio::time::sleep(Duration::from_millis(5)).await;
            Ok(models("good"))
        };
        assert_eq!(
            cache
                .get_with(false, || Ok([1; 32]), probe)
                .await
                .unwrap()
                .source,
            "live"
        );
        assert_eq!(
            cache
                .get_with(false, || Ok([1; 32]), probe)
                .await
                .unwrap()
                .source,
            "cache"
        );
        let results = futures::future::join_all(
            (0..100).map(|_| cache.get_with(true, || Ok([1; 32]), probe)),
        )
        .await;
        assert!(results.iter().all(Result::is_ok));
        assert_eq!(calls.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn stress_10000_overlapping_requests_keep_catalog_through_100_outages() {
        let cache = Catalog::default();
        let calls = AtomicUsize::new(0);
        for cycle in 0..100 {
            expire(&cache).await;
            let expected = models(&format!("account-model-{cycle}"));
            assert_eq!(
                cache
                    .get(
                        || Ok([1; 32]),
                        || async {
                            calls.fetch_add(1, Ordering::Relaxed);
                            Ok(expected.clone())
                        }
                    )
                    .await
                    .unwrap(),
                expected
            );
            expire(&cache).await;
            let results = futures::future::join_all((0..100).map(|_| {
                cache.get(
                    || Ok([1; 32]),
                    || async {
                        calls.fetch_add(1, Ordering::Relaxed);
                        tokio::task::yield_now().await;
                        Err(HarnessError::Protocol("rate limit exceeded".into()))
                    },
                )
            }))
            .await;
            for result in results {
                assert_eq!(result.unwrap(), expected);
            }
        }
        assert_eq!(
            calls.load(Ordering::Relaxed),
            200,
            "one success and one failed refresh per wave"
        );
        println!(
            "stress: 10000 callers, 100 injected rate-limit outages, 200 probes, zero catalog collapses"
        );
    }

    #[tokio::test]
    async fn cold_failure_is_shared_and_never_fabricates_a_catalog() {
        let cache = Catalog::default();
        let calls = AtomicUsize::new(0);
        let results = futures::future::join_all((0..1000).map(|_| {
            cache.get(
                || Ok([1; 32]),
                || async {
                    calls.fetch_add(1, Ordering::Relaxed);
                    Err(HarnessError::Protocol("rate limit exceeded".into()))
                },
            )
        }))
        .await;
        assert!(results.iter().all(Result::is_err));
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        expire(&cache).await;
        assert_eq!(
            cache
                .get(|| Ok([1; 32]), || async { Ok(models("recovered")) })
                .await
                .unwrap(),
            models("recovered")
        );
    }

    #[tokio::test]
    async fn account_change_and_in_flight_login_never_leak_the_old_catalog() {
        let cache = Catalog::default();
        cache
            .get(|| Ok([1; 32]), || async { Ok(models("old-account")) })
            .await
            .unwrap();
        assert!(
            cache
                .get(
                    || Ok([2; 32]),
                    || async { Err(HarnessError::Protocol("invalid API key".into())) }
                )
                .await
                .is_err()
        );
        let key = AtomicUsize::new(3);
        assert!(
            cache
                .get(
                    || Ok([key.load(Ordering::Relaxed) as u8; 32]),
                    || async {
                        key.store(4, Ordering::Relaxed);
                        Ok(models("raced-login"))
                    }
                )
                .await
                .is_err()
        );
        assert_eq!(
            cache
                .get(|| Ok([4; 32]), || async { Ok(models("new-account")) })
                .await
                .unwrap(),
            models("new-account")
        );
    }

    #[tokio::test]
    async fn transient_and_empty_responses_retry_before_publishing() {
        let cache = Catalog::default();
        let calls = AtomicUsize::new(0);
        let result = cache
            .get(
                || Ok([1; 32]),
                || async {
                    match calls.fetch_add(1, Ordering::Relaxed) {
                        0 => Err(HarnessError::Protocol("temporary transport error".into())),
                        1 => Ok(vec![]),
                        _ => Ok(models("full")),
                    }
                },
            )
            .await
            .unwrap();
        assert_eq!(result, models("full"));
        assert_eq!(calls.load(Ordering::Relaxed), 3);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn real_subprocess_large_catalog_survives_malformed_and_failed_refreshes() {
        use crate::CursorHarness;
        use crate::Harness;
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("shim");
        std::fs::write(&script, "#!/bin/sh\ncd -- \"$(dirname -- \"$0\")\"\nprintf 'probe\\n' >> calls\ncat response\nexit \"$(cat status)\"\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let response = dir.path().join("response");
        let status = dir.path().join("status");
        let items: Vec<_> = (0..256).map(|i| serde_json::json!({"id":format!("model-{i}"),"displayName":format!("Model {i}"),"description":"x".repeat(4096)})).collect();
        let full = serde_json::json!({"ev":"models","items":items}).to_string();
        std::fs::write(&response, &full).unwrap();
        std::fs::write(&status, "0").unwrap();
        let harness = CursorHarness::new().with_executable(script);
        let baseline = harness.models().await.unwrap();
        assert_eq!(baseline.len(), 256);
        for cycle in 0..10 {
            expire(&harness.models_cache).await;
            std::fs::write(
                &response,
                match cycle % 3 {
                    0 => "{\"ev\":\"models\",\"items\":[",
                    1 => "{\"ev\":\"models\",\"items\":[]}",
                    _ => &full,
                },
            )
            .unwrap();
            std::fs::write(&status, if cycle % 3 == 2 { "1" } else { "0" }).unwrap();
            assert_eq!(harness.models().await.unwrap(), baseline);
            expire(&harness.models_cache).await;
            std::fs::write(&response, &full).unwrap();
            std::fs::write(&status, "0").unwrap();
            assert_eq!(harness.models().await.unwrap(), baseline);
        }
        assert_eq!(
            std::fs::read_to_string(dir.path().join("calls"))
                .unwrap()
                .lines()
                .count(),
            41
        );
        println!(
            "stress: 41 real subprocess probes, 10 malformed/empty/nonzero outages, full 256-model catalog preserved"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn hung_discovery_is_bounded_and_cold_failure_is_not_a_catalog() {
        let cache = Catalog::default();
        let started = tokio::time::Instant::now();
        let result = cache.get(|| Ok([1; 32]), std::future::pending).await;
        assert!(result.unwrap_err().to_string().contains("timed out"));
        assert_eq!(started.elapsed(), Duration::from_secs(20));
    }

    #[tokio::test]
    async fn cancelling_a_refresh_releases_the_lock_without_losing_good_data() {
        let cache = Arc::new(Catalog::default());
        cache
            .get(|| Ok([1; 32]), || async { Ok(models("good")) })
            .await
            .unwrap();
        expire(&cache).await;
        let (tx, rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn({
            let cache = cache.clone();
            async move {
                let tx = std::sync::Mutex::new(Some(tx));
                cache
                    .get(
                        || Ok([1; 32]),
                        || async {
                            let _ = tx.lock().unwrap().take().unwrap().send(());
                            std::future::pending().await
                        },
                    )
                    .await
            }
        });
        rx.await.unwrap();
        task.abort();
        let _ = task.await;
        assert_eq!(
            cache
                .get(
                    || Ok([1; 32]),
                    || async { Err(HarnessError::Protocol("rate limit".into())) }
                )
                .await
                .unwrap(),
            models("good")
        );
    }
}
