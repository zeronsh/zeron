use super::{
    credentials::{self, CredentialKey},
    desktop::Desktop,
    input::DesktopEvent,
    profiles::{Profile, ViewMode},
    view::EditForm,
};
use crate::settings::{self, SavePolicy};
use gpui::{prelude::*, *};
use gpui_base::input::InputState;
use std::{path::PathBuf, time::Duration};
use uuid::Uuid;
use zeron_rdp::{Command, ConnectConfig, Password, SessionHandle, SessionState, Snapshot};
#[derive(Clone)]
pub enum SurfaceEvent {
    Changed,
    OpenProfile(Uuid),
}
impl EventEmitter<SurfaceEvent> for RemoteDesktopSurface {}
pub struct RemoteDesktopSurface {
    pub(super) profile: Option<Profile>,
    pub(super) form: Option<EditForm>,
    pub(super) password: Entity<InputState>,
    pub(super) desktop: Entity<Desktop>,
    pub(super) snapshot: Snapshot,
    pub(super) notice: Option<String>,
    data_dir: PathBuf,
    generation: u64,
    session: Option<SessionHandle>,
    observer: Option<Task<()>>,
    credentials_task: Option<Task<()>>,
    cleanup_task: Option<Task<()>>,
    _input_subscription: Subscription,
    visible: bool,
    resize_paused: bool,
    geometry: Option<(u16, u16)>,
    clipboard_seq: u64,
    pending_clipboard: Option<(u64, Option<FocusHandle>)>,
    closed: bool,
}
impl RemoteDesktopSurface {
    pub fn new(data_dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let desktop = cx.new(|cx| Desktop::new(window, cx));
        let input_subscription =
            cx.subscribe_in(&desktop, window, |this, _, event, _, cx| match event {
                DesktopEvent::Input(input) if this.visible => {
                    this.send(Command::Input(input.clone()), cx)
                }
                DesktopEvent::Move(x, y) if this.visible => {
                    if let Some(handle) = &this.session {
                        handle.move_pointer(*x, *y);
                    }
                }
                DesktopEvent::Geometry(w, h) => {
                    this.geometry = Some((*w, *h));
                    this.request_resize();
                }
                DesktopEvent::Release => {
                    this.pending_clipboard = None;
                    if let Some(handle) = &this.session {
                        let _ = handle.send(Command::ReleaseAll);
                    }
                }
                _ => {}
            });
        Self {
            profile: None,
            form: None,
            password: cx.new(|cx| InputState::new(window, cx).masked(true)),
            desktop,
            snapshot: Snapshot::new(0),
            notice: None,
            data_dir,
            generation: 0,
            session: None,
            observer: None,
            credentials_task: None,
            cleanup_task: None,
            _input_subscription: input_subscription,
            visible: true,
            resize_paused: false,
            geometry: None,
            clipboard_seq: 0,
            pending_clipboard: None,
            closed: false,
        }
    }
    pub fn profile_id(&self) -> Option<Uuid> {
        self.profile.as_ref().map(|p| p.id)
    }
    pub fn title(&self) -> SharedString {
        self.profile
            .as_ref()
            .map(|p| format!("Desktop · {}", p.name))
            .unwrap_or_else(|| "Remote Desktop".into())
            .into()
    }
    pub fn endpoint(&self) -> Option<SharedString> {
        self.profile.as_ref().map(|p| p.endpoint().into())
    }
    pub fn open_profile(&mut self, id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        if self.closed || self.snapshot.state.is_running() {
            return;
        }
        let Some(profile) = settings::current(cx)
            .remote_desktop_profiles
            .into_iter()
            .find(|p| p.id == id)
        else {
            return;
        };
        self.profile = Some(profile);
        self.form = None;
        self.notice = None;
        cx.emit(SurfaceEvent::Changed);
        self.begin_connect(window, cx);
    }
    pub(super) fn begin_connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.closed || self.snapshot.state.is_running() {
            return;
        }
        let Some(id) = self.profile_id() else { return };
        let Some(profile) = settings::current(cx)
            .remote_desktop_profiles
            .into_iter()
            .find(|p| p.id == id)
        else {
            self.notice =
                Some("This connection was deleted. Choose another saved connection".into());
            cx.notify();
            return;
        };
        self.profile = Some(profile.clone());
        if let Err(e) = profile.validate() {
            self.notice = Some(e);
            cx.notify();
            return;
        }
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.session = None;
        self.observer = None;
        self.snapshot = Snapshot::new(generation);
        self.notice = None;
        self.desktop.update(cx, |d, cx| d.clear(window, cx));
        let password = self.password.read(cx).value().to_string();
        self.password
            .update(cx, |p, cx| p.set_value("", window, cx));
        if !password.is_empty() {
            self.start_session(profile, password, window, cx);
            return;
        }
        let key = CredentialKey::new(&self.data_dir, &profile);
        if !profile.remember_password
            || settings::current(cx)
                .remote_desktop_credential_cleanup
                .contains(&key.service)
        {
            self.notice = Some("Enter your remote password, then press Reconnect".into());
            cx.notify();
            return;
        }
        self.snapshot.state = SessionState::Connecting;
        let read = key.read(cx);
        let timer = cx.background_executor().timer(Duration::from_secs(10));
        self.credentials_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = match futures::future::select(read, timer).await {
                futures::future::Either::Left((result, _)) => result,
                futures::future::Either::Right(_) => {
                    Err(anyhow::anyhow!("System keyring did not respond"))
                }
            };
            let _ = this.update_in(cx, |this, window, cx| {
                if this.closed || this.generation != generation {
                    return;
                }
                if !settings::current(cx)
                    .remote_desktop_profiles
                    .iter()
                    .any(|p| p.id == profile.id && p.same_identity(&profile) && p.remember_password)
                {
                    this.snapshot.state = SessionState::Idle;
                    this.notice = Some(
                        "Connection settings changed. Reconnect to use the current settings".into(),
                    );
                    cx.notify();
                    return;
                }
                match result {
                    Ok(Some((account, bytes))) if key.matches(&account) => {
                        match String::from_utf8(bytes) {
                            Ok(password) => this.start_session(profile, password, window, cx),
                            Err(_) => {
                                this.snapshot.state = SessionState::Idle;
                                this.notice =
                                    Some("Saved password is invalid; enter it again".into());
                            }
                        }
                    }
                    Ok(_) => {
                        this.snapshot.state = SessionState::Idle;
                        this.notice =
                            Some("No saved password for this connection; enter it below".into());
                    }
                    Err(_) => {
                        this.snapshot.state = SessionState::Idle;
                        this.notice = Some(
                            "System keyring is unavailable. Enter a temporary password below"
                                .into(),
                        );
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
    fn start_session(
        &mut self,
        profile: Profile,
        password: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let generation = self.generation;
        let config = ConnectConfig {
            host: profile.host.clone(),
            port: profile.port,
            username: profile.username.clone(),
            keyboard_layout: profile.keyboard_layout,
            domain: profile.domain.clone(),
            password: Password::new(password),
            width: profile.desktop_width,
            height: profile.desktop_height,
            trusted_certificate_sha256: profile.trusted_certificate_sha256.clone(),
            timeout: zeron_rdp::CONNECT_TIMEOUT,
        };
        match zeron_rdp::connect(config, generation) {
            Ok(handle) => {
                handle.set_visible(self.visible);
                let mut receiver = handle.snapshots.clone();
                self.session = Some(handle);
                self.snapshot.state = SessionState::Connecting;
                self.desktop.update(cx, |d, _| d.mode = profile.view_mode);
                self.observer = Some(cx.spawn_in(window, async move |this, cx| {
                    loop {
                        let snapshot = receiver.borrow_and_update().clone();
                        let terminal = matches!(
                            snapshot.state,
                            SessionState::Failed(_) | SessionState::Disconnected
                        );
                        if this
                            .update_in(cx, |this, window, cx| {
                                this.apply_snapshot(snapshot, window, cx)
                            })
                            .is_err()
                        {
                            break;
                        }
                        if terminal || receiver.changed().await.is_err() {
                            break;
                        }
                    }
                }));
            }
            Err(error) => self.snapshot.state = SessionState::Failed(error),
        }
        cx.notify();
    }
    fn apply_snapshot(
        &mut self,
        mut snapshot: Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.closed || snapshot.generation != self.generation {
            return;
        }
        let state_changed = self.snapshot.state != snapshot.state;
        if let Some(pin) = &snapshot.accepted_pin
            && let Some(profile) = &mut self.profile
            && profile.trusted_certificate_sha256.as_ref() != Some(pin)
        {
            let id = profile.id;
            profile.trusted_certificate_sha256 = Some(pin.clone());
            settings::update(SavePolicy::Immediate, cx, |settings| {
                if let Some(saved) = settings
                    .remote_desktop_profiles
                    .iter_mut()
                    .find(|p| p.id == id && p.same_identity(profile))
                {
                    saved.trusted_certificate_sha256 = Some(pin.clone());
                }
            });
        }
        if let Some((id, result)) = &snapshot.clipboard {
            if *id == 0 {
                if let Err(e) = result {
                    self.notice = Some(e.to_string());
                }
            } else if self
                .pending_clipboard
                .as_ref()
                .is_some_and(|(pending, _)| pending == id)
            {
                let (_, focus) = self.pending_clipboard.take().unwrap();
                if self.visible && window.is_window_active() && window.focused(cx) == focus {
                    match result {
                        Ok(text) => {
                            cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
                            self.notice = Some("Remote text copied to your clipboard".into());
                        }
                        Err(e) => self.notice = Some(e.to_string()),
                    }
                }
            }
        }
        snapshot.clipboard = None;
        let connected = snapshot.state == SessionState::Connected;
        self.desktop.update(cx, |desktop, cx| {
            desktop.enabled = connected && self.visible && !snapshot.reactivating;
            if snapshot.reactivating {
                desktop.clear(window, cx);
            }
            if self.visible {
                if let Some(frame) = &snapshot.frame {
                    desktop.update_frame(frame, window, cx);
                }
                desktop.update_cursor(snapshot.cursor.clone(), window, cx);
            }
            if !connected && !snapshot.state.is_running() {
                desktop.release_capture(window, cx);
                desktop.clear(window, cx);
            }
        });
        // The watch mailbox owns the pending snapshot; the surface keeps only
        // GPUI's visible image and state, avoiding another retained full frame.
        snapshot.frame = None;
        self.snapshot = snapshot;
        self.request_resize();
        if state_changed {
            cx.emit(SurfaceEvent::Changed);
        }
        cx.notify();
    }
    pub(super) fn send(&mut self, command: Command, cx: &mut Context<Self>) {
        if let Some(handle) = &self.session
            && let Err(error) = handle.send(command)
        {
            self.notice = Some(error.to_string());
            cx.notify();
        }
    }
    pub fn disconnect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.desktop.update(cx, |d, cx| {
            d.release_capture(window, cx);
            d.enabled = false;
            d.clear(window, cx);
        });
        if let Some(handle) = &self.session {
            let _ = handle.send(Command::ReleaseAll);
            handle.disconnect();
        }
        self.pending_clipboard = None;
        self.generation = self.generation.wrapping_add(1);
        self.session = None;
        self.observer = None;
        self.credentials_task = None;
        self.snapshot = Snapshot::new(self.generation);
        self.snapshot.state = SessionState::Disconnected;
        self.password
            .update(cx, |p, cx| p.set_value("", window, cx));
        cx.notify();
    }
    pub fn stop(&mut self, cx: &mut Context<Self>) {
        self.generation = self.generation.wrapping_add(1);
        self.session = None;
        self.observer = None;
        self.credentials_task = None;
        self.pending_clipboard = None;
        self.desktop.update(cx, |desktop, cx| {
            desktop.enabled = false;
            desktop.release_input(cx);
        });
        self.snapshot = Snapshot::new(self.generation);
        self.snapshot.state = SessionState::Disconnected;
        cx.notify();
    }
    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.disconnect(window, cx);
        self.closed = true;
    }
    pub fn set_visible(&mut self, visible: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        if !visible {
            self.pending_clipboard = None;
        }
        if let Some(handle) = &self.session {
            handle.set_visible(visible);
        }
        self.desktop.update(cx, |d, cx| {
            d.enabled = visible
                && self.snapshot.state == SessionState::Connected
                && !self.snapshot.reactivating;
            if !visible {
                d.release_capture(window, cx);
                d.clear(window, cx);
            }
        });
        cx.notify();
    }
    pub fn set_resize_paused(&mut self, paused: bool) {
        if self.resize_paused != paused {
            self.resize_paused = paused;
            self.request_resize();
        }
    }
    fn request_resize(&self) {
        if let Some(handle) = &self.session {
            if self.visible
                && !self.resize_paused
                && self.profile.as_ref().is_some_and(|p| p.resize_remote)
            {
                if let Some((width, height)) = self.geometry {
                    let _ = handle.resize(width, height);
                }
            } else {
                handle.clear_resize();
            }
        }
    }
    pub(super) fn send_clipboard(&mut self, cx: &mut Context<Self>) {
        if !self.visible || !self.snapshot.capabilities.clipboard_text {
            self.notice = Some("Remote clipboard channel is unavailable".into());
            cx.notify();
            return;
        }
        match cx.read_from_clipboard().and_then(|item| item.text()) {
            Some(text) => {
                match zeron_rdp::clipboard::normalize_send(&text) {
                    Ok(_) => {
                        self.send(Command::SendClipboard(text), cx);
                        self.notice=Some("Clipboard text offered to the remote desktop. Paste it there to use it".into());
                    }
                    Err(e) => self.notice = Some(e.to_string()),
                }
            }
            None => self.notice = Some("Your clipboard does not contain text".into()),
        }
        cx.notify();
    }
    pub(super) fn copy_remote_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.visible || !self.snapshot.capabilities.clipboard_text {
            self.notice = Some("Remote clipboard channel is unavailable".into());
            cx.notify();
            return;
        }
        if self.pending_clipboard.is_some() {
            return;
        }
        self.clipboard_seq = self.clipboard_seq.wrapping_add(1).max(1);
        self.pending_clipboard = Some((self.clipboard_seq, window.focused(cx)));
        self.send(Command::RequestClipboard(self.clipboard_seq), cx);
        self.notice = Some("Requesting remote clipboard text…".into());
        cx.notify();
    }

    pub(super) fn toggle_resize(&mut self, cx: &mut Context<Self>) {
        if let Some(profile) = &mut self.profile {
            profile.resize_remote = !profile.resize_remote;
            let id = profile.id;
            let value = profile.resize_remote;
            settings::update(SavePolicy::Immediate, cx, |s| {
                if let Some(p) = s.remote_desktop_profiles.iter_mut().find(|p| p.id == id) {
                    p.resize_remote = value;
                }
            });
        }
        self.request_resize();
        cx.notify();
    }
    pub(super) fn pan_desktop(&mut self, x: f32, y: f32, cx: &mut Context<Self>) {
        self.desktop.update(cx, |d, cx| {
            let scale = d.transform.scale.max(f32::EPSILON);
            let max_x = (d.transform.width as f32 - f32::from(d.bounds.size.width) / scale).max(0.);
            let max_y =
                (d.transform.height as f32 - f32::from(d.bounds.size.height) / scale).max(0.);
            d.pan = (
                (d.pan.0 + x).clamp(0., max_x),
                (d.pan.1 + y).clamp(0., max_y),
            );
            cx.notify();
        });
    }

    pub(super) fn toggle_view(&mut self, cx: &mut Context<Self>) {
        self.desktop.update(cx, |d, cx| {
            d.mode = if d.mode == ViewMode::Fit {
                ViewMode::ActualSize
            } else {
                ViewMode::Fit
            };
            d.pan = (0., 0.);
            cx.notify();
        });
        if let Some(profile) = &mut self.profile {
            profile.view_mode = self.desktop.read(cx).mode;
            let id = profile.id;
            let mode = profile.view_mode;
            settings::update(SavePolicy::Immediate, cx, |s| {
                if let Some(p) = s.remote_desktop_profiles.iter_mut().find(|p| p.id == id) {
                    p.view_mode = mode;
                }
            });
        }
        cx.notify();
    }
    pub(super) fn save_profile(
        &mut self,
        connect: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(form) = &self.form else {
            return;
        };
        let (mut profile, password) = match form.value(cx) {
            Ok(value) => value,
            Err(error) => {
                self.notice = Some(error);
                cx.notify();
                return;
            }
        };
        let previous = settings::current(cx)
            .remote_desktop_profiles
            .into_iter()
            .find(|p| p.id == profile.id);
        if let Some(previous) = previous {
            if profile.invalidate_changed_identity(&previous)
                || (!profile.remember_password && previous.remember_password)
            {
                credentials::queue_delete(
                    CredentialKey::new(&self.data_dir, &previous).service,
                    cx,
                );
                self.retry_cleanup(window, cx);
            }
        }
        let id = profile.id;
        settings::update(SavePolicy::Immediate, cx, |s| {
            if let Some(old) = s.remote_desktop_profiles.iter_mut().find(|p| p.id == id) {
                *old = profile.clone();
            } else {
                s.remote_desktop_profiles.push(profile.clone());
            }
        });
        if profile.remember_password && !password.is_empty() {
            let key = CredentialKey::new(&self.data_dir, &profile);
            if !settings::current(cx)
                .remote_desktop_credential_cleanup
                .contains(&key.service)
            {
                let write = key.write(password.as_bytes(), cx);
                // Keep save completion separate from the cancellable connection lookup.
                let timer = cx.background_executor().timer(Duration::from_secs(10));
                cx.spawn_in(window,async move |this,cx|{
                    let failed=match futures::future::select(write,timer).await {
                        futures::future::Either::Left((result,_))=>result.is_err(),
                        futures::future::Either::Right(_)=>true,
                    };
                    if failed{let _=this.update_in(cx,|this,_,cx|{this.notice=Some("Connection saved, but the system keyring could not confirm its password. You can use a temporary password".into());cx.notify();});}
                }).detach();
            } else {
                self.notice=Some("Connection saved. Remove the previous keyring entry before remembering a new password".into());
            }
        }
        self.form = None;
        if self.profile_id() == Some(id) {
            self.profile = Some(profile);
        }
        if connect {
            self.password
                .update(cx, |field, cx| field.set_value(password, window, cx));
            cx.emit(SurfaceEvent::OpenProfile(id));
        }
        cx.emit(SurfaceEvent::Changed);
        cx.notify();
    }
    pub(super) fn delete_profile(&mut self, id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(profile) = settings::current(cx)
            .remote_desktop_profiles
            .into_iter()
            .find(|p| p.id == id)
        {
            credentials::queue_delete(CredentialKey::new(&self.data_dir, &profile).service, cx);
            settings::update(SavePolicy::Immediate, cx, |s| {
                s.remote_desktop_profiles.retain(|p| p.id != id)
            });
            self.retry_cleanup(window, cx);
            cx.notify();
        }
    }
    pub(super) fn retry_cleanup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let services = settings::current(cx).remote_desktop_credential_cleanup;
        let operations: Vec<_> = services
            .into_iter()
            .map(|service| {
                let task = credentials::delete(&service, cx);
                (service, task)
            })
            .collect();
        self.cleanup_task=Some(cx.spawn_in(window,async move |this,cx| {
            for (_,task) in operations {
                let timer=cx.background_executor().timer(Duration::from_secs(10));
                let result=match futures::future::select(task,timer).await {
                    futures::future::Either::Left((result,_))=>result,
                    futures::future::Either::Right((_,task))=>{
                        let _=this.update_in(cx,|this,_,cx|{this.notice=Some("System keyring did not respond. Password cleanup remains pending".into());cx.notify();});
                        task.await
                    }
                };
                if result.is_err(){let _=this.update_in(cx,|this,_,cx|{this.notice=Some("Could not remove a password from the system keyring. Use Retry removing saved credentials".into());cx.notify();});}
            }
        }));
    }
}
impl Render for RemoteDesktopSurface {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.render_content(window, cx)
    }
}

