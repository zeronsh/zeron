//! Permission modes in the composer (docs/plans/2026-09-30-agent-mobility-and-policy.md,
//! Part 4): the pure half of the mode chip and its menu — which modes a
//! harness offers, why the others are greyed, what Shift+Tab cycles to, how
//! the chip reads — plus the device's default mode, shared app-wide.
//!
//! Rendering lives with the other composer pickers (`pickers.rs`).

use zeron_proto::policy::{APPROVAL_ALLOW_ALWAYS, APPROVAL_ALLOW_ONCE, APPROVAL_DENY};
use zeron_proto::{PermissionMode, PolicyCaps, SandboxMode};

/// The device's default permission mode (Settings → General), once known.
/// Composers seed new chats from it; the settings card updates it on save.
#[derive(Debug, Clone, Copy, Default)]
pub struct DefaultPermissionMode(pub Option<PermissionMode>);

impl gpui::Global for DefaultPermissionMode {}

/// The device default as currently known (Bypass until loaded).
pub fn device_default(cx: &gpui::App) -> PermissionMode {
    cx.try_global::<DefaultPermissionMode>()
        .and_then(|d| d.0)
        .unwrap_or_default()
}

/// Whether the device default has been loaded this session.
pub fn device_default_known(cx: &gpui::App) -> bool {
    cx.try_global::<DefaultPermissionMode>()
        .is_some_and(|d| d.0.is_some())
}

pub fn set_device_default(mode: PermissionMode, cx: &mut gpui::App) {
    cx.set_global(DefaultPermissionMode(Some(mode)));
}

/// One row of the mode menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeRow {
    pub mode: PermissionMode,
    pub label: &'static str,
    pub description: &'static str,
    pub selected: bool,
    /// Why the harness can't run in this mode; `None` = offered.
    pub unsupported: Option<String>,
}

impl ModeRow {
    pub fn enabled(&self) -> bool {
        self.unsupported.is_none()
    }
}

/// The five modes in menu order, greyed where `caps` (the harness's, from
/// its host's catalog) can't honour them. Unknown caps (catalog still
/// loading) offer everything; the host still refuses what it can't run.
pub fn mode_rows(
    caps: Option<&PolicyCaps>,
    harness: &str,
    current: PermissionMode,
) -> Vec<ModeRow> {
    PermissionMode::ALL
        .iter()
        .map(|&mode| ModeRow {
            mode,
            label: mode.label(),
            description: mode.description(),
            selected: mode == current,
            unsupported: caps.and_then(|caps| caps.unsupported_reason(harness, mode)),
        })
        .collect()
}

/// The sandbox choices, in menu order.
pub const SANDBOXES: [SandboxMode; 3] = [
    SandboxMode::Off,
    SandboxMode::WorkspaceWrite,
    SandboxMode::ReadOnly,
];

/// One row of the menu's sandbox section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxRow {
    pub sandbox: SandboxMode,
    pub label: &'static str,
    pub description: &'static str,
    pub selected: bool,
    /// Why this device can't sandbox the harness this way; `None` = offered.
    pub unsupported: Option<String>,
}

impl SandboxRow {
    pub fn enabled(&self) -> bool {
        self.unsupported.is_none()
    }
}

pub fn sandbox_description(sandbox: SandboxMode) -> &'static str {
    match sandbox {
        SandboxMode::Off => "The agent can touch anything you can",
        SandboxMode::WorkspaceWrite => "Writes only inside the project; secrets hidden",
        SandboxMode::ReadOnly => "Writes nothing outside its own state; secrets hidden",
    }
}

pub fn sandbox_icon(sandbox: SandboxMode) -> &'static str {
    match sandbox {
        SandboxMode::Off => crate::icons::FAST_TIER,
        SandboxMode::WorkspaceWrite => crate::icons::FOLDER_WITH_FILES,
        SandboxMode::ReadOnly => crate::icons::EYE,
    }
}

/// Why `harness` can't run in `sandbox` here; `None` when it can (or the
/// caps aren't known yet: the host still refuses what it can't provide).
pub fn sandbox_unsupported(
    caps: Option<&PolicyCaps>,
    harness: &str,
    sandbox: SandboxMode,
) -> Option<String> {
    let caps = caps?;
    (!caps.sandboxes.contains(&sandbox))
        .then(|| format!("This device can't sandbox {harness}"))
}

