//! Settings → General: device-addressed git settings — where new worktrees
//! go, and whether agents' commits credit Zeron.

use gpui::{AnyElement, Context, Entity, Render, Task, Window, div, prelude::*, px};
use zeron_proto::{FolderListing, WorktreeSettings, WorktreeSettingsStatus};
use zeron_rpc::methods;

use super::search::{SearchAnchor, SettingId};
use super::widgets;
use crate::icons;
use crate::popover::Loadable;
use crate::state::AppState;
use crate::theme::Theme;
use zeron_proto::device_paths::{child_folder as child_path, parent_folder as folder_parent};

pub(super) struct WorktreeSettingsCard {
    state: Entity<AppState>,
    target: Option<String>,
    device_select: widgets::SelectState,
    settings: Loadable<WorktreeSettingsStatus>,
    /// The switch, ahead of the saved settings while a folder is still to be
    /// chosen.
    enabled: bool,
    error: Option<String>,
    saving: bool,
    task: Option<Task<()>>,
    browsing: bool,
    folders: Loadable<FolderListing>,
    folder_task: Option<Task<()>>,
    /// Whether agents' commits on the device credit Zeron.
    credit: Loadable<zeron_engine::commit_credit::CommitCreditStatus>,
    credit_saving: bool,
    credit_task: Option<Task<()>>,
}

