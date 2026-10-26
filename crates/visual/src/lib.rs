//! Agent visuals as sandboxed Luau programs.
//!
//! An agent's `show_visual` call carries a Luau script. The script builds its
//! data once and returns a view function; the view returns a tree of `ui.*`
//! nodes (layout, text, stats, tables, charts, controls, canvases, 3D scenes).
//! Zeron renders that tree with native elements, so a visual scrolls, clips,
//! takes input, and sits on frosted glass exactly like the transcript around
//! it, on every platform. Interactivity is Lua: controls call handlers that
//! change the script's state, and Zeron renders the view again.
//!
//! The style rules are the API: colors are theme tokens, text sizes are steps
//! on the transcript's type scale, the font is always the interface font, and
//! layout nodes have no fills, borders, or shadows to reach for.
//!
//! Scripts run in a Luau sandbox with no I/O, a memory cap, and a time budget
//! per call, so a runaway script fails with an error instead of freezing the
//! app. `math.random` is seeded from the visual's id before every call, so a
//! script's random data is the same on every render and every device.

pub mod assets;
mod draw;
pub mod image;
mod model;
mod node;
mod raster;
mod scene;
pub mod shader;
mod style;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use mlua::{Function, Lua, LuaOptions, StdLib, Table, Value, VmState};

pub use assets::Assets;
pub use draw::{DrawCmd, DrawText, Pointer};
pub use node::{
    Axis, Canvas, Chart, ChartKind, Column, HandlerId, Node, NodeKind, NumberFormat, SceneNode,
    Series, ShaderNode, Span, Stack, Table_ as TableNode, Text,
};
pub use raster::{Quality, Raster, RasterLabel};
pub use scene::{
    Camera, CameraFrame, Environment, FaceView, Material, Prim, Projection, Scene, Sprite,
    TextureData, TextureSource, V3,
};
pub use style::{Align, CHART_SERIES, Color, Justify, TOKEN_NAMES, TextSize, Token, Weight};

/// Largest script `show_visual` accepts.
pub const MAX_SCRIPT_BYTES: usize = 256 * 1024;
const MEMORY_LIMIT: usize = 64 * 1024 * 1024;
/// Setup and view calls may take this long before they fail.
const CALL_BUDGET: Duration = Duration::from_millis(250);
/// A canvas or scene draw runs per frame while animating.
const DRAW_BUDGET: Duration = Duration::from_millis(60);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct VisualError {
    pub message: String,
}

