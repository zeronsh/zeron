//! Settings → General → Agent presets: the user's named agents (harness,
//! model, mode, instructions), plus the project's and imported ones read-only
//! (docs/agent-presets.md). Presets are stored by the device's engine
//! (`ListPresets` / `UpsertPreset` / `DeletePreset`).

use gpui::{AnyElement, Context, Entity, Subscription, Task, div, prelude::*, px};
use zeron_engine::registry::HarnessDescriptor;
use zeron_proto::{AgentPreset, HarnessId, PermissionMode, PresetSource, ReasoningLevel};
use zeron_rpc::methods;

use crate::composer::{ComposerInput, ComposerInputEvent};
use crate::popover::Loadable;
use crate::settings::widgets;
use crate::state::AppState;
use crate::theme::Theme;

/// The reasoning levels a preset can pin, in menu order (`None` = the
/// model's default).
const REASONING: [Option<ReasoningLevel>; 6] = [
    None,
    Some(ReasoningLevel::Low),
    Some(ReasoningLevel::Medium),
    Some(ReasoningLevel::High),
    Some(ReasoningLevel::XHigh),
    Some(ReasoningLevel::Max),
];

fn reasoning_label(level: Option<ReasoningLevel>) -> &'static str {
    match level {
        None => "Default",
        Some(ReasoningLevel::Minimal) => "Minimal",
        Some(ReasoningLevel::Low) => "Low",
        Some(ReasoningLevel::Medium) => "Medium",
        Some(ReasoningLevel::High) => "High",
        Some(ReasoningLevel::XHigh) => "Extra high",
        Some(ReasoningLevel::Max) => "Max",
        Some(ReasoningLevel::Ultra) => "Ultra",
        Some(ReasoningLevel::Ultracode) => "Ultracode",
        Some(ReasoningLevel::Ultrathink) => "Ultrathink",
    }
}

/// A preset being created or edited. Fields the form has no control for
/// (fallbacks, tool limits, rules…) ride along in `keep`.
struct Editor {
    /// `None` for a new preset.
    original_id: Option<String>,
    keep: AgentPreset,
    name: Entity<ComposerInput>,
    description: Entity<ComposerInput>,
    model: Entity<ComposerInput>,
    instructions: Entity<ComposerInput>,
    harness: usize,
    reasoning: usize,
    mode: usize,
    may_spawn: bool,
    worktree: bool,
    harness_select: widgets::SelectState,
    reasoning_select: widgets::SelectState,
    mode_select: widgets::SelectState,
}

pub struct PresetsCard {
    state: Entity<AppState>,
    presets: Loadable<Vec<AgentPreset>>,
    /// The harnesses this device lists (installed ones are offered), with
    /// the names people know them by.
    harnesses: Vec<(HarnessId, String)>,
    editor: Option<Editor>,
    error: Option<String>,
    task: Option<Task<()>>,
    _state: Subscription,
}

