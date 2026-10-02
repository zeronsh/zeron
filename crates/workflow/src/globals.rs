//! The script's world: the builtins a workflow can call and the values they
//! hand back (`Actor`, `Handle`, result structs).
//!
//! Everything effectful goes through [`Host`](crate::host::Host) and is keyed
//! by call site (see [`crate::site`]). Failures of the world are *values*
//! (Starlark has no `try`): `Handle.result()` returns a struct with `ok`,
//! `value`, `error`; `run()` returns the exit code instead of raising.

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use allocative::Allocative;
use serde_json::{Map, Value as Json, json};
use starlark::any::ProvidesStaticType;
use starlark::environment::{
    Globals, GlobalsBuilder, LibraryExtension, Methods, MethodsBuilder, MethodsStatic, Module,
};
use starlark::eval::Evaluator;
use starlark::values::list::UnpackList;
use starlark::values::none::{NoneOr, NoneType};
use starlark::values::structs::AllocStruct;
use starlark::values::{
    FrozenValue, Heap, NoSerialize, StarlarkPagablePanic, StarlarkValue, Value, ValueLike,
};
use starlark::{starlark_module, starlark_simple_value};
use starlark_derive::starlark_value;

use crate::analysis::{CallSites, SiteKind};
use crate::host::{
    ActorSpec, ArtifactContent, ArtifactRequest, AskReply, AskRequest, Completion, Host, Metric,
    ReadOp, RunReply, RunRequest, Wait,
};
use crate::limits::*;
use crate::site::{Ordinals, SiteKey, site_of};

/// Why evaluation stopped for a reason other than a script bug.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Abort {
    #[error("the workflow was cancelled")]
    Cancelled,
    #[error("{0}")]
    Limit(String),
}

/// State shared by the main thread and every `pmap` worker of one run.
pub(crate) struct Shared {
    pub host: Arc<dyn Host>,
    pub cancel: Arc<AtomicBool>,
    pub limits: Limits,
    pub sites: CallSites,
    reports: AtomicUsize,
    logs: AtomicUsize,
    /// artifact id → versions published so far.
    artifacts: Mutex<HashMap<String, u32>>,
}

impl Shared {
    pub fn new(
        host: Arc<dyn Host>,
        cancel: Arc<AtomicBool>,
        limits: Limits,
        sites: CallSites,
    ) -> Self {
        Self {
            host,
            cancel,
            limits,
            sites,
            reports: AtomicUsize::new(0),
            logs: AtomicUsize::new(0),
            artifacts: Mutex::new(HashMap::new()),
        }
    }
}

/// Per-evaluator state, reached through `Evaluator::extra`.
#[derive(ProvidesStaticType)]
pub(crate) struct RunCtx {
    pub shared: Arc<Shared>,
    pub ordinals: Ordinals,
    /// Inside a `pmap` / `parallel` worker.
    pub worker: bool,
    /// While the module's top level runs (before `main`): no host effects.
    pub loading: AtomicBool,
    started: Instant,
    /// Time this evaluator spent blocked on the host (not compute).
    blocked: Mutex<Duration>,
}

impl RunCtx {
    pub fn new(shared: Arc<Shared>, ordinals: Ordinals, worker: bool) -> Self {
        Self {
            shared,
            ordinals,
            worker,
            loading: AtomicBool::new(false),
            started: Instant::now(),
            blocked: Mutex::new(Duration::ZERO),
        }
    }

    /// Cancelled by the caller, or out of compute budget.
    pub fn should_stop(&self) -> bool {
        if self.shared.cancel.load(Ordering::Relaxed) {
            return true;
        }
        let blocked = *self.blocked.lock().unwrap_or_else(|e| e.into_inner());
        self.started.elapsed().saturating_sub(blocked) > self.shared.limits.compute
    }

    pub fn compute_exhausted(&self) -> bool {
        !self.shared.cancel.load(Ordering::Relaxed) && self.should_stop()
    }

    fn wait<T: Clone>(&self, completion: &Completion<T>) -> Result<Option<T>, Abort> {
        let mut blocked = Duration::ZERO;
        let out = completion.wait(&self.shared.cancel, &mut blocked);
        *self.blocked.lock().unwrap_or_else(|e| e.into_inner()) += blocked;
        match out {
            Wait::Ready(v) => Ok(Some(v)),
            Wait::Abandoned => Ok(None),
            Wait::Cancelled => Err(Abort::Cancelled),
        }
    }
}

fn ctx<'a>(eval: &Evaluator<'_, 'a, '_>) -> anyhow::Result<&'a RunCtx> {
    eval.extra
        .and_then(|e| e.downcast_ref::<RunCtx>())
        .ok_or_else(|| anyhow::anyhow!("internal error: no run context"))
}

/// Configure an evaluator's limits and cancellation.
pub(crate) fn configure<'v, 'a>(
    ev: &mut Evaluator<'v, 'a, '_>,
    ctx: &'a RunCtx,
) -> anyhow::Result<()> {
    let limits = &ctx.shared.limits;
    ev.set_check_cancelled(Box::new(move || ctx.should_stop()));
    ev.set_max_callstack_size(limits.callstack)?;
    ev.set_max_tick_count(limits.ticks)?;
    ev.set_max_heap_size(limits.heap_bytes)?;
    Ok(())
}

fn call_site(eval: &Evaluator<'_, '_, '_>) -> anyhow::Result<String> {
    eval.call_stack_top_location()
        .map(|loc| site_of(&loc.resolve_span()))
        .ok_or_else(|| anyhow::anyhow!("internal error: cannot locate the call"))
}