#[cfg(test)]
mod tests {
    use super::{RemoteDesktopSurface, SessionState, Snapshot};
    use gpui::{ClipboardItem, TestAppContext};
    #[gpui::test]
    fn remote_desktop_clipboard_discards_old_generation_hidden_and_focus_changed(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(crate::theme::Theme::default());
            cx.write_to_clipboard(ClipboardItem::new_string("keep local".into()));
        });
        let window = cx.add_window(|w, cx| RemoteDesktopSurface::new(std::env::temp_dir(), w, cx));
        window
            .update(cx, |view, w, cx| {
                view.generation = 2;
                for (generation, visible, change_focus) in
                    [(1, true, false), (2, false, false), (2, true, true)]
                {
                    view.visible = visible;
                    view.pending_clipboard = Some((7, w.focused(cx)));
                    let mut snapshot = Snapshot::new(generation);
                    snapshot.state = SessionState::Connected;
                    snapshot.clipboard = Some((7, Ok("remote text".into())));
                    let focus = cx.focus_handle();
                    if change_focus {
                        w.focus(&focus, cx);
                    }
                    view.apply_snapshot(snapshot, w, cx);
                    assert_eq!(
                        cx.read_from_clipboard()
                            .and_then(|item| item.text())
                            .as_deref(),
                        Some("keep local")
                    );
                }
            })
            .unwrap();
    }
}

#[cfg(feature = "remote-desktop-fixture")]
impl RemoteDesktopSurface {
    pub fn fixture_connect(
        &mut self,
        profile: Profile,
        password: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        settings::update(SavePolicy::Immediate, cx, |s| {
            s.remote_desktop_profiles = vec![profile.clone()]
        });
        self.password
            .update(cx, |p, cx| p.set_value(password, window, cx));
        self.open_profile(profile.id, window, cx);
    }
    pub fn fixture_has_frame(&self, cx: &App) -> bool {
        self.snapshot.state == SessionState::Connected && self.desktop.read(cx).dimensions.0 > 0
    }
}
