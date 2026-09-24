//! Process-wide admission for chat data traffic. Registry/device control links
//! deliberately remain outside this small budget. Permits cover the resource's
//! entire lifetime, including response bodies and socket teardown.
use crate::SyncError;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;

#[derive(Clone, Copy)]
pub enum Priority {
    Interactive,
    Background,
}

pub struct Budget {
    sockets: Arc<Semaphore>,
    dials: Arc<Semaphore>,
    http: Arc<Semaphore>,
    background_http: Arc<Semaphore>,
    waiters: Arc<Semaphore>,
    limits: [usize; 3],
    paused_until: Mutex<Option<Instant>>,
    next_dial: Mutex<Option<Instant>>,
}

pub struct Permit {
    _resource: OwnedSemaphorePermit,
    _background: Option<OwnedSemaphorePermit>,
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BudgetStats {
    pub sockets: usize,
    pub socket_limit: usize,
    pub dials: usize,
    pub dial_limit: usize,
    pub http: usize,
    pub http_limit: usize,
    pub waiting: usize,
    pub resource_paused: bool,
}

impl Budget {
    pub fn new(sockets: usize, dials: usize, http: usize) -> Arc<Self> {
        assert!(sockets > 0 && dials > 0 && http > 0);
        Arc::new(Self {
            sockets: Arc::new(Semaphore::new(sockets)),
            dials: Arc::new(Semaphore::new(dials)),
            http: Arc::new(Semaphore::new(http)),
            background_http: Arc::new(Semaphore::new(http.saturating_sub(2).max(1))),
            waiters: Arc::new(Semaphore::new(128)),
            limits: [sockets, dials, http],
            paused_until: Mutex::new(None),
            next_dial: Mutex::new(None),
        })
    }

    async fn acquire(
        &self,
        resource: &Arc<Semaphore>,
        background: bool,
    ) -> Result<Permit, SyncError> {
        // Never allow callers to build an unbounded semaphore wait queue.
        let _waiting =
            self.waiters.clone().try_acquire_owned().map_err(|_| {
                SyncError::TemporarilyUnavailable("sync admission queue full".into())
            })?;
        let background = if background {
            Some(
                self.background_http
                    .clone()
                    .acquire_owned()
                    .await
                    .map_err(|_| SyncError::Closed)?,
            )
        } else {
            None
        };
        let resource = resource
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| SyncError::Closed)?;
        self.wait_for_resources().await;
        Ok(Permit {
            _resource: resource,
            _background: background,
        })
    }

    pub async fn socket(&self) -> Result<Permit, SyncError> {
        self.acquire(&self.sockets, false).await
    }
    pub async fn dial(&self) -> Result<Permit, SyncError> {
        let permit = self.acquire(&self.dials, false).await?;
        let at = {
            let mut next = self.next_dial.lock().unwrap_or_else(|e| e.into_inner());
            let at = next.unwrap_or_else(Instant::now).max(Instant::now());
            *next = Some(at + Duration::from_millis(50));
            at
        };
        tokio::time::sleep_until(at).await;
        self.wait_for_resources().await;
        Ok(permit)
    }
    pub async fn http(&self, priority: Priority) -> Result<Permit, SyncError> {
        self.acquire(&self.http, matches!(priority, Priority::Background))
            .await
    }
    /// Preserve OS error identity before transport layers stringify it.
    pub fn observe_error(&self, error: &(dyn std::error::Error + 'static)) {
        let mut current = Some(error);
        while let Some(error) = current {
            if let Some(io) = error.downcast_ref::<std::io::Error>() {
                #[cfg(unix)]
                let exhausted = matches!(io.raw_os_error(), Some(23 | 24));
                #[cfg(windows)]
                let exhausted = matches!(io.raw_os_error(), Some(4 | 10024));
                #[cfg(not(any(unix, windows)))]
                let exhausted = false;
                if exhausted {
                    *self.paused_until.lock().unwrap_or_else(|e| e.into_inner()) =
                        Some(Instant::now() + Duration::from_secs(2));
                    tracing::warn!("sync admission paused: process file descriptors exhausted");
                    return;
                }
            }
            current = error.source();
        }
    }

    async fn wait_for_resources(&self) {
        loop {
            let until = *self.paused_until.lock().unwrap_or_else(|e| e.into_inner());
            match until {
                Some(at) if at > Instant::now() => tokio::time::sleep_until(at).await,
                _ => return,
            }
        }
    }

    pub fn stats(&self) -> BudgetStats {
        BudgetStats {
            sockets: self.limits[0] - self.sockets.available_permits(),
            socket_limit: self.limits[0],
            dials: self.limits[1] - self.dials.available_permits(),
            dial_limit: self.limits[1],
            http: self.limits[2] - self.http.available_permits(),
            http_limit: self.limits[2],
            waiting: 128 - self.waiters.available_permits(),
            resource_paused: self
                .paused_until
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_some_and(|at| at > Instant::now()),
        }
    }
}

pub fn shared() -> &'static Arc<Budget> {
    static BUDGET: OnceLock<Arc<Budget>> = OnceLock::new();
    BUDGET.get_or_init(|| Budget::new(24, 4, 8))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    #[tokio::test(start_paused = true)]
    async fn resource_exhaustion_pauses_all_data_admission() {
        let budget = Budget::new(2, 2, 2);
        budget.observe_error(&std::io::Error::from_raw_os_error(24));
        let pending = tokio::spawn({
            let budget = budget.clone();
            async move { budget.socket().await }
        });
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(!pending.is_finished());
        tokio::time::advance(Duration::from_secs(2)).await;
        drop(pending.await.unwrap().unwrap());
        assert_eq!(budget.stats().sockets, 0);
    }
    #[tokio::test]
    async fn cancellation_releases_capacity_and_background_leaves_interactive_room() {
        let budget = Budget::new(1, 1, 3);
        let background = budget.http(Priority::Background).await.unwrap();
        let waiting = tokio::spawn({
            let budget = budget.clone();
            async move { budget.http(Priority::Background).await }
        });
        tokio::task::yield_now().await;
        let interactive = budget.http(Priority::Interactive).await.unwrap();
        assert_eq!(budget.stats().http, 2);
        waiting.abort();
        let _ = waiting.await;
        drop((background, interactive));
        assert_eq!(budget.stats().http, 0);
        assert_eq!(budget.stats().waiting, 0);
        let socket = budget.socket().await.unwrap();
        let waiting = tokio::spawn({
            let budget = budget.clone();
            async move { budget.socket().await }
        });
        tokio::task::yield_now().await;
        assert!(!waiting.is_finished());
        drop(socket);
        drop(waiting.await.unwrap().unwrap());
        assert_eq!(budget.stats().sockets, 0);
    }
}
