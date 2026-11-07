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
//! A VM the capability gate turned away never reaches that function, so its
//! error is zgx's capability name rather than the host's sentence. The SDK
//! detects that refusal and asks `plugin.<id>.imgui_refused` — exposed
//! without the capability — for the sentence: it is recorded once for the
//! plugin's diagnostics and raised in its place.
//!
//! The frame pump's widget ops are here too: the pump opens a frame and asks
//! the plugin for it through `plugin.<id>.calls`; the plugin's SDK draws with
//! [`Windows`] through `text` and `button`, and closes the frame with
//! `plugin.<id>.imgui_end`. A widget the host refuses is reported once,
//! naming the widget, and handed back to the plugin as an error at its own
//! line.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use zgx_script::{ScriptHost, Shared, Value};

use crate::plugins::{Item, imgui::permission};

use super::frame::FrameCalls;
use super::window::{CloseReason, WindowSpec, Windows, size_argument};

/// The ops this layer answers. The pump's own (`render`, `frame_end`) and
/// the widgets sit beside the window controls.
const WINDOW_OPS: &[&str] = &[
    "show",
    "hide",
    "focus",
    "minimize",
    "restore",
    "maximize",
    "toggle_maximize",
    "is_maximized",
    "begin_drag",
    "begin_resize",
    "set_title",
    "set_size",
    "close",
    "is_open",
    "state",
    "invalidate",
    "render",
];

/// The widgets a plugin may draw, applied to the open frame synchronously.
const WIDGET_OPS: &[&str] = &[
    "text",
    "text_colored",
    "separator",
    "button",
    "small_button",
    "checkbox",
    "radio_button",
    "input_text",
    "input_text_multiline",
    "input_int",
    "input_float",
    "slider_int",
    "slider_float",
    "drag_int",
    "drag_float",
    "progress_bar",
    "combo",
    "selectable",
    "bullet_text",
    "image",
    "paint",
    "invisible_button",
    "is_item_hovered",
    "is_item_active",
    "mouse_delta",
    "is_mouse_dragging",
    "same_line",
    "new_line",
    "spacing",
    "dummy",
    "indent",
    "unindent",
    "begin_child",
    "end_child",
    "columns",
    "next_column",
    "begin_group",
    "end_group",
    "set_next_item_width",
    "begin_table",
    "table_setup_column",
    "table_headers_row",
    "table_next_row",
    "table_next_column",
    "table_sort_specs",
    "end_table",
    "begin_tab_bar",
    "begin_tab_item",
    "end_tab_item",
    "end_tab_bar",
    "begin_menu_bar",
    "end_menu_bar",
    "begin_menu",
    "end_menu",
    "menu_item",
    "open_popup",
    "begin_popup",
    "begin_popup_modal",
    "begin_popup_context_item",
    "end_popup",
    "set_tooltip",
    "tree_node",
    "tree_pop",
    "collapsing_header",
    "begin_list_box",
    "end_list_box",
    "color_edit3",
    "color_button",
    "begin_drag_drop_source",
    "set_drag_drop_payload",
    "end_drag_drop_source",
    "begin_drag_drop_target",
    "accept_drag_drop_payload",
    "end_drag_drop_target",
    "shortcut",
    "date_picker",
    "time_picker",
];

/// Ends the frame the render callback was given.
const FRAME_END: &str = "frame_end";

/// The ops that name no window: the plugin's own look and its pictures.
const PLUGIN_OPS: &[&str] = &["style", "set_style", "reset_style", "texture", "font"];

/// What the host keeps of one plugin's windows, as the action closes over it.
pub struct ImGuiWindows {
    windows: Rc<Windows>,
    plugin: String,
    item: Rc<dyn Fn() -> Option<Item>>,
    /// Told each refusal of a widget call, naming the widget and what was
    /// wrong: the app reports it once per distinct sentence.
    refused: Rc<dyn Fn(&str)>,
}