impl VisualError {
    pub fn script(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl From<mlua::Error> for VisualError {
    fn from(error: mlua::Error) -> Self {
        Self::script(describe(&error))
    }
}

/// The innermost message of a Lua error, without mlua's wrapping.
fn describe(error: &mlua::Error) -> String {
    match error {
        // A Rust callback's error carries no position; take the script line
        // that called it from the traceback.
        mlua::Error::CallbackError { cause, traceback } => {
            let message = describe(cause);
            match traceback
                .lines()
                .find_map(|line| line.trim().strip_prefix("visual:"))
                .and_then(|rest| rest.split(':').next())
                .filter(|_| !message.starts_with("visual:"))
            {
                Some(line) => format!("visual:{line}: {message}"),
                None => message,
            }
        }
        mlua::Error::RuntimeError(message) => message.clone(),
        mlua::Error::SyntaxError { message, .. } => message.clone(),
        mlua::Error::MemoryError(_) => {
            format!("the script used more than {} MB", MEMORY_LIMIT >> 20)
        }
        mlua::Error::WithContext { context, cause } => format!("{context}: {}", describe(cause)),
        other => node::short_error(other),
    }
}

/// Arguments a node passes to its handler.
#[derive(Debug, Clone, PartialEq)]
pub enum HandlerArg {
    None,
    Text(String),
    Bool(bool),
    Number(f64),
    Point(f32, f32),
}

/// One run of a canvas's draw function.
#[derive(Debug, Clone, PartialEq)]
pub struct CanvasDrawing {
    pub commands: Vec<DrawCmd>,
    /// The drawing read `g.pointer`, so it responds to where the pointer is.
    pub reads_pointer: bool,
}

/// One visual's live program.
pub struct Visual {
    lua: Lua,
    view: Function,
    handlers: Vec<Function>,
    /// Tabs' option values by handler, from the last render.
    choices: std::collections::HashMap<HandlerId, Vec<(String, Value)>>,
    randomseed: Function,
    seed: i64,
    deadline: Rc<Cell<Option<Instant>>>,
    logs: Rc<RefCell<Vec<String>>>,
    assets: Assets,
}

const MAX_LOG_LINES: usize = 200;

impl Visual {
    /// Compile and run a script's setup. `seed` keeps its random numbers
    /// stable (derive it from the visual's id).
    pub fn load(source: &str, seed: u64) -> Result<Self, VisualError> {
        if source.len() > MAX_SCRIPT_BYTES {
            return Err(VisualError::script(format!(
                "the script is over {} KB",
                MAX_SCRIPT_BYTES / 1024
            )));
        }
        let lua = Lua::new_with(
            StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8 | StdLib::BIT,
            LuaOptions::default(),
        )?;
        lua.set_memory_limit(MEMORY_LIMIT)?;

        let deadline: Rc<Cell<Option<Instant>>> = Rc::default();
        let ticks = Cell::new(0u32);
        let watched = deadline.clone();
        lua.set_interrupt(move |_| {
            // Reading the clock on every safepoint is measurable; every 256th
            // is plenty to stop a runaway loop within a few milliseconds.
            let tick = ticks.get().wrapping_add(1);
            ticks.set(tick);
            if tick.is_multiple_of(256)
                && let Some(deadline) = watched.get()
                && Instant::now() > deadline
            {
                return Err(mlua::Error::RuntimeError(
                    "the script ran past its time budget (an endless loop, or too much work \
                     per render; precompute data at the top level)"
                        .into(),
                ));
            }
            Ok(VmState::Continue)
        });

        let globals = lua.globals();
        let logs: Rc<RefCell<Vec<String>>> = Rc::default();
        let sink = logs.clone();
        globals.set(
            "print",
            lua.create_function(move |_, args: mlua::Variadic<Value>| {
                let mut logs = sink.borrow_mut();
                if logs.len() < MAX_LOG_LINES {
                    let line: Vec<String> = args.iter().map(node::value_text).collect();
                    logs.push(line.join("\t"));
                }
                Ok(())
            })?,
        )?;
        let ui: Table = lua
            .load(include_str!("prelude.luau"))
            .set_name("=ui")
            .eval()?;
        ui.set(
            "fmt",
            lua.create_function(|_, (value, options): (f64, Option<Table>)| {
                let mut format = NumberFormat::default();
                if let Some(o) = options {
                    format.prefix = o.get::<Option<String>>("prefix")?.unwrap_or_default();
                    format.unit = o.get::<Option<String>>("unit")?.unwrap_or_default();
                    format.decimals = o
                        .get::<Option<f64>>("decimals")?
                        .map(|d| d.clamp(0.0, 6.0) as u8);
                }
                Ok(format.format(value))
            })?,
        )?;
        ui.set(
            "ticks",
            lua.create_function(|_, (lo, hi, count): (f64, f64, Option<usize>)| {
                Ok(nice_ticks(lo, hi, count.unwrap_or(5)))
            })?,
        )?;
        globals.set("ui", ui)?;
        let math: Table = globals.get("math")?;
        let randomseed: Function = math.get("randomseed")?;
        lua.sandbox(true)?;

        let seed = (seed & 0x7fff_ffff) as i64;
        let visual_deadline = deadline.clone();
        let call = |f: &dyn Fn() -> mlua::Result<Value>| {
            visual_deadline.set(Some(Instant::now() + CALL_BUDGET));
            let result = f();
            visual_deadline.set(None);
            result
        };
        randomseed.call::<()>(seed)?;
        let returned = call(&|| lua.load(source).set_name("=visual").eval::<Value>())?;
        let view = match returned {
            Value::Function(view) => view,
            Value::Table(tree) => lua.create_function(move |_, ()| Ok(tree.clone()))?,
            Value::Nil => {
                return Err(VisualError::script(
                    "the script must end with `return function() return ui.column { ... } end` \
                     (or return a node)",
                ));
            }
            other => {
                return Err(VisualError::script(format!(
                    "the script returned a {}; return a view function or a ui node",
                    other.type_name()
                )));
            }
        };
        Ok(Self {
            lua,
            view,
            handlers: Vec::new(),
            choices: Default::default(),
            randomseed,
            seed,
            deadline,
            logs,
            assets: Assets::default(),
        })
    }

    /// Give the script its asset files (textures, models).
    pub fn with_assets(mut self, assets: Assets) -> Self {
        self.assets = assets;
        self
    }

    pub fn assets(&self) -> &Assets {
        &self.assets
    }

    fn budgeted<R>(
        &self,
        budget: Duration,
        f: impl FnOnce() -> mlua::Result<R>,
    ) -> Result<R, VisualError> {
        self.randomseed.call::<()>(self.seed)?;
        self.deadline.set(Some(Instant::now() + budget));
        let result = f();
        self.deadline.set(None);
        Ok(result?)
    }

    /// Run the view and parse its tree. Handler ids in the tree stay valid
    /// until the next render.
    pub fn render(&mut self) -> Result<Node, VisualError> {
        let value = self.budgeted(CALL_BUDGET, || self.view.call::<Value>(()))?;
        let mut parser = node::Parser::new();
        let deadline = Instant::now() + CALL_BUDGET;
        self.deadline.set(Some(deadline));
        let tree = parser.root(value);
        self.deadline.set(None);
        let tree = tree?;
        self.handlers = parser.handlers;
        self.choices = parser.choices;
        Ok(tree)
    }

    fn handler(&self, id: HandlerId) -> Result<&Function, VisualError> {
        self.handlers
            .get(id.0 as usize)
            .ok_or_else(|| VisualError::script("stale handler"))
    }

    /// Call an event handler. The caller renders again afterwards.
    pub fn call(&mut self, id: HandlerId, arg: HandlerArg) -> Result<(), VisualError> {
        let handler = self.handler(id)?.clone();
        // A tab hands over its option's own value (a key or a position), not
        // the label it shows.
        let choice = match (&arg, self.choices.get(&id)) {
            (HandlerArg::Text(label), Some(choices)) => choices
                .iter()
                .find(|(shown, _)| shown == label)
                .map(|(_, value)| value.clone()),
            _ => None,
        };
        self.budgeted(CALL_BUDGET, || match arg {
            _ if choice.is_some() => handler.call::<()>(choice),
            HandlerArg::None => handler.call::<()>(()),
            HandlerArg::Text(text) => handler.call::<()>(text),
            HandlerArg::Bool(b) => handler.call::<()>(b),
            HandlerArg::Number(n) => handler.call::<()>(n),
            HandlerArg::Point(x, y) => handler.call::<()>((x, y)),
        })
    }

    /// Run a canvas's draw function at a size, in design pixels.
    pub fn draw_canvas(
        &mut self,
        id: HandlerId,
        width: f32,
        height: f32,
        time: f64,
        pointer: Option<Pointer>,
    ) -> Result<Vec<DrawCmd>, VisualError> {
        Ok(self
            .draw_canvas_traced(id, width, height, time, pointer)?
            .commands)
    }

    /// [`Visual::draw_canvas`], also reporting whether the drawing read the
    /// pointer.
    pub fn draw_canvas_traced(
        &mut self,
        id: HandlerId,
        width: f32,
        height: f32,
        time: f64,
        pointer: Option<Pointer>,
    ) -> Result<CanvasDrawing, VisualError> {
        let draw = self.handler(id)?.clone();
        let graphics = self
            .lua
            .create_userdata(draw::Graphics::new(width, height, time, pointer))?;
        self.budgeted(DRAW_BUDGET, || {
            draw.call::<()>((&graphics, width, height, time))
        })?;
        let graphics = graphics.take::<draw::Graphics>()?;
        Ok(CanvasDrawing {
            reads_pointer: graphics.read_pointer.get(),
            commands: graphics.commands,
        })
    }

    /// Run a scene's draw function.
    pub fn draw_scene(&mut self, id: HandlerId, time: f64) -> Result<Scene, VisualError> {
        let draw = self.handler(id)?.clone();
        let scene = self
            .lua
            .create_userdata(Scene::with_assets(time, self.assets.clone()))?;
        self.budgeted(DRAW_BUDGET, || draw.call::<()>((&scene, time)))?;
        Ok(scene.take::<Scene>()?)
    }

    /// Lines the script printed, oldest first.
    pub fn logs(&self) -> Vec<String> {
        self.logs.borrow().clone()
    }
}

/// Round tick values covering `lo..=hi`, about `count` of them.
pub fn nice_ticks(lo: f64, hi: f64, count: usize) -> Vec<f64> {
    let (lo, hi) = if hi < lo { (hi, lo) } else { (lo, hi) };
    if !lo.is_finite() || !hi.is_finite() {
        return Vec::new();
    }
    if (hi - lo).abs() < f64::EPSILON {
        return vec![lo];
    }
    let raw = (hi - lo) / count.max(1) as f64;
    let magnitude = 10f64.powf(raw.log10().floor());
    let normalized = raw / magnitude;
    let step = magnitude
        * if normalized < 1.5 {
            1.0
        } else if normalized < 3.0 {
            2.0
        } else if normalized < 7.0 {
            5.0
        } else {
            10.0
        };
    let mut out = Vec::new();
    let mut v = (lo / step).ceil() * step;
    // Snap away float noise (0.6000000000000001) at the step's precision.
    let digits = 10f64.powi((-step.log10().floor()).max(0.0) as i32);
    while v <= hi + step * 1e-9 && out.len() < 100 {
        out.push(((v / step).round() * step * digits).round() / digits);
        v += step;
    }
    out
}

/// What `show_visual` learned by running a script before showing it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    pub nodes: usize,
    pub interactions: usize,
    /// Tabs controls whose every option was checked to show its own view.
    pub tabs: usize,
    pub logs: Vec<String>,
}

/// Run a script the way the desktop will: setup, the first render, every
/// canvas and scene draw, and then each tab, toggle, slider end, and button
/// once with a render after each. The first failure comes back with where
/// it happened, so the agent can fix the script before the reader sees it.
pub fn check(source: &str, seed: u64) -> Result<Report, VisualError> {
    check_with_assets(source, seed, Assets::default())
}

/// [`check`] for a script shipped with asset files.
pub fn check_with_assets(source: &str, seed: u64, assets: Assets) -> Result<Report, VisualError> {
    let mut visual = Visual::load(source, seed)?.with_assets(assets.clone());
    let tree = visual.render()?;
    let mut report = Report::default();
    exercise_draws(&mut visual, &tree, &mut report)?;

    let mut interactions = Vec::new();
    collect_interactions(&tree, &mut interactions);
    for (describe, path, arg) in interactions.into_iter().take(24) {
        // Ids are per render; find the control again in the current tree.
        let current = visual.render()?;
        let Some(id) = handler_at(&current, &path) else {
            continue;
        };
        visual
            .call(id, arg)
            .and_then(|()| {
                let tree = visual.render()?;
                exercise_draws(&mut visual, &tree, &mut Report::default())
            })
            .map_err(|e| VisualError::script(format!("after {describe}: {}", e.message)))?;
        report.interactions += 1;
    }
    report.tabs = check_tabs(source, seed, &assets)?;
    report.logs = visual.logs();
    Ok(report)
}

/// Tabs controls checked (each on a fresh run), nested ones included.
const MAX_TAB_CHECKS: usize = 8;
/// Selection prefixes replayed while looking for tabs nested in a tab.
const MAX_TAB_PREFIXES: usize = 24;
/// Each replay reruns setup; a heavy script stops looking for more tabs here.
const TAB_CHECK_BUDGET: Duration = Duration::from_millis(1500);

type Selection = (Vec<usize>, String);

/// Every tab must work: selecting an option selects it, each option shows its
/// own view, and the first render is the view of the tab it shows selected.
/// A view that keys on a different string than the option (`"overview"` for
/// `"Overview"`), or state that starts nil, otherwise passes the plain check
/// and leaves the reader clicking tabs that show nothing.
fn check_tabs(source: &str, seed: u64, assets: &Assets) -> Result<usize, VisualError> {
    let replay = |prefix: &[Selection]| -> Result<(Visual, Node), VisualError> {
        let mut visual = Visual::load(source, seed)?.with_assets(assets.clone());
        let mut tree = visual.render()?;
        for (path, option) in prefix {
            let id = handler_at(&tree, path).ok_or_else(|| VisualError::script("stale tabs"))?;
            visual.call(id, HandlerArg::Text(option.clone()))?;
            tree = visual.render()?;
        }
        Ok((visual, tree))
    };
    let mut seen = std::collections::HashSet::new();
    let mut queue = std::collections::VecDeque::from([Vec::<Selection>::new()]);
    let (mut checked, mut replayed) = (0, 0);
    let started = Instant::now();
    while let Some(prefix) = queue.pop_front() {
        if checked >= MAX_TAB_CHECKS
            || replayed >= MAX_TAB_PREFIXES
            || started.elapsed() > TAB_CHECK_BUDGET
        {
            break;
        }
        replayed += 1;
        let (_, tree) = replay(&prefix)?;
        let mut paths = Vec::new();
        tabs_paths(&tree, &mut Vec::new(), &mut paths);
        for path in paths {
            if checked >= MAX_TAB_CHECKS || !seen.insert(path.clone()) {
                continue;
            }
            checked += 1;
            let (mut visual, initial) = replay(&prefix)?;
            let Some(NodeKind::Tabs { options, value, .. }) =
                node_at(&initial, &path).map(|n| &n.kind)
            else {
                continue;
            };
            let first = value.clone();
            if let Some(NodeKind::Tabs { on_change, .. }) =
                node_at(&initial, &path).map(|n| &n.kind)
                && let Some(choices) = visual.choices.get(on_change)
            {
                miskeyed_tab(source, choices)?;
            }
            let mut order: Vec<String> = Vec::new();
            for option in options.iter().filter(|o| **o != first) {
                if !order.contains(option) {
                    order.push(option.clone());
                }
            }
            order.push(first.clone());
            let mut views: Vec<(String, View)> = Vec::new();
            for option in order {
                let failed = |e: VisualError| {
                    VisualError::script(format!("after selecting tab '{option}': {}", e.message))
                };
                let current = visual.render()?;
                let Some(id) = handler_at(&current, &path) else {
                    break;
                };
                visual
                    .call(id, HandlerArg::Text(option.clone()))
                    .map_err(failed)?;
                let tree = visual.render().map_err(failed)?;
                match node_at(&tree, &path).map(|n| &n.kind) {
                    Some(NodeKind::Tabs { value, .. }) if *value == option => {}
                    Some(NodeKind::Tabs { value, .. }) => {
                        return Err(VisualError::script(format!(
                            "after selecting tab '{option}', the tabs still show '{value}' \
                             selected: on_change receives the option ('{option}'); store it in \
                             state and pass that state back as value"
                        )));
                    }
                    // The view moved the tabs; nothing to compare against.
                    _ => break,
                }
                let view = view_of(&mut visual, &tree, &path).map_err(failed)?;
                if let Some((other, _)) = views.iter().find(|(_, v)| *v == view) {
                    return Err(VisualError::script(format!(
                        "tabs '{other}' and '{option}' show the same view: the view must \
                         change with the selected tab (on_change receives the option text, \
                         e.g. '{option}'; if you branch with `cond and node or nil`, compare \
                         against exactly those strings)"
                    )));
                }
                if queue.len() < MAX_TAB_PREFIXES {
                    let mut next = prefix.clone();
                    next.push((path.clone(), option.clone()));
                    queue.push_back(next);
                }
                views.push((option, view));
            }
            // The last selection was the tab shown first: before any click the
            // reader must see its content. Only less is a fault; a handler may
            // also reset other state (a selection), which changes but doesn't
            // empty the view.
            if let Some((_, selected)) = views.last()
                && let (mut fresh, untouched) = replay(&prefix)?
                && count_nodes(&view_of(&mut fresh, &untouched, &path)?.tree)
                    < count_nodes(&selected.tree)
            {
                return Err(VisualError::script(format!(
                    "the first view shows tab '{first}' selected but not its content (is the \
                     tab state nil at first? start it at \"{first}\")"
                )));
            }
        }
    }
    Ok(checked)
}

/// A string the script compares against (`s.tab == "details"`) that matches
/// a tab's value only up to case: that tab would show nothing of its own.
fn miskeyed_tab(source: &str, choices: &[(String, Value)]) -> Result<(), VisualError> {
    let values: Vec<String> = choices
        .iter()
        .filter_map(|(_, value)| value.as_string().map(|s| s.to_string_lossy()))
        .collect();
    for literal in compared_literals(source) {
        if values.contains(&literal) {
            continue;
        }
        if let Some(value) = values
            .iter()
            .find(|v| v.to_lowercase() == literal.to_lowercase())
        {
            return Err(VisualError::script(format!(
                "the script compares the tab state with \"{literal}\", but the tab is \
                 \"{value}\": on_change receives the option exactly, so that tab would show \
                 nothing"
            )));
        }
    }
    Ok(())
}

/// String literals on either side of `==` or `~=`.
fn compared_literals(source: &str) -> Vec<String> {
    let bytes = source.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(at) = source[i..].find(['"', '\'']).map(|at| at + i) {
        let quote = bytes[at];
        let Some(len) = source[at + 1..].find(quote as char) else {
            break;
        };
        let end = at + 1 + len;
        let before = source[..at].trim_end();
        let after = source[end + 1..].trim_start();
        let compared = |s: &str| s.ends_with("==") || s.ends_with("~=");
        if compared(before) || after.starts_with("==") || after.starts_with("~=") {
            out.push(source[at + 1..end].to_owned());
        }
        i = end + 1;
    }
    out
}

/// What a reader sees with one tabs control's selection taken out: the tree
/// and every canvas and scene drawing.
#[derive(PartialEq)]
struct View {
    tree: Node,
    drawings: Vec<Drawing>,
}

#[derive(PartialEq)]
enum Drawing {
    Canvas(Vec<DrawCmd>),
    Scene(Scene),
}

fn view_of(visual: &mut Visual, tree: &Node, tabs: &[usize]) -> Result<View, VisualError> {
    fn drawings(
        visual: &mut Visual,
        node: &Node,
        out: &mut Vec<Drawing>,
    ) -> Result<(), VisualError> {
        match &node.kind {
            NodeKind::Canvas(canvas) => out.push(Drawing::Canvas(visual.draw_canvas(
                canvas.draw,
                640.0,
                canvas.height,
                0.0,
                None,
            )?)),
            NodeKind::Scene(scene) => out.push(Drawing::Scene(visual.draw_scene(scene.draw, 0.0)?)),
            _ => {}
        }
        for child in children(node) {
            drawings(visual, child, out)?;
        }
        Ok(())
    }
    let mut out = Vec::new();
    drawings(visual, tree, &mut out)?;
    let mut tree = tree.clone();
    if let Some(NodeKind::Tabs { value, .. }) = node_at_mut(&mut tree, tabs).map(|n| &mut n.kind) {
        value.clear();
    }
    Ok(View {
        tree,
        drawings: out,
    })
}

fn count_nodes(node: &Node) -> usize {
    1 + children(node).map(count_nodes).sum::<usize>()
}

fn tabs_paths(node: &Node, path: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
    if matches!(node.kind, NodeKind::Tabs { .. }) {
        out.push(path.clone());
    }
    for (ix, child) in children(node).enumerate() {
        path.push(ix);
        tabs_paths(child, path, out);
        path.pop();
    }
}

fn node_at<'a>(mut node: &'a Node, path: &[usize]) -> Option<&'a Node> {
    for ix in path {
        node = children(node).nth(*ix)?;
    }
    Some(node)
}

