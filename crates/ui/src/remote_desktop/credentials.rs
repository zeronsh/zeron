//! OS keyring integration. An identity hash is the account, so stale results
//! cannot supply credentials to a profile whose endpoint or account has changed.
use super::profiles::Profile;
use crate::settings::{self, SavePolicy};
use futures::{
    FutureExt,
    future::{LocalBoxFuture, Shared},
};
use gpui::{App, Global, Task};
use sha2::{Digest, Sha256};
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CredentialKey {
    pub service: String,
    pub account: String,
}
impl CredentialKey {
    pub fn new(data_dir: &Path, profile: &Profile) -> Self {
        let directory = std::fs::canonicalize(data_dir).unwrap_or_else(|_| data_dir.to_path_buf());
        let scope = format!(
            "{:x}",
            Sha256::digest(directory.as_os_str().as_encoded_bytes())
        );
        let identity = serde_json::to_vec(&(
            profile.host.to_ascii_lowercase(),
            profile.port,
            &profile.domain,
            &profile.username,
        ))
        .expect("string tuple serializes");
        Self {
            service: format!("zeron.remote-desktop.v1/{scope}/{}", profile.id),
            account: format!("{:x}", Sha256::digest(identity)),
        }
    }
    pub fn read(&self, cx: &mut App) -> Task<anyhow::Result<Option<(String, Vec<u8>)>>> {
        let service = self.service.clone();
        schedule(service.clone(), cx, move |cx| {
            provider(cx).read(&service, cx)
        })
    }
    pub fn write(&self, password: &[u8], cx: &mut App) -> Task<anyhow::Result<()>> {
        let key = self.clone();
        let password = password.to_vec();
        schedule(key.service.clone(), cx, move |cx| {
            provider(cx).write(&key.service, &key.account, &password, cx)
        })
    }
    pub fn matches(&self, account: &str) -> bool {
        self.account == account
    }
}

