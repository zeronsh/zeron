//! Repository artwork is fetched once per project/connection, off the render thread.
use super::*;
use crate::files::client::{FilesRequestContext, WorkspaceFilesClient};
use crate::image_media::{MediaImage, decode_project_icon, release_media};

enum ProjectIconSource<'a> {
    Chat(&'a str),
    Space(&'a str),
}

pub(super) const ICON_PATHS: &[&str] = &[
    "public/apple-touch-icon.png",
    "apple-touch-icon.png",
    "public/favicon.svg",
    "favicon.svg",
    "public/favicon.png",
    "public/icon.png",
    "public/logo.png",
    "favicon.png",
    "app/icon.png",
    "src/app/icon.png",
    "public/favicon.ico",
    "favicon.ico",
    "app/favicon.ico",
    "static/favicon.ico",
    "src-tauri/icons/icon.png",
    "assets/icon.png",
    "src/assets/icon.png",
];

fn load_local_icon(root: &std::path::Path) -> Option<MediaImage> {
    // A project can point at a subdirectory; match the remote workspace RPC's
    // checkout-root resolution, including linked worktrees with a .git file.
    let canonical = root.canonicalize().ok()?;
    let root = canonical
        .ancestors()
        .find(|path| path.join(".git").exists())
        .unwrap_or(&canonical);
    for path in ICON_PATHS {
        let path = root.join(path);
        if !path.is_file() {
            continue;
        }
        let bytes = std::fs::File::open(&path).ok().and_then(|file| {
            use std::io::Read;
            let mut bytes = Vec::new();
            file.take(zeron_proto::MAX_WORKSPACE_IMAGE_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
                .ok()?;
            Some(bytes)
        })?;
        let mime = if path.extension().is_some_and(|e| e == "svg") {
            "image/svg+xml"
        } else {
            "image/png"
        };
        return decode_project_icon(mime, bytes).ok();
    }
    None
}

/// Validate before copying so cancellation or invalid artwork leaves the current icon intact.
fn import_project_icon(
    source: &std::path::Path,
    directory: &std::path::Path,
) -> Result<String, String> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(source)
        .map_err(|e| e.to_string())?
        .take(zeron_proto::MAX_WORKSPACE_IMAGE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    let svg = source
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("svg"));
    decode_project_icon(
        if svg { "image/svg+xml" } else { "image/png" },
        bytes.clone(),
    )?;
    std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    let name = format!(
        "{}.{}",
        uuid::Uuid::new_v4(),
        if svg { "svg" } else { "image" }
    );
    std::fs::write(directory.join(&name), bytes).map_err(|e| e.to_string())?;
    Ok(name)
}

/// Only files created by the project-icon importer are safe to retire.
fn remove_managed_project_icon(directory: &std::path::Path, name: &str) {
    let path = std::path::Path::new(name);
    let managed_name = path
        .parent()
        .is_none_or(|parent| parent.as_os_str().is_empty())
        && path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .is_some_and(|stem| uuid::Uuid::parse_str(stem).is_ok())
        && matches!(
            path.extension().and_then(|ext| ext.to_str()),
            Some("svg" | "image")
        );
    if managed_name {
        let _ = std::fs::remove_file(directory.join(name));
    }
}

/// Reclaim files left by older picker implementations without touching
/// artwork still referenced by any project or unrelated files in this folder.
pub(super) fn cleanup_orphaned_project_icons(
    data_dir: &std::path::Path,
    references: &std::collections::HashMap<String, String>,
    older_than: std::time::SystemTime,
) {
    let directory = data_dir.join("project-icons");
    let saved = settings::UiSettings::load(data_dir);
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return;
    };
    for entry in entries.flatten() {
        // A picker running as this scan starts may have copied a file that
        // has not yet been installed in settings. Leave recent files alone.
        if !entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .is_ok_and(|modified| modified < older_than)
        {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if !references.values().any(|referenced| referenced == &name)
            && !saved
                .project_icon_overrides
                .values()
                .any(|referenced| referenced == &name)
        {
            remove_managed_project_icon(&directory, &name);
        }
    }
}

fn load_uploaded_icon(path: &std::path::Path) -> Option<MediaImage> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(zeron_proto::MAX_WORKSPACE_IMAGE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    decode_project_icon(
        if path.extension().is_some_and(|ext| ext == "svg") {
            "image/svg+xml"
        } else {
            "image/png"
        },
        bytes,
    )
    .ok()
}

// Curated badge tones: (dark appearance, light appearance). Keep the ordering
// stable so projects retain their assigned color. These are explicit colors,
// independent of the selected theme accent; only their appearance variant changes.
const MONOGRAM_PALETTE: [(u32, u32); 8] = [
    (0x94a3b8, 0x475569), // slate
    (0x93c5fd, 0x2563eb), // blue
    (0xc4b5fd, 0x7c3aed), // violet
    (0xfda4af, 0xbe123c), // rose
    (0xfcd34d, 0xa16207), // amber
    (0x6ee7b7, 0x047857), // emerald
    (0x5eead4, 0x0f766e), // teal
    (0xfdba74, 0xc2410c), // orange
];

fn monogram(name: &str, seed: &str, selected: bool, theme: &Theme) -> AnyElement {
    // FNV-1a selects a stable entry from the curated palette.
    let hash = seed.bytes().fold(2166136261u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(16777619)
    });
    let letter = name
        .trim()
        .chars()
        .next()
        .unwrap_or('?')
        .to_uppercase()
        .to_string();
    let (dark, light) = MONOGRAM_PALETTE[hash as usize % MONOGRAM_PALETTE.len()];
    let tone = gpui::Hsla::from(gpui::rgb(if theme.appearance.is_dark() {
        dark
    } else {
        light
    }));
    let mut hover_text = tone;
    hover_text.l = if theme.appearance.is_dark() {
        (tone.l + 0.12).min(0.95)
    } else {
        (tone.l - 0.10).max(0.15)
    };
    let tile = div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(3.0))
        // The active row wears the hover tint permanently.
        .bg(tone.opacity(if selected { 0.24 } else { 0.08 }))
        .text_color(if selected {
            hover_text
        } else {
            tone.opacity(0.85)
        })
        .group_hover("sidebar-session-row", move |style| {
            style.bg(tone.opacity(0.24)).text_color(hover_text)
        })
        .font_family(theme.font_mono.clone())
        .font_weight(gpui::FontWeight::MEDIUM)
        .child(
            div()
                .w_full()
                .text_center()
                .text_size(px(9.0))
                .line_height(px(13.0))
                .child(letter),
        );
    crate::frost::frosted(3.0, crate::frost::MENU_BLUR, tile).into_any_element()
}

