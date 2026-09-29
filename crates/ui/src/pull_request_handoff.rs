//! Hand a pull request to an agent: each action opens a new session with a
//! prompt that carries the PR's identity, branch, and the exact `gh`
//! commands an agent needs. The prompt is only staged in the composer; the
//! user reviews and sends it.
use super::*;
use zeron_proto::ChangeRequestCheck;

/// What the new session should do with the pull request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Handoff {
    Review,
    AddressFeedback,
    FixChecks,
}

impl Handoff {
    fn label(self) -> &'static str {
        match self {
            Self::Review => "Review",
            Self::AddressFeedback => "Address feedback",
            Self::FixChecks => "Fix failing checks",
        }
    }

    fn id(self) -> &'static str {
        match self {
            Self::Review => "pr-handoff-review",
            Self::AddressFeedback => "pr-handoff-feedback",
            Self::FixChecks => "pr-handoff-checks",
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Self::Review => crate::icons::EYE,
            Self::AddressFeedback => crate::icons::CHAT_ROUND_LINE,
            Self::FixChecks => crate::icons::DANGER_TRIANGLE,
        }
    }
}

fn check_status(check: &ChangeRequestCheck) -> String {
    [&check.conclusion, &check.state, &check.status]
        .into_iter()
        .find(|value| !value.is_empty())
        .map(|value| value.to_ascii_uppercase())
        .unwrap_or_default()
}

/// Failed checks as (name, details link).
pub(super) fn failing_checks(detail: &ChangeRequestDetail) -> Vec<(String, String)> {
    detail
        .status_check_rollup
        .iter()
        .filter(|check| {
            matches!(
                check_status(check).as_str(),
                "FAILURE"
                    | "ERROR"
                    | "TIMED_OUT"
                    | "ACTION_REQUIRED"
                    | "CANCELLED"
                    | "STARTUP_FAILURE"
            )
        })
        .map(|check| {
            let name = if check.name.is_empty() {
                check.context.clone()
            } else {
                check.name.clone()
            };
            let link = if check.details_url.is_empty() {
                check.target_url.clone()
            } else {
                check.details_url.clone()
            };
            (name, link)
        })
        .collect()
}

/// `owner/name` from a GitHub pull request URL.
pub(super) fn repository(url: &str) -> String {
    url.split('/').skip(3).take(2).collect::<Vec<_>>().join("/")
}

pub(super) fn checkout_command(detail: &ChangeRequestDetail, url: &str) -> String {
    format!(
        "gh pr checkout {} --repo {}",
        detail.number,
        repository(url)
    )
}

pub(super) fn prompt(kind: Handoff, detail: &ChangeRequestDetail, url: &str) -> String {
    let repo = repository(url);
    let number = detail.number;
    let mut prompt = format!(
        "Pull request #{number} in {repo}: {title}\n{url}\nBranch: {head} → {base}\nCheck it out with `{checkout}`.\n\n",
        title = detail.title,
        head = detail.head_ref_name,
        base = detail.base_ref_name,
        checkout = checkout_command(detail, url),
    );
    match kind {
        Handoff::Review => prompt.push_str(&format!(
            "Review this pull request. Read the description with `gh pr view {number} --repo {repo}` and the diff with `gh pr diff {number} --repo {repo}`. Look for correctness bugs, regressions, and missing tests. Report findings by severity with file:line references. Don't push changes or post comments."
        )),
        Handoff::AddressFeedback => prompt.push_str(&format!(
            "Address the open review feedback. Read it with `gh pr view {number} --repo {repo} --comments`. Make the requested changes on `{head}`, run the relevant tests, and summarize what changed for each comment. Ask before pushing.",
            head = detail.head_ref_name,
        )),
        Handoff::FixChecks => {
            prompt.push_str("These checks are failing:\n");
            for (name, link) in failing_checks(detail) {
                if link.is_empty() {
                    prompt.push_str(&format!("- {name}\n"));
                } else {
                    prompt.push_str(&format!("- {name}: {link}\n"));
                }
            }
            prompt.push_str(&format!(
                "\nInspect them with `gh pr checks {number} --repo {repo}` and the failed logs with `gh run view <run-id> --repo {repo} --log-failed`. Reproduce each failure locally, fix the root cause on `{head}`, and verify the fix. Ask before pushing.",
                head = detail.head_ref_name,
            ));
        }
    }
    prompt
}

/// The actions that apply to this pull request, most useful first.
pub(super) fn handoffs(detail: &ChangeRequestDetail) -> Vec<Handoff> {
    let mut kinds = Vec::new();
    if !failing_checks(detail).is_empty() {
        kinds.push(Handoff::FixChecks);
    }
    if detail
        .review_decision
        .eq_ignore_ascii_case("CHANGES_REQUESTED")
        || !detail.comments.is_empty()
        || !detail.reviews.is_empty()
    {
        kinds.push(Handoff::AddressFeedback);
    }
    kinds.push(Handoff::Review);
    kinds
}

