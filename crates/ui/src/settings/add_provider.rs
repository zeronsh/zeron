//! Settings → Providers → Add a provider: a search over the ACP registry
//! right under the provider list, plus a custom command for any local agent
//! that speaks ACP. Rows reuse the provider rows' anatomy (tile, title, meta,
//! trailing action), so an agent looks the same before and after it joins.
//!
//! The engine does the work (`ListAcpRegistry` / `AddAcpAgent`, forwarded to
//! the target device like every other provider RPC): availability depends
//! on the device (its platform, whether npm/uv exist) and adding installs the
//! agent there before it joins the list.

use gpui::{
    AnyElement, Context, Entity, IntoElement, SharedString, Subscription, Task, div, prelude::*, px,
};

use zeron_engine::registry::{AcpRegistryListing, HarnessDescriptor};
use zeron_rpc::methods;

use super::{DETAILS_INSET, HarnessesPage, provider_tile};
use crate::composer::{ComposerInput, ComposerInputEvent};
use crate::popover::{self, Loadable};
use crate::settings::widgets;
use crate::theme::Theme;

/// Registry rows shown before "Show all" while nothing is typed.
const COLLAPSED_ROWS: usize = 5;

pub(super) struct AddProvider {
    search: Entity<ComposerInput>,
    name: Entity<ComposerInput>,
    command: Entity<ComposerInput>,
    env: Entity<ComposerInput>,
    registry: Loadable<Vec<AcpRegistryListing>>,
    show_all: bool,
    custom_open: bool,
    /// The registry id being added, or `""` for the custom command.
    adding: Option<String>,
    error: Option<String>,
    load_task: Option<Task<()>>,
    add_task: Option<Task<()>>,
    _events: Vec<Subscription>,
}

/// A registry row's state on the target device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum RowState {
    Addable,
    Added,
    BuiltIn,
    Unavailable,
}

fn row_state(listing: &AcpRegistryListing) -> RowState {
    if listing.built_in.is_some() {
        RowState::BuiltIn
    } else if listing.added {
        RowState::Added
    } else if listing.unavailable.is_some() {
        RowState::Unavailable
    } else {
        RowState::Addable
    }
}

/// Rows matching `query` (name, id, then description), best match first;
/// addable agents ahead of the ones already added, built in or unavailable.
fn filtered<'a>(listings: &'a [AcpRegistryListing], query: &str) -> Vec<&'a AcpRegistryListing> {
    let query = query.trim().to_lowercase();
    let mut rows: Vec<(u8, RowState, String, &AcpRegistryListing)> = listings
        .iter()
        .filter_map(|listing| {
            let name = listing.name.to_lowercase();
            let rank =
                if query.is_empty() || name.starts_with(&query) || listing.id.starts_with(&query) {
                    0
                } else if name.contains(&query) || listing.id.contains(&query) {
                    1
                } else if listing.description.to_lowercase().contains(&query) {
                    2
                } else {
                    return None;
                };
            Some((rank, row_state(listing), name, listing))
        })
        .collect();
    rows.sort_by(|a, b| (a.0, a.1, &a.2).cmp(&(b.0, b.1, &b.2)));
    rows.into_iter().map(|(.., listing)| listing).collect()
}

