use super::*;
use zeron_proto::GitHubTarget;

impl Shell {
    pub(super) fn open_github_action(
        &mut self,
        action: &crate::github::OpenGitHub,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = GitHubTarget::from_url(&action.url) else {
            cx.open_url(&action.url);
            return;
        };
        if !self
            .state
            .read(cx)
            .chats
            .iter()
            .any(|chat| chat.id == action.source_session)
        {
            return;
        }
        if action.source_session != self.active_chat
            && !self.side_chat_open_here(&action.source_session, cx)
        {
            self.open_chat(action.source_session.clone(), cx);
            self.on_state_changed(&self.state.clone(), cx);
        }
        self.add_github_surface(action.source_session.clone(), target, window, cx);
    }

    pub(super) fn add_github_surface(
        &mut self,
        source: String,
        target: GitHubTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active_chat.is_empty() {
            return;
        }
        let key = self.panel_key(cx);
        let existing = self
            .right_tabs
            .get(&key)
            .into_iter()
            .flatten()
            .find_map(|surface| {
                let RightSurface::GitHub(id) = surface else {
                    return None;
                };
                let view = self.github_surfaces.get(id)?.read(cx);
                ((view.target == target
                    || (target.resolves_checkout()
                        && view.target.resource == zeron_proto::GitHubResource::Repository))
                    && view.source_session() == source)
                    .then_some(*id)
            });
        self.set_surfaces_open(true, cx);
        if let Some(id) = existing {
            self.set_right_active(RightSurface::GitHub(id), cx);
            self.focus_right_file_editor(RightSurface::GitHub(id), window, cx);
            return;
        }
        self.github_seq += 1;
        let id = self.github_seq;
        let view =
            cx.new(|cx| crate::github::GitHubSurface::new(self.state.clone(), source, target, cx));
        let owner = key.clone();
        let subscription =
            cx.subscribe_in(
                &view,
                window,
                move |this, _, event, window, cx| match event {
                    crate::github::GitHubEvent::Changed => cx.notify(),
                    crate::github::GitHubEvent::OpenLink(activation)
                        if this.panel_key(cx) == owner =>
                    {
                        if let crate::markdown::render::LinkOutcome::External(url) =
                            this.activate_session_link(activation, window, cx)
                        {
                            cx.open_url(&url);
                        }
                    }
                    _ => {}
                },
            );
        self.github_surfaces.insert(id, view);
        self.github_subs.insert(id, subscription);
        self.right_tabs
            .entry(key)
            .or_default()
            .push(RightSurface::GitHub(id));
        self.set_right_active(RightSurface::GitHub(id), cx);
        self.focus_right_file_editor(RightSurface::GitHub(id), window, cx);
    }

    pub(super) fn github_repository_target(&self, cx: &App) -> Option<GitHubTarget> {
        let state = self.state.read(cx);
        let space = state.selected_space_row()?;
        if !space.git_detected {
            return None;
        }
        space
            .repository_id
            .as_deref()
            .and_then(|repository| GitHubTarget::from_url(&format!("https://{repository}")))
            .or_else(|| {
                Some(GitHubTarget {
                    owner: String::new(),
                    repository: String::new(),
                    resource: zeron_proto::GitHubResource::Repository,
                })
            })
    }

    pub(super) fn open_github_repository(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(target) = self.github_repository_target(cx) {
            self.add_github_surface(self.active_chat.clone(), target, window, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::render::{LinkAction, LinkActivation, LinkOutcome, LinkTarget};
    use gpui::TestAppContext;

    #[gpui::test]
    fn native_github_links_reuse_tabs_and_preserve_explicit_external_routing(
        cx: &mut TestAppContext,
    ) {
        let directory = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            let mut settings = settings::UiSettings::default();
            settings.open_web_links_in_zeron = false;
            settings::init(settings, directory.path(), cx);
            crate::history::init(
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                cx,
            );
            gpui_base::init(cx);
            cx.set_global(Theme::default());
        });
        let window = cx.add_window(|_, cx| {
            let state = cx.new(|_| {
                let mut state = AppState::new();
                state.selected_chat = Some("owner".into());
                state.local_device_id = Some("local".into());
                state.chats = ["owner", "other"].into_iter().map(|id| serde_json::from_value(serde_json::json!({
                    "id": id, "deviceId": "local", "cwd": "C:/workspace", "archived": false, "createdAt": "2026-10-09T00:00:00Z"
                })).unwrap()).collect();
                state
            });
            Shell::new(state, EngineBootConfig { data_dir: directory.path().into(), ipc_port: 0, edge_url: String::new(), edge_token: None, org_id: None, workos_client_id: None, default_harness: zeron_proto::HarnessId::Mock }, cx)
        });
        window
            .update(cx, |shell, window, cx| {
                shell.active_chat = "owner".into();
                let mut activation = LinkActivation {
                    target: LinkTarget::new("PR", "https://github.com/zeronsh/zeron/pull/42"),
                    action: LinkAction::Primary,
                    source_session: Some("owner".into()),
                };
                for _ in 0..2 {
                    assert_eq!(
                        shell.activate_session_link(&activation, window, cx),
                        LinkOutcome::Internal
                    );
                }
                assert_eq!(shell.github_surfaces.len(), 1);
                assert!(shell.browsers.is_empty());
                let tab = shell.resolved_right_active(cx);
                assert!(matches!(tab, RightSurface::GitHub(_)));
                activation.action = LinkAction::External;
                assert_eq!(
                    shell.activate_session_link(&activation, window, cx),
                    LinkOutcome::External("https://github.com/zeronsh/zeron/pull/42".into())
                );
                activation.source_session = Some("stale".into());
                activation.action = LinkAction::Primary;
                assert_eq!(
                    shell.activate_session_link(&activation, window, cx),
                    LinkOutcome::Rejected
                );
                shell.close_right_surface(tab, window, cx);
                assert!(shell.github_surfaces.is_empty());
                assert!(shell.github_subs.is_empty());
                activation.source_session = Some("owner".into());
                for url in [
                    "https://github.com/zeronsh/zeron/commit/42926c802a837097e6a05de89d48ca7ade326658",
                    "https://github.com/zeronsh/zeron/blob/main/README.md",
                    "https://github.com/zeronsh/zeron/actions/runs/123",
                    "https://github.com/zeronsh",
                    "https://github.com/notifications",
                ] {
                    activation.target = LinkTarget::new("GitHub", url);
                    assert_eq!(shell.activate_session_link(&activation, window, cx), LinkOutcome::Internal, "{url}");
                    assert!(shell.browsers.is_empty(), "{url}");
                    shell.close_right_surface(shell.resolved_right_active(cx), window, cx);
                }
            })
            .unwrap();
    }
}