/// Check the call may run now and mint its key.
fn begin<'a>(
    eval: &Evaluator<'_, 'a, '_>,
    what: &str,
) -> anyhow::Result<(SiteKey, &'a RunCtx, String)> {
    let c = ctx(eval)?;
    if c.loading.load(Ordering::Relaxed) {
        anyhow::bail!("{what}() cannot run at module level; call it from `def main(args)`");
    }
    let site = call_site(eval)?;
    let key = c.ordinals.next(&site);
    Ok((key, c, site))
}

// ── values ────────────────────────────────────────────────────────────────

/// One persistent child chat (created at its first ask).
#[derive(Debug, ProvidesStaticType, NoSerialize, Allocative, StarlarkPagablePanic)]
pub(crate) struct Actor {
    #[allocative(skip)]
    key: SiteKey,
    name: String,
}

impl fmt::Display for Actor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<actor {}>", self.name)
    }
}
starlark_simple_value!(Actor);

#[starlark_value(type = "Actor")]
impl<'v> StarlarkValue<'v> for Actor {
    fn get_methods() -> Option<&'static Methods> {
        static M: MethodsStatic = MethodsStatic::new("Actor", actor_methods);
        Some(M.methods())
    }

    fn get_attr(&self, attribute: &str, heap: Heap<'v>) -> Option<Value<'v>> {
        (attribute == "name").then(|| heap.alloc(self.name.as_str()))
    }

    fn dir_attr(&self) -> Vec<String> {
        vec!["name".into(), "ask".into()]
    }
}

/// An ask in flight. `.result()` joins it.
#[derive(Debug, ProvidesStaticType, NoSerialize, Allocative, StarlarkPagablePanic)]
pub(crate) struct Handle {
    #[allocative(skip)]
    completion: Completion<AskReply>,
}

impl fmt::Display for Handle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<handle>")
    }
}
starlark_simple_value!(Handle);

impl fmt::Debug for Completion<AskReply> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Completion")
    }
}

#[starlark_value(type = "Handle")]
impl<'v> StarlarkValue<'v> for Handle {
    fn get_methods() -> Option<&'static Methods> {
        static M: MethodsStatic = MethodsStatic::new("Handle", handle_methods);
        Some(M.methods())
    }
}

fn ask_struct<'v>(heap: Heap<'v>, reply: &AskReply) -> Value<'v> {
    heap.alloc(AllocStruct([
        ("ok", heap.alloc(reply.ok)),
        ("value", heap.alloc(&reply.value)),
        (
            "error",
            match &reply.error {
                Some(e) => heap.alloc(e.as_str()),
                None => Value::new_none(),
            },
        ),
        ("cached", heap.alloc(reply.cached)),
        ("tokens", heap.alloc(reply.tokens as i64)),
    ]))
}

fn join<'v>(eval: &Evaluator<'v, '_, '_>, handle: &Handle) -> anyhow::Result<AskReply> {
    let c = ctx(eval)?;
    match c.wait(&handle.completion)? {
        Some(reply) => Ok(reply),
        // The host dropped the ask (the run is being torn down): a failure
        // value; a cancelled run never gets here (`wait` raised).
        None => Ok(AskReply::failed("the ask was dropped by the engine")),
    }
}

#[starlark_module]
fn handle_methods(b: &mut MethodsBuilder) {
    /// Block until the ask settles. Returns a struct `ok`, `value`, `error`,
    /// `cached`, `tokens`; asking again returns the same result.
    fn result<'v>(this: &Handle, eval: &mut Evaluator<'v, '_, '_>) -> anyhow::Result<Value<'v>> {
        let reply = join(eval, this)?;
        Ok(ask_struct(eval.heap(), &reply))
    }
}

#[starlark_module]
fn actor_methods(b: &mut MethodsBuilder) {
    /// `actor.ask(instructions, schema=None, read_only=False, timeout_s=None)`:
    /// dispatch a prompt to the actor's child chat and return at once.
    fn ask<'v>(
        this: &Actor,
        instructions: &str,
        #[starlark(default = NoneOr::None)] schema: NoneOr<Value<'v>>,
        #[starlark(require = named, default = false)] read_only: bool,
        #[starlark(require = named, default = NoneOr::None)] timeout_s: NoneOr<i32>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Handle> {
        let (key, c, _) = begin(eval, "ask")?;
        if instructions.trim().is_empty() {
            anyhow::bail!("ask() needs non-empty instructions");
        }
        if instructions.len() > MAX_INSTRUCTIONS_BYTES {
            anyhow::bail!(
                "ask() instructions are {} bytes; the limit is {MAX_INSTRUCTIONS_BYTES}",
                instructions.len()
            );
        }
        let schema = match schema {
            NoneOr::None => None,
            NoneOr::Other(v) => {
                let json = v.to_json_value()?;
                if !json.is_object() {
                    anyhow::bail!(
                        "ask(schema=...) must be a schema built with schema.obj/str/list/... (a dict)"
                    );
                }
                Some(json)
            }
        };
        let timeout_s = match timeout_s {
            NoneOr::None => None,
            NoneOr::Other(s) if (1..=86_400).contains(&s) => Some(s as u64),
            NoneOr::Other(s) => anyhow::bail!("timeout_s must be between 1 and 86400, got {s}"),
        };
        let completion = c.shared.host.ask(AskRequest {
            key,
            actor: this.key.clone(),
            instructions: instructions.to_owned(),
            schema,
            read_only,
            timeout_s,
        });
        Ok(Handle { completion })
    }
}

