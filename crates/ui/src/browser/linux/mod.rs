//! WebKitGTK renders offscreen in an isolated helper process. GPUI composites
//! its frames, so browser content uses the same clipping and blur as other UI.
use super::model::{PageFailure, PageState, Presentation};
use crate::i18n::{self, MessageId};
use gpui::{Bounds, Pixels, RenderImage};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::{Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicU32, Ordering},
    },
};
use tokio::sync::mpsc::Sender;

#[derive(Clone, Debug)]
pub enum NativeEvent {
    Changed,
    Frame,
    NewTab(String),
    Clipboard(String),
    Menu(Value),
}

#[derive(Clone, Default)]
pub struct BrowserData(Arc<Mutex<Weak<Worker>>>);

struct Worker {
    child: Mutex<Child>,
    stdin: Mutex<ChildStdin>,
    routes: Arc<Mutex<HashMap<u32, Weak<Route>>>>,
    next_id: AtomicU32,
}
impl Drop for Worker {
    fn drop(&mut self) {
        if let Ok(child) = self.child.get_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
struct Route {
    tx: Sender<NativeEvent>,
    state: Mutex<PageState>,
    frame: Mutex<Option<(Arc<RenderImage>, f32)>>,
    input: Mutex<Value>,
    #[cfg(feature = "browser-fixture")]
    evaluation: Mutex<Option<Value>>,
}

fn helper_path() -> Result<std::path::PathBuf, PageFailure> {
    use sha2::{Digest, Sha256};
    const HELPER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/zeron-webkit"));
    let hash = format!("{:x}", Sha256::digest(HELPER));
    let root = std::env::var_os("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| std::path::PathBuf::from(p).join(".cache")))
        .ok_or_else(|| PageFailure::message(MessageId::BrowserCacheDirUnavailable))?
        .join("zeron/browser");
    std::fs::create_dir_all(&root).map_err(|e| PageFailure::detail(e.to_string()))?;
    let path = root.join(format!("webkit-{hash}"));
    if std::fs::read(&path).ok().as_deref() != Some(HELPER) {
        let temp = root.join(format!(".webkit-{}", std::process::id()));
        let result = (|| -> std::io::Result<()> {
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o700)
                .open(&temp)?;
            f.write_all(HELPER)?;
            f.sync_all()?;
            std::fs::rename(&temp, &path)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        result.map_err(|e| PageFailure::detail(e.to_string()))?;
    }
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| PageFailure::detail(e.to_string()))?;
    Ok(path)
}
impl BrowserData {
    fn worker(&self) -> Result<Arc<Worker>, PageFailure> {
        let mut current = self.0.lock().unwrap();
        if let Some(worker) = current.upgrade() {
            if worker
                .child
                .lock()
                .unwrap()
                .try_wait()
                .map_err(|e| PageFailure::detail(e.to_string()))?
                .is_none()
            {
                return Ok(worker);
            }
        }
        let mut child = Command::new(helper_path()?)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| {
                PageFailure::message(MessageId::BrowserWebkitStartFailed)
                    .with("{error}", e.to_string())
            })?;
        let stdin = child.stdin.take().unwrap();
        let mut stdout = child.stdout.take().unwrap();
        let routes: Arc<Mutex<HashMap<u32, Weak<Route>>>> = Arc::default();
        let reader_routes = routes.clone();
        std::thread::Builder::new()
            .name("browser-frames".into())
            .spawn(move || {
                let result = (|| -> std::io::Result<()> {
                    loop {
                        let mut header = [0u8; 9];
                        stdout.read_exact(&mut header)?;
                        let id = u32::from_le_bytes(header[1..5].try_into().unwrap());
                        let length = u32::from_le_bytes(header[5..9].try_into().unwrap()) as usize;
                        if length > 8192 * 8192 * 4 + 12 {
                            return Err(std::io::Error::other("Browser packet is too large"));
                        }
                        let mut data = vec![0; length];
                        stdout.read_exact(&mut data)?;
                        let route = reader_routes
                            .lock()
                            .unwrap()
                            .get(&id)
                            .and_then(Weak::upgrade);
                        let Some(route) = route else {
                            continue;
                        };
                        let event = match header[0] {
                            b'F' => {
                                if data.len() < 12 {
                                    continue;
                                }
                                let width = u32::from_le_bytes(data[..4].try_into().unwrap());
                                let height = u32::from_le_bytes(data[4..8].try_into().unwrap());
                                let scale = f32::from_bits(u32::from_le_bytes(
                                    data[8..12].try_into().unwrap(),
                                ));
                                if !scale.is_finite() || !(0.5..=4.).contains(&scale) {
                                    continue;
                                }
                                data.drain(..12);
                                let Some(pixels) = image::RgbaImage::from_raw(width, height, data)
                                else {
                                    continue;
                                };
                                *route.frame.lock().unwrap() = Some((
                                    Arc::new(RenderImage::new([image::Frame::new(pixels)])),
                                    scale,
                                ));
                                NativeEvent::Frame
                            }
                            b'I' => {
                                if let Ok(Value::Object(update)) =
                                    serde_json::from_slice::<Value>(&data)
                                {
                                    let mut input = route.input.lock().unwrap();
                                    if !input.is_object() {
                                        *input = json!({});
                                    }
                                    for (key, value) in update {
                                        input[&key] = value;
                                    }
                                }
                                NativeEvent::Frame
                            }
                            b'S' => {
                                let Ok(state) = serde_json::from_slice(&data) else {
                                    continue;
                                };
                                *route.state.lock().unwrap() = state;
                                NativeEvent::Changed
                            }
                            b'M' => {
                                let Ok(menu) = serde_json::from_slice(&data) else {
                                    continue;
                                };
                                NativeEvent::Menu(menu)
                            }
                            b'C' => {
                                NativeEvent::Clipboard(String::from_utf8_lossy(&data).into_owned())
                            }
                            b'N' => {
                                NativeEvent::NewTab(String::from_utf8_lossy(&data).into_owned())
                            }
                            #[cfg(feature = "browser-fixture")]
                            b'J' => {
                                *route.evaluation.lock().unwrap() =
                                    serde_json::from_slice(&data).ok();
                                continue;
                            }
                            _ => continue,
                        };
                        // At most one latest frame is retained per page. A busy UI
                        // never accumulates video frames or blocks the engine.
                        match event {
                            NativeEvent::NewTab(_)
                            | NativeEvent::Clipboard(_)
                            | NativeEvent::Menu(_) => {
                                let _ = route.tx.blocking_send(event);
                            }
                            _ => {
                                let _ = route.tx.try_send(event);
                            }
                        }
                    }
                })();
                if result.is_err() {
                    for route in reader_routes
                        .lock()
                        .unwrap()
                        .values()
                        .filter_map(Weak::upgrade)
                    {
                        let mut state = route.state.lock().unwrap();
                        state.loading = false;
                        state.error = Some(PageFailure::message(MessageId::BrowserHelperStopped));
                        drop(state);
                        let _ = route.tx.try_send(NativeEvent::Changed);
                    }
                }
            })
            .map_err(|e| PageFailure::detail(e.to_string()))?;
        let worker = Arc::new(Worker {
            child: Mutex::new(child),
            stdin: Mutex::new(stdin),
            routes,
            next_id: AtomicU32::new(1),
        });
        *current = Arc::downgrade(&worker);
        Ok(worker)
    }
}
impl Worker {
    fn send(&self, id: u32, mut command: Value) -> Result<(), PageFailure> {
        command["id"] = id.into();
        let data = serde_json::to_vec(&command).map_err(|e| PageFailure::detail(e.to_string()))?;
        let mut pipe = self.stdin.lock().unwrap();
        pipe.write_all(&(data.len() as u32).to_le_bytes())
            .and_then(|_| pipe.write_all(&data))
            .map_err(|e| PageFailure::detail(e.to_string()))
    }
}

pub struct NativePage {
    worker: Arc<Worker>,
    route: Arc<Route>,
    id: u32,
    pub bounds: Bounds<Pixels>,
    pub scale: f32,
    geometry: Option<(u32, u32, u32)>,
    presentation: Presentation,
    pub image: Option<Arc<RenderImage>>,
    pub image_scale: f32,
    pub menu: Option<Value>,
    pub menu_active: usize,
    preedit: String,
    pub pressed: std::cell::Cell<Option<gpui::MouseButton>>,
}
impl NativePage {
    pub fn new(
        _: &gpui::Window,
        data: &BrowserData,
        tx: Sender<NativeEvent>,
    ) -> Result<Self, PageFailure> {
        let worker = data.worker()?;
        let id = worker.next_id.fetch_add(1, Ordering::Relaxed);
        let route = Arc::new(Route {
            tx,
            state: Mutex::default(),
            frame: Mutex::default(),
            input: Mutex::new(json!({})),
            #[cfg(feature = "browser-fixture")]
            evaluation: Mutex::default(),
        });
        worker
            .routes
            .lock()
            .unwrap()
            .insert(id, Arc::downgrade(&route));
        worker.send(id, json!({"cmd":"create"}))?;
        Ok(Self {
            worker,
            route,
            id,
            bounds: Bounds::default(),
            scale: 1.,
            geometry: None,
            presentation: Presentation::Live,
            image: None,
            image_scale: 1.,
            menu: None,
            menu_active: 0,
            preedit: String::new(),
            pressed: std::cell::Cell::new(None),
        })
    }
    pub fn load(&self, url: &str) -> Result<(), PageFailure> {
        self.worker.send(self.id, json!({"cmd":"load","url":url}))
    }
    pub fn reload(&self) {
        self.command(json!({"cmd":"reload"}));
    }
    pub fn history(&self, forward: bool) {
        self.command(json!({"cmd":if forward {"forward"} else {"back"}}));
    }
    pub fn state(&self) -> PageState {
        self.route.state.lock().unwrap().clone()
    }
    pub fn present(&mut self, presentation: Presentation) {
        if (self.presentation == Presentation::Hidden) != (presentation == Presentation::Hidden) {
            self.command(
                json!({"cmd":"visible","value":u8::from(presentation != Presentation::Hidden)}),
            );
        }
        if presentation == Presentation::Hidden && self.menu.take().is_some() {
            self.command(json!({"cmd":"dismiss-menu"}));
        }
        self.presentation = presentation;
    }
    pub fn update_frame(&mut self, window: &mut gpui::Window) {
        if let Some((frame, scale)) = self.route.frame.lock().unwrap().take() {
            self.image_scale = scale;
            if let Some(old) = self.image.replace(frame) {
                let _ = window.drop_image(old);
            }
        }
    }
    pub fn sync(&mut self, bounds: Bounds<Pixels>, scale: f32) {
        self.bounds = bounds;
        self.scale = scale;
        let geometry = (
            (f32::from(bounds.size.width) * scale)
                .round()
                .clamp(1., 8192.) as u32,
            (f32::from(bounds.size.height) * scale)
                .round()
                .clamp(1., 8192.) as u32,
            scale.to_bits(),
        );
        if self.geometry != Some(geometry) {
            self.command(
                json!({"cmd":"resize","width":geometry.0,"height":geometry.1,"scale":scale}),
            );
            self.geometry = Some(geometry);
        }
    }
    pub fn command(&self, value: Value) {
        let _ = self.worker.send(self.id, value);
    }
    #[cfg(feature = "browser-fixture")]
    pub fn evaluate(&self, script: &str) {
        *self.route.evaluation.lock().unwrap() = None;
        self.command(json!({"cmd":"eval","script":script}));
    }
    #[cfg(feature = "browser-fixture")]
    pub fn evaluation(&self) -> Option<Value> {
        self.route.evaluation.lock().unwrap().take()
    }
}
impl Drop for NativePage {
    fn drop(&mut self) {
        self.command(json!({"cmd":"close"}));
        self.worker.routes.lock().unwrap().remove(&self.id);
    }
}

impl super::BrowserSurface {
    pub(super) fn on_native_event(
        &mut self,
        event: NativeEvent,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        use gpui::Focusable;
        let Some(native) = &mut self.native else {
            return;
        };
        native.update_frame(window);
        match event {
            NativeEvent::NewTab(url) => {
                if self.presentation == Presentation::Live {
                    cx.emit(super::BrowserEvent::NewTab(Some(url)));
                }
            }
            NativeEvent::Menu(menu) => {
                if self.presentation == Presentation::Live {
                    native.menu_active = menu["items"]
                        .as_array()
                        .and_then(|items| {
                            items
                                .iter()
                                .position(|item| item["selected"].as_bool().unwrap_or(false))
                        })
                        .unwrap_or(0);
                    native.menu = Some(menu);
                } else {
                    native.command(json!({"cmd":"dismiss-menu"}));
                }
            }
            NativeEvent::Clipboard(text) => {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
            }
            NativeEvent::Frame | NativeEvent::Changed => {
                let mut page = native.state();
                if page.url.is_none() {
                    page.url = self.page.url.clone();
                }
                if page.error.is_some() {
                    page.loading = false;
                }
                if !self.address.focus_handle(cx).is_focused(window) {
                    if let Some(url) = &page.url {
                        if self.address.read(cx).text() != url {
                            self.address
                                .update(cx, |input, cx| input.set_text(url.clone(), cx));
                        }
                    }
                    self.address_edited = false;
                }
                if page != self.page {
                    self.page = page;
                    cx.emit(super::BrowserEvent::Changed);
                }
            }
        }
        cx.notify();
    }
    pub(super) fn linux_pointer(
        &self,
        kind: &str,
        position: gpui::Point<Pixels>,
        button: Option<gpui::MouseButton>,
        modifiers: gpui::Modifiers,
    ) {
        let Some(native) = &self.native else {
            return;
        };
        if self.presentation != Presentation::Live {
            return;
        }
        if kind == "down" {
            native.pressed.set(button);
        }
        if kind == "up" && native.pressed.replace(None) != button {
            return;
        }
        let position = position - native.bounds.origin;
        native.command(json!({"cmd":kind,"x":f32::from(position.x),"y":f32::from(position.y),"button":button.map(mouse_button).unwrap_or(0),"mods":modifiers_mask(modifiers) | if kind == "move" { button.map(|b| 1 << (mouse_button(b)+7)).unwrap_or(0) } else { 0 }}));
    }
    pub(super) fn linux_key(&self, stroke: &gpui::Keystroke, down: bool) {
        let Some(native) = &self.native else {
            return;
        };
        let printable = stroke.key_char.as_deref().filter(|s| {
            s.chars().count() == 1
                && !s.chars().any(char::is_control)
                && !stroke.modifiers.control
                && !stroke.modifiers.platform
        });
        let key = match printable.unwrap_or(stroke.key.as_str()) {
            "enter" => "Return",
            "backspace" => "BackSpace",
            "delete" => "Delete",
            "escape" => "Escape",
            "tab" => "Tab",
            "left" => "Left",
            "right" => "Right",
            "up" => "Up",
            "down" => "Down",
            "home" => "Home",
            "end" => "End",
            "pageup" => "Page_Up",
            "pagedown" => "Page_Down",
            "space" => "space",
            key => key,
        };
        native.command(json!({"cmd":if down {"key_down"} else {"key_up"},"key":key,"text":stroke.key_char.as_deref().unwrap_or(""),"mods":modifiers_mask(stroke.modifiers)}));
    }
}
/// The helper sends an action id with each context-menu row. Actions this UI
/// owns render their own copy; anything else is page content — a `<select>`
/// option arrives as `option:<index>` and must stay verbatim.
fn menu_label(action: &str) -> Option<MessageId> {
    Some(match action {
        "open-link" => MessageId::BrowserMenuOpenLink,
        "copy-link" => MessageId::MarkdownCopyLinkAddress,
        "copy" => MessageId::EditCopy,
        "text" => MessageId::EditPaste,
        "select-all" => MessageId::BrowserMenuSelectAll,
        "back" => MessageId::CommonBack,
        "forward" => MessageId::BrowserForward,
        "reload" => MessageId::BrowserMenuReload,
        _ => return None,
    })
}
pub(super) fn modifiers_mask(m: gpui::Modifiers) -> u32 {
    u32::from(m.shift)
        | (u32::from(m.control) << 2)
        | (u32::from(m.alt) << 3)
        | (u32::from(m.platform) << 26)
}
fn mouse_button(button: gpui::MouseButton) -> u32 {
    match button {
        gpui::MouseButton::Left => 1,
        gpui::MouseButton::Middle => 2,
        gpui::MouseButton::Right => 3,
        gpui::MouseButton::Navigate(gpui::NavigationDirection::Back) => 8,
        gpui::MouseButton::Navigate(gpui::NavigationDirection::Forward) => 9,
    }
}

#[cfg(feature = "browser-fixture")]
impl super::BrowserSurface {
    pub fn fixture_linux_menu_open(&self) -> bool {
        self.native.as_ref().is_some_and(|n| n.menu.is_some())
    }
    pub fn fixture_linux_bounds(&self) -> Bounds<Pixels> {
        self.native.as_ref().unwrap().bounds
    }
    pub fn fixture_linux_evaluation(&self) -> Option<Value> {
        self.native.as_ref().unwrap().evaluation()
    }
}

impl gpui::EntityInputHandler for super::BrowserSurface {
    fn text_for_range(
        &mut self,
        range: std::ops::Range<usize>,
        actual: &mut Option<std::ops::Range<usize>>,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) -> Option<String> {
        let native = self.native.as_ref()?;
        let input = native.route.input.lock().unwrap();
        let text = input["text"].as_str().unwrap_or("");
        let units: Vec<u16> = text.encode_utf16().collect();
        let range = range.start.min(units.len())..range.end.min(units.len());
        *actual = Some(range.clone());
        Some(String::from_utf16_lossy(units.get(range)?))
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) -> Option<gpui::UTF16Selection> {
        let native = self.native.as_ref()?;
        let input = native.route.input.lock().unwrap();
        let text = input["text"].as_str().unwrap_or("");
        let offset = |key: &str| {
            let n = input[key].as_u64().unwrap_or(0) as usize;
            text.get(..n).unwrap_or(text).encode_utf16().count()
        };
        let cursor = offset("cursor");
        let selection = offset("selection");
        Some(gpui::UTF16Selection {
            range: cursor.min(selection)..cursor.max(selection),
            reversed: cursor < selection,
        })
    }
    fn marked_text_range(
        &self,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) -> Option<std::ops::Range<usize>> {
        let native = self.native.as_ref()?;
        (!native.preedit.is_empty()).then(|| 0..native.preedit.encode_utf16().count())
    }
    fn unmark_text(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) {
        if let Some(native) = &mut self.native {
            native.command(json!({"cmd":"unmark"}));
            native.preedit.clear();
        }
    }
    fn replace_text_in_range(
        &mut self,
        _: Option<std::ops::Range<usize>>,
        text: &str,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) {
        if let Some(native) = &mut self.native {
            native.command(json!({"cmd":"commit","text":text}));
            native.preedit.clear();
        }
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<std::ops::Range<usize>>,
        text: &str,
        _: Option<std::ops::Range<usize>>,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) {
        if let Some(native) = &mut self.native {
            native.command(json!({"cmd":"preedit","text":text}));
            native.preedit = text.to_owned();
        }
    }
    fn bounds_for_range(
        &mut self,
        _: std::ops::Range<usize>,
        _: Bounds<Pixels>,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let native = self.native.as_ref()?;
        let input = native.route.input.lock().unwrap();
        let c = &input["caret"];
        Some(Bounds::new(
            native.bounds.origin
                + gpui::point(
                    gpui::px(c[0].as_f64().unwrap_or(0.) as f32 / native.scale),
                    gpui::px(c[1].as_f64().unwrap_or(0.) as f32 / native.scale),
                ),
            gpui::size(
                gpui::px(c[2].as_f64().unwrap_or(1.) as f32 / native.scale),
                gpui::px(c[3].as_f64().unwrap_or(18.) as f32 / native.scale),
            ),
        ))
    }
    fn character_index_for_point(
        &mut self,
        _: gpui::Point<Pixels>,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) -> Option<usize> {
        None
    }
    fn accepts_text_input(&self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> bool {
        self.native.as_ref().is_some_and(|n| {
            n.route.input.lock().unwrap()["focused"]
                .as_bool()
                .unwrap_or(false)
        })
    }
}

impl super::BrowserSurface {
    pub(super) fn linux_dismiss_menu(&mut self, cx: &mut gpui::Context<Self>) {
        if let Some(native) = &mut self.native {
            native.menu = None;
            native.command(json!({"cmd":"dismiss-menu"}));
        }
        cx.notify();
    }
    pub(super) fn linux_choose_menu(&mut self, index: usize, cx: &mut gpui::Context<Self>) {
        if let Some(native) = &mut self.native {
            if let Some(item) = native
                .menu
                .as_ref()
                .and_then(|m| m["items"].get(index))
                .filter(|i| i["enabled"].as_bool().unwrap_or(false))
            {
                let action = item["action"].as_str().unwrap_or("");
                if action == "text" {
                    if let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) {
                        native.command(json!({"cmd":"text","text":text}));
                    }
                } else {
                    native.command(json!({"cmd":action}));
                }
            }
        }
        self.linux_dismiss_menu(cx);
    }
    pub(super) fn linux_menu_key(&mut self, key: &str, cx: &mut gpui::Context<Self>) -> bool {
        let Some(native) = self.native.as_mut().filter(|n| n.menu.is_some()) else {
            return false;
        };
        match key {
            "escape" => self.linux_dismiss_menu(cx),
            "enter" => {
                let index = native.menu_active;
                self.linux_choose_menu(index, cx);
            }
            "up" | "down" => {
                if let Some(items) = native.menu.as_ref().unwrap()["items"]
                    .as_array()
                    .filter(|items| !items.is_empty())
                {
                    for _ in 0..items.len() {
                        native.menu_active = if key == "down" {
                            (native.menu_active + 1) % items.len()
                        } else {
                            (native.menu_active + items.len() - 1) % items.len()
                        };
                        if items[native.menu_active]["enabled"]
                            .as_bool()
                            .unwrap_or(false)
                        {
                            break;
                        }
                    }
                    cx.notify();
                }
            }
            _ => {}
        }
        true
    }
    pub(super) fn linux_menu(
        &self,
        theme: &crate::theme::Theme,
        cx: &mut gpui::Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let theme = &theme.for_popup();
        use gpui::{IntoElement, div, prelude::*, px};
        let locale = i18n::locale(cx);
        let native = self.native.as_ref()?;
        let menu = native.menu.as_ref()?;
        let items = menu["items"].as_array()?;
        let pos = native.bounds.origin
            + gpui::point(
                px(menu["x"].as_f64().unwrap_or(0.) as f32),
                px(menu["y"].as_f64().unwrap_or(0.) as f32),
            );
        let mut content = crate::popover::popover_card(theme)
            .w(px(240.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.linux_dismiss_menu(cx)));
        let mut rows = div()
            .id("browser-menu-options")
            .flex()
            .flex_col()
            .max_h(px(320.))
            .overflow_y_scroll();
        for (index, item) in items.iter().enumerate() {
            let enabled = item["enabled"].as_bool().unwrap_or(false);
            let label = item["action"]
                .as_str()
                .and_then(menu_label)
                .map(|id| i18n::translate(id, locale).to_owned())
                .unwrap_or_else(|| item["label"].as_str().unwrap_or("").to_owned());
            let row = crate::popover::menu_row(
                theme,
                index == native.menu_active,
                format!("browser-option-{index}"),
            )
            .id(("browser-option", index))
            .child(gpui::SharedString::from(label))
            .when(!enabled, |el| el.opacity(0.4))
            .when(enabled, |el| {
                el.on_click(cx.listener(move |this, _, _, cx| this.linux_choose_menu(index, cx)))
            });
            rows = rows.child(row);
        }
        content = content.child(rows);
        Some(crate::popover::menu_at(
            "browser-page-menu",
            pos,
            content.into_any_element(),
            None,
        ))
    }
}
