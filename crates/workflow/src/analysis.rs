//! Static analysis of a workflow script.
//!
//! One parser serves analysis and execution (the same [`dialect`]), so the
//! graph shown at approval can never disagree with what runs. Analysis
//! produces either a list of `path:line:col message` diagnostics — in which
//! case no run is created — or an [`Analysis`] holding:
//!
//! * the [`WorkflowGraph`] (phases, actors, asks per phase, literal commands),
//!   which the approval dialog and the card skeleton render, and
//! * the [`CallSites`] the runtime checks every `run()` / `phase()` /
//!   `artifact.*()` call against, so a call that analysis could not see
//!   (an aliased builtin: `r = run; r(cmd)`) is refused at run time.
//!
//! ## What is enforced
//!
//! * the script defines `def main(args)`;
//! * `phase("literal")` is a statement; a phase **claims the rest of its
//!   block** — every statement after it in the same block, nested blocks
//!   included, and every top-level `def` those statements call — until the
//!   next `phase()` in that block; every phase must contain at least one
//!   `.ask(...)` or `run(...)`;
//! * `run("literal", ...)`: the program is a string literal; an interpreter
//!   or shell (`sh`, `bash`, `python`, `env`, …) additionally needs a literal
//!   argument list, so the user approving the run sees everything it can do;
//! * `artifact.*(id, ...)` ids are string literals;
//! * no recursion between top-level defs (the interpreter would only fail at
//!   depth; this says it at parse time);
//! * `pmap` / `parallel` take top-level `def` names, never lambdas;
//! * no host calls at module level; names the language does not define
//!   (`time`, `open`, `getenv`, …) are reported where they are used.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use starlark::analysis::AstModuleLint;
use starlark::codemap::Span;
use starlark::syntax::ast::{
    ArgumentP, AssignTargetP, AstExpr, AstLiteral, AstStmt, CallArgsP, ClauseP, ExprP, ParameterP,
    StmtP,
};
use starlark::syntax::{AstModule, Dialect};
use zeron_proto::{GraphActor, GraphAsk, GraphCommand, GraphPhase, GraphSite, WorkflowGraph};

use crate::diagnostic::Diagnostic;
use crate::limits::MAX_SCRIPT_BYTES;
use crate::site::site_of;

/// The one dialect: Starlark with f-strings (bare identifiers only), keyword-
/// only parameters, `def` and `lambda`, no `load`, no top-level statements.
pub fn dialect() -> Dialect {
    Dialect {
        enable_f_strings: true,
        enable_keyword_only_arguments: true,
        enable_load: false,
        ..Dialect::Standard
    }
}

/// Programs that run other programs or arbitrary code: allowed as a literal
/// command only with a literal argument list.
const INTERPRETERS: &[&str] = &[
    "sh",
    "bash",
    "zsh",
    "fish",
    "dash",
    "ksh",
    "csh",
    "tcsh",
    "cmd",
    "cmd.exe",
    "powershell",
    "powershell.exe",
    "pwsh",
    "env",
    "xargs",
    "eval",
    "sudo",
    "doas",
    "nohup",
    "busybox",
    "python",
    "python3",
    "node",
    "deno",
    "bun",
    "perl",
    "ruby",
    "php",
    "lua",
];

/// Names of the builtins a script may call; `lint` is told these are defined.
pub const HOST_GLOBALS: &[&str] = &[
    "phase", "agent", "run", "pmap", "parallel", "wait_all", "results", "log", "report", "schema",
    "files", "git", "artifact",
];

/// What the runtime may call, per call site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SiteKind {
    Phase(String),
    /// A `run("program", ...)` call.
    Run {
        program: String,
    },
    /// `artifact.<kind>("id", ...)`.
    Artifact {
        id: String,
    },
}

/// Sites the runtime is allowed to execute host-effecting builtins from.
#[derive(Debug, Clone, Default)]
pub struct CallSites {
    sites: HashMap<String, SiteKind>,
}

impl CallSites {
    pub fn get(&self, site: &str) -> Option<&SiteKind> {
        self.sites.get(site)
    }

    pub fn len(&self) -> usize {
        self.sites.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sites.is_empty()
    }
}

/// A script that passed analysis.
#[derive(Debug, Clone)]
pub struct Analysis {
    pub graph: WorkflowGraph,
    pub sites: CallSites,
    /// Non-blocking findings (shown to the author, never refuse a run).
    pub warnings: Vec<Diagnostic>,
}