// ── builtins ──────────────────────────────────────────────────────────────

fn opt_string(v: NoneOr<&str>) -> Option<String> {
    match v {
        NoneOr::None => None,
        NoneOr::Other(s) => Some(s.to_owned()),
    }
}

fn cap_bytes(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[starlark_module]
fn host_fns(b: &mut GlobalsBuilder) {
    /// `phase("name")`: the work after this statement, up to the next
    /// `phase()` in the same block, belongs to the named phase.
    fn phase(name: &str, eval: &mut Evaluator<'_, '_, '_>) -> anyhow::Result<NoneType> {
        let (_, c, site) = begin(eval, "phase")?;
        if c.worker {
            anyhow::bail!(
                "phase() cannot be called from a pmap/parallel function; call it in main"
            );
        }
        match c.shared.sites.get(&site) {
            Some(SiteKind::Phase(n)) if n == name => {}
            _ => anyhow::bail!("phase() must be called directly with a string literal"),
        }
        c.shared.host.enter_phase(name);
        Ok(NoneType)
    }

    /// `agent(name, persona=None, harness=None, model=None, reasoning=None)`
    fn agent<'v>(
        name: &str,
        #[starlark(default = NoneOr::None)] persona: NoneOr<&str>,
        #[starlark(default = NoneOr::None)] harness: NoneOr<&str>,
        #[starlark(default = NoneOr::None)] model: NoneOr<&str>,
        #[starlark(default = NoneOr::None)] reasoning: NoneOr<&str>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Actor> {
        let (key, c, _) = begin(eval, "agent")?;
        if name.trim().is_empty() || name.len() > 80 {
            anyhow::bail!("agent() needs a name of 1 to 80 characters");
        }
        let persona = opt_string(persona);
        if persona.as_ref().is_some_and(|p| p.len() > 16 * 1024) {
            anyhow::bail!("agent(persona=...) is limited to 16384 bytes");
        }
        c.shared.host.create_actor(
            &key,
            &ActorSpec {
                name: name.to_owned(),
                persona,
                harness: opt_string(harness),
                model: opt_string(model),
                reasoning: opt_string(reasoning),
            },
        );
        Ok(Actor {
            key,
            name: name.to_owned(),
        })
    }

    /// `run(cmd, args=[], timeout_s=300, cwd=None)`: run a program (no shell)
    /// and return `exit_code`, `stdout`, `stderr`, `timed_out`, `ok`.
    fn run<'v>(
        cmd: &str,
        #[starlark(default = UnpackList::default())] args: UnpackList<String>,
        #[starlark(require = named, default = NoneOr::None)] timeout_s: NoneOr<i32>,
        #[starlark(require = named, default = NoneOr::None)] cwd: NoneOr<&str>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        let (key, c, site) = begin(eval, "run")?;
        match c.shared.sites.get(&site) {
            Some(SiteKind::Run { program }) if program == cmd => {}
            _ => anyhow::bail!(
                "run() must be called directly with the command as a string literal: run(\"cargo\", [\"test\"])"
            ),
        }
        let timeout_s = match timeout_s {
            NoneOr::None => DEFAULT_RUN_TIMEOUT_S,
            NoneOr::Other(s) if (1..=MAX_RUN_TIMEOUT_S as i32).contains(&s) => s as u64,
            NoneOr::Other(s) => {
                anyhow::bail!("timeout_s must be between 1 and {MAX_RUN_TIMEOUT_S}, got {s}")
            }
        };
        let cwd = opt_string(cwd);
        if let Some(cwd) = &cwd {
            check_relative("cwd", cwd)?;
        }
        let completion = c.shared.host.run(RunRequest {
            key,
            program: cmd.to_owned(),
            args: args.items,
            cwd,
            timeout_s,
        });
        let reply = match c.wait(&completion)? {
            Some(r) => r,
            None => RunReply {
                start_error: Some("the command was dropped by the engine".into()),
                ..RunReply::default()
            },
        };
        Ok(run_struct(eval.heap(), &reply))
    }

    /// `wait_all(handles)`: block until every handle settles; the list of
    /// result structs, in order.
    fn wait_all<'v>(
        handles: UnpackList<Value<'v>>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        let mut out = Vec::with_capacity(handles.items.len());
        for v in handles.items {
            let handle = v
                .downcast_ref::<Handle>()
                .ok_or_else(|| anyhow::anyhow!("wait_all() takes a list of handles from ask()"))?;
            let reply = join(eval, handle)?;
            out.push(ask_struct(eval.heap(), &reply));
        }
        Ok(eval.heap().alloc(out))
    }

    /// `results(handles)`: wait for all and return the `.value` of the ones
    /// that succeeded (failed asks are skipped).
    fn results<'v>(
        handles: UnpackList<Value<'v>>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        let mut out = Vec::new();
        for v in handles.items {
            let handle = v
                .downcast_ref::<Handle>()
                .ok_or_else(|| anyhow::anyhow!("results() takes a list of handles from ask()"))?;
            let reply = join(eval, handle)?;
            if reply.ok {
                out.push(eval.heap().alloc(&reply.value));
            }
        }
        Ok(eval.heap().alloc(out))
    }

    /// `log(message)`: a line in the run's log.
    fn log<'v>(message: Value<'v>, eval: &mut Evaluator<'v, '_, '_>) -> anyhow::Result<NoneType> {
        let (_, c, _) = begin(eval, "log")?;
        if c.shared.logs.fetch_add(1, Ordering::Relaxed) >= MAX_LOG_LINES {
            return Ok(NoneType); // past the cap: dropped, not an error
        }
        let text = message.to_str();
        c.shared.host.log(cap_bytes(&text, MAX_LOG_LINE_BYTES));
        Ok(NoneType)
    }

    /// `report(item, artifact_id=None)`: record a finding as you go; it
    /// survives a failed run.
    fn report<'v>(
        item: Value<'v>,
        #[starlark(default = NoneOr::None)] artifact_id: NoneOr<&str>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<NoneType> {
        let (_, c, _) = begin(eval, "report")?;
        let json = item.to_json_value()?;
        let size = json.to_string().len();
        if size > MAX_REPORT_BYTES {
            anyhow::bail!(
                "report() items are limited to {MAX_REPORT_BYTES} bytes; this one is {size}"
            );
        }
        if c.shared.reports.fetch_add(1, Ordering::Relaxed) >= MAX_REPORTS {
            anyhow::bail!("a run may report at most {MAX_REPORTS} items");
        }
        c.shared
            .host
            .report(json, opt_string(artifact_id))
            .map_err(|e| anyhow::anyhow!("report(): {e}"))?;
        Ok(NoneType)
    }

    /// `pmap(fn, items)`: call a top-level `def` on every item concurrently;
    /// results in item order. Items and results must be JSON-able.
    fn pmap<'v>(
        f: Value<'v>,
        items: Value<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        let (key, c, _) = begin(eval, "pmap")?;
        if c.worker {
            anyhow::bail!("pmap() cannot be nested inside a pmap/parallel function");
        }
        let frozen = frozen_def(f, "pmap")?;
        let Json::Array(items) = items.to_json_value()? else {
            anyhow::bail!("pmap(fn, items): items must be a list");
        };
        if items.len() > MAX_PMAP_ITEMS {
            anyhow::bail!(
                "pmap() takes at most {MAX_PMAP_ITEMS} items, got {}",
                items.len()
            );
        }
        let jobs: Vec<(FrozenValue, Option<Json>)> =
            items.into_iter().map(|i| (frozen, Some(i))).collect();
        let out = fan_out(c, &key, "pmap", jobs)?;
        Ok(eval.heap().alloc(Json::Array(out)))
    }

    /// `parallel([fn, ...])`: call several zero-argument top-level `def`s
    /// concurrently; results in order.
    fn parallel<'v>(
        fns: UnpackList<Value<'v>>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        let (key, c, _) = begin(eval, "parallel")?;
        if c.worker {
            anyhow::bail!("parallel() cannot be nested inside a pmap/parallel function");
        }
        if fns.items.len() > MAX_PMAP_ITEMS {
            anyhow::bail!("parallel() takes at most {MAX_PMAP_ITEMS} functions");
        }
        let jobs = fns
            .items
            .iter()
            .map(|f| Ok((frozen_def(*f, "parallel")?, None)))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let out = fan_out(c, &key, "parallel", jobs)?;
        Ok(eval.heap().alloc(Json::Array(out)))
    }
}