impl WorktreeSettingsCard {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let mut card = Self {
            state,
            target: None,
            device_select: widgets::SelectState::default(),
            settings: Loadable::Idle,
            enabled: false,
            error: None,
            saving: false,
            task: None,
            browsing: false,
            folders: Loadable::Idle,
            folder_task: None,
            credit: Loadable::Idle,
            credit_saving: false,
            credit_task: None,
        };
        card.load(None, cx);
        card
    }

    fn render_device_switcher(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        use crate::icons::{self, icon};
        let (mut devices, local_id) = {
            let s = self.state.read(cx);
            (s.devices.clone(), s.local_device_id.clone())
        };
        devices.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        let effective = self.target.clone().or_else(|| local_id.clone());
        let platform_glyph = |platform: &str| match platform {
            "macos" | "darwin" => icons::LAPTOP,
            "ios" | "android" => icons::SMARTPHONE,
            _ => icons::MONITOR,
        };
        // Local device = no passthrough (calls stay direct).
        let mut targets: Vec<Option<String>> = Vec::new();
        let mut options = Vec::new();
        for device in &devices {
            let is_local = local_id.as_deref() == Some(device.id.as_str());
            let glyph = platform_glyph(&device.platform);
            let muted = theme.text_muted;
            let option = widgets::SelectOption::new(device.name.clone()).leading(move || {
                icon(glyph)
                    .size(px(16.0))
                    .flex_none()
                    .text_color(muted)
                    .into_any_element()
            });
            options.push(if is_local {
                option.detail("You")
            } else {
                option
            });
            targets.push((!is_local).then(|| device.id.clone()));
        }
        let selected = match devices
            .iter()
            .position(|d| Some(d.id.as_str()) == effective.as_deref())
        {
            Some(ix) => ix,
            // Not registered (yet): keep the current target reachable.
            None => {
                let muted = theme.text_muted;
                options.push(widgets::SelectOption::new("This device").leading(move || {
                    icon(icons::LAPTOP)
                        .size(px(16.0))
                        .flex_none()
                        .text_color(muted)
                        .into_any_element()
                }));
                targets.push(self.target.clone());
                options.len() - 1
            }
        };
        widgets::select(
            "worktrees-device-switcher",
            "Device",
            theme,
            |page: &mut Self| &mut page.device_select,
        )
        .options(options, selected)
        .menu_width(260.0)
        .heading("Devices")
        .open_rightward()
        .on_select(move |page, ix, _, cx| {
            if let Some(target) = targets.get(ix) {
                widgets::close_select(page, |card: &mut Self| &mut card.device_select, cx);
                if page.target != *target {
                    page.load(target.clone(), cx);
                }
            }
        })
        .render(&self.device_select, cx)
        .into_any_element()
    }

    fn load(&mut self, target: Option<String>, cx: &mut Context<Self>) {
        self.task = None;
        self.folder_task = None;
        self.target = target;
        self.settings = Loadable::Loading;
        self.enabled = false;
        self.saving = false;
        self.error = None;
        self.browsing = false;
        self.folders = Loadable::Idle;
        self.request(None, cx);
        self.credit = Loadable::Loading;
        self.request_credit(None, cx);
    }

    /// Read (or, with `save`, set) the device's commit credit.
    fn request_credit(&mut self, save: Option<bool>, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.credit = Loadable::Error("Device is not connected".into());
            cx.notify();
            return;
        };
        self.credit_saving = save.is_some();
        let (method, mut params) = match save {
            Some(enabled) => (
                methods::SET_COMMIT_CREDIT,
                serde_json::json!({ "enabled": enabled }),
            ),
            None => (methods::GET_COMMIT_CREDIT, serde_json::json!({})),
        };
        params["targetDeviceId"] = serde_json::json!(self.target);
        self.credit_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(method, params)
                .await
                .map_err(|error| match error {
                    zeron_rpc::RpcError::UnknownMethod(_) => {
                        "Update Zeron on the selected device to change this.".to_string()
                    }
                    _ => error.to_string(),
                })
                .and_then(|value| {
                    serde_json::from_value::<zeron_engine::commit_credit::CommitCreditStatus>(value)
                        .map_err(|error| error.to_string())
                });
            this.update(cx, |card, cx| {
                card.credit_saving = false;
                match result {
                    Ok(status) => card.credit = Loadable::Ready(status),
                    Err(error) => card.credit = Loadable::Error(error),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn toggle_credit(&mut self, cx: &mut Context<Self>) {
        let Some(status) = self.credit.ready() else {
            return;
        };
        if status.cloud || self.credit_saving {
            return;
        }
        let next = !status.enabled;
        self.request_credit(Some(next), cx);
    }

    fn render_credit_row(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let status = self.credit.ready();
        let enabled = status.is_some_and(|s| s.enabled);
        let interactive = status.is_some_and(|s| !s.cloud) && !self.credit_saving;
        let meta = match &self.credit {
            Loadable::Ready(s) if s.cloud => "Always on for Cloud".to_string(),
            Loadable::Ready(_) => "Agents' commits add zeronsh[bot] as a co-author".to_string(),
            Loadable::Error(error) => error.clone(),
            _ => "Loading…".to_string(),
        };
        widgets::card_row(theme, false)
            .child(
                div()
                    .flex_1()
                    .min_w(px(160.0))
                    .child(widgets::row_title(theme, "Credit Zeron in commits"))
                    .child(widgets::meta_line(
                        theme,
                        vec![div().truncate().child(meta).into_any_element()],
                    )),
            )
            .child(
                widgets::toggle_switch(theme, enabled, "commit-credit")
                    .id("commit-credit-toggle")
                    .flex_none()
                    .role(gpui::Role::Switch)
                    .aria_label("Credit Zeron in commits")
                    .aria_toggled(if enabled {
                        gpui::Toggled::True
                    } else {
                        gpui::Toggled::False
                    })
                    .when(interactive, |el| {
                        el.cursor_pointer()
                            .tab_index(0)
                            .focus_visible(|s| s.border_2().border_color(theme.accent))
                            .on_click(cx.listener(|card, _, _, cx| card.toggle_credit(cx)))
                    })
                    .when(!interactive, |el| el.opacity(0.5)),
            )
            .search_anchor(SettingId::CommitCredit)
            .into_any_element()
    }

    fn request(&mut self, save: Option<WorktreeSettings>, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.settings = Loadable::Error("Device is not connected".into());
            cx.notify();
            return;
        };
        self.saving = save.is_some();
        let saving = self.saving;
        let method = if saving {
            methods::SET_WORKTREE_SETTINGS
        } else {
            methods::GET_WORKTREE_SETTINGS
        };
        let mut params = save
            .map(|settings| serde_json::to_value(settings).unwrap())
            .unwrap_or_else(|| serde_json::json!({}));
        params["targetDeviceId"] = serde_json::json!(self.target);
        self.error = None;
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(method, params)
                .await
                .map_err(|error| match error {
                    zeron_rpc::RpcError::UnknownMethod(_) => {
                        "Update Zeron on the selected device to configure worktree locations."
                            .to_string()
                    }
                    _ => error.to_string(),
                })
                .and_then(|value| {
                    serde_json::from_value::<WorktreeSettingsStatus>(value)
                        .map_err(|error| error.to_string())
                });
            this.update(cx, |card, cx| {
                card.saving = false;
                match result {
                    Ok(status) => {
                        card.enabled = status.settings.use_custom_directory;
                        card.settings = Loadable::Ready(status);
                        card.browsing = false;
                    }
                    Err(error) if saving => card.error = Some(error),
                    Err(error) => card.settings = Loadable::Error(error),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn saved(&self) -> Option<&WorktreeSettings> {
        match &self.settings {
            Loadable::Ready(status) if status.environment_override.is_none() => {
                Some(&status.settings)
            }
            _ => None,
        }
    }

    fn save(&mut self, settings: WorktreeSettings, cx: &mut Context<Self>) {
        if self.saving || self.saved().is_none() {
            return;
        }
        self.browsing = false;
        self.request(Some(settings), cx);
    }

    /// Turning on reuses the remembered folder, or waits for Browse to pick
    /// one; turning off keeps the folder for next time.
    fn toggle(&mut self, cx: &mut Context<Self>) {
        let Some(saved) = self.saved().cloned() else {
            return;
        };
        self.enabled = !self.enabled;
        self.browsing = false;
        self.folder_task = None;
        self.error = None;
        if self.enabled != saved.use_custom_directory && saved.custom_directory.is_some() {
            self.save(
                WorktreeSettings {
                    use_custom_directory: self.enabled,
                    ..saved
                },
                cx,
            );
        }
        cx.notify();
    }

    fn reset(&mut self, cx: &mut Context<Self>) {
        let Some(saved) = self.saved() else {
            return;
        };
        let changed = *saved != WorktreeSettings::default();
        self.enabled = false;
        self.browsing = false;
        self.folder_task = None;
        self.error = None;
        if changed {
            self.save(WorktreeSettings::default(), cx);
        }
        cx.notify();
    }

    fn use_folder(&mut self, path: String, cx: &mut Context<Self>) {
        self.save(
            WorktreeSettings {
                use_custom_directory: true,
                custom_directory: Some(path),
            },
            cx,
        );
    }

    fn browse_folder(&mut self, cx: &mut Context<Self>) {
        if self.target.is_none() {
            self.choose_local_folder(cx);
            return;
        }
        let start = self
            .saved()
            .and_then(|saved| saved.custom_directory.clone());
        self.browse(start, cx);
    }

    fn choose_local_folder(&mut self, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        self.browsing = false;
        self.error = None;
        let receiver = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose Worktree Folder".into()),
        });
        // Retargeting the card cancels this task, so a local selection cannot
        // be saved to a remote host.
        self.folder_task = Some(cx.spawn(async move |this, cx| {
            let result = receiver.await;
            let _ = this.update(cx, |card, cx| {
                match result {
                    Ok(Ok(Some(paths))) => {
                        if let Some(path) = paths.into_iter().next() {
                            card.use_folder(path.to_string_lossy().into_owned(), cx);
                        }
                    }
                    Ok(Err(error)) => card.error = Some(error.to_string()),
                    // Cancelling the system dialog keeps the current settings.
                    Ok(Ok(None)) | Err(_) => {}
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn browse(&mut self, path: Option<String>, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        self.browsing = true;
        self.folders = Loadable::Loading;
        let target = self.target.clone();
        self.folder_task = Some(cx.spawn(async move |this, cx| {
            let result: Result<FolderListing, String> = async {
                let value = engine
                    .client()
                    .call(
                        methods::LIST_FOLDERS,
                        serde_json::json!({"path": path, "targetDeviceId": target}),
                    )
                    .await
                    .map_err(|error| error.to_string())?;
                serde_json::from_value(value).map_err(|error| error.to_string())
            }
            .await;
            this.update(cx, |card, cx| {
                card.folders = match result {
                    Ok(listing) => Loadable::Ready(listing),
                    Err(error) => Loadable::Error(error),
                };
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn render_browser(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let parent = match &self.folders {
            Loadable::Ready(listing) => folder_parent(&listing.path),
            _ => None,
        };
        let has_parent = parent.is_some();
        let mut browser = div()
            .mt(px(12.0))
            .overflow_hidden()
            .border_1()
            .border_color(widgets::row_divider(theme))
            .rounded(px(10.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.0))
                    .p(px(4.0))
                    .border_b_1()
                    .border_color(widgets::row_divider(theme))
                    .child(
                        widgets::ghost_action(theme)
                            .id("worktrees-home")
                            .child(
                                icons::icon(icons::HOME)
                                    .size(px(14.0))
                                    .text_color(theme.text_muted),
                            )
                            .child("Home")
                            .on_click(cx.listener(|card, _, _, cx| card.browse(None, cx))),
                    )
                    .child(
                        widgets::ghost_action(theme)
                            .id("worktrees-up")
                            .child(
                                icons::icon(icons::ARROW_UP)
                                    .size(px(14.0))
                                    .text_color(theme.text_muted),
                            )
                            .child("Up")
                            .when(has_parent, |el| {
                                el.on_click(cx.listener(move |card, _, _, cx| {
                                    card.browse(parent.clone(), cx)
                                }))
                            })
                            .when(!has_parent, |el| el.opacity(0.5)),
                    ),
            );
        match &self.folders {
            Loadable::Ready(listing) => {
                let mut entries = div()
                    .id("worktree-folder-list")
                    .p(px(4.0))
                    .max_h(px(200.0))
                    .overflow_y_scroll();
                for (index, entry) in listing
                    .entries
                    .iter()
                    .filter(|entry| entry.is_dir)
                    .enumerate()
                {
                    let path = child_path(&listing.path, &entry.name);
                    entries = entries.child(
                        widgets::ghost_action(theme)
                            .id(("worktree-folder", index))
                            .w_full()
                            .gap(px(8.0))
                            .child(
                                icons::icon(icons::FOLDER)
                                    .size(px(16.0))
                                    .flex_none()
                                    .text_color(theme.text_muted),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .child(entry.name.clone()),
                            )
                            .child(
                                icons::icon(icons::ALT_ARROW_RIGHT)
                                    .size(px(12.0))
                                    .flex_none()
                                    .text_color(theme.text_muted.opacity(0.5)),
                            )
                            .on_click(cx.listener(move |card, _, _, cx| {
                                card.browse(Some(path.clone()), cx)
                            })),
                    );
                }
                if !listing.entries.iter().any(|entry| entry.is_dir) {
                    entries = entries.child(
                        div()
                            .px(px(10.0))
                            .py(px(8.0))
                            .child(description(theme, "No subfolders")),
                    );
                }
                browser = browser
                    .child(
                        div()
                            .px(px(14.0))
                            .pt(px(10.0))
                            .pb(px(6.0))
                            .child(description(theme, &listing.path).truncate()),
                    )
                    .child(entries);
                if listing.truncated {
                    browser = browser.child(div().px(px(14.0)).py(px(6.0)).child(description(
                        theme,
                        "More folders exist than are shown here.",
                    )));
                }
                browser = browser.child(
                    div()
                        .flex()
                        .justify_end()
                        .p(px(4.0))
                        .border_t_1()
                        .border_color(widgets::row_divider(theme))
                        .child(
                            widgets::ghost_action(theme)
                                .id("worktrees-use-folder")
                                .child("Use this folder")
                                .on_click(cx.listener(|card, _, _, cx| {
                                    if let Loadable::Ready(listing) = &card.folders {
                                        let path = listing.path.clone();
                                        card.use_folder(path, cx);
                                    }
                                })),
                        ),
                );
            }
            Loadable::Error(error) => {
                browser = browser.child(
                    div()
                        .p(px(10.0))
                        .child(widgets::error_strip(theme, error.clone())),
                )
            }
            _ => {
                browser = browser.child(
                    div()
                        .px(px(14.0))
                        .py(px(10.0))
                        .child(description(theme, "Loading folders…")),
                )
            }
        }
        browser.into_any_element()
    }
}

impl Render for WorktreeSettingsCard {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).for_settings_surface();
        let switcher = self.render_device_switcher(&theme, cx);
        let status = self.settings.ready();
        let overridden = status.is_some_and(|s| s.environment_override.is_some());
        let interactive = status.is_some() && !self.saving && !overridden;
        let enabled = self.enabled;
        let mut meta = vec![match status {
            Some(status) if overridden || (enabled && status.settings.use_custom_directory) => {
                status.effective_directory.clone()
            }
            Some(_) if enabled => "Choose a folder for new worktrees".into(),
            Some(status) => status.default_directory.clone(),
            None if matches!(self.settings, Loadable::Error(_)) => {
                "Couldn't load worktree settings".into()
            }
            None => "Loading…".into(),
        }];
        if overridden {
            meta.push("Set by ZERON_WORKTREES_DIR".into());
        }
        let meta = meta
            .into_iter()
            .map(|text| div().truncate().child(text).into_any_element())
            .collect();
        let resettable = !overridden
            && (enabled || status.is_some_and(|s| s.settings != WorktreeSettings::default()));

        let controls = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.0))
            .when(resettable, |row| {
                row.child(
                    widgets::action_button(&theme, widgets::ActionTone::Quiet)
                        .id("worktrees-reset")
                        .w(px(32.0))
                        .px_0()
                        .justify_center()
                        .tab_index(0)
                        .role(gpui::Role::Button)
                        .aria_label("Use the default worktree location")
                        .tooltip(widgets::text_tooltip("Reset to default"))
                        .focus_visible(|s| s.border_2().border_color(theme.accent))
                        .child(
                            icons::icon(icons::REFRESH)
                                .size(px(14.0))
                                .text_color(theme.text_muted),
                        )
                        .when(interactive, |el| {
                            el.on_click(cx.listener(|card, _, _, cx| card.reset(cx)))
                        })
                        .when(!interactive, |el| el.opacity(0.5)),
                )
            })
            .when(matches!(self.settings, Loadable::Error(_)), |row| {
                row.child(
                    widgets::text_action(&theme, widgets::ActionTone::Quiet, "Retry")
                        .id("worktrees-retry")
                        .tab_index(0)
                        .role(gpui::Role::Button)
                        .focus_visible(|s| s.border_2().border_color(theme.accent))
                        .on_click(cx.listener(|card, _, _, cx| card.load(card.target.clone(), cx))),
                )
            })
            .child(switcher)
            .when(enabled && !overridden, |row| {
                row.child(
                    widgets::action_button(&theme, widgets::ActionTone::Filled)
                        .id("worktrees-browse")
                        .gap(px(6.0))
                        .aria_label("Choose worktree folder")
                        .child(
                            icons::icon(icons::FOLDER)
                                .size(px(14.0))
                                .flex_none()
                                .text_color(theme.text_muted),
                        )
                        .child("Browse")
                        .when(interactive, |el| {
                            el.tab_index(0)
                                .role(gpui::Role::Button)
                                .focus_visible(|s| s.border_2().border_color(theme.accent))
                                .on_click(cx.listener(|card, _, _, cx| card.browse_folder(cx)))
                        })
                        .when(!interactive, |el| el.opacity(0.5)),
                )
            })
            .child(
                widgets::toggle_switch(&theme, enabled, "worktrees-custom")
                    .id("worktrees-custom-toggle")
                    .flex_none()
                    .role(gpui::Role::Switch)
                    .aria_label("Custom worktree location")
                    .aria_toggled(if enabled {
                        gpui::Toggled::True
                    } else {
                        gpui::Toggled::False
                    })
                    .when(interactive, |el| {
                        el.cursor_pointer()
                            .tab_index(0)
                            .focus_visible(|s| s.border_2().border_color(theme.accent))
                            .on_click(cx.listener(|card, _, _, cx| card.toggle(cx)))
                    })
                    .when(!interactive, |el| el.opacity(0.5)),
            );

        let error = match &self.settings {
            Loadable::Error(error) => Some(error.clone()),
            _ => self.error.clone(),
        };
        // Match the text inset of the shared settings block rows.
        let detail = div()
            .px(px(16.0))
            .pb(px(16.0))
            .when(self.browsing, |el| {
                el.child(self.render_browser(&theme, cx))
            })
            .when_some(error.clone(), |el, error| {
                el.child(widgets::error_strip(&theme, error).mt(px(8.0)))
            });
        let card = widgets::section_card(&theme)
            .mt_0()
            .child(
                widgets::card_row(&theme, true)
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(160.0))
                            .child(widgets::row_title(&theme, "Custom worktree location"))
                            .child(widgets::meta_line(&theme, meta)),
                    )
                    .child(controls)
                    .search_anchor(SettingId::WorktreeLocation),
            )
            .when(self.browsing || error.is_some(), |card| card.child(detail))
            .child(self.render_credit_row(&theme, cx));
        widgets::section(&theme, "Git", card).into_any_element()
    }
}

fn description(theme: &Theme, text: &str) -> gpui::Div {
    div()
        .text_size(crate::typography::ui_rems(widgets::ROW_DESCRIPTION_SIZE))
        .line_height(crate::typography::ui_rems(16.0))
        .text_color(theme.text_muted)
        .child(text.to_owned())
}