pub(super) struct ProjectIcon {
    name: String,
    seed: String,
    media: Option<MediaImage>,
    refreshed: std::time::Instant,
    _task: Task<()>,
}

impl ProjectIcon {
    fn new(
        name: String,
        seed: String,
        context: FilesRequestContext,
        engine: Option<crate::state::EngineHandle>,
        uploaded: Option<std::path::PathBuf>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.on_release(|this, cx| release_media(this.media.take(), cx))
            .detach();
        let task = cx.spawn(async move |this, cx| {
            let executor = cx.background_executor().clone();
            let load = async {
                if let Some(path) = uploaded {
                    if let Some(media) = executor
                        .spawn(async move { load_uploaded_icon(&path) })
                        .await
                    {
                        return Some(media);
                    }
                }
                if context.target_device_id.is_none() {
                    let root = std::path::PathBuf::from(context.cwd);
                    return executor.spawn(async move { load_local_icon(&root) }).await;
                }
                let client = WorkspaceFilesClient::new(engine?, context.clone());
                for path in ICON_PATHS {
                    // This also resolves the current checkout identity on the owning host.
                    let file = match client
                        .read_file(zeron_proto::ReadWorkspaceFileRequest {
                            target: context.target.clone(),
                            path: (*path).into(),
                        })
                        .await
                    {
                        Ok(file) => file,
                        Err(error) if error.retryable() => return None,
                        Err(_) => continue,
                    };
                    let (mime, bytes) = client
                        .read_image((*path).into(), file.checkout_id)
                        .await
                        .ok()?;
                    return executor
                        .spawn(async move { decode_project_icon(&mime, bytes).ok() })
                        .await;
                }
                None
            };
            let media = match futures::future::select(
                Box::pin(load),
                Box::pin(executor.timer(Duration::from_secs(30))),
            )
            .await
            {
                futures::future::Either::Left((media, _)) => {
                    media.map(|media| media.for_view((16.0, 16.0), 2.0, 4096))
                }
                _ => None,
            };
            let _ = this.update(cx, |this, cx| {
                this.media = media;
                cx.notify();
            });
        });
        Self {
            name,
            seed,
            media: None,
            refreshed: std::time::Instant::now(),
            _task: task,
        }
    }
}