fn check_relative(what: &str, path: &str) -> anyhow::Result<()> {
    let p = std::path::Path::new(path);
    let escapes = p.is_absolute()
        || path.starts_with('~')
        || p.components().any(|c| {
            matches!(
                c,
                std::path::Component::ParentDir | std::path::Component::Prefix(_)
            )
        });
    if escapes {
        anyhow::bail!(
            "{what} must be a path inside the project (no absolute paths, no ..): {path:?}"
        );
    }
    Ok(())
}

fn run_struct<'v>(heap: Heap<'v>, reply: &RunReply) -> Value<'v> {
    let code = match reply.exit_code {
        Some(c) => heap.alloc(c as i64),
        None => Value::new_none(),
    };
    heap.alloc(AllocStruct([
        (
            "ok",
            heap.alloc(
                reply.exit_code == Some(0) && !reply.timed_out && reply.start_error.is_none(),
            ),
        ),
        ("exit_code", code),
        ("stdout", heap.alloc(reply.stdout.as_str())),
        ("stderr", heap.alloc(reply.stderr.as_str())),
        ("timed_out", heap.alloc(reply.timed_out)),
        ("truncated", heap.alloc(reply.truncated)),
        (
            "error",
            match &reply.start_error {
                Some(e) => heap.alloc(e.as_str()),
                None => Value::new_none(),
            },
        ),
    ]))
}

/// A `pmap`/`parallel` function must be a top-level `def`: only frozen
/// values can cross to worker threads.
fn frozen_def(f: Value<'_>, who: &str) -> anyhow::Result<FrozenValue> {
    f.unpack_frozen().ok_or_else(|| {
        anyhow::anyhow!(
            "{who}() needs a top-level `def`; lambdas and nested functions cannot run on worker threads"
        )
    })
}

// ── fan-out ───────────────────────────────────────────────────────────────