fn node_at_mut<'a>(mut node: &'a mut Node, path: &[usize]) -> Option<&'a mut Node> {
    for ix in path {
        node = match &mut node.kind {
            NodeKind::Stack(stack) => stack.children.get_mut(*ix)?,
            NodeKind::Grid { children, .. } => children.get_mut(*ix)?,
            NodeKind::Table(table) => table.rows.iter_mut().flatten().nth(*ix)?,
            _ => return None,
        };
    }
    Some(node)
}

fn exercise_draws(
    visual: &mut Visual,
    node: &Node,
    report: &mut Report,
) -> Result<(), VisualError> {
    report.nodes += 1;
    match &node.kind {
        NodeKind::Canvas(canvas) => {
            let frames: &[f64] = if canvas.animate { &[0.0, 1.5] } else { &[0.0] };
            for time in frames {
                visual
                    .draw_canvas(
                        canvas.draw,
                        640.0,
                        canvas.height,
                        *time,
                        Some(Pointer {
                            x: 320.0,
                            y: canvas.height / 2.0,
                            down: false,
                        }),
                    )
                    .map_err(|e| VisualError::script(format!("in a canvas draw: {}", e.message)))?;
            }
        }
        NodeKind::Scene(scene) => {
            let drawn = visual
                .draw_scene(scene.draw, 0.0)
                .map_err(|e| VisualError::script(format!("in a scene draw: {}", e.message)))?;
            if drawn
                .project(
                    Camera {
                        yaw: scene.yaw,
                        pitch: scene.pitch,
                        zoom: scene.zoom,
                    },
                    640.0,
                    scene.height,
                )
                .prims
                .is_empty()
                && drawn.sprites().is_empty()
            {
                return Err(VisualError::script(
                    "a scene draw added no geometry (use s:box, s:sphere, s:surface, s:mesh, \
                     s:line, s:point, s:model, s:sprite)",
                ));
            }
            for texture in drawn.textures() {
                if let TextureData::Encoded(bytes) = &texture.data {
                    crate::image::dimensions(bytes).map_err(|e| {
                        VisualError::script(format!(
                            "in a scene draw: {}: {e}",
                            texture.key.trim_start_matches("asset:")
                        ))
                    })?;
                }
            }
        }
        NodeKind::Shader(shader) => {
            let names: Vec<String> = shader.inputs.iter().map(|(n, _)| n.clone()).collect();
            for (i, buffer) in shader.buffers.iter().enumerate() {
                crate::shader::validate(buffer, &names)
                    .map_err(|e| VisualError::script(format!("in shader buffer {}: {e}", i + 1)))?;
            }
            crate::shader::validate(&shader.code, &names).map_err(VisualError::script)?;
            for name in &shader.textures {
                visual
                    .assets
                    .require(name)
                    .and_then(|bytes| {
                        crate::image::dimensions(&bytes).map_err(|e| format!("'{name}': {e}"))
                    })
                    .map_err(|e| VisualError::script(format!("shader textures: {e}")))?;
            }
        }
        _ => {}
    }
    for child in children(node) {
        exercise_draws(visual, child, report)?;
    }
    Ok(())
}

