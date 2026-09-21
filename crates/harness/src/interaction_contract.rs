//! Adapter transport contracts, deliberately separate from live availability.
//!
//! A transport here is not a promise that a model exposes a tool. Modes still
//! require catalog/session discovery; question flags remain request-specific.
use zeron_proto::{HarnessId, UserInputQuestion};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuestionTransport {
    ClaudeControl,
    CodexAppServer,
    OpenCodeHttp,
    AcpOptionId,
    Unavailable,
    Fixture,
}
impl QuestionTransport {
    pub fn accepts(self, question: &UserInputQuestion) -> bool {
        match self {
            Self::Unavailable => false,
            Self::AcpOptionId => {
                !question.non_blocking
                    && !question.allow_custom
                    && !question.multi_select
                    && !question.options.is_empty()
            }
            Self::ClaudeControl | Self::OpenCodeHttp => {
                !question.non_blocking && (question.allow_custom || !question.options.is_empty())
            }
            _ => question.allow_custom || !question.options.is_empty(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeSource {
    ClaudePermissionMode,
    CodexLivePresets,
    CursorSdk,
    OpenCodeLivePrimaryAgents,
    AcpLiveConfiguration,
    Fixture,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionSignal {
    ItemLifecycle,
    /// Do not fabricate progress from a slash-command name or elapsed time.
    NotMapped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanProseTransport {
    CodexItems,
    ClaudePlanExit,
    CursorCreatePlan,
    NotMapped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskTransport {
    ClaudeTaskTools,
    CodexPlansAndTodos,
    CursorUpdateTodos,
    OpenCodeTodoEvents,
    AcpPlanEntries,
    Fixture,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InteractionContract {
    pub questions: QuestionTransport,
    pub modes: ModeSource,
    pub compaction: CompactionSignal,
    pub plan_prose: PlanProseTransport,
    pub tasks: TaskTransport,
    /// Still requires the live goals feature to be enabled.
    pub native_goal_transport: bool,
}

/// Exhaustive on HarnessId: adding a harness requires an explicit contract.
pub fn for_harness(harness: HarnessId) -> InteractionContract {
    use HarnessId::*;
    let (questions, modes, compaction, native_goal_transport) = match harness {
        ClaudeCode => (
            QuestionTransport::ClaudeControl,
            ModeSource::ClaudePermissionMode,
            CompactionSignal::NotMapped,
            false,
        ),
        Codex => (
            QuestionTransport::CodexAppServer,
            ModeSource::CodexLivePresets,
            CompactionSignal::ItemLifecycle,
            true,
        ),
        Cursor => (
            QuestionTransport::Unavailable,
            ModeSource::CursorSdk,
            CompactionSignal::NotMapped,
            false,
        ),
        Opencode => (
            QuestionTransport::OpenCodeHttp,
            ModeSource::OpenCodeLivePrimaryAgents,
            CompactionSignal::NotMapped,
            false,
        ),
        Devin | Grok | Hermes | Pi | Antigravity => (
            QuestionTransport::AcpOptionId,
            ModeSource::AcpLiveConfiguration,
            CompactionSignal::NotMapped,
            false,
        ),
        Mock => (
            QuestionTransport::Fixture,
            ModeSource::Fixture,
            CompactionSignal::NotMapped,
            false,
        ),
    };
    let (plan_prose, tasks) = match harness {
        ClaudeCode => (
            PlanProseTransport::ClaudePlanExit,
            TaskTransport::ClaudeTaskTools,
        ),
        Codex => (
            PlanProseTransport::CodexItems,
            TaskTransport::CodexPlansAndTodos,
        ),
        Cursor => (
            PlanProseTransport::CursorCreatePlan,
            TaskTransport::CursorUpdateTodos,
        ),
        Opencode => (
            PlanProseTransport::NotMapped,
            TaskTransport::OpenCodeTodoEvents,
        ),
        Devin | Grok | Hermes | Pi | Antigravity => {
            (PlanProseTransport::NotMapped, TaskTransport::AcpPlanEntries)
        }
        Mock => (PlanProseTransport::NotMapped, TaskTransport::Fixture),
    };
    InteractionContract {
        questions,
        modes,
        compaction,
        plan_prose,
        tasks,
        native_goal_transport,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_production_adapter_has_an_explicit_transport_contract() {
        for harness in [
            HarnessId::ClaudeCode,
            HarnessId::Codex,
            HarnessId::Cursor,
            HarnessId::Opencode,
            HarnessId::Devin,
            HarnessId::Grok,
            HarnessId::Hermes,
            HarnessId::Pi,
            HarnessId::Antigravity,
        ] {
            let contract = for_harness(harness);
            assert_ne!(contract.questions, QuestionTransport::Fixture);
            assert_ne!(contract.tasks, TaskTransport::Fixture);
            assert_eq!(contract.native_goal_transport, harness == HarnessId::Codex);
            assert_eq!(
                contract.compaction == CompactionSignal::ItemLifecycle,
                harness == HarnessId::Codex
            );
        }
        let mut question: UserInputQuestion = serde_json::from_value(serde_json::json!({"id":"q","header":"Decision","question":"Continue?","options":["Yes","No"],"allowCustom":false})).unwrap();
        assert!(QuestionTransport::AcpOptionId.accepts(&question));
        question.allow_custom = true;
        assert!(!QuestionTransport::AcpOptionId.accepts(&question));
        assert!(QuestionTransport::CodexAppServer.accepts(&question));
        assert!(!QuestionTransport::Unavailable.accepts(&question));
    }

    #[test]
    fn only_codex_supports_asynchronous_production_questions() {
        let question: UserInputQuestion = serde_json::from_value(serde_json::json!({
            "id": "q", "header": "Decision", "question": "Continue?",
            "options": ["Yes", "No"], "allowCustom": false, "nonBlocking": true
        }))
        .unwrap();
        for harness in [
            HarnessId::ClaudeCode,
            HarnessId::Codex,
            HarnessId::Cursor,
            HarnessId::Opencode,
            HarnessId::Devin,
            HarnessId::Grok,
            HarnessId::Hermes,
            HarnessId::Pi,
            HarnessId::Antigravity,
        ] {
            assert_eq!(
                for_harness(harness).questions.accepts(&question),
                harness == HarnessId::Codex,
                "unexpected asynchronous input contract for {harness:?}"
            );
        }
    }
}
