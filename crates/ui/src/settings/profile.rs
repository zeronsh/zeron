use std::{io::Cursor, path::Path, time::Duration};

use gpui::{
    AnyElement, App, Context, Entity, Global, PathPromptOptions, Task, Window, div, img,
    prelude::*, px,
};
use serde::{Deserialize, Serialize};

use super::{SavePolicy, SettingsStore, current, replace, widgets};
use crate::{composer::ComposerInput, icons, popover, privacy, theme::Theme, typography::ui_rems};

const AVATAR_DIR: &str = "profile-avatars";
const MAX_AVATAR_BYTES: usize = 10 * 1024 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Profile {
    pub github: Option<GitHubProfile>,
    pub name: Option<String>,
    pub avatar_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubProfile {
    pub login: String,
    pub name: Option<String>,
    pub avatar_path: Option<String>,
}

impl Profile {
    pub fn display_name(&self) -> Option<&str> {
        self.name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .or_else(|| {
                self.github.as_ref().map(|github| {
                    github
                        .name
                        .as_deref()
                        .filter(|name| !name.trim().is_empty())
                        .unwrap_or(&github.login)
                })
            })
    }

    fn avatar_path(&self) -> Option<&str> {
        self.avatar_path.as_deref().or_else(|| {
            self.github
                .as_ref()
                .and_then(|github| github.avatar_path.as_deref())
        })
    }
}

#[derive(Default)]
struct GitHubLoader {
    task: Option<Task<()>>,
    error: Option<String>,
}

impl Global for GitHubLoader {}

#[derive(Deserialize)]
struct GitHubUser {
    login: String,
    name: Option<String>,
    avatar_url: String,
}

async fn fetch_github() -> Result<(GitHubUser, Option<Vec<u8>>), String> {
    let mut command = tokio::process::Command::new("gh");
    zeron_harness::compose_login_shell_path(&mut command);
    command
        .args(["api", "--hostname", "github.com", "user"])
        .env("GH_PROMPT_DISABLED", "1")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    let output = tokio::time::timeout(Duration::from_secs(15), command.output())
        .await
        .map_err(|_| "GitHub took too long to respond. Try refreshing.".to_owned())?
        .map_err(|_| {
            "Install GitHub CLI and sign in with gh auth login to use your GitHub profile."
                .to_owned()
        })?;
    if !output.status.success() {
        return Err("Sign in to GitHub CLI with gh auth login, then refresh your profile.".into());
    }
    let user: GitHubUser = serde_json::from_slice(&output.stdout)
        .map_err(|_| "GitHub returned an unreadable profile. Try refreshing.".to_owned())?;
    if user.login.trim().is_empty() {
        return Err("GitHub returned a profile without a username.".into());
    }
    let avatar = fetch_github_avatar(&user.avatar_url).await.ok();
    Ok((user, avatar))
}

async fn fetch_github_avatar(url: &str) -> anyhow::Result<Vec<u8>> {
    let mut url = url::Url::parse(url)?;
    anyhow::ensure!(
        url.scheme() == "https" && url.host_str() == Some("avatars.githubusercontent.com"),
        "Unexpected avatar host"
    );
    url.query_pairs_mut().append_pair("s", "256");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let mut response = client.get(url).send().await?.error_for_status()?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        anyhow::ensure!(
            bytes.len() + chunk.len() <= MAX_AVATAR_BYTES,
            "Avatar is too large"
        );
        bytes.extend_from_slice(&chunk);
    }
    normalize_avatar(&bytes).map_err(anyhow::Error::msg)
}

fn normalize_avatar(bytes: &[u8]) -> Result<Vec<u8>, String> {
    if bytes.len() > MAX_AVATAR_BYTES {
        return Err("Choose a photo smaller than 10 MB.".into());
    }
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| "Could not read this photo.".to_owned())?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let image = reader
        .decode()
        .map_err(|_| {
            "Choose a valid PNG, JPEG, GIF, or WebP photo under 16 megapixels.".to_owned()
        })?
        .resize_to_fill(256, 256, image::imageops::FilterType::Lanczos3);
    let mut png = Cursor::new(Vec::new());
    image
        .write_to(&mut png, image::ImageFormat::Png)
        .map_err(|_| "Could not prepare this photo.".to_owned())?;
    Ok(png.into_inner())
}