pub fn sandbox_rows(
    caps: Option<&PolicyCaps>,
    harness: &str,
    current: SandboxMode,
) -> Vec<SandboxRow> {
    SANDBOXES
        .iter()
        .map(|&sandbox| SandboxRow {
            sandbox,
            label: sandbox.label(),
            description: sandbox_description(sandbox),
            selected: sandbox == current,
            unsupported: sandbox_unsupported(caps, harness, sandbox),
        })
        .collect()
}

/// Shift+Tab: the next mode the harness offers after `current`, wrapping.
/// `current` itself when nothing else is offered.
pub fn next_mode(current: PermissionMode, caps: Option<&PolicyCaps>) -> PermissionMode {
    let all = PermissionMode::ALL;
    let at = all.iter().position(|m| *m == current).unwrap_or(0);
    (1..=all.len())
        .map(|step| all[(at + step) % all.len()])
        .find(|mode| caps.is_none_or(|caps| caps.supports(*mode)))
        .unwrap_or(current)
}

/// The chip's own text: none for Bypass (an icon only — it's the old,
/// everyday behaviour), a short name otherwise.
pub fn chip_label(mode: PermissionMode) -> Option<&'static str> {
    match mode {
        PermissionMode::Bypass => None,
        PermissionMode::Auto => Some("Auto"),
        PermissionMode::AcceptEdits => Some("Accept edits"),
        PermissionMode::Ask => Some("Ask"),
        PermissionMode::Plan => Some("Plan"),
    }
}

pub fn mode_icon(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Bypass => crate::icons::FAST_TIER,
        PermissionMode::Auto => crate::icons::MAGIC_STICK_3,
        PermissionMode::AcceptEdits => crate::icons::PEN,
        PermissionMode::Ask => crate::icons::CHAT_ROUND_LINE,
        PermissionMode::Plan => crate::icons::CHECKLIST,
    }
}

/// How loudly the chip reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeTone {
    /// Bypass: the muted footer tone.
    Subtle,
    /// Auto / Accept edits: the agent still acts on its own for most things.
    Accent,
    /// Ask / Plan: the agent stops for the user.
    Marked,
}

pub fn mode_tone(mode: PermissionMode) -> ModeTone {
    match mode {
        PermissionMode::Bypass => ModeTone::Subtle,
        PermissionMode::Auto | PermissionMode::AcceptEdits => ModeTone::Accent,
        PermissionMode::Ask | PermissionMode::Plan => ModeTone::Marked,
    }
}

/// The options of an approval question, for the question panel to style.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalOption {
    AllowOnce,
    AllowAlways,
    Deny,
}

