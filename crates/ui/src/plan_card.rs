//! A plan in the question panel (docs/plan-mode.md).
//!
//! The agent's plan is markdown. The panel shows it as a scrollable card with
//! headings, bullets and code blocks laid out; inline emphasis and links stay
//! as typed. (The transcript's full renderer is built around its own row
//! cache, which a one-off card has no use for.)

use gpui::{
    AnyElement, FontWeight, IntoElement, ParentElement, SharedString, Styled, div, prelude::*, px,
};

use crate::theme::Theme;

/// One laid-out line of a plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanLine {
    Heading { level: u8, text: String },
    /// `marker` is the bullet or the number as written ("-" shows as "•").
    Item { indent: usize, marker: String, text: String },
    Code(String),
    Text(String),
    Blank,
}

/// Split plan markdown into lines to lay out. Anything it doesn't recognise
/// stays plain text, so no plan is ever hidden by the parser.
pub fn lines(plan: &str) -> Vec<PlanLine> {
    let mut out = Vec::new();
    let mut in_code = false;
    for raw in plan.trim().lines() {
        let line = raw.trim_end();
        if line.trim_start().starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            out.push(PlanLine::Code(line.to_string()));
            continue;
        }
        let trimmed = line.trim_start();
        let indent = (line.len() - trimmed.len()) / 2;
        if trimmed.is_empty() {
            // One blank line between blocks is enough.
            if !matches!(out.last(), Some(PlanLine::Blank) | None) {
                out.push(PlanLine::Blank);
            }
        } else if let Some(heading) = heading(trimmed) {
            out.push(heading);
        } else if let Some(item) = item(trimmed, indent) {
            out.push(item);
        } else {
            out.push(PlanLine::Text(trimmed.to_string()));
        }
    }
    while matches!(out.last(), Some(PlanLine::Blank)) {
        out.pop();
    }
    out
}

fn heading(line: &str) -> Option<PlanLine> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    let rest = line.get(hashes..)?;
    ((1..=6).contains(&hashes) && rest.starts_with(' ')).then(|| PlanLine::Heading {
        level: hashes as u8,
        text: rest.trim().to_string(),
    })
}

fn item(line: &str, indent: usize) -> Option<PlanLine> {
    for bullet in ["- ", "* ", "+ "] {
        if let Some(rest) = line.strip_prefix(bullet) {
            return Some(PlanLine::Item {
                indent,
                marker: "•".into(),
                text: rest.trim().to_string(),
            });
        }
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    let (number, rest) = line.split_at(digits);
    if digits > 0 && digits <= 3 {
        let rest = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") "))?;
        return Some(PlanLine::Item {
            indent,
            marker: format!("{number}."),
            text: rest.trim().to_string(),
        });
    }
    None
}

/// What a plan question's option leads to, shown under its label; `None` for
/// any other question.
pub fn option_hint(question_id: &str, label: &str) -> Option<String> {
    use zeron_proto::policy::{
        PLAN_APPROVE_MODES, PLAN_KEEP_PLANNING, is_plan_question, plan_approve_label,
    };
    if !is_plan_question(question_id) {
        return None;
    }
    if label == PLAN_KEEP_PLANNING {
        return Some("Or type what to change below".into());
    }
    PLAN_APPROVE_MODES
        .iter()
        .find(|mode| plan_approve_label(**mode) == label)
        .map(|mode| format!("Carry out the plan. {}", mode.description()))
}

/// The plan as a card: a rounded, scrolling block capped in height so the
/// options below it stay on screen.
pub fn render(theme: &Theme, plan: &str) -> AnyElement {
    let body = lines(plan).into_iter().map(|line| match line {
        PlanLine::Heading { level, text } => div()
            .mt(px(if level <= 2 { 8.0 } else { 4.0 }))
            .text_size(crate::typography::ui_rems(if level <= 1 { 15.0 } else { 14.0 }))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme.text)
            .child(SharedString::from(text))
            .into_any_element(),
        PlanLine::Item {
            indent,
            marker,
            text,
        } => div()
            .flex()
            .flex_row()
            .gap(px(8.0))
            .pl(px(indent as f32 * 14.0))
            .child(
                div()
                    .w(px(18.0))
                    .flex_none()
                    .text_color(theme.text_muted)
                    .child(SharedString::from(marker)),
            )
            .child(div().flex_1().min_w_0().child(SharedString::from(text)))
            .into_any_element(),
        PlanLine::Code(text) => div()
            .px(px(8.0))
            .font_family(theme.font_mono.clone())
            .text_size(crate::typography::ui_rems(12.5))
            .bg(crate::theme::ink(0.05))
            .text_color(theme.text.opacity(0.9))
            .child(SharedString::from(if text.is_empty() { " ".into() } else { text }))
            .into_any_element(),
        PlanLine::Text(text) => div().child(SharedString::from(text)).into_any_element(),
        PlanLine::Blank => div().h(px(6.0)).into_any_element(),
    });
    div()
        .id("plan-card")
        .mt(px(10.0))
        .px(px(14.0))
        .py(px(10.0))
        .max_h(px(280.0))
        .overflow_y_scroll()
        .rounded(px(12.0))
        .border_1()
        .border_color(crate::theme::hairline(0.08))
        .bg(crate::theme::ink(0.03))
        .flex()
        .flex_col()
        .gap(px(2.0))
        .text_size(crate::typography::ui_rems(13.5))
        .line_height(px(20.0))
        .text_color(theme.text.opacity(0.92))
        .children(body)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plan_lays_out_as_headings_items_code_and_text() {
        let plan = "# Fix the sync test\n\nWe'll make it deterministic.\n\n1. Seed the clock\n2) Run it 50 times\n   - in CI too\n\n```sh\ncargo test -p zeron-sync\n```\n";
        assert_eq!(
            lines(plan),
            vec![
                PlanLine::Heading { level: 1, text: "Fix the sync test".into() },
                PlanLine::Blank,
                PlanLine::Text("We'll make it deterministic.".into()),
                PlanLine::Blank,
                PlanLine::Item { indent: 0, marker: "1.".into(), text: "Seed the clock".into() },
                PlanLine::Item { indent: 0, marker: "2.".into(), text: "Run it 50 times".into() },
                PlanLine::Item { indent: 1, marker: "•".into(), text: "in CI too".into() },
                PlanLine::Blank,
                PlanLine::Code("cargo test -p zeron-sync".into()),
            ]
        );
    }

    #[test]
    fn plan_options_say_what_they_lead_to() {
        use zeron_proto::policy::{plan_approve_label, plan_question_id};
        let id = plan_question_id("n");
        let hint = option_hint(&id, &plan_approve_label(zeron_proto::PermissionMode::Auto));
        assert!(hint.unwrap().starts_with("Carry out the plan."));
        assert!(option_hint(&id, "Keep planning").unwrap().contains("type"));
        assert_eq!(option_hint(&id, "Something else"), None);
        assert_eq!(option_hint("q-1", "Keep planning"), None);
    }

    #[test]
    fn unrecognised_lines_stay_visible_as_text() {
        assert_eq!(
            lines("#hashtag not a heading\n1.5 is a number\n- "),
            vec![
                PlanLine::Text("#hashtag not a heading".into()),
                PlanLine::Text("1.5 is a number".into()),
                PlanLine::Text("-".into()),
            ]
        );
        assert!(lines("   \n\n").is_empty());
    }
}