/// Cache only public profile fields and a normalized photo; credentials stay
/// in GitHub CLI. Refreshing never changes the user's overrides.
pub fn refresh_github(cx: &mut App) {
    if !cx.has_global::<GitHubLoader>() {
        cx.set_global(GitHubLoader::default());
    }
    if cx.global::<GitHubLoader>().task.is_some() {
        return;
    }
    cx.global_mut::<GitHubLoader>().error = None;
    let fetch = gpui_tokio::Tokio::spawn(cx, fetch_github());
    let task = cx.spawn(async move |cx| {
        let result = match fetch.await {
            Ok(result) => result,
            Err(_) => Err("Could not load your GitHub profile. Try refreshing.".into()),
        };
        cx.update(|cx| {
            let outcome = result.and_then(|(user, avatar)| {
                let mut profile = current(cx).profile;
                let old_avatar = profile
                    .github
                    .as_ref()
                    .filter(|old| old.login == user.login)
                    .and_then(|old| old.avatar_path.clone());
                profile.github = Some(GitHubProfile {
                    login: user.login,
                    name: user.name,
                    avatar_path: old_avatar,
                });
                save_profile(
                    profile,
                    avatar.map(|bytes| (PhotoTarget::GitHub, bytes)),
                    cx,
                )
            });
            let loader = cx.global_mut::<GitHubLoader>();
            loader.task = None;
            loader.error = outcome.err();
            cx.refresh_windows();
        });
    });
    cx.global_mut::<GitHubLoader>().task = Some(task);
    cx.refresh_windows();
}

enum PhotoTarget {
    Custom,
    GitHub,
}

fn save_profile(
    mut profile: Profile,
    photo: Option<(PhotoTarget, Vec<u8>)>,
    cx: &mut App,
) -> Result<(), String> {
    let data_dir = cx
        .try_global::<SettingsStore>()
        .map(|store| store.data_dir.clone())
        .ok_or_else(|| "Unable to save your profile. Restart Zeron and try again.".to_owned())?;
    let directory = data_dir.join(AVATAR_DIR);
    let mut new_path = None;
    if let Some((target, bytes)) = photo {
        std::fs::create_dir_all(&directory).map_err(|_| "Unable to save your photo.".to_owned())?;
        let path = directory.join(format!("{}.png", uuid::Uuid::new_v4()));
        std::fs::write(&path, bytes).map_err(|_| "Unable to save your photo.".to_owned())?;
        let value = Some(path.to_string_lossy().into_owned());
        match target {
            PhotoTarget::Custom => profile.avatar_path = value,
            PhotoTarget::GitHub => {
                if let Some(github) = profile.github.as_mut() {
                    github.avatar_path = value;
                }
            }
        }
        new_path = Some(path);
    }
    let mut next = current(cx);
    let old = std::mem::replace(&mut next.profile, profile);
    if next.save(&data_dir).is_err() {
        if let Some(path) = new_path {
            let _ = std::fs::remove_file(path);
        }
        return Err("Unable to save your profile. Check folder permissions and try again.".into());
    }
    let keep = [
        &next.profile.avatar_path,
        &next
            .profile
            .github
            .as_ref()
            .and_then(|github| github.avatar_path.clone()),
    ];
    for path in [
        old.avatar_path,
        old.github.and_then(|github| github.avatar_path),
    ]
    .into_iter()
    .flatten()
    {
        if !keep
            .iter()
            .any(|keep| keep.as_deref() == Some(path.as_str()))
            && Path::new(&path).parent() == Some(directory.as_path())
        {
            let _ = std::fs::remove_file(path);
        }
    }
    replace(next, SavePolicy::Immediate, cx);
    cx.refresh_windows();
    Ok(())
}

pub fn avatar(profile: &Profile, name: &str, size: f32, theme: &Theme) -> AnyElement {
    let initial = name
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "?".into());
    let background = theme.text;
    let foreground = theme.bg;
    let fallback = move || {
        div()
            .size_full()
            .rounded_full()
            .bg(background)
            .flex()
            .items_center()
            .justify_center()
            .text_size(ui_rems(size * 0.4))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(foreground)
            .child(initial.clone())
            .into_any_element()
    };
    div()
        .size(px(size))
        .flex_none()
        .rounded_full()
        .child(match profile.avatar_path() {
            Some(path) => img(std::path::PathBuf::from(path))
                .size_full()
                .rounded_full()
                .object_fit(gpui::ObjectFit::Cover)
                .with_fallback(fallback)
                .into_any_element(),
            None => fallback(),
        })
        .into_any_element()
}

