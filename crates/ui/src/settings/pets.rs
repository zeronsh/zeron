//! Settings → Pets: show the desktop pet, size it, and pick which one.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    AnyElement, Context, RenderImage, SharedString, Task, Window, div, img, prelude::*, px,
};

use crate::pets::{self, PetInfo};
use crate::settings::{self, SavePolicy, widgets};
use crate::theme::Theme;

/// Size presets (pet width in px), inside [`pets::SIZE_MIN`]..[`pets::SIZE_MAX`].
const SIZES: [(&str, f32); 4] = [
    ("Small", 80.0),
    ("Medium", pets::SIZE_DEFAULT),
    ("Large", 160.0),
    ("Huge", 224.0),
];

struct Entry {
    pet: PetInfo,
    preview: Option<Arc<RenderImage>>,
}

pub struct PetsPage {
    data_dir: Option<PathBuf>,
    entries: Vec<Entry>,
    issues: Vec<String>,
    _scan: Task<()>,
}

impl PetsPage {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut page = Self {
            data_dir: settings::data_dir(cx),
            entries: Vec::new(),
            issues: Vec::new(),
            _scan: Task::ready(()),
        };
        page.scan(cx);
        page
    }

    /// Re-read the pets folder and decode each preview off the UI thread.
    fn scan(&mut self, cx: &mut Context<Self>) {
        let data_dir = self.data_dir.clone();
        self._scan = cx.spawn(async move |this, cx| {
            let (entries, issues) = cx
                .background_executor()
                .spawn(async move {
                    let (pets, mut issues) = pets::library(data_dir.as_deref());
                    let entries: Vec<Entry> = pets
                        .into_iter()
                        .filter_map(|pet| match pets::preview(&pet) {
                            Ok(preview) => Some(Entry {
                                pet,
                                preview: Some(preview),
                            }),
                            Err(error) => {
                                issues.push(format!("{}: {error}", pet.id));
                                None
                            }
                        })
                        .collect();
                    (entries, issues)
                })
                .await;
            this.update(cx, |page, cx| {
                let old = std::mem::replace(&mut page.entries, entries);
                page.issues = issues;
                // Free the replaced previews' GPU textures.
                cx.defer(move |cx| {
                    for image in old.into_iter().filter_map(|entry| entry.preview) {
                        cx.drop_image(image, None);
                    }
                });
                cx.notify();
            })
            .ok();
        });
    }

    fn open_folder(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = self.data_dir.as_deref().map(pets::pets_dir) else {
            return;
        };
        if let Err(error) = std::fs::create_dir_all(&dir) {
            self.issues = vec![format!("Unable to create {}: {error}", dir.display())];
            cx.notify();
            return;
        }
        cx.open_with_system(&dir);
    }
}

fn change(cx: &mut Context<PetsPage>, mutate: impl FnOnce(&mut settings::UiSettings)) {
    settings::update(SavePolicy::Debounced, cx, mutate);
    cx.notify();
}

fn row(
    theme: &Theme,
    first: bool,
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    control: AnyElement,
) -> gpui::Div {
    widgets::card_row(theme, first)
        .flex_wrap()
        .child(
            div()
                .flex_1()
                .min_w(px(160.0))
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(widgets::row_title(theme, title.into()))
                .child(
                    div()
                        .text_size(crate::typography::ui_rems(12.0))
                        .line_height(crate::typography::ui_rems(16.0))
                        .text_color(theme.text_muted)
                        .child(description.into()),
                ),
        )
        .child(control)
}