impl PresetsCard {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let sub = cx.observe(&state, |this: &mut Self, _, cx| {
            if matches!(this.presets, Loadable::Idle) {
                this.load(cx);
            }
        });
        let mut card = Self {
            state,
            presets: Loadable::Idle,
            harnesses: Vec::new(),
            editor: None,
            error: None,
            task: None,
            _state: sub,
        };
        card.load(cx);
        card
    }

    fn engine(&self, cx: &Context<Self>) -> Option<crate::state::EngineHandle> {
        self.state.read(cx).engine().cloned()
    }

    /// The device's presets (outside any project: the user's own) and the
    /// harnesses the form can pick.
    fn load(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.engine(cx) else {
            return;
        };
        if matches!(self.presets, Loadable::Idle) {
            self.presets = Loadable::Loading;
        }
        self.task = Some(cx.spawn(async move |this, cx| {
            let presets = engine
                .client()
                .call(methods::LIST_PRESETS, serde_json::json!({}))
                .await
                .map_err(|e| e.to_string())
                .and_then(|v| {
                    serde_json::from_value::<Vec<AgentPreset>>(
                        v.get("presets").cloned().unwrap_or_default(),
                    )
                    .map_err(|e| e.to_string())
                });
            let harnesses = engine
                .client()
                .call(methods::LIST_HARNESSES, serde_json::json!({}))
                .await
                .ok()
                .and_then(|v| serde_json::from_value::<Vec<HarnessDescriptor>>(v).ok())
                .unwrap_or_default();
            this.update(cx, |card, cx| {
                card.harnesses = harnesses
                    .iter()
                    .filter(|d| d.installed)
                    .map(|d| (d.id, d.name.clone()))
                    .collect();
                match presets {
                    Ok(list) => {
                        card.presets = Loadable::Ready(list);
                        card.error = None;
                    }
                    Err(error) if matches!(card.presets, Loadable::Ready(_)) => {
                        card.error = Some(error);
                    }
                    Err(error) => card.presets = Loadable::Error(error),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn open_editor(&mut self, preset: Option<AgentPreset>, cx: &mut Context<Self>) {
        let original_id = preset.as_ref().map(|p| p.id.clone());
        let keep = preset
            .clone()
            .unwrap_or_else(|| {
                AgentPreset::new(
                    "",
                    // Claude Code when this device lists it, else its first.
                    if self.harnesses.iter().any(|(id, _)| *id == HarnessId::ClaudeCode) {
                        HarnessId::ClaudeCode
                    } else {
                        self.harnesses
                            .first()
                            .map_or(HarnessId::ClaudeCode, |(id, _)| *id)
                    },
                )
            });
        let input = |placeholder: &'static str, text: &str, cx: &mut Context<Self>| {
            let input = cx.new(|cx| ComposerInput::new(placeholder, cx));
            input.update(cx, |input, cx| input.set_text(text.to_string(), cx));
            input
        };
        let harnesses = self.harness_options(keep.harness);
        let editor = Editor {
            original_id,
            name: input("Reviewer", &keep.name, cx),
            description: input("When an orchestrating agent should use this", &keep.description, cx),
            model: input("Model id (blank = the harness default)", keep.model.as_deref().unwrap_or(""), cx),
            instructions: input(
                "Instructions added to the agent's system prompt",
                keep.instructions.as_deref().unwrap_or(""),
                cx,
            ),
            harness: harnesses.iter().position(|h| *h == keep.harness).unwrap_or(0),
            reasoning: REASONING.iter().position(|r| *r == keep.reasoning).unwrap_or(0),
            mode: PermissionMode::ALL
                .iter()
                .position(|m| *m == keep.policy.mode)
                .unwrap_or(0),
            may_spawn: keep.may_spawn,
            worktree: keep.worktree,
            harness_select: Default::default(),
            reasoning_select: Default::default(),
            mode_select: Default::default(),
            keep,
        };
        self.editor = Some(editor);
        cx.notify();
    }

    /// The harness menu: the installed ones, plus the preset's own if it
    /// isn't among them (so an edit never silently moves it).
    fn harness_options(&self, current: HarnessId) -> Vec<HarnessId> {
        let mut list: Vec<HarnessId> = self.harnesses.iter().map(|(id, _)| *id).collect();
        if !list.contains(&current) {
            list.insert(0, current);
        }
        list
    }

    /// A harness's name as the device lists it.
    fn harness_name(&self, id: HarnessId) -> String {
        self.harnesses
            .iter()
            .find(|(candidate, _)| *candidate == id)
            .map_or_else(|| format!("{id:?}"), |(_, name)| name.clone())
    }

    /// Screenshot fixtures: open the editor on a new preset.
    pub fn fixture_open_editor(&mut self, cx: &mut Context<Self>) {
        self.open_editor(None, cx);
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = &self.editor else {
            return;
        };
        let harnesses = self.harness_options(editor.keep.harness);
        let mut preset = editor.keep.clone();
        preset.name = editor.name.read(cx).text().trim().to_string();
        if preset.name.is_empty() {
            self.error = Some("A preset needs a name.".into());
            cx.notify();
            return;
        }
        if editor.original_id.is_none() {
            preset.id = zeron_proto::preset::slug(&preset.name);
        }
        preset.description = editor.description.read(cx).text().trim().to_string();
        preset.harness = harnesses.get(editor.harness).copied().unwrap_or(preset.harness);
        preset.model = Some(editor.model.read(cx).text().trim().to_string()).filter(|m| !m.is_empty());
        preset.reasoning = REASONING.get(editor.reasoning).copied().flatten();
        preset.policy.mode = PermissionMode::ALL
            .get(editor.mode)
            .copied()
            .unwrap_or_default();
        preset.instructions =
            Some(editor.instructions.read(cx).text().trim().to_string()).filter(|i| !i.is_empty());
        preset.may_spawn = editor.may_spawn;
        preset.worktree = editor.worktree;
        self.editor = None;
        self.call(methods::UPSERT_PRESET, serde_json::json!({ "preset": preset }), cx);
    }

    fn delete(&mut self, id: String, cx: &mut Context<Self>) {
        self.call(methods::DELETE_PRESET, serde_json::json!({ "id": id }), cx);
    }

    fn duplicate(&mut self, preset: &AgentPreset, cx: &mut Context<Self>) {
        let mut copy = preset.clone();
        copy.name = format!("{} copy", preset.name);
        copy.id = zeron_proto::preset::slug(&copy.name);
        copy.source = PresetSource::User;
        self.call(methods::UPSERT_PRESET, serde_json::json!({ "preset": copy }), cx);
    }

    /// A write, then a fresh list (the engine's file is the source of truth).
    fn call(&mut self, method: &'static str, params: serde_json::Value, cx: &mut Context<Self>) {
        let Some(engine) = self.engine(cx) else {
            return;
        };
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(method, params)
                .await
                .map_err(|e| e.to_string());
            this.update(cx, |card, cx| {
                match result {
                    Ok(_) => card.error = None,
                    Err(error) => card.error = Some(error),
                }
                card.load(cx);
            })
            .ok();
        }));
    }

    fn input_row(theme: &Theme, label: &'static str, input: Entity<ComposerInput>) -> gpui::Div {
        div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(widgets::field_label(theme, label))
            .child(crate::popover::dialog_field(input.into_any_element()))
    }

    fn render_editor(&mut self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let harnesses = self.editor.as_ref().map(|e| self.harness_options(e.keep.harness))?;
        let editor = self.editor.as_ref()?;
        let (harness_ix, reasoning_ix, mode_ix) = (editor.harness, editor.reasoning, editor.mode);
        let (may_spawn, worktree) = (editor.may_spawn, editor.worktree);
        let editing = editor.original_id.is_some();
        let (name, description, model, instructions) = (
            editor.name.clone(),
            editor.description.clone(),
            editor.model.clone(),
            editor.instructions.clone(),
        );
        let selects = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .gap(px(8.0))
            .child(
                widgets::select(
                    "preset-harness",
                    "Harness",
                    theme,
                    |card: &mut Self| {
                        &mut card.editor.as_mut().expect("editor open").harness_select
                    },
                )
                .options(
                    harnesses
                        .iter()
                        .map(|h| widgets::SelectOption::new(self.harness_name(*h))),
                    harness_ix,
                )
                .width(150.0)
                .on_select(|card, ix, _, cx| {
                    if let Some(editor) = card.editor.as_mut() {
                        editor.harness = ix;
                    }
                    cx.notify();
                })
                .render(&self.editor.as_ref()?.harness_select, cx),
            )
            .child(
                widgets::select(
                    "preset-reasoning",
                    "Reasoning",
                    theme,
                    |card: &mut Self| {
                        &mut card.editor.as_mut().expect("editor open").reasoning_select
                    },
                )
                .options(
                    REASONING
                        .iter()
                        .map(|r| widgets::SelectOption::new(reasoning_label(*r))),
                    reasoning_ix,
                )
                .width(120.0)
                .on_select(|card, ix, _, cx| {
                    if let Some(editor) = card.editor.as_mut() {
                        editor.reasoning = ix;
                    }
                    cx.notify();
                })
                .render(&self.editor.as_ref()?.reasoning_select, cx),
            )
            .child(
                widgets::select(
                    "preset-mode",
                    "Permission mode",
                    theme,
                    |card: &mut Self| &mut card.editor.as_mut().expect("editor open").mode_select,
                )
                .options(
                    PermissionMode::ALL
                        .iter()
                        .map(|m| widgets::SelectOption::new(m.label())),
                    mode_ix,
                )
                .width(184.0)
                .on_select(|card, ix, _, cx| {
                    if let Some(editor) = card.editor.as_mut() {
                        editor.mode = ix;
                    }
                    cx.notify();
                })
                .render(&self.editor.as_ref()?.mode_select, cx),
            );
        let switch = |id: &'static str, label: &'static str, on: bool, theme: &Theme, cx: &mut Context<Self>| {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(10.0))
                .child(
                    widgets::toggle_switch(theme, on, id)
                        .id(id)
                        .cursor_pointer()
                        .on_click(cx.listener(move |card, _, _, cx| {
                            if let Some(editor) = card.editor.as_mut() {
                                match id {
                                    "preset-spawn" => editor.may_spawn = !editor.may_spawn,
                                    _ => editor.worktree = !editor.worktree,
                                }
                            }
                            cx.notify();
                        })),
                )
                .child(div().text_color(theme.text_muted).child(label))
        };
        Some(
            div()
                .p(px(16.0))
                .flex()
                .flex_col()
                .gap(px(12.0))
                .child(widgets::row_title(
                    theme,
                    if editing { "Edit preset" } else { "New preset" },
                ))
                .child(Self::input_row(theme, "Name", name))
                .child(Self::input_row(theme, "When to use it", description))
                .child(selects)
                .child(Self::input_row(theme, "Model", model))
                .child(Self::input_row(theme, "Instructions", instructions))
                .child(switch("preset-spawn", "May create chats itself", may_spawn, theme, cx))
                .child(switch("preset-worktree", "Start in a new worktree", worktree, theme, cx))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_end()
                        .gap(px(8.0))
                        .child(
                            widgets::text_action(theme, widgets::ActionTone::Quiet, "Cancel")
                                .id("preset-cancel")
                                .on_click(cx.listener(|card, _, _, cx| {
                                    card.editor = None;
                                    cx.notify();
                                })),
                        )
                        .child(
                            widgets::text_action(theme, widgets::ActionTone::Solid, "Save")
                                .id("preset-save")
                                .on_click(cx.listener(|card, _, _, cx| card.save(cx))),
                        ),
                )
                .into_any_element(),
        )
    }
}

