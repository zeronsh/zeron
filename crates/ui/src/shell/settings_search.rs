//! The search field atop the Settings sidebar. It finds any setting on any
//! page ([`settings::search`]) and lists the hits in the sections' place; a
//! pick opens the page and lands on the row. Settings still opens without
//! selecting a control, so the field waits for ⌘F (Ctrl+F) or a click.

use super::*;
use crate::settings::search::{Hit, Target};

/// The field, and the hits for what it holds.
pub(super) struct SettingsSearch {
    input: Entity<ComposerInput>,
    hits: Vec<Hit>,
    /// The hit Enter opens, moved by the arrows and the pointer alike.
    active: usize,
    results: gpui::ScrollHandle,
    _edits: Subscription,
}

impl Shell {
    /// The search, made on its first use.
    fn settings_search(&mut self, cx: &mut Context<Self>) -> &mut SettingsSearch {
        self.settings_search.get_or_insert_with(|| {
            // `PaletteSearch` binds text editing only: ↑↓, Enter and Escape
            // bubble to the field's own key handler.
            let input = cx.new(|cx| {
                ComposerInput::with_context("Search settings", "PaletteSearch", cx)
                    .with_text_metrics(13.0, 17.0)
                    .with_accessibility_role(gpui::Role::SearchInput)
                    .with_single_line()
                    .with_tab_stop()
            });
            let edits = cx.subscribe(&input, |shell: &mut Self, _, event, cx| {
                if matches!(event, ComposerInputEvent::Edited) {
                    shell.on_settings_search_edited(cx);
                }
            });
            SettingsSearch {
                input,
                hits: Vec::new(),
                active: 0,
                results: gpui::ScrollHandle::new(),
                _edits: edits,
            }
        })
    }

    /// Whether the hits stand in for the sections: the field holds a query.
    pub(super) fn settings_searching(&self, cx: &App) -> bool {
        self.settings_search
            .as_ref()
            .is_some_and(|search| !search.input.read(cx).text().trim().is_empty())
    }

    /// Whether the field holds the keyboard. A page a pick opens leaves it
    /// there, so the arrows go on walking the hits.
    pub(super) fn settings_search_focused(&self, window: &Window, cx: &App) -> bool {
        self.settings_search
            .as_ref()
            .is_some_and(|search| search.input.focus_handle(cx).is_focused(window))
    }

    fn on_settings_search_edited(&mut self, cx: &mut Context<Self>) {
        let cloud_offered = self.state.read(cx).cloud_offered();
        let Some(search) = self.settings_search.as_mut() else {
            return;
        };
        // Only what the nav shows: Cloud and Billing follow the account.
        search.hits = settings::search::search(search.input.read(cx).text(), |section| {
            section.shown_in_nav(cloud_offered)
        });
        search.active = 0;
        search.results.set_offset(Point::default());
        cx.notify();
    }

    /// Open hit `ix`: its page, landing on its row.
    fn open_settings_hit(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(search) = self.settings_search.as_mut() else {
            return;
        };
        let Some(entry) = search.hits.get(ix).map(|hit| hit.entry) else {
            return;
        };
        search.active = ix;
        self.open_settings(entry.section, cx);
        settings::search::reveal(entry.target, window);
    }