/// Child nodes in render order, including table cells.
pub fn children(node: &Node) -> Box<dyn Iterator<Item = &Node> + '_> {
    match &node.kind {
        NodeKind::Stack(stack) => Box::new(stack.children.iter()),
        NodeKind::Grid { children, .. } => Box::new(children.iter()),
        NodeKind::Table(table) => Box::new(table.rows.iter().flatten()),
        _ => Box::new(std::iter::empty()),
    }
}

type Interaction = (String, Vec<usize>, HandlerArg);

fn collect_interactions(node: &Node, out: &mut Vec<Interaction>) {
    fn walk(node: &Node, path: &mut Vec<usize>, out: &mut Vec<Interaction>) {
        match &node.kind {
            NodeKind::Tabs { options, value, .. } => {
                for option in options.iter().filter(|o| *o != value) {
                    out.push((
                        format!("selecting tab '{option}'"),
                        path.clone(),
                        HandlerArg::Text(option.clone()),
                    ));
                }
                out.push((
                    format!("selecting tab '{value}'"),
                    path.clone(),
                    HandlerArg::Text(value.clone()),
                ));
            }
            NodeKind::Toggle { label, value, .. } => {
                out.push((
                    format!("switching '{label}'"),
                    path.clone(),
                    HandlerArg::Bool(!value),
                ));
            }
            NodeKind::Slider {
                label, min, max, ..
            } => {
                let name = label.as_deref().unwrap_or("a slider");
                out.push((
                    format!("moving {name} to its maximum"),
                    path.clone(),
                    HandlerArg::Number(*max),
                ));
                out.push((
                    format!("moving {name} to its minimum"),
                    path.clone(),
                    HandlerArg::Number(*min),
                ));
            }
            NodeKind::Button { label, .. } => {
                out.push((
                    format!("pressing '{label}'"),
                    path.clone(),
                    HandlerArg::None,
                ));
            }
            NodeKind::Stack(Stack {
                on_click: Some(_), ..
            }) => {
                out.push(("clicking a row".into(), path.clone(), HandlerArg::None));
            }
            _ => {}
        }
        for (ix, child) in children(node).enumerate() {
            path.push(ix);
            walk(child, path, out);
            path.pop();
        }
    }
    walk(node, &mut Vec::new(), out);
}

fn handler_at(node: &Node, path: &[usize]) -> Option<HandlerId> {
    match &node_at(node, path)?.kind {
        NodeKind::Tabs { on_change, .. }
        | NodeKind::Toggle { on_change, .. }
        | NodeKind::Slider { on_change, .. } => Some(*on_change),
        NodeKind::Button { on_click, .. } => Some(*on_click),
        NodeKind::Stack(Stack { on_click, .. }) => *on_click,
        _ => None,
    }
}

/// A stable seed from a visual's id.
pub fn seed_for(id: &str) -> u64 {
    id.bytes().fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

#[cfg(test)]
mod tests;