impl ImGuiWindows {
    pub fn new(
        windows: Rc<Windows>,
        plugin: impl Into<String>,
        item: Rc<dyn Fn() -> Option<Item>>,
        refused: impl Fn(&str) + 'static,
    ) -> Rc<Self> {
        Rc::new(Self {
            windows,
            plugin: plugin.into(),
            item,
            refused: Rc::new(refused),
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
    frames: Option<FrameCalls>,
    refused: impl Fn(&str) + 'static,
) {
    windows.listen(id, closed);
    if let Some(frames) = frames {
        windows.listen_calls(id, frames);
    }
    // Where this plugin's own files are, for `imgui.texture` and
    // `imgui.font`: the folder its scripts were loaded from.
    if let Some(folder) =
        item().and_then(|item| item.json["snapshot"].as_str().map(std::path::PathBuf::from))
    {
        windows.set_root(id, folder);
    }
    let name = format!("plugin.{id}.imgui");
    // The VM layer: a plugin not granted `imgui.windows` cannot even bind,
    // invoke or call this action.
    permission::expose_action(host, name.clone());
    let plugin = id.to_string();
    let refusal = Rc::new(refused);
    host.function(name, {
        let windows = windows.clone();
        let (item, refused) = (item.clone(), refusal.clone());
        move |arg| {
            let action = ImGuiWindows::new(windows.clone(), plugin.clone(), item.clone(), {
                let refused = refused.clone();
                move |sentence: &str| refused(sentence)
            });
            action.call(&arg)
        }
    });
    // The end-of-frame action, behind the same capability: it closes the
    // frame the pump opened and tells it what the render callback raised.
    let end = format!("plugin.{id}.imgui_end");
    permission::expose_action(host, end.clone());
    host.action(end, {
        let plugin = id.to_string();
        let (windows, item, refused) = (windows.clone(), item.clone(), refusal.clone());
        move |arg: Value| {
            let action = ImGuiWindows::new(windows.clone(), plugin.clone(), item.clone(), {
                let refused = refused.clone();
                move |sentence: &str| refused(sentence)
            });
            let _ = action.frame_end(&arg);
        }
    });
    // What the SDK asks for when the VM's own capability gate refused the
    // call before this host was reached: the sentence the person needs, in
    // the host's words, recorded once for diagnostics.
    //
    // It is deliberately exposed without the capability — nothing here can
    // create a window or draw, and a VM the gate turned away could not hear
    // the host's sentence otherwise. The sentence is the host's own, never
    // text the plugin chose, so a plugin can neither soften nor fake it.
    let denied = format!("plugin.{id}.imgui_refused");
    host.expose_action(denied.clone(), None);
    host.function(denied, {
        let (item, refused) = (item.clone(), refusal.clone());
        move |_| {
            let sentence = permission::open_window_refusal(item().as_ref());
            refused(&sentence);
            Value::record([
                ("ok", Value::Bool(false)),
                ("error", Value::from(sentence.as_str())),
            ])
        }
    });
}

/// A refusal reporter that says each distinct sentence once: a plugin that
/// retries a refused call adds one error to its diagnostics, not one per
/// attempt (`Bridge::widget_refused` dedupes the installed path the same
/// way; this is what a dev plugin's log gets).
pub fn once(report: impl Fn(&str) + 'static) -> impl Fn(&str) + 'static {
    let seen: Rc<RefCell<HashSet<String>>> = Rc::default();
    move |sentence: &str| {
        if seen.borrow_mut().insert(sentence.to_string()) {
            report(sentence);
        }
    }
}

impl ImGuiWindows {
    fn call(&self, arg: &Value) -> Value {
        let Some(op) = text(arg, "op") else {
            return failed("zeron.imgui needs an op, like \"open_window\" or \"show\"");
        };
        if op == "open_window" {
            return self.open_window(arg);
        }
        if WIDGET_OPS.contains(&op.as_str()) {
            return self.widget(&op, arg);
        }
        if PLUGIN_OPS.contains(&op.as_str()) {
            return self.plugin_op(&op, arg);
        }
        if op == FRAME_END {
            return self.frame_end(arg);
        }
        if !WINDOW_OPS.contains(&op.as_str()) {
            return failed(&format!(
                "zeron.imgui has no op \"{op}\". A window has show, hide, focus, minimize, restore, maximize, toggle_maximize, is_maximized, begin_drag, begin_resize, set_title, set_size, close, is_open and state; a render callback has text, text_colored, separator, button, small_button, checkbox, radio_button, input_text, input_text_multiline, input_int, input_float, slider_int, slider_float, drag_int, drag_float, progress_bar, combo, selectable, bullet_text, invisible_button, is_item_hovered, is_item_active, mouse_delta, is_mouse_dragging, same_line, new_line, spacing, dummy, indent, unindent, begin_child, end_child, columns, next_column, begin_group, end_group, set_next_item_width, begin_table, table_setup_column, table_headers_row, table_next_row, table_next_column, table_sort_specs, end_table, begin_tab_bar, begin_tab_item, end_tab_item, end_tab_bar, begin_menu_bar, end_menu_bar, begin_menu, end_menu, menu_item, open_popup, begin_popup, begin_popup_modal, begin_popup_context_item, end_popup, set_tooltip, tree_node, tree_pop, collapsing_header, begin_list_box, end_list_box, color_edit3, color_button, begin_drag_drop_source, set_drag_drop_payload, end_drag_drop_source, begin_drag_drop_target, accept_drag_drop_payload, end_drag_drop_target, shortcut, date_picker and time_picker."
            ));
        }
        if let Err(error) = permission::allowed(self.item().as_ref(), "use its native windows") {
            return self.refuse(error);
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
            "focus" => answer(window.focus_window()),
            "minimize" => answer(window.minimize()),
            "restore" => answer(window.restore()),
            "maximize" => answer(window.maximize()),
            "toggle_maximize" => answer(window.toggle_maximize()),
            "is_maximized" => Value::record([
                ("ok", Value::Bool(true)),
                ("maximized", Value::Bool(window.is_maximized())),
            ]),
            "begin_drag" => answer(window.begin_drag()),
            "begin_resize" => match arg.field("edge") {
                Some(Value::Str(edge)) => answer(window.begin_resize(Some(edge))),
                Some(_) => failed(
                    "zeron.imgui.begin_resize can't use edge: edge must be text, like \"se\"",
                ),
                None => answer(window.begin_resize(None)),
            },
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
            // The plugin has an on_render: from now on the pump asks this
            // window for frames while it is shown.
            "render" => {
                let on = arg.field("on").is_none_or(Value::truthy);
                window.set_renderable(on);
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
                    ("maximized", Value::Bool(window.is_maximized())),
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
            return self.refuse(error);
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

    /// Applies one widget call from the plugin's render callback to the open
    /// frame. The refusal reaches the plugin as `ui.text` raising at its
    /// line; nothing is drawn and nothing crashes.
    fn widget(&self, op: &str, arg: &Value) -> Value {
        if let Err(error) = permission::allowed(self.item().as_ref(), "draw in its plugin windows")
        {
            return self.refuse(error);
        }
        let Some(id) = text(arg, "window") else {
            return self.refuse(format!("ui.{op} needs the window it belongs to"));
        };
        let Some(seq) = number(arg, "seq") else {
            return self.refuse(format!("ui.{op} needs the frame it belongs to"));
        };
        let Some(window) = self.windows.window(&self.plugin, &id) else {
            return self.refuse(format!(
                "ui.{op} was called after window \"{id}\" was closed. Its ui does nothing once the window is gone."
            ));
        };
        match window.widget(op, seq as u64, arg) {
            Ok(answer) => answer,
            Err(error) => self.refuse(error),
        }
    }

    /// The plugin's own look and pictures: `imgui.style`, `imgui.set_style`,
    /// `imgui.reset_style`, `imgui.texture` and `imgui.font`. None of them
    /// names a window or draws anything; every one passes the same gate.
    fn plugin_op(&self, op: &str, arg: &Value) -> Value {
        if let Err(error) = permission::allowed(self.item().as_ref(), "use its native windows") {
            return self.refuse(error);
        }
        let style = self.windows.style(&self.plugin);
        match op {
            "style" => super::style::view(&style.resolve(), self.windows.scale()),
            "set_style" => match style.set(arg) {
                Ok(()) => Value::record([("ok", Value::Bool(true))]),
                Err(error) => self.refuse(error),
            },
            "reset_style" => {
                style.reset();
                Value::record([("ok", Value::Bool(true))])
            }
            "texture" => {
                let Some(path) = arg.field("path").map(crate::auth::text) else {
                    return self.refuse(
                        "imgui.texture needs a path, like imgui.texture{ path = \"assets/icon.png\" }."
                            .into(),
                    );
                };
                let textures = self.windows.textures(&self.plugin);
                match textures.register(style.root(), &path) {
                    Ok(image) => Value::record([
                        ("ok", Value::Bool(true)),
                        ("id", Value::from(image.id as f64)),
                        ("width", Value::from(image.width as f64)),
                        ("height", Value::from(image.height as f64)),
                    ]),
                    Err(error) => self.refuse(error),
                }
            }
            "font" => match style.load_font(arg) {
                Ok(font) => Value::record([
                    ("ok", Value::Bool(true)),
                    ("id", Value::from(font.id as f64)),
                    ("path", Value::from(font.path.as_str())),
                    ("size", Value::from(f64::from(font.size))),
                ]),
                Err(error) => self.refuse(error),
            },
            _ => unreachable!("every plugin op is matched above"),
        }
    }

    /// A call the host turned down — a widget drawn outside its frame, or an
    /// ImGui call the `imgui.windows` gate refused: reported once, in the
    /// sentence the person needs, and handed back to the plugin to raise at
    /// its own line. Nothing is created or drawn.
    fn refuse(&self, error: String) -> Value {
        (self.refused)(&error);
        failed(&error)
    }

    /// Ends the frame the render callback was given, painting it or throwing
    /// it away with the reason. The pump reports what comes of it.
    fn frame_end(&self, arg: &Value) -> Value {
        if let Err(error) = permission::allowed(self.item().as_ref(), "draw in its plugin windows")
        {
            return self.refuse(error);
        }
        let Some(id) = text(arg, "window") else {
            return failed("zeron.imgui.frame_end needs the window it belongs to");
        };
        let Some(seq) = number(arg, "seq") else {
            return failed("zeron.imgui.frame_end needs the frame it ends");
        };
        let Some(window) = self.windows.window(&self.plugin, &id) else {
            return failed(&format!(
                "zeron.imgui.frame_end was called for window \"{id}\", which is closed."
            ));
        };
        let error = text(arg, "error").filter(|error| !error.is_empty());
        // A failure the widget ops caused (drawn outside the frame, or after
        // the window closed), rather than the plugin's own error: the frame
        // is dropped either way, but a refusal never takes the plugin down.
        let refused = arg.field("refused").is_some_and(Value::truthy);
        match window.end_frame(seq as u64, error, refused) {
            Ok(()) => Value::record([("ok", Value::Bool(true))]),
            // No frame of that seq is open here (it already ended, or this
            // is another window's frame): said plainly rather than dropped.
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

/// A field that is a number, if it is.
fn number(value: &Value, field: &str) -> Option<f64> {
    match value.field(field) {
        Some(Value::Num(number)) => Some(*number),
        _ => None,
    }
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

/// The frame calls of a dev plugin.
///
/// A dev plugin has no bridge: its calls live in the plugin's own
/// `plugin.<id>.calls` state, so the pump owns a list here and publishes it
/// the way the bridge does. `failed` goes to the app's log, which is what a
/// fixture run reads.
pub fn dev_calls(host: &ScriptHost, id: &str, refused: impl Fn(&str) + 'static) -> FrameCalls {
    let shared: Shared = host.shared(
        &format!("plugin.{id}.calls"),
        crate::plugins::vlist(Vec::new()),
    );
    let list: Rc<RefCell<Vec<(String, Value)>>> = Rc::default();
    // The queue is published with no borrow held: setting it runs the
    // plugin's effects, which may read the queue again.
    let publish = {
        let list = list.clone();
        let shared = shared.clone();
        move || {
            let value = {
                let items = list.borrow();
                crate::plugins::vlist(items.iter().map(|(_, call)| call.clone()).collect())
            };
            shared.set(value);
        }
    };
    let push = {
        let list = list.clone();
        let publish = publish.clone();
        move |call: String, kind: &'static str, fields: Vec<(&'static str, Value)>| {
            let mut record = vec![
                ("id", Value::from(call.as_str())),
                ("kind", Value::from(kind)),
            ];
            record.extend(fields);
            list.borrow_mut().push((call, Value::record(record)));
            publish();
        }
    };
    let take = {
        let list = list.clone();
        move |call: &str| {
            let removed = {
                let mut items = list.borrow_mut();
                let before = items.len();
                items.retain(|(id, _)| id != call);
                items.len() != before
            };
            if removed {
                publish();
            }
        }
    };
    let failed = {
        let plugin = id.to_string();
        move |error: &str| {
            // The pump's sentence already names the window and the script's
            // own words; the dev route has no Settings row to put it on.
            eprintln!("zeron-zgx: dev plugin {plugin}: {error}");
        }
    };
    FrameCalls::new(push, take, failed).with_refused(refused)
}

/// The close of a dev plugin's window, as its `on_close` hears it.
///
/// A dev plugin has no bridge, so this is the installed path's
/// `{ kind = "imgui.close", window }` pushed through the same
/// `plugin.<id>.calls` queue the frame pump uses ([`dev_calls`]): the SDK's
/// effect loop runs the plugin's `on_close` at once and marks every handle
/// for that window closed, exactly as it does for an installed plugin.
///
/// The SDK answers a call synchronously, so the entry is dropped again
/// before this returns: the queue does not grow over a session, and a
/// window id reused after a reopen is a fresh call rather than one the SDK
/// believes it has already run.
fn dev_close(calls: FrameCalls) -> impl Fn(&str) + 'static {
    move |window: &str| {
        let call = format!("imgui-close-{window}");
        calls.push(
            call.clone(),
            "imgui.close",
            vec![("window", Value::from(window))],
        );
        calls.take(&call);
    }
}

/// What `serve` takes for a dev plugin: the queue its frames and its
/// closes go through, and the close handler that queues them.
///
/// One [`dev_calls`] serves both, so a closed window is the same channel
/// as a frame — the dev plugin's own `plugin.<id>.calls` — rather than the
/// stderr line it used to be alone (which stays, for a native probe to
/// read).
pub fn dev_listen(
    host: &ScriptHost,
    id: &str,
    refused: impl Fn(&str) + 'static,
) -> (FrameCalls, impl Fn(&str) + 'static) {
    let calls = dev_calls(host, id, refused);
    let closed = dev_close(calls.clone());
    let plugin = id.to_string();
    let handler = move |window: &str| {
        eprintln!("zeron-zgx: dev plugin {plugin}: window {window} closed");
        closed(window);
    };
    (calls, handler)
}