pub struct ProfilePage {
    scroll: widgets::PageScroll,
    name: Entity<ComposerInput>,
    error: Option<String>,
    choosing_photo: bool,
}

impl ProfilePage {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let name = current(cx).profile.name.unwrap_or_default();
        Self {
            scroll: widgets::PageScroll::default(),
            name: cx.new(|cx| {
                let mut input = ComposerInput::new("Use GitHub name", cx).with_single_line();
                input.set_text(name, cx);
                input
            }),
            error: None,
            choosing_photo: false,
        }
    }

    fn save_name(&mut self, cx: &mut Context<Self>) {
        let text = self.name.read(cx).text().trim().to_owned();
        let mut profile = current(cx).profile;
        profile.name = (!text.is_empty()).then_some(text);
        self.error = save_profile(profile, None, cx).err();
        cx.notify();
    }

    fn reset(&mut self, cx: &mut Context<Self>) {
        let mut profile = current(cx).profile;
        profile.name = None;
        profile.avatar_path = None;
        self.error = save_profile(profile, None, cx).err();
        if self.error.is_none() {
            self.name.update(cx, |input, cx| input.set_text("", cx));
        }
        cx.notify();
    }

    fn choose_photo(&mut self, cx: &mut Context<Self>) {
        if self.choosing_photo {
            return;
        }
        self.choosing_photo = true;
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose a profile photo".into()),
        });
        cx.spawn(async move |this, cx| {
            let path = match receiver.await {
                Ok(Ok(Some(mut paths))) => paths.pop(),
                _ => None,
            };
            let photo = if let Some(path) = path {
                Some(
                    cx.background_executor()
                        .spawn(async move {
                            let metadata = std::fs::metadata(&path)
                                .map_err(|_| "Could not read this photo.".to_owned())?;
                            if metadata.len() > MAX_AVATAR_BYTES as u64 {
                                return Err("Choose a photo smaller than 10 MB.".into());
                            }
                            let bytes = std::fs::read(path)
                                .map_err(|_| "Could not read this photo.".to_owned())?;
                            normalize_avatar(&bytes)
                        })
                        .await,
                )
            } else {
                None
            };
            let _ = this.update(cx, |page, cx| {
                page.choosing_photo = false;
                if let Some(photo) = photo {
                    page.error = photo
                        .and_then(|bytes| {
                            save_profile(
                                current(cx).profile,
                                Some((PhotoTarget::Custom, bytes)),
                                cx,
                            )
                        })
                        .err();
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

impl popover::ScrollRailHost for ProfilePage {
    fn rail_bar(&mut self) -> &mut popover::MenuScrollbarState {
        self.scroll.rail_bar()
    }
    fn rail_scroll(&self) -> Option<gpui::ScrollHandle> {
        self.scroll.rail_scroll()
    }
}

impl Render for ProfilePage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let accent = theme.accent;
        let settings = current(cx);
        let profile = &settings.profile;
        let display_name = profile.display_name().unwrap_or("Your profile").to_owned();
        let loading = cx
            .try_global::<GitHubLoader>()
            .is_some_and(|loader| loader.task.is_some());
        let github_error = cx
            .try_global::<GitHubLoader>()
            .and_then(|loader| loader.error.clone());
        let source = profile
            .github
            .as_ref()
            .map(|github| format!("@{} on GitHub", github.login))
            .unwrap_or_else(|| "Connect your GitHub profile or make it your own.".into());
        let content = widgets::page_column()
            .child(widgets::page_header(&theme, "Profile", None))
            .child(widgets::page_subtitle(&theme, "Your name, photo, and privacy on this device."))
            .child(widgets::section_card(&theme).p(px(20.0)).gap(px(20.0))
                .child(div().flex().flex_wrap().items_center().gap(px(16.0))
                    .child(avatar(profile, &display_name, 56.0, &theme))
                    .child(div().flex_1().min_w_0()
                        .child(div().text_size(ui_rems(16.0)).font_weight(gpui::FontWeight::SEMIBOLD).child(privacy::identity(display_name, cx)))
                        .child(widgets::page_subtitle(&theme, source).text_size(ui_rems(12.0))))
                    .child(widgets::ghost_action(&theme).id("profile-choose-photo").child(if self.choosing_photo { "Opening…" } else { "Change photo" })
                        .on_click(cx.listener(|this, _, _, cx| this.choose_photo(cx)))))
                .child(div().flex().flex_col().gap(px(8.0))
                    .child(widgets::field_label(&theme, "Display name"))
                    .child(div().flex().flex_wrap().items_center().gap(px(10.0))
                        .child(div().flex_1().min_w_0().flex_basis(ui_rems(240.0)).px(px(12.0)).py(px(10.0))
                            .rounded(px(8.0)).border_1().border_color(theme.border).bg(theme.bg).child(self.name.clone()))
                    .child(widgets::ghost_action(&theme).id("profile-save-name").child("Save name")
                            .debug_selector(|| "profile-save-name".into())
                            .on_click(cx.listener(|this, _, _, cx| this.save_name(cx))))))
                .child(div().flex().flex_wrap().items_center().gap(px(10.0))
                    .child(widgets::ghost_action(&theme).id("profile-refresh-github").child(if loading { "Refreshing…" } else { "Refresh GitHub profile" })
                        .on_click(|_, _, cx| refresh_github(cx)))
                    .when(profile.name.is_some() || profile.avatar_path.is_some(), |row| row.child(
                        widgets::ghost_action(&theme).id("profile-reset").child("Use GitHub defaults")
                            .on_click(cx.listener(|this, _, _, cx| this.reset(cx))))))
                .child(widgets::page_subtitle(&theme, "Uses the GitHub account signed in on this device. Custom details stay in Zeron.").text_size(ui_rems(12.0))))
            .when_some(self.error.clone().or(github_error), |column, error| column.child(widgets::error_strip(&theme, error)))
            .child(widgets::section_card(&theme)
                .child(widgets::card_row(&theme, true).flex_wrap()
                    .child(widgets::row_tile(&theme, icons::EYE_CLOSED))
                    .child(div().flex_1().min_w_0().flex_basis(ui_rems(220.0))
                        .child(widgets::row_title(&theme, "Blur email addresses"))
                        .child(widgets::page_subtitle(&theme, "Hide emails in account details, the sidebar, and sign-in dialogs.").text_size(ui_rems(12.0))))
                    .child(div().id("profile-blur-emails").flex_none().size(px(40.0)).flex().items_center().justify_center()
                        .debug_selector(|| "profile-blur-emails".into())
                        .role(gpui::Role::Switch).aria_label("Blur email addresses")
                        .aria_toggled(if settings.blur_emails { gpui::Toggled::True } else { gpui::Toggled::False })
                        .tab_index(0).cursor_pointer()
                        .focus_visible(move |style| style.border_2().border_color(accent))
                        .on_click(|_, _, cx| privacy::set_emails_hidden(!privacy::emails_hidden(cx), cx))
                        .on_key_down(|event, _, cx| {
                            if !event.is_held && matches!(event.keystroke.key.as_str(), "space" | "enter") {
                                privacy::set_emails_hidden(!privacy::emails_hidden(cx), cx);
                                cx.stop_propagation();
                            }
                        })
                        .child(widgets::toggle_switch(&theme, settings.blur_emails))))
                .child(widgets::card_row(&theme, false).flex_wrap().text_size(ui_rems(13.0)).text_color(theme.text_muted)
                    .child("Preview")
                    .child(privacy::identity("you@example.com", cx))));
        let scrollbar = popover::rail(self, "profile-scrollbar", &theme, cx);
        div()
            .id("profile-host")
            .relative()
            .size_full()
            .on_hover(cx.listener(|this, hovered, _, cx| {
                if this.scroll.set_list_hovered(*hovered) {
                    cx.notify();
                }
            }))
            .child(
                div()
                    .id("profile-page")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll.scroll)
                    .child(content),
            )
            .children(scrollbar)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    fn github() -> GitHubProfile {
        GitHubProfile {
            login: "octocat".into(),
            name: Some("Mona".into()),
            avatar_path: None,
        }
    }

    fn photo() -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(3, 2, image::Rgba([90, 80, 200, 255]));
        let mut bytes = Cursor::new(Vec::new());
        image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        normalize_avatar(&bytes.into_inner()).unwrap()
    }

    #[test]
    fn github_is_the_default_and_custom_details_take_precedence() {
        let mut profile = Profile {
            github: Some(github()),
            ..Default::default()
        };
        assert_eq!(profile.display_name(), Some("Mona"));
        profile.github.as_mut().unwrap().name = None;
        assert_eq!(profile.display_name(), Some("octocat"));
        profile.name = Some("My name".into());
        profile.github.as_mut().unwrap().name = Some("New GitHub name".into());
        assert_eq!(profile.display_name(), Some("My name"));
        assert!(normalize_avatar(b"this is not an image").is_err());
        let normalized = image::load_from_memory(&photo()).unwrap();
        assert_eq!((normalized.width(), normalized.height()), (256, 256));
    }

    #[gpui::test]
    fn photo_replacement_and_reset_persist_without_deleting_sources(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("original.png");
        std::fs::write(&original, photo()).unwrap();
        cx.update(|cx| {
            super::super::init(Default::default(), dir.path(), cx);
            let profile = Profile {
                github: Some(github()),
                name: Some("My name".into()),
                avatar_path: Some(original.to_string_lossy().into_owned()),
            };
            save_profile(profile, Some((PhotoTarget::Custom, photo())), cx).unwrap();
            let first = current(cx).profile.avatar_path.unwrap();
            save_profile(
                current(cx).profile,
                Some((PhotoTarget::Custom, photo())),
                cx,
            )
            .unwrap();
            let second = current(cx).profile.avatar_path.unwrap();
            assert_ne!(first, second);
            assert!(!Path::new(&first).exists());
            assert!(Path::new(&second).exists());
            assert!(original.exists());
            assert_eq!(
                super::super::UiSettings::load(dir.path())
                    .profile
                    .display_name(),
                Some("My name")
            );
            let mut profile = current(cx).profile;
            profile.name = None;
            profile.avatar_path = None;
            save_profile(profile, None, cx).unwrap();
            assert_eq!(
                super::super::UiSettings::load(dir.path())
                    .profile
                    .display_name(),
                Some("Mona")
            );
            assert!(!Path::new(&second).exists());
            assert!(original.exists());
        });
    }

    #[gpui::test]
    fn failed_profile_save_keeps_the_previous_photo(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            super::super::init(Default::default(), dir.path(), cx);
            save_profile(Profile::default(), Some((PhotoTarget::Custom, photo())), cx).unwrap();
            let before = current(cx);
            let saved_path = super::super::UiSettings::path(dir.path());
            std::fs::remove_file(&saved_path).unwrap();
            std::fs::create_dir(&saved_path).unwrap();
            assert!(
                save_profile(
                    before.profile.clone(),
                    Some((PhotoTarget::Custom, photo())),
                    cx
                )
                .is_err()
            );
            assert_eq!(current(cx), before);
            assert!(Path::new(before.profile.avatar_path.as_ref().unwrap()).exists());
            assert_eq!(
                std::fs::read_dir(dir.path().join(AVATAR_DIR))
                    .unwrap()
                    .count(),
                1
            );
        });
    }

    #[gpui::test]
    fn profile_controls_save_name_and_toggle_email_privacy(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            super::super::init(
                super::super::UiSettings {
                    profile: Profile {
                        github: Some(github()),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                dir.path(),
                cx,
            );
        });
        let (page, cx) = cx.add_window_view(|_, cx| ProfilePage::new(cx));
        page.update(cx, |page, cx| {
            page.name
                .update(cx, |input, cx| input.set_text("My custom name", cx));
        });
        cx.run_until_parked();
        let bounds = cx.debug_bounds("profile-save-name").unwrap();
        cx.simulate_click(bounds.center(), Default::default());
        cx.run_until_parked();
        assert_eq!(
            super::super::UiSettings::load(dir.path())
                .profile
                .display_name(),
            Some("My custom name")
        );
        let bounds = cx.debug_bounds("profile-blur-emails").unwrap();
        cx.simulate_click(bounds.center(), Default::default());
        cx.run_until_parked();
        assert!(super::super::UiSettings::load(dir.path()).blur_emails);
        cx.simulate_click(bounds.center(), Default::default());
        cx.run_until_parked();
        assert!(!super::super::UiSettings::load(dir.path()).blur_emails);
    }
}