    /// ⌘F: the field, its query selected so typing replaces it.
    pub(super) fn focus_settings_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.settings_search(cx).input.clone();
        window.focus(&input.focus_handle(cx), cx);
        input.update(cx, |input, cx| input.select_all_text(cx));
    }

    /// Settings left: the next visit starts from the sections.
    pub(super) fn reset_settings_search(&mut self, cx: &mut Context<Self>) {
        if let Some(search) = &self.settings_search
            && !search.input.read(cx).text().is_empty()
        {
            search.input.update(cx, |input, cx| input.set_text("", cx));
        }
    }

    /// ↑↓ walk the hits and Enter opens one, from the input; on the clear
    /// button Enter clears. Escape clears the query first; once the field is
    /// empty it closes Settings, as anywhere on the page.
    fn on_settings_search_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.keystroke.modifiers.modified() {
            return;
        }
        let Some(search) = self.settings_search.as_mut() else {
            return;
        };
        let typing = search.input.focus_handle(cx).is_focused(window);
        let last = search.hits.len().checked_sub(1).filter(|_| typing);
        match (event.keystroke.key.as_str(), last) {
            ("up", Some(_)) => search.active = search.active.saturating_sub(1),
            ("down", Some(last)) => search.active = (search.active + 1).min(last),
            ("enter", Some(_)) => {
                cx.stop_propagation();
                if !event.is_held {
                    let active = search.active;
                    self.open_settings_hit(active, window, cx);
                }
                return;
            }
            ("escape", _) if !search.input.read(cx).text().is_empty() => {
                cx.stop_propagation();
                let input = search.input.clone();
                input.update(cx, |input, cx| input.set_text("", cx));
                // From the clear button too, which goes with the query.
                window.focus(&input.focus_handle(cx), cx);
                return;
            }
            _ => return,
        }
        search.results.scroll_to_item(search.active);
        cx.stop_propagation();
        cx.notify();
    }

    /// The field under Back: ⌘F's hint while it sits idle, a clear button
    /// once it holds a query.
    pub(super) fn render_settings_search_field(
        &mut self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let input = self.settings_search(cx).input.clone();
        let focus = input.focus_handle(cx);
        let empty = input.read(cx).text().is_empty();
        // A shortcut of the user's on ⌘F takes it before Settings can.
        let mod_f_free = !ShortcutId::ALL
            .into_iter()
            .any(|id| id.available() && self.settings.keymap.get(id) == "mod-f");
        let idle = empty && !focus.is_focused(window) && mod_f_free;
        crate::surface_chrome::input()
            .id("settings-search")
            .debug_selector(|| "settings-search".into())
            .flex_none()
            .w_full()
            .h(px(32.0))
            .rounded(px(8.0))
            .cursor_text()
            .hover(|style| style.bg(crate::theme::ink(0.055)))
            // Clicking the field's padding focuses the input too.
            .on_mouse_down(MouseButton::Left, {
                let focus = focus.clone();
                move |_, window, cx| {
                    window.focus(&focus, cx);
                    cx.stop_propagation();
                }
            })
            .on_key_down(cx.listener(Self::on_settings_search_key))
            // The magnifier takes a nav glyph's slot and gap, so the query
            // lines up with the section names it stands above.
            .gap(px(8.0))
            .child(
                div().w(px(16.0)).flex_none().flex().justify_center().child(
                    icon(icons::MAGNIFER)
                        .size(px(14.0))
                        .text_color(theme.text_muted),
                ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h(px(17.0))
                    .overflow_hidden()
                    .child(input.clone()),
            )
            .when(idle, |field| {
                field.child(popover::kbd_hint(theme, &badge_combo("mod-f")))
            })
            .when(!empty, |field| {
                field.child(
                    div()
                        .id("settings-search-clear")
                        .role(gpui::Role::Button)
                        .aria_label("Clear search")
                        .tooltip(settings::widgets::text_tooltip("Clear search"))
                        .tab_index(0)
                        .focus_visible(|s| s.border_2().border_color(theme.accent))
                        .size(px(20.0))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(4.0))
                        .cursor_pointer()
                        .hover(|style| style.bg(crate::theme::wash(0.08)))
                        .on_click(move |_, window, cx| {
                            input.update(cx, |input, cx| input.set_text("", cx));
                            // The button goes with the query: the input
                            // takes the keyboard, for the next one.
                            window.focus(&focus, cx);
                        })
                        .child(
                            icon(icons::CLOSE)
                                .size(px(12.0))
                                .text_color(theme.text_muted),
                        ),
                )
            })
            .into_any_element()
    }

    /// The hits, best first, where the sections were: each under its page's
    /// glyph, a page as its nav row, a setting over its page and block, and a
    /// shortcut with its binding.
    pub(super) fn render_settings_search_results(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(search) = self.settings_search.as_ref() else {
            return Empty.into_any_element();
        };
        if search.hits.is_empty() {
            return command_palette::palette_empty(
                theme,
                "No matching settings",
                "Try a setting, page, or shortcut.",
            )
            .into_any_element();
        }
        let field = search.input.focus_handle(cx);
        let rows = search.hits.iter().enumerate().map(|(ix, hit)| {
            let entry = hit.entry;
            let active = ix == search.active;
            let key = format!("settings-search-{:?}", entry.target);
            let hover_key = format!("{key}-hover");
            let glyph = motion::hover_blend(
                &hover_key,
                if active { theme.text } else { theme.text_muted },
                theme.text,
            );
            let trail = match (entry.target, entry.group) {
                (Target::Page(_), _) => None,
                (_, Some(group)) => Some(format!("{} · {group}", entry.section.label())),
                (_, None) => Some(entry.section.label().to_string()),
            };
            let binding = match entry.target {
                Target::Shortcut(id) => Some(self.settings.keymap.get(id))
                    .filter(|combo| !combo.is_empty())
                    .map(badge_combo),
                Target::Setting(_) | Target::Page(_) => None,
            };
            let title = sidebar_faded_label(
                format!("{key}-title").into(),
                false,
                div()
                    .text_size(crate::typography::ui_rems(13.0))
                    .line_height(px(17.0))
                    .child(popover::search_highlight_ranges(
                        entry.title.into(),
                        hit.ranges.clone(),
                        theme,
                    )),
            );
            settings::widgets::section_tab(
                theme,
                active,
                if active { 1.0 } else { 0.0 },
                key.clone(),
                hover_key,
            )
            .debug_selector(move || format!("settings-search-hit-{ix}"))
            .role(gpui::Role::ListBoxOption)
            .aria_label(entry.title)
            .aria_selected(active)
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, {
                let field = field.clone();
                move |_, window, cx| {
                    // Settings would take the keyboard on the way down: it
                    // stays with the field, for the next pick.
                    window.prevent_default();
                    window.focus(&field, cx);
                }
            })
            .on_mouse_move(cx.listener(move |this, _: &gpui::MouseMoveEvent, _, cx| {
                if let Some(search) = this.settings_search.as_mut()
                    && search.active != ix
                {
                    search.active = ix;
                    cx.notify();
                }
            }))
            .on_click(
                cx.listener(move |this, _, window, cx| this.open_settings_hit(ix, window, cx)),
            )
            .child(icon(entry.section.icon()).size(px(16.0)).text_color(glyph))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(title)
                    .children(trail.map(|trail| {
                        sidebar_faded_label(
                            format!("{key}-trail").into(),
                            false,
                            div()
                                .text_size(crate::typography::ui_rems(11.0))
                                .line_height(px(14.0))
                                .font_weight(gpui::FontWeight::NORMAL)
                                .text_color(theme.text_muted)
                                .child(trail),
                        )
                    })),
            )
            .children(binding.map(|binding| popover::kbd_hint(theme, &binding)))
        });
        // Rows are the scroll's own children, so the arrows can keep the
        // active one in view.
        crate::edge_fade::edge_faded(
            16.0,
            true,
            true,
            div()
                .id("settings-search-results")
                .role(gpui::Role::ListBox)
                .aria_label("Settings search results")
                .size_full()
                .overflow_y_scroll()
                .track_scroll(&search.results)
                .px(px(Theme::SPACE_SM))
                .pt(px(Theme::SPACE_MD))
                .pb(px(Theme::SPACE_SM))
                .flex()
                .flex_col()
                .gap(px(2.0))
                .children(rows),
        )
        .fade_overflow_y(&search.results)
        .into_any_element()
    }
}

