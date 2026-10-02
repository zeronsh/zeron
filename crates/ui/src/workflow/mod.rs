//! Dynamic workflows in the desktop UI (`docs/workflows.md`, `docs/workflows-ui.md`).
//!
//! * [`model`] — the pure view-model every surface reads (card, run pane,
//!   sidebar lines, acknowledgements). Unit-tested without a window.
//!
//! The gpui halves live next to what they extend: the transcript row is in
//! `transcript.rs`, the run pane is a right-pane surface in `shell.rs`.

pub mod approval;
pub mod artifact;
pub mod card;
pub mod model;
pub mod pane;
pub mod sidebar;
pub(crate) mod widgets;

use std::rc::Rc;

use gpui::{App, Window};

/// What a click on a workflow surface asks for. The surfaces (transcript
/// card, run pane, sidebar line, result row) only describe the intent; the
/// transcript, the shell or `AppState` carries it out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowAction {
    /// Expand or collapse a card (render-local state).
    ToggleCard(String),
    /// Expand or collapse a result row.
    ToggleResult(String),
    /// Open the run pane; `landing` is the phase to scroll it to.
    OpenRun {
        run_id: String,
        landing: Option<String>,
    },
    /// Open an agent's chat read-only.
    OpenActor {
        child_chat_id: String,
        title: String,
    },
    OpenArtifact {
        run_id: String,
        artifact_id: String,
    },
    Stop {
        run_id: String,
    },
    Resume {
        run_id: String,
    },
}

pub type ActionSink = Rc<dyn Fn(WorkflowAction, &mut Window, &mut App)>;

/// Whitespace collapsed, capped at 220 characters: one line of prose.
pub(crate) fn one_line(text: &str) -> String {
    zeron_proto::view::one_line(text)
}

/// Capture knob `ZERON_WORKFLOW_CARD=expanded|collapsed`: the default open
/// state of every card (screenshots). Read once.
pub(crate) fn card_open_override() -> Option<bool> {
    static KNOB: std::sync::OnceLock<Option<bool>> = std::sync::OnceLock::new();
    *KNOB.get_or_init(|| match std::env::var("ZERON_WORKFLOW_CARD").as_deref() {
        Ok("expanded") => Some(true),
        Ok("collapsed") => Some(false),
        _ => None,
    })
}

/// Capture knob `ZERON_WORKFLOW_RESULT=expanded`: result rows start open.
pub(crate) fn result_open_override() -> bool {
    static KNOB: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *KNOB.get_or_init(|| std::env::var("ZERON_WORKFLOW_RESULT").as_deref() == Ok("expanded"))
}