impl Render for PetsPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).for_settings_surface();
        let accent = theme.accent;
        let current = settings::current(cx);
        let enabled = current.pet_enabled;
        let action = |id: SharedString, label: SharedString, tone: widgets::ActionTone| {
            widgets::text_action(&theme, tone, label.clone())
                .id(id)
                .role(gpui::Role::Button)
                .aria_label(label)
                .tab_index(0)
                .focus_visible(move |style| style.border_2().border_color(accent))
        };

        let toggle = widgets::toggle_switch(&theme, enabled, "pet-enabled")
            .id("pet-enabled")
            .role(gpui::Role::Switch)
            .aria_label("Show the desktop pet")
            .aria_toggled(if enabled {
                gpui::Toggled::True
            } else {
                gpui::Toggled::False
            })
            .tab_index(0)
            .focus_visible(move |style| style.border_2().border_color(accent))
            .cursor_pointer()
            .on_click(cx.listener(move |_, _, _, cx| {
                change(cx, |s| s.pet_enabled = !enabled);
            }));

        let sizes = div()
            .flex()
            .flex_wrap()
            .gap(px(6.0))
            .children(SIZES.iter().map(|(label, width)| {
                let width = *width;
                let active = (current.pet_size - width).abs() < 0.5;
                let tone = if active {
                    widgets::ActionTone::Filled
                } else {
                    widgets::ActionTone::Quiet
                };
                action(format!("pet-size-{label}").into(), (*label).into(), tone)
                    .aria_toggled(if active {
                        gpui::Toggled::True
                    } else {
                        gpui::Toggled::False
                    })
                    .on_click(cx.listener(move |_, _, _, cx| {
                        change(cx, |s| s.pet_size = width);
                    }))
            }));

        let general = widgets::section_card(&theme)
            .child(row(
                &theme,
                true,
                "Show pet",
                "A companion that floats above your apps and acts out what agents are \
                 doing. Drag it anywhere; click its bubble to open that chat.",
                toggle.into_any_element(),
            ))
            .child(row(
                &theme,
                false,
                "Size",
                "How wide the pet is on screen.",
                sizes.into_any_element(),
            ));

        let selected_id = current.pet_id.clone();
        let pets = widgets::section_card(&theme).children(self.entries.iter().enumerate().map(
            |(index, entry)| {
                let selected = entry.pet.id == selected_id;
                let control = if selected {
                    widgets::badge_active(&theme, "Selected").into_any_element()
                } else {
                    let id = entry.pet.id.clone();
                    action(
                        format!("pet-use-{}", entry.pet.id).into(),
                        "Use pet".into(),
                        widgets::ActionTone::Filled,
                    )
                    .on_click(cx.listener(move |_, _, _, cx| {
                        let id = id.clone();
                        change(cx, |s| {
                            s.pet_id = id;
                            s.pet_enabled = true;
                        });
                    }))
                    .into_any_element()
                };
                let thumbnail = div()
                    .flex_none()
                    .w(px(48.0))
                    .h(px(52.0))
                    .children(entry.preview.clone().map(|p| img(p).size_full()));
                widgets::card_row(&theme, index == 0)
                    .gap(px(12.0))
                    .child(thumbnail)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(widgets::row_title(&theme, entry.pet.name.clone()))
                            .child(
                                div()
                                    .text_size(crate::typography::ui_rems(12.0))
                                    .text_color(theme.text_muted)
                                    .truncate()
                                    .child(entry.pet.description.clone()),
                            ),
                    )
                    .child(control)
            },
        ));

        let folder_hint: SharedString = match self.data_dir.as_deref() {
            Some(dir) => format!(
                "Add a pet as a folder in {}: a pet.json with displayName, description and \
                 spritesheetPath, next to a 1536×2288 (or 1536×1872) PNG or WebP sheet of \
                 192×208 cells — the Codex pet format.",
                pets::pets_dir(dir).display()
            )
            .into(),
            None => "Custom pets are unavailable: Zeron has no data folder.".into(),
        };

        let footer = div()
            .mt(px(10.0))
            .flex()
            .flex_wrap()
            .gap(px(8.0))
            .child(
                action(
                    "pets-open-folder".into(),
                    "Open pets folder".into(),
                    widgets::ActionTone::Filled,
                )
                .on_click(cx.listener(|this, _, _, cx| this.open_folder(cx))),
            )
            .child(
                action(
                    "pets-rescan".into(),
                    "Refresh".into(),
                    widgets::ActionTone::Quiet,
                )
                .on_click(cx.listener(|this, _, _, cx| this.scan(cx))),
            );

        widgets::scroll_faded(
            "pets-settings-scroll",
            div()
                .id("pets-settings-page")
                .size_full()
                .overflow_y_scroll()
                .child(
                    widgets::page_column()
                        .child(widgets::page_header(&theme, "Pets", None))
                        .child(
                            widgets::page_subtitle(
                                &theme,
                                "Choose a desktop companion that keeps an eye on your agents.",
                            )
                            .line_height(px(20.0)),
                        )
                        .child(general)
                        .child(
                            div()
                                .mt(px(24.0))
                                .child(widgets::section_label(&theme, "Your pets")),
                        )
                        .child(pets)
                        .children((!self.issues.is_empty()).then(|| {
                            div()
                                .mt(px(10.0))
                                .child(widgets::warning_strip(&theme, self.issues.join("\n")))
                        }))
                        .child(footer)
                        .child(
                            div()
                                .mt(px(12.0))
                                .px(px(8.0))
                                .text_size(px(12.0))
                                .text_color(theme.text_muted)
                                .child(folder_hint),
                        ),
                ),
        )
    }
}