/// Persist deletion intent *before* asking the keyring. A failed delete remains
/// retryable after the profile is removed or the application is restarted.
pub fn queue_delete(service: String, cx: &mut App) {
    settings::update(SavePolicy::Immediate, cx, |s| {
        if !s.remote_desktop_credential_cleanup.contains(&service) {
            s.remote_desktop_credential_cleanup.push(service);
        }
    });
}
pub fn delete(service: &str, cx: &mut App) -> Task<anyhow::Result<()>> {
    let service = service.to_string();
    schedule(service.clone(), cx, move |cx| {
        let operation = provider(cx).delete(&service, cx);
        cx.spawn(async move |cx| {
            let result = operation.await;
            if result.is_ok() {
                cx.update(|cx| finish_delete(&service, cx));
            }
            result
        })
    })
}
pub fn finish_delete(service: &str, cx: &mut App) {
    settings::update(SavePolicy::Immediate, cx, |s| {
        s.remote_desktop_credential_cleanup
            .retain(|key| key != service)
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn remote_desktop_keyring_scope_tracks_identity_not_label() {
        let mut profile = Profile::default();
        profile.host = "server".into();
        let a = CredentialKey::new(Path::new("/tmp/rdp-a"), &profile);
        profile.name = "Renamed".into();
        assert_eq!(a, CredentialKey::new(Path::new("/tmp/rdp-a"), &profile));
        profile.username = "another".into();
        let b = CredentialKey::new(Path::new("/tmp/rdp-a"), &profile);
        assert_eq!(a.service, b.service);
        assert!(!a.matches(&b.account));
        assert_ne!(
            a.service,
            CredentialKey::new(Path::new("/tmp/rdp-b"), &profile).service
        );
        profile.id = uuid::Uuid::new_v4();
        assert_ne!(
            a.service,
            CredentialKey::new(Path::new("/tmp/rdp-a"), &profile).service
        );
    }
}

/// Small injectable OS boundary; tests can exercise a locked/missing keyring.
pub trait CredentialProvider {
    fn read(&self, service: &str, cx: &App) -> Task<anyhow::Result<Option<(String, Vec<u8>)>>>;
    fn write(
        &self,
        service: &str,
        account: &str,
        password: &[u8],
        cx: &App,
    ) -> Task<anyhow::Result<()>>;
    fn delete(&self, service: &str, cx: &App) -> Task<anyhow::Result<()>>;
}
pub struct CredentialProviderOverride(pub std::rc::Rc<dyn CredentialProvider>);
impl Global for CredentialProviderOverride {}
struct SystemKeyring;
impl CredentialProvider for SystemKeyring {
    fn read(&self, service: &str, cx: &App) -> Task<anyhow::Result<Option<(String, Vec<u8>)>>> {
        cx.read_credentials(service)
    }
    fn write(
        &self,
        service: &str,
        account: &str,
        password: &[u8],
        cx: &App,
    ) -> Task<anyhow::Result<()>> {
        cx.write_credentials(service, account, password)
    }
    fn delete(&self, service: &str, cx: &App) -> Task<anyhow::Result<()>> {
        cx.delete_credentials(service)
    }
}
fn provider(cx: &App) -> std::rc::Rc<dyn CredentialProvider> {
    cx.try_global::<CredentialProviderOverride>()
        .map(|p| p.0.clone())
        .unwrap_or_else(|| std::rc::Rc::new(SystemKeyring))
}
#[derive(Default)]
struct Queue {
    tails: std::collections::HashMap<String, (u64, Shared<LocalBoxFuture<'static, ()>>)>,
    next: u64,
    pending: usize,
}
impl Global for Queue {}
/// Serialize OS operations per service. Dropping a view cannot cancel an earlier
/// write and allow a later delete to overtake it, leaving an orphaned password.
fn schedule<T: 'static>(
    service: String,
    cx: &mut App,
    operation: impl FnOnce(&mut App) -> Task<anyhow::Result<T>> + 'static,
) -> Task<anyhow::Result<T>> {
    if !cx.has_global::<Queue>() {
        cx.set_global(Queue::default());
    }
    let queue = cx.global_mut::<Queue>();
    if queue.pending >= 32 {
        return Task::ready(Err(anyhow::anyhow!(
            "System keyring is busy; use a temporary password"
        )));
    }
    queue.pending += 1;
    queue.next += 1;
    let id = queue.next;
    let (done, wait) = futures::channel::oneshot::channel::<()>();
    let previous = queue.tails.insert(
        service.clone(),
        (id, wait.map(|_| ()).boxed_local().shared()),
    );
    let (result_tx, result_rx) = futures::channel::oneshot::channel();
    cx.spawn(async move |cx| {
        if let Some((_, previous)) = previous {
            previous.await;
        }
        let task = cx.update(operation);
        let result = task.await;
        let _ = result_tx.send(result);
        let _ = done.send(());
        cx.update(|cx| {
            let queue = cx.global_mut::<Queue>();
            queue.pending -= 1;
            if queue
                .tails
                .get(&service)
                .is_some_and(|(current, _)| *current == id)
            {
                queue.tails.remove(&service);
            }
        });
    })
    .detach();
    cx.spawn(async move |_| {
        result_rx
            .await
            .unwrap_or_else(|_| Err(anyhow::anyhow!("System keyring request stopped")))
    })
}

#[cfg(test)]
mod provider_tests {
    use super::*;
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
    };
    struct Fake {
        calls: Rc<RefCell<Vec<&'static str>>>,
        write_gate: RefCell<Option<futures::channel::oneshot::Receiver<()>>>,
        fail_delete: Cell<bool>,
        fail_read: Cell<bool>,
    }
    impl CredentialProvider for Fake {
        fn read(&self, _: &str, _: &App) -> Task<anyhow::Result<Option<(String, Vec<u8>)>>> {
            Task::ready(if self.fail_read.get() {
                Err(anyhow::anyhow!("locked"))
            } else {
                Ok(None)
            })
        }
        fn write(&self, _: &str, _: &str, _: &[u8], cx: &App) -> Task<anyhow::Result<()>> {
            self.calls.borrow_mut().push("write started");
            let gate = self.write_gate.borrow_mut().take();
            let calls = self.calls.clone();
            cx.spawn(async move |_| {
                if let Some(gate) = gate {
                    let _ = gate.await;
                }
                calls.borrow_mut().push("write finished");
                Ok(())
            })
        }
        fn delete(&self, _: &str, _: &App) -> Task<anyhow::Result<()>> {
            self.calls.borrow_mut().push("delete");
            Task::ready(if self.fail_delete.get() {
                Err(anyhow::anyhow!("locked"))
            } else {
                Ok(())
            })
        }
    }
    #[gpui::test]
    fn remote_desktop_keyring_serializes_detached_writes_and_retains_failed_cleanup(
        cx: &mut gpui::TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let (open, gate) = futures::channel::oneshot::channel();
        let fake = Rc::new(Fake {
            calls: Default::default(),
            write_gate: RefCell::new(Some(gate)),
            fail_delete: Cell::new(true),
            fail_read: Cell::new(false),
        });
        let key = CredentialKey::new(dir.path(), &Profile::default());
        cx.update(|cx| {
            settings::init(Default::default(), dir.path(), cx);
            cx.set_global(CredentialProviderOverride(fake.clone()));
        });
        let write = cx.update(|cx| key.write(b"test secret", cx));
        drop(write);
        cx.run_until_parked();
        let deletion = cx.update(|cx| {
            queue_delete(key.service.clone(), cx);
            delete(&key.service, cx)
        });
        cx.run_until_parked();
        assert_eq!(&*fake.calls.borrow(), &["write started"]);
        open.send(()).unwrap();
        cx.run_until_parked();
        assert!(deletion.now_or_never().unwrap().is_err());
        assert_eq!(
            &*fake.calls.borrow(),
            &["write started", "write finished", "delete"]
        );
        cx.update(|cx| {
            assert!(
                settings::current(cx)
                    .remote_desktop_credential_cleanup
                    .contains(&key.service)
            )
        });
        assert!(
            !std::fs::read_to_string(settings::UiSettings::path(dir.path()))
                .unwrap()
                .contains("test secret")
        );
        fake.fail_delete.set(false);
        let retry = cx.update(|cx| delete(&key.service, cx));
        cx.run_until_parked();
        assert!(retry.now_or_never().unwrap().is_ok());
        cx.update(|cx| {
            finish_delete(&key.service, cx);
            assert!(
                settings::current(cx)
                    .remote_desktop_credential_cleanup
                    .is_empty()
            );
            assert!(cx.global::<Queue>().tails.is_empty());
        });
        let missing = cx.update(|cx| key.read(cx));
        cx.run_until_parked();
        assert!(missing.now_or_never().unwrap().unwrap().is_none());
        fake.fail_read.set(true);
        let locked = cx.update(|cx| key.read(cx));
        cx.run_until_parked();
        assert!(locked.now_or_never().unwrap().is_err());
    }
}