/// Parse and analyse `src`. `path` only labels diagnostics.
pub fn analyze(path: &str, src: &str) -> Result<Analysis, Vec<Diagnostic>> {
    if src.len() > MAX_SCRIPT_BYTES {
        return Err(vec![Diagnostic::unpositioned(
            path,
            format!(
                "the script is {} bytes; the limit is {MAX_SCRIPT_BYTES}",
                src.len()
            ),
        )]);
    }
    let ast = AstModule::parse(path, src.to_owned(), &dialect()).map_err(|e| {
        let message = hint(&e.without_diagnostic().to_string());
        vec![match e.span() {
            Some(span) => {
                let r = span.resolve_span();
                Diagnostic::new(
                    path,
                    r.begin.line as u32 + 1,
                    r.begin.column as u32 + 1,
                    message,
                )
            }
            None => Diagnostic::unpositioned(path, message),
        }]
    })?;
    let mut a = Analyzer::new(path, &ast);
    a.run();
    a.finish()
}

/// Python-isms the language lacks get a pointer to what to write instead.
fn hint(message: &str) -> String {
    let first = message.lines().next().unwrap_or(message).trim().to_owned();
    let lower = first.to_lowercase();
    let advice = if lower.contains("while") {
        Some("there is no `while`; use `for _ in range(N):` with `break` (loops are bounded)")
    } else if lower.contains("try") || lower.contains("except") || lower.contains("raise") {
        Some("there is no try/except; failures are values (`r = h.result(); if r.ok: ...`)")
    } else if lower.contains("import") {
        Some("there are no imports; the host API is built in")
    } else if lower.contains("class") {
        Some("there are no classes; use dicts and functions")
    } else if lower.contains("load") {
        Some("`load` is disabled")
    } else if lower.contains("f-string") || lower.contains("fstring") {
        Some("f-strings only take bare names (`f\"{name}\"`); use \"a\" + b.c or \"%s\" % x")
    } else {
        None
    };
    match advice {
        Some(advice) => format!("{first} ({advice})"),
        None => first,
    }
}

// ── the walker ────────────────────────────────────────────────────────────

#[derive(Default)]
struct DefInfo {
    /// Line/col of the def.
    line: u32,
    col: u32,
    /// Bare-identifier calls and `pmap`/`parallel` function references.
    calls: BTreeSet<String>,
    /// Sites lexically inside the def (indices into `Analyzer::asks` / `runs`).
    asks: Vec<usize>,
    runs: Vec<usize>,
    /// Phases that appear inside the def, with the sites each claims
    /// lexically and the defs each claims by calling them.
    phases: Vec<usize>,
}

struct PhaseInfo {
    name: String,
    line: u32,
    col: u32,
    asks: Vec<usize>,
    runs: Vec<usize>,
    /// Defs called from the claimed region.
    calls: BTreeSet<String>,
}

struct AskInfo {
    graph: GraphAsk,
    in_loop: bool,
    def: Option<String>,
}

struct RunInfo {
    graph: GraphCommand,
    in_loop: bool,
    def: Option<String>,
}

struct ActorInfo {
    graph: GraphActor,
    in_loop: bool,
    def: Option<String>,
}

#[derive(Clone)]
struct Ctx {
    def: Option<String>,
    in_loop: bool,
    /// Stack of phase indices this region is claimed by (innermost last).
    phase: Option<usize>,
}

struct Analyzer<'a> {
    path: &'a str,
    ast: &'a AstModule,
    errors: Vec<Diagnostic>,
    warnings: Vec<Diagnostic>,
    defs: BTreeMap<String, DefInfo>,
    phases: Vec<PhaseInfo>,
    asks: Vec<AskInfo>,
    runs: Vec<RunInfo>,
    actors: Vec<ActorInfo>,
    sites: HashMap<String, SiteKind>,
    /// Defs passed to pmap/parallel: their bodies run once per item.
    fanned_defs: BTreeSet<String>,
    /// Defs called from a loop or comprehension.
    looped_calls: BTreeSet<String>,
    /// `phase()` statement spans already accounted for (so the expression
    /// visitor does not re-report them as misplaced).
    phase_stmts: HashSet<Span>,
    /// `pmap(f, ..)` / `parallel([f, ..])` references, checked once every def is known.
    fan_refs: Vec<(String, Span, String)>,
    has_main: bool,
}

impl<'a> Analyzer<'a> {
    fn new(path: &'a str, ast: &'a AstModule) -> Self {
        Self {
            path,
            ast,
            errors: Vec::new(),
            warnings: Vec::new(),
            defs: BTreeMap::new(),
            phases: Vec::new(),
            asks: Vec::new(),
            runs: Vec::new(),
            actors: Vec::new(),
            sites: HashMap::new(),
            fanned_defs: BTreeSet::new(),
            looped_calls: BTreeSet::new(),
            phase_stmts: HashSet::new(),
            fan_refs: Vec::new(),
            has_main: false,
        }
    }