/// `Some` for the three options of an approval question (its id carries the
/// approval prefix); `None` for an agent's own questions.
pub fn approval_option(question_id: &str, label: &str) -> Option<ApprovalOption> {
    if !zeron_proto::policy::is_approval_question(question_id) {
        return None;
    }
    match label {
        APPROVAL_ALLOW_ONCE => Some(ApprovalOption::AllowOnce),
        APPROVAL_ALLOW_ALWAYS => Some(ApprovalOption::AllowAlways),
        APPROVAL_DENY => Some(ApprovalOption::Deny),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_list_every_mode_and_grey_what_the_harness_cant_do() {
        let caps = PolicyCaps::bypass_only();
        let rows = mode_rows(Some(&caps), "Cursor", PermissionMode::Bypass);
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0].mode, PermissionMode::Bypass);
        assert!(rows[0].enabled() && rows[0].selected);
        assert_eq!(rows[0].label, "Bypass permissions");
        assert_eq!(rows[4].description, PermissionMode::Plan.description());
        for row in &rows[1..] {
            assert!(!row.enabled());
            assert_eq!(
                row.unsupported.as_deref(),
                Some("Cursor runs without asking — it can only bypass permissions")
            );
        }
        // Unknown caps: everything offered.
        assert!(mode_rows(None, "Claude Code", PermissionMode::Ask)
            .iter()
            .all(ModeRow::enabled));
        let all = PolicyCaps::all_modes();
        let rows = mode_rows(Some(&all), "Claude Code", PermissionMode::Plan);
        assert!(rows.iter().all(ModeRow::enabled));
        assert_eq!(rows.iter().filter(|r| r.selected).count(), 1);
        assert!(rows[4].selected);
    }

    #[test]
    fn sandbox_rows_grey_what_this_device_cant_provide() {
        let none = PolicyCaps::bypass_only();
        let rows = sandbox_rows(Some(&none), "Pi", SandboxMode::Off);
        assert_eq!(rows.len(), 3);
        assert!(rows[0].enabled() && rows[0].selected);
        assert_eq!(
            rows[1].unsupported.as_deref(),
            Some("This device can't sandbox Pi")
        );
        assert!(!rows[2].enabled());
        let all = PolicyCaps {
            sandboxes: SANDBOXES.to_vec(),
            ..PolicyCaps::bypass_only()
        };
        let rows = sandbox_rows(Some(&all), "Pi", SandboxMode::ReadOnly);
        assert!(rows.iter().all(SandboxRow::enabled));
        assert!(rows[2].selected);
        assert!(sandbox_rows(None, "Pi", SandboxMode::Off)
            .iter()
            .all(SandboxRow::enabled));
    }

    #[test]
    fn shift_tab_cycles_through_offered_modes_only() {
        let all = PolicyCaps::all_modes();
        let mut mode = PermissionMode::Bypass;
        let mut seen = Vec::new();
        for _ in 0..5 {
            mode = next_mode(mode, Some(&all));
            seen.push(mode);
        }
        assert_eq!(
            seen,
            [
                PermissionMode::Auto,
                PermissionMode::AcceptEdits,
                PermissionMode::Ask,
                PermissionMode::Plan,
                PermissionMode::Bypass,
            ]
        );
        let some = PolicyCaps {
            modes: vec![PermissionMode::Bypass, PermissionMode::Ask, PermissionMode::Plan],
            ..PolicyCaps::bypass_only()
        };
        assert_eq!(next_mode(PermissionMode::Bypass, Some(&some)), PermissionMode::Ask);
        assert_eq!(next_mode(PermissionMode::Plan, Some(&some)), PermissionMode::Bypass);
        // An unsupported current mode moves to the next offered one.
        assert_eq!(next_mode(PermissionMode::Auto, Some(&some)), PermissionMode::Ask);
        let bypass = PolicyCaps::bypass_only();
        assert_eq!(next_mode(PermissionMode::Bypass, Some(&bypass)), PermissionMode::Bypass);
        assert_eq!(next_mode(PermissionMode::Plan, None), PermissionMode::Bypass);
    }

    #[test]
    fn the_chip_is_quiet_for_bypass_and_marked_otherwise() {
        assert_eq!(chip_label(PermissionMode::Bypass), None);
        assert_eq!(mode_tone(PermissionMode::Bypass), ModeTone::Subtle);
        assert_eq!(chip_label(PermissionMode::Plan), Some("Plan"));
        assert_eq!(mode_tone(PermissionMode::Plan), ModeTone::Marked);
        assert_eq!(mode_tone(PermissionMode::Ask), ModeTone::Marked);
        assert_eq!(mode_tone(PermissionMode::Auto), ModeTone::Accent);
        assert_eq!(chip_label(PermissionMode::AcceptEdits), Some("Accept edits"));
        let icons: std::collections::HashSet<_> =
            PermissionMode::ALL.iter().map(|m| mode_icon(*m)).collect();
        assert_eq!(icons.len(), 5, "each mode reads differently");
    }

    #[test]
    fn approval_options_are_recognised_only_on_approval_questions() {
        let id = zeron_proto::policy::approval_question_id("n", None);
        assert_eq!(approval_option(&id, "Allow once"), Some(ApprovalOption::AllowOnce));
        assert_eq!(approval_option(&id, "Always allow"), Some(ApprovalOption::AllowAlways));
        assert_eq!(approval_option(&id, "Deny"), Some(ApprovalOption::Deny));
        assert_eq!(approval_option(&id, "Maybe"), None);
        assert_eq!(approval_option("q-sync", "Deny"), None);
    }
}