fn fan_out(
    parent: &RunCtx,
    parent_key: &SiteKey,
    who: &str,
    jobs: Vec<(FrozenValue, Option<Json>)>,
) -> anyhow::Result<Vec<Json>> {
    let n = jobs.len();
    let results: Mutex<Vec<Option<Result<Json, WorkerError>>>> =
        Mutex::new((0..n).map(|_| None).collect());
    let next = AtomicUsize::new(0);
    let workers = n.min(PMAP_WORKERS);
    let shared = &parent.shared;
    let jobs = &jobs;
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    if i >= n || shared.cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    let (f, arg) = &jobs[i];
                    let out = run_worker(shared, parent_key, i, *f, arg.as_ref());
                    results.lock().unwrap_or_else(|e| e.into_inner())[i] = Some(out);
                }
            });
        }
    });
    let results = results.into_inner().unwrap_or_else(|e| e.into_inner());
    if shared.cancel.load(Ordering::Relaxed) {
        return Err(Abort::Cancelled.into());
    }
    let mut out = Vec::with_capacity(n);
    for (i, r) in results.into_iter().enumerate() {
        match r {
            Some(Ok(v)) => out.push(v),
            Some(Err(WorkerError::Abort(a))) => return Err(a.into()),
            Some(Err(WorkerError::Script(m))) => anyhow::bail!("{who}() item {i} failed: {m}"),
            None => anyhow::bail!("{who}() item {i} did not run"),
        }
    }
    Ok(out)
}

enum WorkerError {
    Abort(Abort),
    Script(String),
}

fn run_worker(
    shared: &Arc<Shared>,
    parent_key: &SiteKey,
    index: usize,
    f: FrozenValue,
    arg: Option<&Json>,
) -> Result<Json, WorkerError> {
    let c = RunCtx::new(shared.clone(), Ordinals::child(parent_key, index), true);
    Module::with_temp_heap(|m| {
        let mut ev = Evaluator::new(&m);
        ev.extra = Some(&c);
        configure(&mut ev, &c).map_err(|e| WorkerError::Script(e.to_string()))?;
        let args: Vec<Value> = arg.map(|a| m.heap().alloc(a)).into_iter().collect();
        let v = ev
            .eval_function(f.to_value(), &args, &[])
            .map_err(|e| classify(&c, &e))?;
        v.to_json_value().map_err(|e| {
            WorkerError::Script(format!(
                "a pmap/parallel function must return JSON-able values (no actors or handles): {e}"
            ))
        })
    })
}

/// A starlark error → how the run should treat it.
pub(crate) fn classify_error(ctx: &RunCtx, e: &starlark::Error) -> Result<String, Abort> {
    if let starlark::ErrorKind::Native(inner) = e.kind()
        && let Some(abort) = inner.downcast_ref::<Abort>()
    {
        return Err(match abort {
            Abort::Cancelled => Abort::Cancelled,
            Abort::Limit(m) => Abort::Limit(m.clone()),
        });
    }
    let text = e.without_diagnostic().to_string();
    if text.contains("cancelled") || text.contains("canceled") {
        return Err(if ctx.compute_exhausted() {
            Abort::Limit(format!(
                "the script used more than {:?} of interpreter time (waiting on agents does not count)",
                ctx.shared.limits.compute
            ))
        } else {
            Abort::Cancelled
        });
    }
    if text.contains("tick") || text.contains("Tick") {
        return Err(Abort::Limit(
            "the script ran too many steps (a loop that does too much work)".into(),
        ));
    }
    if text.to_lowercase().contains("memory limit") {
        return Err(Abort::Limit("the script used too much memory".into()));
    }
    Ok(text)
}

fn classify(ctx: &RunCtx, e: &starlark::Error) -> WorkerError {
    match classify_error(ctx, e) {
        Ok(text) => WorkerError::Script(text),
        Err(abort) => WorkerError::Abort(abort),
    }
}

// ── world reads ───────────────────────────────────────────────────────────

fn world_read<'v>(eval: &mut Evaluator<'v, '_, '_>, op: ReadOp) -> anyhow::Result<Value<'v>> {
    let (key, c, _) = begin(eval, op.name())?;
    let name = op.name();
    let value = c
        .shared
        .host
        .read(&key, &op)
        .map_err(|e| anyhow::anyhow!("{name}: {e}"))?;
    Ok(eval.heap().alloc(&value))
}

#[starlark_module]
fn files_fns(b: &mut GlobalsBuilder) {
    /// `files.glob(pattern)`: project-relative paths matching the pattern
    /// (gitignore rules apply). More than 2000 matches is an error.
    fn glob<'v>(pattern: &str, eval: &mut Evaluator<'v, '_, '_>) -> anyhow::Result<Value<'v>> {
        check_pattern(pattern)?;
        world_read(
            eval,
            ReadOp::Glob {
                pattern: pattern.to_owned(),
            },
        )
    }

    /// `files.read(path)`: the file's text, or None when it does not exist.
    /// Files over 256 KB are an error.
    fn read<'v>(path: &str, eval: &mut Evaluator<'v, '_, '_>) -> anyhow::Result<Value<'v>> {
        check_relative("path", path)?;
        world_read(
            eval,
            ReadOp::Read {
                path: path.to_owned(),
            },
        )
    }

    /// `files.grep(pattern, glob=None)`: `{path, line, text}` matches of a
    /// regular expression. More than 2000 matches is an error.
    fn grep<'v>(
        pattern: &str,
        #[starlark(default = NoneOr::None)] glob: NoneOr<&str>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        check_pattern(pattern)?;
        let glob = opt_string(glob);
        if let Some(g) = &glob {
            check_pattern(g)?;
        }
        world_read(
            eval,
            ReadOp::Grep {
                pattern: pattern.to_owned(),
                glob,
            },
        )
    }
}

fn check_pattern(p: &str) -> anyhow::Result<()> {
    if p.is_empty() || p.len() > 512 {
        anyhow::bail!("a pattern must be 1 to 512 characters");
    }
    if p.starts_with('/') || p.split('/').any(|s| s == "..") {
        anyhow::bail!("patterns are relative to the project and cannot contain ..: {p:?}");
    }
    Ok(())
}