    fn pos(&self, span: Span) -> (u32, u32) {
        let r = self.ast.file_span(span).resolve_span();
        (r.begin.line as u32 + 1, r.begin.column as u32 + 1)
    }

    fn site(&self, span: Span, in_loop: bool) -> GraphSite {
        let r = self.ast.file_span(span).resolve_span();
        GraphSite {
            site_id: site_of(&r),
            line: r.begin.line as u32 + 1,
            col: r.begin.column as u32 + 1,
            fan_out: in_loop,
        }
    }

    fn error(&mut self, span: Span, message: impl Into<String>) {
        let (line, col) = self.pos(span);
        self.errors
            .push(Diagnostic::new(self.path, line, col, message));
    }

    fn run(&mut self) {
        let top = self.ast.statement();
        let statements: Vec<&AstStmt> = match &top.node {
            StmtP::Statements(list) => list.iter().collect(),
            _ => vec![top],
        };
        for stmt in statements {
            match &stmt.node {
                StmtP::Def(def) => {
                    let name = def.name.node.ident.clone();
                    if name == "main" {
                        self.has_main = true;
                        let positional = def
                            .params
                            .iter()
                            .filter(|p| matches!(p.node, ParameterP::Normal(..)))
                            .count();
                        if positional != 1 {
                            self.error(
                                def.name.span,
                                "`main` must take exactly one parameter: `def main(args):`",
                            );
                        }
                    }
                    let (line, col) = self.pos(stmt.span);
                    self.defs.insert(
                        name.clone(),
                        DefInfo {
                            line,
                            col,
                            ..DefInfo::default()
                        },
                    );
                    let ctx = Ctx {
                        def: Some(name),
                        in_loop: false,
                        phase: None,
                    };
                    self.block(&def.body, &ctx);
                }
                StmtP::Assign(assign) => {
                    // Constants. Host calls here would run at load time.
                    let ctx = Ctx {
                        def: None,
                        in_loop: false,
                        phase: None,
                    };
                    self.expr(&assign.rhs, &ctx);
                }
                StmtP::Pass => {}
                StmtP::Expression(e) => {
                    self.error(
                        e.span,
                        "top-level expression statements are not allowed; put the work in `def main(args)`",
                    );
                }
                _ => self.error(
                    stmt.span,
                    "only `def` and constant assignments are allowed at the top level",
                ),
            }
        }
        if !self.has_main {
            self.errors.push(Diagnostic::new(
                self.path,
                1,
                1,
                "the script must define `def main(args):` returning the result",
            ));
        }
    }

    /// Walk a block, tracking which phase claims each statement.
    fn block(&mut self, stmt: &AstStmt, outer: &Ctx) {
        let list: Vec<&AstStmt> = match &stmt.node {
            StmtP::Statements(list) => list.iter().collect(),
            _ => vec![stmt],
        };
        let mut ctx = outer.clone();
        for s in list {
            if let Some((name, span)) = self.phase_statement(s) {
                let (line, col) = self.pos(span);
                let idx = self.phases.len();
                self.phases.push(PhaseInfo {
                    name: name.clone(),
                    line,
                    col,
                    asks: Vec::new(),
                    runs: Vec::new(),
                    calls: BTreeSet::new(),
                });
                if let Some(def) = ctx.def.as_ref().and_then(|d| self.defs.get_mut(d)) {
                    def.phases.push(idx);
                }
                let site = self.site(span, ctx.in_loop);
                self.sites.insert(site.site_id, SiteKind::Phase(name));
                ctx.phase = Some(idx);
                continue;
            }
            self.statement(s, &ctx);
        }
    }

    /// `phase("literal")` as a whole statement → its name and call span.
    fn phase_statement(&mut self, stmt: &AstStmt) -> Option<(String, Span)> {
        let StmtP::Expression(e) = &stmt.node else {
            return None;
        };
        let ExprP::Call(callee, args) = &e.node else {
            return None;
        };
        if !is_ident(callee, "phase") {
            return None;
        }
        self.phase_stmts.insert(e.span);
        match literal_first_arg(args) {
            Some(name) if !name.trim().is_empty() && args.args.len() == 1 => {
                Some((name.to_owned(), e.span))
            }
            _ => {
                self.error(
                    e.span,
                    "phase() takes exactly one string literal: phase(\"review\")",
                );
                // Still a phase boundary, so later asks are not reported as unphased.
                Some((String::from("(invalid)"), e.span))
            }
        }
    }