impl Render for PresetsCard {
    fn render(&mut self, _: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).for_settings_surface();
        let header = widgets::card_row(&theme, true)
            .child(
                div()
                    .flex_1()
                    .min_w(px(160.0))
                    .child(widgets::row_title(&theme, "Agent presets"))
                    .child(widgets::meta_line(
                        &theme,
                        vec![
                            div()
                                .child(
                                    "Named agents to start a chat as: a harness, model, permission mode \
                                     and standing instructions. A project's own live in .zeron/agents.",
                                )
                                .into_any_element(),
                        ],
                    )),
            )
            .child(
                widgets::text_action(&theme, widgets::ActionTone::Outlined, "New preset")
                    .id("preset-new")
                    .on_click(cx.listener(|card, _, _, cx| card.open_editor(None, cx))),
            );
        let rows: Vec<AnyElement> = match &self.presets {
            Loadable::Ready(list) if list.is_empty() => vec![
                widgets::card_row(&theme, false)
                    .child(
                        div()
                            .text_color(theme.text_muted)
                            .child("No presets yet. Create one, or add .zeron/agents/<name>.md to a project."),
                    )
                    .into_any_element(),
            ],
            Loadable::Ready(list) => list
                .iter()
                .cloned()
                .enumerate()
                .map(|(ix, preset)| {
                    let (edit, copy, id) = (preset.clone(), preset.clone(), preset.id.clone());
                    let editable = preset.source == PresetSource::User;
                    let mut row = widgets::card_row(&theme, false)
                        .gap(px(10.0))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(widgets::row_title(&theme, preset.name.clone()))
                                .child(widgets::meta_line(
                                    &theme,
                                    vec![
                                        div()
                                            .child(format!(
                                                "{} · {}{}",
                                                self.harness_name(preset.harness),
                                                preset.policy.mode.label(),
                                                if preset.description.is_empty() {
                                                    String::new()
                                                } else {
                                                    format!(" · {}", preset.description)
                                                }
                                            ))
                                            .into_any_element(),
                                    ],
                                )),
                        );
                    if let Some(label) = crate::agent_preset::source_label(preset.source) {
                        row = row.child(widgets::badge(&theme, label));
                    }
                    if editable {
                        row = row
                            .child(
                                widgets::text_action(&theme, widgets::ActionTone::Quiet, "Edit")
                                    .id(("preset-edit", ix))
                                    .on_click(cx.listener(move |card, _, _, cx| {
                                        card.open_editor(Some(edit.clone()), cx)
                                    })),
                            )
                            .child(
                                widgets::text_action(&theme, widgets::ActionTone::Quiet, "Duplicate")
                                    .id(("preset-copy", ix))
                                    .on_click(cx.listener(move |card, _, _, cx| {
                                        card.duplicate(&copy, cx)
                                    })),
                            )
                            .child(
                                widgets::text_action(&theme, widgets::ActionTone::Quiet, "Delete")
                                    .id(("preset-delete", ix))
                                    .on_click(cx.listener(move |card, _, _, cx| {
                                        card.delete(id.clone(), cx)
                                    })),
                            );
                    }
                    row.into_any_element()
                })
                .collect(),
            Loadable::Error(error) => vec![
                widgets::card_row(&theme, false)
                    .child(div().text_color(theme.text_muted).child(error.clone()))
                    .into_any_element(),
            ],
            Loadable::Idle | Loadable::Loading => vec![
                widgets::card_row(&theme, false)
                    .child(div().text_color(theme.text_muted).child("Loading…"))
                    .into_any_element(),
            ],
        };
        let editor = self.render_editor(&theme, cx);
        widgets::section_card(&theme)
            .child(header)
            .children(rows)
            .children(editor)
            .when_some(self.error.clone(), |card, error| {
                card.child(widgets::card_row(&theme, false).child(widgets::error_strip(&theme, error)))
            })
    }
}

// Submitting in a text box saves, like the other settings forms.
impl PresetsCard {
    #[allow(dead_code)]
    fn on_input_event(&mut self, event: &ComposerInputEvent, cx: &mut Context<Self>) {
        if matches!(event, ComposerInputEvent::Submitted) {
            self.save(cx);
        }
    }
}