#[starlark_module]
fn git_fns(b: &mut GlobalsBuilder) {
    /// `git.changed_files(base=None)`: files changed against `base` (default:
    /// the working tree against HEAD).
    fn changed_files<'v>(
        #[starlark(default = NoneOr::None)] base: NoneOr<&str>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        world_read(
            eval,
            ReadOp::GitChangedFiles {
                base: git_ref(base)?,
            },
        )
    }

    /// `git.diff(base=None, path=None)`: unified diff text; over 256 KB is an error.
    fn diff<'v>(
        #[starlark(default = NoneOr::None)] base: NoneOr<&str>,
        #[starlark(default = NoneOr::None)] path: NoneOr<&str>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        let path = opt_string(path);
        if let Some(p) = &path {
            check_relative("path", p)?;
        }
        world_read(
            eval,
            ReadOp::GitDiff {
                base: git_ref(base)?,
                path,
            },
        )
    }

    /// `git.status()`: porcelain status lines.
    fn status<'v>(eval: &mut Evaluator<'v, '_, '_>) -> anyhow::Result<Value<'v>> {
        world_read(eval, ReadOp::GitStatus)
    }

    /// `git.log(limit=20, path=None)`: `{hash, subject, author, date}`.
    fn log<'v>(
        #[starlark(default = 20)] limit: i32,
        #[starlark(default = NoneOr::None)] path: NoneOr<&str>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        if !(1..=500).contains(&limit) {
            anyhow::bail!("git.log limit must be between 1 and 500");
        }
        let path = opt_string(path);
        if let Some(p) = &path {
            check_relative("path", p)?;
        }
        world_read(
            eval,
            ReadOp::GitLog {
                limit: limit as u32,
                path,
            },
        )
    }
}

fn git_ref(base: NoneOr<&str>) -> anyhow::Result<Option<String>> {
    let Some(r) = opt_string(base) else {
        return Ok(None);
    };
    // A ref is data to `git`, never an option: no leading dash, no spaces.
    if r.is_empty() || r.starts_with('-') || r.len() > 200 || r.chars().any(char::is_whitespace) {
        anyhow::bail!("not a usable git ref: {r:?}");
    }
    Ok(Some(r))
}

// ── artifacts ─────────────────────────────────────────────────────────────

fn publish(
    eval: &Evaluator<'_, '_, '_>,
    id: &str,
    title: &str,
    content: ArtifactContent,
    what: &str,
) -> anyhow::Result<NoneType> {
    let (key, c, site) = begin(eval, what)?;
    match c.shared.sites.get(&site) {
        Some(SiteKind::Artifact { id: literal }) if literal == id => {}
        _ => anyhow::bail!("{what}() must be called directly with the id as a string literal"),
    }
    if c.worker {
        anyhow::bail!("{what}() cannot be called from a pmap/parallel function; publish from main");
    }
    if id.is_empty()
        || id.len() > 64
        || !id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        anyhow::bail!("artifact ids are 1 to 64 characters of letters, digits, - _ . : {id:?}");
    }
    if title.len() > 200 {
        anyhow::bail!("artifact titles are limited to 200 characters");
    }
    let version = {
        let mut ids = c.shared.artifacts.lock().unwrap_or_else(|e| e.into_inner());
        if !ids.contains_key(id) && ids.len() >= MAX_ARTIFACT_IDS {
            anyhow::bail!("a run may publish at most {MAX_ARTIFACT_IDS} distinct artifact ids");
        }
        let v = ids.entry(id.to_owned()).or_insert(0);
        if *v as usize >= MAX_ARTIFACT_VERSIONS {
            anyhow::bail!("artifact {id:?} already has {MAX_ARTIFACT_VERSIONS} versions");
        }
        *v += 1;
        *v
    };
    c.shared
        .host
        .artifact(ArtifactRequest {
            key,
            id: id.to_owned(),
            title: title.to_owned(),
            content,
            version,
        })
        .map_err(|e| anyhow::anyhow!("{what}: {e}"))?;
    Ok(NoneType)
}