    fn statement(&mut self, stmt: &AstStmt, ctx: &Ctx) {
        match &stmt.node {
            StmtP::Statements(_) => self.block(stmt, ctx),
            StmtP::Expression(e) => self.expr(e, ctx),
            StmtP::Return(e) => {
                if let Some(e) = e {
                    self.expr(e, ctx);
                }
            }
            StmtP::Assign(a) => {
                self.target(&a.lhs, ctx);
                self.expr(&a.rhs, ctx);
            }
            StmtP::AssignModify(t, _, rhs) => {
                self.target(t, ctx);
                self.expr(rhs, ctx);
            }
            StmtP::If(cond, then) => {
                self.expr(cond, ctx);
                self.block(then, ctx);
            }
            StmtP::IfElse(cond, branches) => {
                self.expr(cond, ctx);
                self.block(&branches.0, ctx);
                self.block(&branches.1, ctx);
            }
            StmtP::For(f) => {
                self.expr(&f.over, ctx);
                let inner = Ctx {
                    in_loop: true,
                    ..ctx.clone()
                };
                self.block(&f.body, &inner);
            }
            StmtP::Def(def) => {
                // A nested def: its body belongs to the enclosing def's
                // analysis (it can call host functions when invoked there).
                self.block(&def.body, ctx);
            }
            StmtP::Break | StmtP::Continue | StmtP::Pass | StmtP::Load(_) => {}
        }
    }

    fn target(&mut self, target: &starlark::syntax::ast::AstAssignTarget, ctx: &Ctx) {
        match &target.node {
            AssignTargetP::Tuple(items) => {
                for t in items {
                    self.target(t, ctx);
                }
            }
            AssignTargetP::Index(pair) => {
                self.expr(&pair.0, ctx);
                self.expr(&pair.1, ctx);
            }
            AssignTargetP::Dot(e, _) => self.expr(e, ctx),
            AssignTargetP::Identifier(_) => {}
        }
    }

    fn note_call(&mut self, name: &str, ctx: &Ctx) {
        if let Some(def) = ctx.def.as_ref().and_then(|d| self.defs.get_mut(d)) {
            def.calls.insert(name.to_owned());
        }
        if let Some(p) = ctx.phase {
            self.phases[p].calls.insert(name.to_owned());
        }
        if ctx.in_loop {
            self.looped_calls.insert(name.to_owned());
        }
    }

    fn expr(&mut self, e: &AstExpr, ctx: &Ctx) {
        match &e.node {
            ExprP::Call(callee, args) => {
                self.call(e, callee, args, ctx);
                self.expr(callee, ctx);
                for a in &args.args {
                    self.expr(a.node.expr(), ctx);
                }
            }
            ExprP::Tuple(items) | ExprP::List(items) => {
                for i in items {
                    self.expr(i, ctx);
                }
            }
            ExprP::Dot(inner, _) => self.expr(inner, ctx),
            ExprP::Index(pair) => {
                self.expr(&pair.0, ctx);
                self.expr(&pair.1, ctx);
            }
            ExprP::Index2(t) => {
                self.expr(&t.0, ctx);
                self.expr(&t.1, ctx);
                self.expr(&t.2, ctx);
            }
            ExprP::Slice(a, b, c, d) => {
                self.expr(a, ctx);
                for x in [b, c, d].into_iter().flatten() {
                    self.expr(x, ctx);
                }
            }
            ExprP::Identifier(_) | ExprP::Literal(_) => {}
            ExprP::Lambda(l) => {
                let inner = Ctx {
                    in_loop: true,
                    ..ctx.clone()
                };
                self.expr(&l.body, &inner);
            }
            ExprP::Not(x) | ExprP::Minus(x) | ExprP::Plus(x) | ExprP::BitNot(x) => {
                self.expr(x, ctx)
            }
            ExprP::Op(a, _, b) => {
                self.expr(a, ctx);
                self.expr(b, ctx);
            }
            ExprP::If(t) => {
                self.expr(&t.0, ctx);
                self.expr(&t.1, ctx);
                self.expr(&t.2, ctx);
            }
            ExprP::Dict(pairs) => {
                for (k, v) in pairs {
                    self.expr(k, ctx);
                    self.expr(v, ctx);
                }
            }
            ExprP::ListComprehension(body, first, rest) => {
                self.comprehension(&[body], first, rest, ctx)
            }
            ExprP::DictComprehension(pair, first, rest) => {
                self.comprehension(&[&pair.0, &pair.1], first, rest, ctx)
            }
            ExprP::FString(f) => {
                for x in &f.expressions {
                    self.expr(x, ctx);
                }
            }
        }
    }