/// Native visual QA types into the field as the keyboard would.
#[cfg(feature = "appshots-fixture")]
impl Shell {
    pub fn fixture_settings_search(&mut self, query: &str, cx: &mut Context<Self>) {
        let input = self.settings_search(cx).input.clone();
        input.update(cx, |input, cx| input.set_text(query, cx));
    }
}

#[cfg(test)]
mod tests {
    use gpui::{TestAppContext, VisualTestContext};
    use zeron_theme::{AccentSelection, SurfacePreference, ThemeSelection};

    use super::*;
    use crate::settings::search::{
        SettingId, lit, per_provider_rows, take_anchored, unanchored, unindexed,
    };
    use crate::settings::widgets::take_row_titles;

    /// A shell on Settings → General, every page drawable, motion reduced.
    fn open_settings(
        cx: &mut TestAppContext,
    ) -> (Entity<Shell>, &mut VisualTestContext, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            settings::init(UiSettings::default(), dir.path(), cx);
            crate::history::init(
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                cx,
            );
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
            crate::appearance::init(
                crate::appearance::AppearanceMode::Dark,
                ThemeSelection::default(),
                AccentSelection::default(),
                SurfacePreference::Opaque,
                cx,
            );
        });
        let data_dir = dir.path().to_path_buf();
        let (shell, cx) = cx.add_window_view(move |_, cx| {
            let state = cx.new(|_| AppState::new());
            Shell::new(
                state,
                EngineBootConfig {
                    data_dir,
                    ipc_port: 0,
                    edge_url: "http://127.0.0.1:1".into(),
                    edge_token: None,
                    org_id: None,
                    workos_client_id: None,
                    default_harness: zeron_proto::HarnessId::Mock,
                },
                cx,
            )
        });
        cx.update(|_, cx| motion::set_reduced_motion(cx, true));
        cx.simulate_resize(gpui::size(px(1200.0), px(800.0)));
        shell.update(cx, |shell, cx| {
            // No engine in tests: render the workspace, not the boot gate.
            shell.debug_gate = Some(GatePhase::Ready);
            shell.splash = SplashPhase::Gone;
            shell.open_settings(SettingsSection::General, cx);
        });
        draw(cx);
        (shell, cx, dir)
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear();
        });
    }

    fn active_hit(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Option<Target> {
        shell.read_with(cx, |shell, _| {
            let search = shell.settings_search.as_ref()?;
            search.hits.get(search.active).map(|hit| hit.entry.target)
        })
    }

    fn field_focused(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> bool {
        cx.update(|window, cx| shell.read(cx).settings_search_focused(window, cx))
    }

    /// ⌘F, then `query` typed into the field.
    fn search_for(query: &str, cx: &mut VisualTestContext) {
        cx.simulate_keystrokes(&platform_combo("mod-f"));
        // The focused field takes typing once it has drawn.
        draw(cx);
        cx.simulate_input(query);
        draw(cx);
    }

    #[gpui::test]
    fn the_keyboard_finds_a_setting_and_lands_on_it(cx: &mut TestAppContext) {
        let (shell, cx, _dir) = open_settings(cx);
        // ⌘F reaches the field without a click.
        cx.simulate_keystrokes(&platform_combo("mod-f"));
        assert!(field_focused(&shell, cx));
        // A query lists its hits in place of the sections.
        search_for("wrap", cx);
        assert!(shell.read_with(cx, |shell, cx| shell.settings_searching(cx)));
        assert_eq!(
            active_hit(&shell, cx),
            Some(Target::Setting(SettingId::WordWrap))
        );
        // Enter opens the page the hit lives on and lights its row.
        take_anchored();
        cx.simulate_keystrokes("enter");
        draw(cx);
        draw(cx);
        shell.read_with(cx, |shell, _| {
            assert_eq!(shell.route, Route::Settings(SettingsSection::Files));
        });
        assert!(take_anchored().contains(&Target::Setting(SettingId::WordWrap)));
        assert_eq!(lit(), Some(Target::Setting(SettingId::WordWrap)));
        // The hits stay for the next pick: ↓ walks them.
        cx.simulate_keystrokes("down");
        assert_ne!(
            active_hit(&shell, cx),
            Some(Target::Setting(SettingId::WordWrap))
        );
        // Escape clears the query first, and the sections come back…
        cx.simulate_keystrokes("escape");
        draw(cx);
        shell.read_with(cx, |shell, cx| {
            assert!(!shell.settings_searching(cx));
            assert_eq!(shell.route, Route::Settings(SettingsSection::Files));
        });
        // …then leaves Settings, as anywhere on the page.
        cx.simulate_keystrokes("escape");
        shell.read_with(cx, |shell, _| assert_eq!(shell.route, Route::Chat));
        assert_eq!(lit(), None, "leaving Settings drops the landing");
    }

    #[gpui::test]
    fn a_shortcut_hit_opens_the_page_that_records_it(cx: &mut TestAppContext) {
        let (shell, cx, _dir) = open_settings(cx);
        search_for("left", cx);
        assert_eq!(
            active_hit(&shell, cx),
            Some(Target::Shortcut(ShortcutId::ToggleSidebar))
        );
        cx.simulate_keystrokes("enter");
        draw(cx);
        draw(cx);
        shell.read_with(cx, |shell, _| {
            assert_eq!(shell.route, Route::Settings(SettingsSection::Shortcuts));
        });
        assert_eq!(lit(), Some(Target::Shortcut(ShortcutId::ToggleSidebar)));
    }

    /// A pick leaves the keyboard in the field, for the next one: Appshots,
    /// which takes it as it opens, leaves it there, and so does a click,
    /// which Settings would take it on. Appshots shows only where it captures
    /// (`appshots::is_desktop`); elsewhere search hides its page.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[gpui::test]
    fn a_pick_keeps_the_keyboard_in_the_field(cx: &mut TestAppContext) {
        let (shell, cx, _dir) = open_settings(cx);
        search_for("destination", cx);
        assert_eq!(
            active_hit(&shell, cx),
            Some(Target::Setting(SettingId::AppshotDestination))
        );
        cx.simulate_keystrokes("enter");
        draw(cx);
        draw(cx);
        shell.read_with(cx, |shell, _| {
            assert_eq!(shell.route, Route::Settings(SettingsSection::Appshots));
        });
        assert!(field_focused(&shell, cx));
        // Back on General, the keyboard elsewhere: a click on an Appshots
        // hit opens it, and the field has the keyboard.
        shell.update(cx, |shell, cx| {
            shell.open_settings(SettingsSection::General, cx)
        });
        cx.update(|window, cx| {
            let settings = shell.read(cx).settings_focus.clone();
            window.focus(&settings, cx);
        });
        draw(cx);
        assert!(!field_focused(&shell, cx));
        let hit = cx.debug_bounds("settings-search-hit-0").expect("a hit");
        cx.simulate_click(hit.center(), gpui::Modifiers::none());
        draw(cx);
        draw(cx);
        shell.read_with(cx, |shell, _| {
            assert_eq!(shell.route, Route::Settings(SettingsSection::Appshots));
        });
        assert!(field_focused(&shell, cx));
    }

    /// A dialog over the page keeps the keyboard: ⌘F waits until it closes.
    #[gpui::test]
    fn mod_f_leaves_an_open_dialog_the_keyboard(cx: &mut TestAppContext) {
        let (shell, cx, _dir) = open_settings(cx);
        shell.update(cx, |shell, cx| {
            shell.open_settings(SettingsSection::Archived, cx)
        });
        draw(cx);
        shell.update(cx, |shell, cx| {
            let page = shell.archived_page.clone().expect("the Archived page");
            page.update(cx, |page, cx| page.open_delete_all_confirm(cx));
        });
        draw(cx);
        cx.simulate_keystrokes(&platform_combo("mod-f"));
        assert!(!field_focused(&shell, cx));
        // Escape closes the dialog, and ⌘F reaches the field again.
        cx.simulate_keystrokes("escape");
        draw(cx);
        shell.read_with(cx, |shell, _| {
            assert_eq!(shell.route, Route::Settings(SettingsSection::Archived));
        });
        cx.simulate_keystrokes(&platform_combo("mod-f"));
        assert!(field_focused(&shell, cx));
    }

    /// On the clear button, Enter is the button's: it clears the query and
    /// opens no hit.
    #[gpui::test]
    fn enter_on_the_clear_button_only_clears(cx: &mut TestAppContext) {
        let (shell, cx, _dir) = open_settings(cx);
        search_for("wrap", cx);
        cx.simulate_keystrokes("tab");
        draw(cx);
        assert!(
            !field_focused(&shell, cx),
            "Tab moves on to the clear button"
        );
        // gpui clicks a focused button as Enter comes back up.
        cx.simulate_keystrokes("enter");
        cx.simulate_event(gpui::KeyUpEvent {
            keystroke: gpui::Keystroke::parse("enter").unwrap(),
        });
        draw(cx);
        shell.read_with(cx, |shell, cx| {
            assert_eq!(shell.route, Route::Settings(SettingsSection::General));
            assert!(!shell.settings_searching(cx));
        });
        // The button went with the query; the input has the keyboard back.
        assert!(field_focused(&shell, cx));
    }

    /// However Settings is left — Back, Escape, or a route of its own like
    /// ⌘N's or history's — the next visit starts from the sections.
    #[gpui::test]
    fn leaving_settings_starts_the_next_visit_from_the_sections(cx: &mut TestAppContext) {
        let (shell, cx, _dir) = open_settings(cx);
        search_for("theme", cx);
        shell.update(cx, |shell, cx| {
            assert!(shell.settings_searching(cx));
            shell.close_settings(cx);
            shell.open_settings(SettingsSection::General, cx);
            assert!(!shell.settings_searching(cx));
        });
        draw(cx);
        search_for("theme", cx);
        shell.update(cx, |shell, cx| {
            assert!(shell.settings_searching(cx));
            shell.set_route(Route::Chat, cx);
            shell.open_settings(SettingsSection::General, cx);
            assert!(!shell.settings_searching(cx));
        });
    }

    /// The index and the pages agree. Every row a page draws can be found,
    /// and no hit opens a page to land nowhere: each row the index lists is
    /// anchored there, or its stand-in is while it hides. Cloud and Billing
    /// draw their rows from account data that tests never load.
    #[gpui::test]
    fn the_index_and_the_pages_agree(cx: &mut TestAppContext) {
        let (shell, cx, _dir) = open_settings(cx);
        let sections = SettingsSection::ALL
            .into_iter()
            .filter(|section| section.visible_in_nav() && !section.needs_account());
        for section in sections {
            take_anchored();
            take_row_titles();
            shell.update(cx, |shell, cx| shell.open_settings(section, cx));
            draw(cx);
            assert_eq!(
                unindexed(section, &take_row_titles()),
                Vec::<String>::new(),
                "{section:?} draws settings the search cannot find"
            );
            assert_eq!(
                unanchored(section, &take_anchored()),
                [],
                "{section:?} has hits that land nowhere"
            );
        }
        // A provider's details draw only once it is expanded, with its data
        // loaded: draw every section they can show, and hold those rows to
        // the same agreement. The provider's own name is not a setting.
        let harness = zeron_proto::HarnessId::ClaudeCode;
        shell.update(cx, |shell, cx| {
            shell.open_settings(SettingsSection::Harnesses, cx)
        });
        draw(cx);
        take_anchored();
        take_row_titles();
        shell.update(cx, |shell, cx| {
            let page = shell.harnesses_page.clone().expect("the Providers page");
            page.update(cx, |page, cx| page.show_provider_details(harness, cx));
        });
        draw(cx);
        let drawn: Vec<_> = take_row_titles()
            .into_iter()
            .filter(|title| *title != format!("{harness:?}"))
            .collect();
        assert_eq!(
            unindexed(SettingsSection::Harnesses, &drawn),
            Vec::<String>::new(),
            "an expanded provider draws settings the search cannot find"
        );
        let anchored = take_anchored();
        let missing: Vec<_> = per_provider_rows()
            .into_iter()
            .filter(|row| !anchored.contains(row))
            .collect();
        assert_eq!(
            missing,
            [],
            "an expanded provider's hits land on the list, not their row"
        );
    }
}