#[starlark_module]
fn artifact_fns(b: &mut GlobalsBuilder) {
    /// `artifact.markdown(id, title, text)`
    fn markdown(
        id: &str,
        title: &str,
        text: &str,
        eval: &mut Evaluator<'_, '_, '_>,
    ) -> anyhow::Result<NoneType> {
        if text.len() > MAX_ARTIFACT_BYTES {
            anyhow::bail!("artifact text is limited to {MAX_ARTIFACT_BYTES} bytes");
        }
        publish(
            eval,
            id,
            title,
            ArtifactContent::Markdown(text.to_owned()),
            "artifact.markdown",
        )
    }

    /// `artifact.table(id, title, columns, rows)`: `rows` is a list of lists
    /// (one cell per column) or a list of dicts keyed by column name.
    fn table<'v>(
        id: &str,
        title: &str,
        columns: UnpackList<String>,
        rows: Value<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<NoneType> {
        let columns = columns.items;
        if columns.is_empty() || columns.len() > MAX_TABLE_COLUMNS {
            anyhow::bail!("a table has 1 to {MAX_TABLE_COLUMNS} columns");
        }
        let Json::Array(rows) = rows.to_json_value()? else {
            anyhow::bail!("artifact.table rows must be a list");
        };
        if rows.len() > MAX_TABLE_ROWS {
            anyhow::bail!(
                "a table has at most {MAX_TABLE_ROWS} rows, got {}",
                rows.len()
            );
        }
        let mut cells = Vec::with_capacity(rows.len());
        for (i, row) in rows.into_iter().enumerate() {
            let row = match row {
                Json::Array(r) if r.len() == columns.len() => r,
                Json::Array(r) => anyhow::bail!(
                    "table row {i} has {} cells for {} columns",
                    r.len(),
                    columns.len()
                ),
                Json::Object(map) => columns
                    .iter()
                    .map(|c| map.get(c).cloned().unwrap_or(Json::Null))
                    .collect(),
                _ => anyhow::bail!("table row {i} must be a list or a dict"),
            };
            cells.push(row);
        }
        let size: usize = cells
            .iter()
            .map(|r| Json::Array(r.clone()).to_string().len())
            .sum();
        if size > MAX_ARTIFACT_BYTES {
            anyhow::bail!("table content is limited to {MAX_ARTIFACT_BYTES} bytes");
        }
        publish(
            eval,
            id,
            title,
            ArtifactContent::Table {
                columns,
                rows: cells,
            },
            "artifact.table",
        )
    }

    /// `artifact.metrics(id, title, items)`: a dict `{label: value}` or a list
    /// of `{label, value, unit?}` dicts.
    fn metrics<'v>(
        id: &str,
        title: &str,
        items: Value<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<NoneType> {
        let metrics = match items.to_json_value()? {
            Json::Object(map) => map
                .into_iter()
                .map(|(label, value)| Metric {
                    label,
                    value,
                    unit: None,
                })
                .collect::<Vec<_>>(),
            Json::Array(list) => list
                .into_iter()
                .map(|m| {
                    let obj = m.as_object().ok_or_else(|| {
                        anyhow::anyhow!("metrics items must be {{label, value, unit?}} dicts")
                    })?;
                    let label = obj
                        .get("label")
                        .and_then(Json::as_str)
                        .ok_or_else(|| anyhow::anyhow!("a metric needs a string `label`"))?
                        .to_owned();
                    let value = obj
                        .get("value")
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!("a metric needs a `value`"))?;
                    let unit = obj.get("unit").and_then(Json::as_str).map(str::to_owned);
                    Ok(Metric { label, value, unit })
                })
                .collect::<anyhow::Result<Vec<_>>>()?,
            _ => anyhow::bail!("artifact.metrics items must be a dict or a list of dicts"),
        };
        if metrics.is_empty() || metrics.len() > MAX_METRICS_ITEMS {
            anyhow::bail!("metrics need 1 to {MAX_METRICS_ITEMS} items");
        }
        publish(
            eval,
            id,
            title,
            ArtifactContent::Metrics(metrics),
            "artifact.metrics",
        )
    }

    /// `artifact.file(id, title, path)`: publish a project file (copied into
    /// the run when the script calls this).
    fn file(
        id: &str,
        title: &str,
        path: &str,
        eval: &mut Evaluator<'_, '_, '_>,
    ) -> anyhow::Result<NoneType> {
        check_relative("path", path)?;
        publish(
            eval,
            id,
            title,
            ArtifactContent::File(path.to_owned()),
            "artifact.file",
        )
    }
}

// ── schema helpers ────────────────────────────────────────────────────────

/// Marker `schema.opt` adds and `schema.obj` consumes.
const OPTIONAL_MARK: &str = "x-optional";

fn described(mut schema: Map<String, Json>, desc: NoneOr<&str>) -> Json {
    if let NoneOr::Other(d) = desc {
        schema.insert("description".into(), json!(d));
    }
    Json::Object(schema)
}

fn put_num(map: &mut Map<String, Json>, key: &str, v: NoneOr<Value<'_>>) -> anyhow::Result<()> {
    if let NoneOr::Other(n) = v {
        let json = n.to_json_value()?;
        if !json.is_number() {
            anyhow::bail!("{key} must be a number");
        }
        map.insert(key.into(), json);
    }
    Ok(())
}