impl PullRequestDetailPage {
    pub(super) fn handoff_card(
        &self,
        detail: &ChangeRequestDetail,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let command = checkout_command(detail, &self.url);
        let copy = command.clone();
        let buttons = handoffs(detail).into_iter().map(|kind| {
            let prompt = prompt(kind, detail, &self.url);
            let id = kind.id();
            widgets::ghost_action(theme)
                .id(id)
                .debug_selector(move || id.to_owned())
                .role(gpui::Role::Button)
                .aria_label(format!("{} in a new session", kind.label()))
                .tab_index(0)
                .h(px(32.0))
                .px(px(12.0))
                .gap(px(6.0))
                .border_1()
                .border_color(theme.border)
                .focus_visible(|style| style.border_color(theme.accent))
                .cursor_pointer()
                .child(
                    crate::icons::icon(kind.glyph())
                        .size(px(14.0))
                        .text_color(theme.text_muted),
                )
                .child(kind.label())
                .on_click(move |_, window, cx| {
                    window.dispatch_action(Box::new(StartPullRequestSession(prompt.clone())), cx)
                })
        });
        widgets::section_card(theme)
                .mt(px(CARD_GAP))
                .id("pr-handoff")
                .debug_selector(|| "pr-handoff".into())
                .child(
                    div()
                        .p(px(16.0))
                        .flex()
                        .flex_col()
                        .gap(px(12.0))
                        .child(
                            div()
                                .flex()
                                .items_start()
                                .gap(px(10.0))
                                .child(
                                    crate::icons::icon(crate::icons::BOT)
                                        .size(px(16.0))
                                        .flex_none()
                                        .mt(px(1.0))
                                        .text_color(theme.text_muted),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .flex()
                                        .flex_col()
                                        .gap(px(2.0))
                                        .child(
                                            div()
                                                .font_weight(gpui::FontWeight::MEDIUM)
                                                .child("Hand off to an agent"),
                                        )
                                        .child(
                                            div()
                                                .text_size(crate::typography::ui_rems(12.0))
                                                .text_color(theme.text_muted)
                                                .child(
                                                    "Opens a new session with this pull request's context. Review the prompt, then send it.",
                                                ),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .id("pr-handoff-actions")
                                .debug_selector(|| "pr-handoff-actions".into())
                                .pl(px(26.0))
                                .flex()
                                .flex_wrap()
                                .gap(px(8.0))
                                .children(buttons),
                        ),
                )
                .child(
                    widgets::card_row(theme, false)
                        .min_h(px(44.0))
                        .py(px(8.0))
                        .gap(px(8.0))
                        .child(
                            div()
                                .w(px(80.0))
                                .flex_none()
                                .child(widgets::row_title(theme, "Checkout")),
                        )
                        .child(
                            div()
                                .id("pr-checkout-command")
                                .debug_selector(|| "pr-checkout-command".into())
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .font_family(theme.font_mono.clone())
                                .text_size(crate::typography::ui_rems(12.0))
                                .child(command),
                        )
                        .child(
                            action("pr-copy-checkout", "Copy checkout command", theme).on_click(
                                move |_, _, cx| {
                                    cx.stop_propagation();
                                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                        copy.clone(),
                                    ));
                                },
                            ),
                        ),
                )
                .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detail() -> ChangeRequestDetail {
        ChangeRequestDetail {
            title: "Add dictation".into(),
            number: 591,
            head_ref_name: "feat/dictation".into(),
            base_ref_name: "main".into(),
            status_check_rollup: vec![
                ChangeRequestCheck {
                    name: "linux".into(),
                    conclusion: "FAILURE".into(),
                    details_url: "https://github.com/acme/zeron/actions/runs/1".into(),
                    ..Default::default()
                },
                ChangeRequestCheck {
                    name: "macos".into(),
                    conclusion: "SUCCESS".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn pull_request_handoff_prompts_carry_identity_and_commands() {
        let url = "https://github.com/acme/zeron/pull/591";
        let detail = detail();
        assert_eq!(handoffs(&detail), [Handoff::FixChecks, Handoff::Review]);
        assert_eq!(
            checkout_command(&detail, url),
            "gh pr checkout 591 --repo acme/zeron"
        );
        let checks = prompt(Handoff::FixChecks, &detail, url);
        assert!(checks.starts_with("Pull request #591 in acme/zeron: Add dictation\n"));
        assert!(checks.contains("Branch: feat/dictation → main"));
        assert!(checks.contains("- linux: https://github.com/acme/zeron/actions/runs/1"));
        assert!(!checks.contains("macos"), "passing checks are not listed");
        let review = prompt(Handoff::Review, &detail, url);
        assert!(review.contains("gh pr diff 591 --repo acme/zeron"));
        let mut discussed = detail.clone();
        discussed.status_check_rollup.clear();
        discussed.review_decision = "CHANGES_REQUESTED".into();
        assert_eq!(
            handoffs(&discussed),
            [Handoff::AddressFeedback, Handoff::Review]
        );
    }
}
