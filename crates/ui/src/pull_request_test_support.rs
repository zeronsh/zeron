//! Shared PR fixtures. Real wire transport, scripted service responses, and
//! progress driven by RPC notifications rather than iteration counts or sleeps.
use futures::future::BoxFuture;
use gpui::{Entity, TestAppContext};
use serde_json::Value;
use std::sync::Arc;
use zeron_rpc::{RpcClient, RpcError, RpcReply, RpcService};

type Reply = Result<RpcReply, RpcError>;

pub(super) struct ScriptedRpc {
    handler: Box<dyn Fn(String, Value) -> BoxFuture<'static, Reply> + Send + Sync>,
    changed: tokio::sync::Notify,
    completed: std::sync::atomic::AtomicUsize,
}

impl ScriptedRpc {
    pub(super) fn new<F, Fut>(handler: F) -> Arc<Self>
    where
        F: Fn(String, Value) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Reply> + Send + 'static,
    {
        Arc::new(Self {
            handler: Box::new(move |method, params| Box::pin(handler(method, params))),
            changed: tokio::sync::Notify::new(),
            completed: Default::default(),
        })
    }

    pub(super) fn completed(&self) -> usize {
        self.completed.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub(super) fn client(self: &Arc<Self>) -> RpcClient {
        zeron_rpc::memory_client(self.clone())
    }

    pub(super) fn settle(
        &self,
        cx: &mut TestAppContext,
        runtime: &tokio::runtime::Runtime,
        mut done: impl FnMut(&mut TestAppContext) -> bool,
    ) {
        loop {
            cx.run_until_parked();
            if done(cx) {
                return;
            }
            runtime.block_on(async {
                tokio::time::timeout(std::time::Duration::from_secs(5), self.changed.notified())
                    .await
                    .expect("PR fixture made no RPC progress");
                // Let the real transport deliver the completed reply before GPUI
                // consumes it. Pending scenarios signal when dispatch starts too.
                tokio::task::yield_now().await;
            });
        }
    }
}

#[async_trait::async_trait]
impl RpcService for ScriptedRpc {
    async fn handle(&self, method: &str, params: Value) -> Reply {
        self.changed.notify_one();
        let reply = (self.handler)(method.to_owned(), params).await;
        self.completed
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.changed.notify_one();
        reply
    }
}

pub(super) fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

pub(super) fn init(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_global(crate::theme::Theme::default()));
}

pub(super) fn settings(
    cx: &mut TestAppContext,
    settings: crate::settings::UiSettings,
) -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    cx.update(|cx| {
        cx.set_global(crate::theme::Theme::default());
        crate::settings::init(settings, directory.path(), cx);
    });
    directory
}

pub(super) fn state(
    cx: &mut gpui::App,
    client: Option<RpcClient>,
) -> Entity<crate::state::AppState> {
    use gpui::AppContext;
    cx.new(|_| {
        let mut state = crate::state::AppState::new();
        if let Some(client) = client {
            state.set_test_engine(crate::state::EngineHandle::from_test_client(client));
        }
        state
    })
}