impl Render for ProjectIcon {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match &self.media {
            Some(media) => gpui::img(media.image.clone())
                .size_full()
                .object_fit(gpui::ObjectFit::Contain)
                .into_any_element(),
            None => monogram(&self.name, &self.seed, false, Theme::of(cx)),
        }
    }
}

/// Same card as the pull-request badge tooltip: project name, then the
/// owning device below in the muted line.
struct ProjectIconTooltip {
    name: SharedString,
    device: SharedString,
}

impl Render for ProjectIconTooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        let card = div()
            .max_w(px(320.0))
            .px(px(9.0))
            .py(px(7.0))
            .flex()
            .flex_col()
            .gap(px(3.0))
            .rounded(px(6.0))
            .border_1()
            .border_color(theme.border_strong)
            .bg(crate::popover::surface_bg(theme))
            .when(!theme.is_frost(), |el| el.shadow_md())
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .whitespace_nowrap()
                    .text_size(px(11.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(theme.text)
                    .child(self.name.clone()),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .whitespace_nowrap()
                    .text_size(px(11.0))
                    .text_color(theme.text_muted)
                    .child(self.device.clone()),
            );
        crate::frost::frosted(6.0, crate::frost::MENU_BLUR, card)
    }
}

fn project_icon_frame(
    chat_id: &str,
    name: &str,
    device: &str,
    size: f32,
    child: impl IntoElement,
) -> AnyElement {
    let name: SharedString = name.to_owned().into();
    let device: SharedString = device.to_owned().into();
    div()
        .id(SharedString::from(format!("project-icon-{chat_id}")))
        .size(px(size))
        .flex_none()
        .tooltip(move |_, cx| {
            cx.new(|_| ProjectIconTooltip {
                name: name.clone(),
                device: device.clone(),
            })
            .into()
        })
        .tooltip_show_delay(Duration::from_millis(350))
        .child(child)
        .into_any_element()
}

impl Shell {
    /// Save the new pointer before deleting the old image. A failed settings
    /// write leaves the previous icon and its file intact.
    fn set_project_icon_override(
        &mut self,
        key: String,
        replacement: Option<String>,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let previous = self.settings.project_icon_overrides.get(&key).cloned();
        if previous == replacement {
            return Ok(());
        }
        match &replacement {
            Some(name) => {
                self.settings
                    .project_icon_overrides
                    .insert(key.clone(), name.clone());
            }
            None => {
                self.settings.project_icon_overrides.remove(&key);
            }
        }
        self.schedule_save(cx);
        if let Err(error) = settings::current(cx).save(&self.boot.data_dir) {
            match previous {
                Some(name) => {
                    self.settings.project_icon_overrides.insert(key, name);
                }
                None => {
                    self.settings.project_icon_overrides.remove(&key);
                }
            }
            self.schedule_save(cx);
            return Err(format!("Could not save project icon: {error}"));
        }
        if let Some(previous) = previous
            && !self
                .settings
                .project_icon_overrides
                .values()
                .any(|name| name == &previous)
        {
            remove_managed_project_icon(&self.boot.data_dir.join("project-icons"), &previous);
        }
        Ok(())
    }

    pub(super) fn project_icon_key(&self, space_id: &str, cx: &App) -> Option<String> {
        let space = self
            .state
            .read(cx)
            .spaces
            .iter()
            .find(|space| space.id == space_id)?;
        Some(format!(
            "{:?}:{}:{}",
            self.active_sidebar_pin_profile_key(cx),
            space.device_id,
            space.id
        ))
    }