    fn comprehension(
        &mut self,
        bodies: &[&AstExpr],
        first: &starlark::syntax::ast::ForClause,
        rest: &[starlark::syntax::ast::Clause],
        ctx: &Ctx,
    ) {
        // The first iterable is evaluated once; everything else per element.
        self.expr(&first.over, ctx);
        let inner = Ctx {
            in_loop: true,
            ..ctx.clone()
        };
        for clause in rest {
            match clause {
                ClauseP::For(f) => self.expr(&f.over, &inner),
                ClauseP::If(c) => self.expr(c, &inner),
            }
        }
        for b in bodies {
            self.expr(b, &inner);
        }
    }

    fn call(
        &mut self,
        call: &AstExpr,
        callee: &AstExpr,
        args: &CallArgsP<starlark::syntax::ast::AstNoPayload>,
        ctx: &Ctx,
    ) {
        match &callee.node {
            ExprP::Identifier(id) => {
                let name = id.node.ident.as_str();
                self.note_call(name, ctx);
                match name {
                    "phase" => {
                        if !self.phase_stmts.contains(&call.span) {
                            self.error(
                                call.span,
                                "phase() must be a statement of its own (`phase(\"name\")`), not part of an expression",
                            );
                        }
                    }
                    "run" => self.run_call(call, args, ctx),
                    "agent" => self.agent_call(call, args, ctx),
                    "pmap" | "parallel" => self.fan_call(call, name, args, ctx),
                    _ => {}
                }
                if ctx.def.is_none() && is_host_effect(name) {
                    self.error(
                        call.span,
                        format!("{name}() cannot run at module level; call it from `def main(args)` or a helper def"),
                    );
                }
            }
            ExprP::Dot(receiver, method) => {
                let m = method.node.as_str();
                if m == "ask" {
                    self.ask_call(call, receiver, args, ctx);
                } else if let ExprP::Identifier(id) = &receiver.node
                    && id.node.ident == "artifact"
                {
                    self.artifact_call(call, m, args, ctx);
                }
                let host_receiver = matches!(
                    &receiver.node,
                    ExprP::Identifier(id) if matches!(id.node.ident.as_str(), "files" | "git" | "artifact")
                );
                if ctx.def.is_none() && (m == "ask" || host_receiver) {
                    self.error(call.span, "host calls cannot run at module level; call them from `def main(args)` or a helper def");
                }
            }
            _ => {}
        }
    }

    fn ask_call(
        &mut self,
        call: &AstExpr,
        receiver: &AstExpr,
        args: &CallArgsP<starlark::syntax::ast::AstNoPayload>,
        ctx: &Ctx,
    ) {
        let actor = match &receiver.node {
            ExprP::Identifier(id) => Some(id.node.ident.clone()),
            _ => None,
        };
        let head = literal_arg(args, 0, "instructions").map(|s| head_chars(s, 120));
        let typed = named_arg(args, "schema").is_some()
            || args.args.len() > 1 && positional(args, 1).is_some();
        let read_only = matches!(
            named_arg(args, "read_only").map(|e| &e.node),
            Some(ExprP::Identifier(id)) if id.node.ident == "True"
        );
        let site = self.site(call.span, ctx.in_loop);
        let idx = self.asks.len();
        self.asks.push(AskInfo {
            graph: GraphAsk {
                site,
                actor,
                instructions_head: head,
                typed,
                read_only,
            },
            in_loop: ctx.in_loop,
            def: ctx.def.clone(),
        });
        if let Some(def) = ctx.def.as_ref().and_then(|d| self.defs.get_mut(d)) {
            def.asks.push(idx);
        }
        if let Some(p) = ctx.phase {
            self.phases[p].asks.push(idx);
        }
    }

