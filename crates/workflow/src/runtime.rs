//! Running a script: parse, load its `def`s, call `main(args)`.
//!
//! Two evaluation passes share the analysis' parse rules:
//!
//! 1. the module is evaluated once (only `def`s and constant assignments are
//!    allowed at the top level; host calls there are refused) and **frozen** —
//!    frozen functions are `Send + Sync`, which is what lets `pmap` call a
//!    top-level `def` on worker threads;
//! 2. `main(args)` runs in a fresh module that sees the frozen one, with `args`
//!    frozen too.
//!
//! A run is blocking: call [`run_script`] from a blocking thread, never from an
//! async worker (`.result()` parks the thread).

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use serde_json::Value as Json;
use starlark::environment::Module;
use starlark::eval::Evaluator;
use starlark::syntax::AstModule;

use crate::analysis::{Analysis, dialect};
use crate::diagnostic::Diagnostic;
use crate::globals::{Abort, RunCtx, Shared, classify_error, configure, globals};
use crate::host::Host;
use crate::limits::Limits;
use crate::site::Ordinals;

/// Why a run did not produce a value.
#[derive(Debug, Clone, PartialEq)]
pub enum RunError {
    /// The script no longer parses (the caller should have analysed it first).
    Diagnostics(Vec<Diagnostic>),
    /// The script failed: `fail(...)`, a type error, a refused host call.
    Script(Diagnostic),
    /// The cancel flag was raised.
    Cancelled,
    /// A step, time or memory limit was hit.
    Limit(String),
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunError::Diagnostics(d) => f.write_str(&crate::diagnostic::render(d)),
            RunError::Script(d) => write!(f, "{d}"),
            RunError::Cancelled => f.write_str("the workflow was cancelled"),
            RunError::Limit(m) => write!(f, "limit reached: {m}"),
        }
    }
}

impl std::error::Error for RunError {}

fn script_error(path: &str, e: &starlark::Error, text: String) -> RunError {
    let (line, col) = e.span().map_or((0, 0), |s| {
        let r = s.resolve_span();
        (r.begin.line as u32 + 1, r.begin.column as u32 + 1)
    });
    RunError::Script(Diagnostic::new(path, line, col, text))
}

/// Run `main(args)` of an analysed script. Blocking.
pub fn run_script(
    path: &str,
    src: &str,
    args: &Json,
    analysis: &Analysis,
    host: Arc<dyn Host>,
    cancel: Arc<AtomicBool>,
    limits: &Limits,
) -> Result<Json, RunError> {
    let ast = AstModule::parse(path, src.to_owned(), &dialect()).map_err(|e| {
        RunError::Diagnostics(vec![Diagnostic::unpositioned(
            path,
            e.without_diagnostic().to_string(),
        )])
    })?;
    let shared = Arc::new(Shared::new(
        host,
        cancel,
        limits.clone(),
        analysis.sites.clone(),
    ));
    let root = RunCtx::new(shared, Ordinals::root(), false);
    let globals = globals();

    // Pass 1: definitions.
    root.loading
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let frozen = Module::with_temp_heap(|m| {
        {
            let mut ev = Evaluator::new(&m);
            ev.extra = Some(&root);
            configure(&mut ev, &root).map_err(|e| RunError::Limit(e.to_string()))?;
            if let Err(e) = ev.eval_module(ast, globals) {
                return Err(match classify_error(&root, &e) {
                    Ok(text) => script_error(path, &e, text),
                    Err(abort) => abort_error(abort),
                });
            }
        }
        m.freeze()
            .map_err(|e| RunError::Limit(format!("could not freeze the script: {e:?}")))
    })?;
    root.loading
        .store(false, std::sync::atomic::Ordering::Relaxed);
    frozen.get("main").map_err(|_| {
        RunError::Diagnostics(vec![Diagnostic::new(
            path,
            1,
            1,
            "the script must define `def main(args):`",
        )])
    })?;
    let frozen_args = Module::with_temp_heap(|m| {
        m.set("args", m.heap().alloc(args));
        m.freeze()
            .map_err(|e| RunError::Limit(format!("could not freeze args: {e:?}")))
    })?;
    let entry = AstModule::parse("entry.star", "main(args)".to_owned(), &dialect())
        .map_err(|e| RunError::Limit(e.to_string()))?;

    // Pass 2: main(args).
    Module::with_temp_heap(|m| {
        m.import_public_symbols(&frozen);
        m.import_public_symbols(&frozen_args);
        let mut ev = Evaluator::new(&m);
        ev.extra = Some(&root);
        configure(&mut ev, &root).map_err(|e| RunError::Limit(e.to_string()))?;
        match ev.eval_module(entry, globals) {
            Ok(v) => v.to_json_value().map_err(|e| {
                RunError::Script(Diagnostic::unpositioned(
                    path,
                    format!("main must return a JSON-able value (no actors or handles): {e}"),
                ))
            }),
            Err(e) => Err(match classify_error(&root, &e) {
                Ok(text) => script_error(path, &e, text),
                Err(abort) => abort_error(abort),
            }),
        }
    })
}

fn abort_error(abort: Abort) -> RunError {
    match abort {
        Abort::Cancelled => RunError::Cancelled,
        Abort::Limit(m) => RunError::Limit(m),
    }
}