    pub(super) fn choose_project_icon(&mut self, space_id: String, cx: &mut Context<Self>) {
        self.close_space_menu(cx);
        self.close_spaces_menu(cx);
        let Some(key) = self.project_icon_key(&space_id, cx) else {
            return;
        };
        let previous = self.settings.project_icon_overrides.get(&key).cloned();
        let directory = self.boot.data_dir.join("project-icons");
        let receiver = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose Project Icon".into()),
        });
        cx.spawn(async move |this, cx| {
            let path = match receiver.await {
                Ok(Ok(Some(mut paths))) => paths.pop(),
                _ => None,
            };
            let Some(path) = path else {
                return;
            };
            let cleanup_directory = directory.clone();
            let result = cx
                .background_executor()
                .spawn(async move { import_project_icon(&path, &directory) })
                .await;
            let imported_name = result.as_ref().ok().cloned();
            if this
                .update(cx, |this, cx| {
                    // Don't apply an old picker result to another workspace/profile.
                    if this.project_icon_key(&space_id, cx).as_ref() != Some(&key)
                        || this.settings.project_icon_overrides.get(&key) != previous.as_ref()
                    {
                        if let Ok(name) = result {
                            remove_managed_project_icon(
                                &this.boot.data_dir.join("project-icons"),
                                &name,
                            );
                        }
                        return;
                    }
                    match result {
                        Ok(name) => {
                            match this.set_project_icon_override(key, Some(name.clone()), cx) {
                                Ok(()) => this.sidebar_notice = Some("Project icon updated".into()),
                                Err(error) => {
                                    remove_managed_project_icon(
                                        &this.boot.data_dir.join("project-icons"),
                                        &name,
                                    );
                                    this.sidebar_notice = Some(error.into());
                                }
                            }
                        }
                        Err(error) => {
                            this.sidebar_notice =
                                Some(format!("Could not use this image: {error}").into())
                        }
                    }
                    cx.notify();
                })
                .is_err()
            {
                if let Some(name) = imported_name {
                    remove_managed_project_icon(&cleanup_directory, &name);
                }
            }
        })
        .detach();
    }

    pub(super) fn reset_project_icon(&mut self, space_id: &str, cx: &mut Context<Self>) {
        self.close_space_menu(cx);
        if let Some(key) = self.project_icon_key(space_id, cx) {
            if let Err(error) = self.set_project_icon_override(key, None, cx) {
                self.sidebar_notice = Some(error.into());
            }
            cx.notify();
        }
    }

    pub(super) fn render_project_icon(
        &self,
        chat_id: &str,
        size: f32,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.render_project_identity_icon(ProjectIconSource::Chat(chat_id), size, selected, cx)
    }

    pub(super) fn render_space_icon(
        &self,
        space_id: &str,
        size: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.render_project_identity_icon(ProjectIconSource::Space(space_id), size, false, cx)
    }

    fn render_project_identity_icon(
        &self,
        source: ProjectIconSource<'_>,
        size: f32,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = self.state.read(cx);
        let (space, device_id, element_id) = match source {
            ProjectIconSource::Chat(chat_id) => {
                let chat = state.chats.iter().find(|chat| chat.id == chat_id);
                (
                    chat.and_then(|chat| state.space_for_chat(chat)),
                    chat.map(|chat| chat.device_id.as_str()),
                    chat_id.to_owned(),
                )
            }
            ProjectIconSource::Space(space_id) => {
                let space = state.space_row(space_id);
                (
                    space,
                    space.map(|space| space.device_id.as_str()),
                    format!("space-{space_id}"),
                )
            }
        };
        let chat_id = element_id.as_str();
        let name = space
            .map(|space| space.display_name().to_string())
            .unwrap_or_else(|| "Home".into());
        // Same fallback as the row's "@ device" fragment.
        let device = device_id
            .and_then(|id| state.device_name(id))
            .unwrap_or("Unknown device")
            .to_string();
        let seed = space
            .map(|space| space.path.clone())
            .unwrap_or_else(|| "home".into());
        let uploaded = space
            .and_then(|space| self.project_icon_key(&space.id, cx))
            .and_then(|key| self.settings.project_icon_overrides.get(&key))
            // Settings contain a managed filename, never an arbitrary path.
            .filter(|name| {
                std::path::Path::new(name).components().count() == 1 && !name.contains(['/', '\\'])
            })
            .map(|name| self.boot.data_dir.join("project-icons").join(name));
        let context = space.map(|space| FilesRequestContext {
            target: zeron_proto::WorkspaceTarget {
                chat_id: None,
                space_id: Some(space.id.clone()),
                checkout_path: None,
            },
            target_device_id: (state.local_device_id.as_deref() != Some(&space.device_id))
                .then(|| space.device_id.clone()),
            cwd: space.path.clone(),
            checkout_id: space.checkout_id.clone(),
        });
        let Some(context) = context else {
            return project_icon_frame(
                chat_id,
                &name,
                &device,
                size,
                monogram(&name, &seed, selected, Theme::of(cx)),
            );
        };
        let key = format!(
            "{:?}:{:?}:{}:{:?}:{}:{:?}",
            self.active_sidebar_pin_profile_key(cx),
            context.target_device_id,
            context.cwd,
            context.checkout_id,
            name,
            uploaded,
        );
        let engine = state.engine().cloned();
        // Don't cache a remote miss before a connection exists.
        if uploaded.is_none() && context.target_device_id.is_some() && engine.is_none() {
            return project_icon_frame(
                chat_id,
                &name,
                &device,
                size,
                monogram(&name, &seed, selected, Theme::of(cx)),
            );
        }
        let mut cache = self.project_icons.borrow_mut();
        cache.retain(|_, entity| entity.read(cx).refreshed.elapsed() < Duration::from_secs(300));
        let entity = cache
            .entry(key)
            .or_insert_with(|| {
                let entity = cx.new(|cx| {
                    ProjectIcon::new(name.clone(), seed.clone(), context, engine, uploaded, cx)
                });
                // The monogram below is drawn by the shell (it needs the row's
                // selected state, which the shared entity can't hold), so the
                // shell must redraw when artwork lands.
                cx.observe(&entity, |_, _, cx| cx.notify()).detach();
                entity
            })
            .clone();
        drop(cache);
        if entity.read(cx).media.is_none() {
            return project_icon_frame(
                chat_id,
                &name,
                &device,
                size,
                monogram(&name, &seed, selected, Theme::of(cx)),
            );
        }
        project_icon_frame(chat_id, &name, &device, size, entity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ProjectHeaderHost(Entity<Shell>);

    impl Render for ProjectHeaderHost {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.0.update(cx, |shell, cx| {
                div()
                    .w(px(310.0))
                    .h(px(80.0))
                    .child(shell.render_spaces_filter(&Theme::of(cx).clone(), cx))
            })
        }
    }

    #[gpui::test]
    fn selected_project_header_opens_icon_picker_without_changing_filter(
        cx: &mut gpui::TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
            settings::init(settings::UiSettings::default(), dir.path(), cx);
        });
        let (host, cx) = cx.add_window_view(|_, cx| {
            ProjectHeaderHost(cx.new(|cx| {
                let state = cx.new(|_| {
                    let mut state = AppState::new();
                    state.workspace_scope = Some(zeron_proto::WorkspaceScope::Local);
                    state.local_device_id = Some("local".into());
                    state.spaces = vec![
                        serde_json::from_value(serde_json::json!({
                            "id": "project", "deviceId": "local", "path": dir.path(),
                            "createdAt": Utc::now(),
                        }))
                        .unwrap(),
                    ];
                    state
                });
                let mut shell = Shell::new(
                    state,
                    EngineBootConfig {
                        data_dir: dir.path().into(),
                        ipc_port: 0,
                        edge_url: String::new(),
                        edge_token: None,
                        org_id: None,
                        workos_client_id: None,
                        default_harness: zeron_proto::HarnessId::Mock,
                    },
                    cx,
                );
                shell.settings.space_filter = Some("project".into());
                shell
            }))
        });
        let shell = host.read_with(cx, |host, _| host.0.clone());
        cx.update(|window, cx| window.draw(cx).clear());
        let icon_bounds = cx.debug_bounds("selected-project-icon").unwrap();
        cx.simulate_click(icon_bounds.center(), gpui::Modifiers::default());
        assert!(cx.did_prompt_for_paths());
        shell.read_with(cx, |shell, _| {
            assert_eq!(shell.settings.space_filter.as_deref(), Some("project"));
            assert!(!shell.spaces_menu.is_open());
        });
        cx.simulate_path_prompt_response(|_| None);
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.blur();
            window.focus_next(cx);
        });
        cx.simulate_keystrokes("enter");
        assert!(cx.did_prompt_for_paths());
        cx.simulate_path_prompt_response(|_| None);
        cx.run_until_parked();
        let header = cx.debug_bounds("spaces-filter").unwrap();
        cx.simulate_mouse_down(
            header.center(),
            MouseButton::Right,
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_up(
            header.center(),
            MouseButton::Right,
            gpui::Modifiers::default(),
        );
        shell.read_with(cx, |shell, _| {
            assert_eq!(
                shell.space_menu.get().map(|(id, _)| id.as_str()),
                Some("project")
            );
        });
        shell.update(cx, |shell, cx| {
            shell.close_space_menu(cx);
            shell.settings.space_filter = None;
        });
        host.update(cx, |_, cx| cx.notify());
        cx.update(|window, cx| window.draw(cx).clear());
        assert!(cx.debug_bounds("selected-project-icon").is_none());
        let header = cx.debug_bounds("spaces-filter").unwrap();
        cx.simulate_click(header.center(), gpui::Modifiers::default());
        assert!(shell.read_with(cx, |shell, _| shell.spaces_menu.is_open()));
    }

    fn png(path: &std::path::Path, width: u32) {
        image::RgbaImage::new(width, 2).save(path).unwrap();
    }
    #[gpui::test]
    fn project_icon_picker_cancel_apply_and_reset(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("upload.png");
        png(&source, 24);
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
            settings::init(settings::UiSettings::default(), dir.path(), cx);
            crate::history::init(
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                cx,
            );
        });
        let window = cx.add_window(|_, cx| {
            let state = cx.new(|_| {
                let mut state = AppState::new();
                state.workspace_scope = Some(zeron_proto::WorkspaceScope::Local);
                state.spaces = vec![serde_json::from_value(serde_json::json!({
                    "id": "project", "deviceId": "local", "path": "/project", "createdAt": Utc::now(),
                })).unwrap()];
                state
            });
            Shell::new(state, EngineBootConfig {
                data_dir: dir.path().into(), ipc_port: 0, edge_url: String::new(),
                edge_token: None, org_id: None, workos_client_id: None,
                default_harness: zeron_proto::HarnessId::Mock,
            }, cx)
        });
        for accepted in [false, true] {
            window
                .update(cx, |shell, _, cx| {
                    shell.choose_project_icon("project".into(), cx)
                })
                .unwrap();
            assert!(cx.did_prompt_for_paths());
            let path = source.clone();
            cx.simulate_path_prompt_response(move |_| accepted.then(|| vec![path]));
            cx.run_until_parked();
            window
                .read_with(cx, |shell, cx| {
                    let key = shell.project_icon_key("project", cx).unwrap();
                    assert_eq!(
                        shell.settings.project_icon_overrides.contains_key(&key),
                        accepted
                    );
                })
                .unwrap();
        }
        let previous = window
            .read_with(cx, |shell, cx| {
                let key = shell.project_icon_key("project", cx).unwrap();
                shell.settings.project_icon_overrides[&key].clone()
            })
            .unwrap();
        png(&source, 32);
        window
            .update(cx, |shell, _, cx| {
                shell.choose_project_icon("project".into(), cx)
            })
            .unwrap();
        assert!(cx.did_prompt_for_paths());
        let replacement = source.clone();
        cx.simulate_path_prompt_response(move |_| Some(vec![replacement]));
        cx.run_until_parked();
        assert!(!dir.path().join("project-icons").join(&previous).exists());
        let current = window
            .read_with(cx, |shell, cx| {
                let key = shell.project_icon_key("project", cx).unwrap();
                shell.settings.project_icon_overrides[&key].clone()
            })
            .unwrap();
        assert_ne!(previous, current);
        assert_eq!(
            settings::UiSettings::load(dir.path())
                .project_icon_overrides
                .values()
                .next(),
            Some(&current)
        );
        std::fs::remove_file(&source).unwrap();
        window
            .update(cx, |shell, _, cx| {
                let key = shell.project_icon_key("project", cx).unwrap();
                let filename = &shell.settings.project_icon_overrides[&key];
                assert!(
                    load_uploaded_icon(&dir.path().join("project-icons").join(filename)).is_some()
                );
                shell.reset_project_icon("project", cx);
                assert!(!shell.settings.project_icon_overrides.contains_key(&key));
            })
            .unwrap();
        assert!(!dir.path().join("project-icons").join(current).exists());
        assert!(
            settings::UiSettings::load(dir.path())
                .project_icon_overrides
                .is_empty()
        );
    }

    #[test]
    fn imported_icon_survives_source_removal_and_rejects_invalid_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source.png");
        let managed = temp.path().join("icons");
        png(&source, 24);
        let name = import_project_icon(&source, &managed).unwrap();
        std::fs::remove_file(&source).unwrap();
        assert_eq!(
            load_uploaded_icon(&managed.join(&name)).unwrap().width,
            24.0
        );
        std::fs::write(&source, b"not an image").unwrap();
        assert!(import_project_icon(&source, &managed).is_err());
        assert_eq!(std::fs::read_dir(&managed).unwrap().count(), 1);
        assert!(load_uploaded_icon(&managed.join(name)).is_some());
    }

    #[test]
    fn startup_cleanup_keeps_referenced_and_foreign_files() {
        let dir = tempfile::tempdir().unwrap();
        let icons = dir.path().join("project-icons");
        std::fs::create_dir(&icons).unwrap();
        let current = format!("{}.image", uuid::Uuid::new_v4());
        let orphan = format!("{}.svg", uuid::Uuid::new_v4());
        for name in [&current, &orphan] {
            std::fs::write(icons.join(name), b"image").unwrap();
        }
        std::fs::write(icons.join("manual.image"), b"keep").unwrap();
        cleanup_orphaned_project_icons(
            dir.path(),
            &std::collections::HashMap::from([("project".into(), current.clone())]),
            std::time::SystemTime::now() + std::time::Duration::from_secs(1),
        );
        assert!(icons.join(current).exists());
        assert!(!icons.join(orphan).exists());
        assert!(icons.join("manual.image").exists());
    }

    #[test]
    fn imported_icon_rejects_oversized_files() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("large.png");
        std::fs::File::create(&source)
            .unwrap()
            .set_len(zeron_proto::MAX_WORKSPACE_IMAGE_BYTES as u64 + 1)
            .unwrap();
        let managed = temp.path().join("icons");
        assert!(import_project_icon(&source, &managed).is_err());
        assert!(!managed.exists());
    }

    #[test]
    fn sidebar_project_icon_priority_and_missing_fallback() {
        let temp = tempfile::tempdir().unwrap();
        assert!(load_local_icon(temp.path()).is_none());
        png(&temp.path().join("favicon.png"), 3);
        assert_eq!(load_local_icon(temp.path()).unwrap().width, 3.0);
        std::fs::create_dir(temp.path().join("public")).unwrap();
        png(&temp.path().join("public/apple-touch-icon.png"), 7);
        assert_eq!(load_local_icon(temp.path()).unwrap().width, 7.0);
        std::fs::write(temp.path().join(".git"), "gitdir: /unused-test-checkout").unwrap();
        std::fs::create_dir_all(temp.path().join("src/nested")).unwrap();
        assert_eq!(
            load_local_icon(&temp.path().join("src/nested"))
                .unwrap()
                .width,
            7.0
        );
        // A corrupt first match uses the default rather than showing unrelated lower-priority art.
        std::fs::write(temp.path().join("public/apple-touch-icon.png"), b"invalid").unwrap();
        assert!(load_local_icon(temp.path()).is_none());
    }
    #[test]
    fn sidebar_project_icons_support_svg_and_ico() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("favicon.svg"), br##"<svg xmlns="http://www.w3.org/2000/svg" width="12" height="8"><rect width="12" height="8" fill="#f00"/></svg>"##).unwrap();
        assert_eq!(load_local_icon(temp.path()).unwrap().width, 12.0);
        std::fs::remove_file(temp.path().join("favicon.svg")).unwrap();
        image::RgbaImage::new(16, 16)
            .save(temp.path().join("favicon.ico"))
            .unwrap();
        assert_eq!(load_local_icon(temp.path()).unwrap().width, 16.0);
    }
}