    fn run_call(
        &mut self,
        call: &AstExpr,
        args: &CallArgsP<starlark::syntax::ast::AstNoPayload>,
        ctx: &Ctx,
    ) {
        let Some(program) = literal_arg(args, 0, "cmd") else {
            self.error(
                call.span,
                "run() needs the command as a string literal (shown to the user at approval): run(\"cargo\", [\"test\"])",
            );
            return;
        };
        let program = program.to_owned();
        let literal_args = match positional(args, 1).or_else(|| named_arg(args, "args")) {
            None => Some(Vec::new()),
            Some(e) => literal_string_list(e),
        };
        let base = program
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(&program)
            .to_lowercase();
        if literal_args.is_none() && INTERPRETERS.contains(&base.as_str()) {
            self.error(
                call.span,
                format!(
                    "run(\"{program}\", ...) with computed arguments would let the script run arbitrary code; pass a literal list of strings, or run the tool you mean directly"
                ),
            );
        }
        let site = self.site(call.span, ctx.in_loop);
        self.sites.insert(
            site.site_id.clone(),
            SiteKind::Run {
                program: program.clone(),
            },
        );
        let idx = self.runs.len();
        self.runs.push(RunInfo {
            graph: GraphCommand {
                site,
                command: program,
                args: literal_args,
            },
            in_loop: ctx.in_loop,
            def: ctx.def.clone(),
        });
        if let Some(def) = ctx.def.as_ref().and_then(|d| self.defs.get_mut(d)) {
            def.runs.push(idx);
        }
        if let Some(p) = ctx.phase {
            self.phases[p].runs.push(idx);
        }
    }

    fn agent_call(
        &mut self,
        call: &AstExpr,
        args: &CallArgsP<starlark::syntax::ast::AstNoPayload>,
        ctx: &Ctx,
    ) {
        let site = self.site(call.span, ctx.in_loop);
        self.actors.push(ActorInfo {
            graph: GraphActor {
                site,
                name: literal_arg(args, 0, "name").map(str::to_owned),
                harness: literal_arg(args, usize::MAX, "harness").map(str::to_owned),
                model: literal_arg(args, usize::MAX, "model").map(str::to_owned),
            },
            in_loop: ctx.in_loop,
            def: ctx.def.clone(),
        });
    }

    fn artifact_call(
        &mut self,
        call: &AstExpr,
        method: &str,
        args: &CallArgsP<starlark::syntax::ast::AstNoPayload>,
        ctx: &Ctx,
    ) {
        if !matches!(method, "markdown" | "table" | "metrics" | "file") {
            return; // an unknown attribute fails at run time with its own message
        }
        let Some(id) = literal_arg(args, 0, "id") else {
            self.error(
                call.span,
                format!("artifact.{method}() needs its id as a string literal"),
            );
            return;
        };
        let site = self.site(call.span, ctx.in_loop);
        self.sites
            .insert(site.site_id, SiteKind::Artifact { id: id.to_owned() });
    }

    fn fan_call(
        &mut self,
        call: &AstExpr,
        name: &str,
        args: &CallArgsP<starlark::syntax::ast::AstNoPayload>,
        ctx: &Ctx,
    ) {
        let functions: Vec<&AstExpr> = if name == "pmap" {
            positional(args, 0)
                .or_else(|| named_arg(args, "fn"))
                .into_iter()
                .collect()
        } else {
            match positional(args, 0)
                .or_else(|| named_arg(args, "fns"))
                .map(|e| &e.node)
            {
                Some(ExprP::List(items)) | Some(ExprP::Tuple(items)) => items.iter().collect(),
                _ => {
                    self.error(
                        call.span,
                        "parallel() takes a list literal of top-level def names: parallel([a, b])",
                    );
                    return;
                }
            }
        };
        if functions.is_empty() {
            self.error(call.span, format!("{name}() needs a function"));
        }
        for f in functions {
            match &f.node {
                ExprP::Identifier(id) => {
                    // Whether the name is a top-level def is checked once all
                    // defs are known (`finish`).
                    self.fanned_defs.insert(id.node.ident.clone());
                    self.note_call(&id.node.ident, ctx);
                    self.fan_refs.push((id.node.ident.clone(), f.span, name.to_owned()));
                }
                ExprP::Lambda(_) => self.error(
                    f.span,
                    format!("{name}() cannot take a lambda: it runs on worker threads, which only top-level `def`s can cross; name the function with a top-level def"),
                ),
                _ => self.error(
                    f.span,
                    format!("{name}() needs the name of a top-level def"),
                ),
            }
        }
    }
}

