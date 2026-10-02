//! `path:line:col message` diagnostics for scripts that cannot run.

use serde::{Deserialize, Serialize};

/// One problem found before (or while) running a script. Lines and columns
/// are 1-based; a diagnostic without a position carries line 0.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub path: String,
    pub line: u32,
    pub col: u32,
    pub message: String,
}

impl Diagnostic {
    pub fn new(path: &str, line: u32, col: u32, message: impl Into<String>) -> Self {
        Self {
            path: path.to_owned(),
            line,
            col,
            message: message.into(),
        }
    }

    pub fn unpositioned(path: &str, message: impl Into<String>) -> Self {
        Self::new(path, 0, 0, message)
    }
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.line == 0 {
            write!(f, "{}: {}", self.path, self.message)
        } else {
            write!(
                f,
                "{}:{}:{} {}",
                self.path, self.line, self.col, self.message
            )
        }
    }
}

/// Render a list one per line, the shape tool errors return to the model.
pub fn render(diagnostics: &[Diagnostic]) -> String {
    diagnostics
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}
