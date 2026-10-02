//! The workflow approval block: what the question panel shows when the
//! agent asks to run a workflow.
//!
//! The engine raises an ordinary input question whose text already says
//! everything (so clients that know nothing about workflows approve fine) and
//! whose `meta` carries the analysed graph. This module turns that payload
//! into a model and draws it in place of the plain question text. The
//! answers stay the stock options — `Run workflow` / `Deny` — so the number
//! keys, Enter and every other client keep working unchanged.

use gpui::{
    AnyElement, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, prelude::*, px,
};
use zeron_proto::{GraphActor, WORKFLOW_APPROVAL_META_KIND, WorkflowApprovalMeta, WorkflowBudgets};
use zeron_syntax::HighlightedDocument;

use super::model::format_tokens;
use super::widgets::lamp;
use crate::icons::{self, icon};
use crate::markdown::parser::Block;
use crate::markdown::render::{self, RenderOptions};
use crate::theme::Theme;
use crate::typography::ui_rems;

/// Literal commands listed before "+n more".
pub const COMMANDS_SHOWN: usize = 5;
/// The excerpt is drawn this tall before it scrolls.
pub const EXCERPT_MAX_HEIGHT: f32 = 220.0;
/// Lines of the excerpt that fit in [`EXCERPT_MAX_HEIGHT`] (the block's header
/// and padding take about 44px, a line 18px); more and the bottom is cut.
const EXCERPT_LINES_FIT: usize = 9;