impl Analyzer<'_> {
    /// Names reachable from `start` through the def call graph (inclusive).
    fn reach(&self, start: &BTreeSet<String>) -> BTreeSet<String> {
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut stack: Vec<String> = start.iter().cloned().collect();
        while let Some(name) = stack.pop() {
            if !self.defs.contains_key(&name) || !seen.insert(name.clone()) {
                continue;
            }
            stack.extend(self.defs[&name].calls.iter().cloned());
        }
        seen
    }

    fn check_recursion(&mut self) {
        // Depth-first search for a cycle among top-level defs.
        fn visit(
            name: &str,
            defs: &BTreeMap<String, DefInfo>,
            path: &mut Vec<String>,
            done: &mut HashSet<String>,
            found: &mut Vec<Vec<String>>,
        ) {
            if let Some(at) = path.iter().position(|p| p == name) {
                found.push(path[at..].to_vec());
                return;
            }
            if done.contains(name) {
                return;
            }
            path.push(name.to_owned());
            if let Some(def) = defs.get(name) {
                for callee in &def.calls {
                    if defs.contains_key(callee) {
                        visit(callee, defs, path, done, found);
                    }
                }
            }
            path.pop();
            done.insert(name.to_owned());
        }
        let mut found = Vec::new();
        let mut done = HashSet::new();
        for name in self.defs.keys() {
            visit(name, &self.defs, &mut Vec::new(), &mut done, &mut found);
        }
        for cycle in found {
            let head = &self.defs[&cycle[0]];
            let chain = cycle
                .iter()
                .chain(std::iter::once(&cycle[0]))
                .cloned()
                .collect::<Vec<_>>()
                .join(" -> ");
            self.errors.push(Diagnostic::new(
                self.path,
                head.line,
                head.col,
                format!("recursion is not allowed ({chain}); express repetition with a bounded `for` loop"),
            ));
        }
    }

    fn finish(mut self) -> Result<Analysis, Vec<Diagnostic>> {
        self.check_recursion();
        for (name, span, via) in std::mem::take(&mut self.fan_refs) {
            if !self.defs.contains_key(&name) {
                self.error(
                    span,
                    format!("{via}() needs a top-level def, and `{name}` is not one (closures and lambdas cannot cross worker threads)"),
                );
            }
        }
        // Names the language does not define, reported where they are used.
        let known = crate::globals::names();
        for lint in self.ast.lint(Some(&known)) {
            let r = lint.location.resolve_span();
            let (line, col) = (r.begin.line as u32 + 1, r.begin.column as u32 + 1);
            if lint.short_name == "using-undefined" {
                self.errors.push(Diagnostic::new(
                    self.path,
                    line,
                    col,
                    format!("{} (scripts have no clock, randomness, files or environment beyond the host API)", lint.problem),
                ));
            }
        }

        // Fan-out: defs called from a loop or comprehension, passed to
        // pmap/parallel, or called by such a def.
        let seeds: BTreeSet<String> = self
            .fanned_defs
            .iter()
            .chain(self.looped_calls.iter())
            .cloned()
            .collect();
        let fan_defs = self.reach(&seeds);

        // Which sites does each phase own: its claimed region's, plus those
        // of the defs the region calls.
        let mut phase_asks: Vec<BTreeSet<usize>> = Vec::new();
        let mut phase_runs: Vec<BTreeSet<usize>> = Vec::new();
        for phase in &self.phases {
            let mut asks: BTreeSet<usize> = phase.asks.iter().copied().collect();
            let mut runs: BTreeSet<usize> = phase.runs.iter().copied().collect();
            for def in self.reach(&phase.calls) {
                asks.extend(self.defs[&def].asks.iter().copied());
                runs.extend(self.defs[&def].runs.iter().copied());
            }
            phase_asks.push(asks);
            phase_runs.push(runs);
        }
        for (i, phase) in self.phases.iter().enumerate() {
            if phase.name != "(invalid)" && phase_asks[i].is_empty() && phase_runs[i].is_empty() {
                self.errors.push(Diagnostic::new(
                    self.path,
                    phase.line,
                    phase.col,
                    format!(
                        "phase \"{}\" contains no ask() or run(); a phase claims the rest of its block and must do work in it",
                        phase.name
                    ),
                ));
            }
        }
        if !self.errors.is_empty() {
            let mut errors = self.errors;
            errors.sort_by_key(|d| (d.line, d.col));
            errors.dedup();
            return Err(errors);
        }

        let fan = |in_loop: bool, def: &Option<String>| {
            in_loop || def.as_ref().is_some_and(|d| fan_defs.contains(d))
        };
        let mut graph = WorkflowGraph::default();
        let mut owned_asks: HashSet<usize> = HashSet::new();
        for (i, phase) in self.phases.iter().enumerate() {
            let mut asks: Vec<GraphAsk> = phase_asks[i]
                .iter()
                .map(|&a| {
                    let info = &self.asks[a];
                    let mut g = info.graph.clone();
                    g.site.fan_out = fan(info.in_loop, &info.def);
                    g
                })
                .collect();
            let mut commands: Vec<GraphCommand> = phase_runs[i]
                .iter()
                .map(|&r| {
                    let info = &self.runs[r];
                    let mut g = info.graph.clone();
                    g.site.fan_out = fan(info.in_loop, &info.def);
                    g
                })
                .collect();
            owned_asks.extend(phase_asks[i].iter().copied());
            asks.sort_by_key(|a| (a.site.line, a.site.col));
            commands.sort_by_key(|c| (c.site.line, c.site.col));
            match graph.phases.iter_mut().find(|p| p.name == phase.name) {
                Some(existing) => {
                    for a in asks {
                        if !existing
                            .asks
                            .iter()
                            .any(|x| x.site.site_id == a.site.site_id)
                        {
                            existing.asks.push(a);
                        }
                    }
                    for c in commands {
                        if !existing
                            .commands
                            .iter()
                            .any(|x| x.site.site_id == c.site.site_id)
                        {
                            existing.commands.push(c);
                        }
                    }
                }
                None => graph.phases.push(GraphPhase {
                    name: phase.name.clone(),
                    line: phase.line,
                    col: phase.col,
                    asks,
                    commands,
                }),
            }
        }
        graph.actors = self
            .actors
            .iter()
            .map(|a| {
                let mut g = a.graph.clone();
                g.site.fan_out = fan(a.in_loop, &a.def);
                g
            })
            .collect();
        graph.actors.sort_by_key(|a| (a.site.line, a.site.col));
        graph.commands = self
            .runs
            .iter()
            .map(|r| {
                let mut g = r.graph.clone();
                g.site.fan_out = fan(r.in_loop, &r.def);
                g
            })
            .collect();
        graph.commands.sort_by_key(|c| (c.site.line, c.site.col));
        graph.unphased_asks = (0..self.asks.len())
            .filter(|a| !owned_asks.contains(a))
            .count() as u32;

        let mut warnings = self.warnings;
        if graph.unphased_asks > 0 && !graph.phases.is_empty() {
            warnings.push(Diagnostic::unpositioned(
                self.path,
                format!(
                    "{} ask(s) run outside any phase(); the card cannot place them",
                    graph.unphased_asks
                ),
            ));
        } else if graph.phases.is_empty() && !self.asks.is_empty() {
            warnings.push(Diagnostic::unpositioned(
                self.path,
                "the script has no phase(\"...\") markers; progress will not be grouped",
            ));
        }
        Ok(Analysis {
            graph,
            sites: CallSites { sites: self.sites },
            warnings,
        })
    }
}