impl HarnessesPage {
    /// Build the section once the target device supports it, and load the
    /// registry as that device sees it.
    pub(super) fn ensure_add_provider(&mut self, cx: &mut Context<Self>) {
        if self.add_provider.is_some() || !self.supports_acp_agents(cx) {
            return;
        }
        let field = |placeholder: &'static str, cx: &mut Context<Self>| {
            cx.new(|cx| {
                ComposerInput::new(placeholder, cx)
                    .with_single_line()
                    .with_tab_stop()
            })
        };
        let search = cx.new(|cx| {
            ComposerInput::new("Search the ACP registry", cx)
                .with_accessibility_role(gpui::Role::SearchInput)
                .with_single_line()
                .with_tab_stop()
        });
        let name = field("Name (optional)", cx);
        let command = field("my-agent --acp", cx);
        let env = field("API_KEY=… (optional)", cx);
        let mut events = vec![cx.observe(&search, |_, _, cx| cx.notify())];
        for input in [&name, &command, &env] {
            events.push(cx.subscribe(input, |page: &mut Self, _, event, cx| {
                if matches!(event, ComposerInputEvent::Submitted) {
                    page.submit_custom_agent(cx);
                }
            }));
        }
        self.add_provider = Some(AddProvider {
            search,
            name,
            command,
            env,
            registry: Loadable::Loading,
            show_all: false,
            custom_open: false,
            adding: None,
            error: None,
            load_task: None,
            add_task: None,
            _events: events,
        });
        self.load_registry(false, cx);
    }

    fn load_registry(&mut self, force: bool, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let params = self.with_target(serde_json::json!({ "force": force }));
        let target = self.target_device.clone();
        let Some(flow) = self.add_provider.as_mut() else {
            return;
        };
        flow.registry = Loadable::Loading;
        flow.load_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::LIST_ACP_REGISTRY, params)
                .await
                .map_err(|error| error.to_string())
                .and_then(|value| {
                    serde_json::from_value::<Vec<AcpRegistryListing>>(value)
                        .map_err(|error| error.to_string())
                });
            this.update(cx, |page, cx| {
                if page.target_device != target {
                    return;
                }
                if let Some(flow) = page.add_provider.as_mut() {
                    flow.registry = match result {
                        Ok(list) => Loadable::Ready(list),
                        Err(error) => Loadable::Error(error),
                    };
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    fn add_registry_agent(&mut self, registry_id: String, cx: &mut Context<Self>) {
        self.add_agent(
            registry_id.clone(),
            serde_json::json!({ "registryId": registry_id }),
            cx,
        );
    }

    fn submit_custom_agent(&mut self, cx: &mut Context<Self>) {
        let Some(flow) = self.add_provider.as_mut() else {
            return;
        };
        let name = flow.name.read(cx).text().trim().to_string();
        let command = flow.command.read(cx).text().to_string();
        let env = flow.env.read(cx).text().to_string();
        let parsed = zeron_harness::acp_agents::split_command_line(&command).and_then(|words| {
            let mut words = words.into_iter();
            let program = words
                .next()
                .ok_or_else(|| "Enter the command that starts the agent".to_string())?;
            let env = zeron_harness::acp_agents::parse_env_assignments(&env)?;
            Ok((program, words.collect::<Vec<_>>(), env))
        });
        let (program, args, env) = match parsed {
            Ok(parsed) => parsed,
            Err(error) => {
                flow.error = Some(error);
                cx.notify();
                return;
            }
        };
        self.add_agent(
            String::new(),
            serde_json::json!({ "name": name, "program": program, "args": args, "env": env }),
            cx,
        );
    }

    /// `AddAcpAgent` on the target device. The reply is the device's fresh
    /// catalog, so the new provider shows up (enabled) in one round trip, and
    /// the registry reloads so its row reads "Added".
    fn add_agent(&mut self, key: String, params: serde_json::Value, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let params = self.with_target(params);
        let target = self.target_device.clone();
        let Some(flow) = self.add_provider.as_mut() else {
            return;
        };
        if flow.adding.is_some() {
            return;
        }
        flow.adding = Some(key);
        flow.error = None;
        flow.add_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::ADD_ACP_AGENT, params)
                .await
                .map_err(|error| error.to_string())
                .and_then(|value| {
                    serde_json::from_value::<Vec<HarnessDescriptor>>(value)
                        .map_err(|error| error.to_string())
                });
            this.update(cx, |page, cx| {
                if page.target_device != target {
                    return;
                }
                let added = result.is_ok();
                match result {
                    Ok(list) => {
                        page.set_catalog(list, cx);
                        crate::pickers::bump_harness_catalog(cx);
                    }
                    Err(error) => {
                        if let Some(flow) = page.add_provider.as_mut() {
                            flow.error = Some(error);
                        }
                    }
                }
                if let Some(flow) = page.add_provider.as_mut() {
                    flow.adding = None;
                    if added {
                        flow.custom_open = false;
                        for input in [&flow.search, &flow.name, &flow.command, &flow.env] {
                            input.update(cx, |input, cx| input.set_text("", cx));
                        }
                    }
                }
                if added {
                    page.load_registry(false, cx);
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// The "Add a provider" section under the provider list.
    pub(super) fn render_add_provider(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        self.ensure_add_provider(cx);
        let flow = self.add_provider.as_ref()?;
        let query = flow.search.read(cx).text().to_string();
        let adding = flow.adding.clone();
        let show_all = flow.show_all || !query.trim().is_empty();
        let error = flow.error.clone();
        let search = div()
            .mx(px(16.0))
            .min_h(px(48.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(12.0))
            .child(
                div()
                    .flex_none()
                    .w(px(36.0))
                    .flex()
                    .justify_center()
                    .child(popover::palette_search_icon(theme)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(crate::typography::ui_rems(13.0))
                    .child(flow.search.clone()),
            );
        let mut card = widgets::section_card(theme).mt(px(0.0)).child(search);
        match &flow.registry {
            Loadable::Idle | Loadable::Loading => {
                card = card.child(
                    div()
                        .px(px(16.0))
                        .pb(px(12.0))
                        .border_t_1()
                        .border_color(widgets::row_divider(theme))
                        .child(popover::skeleton_rows(
                            "acp-registry-skeleton",
                            theme,
                            3,
                            cx.entity_id(),
                            cx,
                        )),
                );
            }
            Loadable::Error(message) => {
                let message = format!("Couldn't load the ACP registry — {message}");
                card = card.child(
                    widgets::card_row(theme, false)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_color(theme.danger_muted)
                                .child(message),
                        )
                        .child(
                            widgets::ghost_action(theme)
                                .id("acp-registry-retry")
                                .flex_none()
                                .on_click(
                                    cx.listener(|page, _, _, cx| page.load_registry(true, cx)),
                                )
                                .child("Retry"),
                        ),
                );
            }
            Loadable::Ready(listings) => {
                let rows = filtered(listings, &query);
                let total = rows.len();
                let shown = if show_all {
                    total
                } else {
                    total.min(COLLAPSED_ROWS)
                };
                if total == 0 {
                    card =
                        card.child(widgets::card_row(theme, false).child(
                            div().text_color(theme.text_muted).child(
                                "No agents match — try another name, or add a custom command.",
                            ),
                        ));
                }
                let rows: Vec<AnyElement> = rows
                    .into_iter()
                    .take(shown)
                    .enumerate()
                    .map(|(ix, listing)| self.registry_row(ix, listing, &adding, theme, cx))
                    .collect();
                card = card.children(rows);
                if shown < total {
                    card = card.child(
                        widgets::card_row(theme, false).min_h(px(44.0)).child(
                            // Text on the titles' edge (tile + gap, less
                            // the button's own padding).
                            widgets::ghost_action(theme)
                                .id("acp-registry-show-all")
                                .ml(px(4.0 + 36.0 + 12.0 - 10.0))
                                .on_click(cx.listener(|page, _, _, cx| {
                                    if let Some(flow) = page.add_provider.as_mut() {
                                        flow.show_all = true;
                                        cx.notify();
                                    }
                                }))
                                .child(format!("Show all {total}")),
                        ),
                    );
                }
            }
        }
        card = card.child(self.custom_row(theme, cx));
        let section = widgets::section(theme, "Add a provider", card)
            .when_some(error, |el, error| {
                el.child(widgets::error_strip(theme, error))
            });
        Some(section.into_any_element())
    }

    fn registry_row(
        &self,
        ix: usize,
        listing: &AcpRegistryListing,
        adding: &Option<String>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = row_state(listing);
        let installing = adding.as_deref() == Some(listing.id.as_str());
        let mark = crate::acp_icons::mark_for_url(listing.icon.as_deref(), cx);
        let mut meta: Vec<AnyElement> = Vec::new();
        if !listing.description.trim().is_empty() {
            meta.push(
                div()
                    .truncate()
                    .child(listing.description.clone())
                    .into_any_element(),
            );
        }
        let trailing: AnyElement = if installing {
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(6.0))
                .text_size(crate::typography::ui_rems(12.0))
                .text_color(theme.text_muted)
                .child(crate::loaders::mini_mono_spinner(
                    format!("acp-registry-spinner-{ix}"),
                    1.5,
                    theme.text_muted,
                    cx.entity_id(),
                    cx,
                ))
                .child("Installing…")
                .into_any_element()
        } else if state == RowState::Addable {
            let id = listing.id.clone();
            widgets::ghost_action(theme)
                .id(("acp-registry-add", ix))
                .flex_none()
                .when(adding.is_none(), |el| {
                    el.on_click(
                        cx.listener(move |page, _, _, cx| page.add_registry_agent(id.clone(), cx)),
                    )
                })
                .when(adding.is_some(), |el| el.opacity(0.5))
                .child("Add")
                .into_any_element()
        } else {
            let label: SharedString = match state {
                RowState::Added => "Added".into(),
                RowState::BuiltIn => "Built in".into(),
                _ => listing.unavailable.clone().unwrap_or_default().into(),
            };
            div()
                .flex_none()
                .text_size(crate::typography::ui_rems(12.0))
                .text_color(theme.text_muted.opacity(0.75))
                .child(label)
                .into_any_element()
        };
        widgets::card_row(theme, false)
            .id(("acp-registry-row", ix))
            .flex_nowrap()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(12.0))
                    .px(px(4.0))
                    .child(provider_tile(theme, mark, None))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(widgets::row_title(theme, listing.name.clone()))
                            .when(!meta.is_empty(), |el| {
                                el.child(widgets::meta_line(theme, meta))
                            }),
                    ),
            )
            .child(trailing)
            .into_any_element()
    }

    /// The last row: any local command, expanding into its fields the way a
    /// provider row expands into its details.
    fn custom_row(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let Some(flow) = self.add_provider.as_ref() else {
            return div().into_any_element();
        };
        let open = flow.custom_open;
        let busy = flow.adding.as_deref() == Some("");
        let header = widgets::card_row(theme, false).child(
            div()
                .id("acp-custom-trigger")
                .group("acp-custom-trigger")
                .flex_1()
                .min_h(px(44.0))
                .px(px(4.0))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(12.0))
                .cursor_pointer()
                .role(gpui::Role::Button)
                .aria_label("Custom ACP command")
                .aria_expanded(open)
                .on_click(cx.listener(|page, _, _, cx| {
                    if let Some(flow) = page.add_provider.as_mut() {
                        flow.custom_open = !flow.custom_open;
                        flow.error = None;
                        cx.notify();
                    }
                }))
                .child(provider_tile(theme, crate::icons::BOT, None))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(widgets::row_title(theme, "Custom command"))
                        .child(widgets::meta_line(
                            theme,
                            vec![
                                div()
                                    .child("Any local agent that speaks ACP over stdio")
                                    .into_any_element(),
                            ],
                        )),
                )
                .child(
                    crate::icons::icon(if open {
                        crate::icons::ALT_ARROW_DOWN
                    } else {
                        crate::icons::ALT_ARROW_RIGHT
                    })
                    .size(px(14.0))
                    .flex_none()
                    .text_color(theme.text_muted)
                    .group_hover("acp-custom-trigger", |s| s.text_color(theme.text)),
                ),
        );
        let field = |label: &'static str, input: &Entity<ComposerInput>| {
            div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(
                    div()
                        .text_size(crate::typography::ui_rems(12.0))
                        .text_color(theme.text_muted)
                        .child(label),
                )
                .child(popover::dialog_field(input.clone().into_any_element()))
        };
        let details = open.then(|| {
            div()
                .mx(px(16.0))
                .pl(px(DETAILS_INSET))
                .pb(px(16.0))
                .flex()
                .flex_col()
                .gap(px(12.0))
                .child(field("Command", &flow.command))
                .child(field("Name", &flow.name))
                .child(field("Environment", &flow.env))
                .child(
                    div().flex().justify_end().child(
                        widgets::text_action(
                            theme,
                            widgets::ActionTone::Filled,
                            if busy { "Adding…" } else { "Add" },
                        )
                        .id("acp-custom-add")
                        .when(!busy, |el| {
                            el.on_click(cx.listener(|page, _, _, cx| page.submit_custom_agent(cx)))
                        }),
                    ),
                )
        });
        div()
            .flex()
            .flex_col()
            .child(header)
            .children(details)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing(id: &str, name: &str, description: &str) -> AcpRegistryListing {
        AcpRegistryListing {
            id: id.into(),
            name: name.into(),
            version: "1.0.0".into(),
            description: description.into(),
            icon: None,
            website: None,
            added: false,
            built_in: None,
            unavailable: None,
        }
    }

    #[test]
    fn search_ranks_name_matches_then_addable_agents_first() {
        let mut codex = listing("codex-acp", "Codex", "OpenAI's coding agent");
        codex.built_in = Some(zeron_proto::HarnessId::Codex);
        let mut goose = listing("goose", "goose", "An open source agent");
        goose.added = true;
        let mut kimi = listing("kimi", "Kimi CLI", "Moonshot's agent");
        kimi.unavailable = Some("No build for this device".into());
        let all = vec![
            codex,
            goose,
            kimi,
            listing("gemini", "Gemini CLI", "Google's official CLI"),
            listing("qwen-code", "Qwen Code", "Agent for Qwen models"),
        ];
        let ids = |query: &str| -> Vec<String> {
            filtered(&all, query).iter().map(|l| l.id.clone()).collect()
        };
        assert_eq!(
            ids(""),
            ["gemini", "qwen-code", "goose", "codex-acp", "kimi"]
        );
        // Name/id prefix, then substring, then description.
        assert_eq!(
            ids("g"),
            ["gemini", "goose", "qwen-code", "codex-acp", "kimi"]
        );
        assert_eq!(ids("cli"), ["gemini", "kimi"]);
        assert_eq!(ids("moonshot"), ["kimi"]);
        assert!(ids("nothing-matches").is_empty());
    }
}