/// The approval payload of a question, if it is one.
pub fn parse_meta(meta: Option<&serde_json::Value>) -> Option<WorkflowApprovalMeta> {
    let meta = meta?;
    if meta.get("kind").and_then(|k| k.as_str()) != Some(WORKFLOW_APPROVAL_META_KIND) {
        return None;
    }
    serde_json::from_value(meta.clone()).ok()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalPhase {
    pub name: String,
    pub asks: u32,
    pub commands: u32,
    /// Some of its calls sit in a loop or a `pmap`: how many is not known
    /// until it runs.
    pub fan_out: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalAgent {
    pub name: String,
    /// `claude · sonnet`, when the script picks one.
    pub model: Option<String>,
    pub many: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalModel {
    pub name: String,
    pub phases: Vec<ApprovalPhase>,
    pub agents: Vec<ApprovalAgent>,
    /// Literal commands, rendered `program arg arg`.
    pub commands: Vec<String>,
    pub caps: Vec<String>,
    pub default_model: Option<String>,
    pub draft_path: Option<String>,
    pub excerpt: String,
}

fn agent_of(actor: &GraphActor) -> ApprovalAgent {
    ApprovalAgent {
        name: actor
            .name
            .clone()
            .unwrap_or_else(|| "agent (named at run time)".into()),
        model: match (&actor.harness, &actor.model) {
            (Some(h), Some(m)) => Some(format!("{h} · {m}")),
            (Some(x), None) | (None, Some(x)) => Some(x.clone()),
            (None, None) => None,
        },
        many: actor.site.fan_out,
    }
}

fn seconds(s: u64) -> String {
    match s {
        0..=89 => format!("{s}s"),
        90..=5399 => format!("{} min", s.div_ceil(60)),
        _ => format!("{:.1} h", s as f64 / 3600.0).replace(".0 h", " h"),
    }
}

/// The caps in words: concurrency first, then whichever budgets are set.
pub fn caps(max_concurrency: u32, budgets: &WorkflowBudgets) -> Vec<String> {
    let mut out = Vec::new();
    if max_concurrency > 0 {
        out.push(format!(
            "{max_concurrency} {} at once",
            if max_concurrency == 1 {
                "agent"
            } else {
                "agents"
            }
        ));
    }
    if let Some(n) = budgets.max_asks {
        out.push(format!("at most {n} asks"));
    }
    if let Some(n) = budgets.max_tokens {
        out.push(format!("{} tokens", format_tokens(n)));
    }
    if let Some(s) = budgets.max_runtime_seconds {
        out.push(format!("{} of run time", seconds(s)));
    }
    out
}

impl ApprovalModel {
    pub fn from_meta(meta: &WorkflowApprovalMeta) -> Self {
        let graph = &meta.graph;
        let phases = graph
            .phases
            .iter()
            .map(|p| ApprovalPhase {
                name: p.name.clone(),
                asks: p.asks.len() as u32,
                commands: p.commands.len() as u32,
                fan_out: p.asks.iter().any(|a| a.site.fan_out)
                    || p.commands.iter().any(|c| c.site.fan_out),
            })
            .collect();
        let commands = graph
            .commands
            .iter()
            .map(|c| match &c.args {
                Some(args) if args.is_empty() => c.command.clone(),
                Some(args) => format!("{} {}", c.command, args.join(" ")),
                None => format!("{} …", c.command),
            })
            .collect();
        ApprovalModel {
            name: meta.name.clone(),
            phases,
            agents: graph.actors.iter().map(agent_of).collect(),
            commands,
            caps: caps(meta.max_concurrency, &meta.budgets),
            default_model: match (&meta.harness, &meta.model) {
                (Some(h), Some(m)) => Some(format!("{h} · {m}")),
                (Some(x), None) | (None, Some(x)) => Some(x.clone()),
                (None, None) => None,
            },
            draft_path: meta.draft_path.clone(),
            // A script that opens with blank lines would draw an empty band.
            excerpt: meta.excerpt.trim_start_matches(['\r', '\n']).to_owned(),
        }
    }

    /// `4 phases · 14 agents · 3 commands`
    pub fn summary(&self) -> String {
        let n = |count: usize, one: &str, many: &str| {
            format!("{count} {}", if count == 1 { one } else { many })
        };
        let mut parts = vec![n(self.phases.len(), "phase", "phases")];
        if !self.agents.is_empty() {
            parts.push(n(self.agents.len(), "agent", "agents"));
        }
        if !self.commands.is_empty() {
            parts.push(n(self.commands.len(), "command", "commands"));
        }
        parts.join(" · ")
    }
}

/// Highlight the excerpt as Python (Starlark is a Python dialect). Cheap:
/// the excerpt is a few dozen lines.
pub fn highlight_excerpt(excerpt: &str) -> Option<HighlightedDocument> {
    zeron_syntax::highlight(zeron_syntax::HighlightRequest {
        source: excerpt,
        path: None,
        fence_tag: Some("python"),
    })
    .ok()
}

fn label(text: &'static str, theme: &Theme) -> gpui::Div {
    div()
        .flex_none()
        .w(px(72.0))
        .pt(px(1.0))
        .text_size(ui_rems(11.0))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(theme.text_faint)
        .child(text)
}

/// The block. `script_open` shows the (highlighted) excerpt; `on_toggle`
/// flips it.
pub fn approval_block(
    model: &ApprovalModel,
    highlight: Option<&HighlightedDocument>,
    script_open: bool,
    on_toggle_script: impl Fn(&mut Window, &mut gpui::App) + 'static,
    theme: &Theme,
    window: &Window,
) -> AnyElement {
    let phases = div().flex().flex_row().flex_wrap().gap(px(6.0)).children(
        model.phases.iter().enumerate().map(|(ix, p)| {
            let mut work = Vec::new();
            if p.asks > 0 {
                work.push(format!("{}{}", p.asks, if p.fan_out { "+" } else { "" }));
            }
            if p.commands > 0 {
                work.push(format!("{} cmd", p.commands));
            }
            div()
                .id(SharedString::from(format!("workflow-approval-phase-{ix}")))
                .h(px(24.0))
                .px(px(8.0))
                .rounded(px(7.0))
                .border_1()
                .border_color(crate::theme::hairline(0.1))
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(
                    div()
                        .text_size(ui_rems(10.5))
                        .text_color(theme.text_faint)
                        .child(SharedString::from(format!("{}", ix + 1))),
                )
                .child(
                    div()
                        .text_size(ui_rems(12.0))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(theme.text)
                        .child(SharedString::from(p.name.clone())),
                )
                .when(!work.is_empty(), |el| {
                    el.child(
                        div()
                            .text_size(ui_rems(11.0))
                            .text_color(theme.text_faint)
                            .child(SharedString::from(work.join(" · "))),
                    )
                })
                .when(p.fan_out, |el| {
                    el.tooltip(crate::settings::widgets::text_tooltip(
                        "Runs once per item, so the number of agents depends on the data",
                    ))
                })
        }),
    );

    let agents =
        div()
            .flex()
            .flex_row()
            .flex_wrap()
            .gap(px(6.0))
            .children(model.agents.iter().map(|a| {
                div()
                    .h(px(22.0))
                    .px(px(8.0))
                    .rounded(px(11.0))
                    .bg(crate::theme::ink(0.06))
                    .flex()
                    .items_center()
                    .gap(px(5.0))
                    .child(icon(icons::BOT).size(px(11.0)).text_color(theme.text_muted))
                    .child(div().text_size(ui_rems(11.5)).text_color(theme.text).child(
                        SharedString::from(if a.many {
                            format!("{} ×n", a.name)
                        } else {
                            a.name.clone()
                        }),
                    ))
                    .when_some(a.model.clone(), |el, m| {
                        el.child(
                            div()
                                .text_size(ui_rems(11.0))
                                .text_color(theme.text_faint)
                                .child(SharedString::from(m)),
                        )
                    })
            }));

    let commands = div().flex().flex_col().gap(px(2.0)).children(
        model
            .commands
            .iter()
            .take(COMMANDS_SHOWN)
            .map(|c| {
                div()
                    .truncate()
                    .font_family(theme.font_mono.clone())
                    .text_size(px(theme.code_font_size - 1.0))
                    .text_color(theme.text)
                    .child(SharedString::from(format!("$ {c}")))
            })
            .chain((model.commands.len() > COMMANDS_SHOWN).then(|| {
                div()
                    .text_size(ui_rems(11.0))
                    .text_color(theme.text_faint)
                    .child(SharedString::from(format!(
                        "+{} more",
                        model.commands.len() - COMMANDS_SHOWN
                    )))
            })),
    );

    let accent = theme.accent;
    let toggle = div()
        .id("workflow-approval-script")
        .role(gpui::Role::Button)
        .aria_label(if script_open {
            "Hide the script"
        } else {
            "Show the script"
        })
        .h(px(24.0))
        .px(px(6.0))
        .rounded(px(6.0))
        .flex()
        .items_center()
        .gap(px(5.0))
        .text_size(ui_rems(11.5))
        .text_color(theme.text_muted)
        .cursor_pointer()
        .tab_index(0)
        .hover(|s| s.bg(crate::theme::ink(0.06)))
        .focus_visible(move |s| s.bg(accent.opacity(0.18)))
        .on_click(move |_, window, cx| on_toggle_script(window, cx))
        .child(
            icon(if script_open {
                icons::ALT_ARROW_DOWN
            } else {
                icons::ALT_ARROW_RIGHT
            })
            .size(px(11.0))
            .text_color(theme.text_muted),
        )
        .child(if script_open {
            "Hide script"
        } else {
            "Show script"
        });

    let script = script_open.then(|| {
        let opts = RenderOptions {
            tasks: None,
            media: None,
            row_key: "workflow-approval-script".into(),
            veil: None,
            cache: None,
            now: std::time::Instant::now(),
            copy: None,
            link: None,
            workspace_root: None,
            code: None,
        };
        let block = Block::CodeBlock {
            language: Some("python".into()),
            code: model.excerpt.clone(),
        };
        // A clipped block loses its own bottom edge: draw one, so the cut
        // reads as a scroll area and not as missing content.
        let cut = model.excerpt.lines().count() > EXCERPT_LINES_FIT;
        div()
            .id("workflow-approval-excerpt")
            .max_h(px(EXCERPT_MAX_HEIGHT))
            .overflow_y_scroll()
            .when(cut, |el| {
                el.rounded_b(px(8.0))
                    .border_b_1()
                    .border_color(crate::theme::hairline(0.1))
            })
            .child(render::render_block(
                &block,
                0,
                0,
                &opts,
                theme,
                window,
                highlight.map(|h| h.lines.as_slice()),
            ))
    });

    div()
        .mt(px(8.0))
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(lamp(super::model::Light::Pending, 7.0, theme))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(ui_rems(15.0))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(theme.text)
                        .child(SharedString::from(model.name.clone())),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(ui_rems(11.5))
                        .text_color(theme.text_faint)
                        .child(SharedString::from(model.summary())),
                ),
        )
        .child(
            div()
                .flex()
                .gap(px(8.0))
                .child(label("Phases", theme))
                .child(div().flex_1().min_w_0().child(phases)),
        )
        .when(!model.agents.is_empty(), |el| {
            el.child(
                div()
                    .flex()
                    .gap(px(8.0))
                    .child(label("Agents", theme))
                    .child(div().flex_1().min_w_0().child(agents)),
            )
        })
        .when(!model.commands.is_empty(), |el| {
            el.child(
                div()
                    .flex()
                    .gap(px(8.0))
                    .child(label("Runs", theme))
                    .child(div().flex_1().min_w_0().child(commands)),
            )
        })
        .child(
            div()
                .flex()
                .gap(px(8.0))
                .child(label("Limits", theme))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(ui_rems(12.0))
                        .line_height(px(17.0))
                        .text_color(theme.text_muted)
                        .child(SharedString::from(
                            [
                                model.caps.join(" · "),
                                model
                                    .default_model
                                    .as_ref()
                                    .map(|m| format!("default agent {m}"))
                                    .unwrap_or_default(),
                            ]
                            .into_iter()
                            .filter(|s| !s.is_empty())
                            .collect::<Vec<_>>()
                            .join(" · "),
                        )),
                ),
        )
        .child(div().child(toggle))
        .children(script)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_proto::{GraphAsk, GraphCommand, GraphPhase, GraphSite, WorkflowGraph};

    fn site(fan_out: bool) -> GraphSite {
        GraphSite {
            site_id: "1:1-1:2".into(),
            line: 1,
            col: 1,
            fan_out,
        }
    }

    fn meta() -> WorkflowApprovalMeta {
        WorkflowApprovalMeta {
            run_id: "r1".into(),
            name: "PR review".into(),
            script_hash: "abcdef".into(),
            draft_path: Some(".zeron/workflow-drafts/pr-review.star".into()),
            graph: WorkflowGraph {
                phases: vec![
                    GraphPhase {
                        name: "review".into(),
                        asks: vec![
                            GraphAsk {
                                site: site(true),
                                ..Default::default()
                            },
                            GraphAsk {
                                site: site(false),
                                ..Default::default()
                            },
                        ],
                        ..Default::default()
                    },
                    GraphPhase {
                        name: "gate".into(),
                        commands: vec![GraphCommand {
                            site: site(false),
                            command: "cargo".into(),
                            args: Some(vec!["test".into(), "--all".into()]),
                        }],
                        ..Default::default()
                    },
                ],
                actors: vec![
                    GraphActor {
                        name: Some("security reviewer".into()),
                        harness: Some("claude".into()),
                        model: Some("sonnet".into()),
                        site: site(true),
                    },
                    GraphActor {
                        name: None,
                        ..Default::default()
                    },
                ],
                commands: vec![
                    GraphCommand {
                        site: site(false),
                        command: "cargo".into(),
                        args: Some(vec!["test".into(), "--all".into()]),
                    },
                    GraphCommand {
                        site: site(false),
                        command: "git".into(),
                        args: Some(vec![]),
                    },
                    GraphCommand {
                        site: site(false),
                        command: "sh".into(),
                        args: None,
                    },
                ],
                unphased_asks: 0,
            },
            max_concurrency: 6,
            budgets: WorkflowBudgets {
                max_asks: Some(500),
                max_tokens: Some(2_000_000),
                max_runtime_seconds: Some(1800),
            },
            harness: Some("claude".into()),
            model: None,
            excerpt: "def main(args):\n    phase(\"review\")\n".into(),
        }
    }

    #[test]
    fn only_a_workflow_approval_payload_is_recognised() {
        let value = serde_json::to_value(meta()).unwrap();
        assert!(parse_meta(Some(&value)).is_none(), "no kind, no approval");
        let mut tagged = value;
        tagged["kind"] = "workflowApproval".into();
        let parsed = parse_meta(Some(&tagged)).unwrap();
        assert_eq!(parsed.name, "PR review");
        tagged["kind"] = "somethingElse".into();
        assert!(parse_meta(Some(&tagged)).is_none());
        assert!(parse_meta(None).is_none());
        assert!(parse_meta(Some(&serde_json::json!({"kind": "workflowApproval"}))).is_none());
    }

    #[test]
    fn the_model_lists_phases_agents_commands_and_caps() {
        let m = ApprovalModel::from_meta(&meta());
        assert_eq!(m.phases.len(), 2);
        assert_eq!(
            (m.phases[0].asks, m.phases[0].fan_out),
            (2, true),
            "a loop makes the count open-ended"
        );
        assert_eq!((m.phases[1].commands, m.phases[1].fan_out), (1, false));
        assert_eq!(m.agents[0].name, "security reviewer");
        assert_eq!(m.agents[0].model.as_deref(), Some("claude · sonnet"));
        assert!(m.agents[0].many);
        assert_eq!(m.agents[1].name, "agent (named at run time)");
        assert_eq!(m.commands, ["cargo test --all", "git", "sh …"]);
        assert_eq!(
            m.caps,
            [
                "6 agents at once",
                "at most 500 asks",
                "2M tokens",
                "30 min of run time"
            ]
        );
        assert_eq!(m.default_model.as_deref(), Some("claude"));
        assert_eq!(m.summary(), "2 phases · 2 agents · 3 commands");
    }

    #[test]
    fn caps_read_naturally() {
        assert_eq!(caps(1, &WorkflowBudgets::default()), ["1 agent at once"]);
        assert!(caps(0, &WorkflowBudgets::default()).is_empty());
        let b = WorkflowBudgets {
            max_runtime_seconds: Some(45),
            ..Default::default()
        };
        assert_eq!(caps(4, &b), ["4 agents at once", "45s of run time"]);
        let b = WorkflowBudgets {
            max_runtime_seconds: Some(7200),
            ..Default::default()
        };
        assert_eq!(caps(4, &b)[1], "2 h of run time");
    }

    #[test]
    fn leading_blank_lines_of_the_excerpt_are_dropped() {
        let mut m = meta();
        m.excerpt = "\n\n  def main(args):\n    pass\n".into();
        assert_eq!(
            ApprovalModel::from_meta(&m).excerpt,
            "  def main(args):\n    pass\n"
        );
    }

    #[test]
    fn the_excerpt_highlights_as_python() {
        let doc = highlight_excerpt("def main(args):\n    phase(\"review\")\n").unwrap();
        assert_eq!(doc.language, zeron_syntax::LanguageId::Python);
        assert!(doc.lines.iter().any(|l| !l.is_empty()), "tokens were found");
    }
}