fn is_host_effect(name: &str) -> bool {
    matches!(
        name,
        "phase" | "run" | "agent" | "pmap" | "parallel" | "wait_all" | "results" | "log" | "report"
    )
}

fn is_ident(e: &AstExpr, name: &str) -> bool {
    matches!(&e.node, ExprP::Identifier(id) if id.node.ident == name)
}

fn positional(
    args: &CallArgsP<starlark::syntax::ast::AstNoPayload>,
    index: usize,
) -> Option<&AstExpr> {
    args.args
        .iter()
        .filter_map(|a| match &a.node {
            ArgumentP::Positional(e) => Some(e),
            _ => None,
        })
        .nth(index)
}

fn named_arg<'a>(
    args: &'a CallArgsP<starlark::syntax::ast::AstNoPayload>,
    name: &str,
) -> Option<&'a AstExpr> {
    args.args.iter().find_map(|a| match &a.node {
        ArgumentP::Named(n, e) if n.node == name => Some(e),
        _ => None,
    })
}

fn string_literal(e: &AstExpr) -> Option<&str> {
    match &e.node {
        ExprP::Literal(AstLiteral::String(s)) => Some(s.node.as_str()),
        _ => None,
    }
}

/// The first positional argument as a string literal.
fn literal_first_arg(args: &CallArgsP<starlark::syntax::ast::AstNoPayload>) -> Option<&str> {
    positional(args, 0).and_then(string_literal)
}

/// A string literal passed positionally at `index` or by keyword `name`.
fn literal_arg<'a>(
    args: &'a CallArgsP<starlark::syntax::ast::AstNoPayload>,
    index: usize,
    name: &str,
) -> Option<&'a str> {
    positional(args, index)
        .or_else(|| named_arg(args, name))
        .and_then(string_literal)
}

/// `["a", "b"]` (or a tuple) of string literals.
fn literal_string_list(e: &AstExpr) -> Option<Vec<String>> {
    match &e.node {
        ExprP::List(items) | ExprP::Tuple(items) => items
            .iter()
            .map(|i| string_literal(i).map(str::to_owned))
            .collect(),
        _ => None,
    }
}

fn head_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{cut}…")
    }
}
