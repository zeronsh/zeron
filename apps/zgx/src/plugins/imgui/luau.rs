//! The window half of the `zeron.imgui` host action.
//!
//! One host function serves a plugin's windows: `plugin.<id>.imgui`, called
//! with `ui.call_sync` and a table naming an `op`. It is exposed behind the
//! `imgui.windows` capability ([`permission::expose_action`]), and every call
//! passes the per-plugin gate again ([`permission::open_window`] for the one
//! that creates a window, [`permission::allowed`] for the rest), so a VM
//! loaded before a grant was taken away is refused the next time it asks.
//!
//! The function answers `{ ok = true, … }` or `{ ok = false, error = "…" }`
//! rather than raising: `ui.call_sync` has no error channel, and the SDK
//! turns `ok = false` into a sentence-style Luau error at the plugin's line.
//! A refusal never creates a window and never takes the plugin down; that is
//! the plugin author's fix, not a crash.
//!
//! The frame pump and the widget ops are a later layer: they call
//! [`Windows`] directly for frames and add their ops here.

use std::rc::Rc;

use zgx_script::{ScriptHost, Value};

use crate::plugins::{Item, imgui::permission};

use super::window::{CloseReason, WindowSpec, Windows, size_argument};

/// The ops this layer answers. The pump adds its own beside these.
const WINDOW_OPS: &[&str] = &[
    "show",
    "hide",
    "focus",
    "minimize",
    "restore",
    "set_title",
    "set_size",
    "close",
    "is_open",
    "state",
    "invalidate",
];

/// What the host keeps of one plugin's windows, as the action closes over it.
pub struct ImGuiWindows {
    windows: Rc<Windows>,
    plugin: String,
    item: Rc<dyn Fn() -> Option<Item>>,
}

impl ImGuiWindows {
    pub fn new(
        windows: Rc<Windows>,
        plugin: impl Into<String>,
        item: Rc<dyn Fn() -> Option<Item>>,
    ) -> Rc<Self> {
        Rc::new(Self {
            windows,
            plugin: plugin.into(),
            item,
        })
    }
}

/// Serves `plugin`'s window action, and tells it when one of its windows
/// closes (`closed`, called with the window id, once per window).
///
/// `plugin`'s listing is looked up on every call: what the engine allows now
/// decides, not what was allowed when the script loaded.
pub fn serve(
    host: &ScriptHost,
    id: &str,
    windows: Rc<Windows>,
    item: Rc<dyn Fn() -> Option<Item>>,
    closed: impl Fn(&str) + 'static,
) {
    windows.listen(id, closed);
    let name = format!("plugin.{id}.imgui");
    // The VM layer: a plugin not granted `imgui.windows` cannot even bind,
    // invoke or call this action.
    permission::expose_action(host, name.clone());
    let plugin = id.to_string();
    host.function(name, move |arg| {
        let action = ImGuiWindows::new(windows.clone(), plugin.clone(), item.clone());
        action.call(&arg)
    });
}

impl ImGuiWindows {
    fn call(&self, arg: &Value) -> Value {
        let Some(op) = text(arg, "op") else {
            return failed("zeron.imgui needs an op, like \"open_window\" or \"show\"");
        };
        if op == "open_window" {
            return self.open_window(arg);
        }
        if !WINDOW_OPS.contains(&op.as_str()) {
            return failed(&format!(
                "zeron.imgui has no op \"{op}\". A window has show, hide, focus, minimize, restore, set_title, set_size, close, is_open and state."
            ));
        }
        if let Err(error) = permission::allowed(self.item().as_ref(), "use its native windows") {
            return failed(&error);
        }
        let Some(id) = text(arg, "window") else {
            return failed("zeron.imgui needs a window id, like \"inspector\"");
        };
        let Some(window) = self.windows.window(&self.plugin, &id) else {
            return failed(&format!(
                "This plugin has no open window \"{id}\". Open it first with zeron.imgui.open_window."
            ));
        };
        match op.as_str() {
            "show" => answer(window.show()),
            "hide" => answer(window.hide()),
            "focus" => answer(window.focus()),
            "minimize" => answer(window.minimize()),
            "restore" => answer(window.restore()),
            "close" => answer(window.close()),
            "set_title" => match text(arg, "title") {
                Some(title) => answer(window.set_title(&title)),
                None => failed("zeron.imgui.set_title needs a title"),
            },
            "set_size" => match arg.field("size") {
                Some(size) => match size_argument(size, &id) {
                    Ok(size) => answer(window.set_size(size)),
                    Err(error) => failed(&error),
                },
                None => failed(&format!(
                    "zeron.imgui.set_size needs a size for window \"{id}\""
                )),
            },
            "is_open" => Value::record([
                ("ok", Value::Bool(true)),
                ("open", Value::Bool(window.is_open())),
            ]),
            // Asks for one frame now: a window that is shown draws it, a
            // hidden or minimized one stays where it was.
            "invalidate" => {
                window.request_frame();
                Value::record([("ok", Value::Bool(true))])
            }
            "state" => {
                let (width, height) = window.size();
                Value::record([
                    ("ok", Value::Bool(true)),
                    ("id", Value::from(window.id())),
                    ("open", Value::Bool(window.is_open())),
                    ("shown", Value::Bool(window.is_shown())),
                    ("minimized", Value::Bool(window.is_minimized())),
                    ("title", Value::from(window.title().as_str())),
                    ("width", Value::from(width)),
                    ("height", Value::from(height)),
                    ("frames", Value::from(window.frames() as f64)),
                ])
            }
            _ => unreachable!("every window op is matched above"),
        }
    }

    /// Creates the window, after the gate and after the request is checked.
    /// Nothing exists until both pass.
    fn open_window(&self, arg: &Value) -> Value {
        if let Err(error) = permission::open_window(self.item().as_ref()) {
            return failed(&error);
        }
        let spec = match WindowSpec::parse(&self.plugin, arg) {
            Ok(spec) => spec,
            Err(error) => return failed(&error),
        };
        match self.windows.open(&self.plugin, spec) {
            Ok(window) => Value::record([
                ("ok", Value::Bool(true)),
                ("id", Value::from(window.id())),
                ("open", Value::Bool(true)),
            ]),
            Err(error) => failed(&error),
        }
    }

    fn item(&self) -> Option<Item> {
        (self.item)()
    }
}

/// `{ ok = true }`, or the refusal when `result` is `Err`.
fn answer(result: Result<(), String>) -> Value {
    match result {
        Ok(()) => Value::record([("ok", Value::Bool(true))]),
        Err(error) => failed(&error),
    }
}

/// `{ ok = false, error = "…" }`.
fn failed(error: &str) -> Value {
    Value::record([("ok", Value::Bool(false)), ("error", Value::from(error))])
}

/// A field that is text, if it is.
fn text(value: &Value, field: &str) -> Option<String> {
    match value.field(field) {
        Some(Value::Str(text)) => Some(text.to_string()),
        _ => None,
    }
}

/// The close reason a plugin's own `close` and the OS chrome use, for the
/// pump to share.
pub const PLUGIN_CLOSED: CloseReason = CloseReason::Closed;