#[starlark_module]
fn schema_fns(b: &mut GlobalsBuilder) {
    /// `schema.str(desc=None, min_len=None, max_len=None)`
    fn str<'v>(
        #[starlark(default = NoneOr::None)] desc: NoneOr<&str>,
        #[starlark(require = named, default = NoneOr::None)] min_len: NoneOr<Value<'v>>,
        #[starlark(require = named, default = NoneOr::None)] max_len: NoneOr<Value<'v>>,
        heap: Heap<'v>,
    ) -> anyhow::Result<Value<'v>> {
        let mut m = Map::new();
        m.insert("type".into(), json!("string"));
        put_num(&mut m, "minLength", min_len)?;
        put_num(&mut m, "maxLength", max_len)?;
        Ok(heap.alloc(described(m, desc)))
    }

    /// `schema.int(desc=None, min=None, max=None)`
    fn int<'v>(
        #[starlark(default = NoneOr::None)] desc: NoneOr<&str>,
        #[starlark(require = named, default = NoneOr::None)] min: NoneOr<Value<'v>>,
        #[starlark(require = named, default = NoneOr::None)] max: NoneOr<Value<'v>>,
        heap: Heap<'v>,
    ) -> anyhow::Result<Value<'v>> {
        let mut m = Map::new();
        m.insert("type".into(), json!("integer"));
        put_num(&mut m, "minimum", min)?;
        put_num(&mut m, "maximum", max)?;
        Ok(heap.alloc(described(m, desc)))
    }

    /// `schema.num(desc=None, min=None, max=None)`
    fn num<'v>(
        #[starlark(default = NoneOr::None)] desc: NoneOr<&str>,
        #[starlark(require = named, default = NoneOr::None)] min: NoneOr<Value<'v>>,
        #[starlark(require = named, default = NoneOr::None)] max: NoneOr<Value<'v>>,
        heap: Heap<'v>,
    ) -> anyhow::Result<Value<'v>> {
        let mut m = Map::new();
        m.insert("type".into(), json!("number"));
        put_num(&mut m, "minimum", min)?;
        put_num(&mut m, "maximum", max)?;
        Ok(heap.alloc(described(m, desc)))
    }

    /// `schema.bool(desc=None)`
    fn bool<'v>(
        #[starlark(default = NoneOr::None)] desc: NoneOr<&str>,
        heap: Heap<'v>,
    ) -> anyhow::Result<Value<'v>> {
        let mut m = Map::new();
        m.insert("type".into(), json!("boolean"));
        Ok(heap.alloc(described(m, desc)))
    }

    /// `schema.enum(values, desc=None)`: one of the given strings (or values).
    fn r#enum<'v>(
        values: Value<'v>,
        #[starlark(default = NoneOr::None)] desc: NoneOr<&str>,
        heap: Heap<'v>,
    ) -> anyhow::Result<Value<'v>> {
        let Json::Array(values) = values.to_json_value()? else {
            anyhow::bail!("schema.enum takes a list of allowed values");
        };
        if values.is_empty() {
            anyhow::bail!("schema.enum needs at least one value");
        }
        let mut m = Map::new();
        if values.iter().all(Json::is_string) {
            m.insert("type".into(), json!("string"));
        }
        m.insert("enum".into(), Json::Array(values));
        Ok(heap.alloc(described(m, desc)))
    }

    /// `schema.list(item, desc=None, min_items=None, max_items=None)`
    fn list<'v>(
        item: Value<'v>,
        #[starlark(default = NoneOr::None)] desc: NoneOr<&str>,
        #[starlark(require = named, default = NoneOr::None)] min_items: NoneOr<Value<'v>>,
        #[starlark(require = named, default = NoneOr::None)] max_items: NoneOr<Value<'v>>,
        heap: Heap<'v>,
    ) -> anyhow::Result<Value<'v>> {
        let mut item = item.to_json_value()?;
        if let Some(o) = item.as_object_mut() {
            o.remove(OPTIONAL_MARK);
        }
        let mut m = Map::new();
        m.insert("type".into(), json!("array"));
        m.insert("items".into(), item);
        put_num(&mut m, "minItems", min_items)?;
        put_num(&mut m, "maxItems", max_items)?;
        Ok(heap.alloc(described(m, desc)))
    }

    /// `schema.obj(props, required=None, desc=None)`: `required` defaults to
    /// every property not wrapped in `schema.opt(...)`.
    fn obj<'v>(
        props: Value<'v>,
        #[starlark(default = NoneOr::None)] required: NoneOr<UnpackList<String>>,
        #[starlark(default = NoneOr::None)] desc: NoneOr<&str>,
        heap: Heap<'v>,
    ) -> anyhow::Result<Value<'v>> {
        let Json::Object(props) = props.to_json_value()? else {
            anyhow::bail!("schema.obj takes a dict of property name -> schema");
        };
        let mut clean = Map::new();
        let mut default_required = Vec::new();
        for (name, mut schema) in props {
            let optional = schema
                .as_object_mut()
                .and_then(|o| o.remove(OPTIONAL_MARK))
                .is_some();
            if !optional {
                default_required.push(name.clone());
            }
            clean.insert(name, schema);
        }
        let required = match required {
            NoneOr::Other(list) => {
                for r in &list.items {
                    if !clean.contains_key(r) {
                        anyhow::bail!(
                            "schema.obj: required property {r:?} is not in the properties"
                        );
                    }
                }
                list.items
            }
            NoneOr::None => default_required,
        };
        let mut m = Map::new();
        m.insert("type".into(), json!("object"));
        m.insert("properties".into(), Json::Object(clean));
        m.insert("required".into(), json!(required));
        Ok(heap.alloc(described(m, desc)))
    }

    /// `schema.opt(s)`: the property may be omitted.
    fn opt<'v>(s: Value<'v>, heap: Heap<'v>) -> anyhow::Result<Value<'v>> {
        let mut json = s.to_json_value()?;
        let Some(o) = json.as_object_mut() else {
            anyhow::bail!("schema.opt takes a schema");
        };
        o.insert(OPTIONAL_MARK.into(), json!(true));
        Ok(heap.alloc(&json))
    }

    /// `schema.any(desc=None)`: any JSON value.
    fn any<'v>(
        #[starlark(default = NoneOr::None)] desc: NoneOr<&str>,
        heap: Heap<'v>,
    ) -> anyhow::Result<Value<'v>> {
        Ok(heap.alloc(described(Map::new(), desc)))
    }
}

// ── assembly ──────────────────────────────────────────────────────────────

fn build() -> Globals {
    let mut b = GlobalsBuilder::extended_by(&[
        LibraryExtension::Json,
        LibraryExtension::Map,
        LibraryExtension::Filter,
    ])
    .with(host_fns);
    b.namespace("files", files_fns);
    b.namespace("git", git_fns);
    b.namespace("artifact", artifact_fns);
    b.namespace("schema", schema_fns);
    b.build()
}

pub(crate) fn globals() -> &'static Globals {
    static G: OnceLock<Globals> = OnceLock::new();
    G.get_or_init(build)
}

/// Names a script may use; the analysis hands them to the undefined-name lint.
pub fn names() -> std::collections::HashSet<String> {
    globals().names().map(|n| n.as_str().to_owned()).collect()
}
