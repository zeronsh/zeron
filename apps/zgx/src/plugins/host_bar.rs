//! What the host draws in the main window itself, outside the scripts'
//! view, so no template can take it away:
//!
//! - while a template runs as a preview (`preview`), a toast: "Previewing
//!   <name>", a line that it runs the template's code, with Keep (the host's
//!   keep sheet) and Revert (the default interface, in safe mode). It stays
//!   while the preview lasts; its minimise button folds it to a pill that
//!   opens it again;
//! - notices, toasts that go by themselves (`notice`): the template
//!   changed without you choosing it here (an agent's `use_template`, an
//!   update, a file written by hand), the account's signing keys (one added
//!   on another device, this device's revoked), and the default interface
//!   at launch because the template crashed Zeron (safe mode);
//!
//! No menu bar where the system has none (Windows, Linux): zgx's drawn one
//! (`app_menu_bar`) was a row of its own across the window's top, over the
//! app's title bar. The menus' shortcuts don't need it (zgx binds them on
//! every window's root as `set_menus` sets them), and what the menus hold
//! is in the command palette (src/main.rs `serve_commands`).
//!
//! The toasts look like the app's own (ds/toast.luau: its width, corner,
//! padding and type) and share their corner, the window's bottom right,
//! 16 points in, in one column: the host's at its foot (they stay put as
//! the app's come and go above them), the app's stacked over them from
//! `toast_floor`, a shared value the host publishes (`follow`).
//!
//! A template replaces the whole interface, `ds/` and its toasts included,
//! so nothing here comes from the scripts: the toasts are on the host's top
//! layer (zgx's `View::top_layer`), over whatever a script opens in the window
//! and live under its modals, so nothing there covers them or takes their
//! clicks. While a preview shows, what the scripts open stays in the window
//! (`PopupsInWindow`), since a popover in a window of its own would be over
//! this layer too. (zgx provides that around a plugin's nodes itself, following
//! decision controls, so a plugin's own popup in its own window is still over
//! these.) their colours, type and glyphs are fixed here, never the
//! theme's tokens (a template sets those), only following whether the
//! theme is light or dark; and their buttons run host commands.

use std::cell::RefCell;
use std::sync::Arc;

use zgx::compose::{View, button, column, component, modal, row, svg, switch, text};
use zgx::cursor::Cursor;
use zgx::scene::{BoxShadow, Color, NodeId, Scene, Transform};
use zgx::semantics::{Live, Role};
use zgx::style::Styled;
use zgx::svg::SvgData;
use zgx::text_layout::FontFamily;
use zgx_desktop::WindowContext;

use super::preview;

/// How a toast reads: plain, or one to look at (amber).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Neutral,
    Warning,
}

/// A notice's button: its label and the host command it runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Action {
    pub label: &'static str,
    pub command: &'static str,
}

/// A notice: a toast that goes by itself (`NOTICE_FOR`) unless it `stays`.
#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub title: String,
    pub body: String,
    pub tone: Tone,
    pub action: Option<Action>,
    pub stays: bool,
}

/// The default interface, in safe mode (the menu bar's "Use Default
/// Interface").
const USE_DEFAULT: Action = Action { label: "Use default interface", command: super::USE_ZERON_TEMPLATE };

impl Notice {
    /// The template changed without you choosing it here: to `name`, or
    /// to the default (`to_default`); by a chat or otherwise. One in
    /// preview has its own toast with Revert, so this one has no button.
    pub fn switched(name: &str, to_default: bool, by_chat: bool, in_preview: bool) -> Self {
        let body = if by_chat {
            "A chat switched the template. You didn't choose it here."
        } else {
            "The template changed without you choosing it here."
        };
        Notice {
            title: if to_default { "Back to the default interface".into() } else { format!("Switched to {name}") },
            body: body.into(),
            tone: Tone::Neutral,
            action: (!to_default && !in_preview).then_some(USE_DEFAULT),
            stays: false,
        }
    }

    /// The engine's word on the account's signing keys (`kind`), in the
    /// host's words; one it doesn't know, in the engine's (`words`).
    pub fn key(kind: &str, device: Option<&str>, words: &str) -> Self {
        let here = if cfg!(target_os = "macos") { "This Mac's" } else { "This device's" };
        let (title, body) = match kind {
            "key_added" => (
                "New signing key".to_owned(),
                format!(
                    "Added to your account on {}. If it wasn't you, revoke it in Settings → Account.",
                    device.filter(|d| !d.trim().is_empty()).unwrap_or("another device")
                ),
            ),
            "own_key_revoked" => (
                "Signing key revoked".to_owned(),
                format!("{here} key was revoked on another device. Sign in again for a new one, then allow your plugins again."),
            ),
            _ => ("Signing keys".to_owned(), words.to_owned()),
        };
        Notice { title, body, tone: Tone::Warning, action: None, stays: true }
    }

    /// Zeron opened with the default interface (safe mode): ⌥ held, or
    /// the template crashed it twice as it opened.
    pub fn safe_at_launch(reason: preview::SafeReason) -> Self {
        let (body, tone, stays) = match reason {
            preview::SafeReason::Held => ("You held ⌥ as Zeron opened. Pick a template in Settings → Templates to use it again.", Tone::Neutral, false),
            preview::SafeReason::Crashed => (
                "Your template closed Zeron twice as it opened. Pick it again in Settings → Templates once it's fixed.",
                Tone::Warning,
                true,
            ),
        };
        Notice { title: "Opened with the default interface".into(), body: body.into(), tone, action: None, stays }
    }
}

thread_local! {
    static NOTICES: RefCell<Option<zgx::reactive::Signal<Vec<Shown>>>> = const { RefCell::new(None) };
    static NEXT: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// Whether Zeron shows Chinese now, as the toasts last read it.
    static CHINESE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Whether a preview shows: the scripts' popups stay in the window then,
    /// under the toasts (`compose`).
    static POPUPS_HELD: RefCell<Option<zgx::reactive::Signal<bool>>> = const { RefCell::new(None) };
}

/// A notice as shown, with its id.
#[derive(Clone, Debug, PartialEq)]
struct Shown {
    id: u64,
    notice: Notice,
}

/// How long a notice stays.
const NOTICE_FOR: std::time::Duration = std::time::Duration::from_secs(12);
/// How many notices show at once (the newest).
const MAX: usize = 3;

/// A notice in the host's toasts: the same one again restarts it rather
/// than adding another.
pub fn notice(notice: Notice) {
    eprintln!("zeron-zgx: notice: {}: {}", notice.title, notice.body);
    let Some(signal) = NOTICES.with(|n| n.borrow().clone()) else { return };
    let id = NEXT.with(|n| {
        n.set(n.get() + 1);
        n.get()
    });
    let stays = notice.stays;
    signal.update(|list| {
        list.retain(|s| s.notice != notice);
        list.push(Shown { id, notice });
        let excess = list.len().saturating_sub(MAX);
        list.drain(..excess);
    });
    if stays {
        return;
    }
    let later = signal.clone();
    super::spawn_on_bridge(async move {
        zgx::timer::sleep(NOTICE_FOR).await;
        dismiss(&later, id);
    });
}

fn dismiss(signal: &zgx::reactive::Signal<Vec<Shown>>, id: u64) {
    if signal.with_untracked(|list| list.iter().any(|s| s.id == id)) {
        signal.update(|list| list.retain(|s| s.id != id));
    }
}

/// The toasts: a layer over the scripts' view.
pub fn view(cx: &mut WindowContext, locale: zgx::reactive::Signal<zgx_script::Value>) -> View {
    column().child(toasts(&cx.ui.runtime, locale))
}

/// Development (debug builds): `ZERON_ZGX_HOST_TOASTS=1` lays the toasts
/// over a fixture's window to look at them: "Inset" in preview, and a
/// switch and a new signing key told. None otherwise.
pub fn dev_toasts(cx: &mut WindowContext) -> Option<View> {
    if !cfg!(debug_assertions) || std::env::var_os("ZERON_ZGX_HOST_TOASTS").is_none() {
        return None;
    }
    preview::set(Some(preview::Preview { id: "inset".into(), name: "Inset".into() }));
    let chrome = toasts(&cx.ui.runtime, cx.ui.runtime.signal(zgx_script::Value::from("en")));
    notice(Notice::switched("Inset", false, true, true));
    notice(Notice::key("key_added", Some("Wing's iPhone"), ""));
    Some(chrome)
}

/// A preview asked to open a link or a page: it can once you keep it. Asked
/// again, the same toast starts over (`notice`).
pub fn links_need_keeping() {
    let title = if CHINESE.with(std::cell::Cell::get) { "保留这个模板后才能打开链接" } else { "Keep this template to open links" };
    notice(Notice { title: title.into(), body: String::new(), tone: Tone::Neutral, action: None, stays: false });
}

/// The main window's root: the host's `chrome` above the scripts' view,
/// which takes the rest of the window, its full width and every point the
/// chrome leaves (the scripts' root, `#shell`, sizes itself to it). (The
/// toasts in it are a layer over the window and take no room.)
pub fn compose(chrome: View, scripts: View) -> View {
    let scripts = scripts.grow().min_h(0.);
    // A popup in a window of its own is over this one, the toasts too: while
    // a preview shows, what the scripts open stays in the window.
    let scripts = match POPUPS_HELD.with(|h| h.borrow().clone()) {
        Some(held) => zgx::compose::provide(zgx::compose::PopupsInWindow(held), scripts),
        None => scripts,
    };
    column().w_full().h_full().child(chrome).child(scripts)
}

// ── Look ────────────────────────────────────────────────────────────────────

/// The toasts' colours in a theme: ds's menus' solid glass, text and
/// washes, from ui/ds/themes (zeron-dark.json, zeron-light.json). Solid,
/// not the menus' frost: nothing a template draws under it changes how
/// it reads.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Ink {
    card: Color,
    edge: Color,
    shadow: BoxShadow,
    text: Color,
    muted: Color,
    faint: Color,
    warning: Color,
    /// A button's fill (ds's secondary), and under the pointer and pressed.
    fill: Color,
    fill_hover: Color,
    fill_pressed: Color,
    /// A quiet icon button's wash under the pointer and pressed.
    hover: Color,
    pressed: Color,
}

const DARK: Ink = Ink {
    card: Color(0x1c, 0x1c, 0x1f, 0xff),
    edge: Color(0xff, 0xff, 0xff, 0x12),
    // 0 20 56 0 #00000052 (CSS blur, twice the sigma).
    shadow: BoxShadow { color: Color(0, 0, 0, 0x52), offset: Transform { x: 0., y: 20. }, blur_radius: 28., spread: 0. },
    text: Color(0xed, 0xed, 0xef, 0xff),
    muted: Color(0xed, 0xed, 0xef, 153),
    faint: Color(0xed, 0xed, 0xef, 107),
    // palette.amber.300
    warning: Color(0xf5, 0xc2, 0x83, 0xff),
    fill: Color(0xff, 0xff, 0xff, 0x14),
    fill_hover: Color(0xff, 0xff, 0xff, 0x1f),
    fill_pressed: Color(0xff, 0xff, 0xff, 0x29),
    hover: Color(0xff, 0xff, 0xff, 0x0d),
    pressed: Color(0xff, 0xff, 0xff, 0x17),
};

const LIGHT: Ink = Ink {
    card: Color(0xfb, 0xfb, 0xfc, 0xff),
    edge: Color(0, 0, 0, 0x24),
    // 0 18 48 0 #0000001c
    shadow: BoxShadow { color: Color(0, 0, 0, 0x1c), offset: Transform { x: 0., y: 18. }, blur_radius: 24., spread: 0. },
    text: Color(0x1b, 0x1b, 0x1d, 0xff),
    muted: Color(0x1b, 0x1b, 0x1d, 163),
    faint: Color(0x1b, 0x1b, 0x1d, 115),
    // palette.amber.700
    warning: Color(0x90, 0x5d, 0x02, 0xff),
    fill: Color(0, 0, 0, 0x0d),
    fill_hover: Color(0, 0, 0, 0x14),
    fill_pressed: Color(0, 0, 0, 0x1c),
    hover: Color(0, 0, 0, 0x0a),
    pressed: Color(0, 0, 0, 0x12),
};

/// ds's toast: `$zeron.toast.width`, `$radius.xl`, its padding (space.4
/// at the start, space.2 at the end, space.3 down) and gap (space.3), 16
/// points off the window's corner, space.2 between toasts.
const WIDTH: f32 = 340.;
const RADIUS: f32 = 12.;
const BORDER: f32 = 1.;
/// The controls' inset from the card's edge: the corner button's and the
/// actions' (ds's toast: space.2 from the end, its × as far from the top).
const INSET: f32 = 8.;
/// What's inside the card at `INSET` is cornered concentric with it.
const INNER: f32 = RADIUS - BORDER - INSET;
const OFF_CORNER: f32 = 16.;
const GAP: f32 = 8.;
/// The pill a minimised preview folds to: a small button's height.
const PILL_H: f32 = 28.;

const PREVIEW_LINE: &str = "A preview runs this template's code. Only preview templates you trust.";
const PREVIEW_LINE_ZH: &str = "预览会运行这个模板的代码。只预览你信任的模板。";

const INFO: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><g fill="none" stroke="currentColor" stroke-width="1.5"><circle cx="12" cy="12" r="10"/><path stroke-linecap="round" d="M12 17v-6"/><path stroke-linecap="round" d="M12 7.5h.01"/></g></svg>"#;
const WARNING: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><g fill="none"><path stroke="currentColor" stroke-width="1.5" d="M5.312 10.762C8.23 5.587 9.689 3 12 3s3.77 2.587 6.688 7.762l.364.644c2.425 4.3 3.638 6.45 2.542 8.022S17.786 21 12.364 21h-.728c-5.422 0-8.134 0-9.23-1.572s.117-3.722 2.542-8.022z"/><path stroke="currentColor" stroke-linecap="round" stroke-width="1.5" d="M12 8v5"/><circle cx="12" cy="16" r="1" fill="currentColor"/></g></svg>"#;
const CLOSE: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none"><path d="m4.75 4.75 6.5 6.5m0-6.5-6.5 6.5" stroke="currentColor" stroke-width="1.25" stroke-linecap="round"/></svg>"#;
const MINUS: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none"><path d="M6 12h12" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/></svg>"#;

fn glyph(label: &str, source: &'static str, size: f32, color: Color) -> View {
    let data = SvgData::new(source.as_bytes()).expect("a glyph").tinted(color);
    svg(label, Arc::new(data)).size(size, size).shrink_0()
}

fn words(value: impl Into<String>, size: f32, line: f32, color: Color) -> View {
    text(value).font_family(FontFamily::from("Geist")).text_size(size).line_height(line).text_color(color).text_wrap(true)
}

/// A small button (ds's `sm`): 28 high, its label at the font's small
/// size, medium, on a fill that firms as it's touched; cornered at `INNER`.
fn action(label: &'static str, ink: Ink, run: impl FnMut() + 'static) -> View {
    button()
        .child(words(label, 12., 16., ink.text).font_weight(500).text_wrap(false))
        .accessible_label(label)
        .h(PILL_H)
        .px(10.)
        .items_center()
        .justify_center()
        .rounded(INNER)
        .bg(ink.fill)
        .border(0.)
        .cursor(Cursor::Pointer)
        .hover(|s| s.bg(ink.fill_hover))
        .active(|s| s.bg(ink.fill_pressed))
        .on_click(run)
}

/// The card's corner button (ds's quiet `sm` icon button): 22 square,
/// `INSET` from the top and the end, cornered at `INNER`.
fn corner(label: &'static str, source: &'static str, ink: Ink, run: impl FnMut() + 'static) -> View {
    button()
        .child(glyph(label, source, 14., ink.faint))
        .accessible_label(label)
        .size(22., 22.)
        .p(0.)
        .items_center()
        .justify_center()
        .shrink_0()
        .rounded(INNER)
        .bg(Color(0, 0, 0, 0))
        .border(0.)
        .cursor(Cursor::Pointer)
        .hover(|s| s.bg(ink.hover))
        .active(|s| s.bg(ink.pressed))
        .on_click(run)
}

/// One toast: a glyph, a title and a line, its corner button, and its
/// actions in the bottom-end corner.
fn card(id: &str, title: &str, body: &str, tone: Tone, ink: Ink, corner_button: View, actions: Vec<View>) -> View {
    let (source, color) = match tone {
        Tone::Neutral => (INFO, ink.muted),
        Tone::Warning => (WARNING, ink.warning),
    };
    let head = row()
        .items_start()
        .gap(12.)
        // (Inside the border, as ds's: the text's start space.4 in, its top
        // space.3 down; the corner button `INSET` from the top and the end.)
        .pl(16.)
        .pr(INSET)
        .pt(INSET)
        .pb(if actions.is_empty() { 12. } else { 0. })
        // (Its 1 down centres the 16 glyph on the title's 18 line.)
        .child(column().pt(12. - INSET + 1.).shrink_0().child(glyph("", source, 16., color)))
        .child(
            column()
                .grow()
                .min_w(0.)
                .gap(2.)
                .pt(12. - INSET)
                .child(words(title, 13., 18., ink.text).font_weight(500))
                .children((!body.is_empty()).then(|| words(body, 12., 16., ink.muted))),
        )
        .child(corner_button);
    let mut out = column()
        .id(id)
        .pointer_events(true)
        .w(WIDTH)
        .rounded(RADIUS)
        .border(BORDER)
        .border_color(ink.edge)
        .bg(ink.card)
        .shadow(ink.shadow)
        .role(if tone == Tone::Warning { Role::Alert } else { Role::Status })
        .live(if tone == Tone::Warning { Live::Assertive } else { Live::Polite })
        .accessible_label(title)
        .child(head);
    if !actions.is_empty() {
        // (space.2 under the words, as ds's toast's action.)
        out = out.child(row().justify_end().gap(GAP).pt(8.).px(INSET).pb(INSET).children(actions));
    }
    out
}

/// The preview's toast, minimised: a pill with its glyph and title that
/// opens it again.
fn pill(title: &str, ink: Ink, open: impl FnMut() + 'static) -> View {
    button()
        .id("host-preview-pill")
        .pointer_events(true)
        .accessible_label(format!("{title}: show"))
        .h(PILL_H)
        .pl(10.)
        .pr(12.)
        .gap(6.)
        .items_center()
        .rounded(PILL_H / 2.)
        .border(BORDER)
        .border_color(ink.edge)
        .bg(ink.card)
        .shadow(ink.shadow)
        .cursor(Cursor::Pointer)
        .hover(|s| s.bg(ink.card))
        .child(glyph("", INFO, 14., ink.muted))
        .child(words(title, 12., 16., ink.text).font_weight(500).text_wrap(false))
        .on_click(open)
}

// ── The toasts ──────────────────────────────────────────────────────────────

/// The toasts as they come and go: the notices above, the preview's at
/// the foot (it stays), in a layer of the host's at the window's
/// bottom-left corner.
fn toasts(runtime: &zgx::reactive::Runtime, locale: zgx::reactive::Signal<zgx_script::Value>) -> View {
    let now = runtime.signal(preview::current());
    let minimised = runtime.signal(false);
    let popups_held = runtime.signal(now.with_untracked(Option::is_some));
    POPUPS_HELD.with(|h| *h.borrow_mut() = Some(popups_held.clone()));
    {
        let (now, minimised) = (now.clone(), minimised.clone());
        preview::on_change(move |p| {
            popups_held.set(p.is_some());
            // Another template in preview: shown open again.
            if now.with_untracked(|was| was.as_ref().map(|w| &w.id) != p.map(|p| &p.id)) {
                minimised.set(false);
            }
            now.set(p.cloned());
        });
    }
    let notices = runtime.signal(Vec::<Shown>::new());
    NOTICES.with(|n| *n.borrow_mut() = Some(notices.clone()));
    if let Some(reason) = preview::take_safe_at_launch() {
        notice(Notice::safe_at_launch(reason));
    }
    let themes = zgx::theme::Themes::of(runtime);
    let shown = runtime.signal(true);
    CORNER.with(|c| *c.borrow_mut() = Some(Corner { scene: None, node: None, floor: None, told: f32::NAN }));
    let (heard, folded) = (notices.clone(), minimised.clone());
    let stack = switch(
        move || {
            themes.current();
            (now.get(), heard.get(), folded.get(), themes.kind().is_dark(), matches!(locale.get(), zgx_script::Value::Str(l) if l.starts_with("zh")))
        },
        move |(preview, list, mini, dark, chinese), _| {
            CHINESE.with(|c| c.set(chinese));
            let ink = if dark { DARK } else { LIGHT };
            let mut out = column().items_end().gap(GAP);
            for shown in list {
                let id = shown.id;
                let gone = notices.clone();
                let n = &shown.notice;
                let close = corner("Dismiss", CLOSE, ink, move || dismiss(&gone, id));
                let actions = n
                    .action
                    .map(|a| {
                        let gone = notices.clone();
                        action(a.label, ink, move || {
                            dismiss(&gone, id);
                            super::host_command(a.command);
                        })
                    })
                    .into_iter()
                    .collect();
                out = out.child(card(&format!("host-notice-{id}"), &n.title, &n.body, n.tone, ink, close, actions));
            }
            if let Some(p) = preview {
                let title = format!("Previewing {}", p.name);
                let fold = minimised.clone();
                if mini {
                    out = out.child(pill(&title, ink, move || {
                        fold.set(false);
                    }));
                } else {
                    let minimise = corner("Minimize", MINUS, ink, move || {
                        fold.set(true);
                    });
                    out = out.child(card(
                        "host-preview",
                        &title,
                        // (True of every preview: don't word one as safe,
                        // preview.rs. Here, not in the catalog: a template
                        // can rewrite that.)
                        if chinese { PREVIEW_LINE_ZH } else { PREVIEW_LINE },
                        Tone::Neutral,
                        ink,
                        minimise,
                        vec![
                            action("Revert", ink, || super::host_command(super::USE_ZERON_TEMPLATE)),
                            action("Keep", ink, || super::host_command(super::KEEP_TEMPLATE)),
                        ],
                    ));
                }
            }
            out
        },
    );
    component(move |_| {
        modal("Zeron", shown)
            .top_layer()
            .autofocus(false)
            .p(0.)
            .bg(Color(0, 0, 0, 0))
            .border(0.)
            .shadow_none()
            .overflow_visible()
            // (The stack's blank space takes no clicks; the toasts take
            // theirs.)
            .pointer_events(false)
            .child(
                column()
                    .id("host-toasts")
                    .absolute()
                    .right(OFF_CORNER)
                    .bottom(OFF_CORNER)
                    .items_end()
                    .pointer_events(false)
                    .on_mount(|handle| {
                        CORNER.with(|c| {
                            if let Some(c) = c.borrow_mut().as_mut() {
                                c.scene = handle.scene();
                                c.node = Some(handle.node());
                            }
                        });
                        // Where the app's toasts stand, as this is laid out.
                        handle.on_layout(measure);
                    })
                    .child(stack),
            )
    })
}

// ── The corner ──────────────────────────────────────────────────────────────

/// The host's stack, measured for the app's toasts to stand over.
struct Corner {
    scene: Option<std::rc::Rc<RefCell<Scene>>>,
    /// The host's stack (`#host-toasts`).
    node: Option<NodeId>,
    /// Where the app's toasts stand, told to the scripts (`toast_floor`).
    floor: Option<zgx_script::Shared>,
    /// The floor last told.
    told: f32,
}

thread_local! {
    static CORNER: RefCell<Option<Corner>> = const { RefCell::new(None) };
}

/// The shared value the app's toasts stand on (`toast_floor`, in points up
/// from the window's foot): over the host's, or 16 with none showing.
pub fn follow(floor: zgx_script::Shared) {
    CORNER.with(|c| {
        if let Some(c) = c.borrow_mut().as_mut() {
            c.told = f32::NAN;
            c.floor = Some(floor);
        }
    });
    measure();
}

/// Tells the app's toasts where to stand: over the host's stack, `GAP`
/// apart, as it is laid out now. After every layout.
fn measure() {
    let found = CORNER.with(|c| {
        let c = c.borrow();
        let c = c.as_ref()?;
        let floor = c.floor.clone()?;
        let scene = c.scene.clone()?;
        let scene = scene.try_borrow().ok()?;
        let node = c.node.filter(|n| scene.contains(*n))?;
        let stack = scene.transformed_bounds(node).height;
        Some((stack, c.told, floor))
    });
    let Some((stack, told, floor)) = found else { return };
    let over = if stack > 0.5 { OFF_CORNER + stack + GAP } else { OFF_CORNER };
    if (told - over).abs() <= 0.5 {
        return;
    }
    floor.set(zgx_script::Value::from(f64::from(over)));
    CORNER.with(|c| {
        if let Some(c) = c.borrow_mut().as_mut() {
            c.told = over;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use zgx_script::{ScriptHost, testing::TestApp};

    thread_local! {
        /// The language a test's toasts show in.
        static TEST_LOCALE: std::cell::Cell<&'static str> = const { std::cell::Cell::new("en") };
    }

    /// `entry` (under ui/) under the host's chrome, as the main window
    /// composes them (`compose`).
    fn mount_at(path: std::path::PathBuf, with_host: impl FnOnce(&ScriptHost) + 'static) -> TestApp {
        let chrome: std::rc::Rc<RefCell<Option<View>>> = Default::default();
        let made = chrome.clone();
        let app = TestApp::wrapped(
            move |runtime| {
                let locale = runtime.signal(zgx_script::Value::from(TEST_LOCALE.with(|l| l.get())));
                *made.borrow_mut() = Some(column().child(toasts(runtime, locale)));
                let host = ScriptHost::new(runtime, path);
                with_host(&host);
                host
            },
            move |scripts| compose(chrome.borrow_mut().take().unwrap(), scripts),
        );
        app.frames(30);
        assert_eq!(app.error(), None, "the scripts failed");
        // As the main window does: the app's toasts told where to stand.
        follow(app.host.shared("toast_floor", 16.));
        app.frames(6);
        app
    }

    /// The shell, with the app's own toasts on command (`fixture.toast`).
    fn mount() -> TestApp {
        mount_at(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/host_toasts_fixture.luau"), |_| {})
    }

    fn overlap(a: zgx::scene::Rect, b: zgx::scene::Rect) -> bool {
        a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height
    }

    /// The card (ds's or the host's, `WIDTH` wide) a text is in.
    fn card_of(app: &mut TestApp, text: &str) -> zgx::scene::Rect {
        let node = app.find(&format!("\"{text}\"")).unwrap_or_else(|| panic!("no {text}"));
        let scene = app.ui.scene.borrow();
        std::iter::once(node)
            .chain(scene.ancestors(node))
            .map(|n| scene.transformed_bounds(n))
            .find(|r| (r.width - WIDTH).abs() < 0.5)
            .unwrap_or_else(|| panic!("{text} in no card"))
    }

    fn bounds(app: &mut TestApp, selector: &str) -> zgx::scene::Rect {
        let node = app.find(selector).unwrap_or_else(|| panic!("no {selector}"));
        app.ui.scene.borrow().transformed_bounds(node)
    }

    /// The words in the host's toasts.
    fn host_texts(app: &mut TestApp) -> String {
        let toasts = app.find("#host-toasts").expect("the toasts");
        let scene = app.ui.scene.borrow();
        let mut out = Vec::new();
        let mut stack = vec![toasts];
        while let Some(node) = stack.pop() {
            if let zgx::scene::NodeKind::Text { text, .. } = scene.kind(node) {
                out.push(text.to_string());
            }
            stack.extend(scene.children(node).iter().rev().copied());
        }
        out.join("|")
    }

    fn sleek() -> preview::Preview {
        preview::Preview { id: "sleek".into(), name: "Sleek".into() }
    }

    /// The scripts' root fills the window whatever the host shows: its
    /// toasts are a layer over it, taking no room.
    #[test]
    fn the_shell_fills_the_window_under_the_toasts() {
        preview::set(None);
        let mut app = mount();
        for (w, h) in [(1624., 836.), (900., 600.), (1240., 820.)] {
            app.ui.scene.borrow_mut().resize(w, h);
            app.frames(6);
            for shown in [false, true] {
                preview::set(shown.then(sleek));
                app.frames(6);
                let shell = bounds(&mut app, "#shell");
                assert_eq!((shell.x, shell.y, shell.width, shell.height), (0., 0., w, h), "preview {shown}, {w}×{h}");
            }
            preview::set(None);
        }
    }

    /// A preview asked for a link: one toast, however often, in the language
    /// shown; and while it shows, popups stay in the window.
    #[test]
    fn a_link_in_a_preview_says_to_keep_it_and_popups_stay_in_the_window() {
        preview::set(Some(sleek()));
        let mut app = mount();
        app.frames(4);
        links_need_keeping();
        links_need_keeping();
        app.frames(6);
        let texts = host_texts(&mut app);
        assert_eq!(texts.matches("Keep this template to open links").count(), 1, "{texts}");
        let held = POPUPS_HELD.with(|h| h.borrow().clone()).expect("the toasts hold it");
        assert!(held.get(), "popups stay in the window during a preview");
        preview::set(None);
        app.frames(4);
        assert!(!held.get(), "and go where they like once it's over");
    }

    /// Its line follows the language Zeron shows; the rest of the host's
    /// toasts are English (docs/i18n.md).
    #[test]
    fn the_preview_s_line_is_in_chinese_when_zeron_is() {
        TEST_LOCALE.with(|l| l.set("zh-Hans"));
        preview::set(Some(sleek()));
        let mut app = mount();
        TEST_LOCALE.with(|l| l.set("en"));
        app.frames(6);
        let texts = app.texts().join("|");
        assert!(texts.contains(PREVIEW_LINE_ZH), "{texts}");
        assert!(!texts.contains(PREVIEW_LINE), "{texts}");
        preview::set(None);
    }

    /// The preview's toast: its title and line, Revert and Keep; in the
    /// bottom-left corner, 16 points in, ds's toast's width; never over the
    /// title bar; gone with the preview.
    #[test]
    fn the_preview_shows_as_a_toast_in_the_bottom_right_corner() {
        preview::set(None);
        let mut app = mount();
        app.ui.scene.borrow_mut().resize(1240., 820.);
        assert!(app.find("#host-preview").is_none(), "no preview, no toast");
        preview::set(Some(sleek()));
        app.frames(6);
        let card = bounds(&mut app, "#host-preview");
        assert_eq!((card.x + card.width, card.width), (1240. - OFF_CORNER, WIDTH), "16 off the end: {card:?}");
        assert!((card.y + card.height - (820. - OFF_CORNER)).abs() < 0.5, "16 off the foot: {card:?}");
        assert!(card.y > 400., "near the foot, far from the title bar: {card:?}");
        let texts = app.texts().join("|");
        assert!(texts.contains("Previewing Sleek"), "{texts}");
        // (It claims no safety a preview doesn't have.)
        assert!(texts.contains(PREVIEW_LINE), "{texts}");
        assert!(!texts.contains("can't run programs"), "{texts}");
        let (revert, keep) = (bounds(&mut app, "role:button \"Revert\""), bounds(&mut app, "role:button \"Keep\""));
        assert!(revert.x < keep.x && (revert.y - keep.y).abs() < 0.5, "Revert then Keep, side by side: {revert:?} {keep:?}");
        // In the bottom-end corner, `INSET` in from the border: concentric
        // at `INNER`, the card's corner less the border and the inset.
        assert!((card.x + card.width - BORDER - INSET - (keep.x + keep.width)).abs() < 0.5, "{card:?} {keep:?}");
        assert!((card.y + card.height - BORDER - INSET - (keep.y + keep.height)).abs() < 0.5, "{card:?} {keep:?}");
        assert_eq!(INNER, 3.);
        let corner = bounds(&mut app, "role:button \"Minimize\"");
        assert!((card.x + card.width - BORDER - INSET - (corner.x + corner.width)).abs() < 0.5, "{card:?} {corner:?}");
        assert!((corner.y - (card.y + BORDER + INSET)).abs() < 0.5, "as far from the top as the end: {card:?} {corner:?}");
        assert!(!host_texts(&mut app).contains('…'), "no ellipses");
        let body = bounds(&mut app, &format!("#host-preview >> \"{PREVIEW_LINE}\""));
        assert!(body.height >= 32., "the line wraps, never cut: {body:?}");
        preview::set(None);
        app.frames(6);
        assert!(app.find("#host-preview").is_none(), "gone with the preview");
    }

    /// Minimised, the preview folds to a pill in the same corner, which
    /// opens it again; another template in preview opens it.
    #[test]
    fn the_preview_minimises_to_a_pill_that_opens_it_again() {
        preview::set(Some(sleek()));
        let mut app = mount();
        app.ui.scene.borrow_mut().resize(1240., 820.);
        app.frames(6);
        app.click("role:button \"Minimize\"");
        app.frames(6);
        assert!(app.find("#host-preview").is_none(), "folded");
        let pill = bounds(&mut app, "#host-preview-pill");
        assert_eq!(pill.x + pill.width, 1240. - OFF_CORNER, "{pill:?}");
        assert!((pill.y + pill.height - (820. - OFF_CORNER)).abs() < 0.5, "{pill:?}");
        assert_eq!(pill.height, PILL_H, "{pill:?}");
        app.click("#host-preview-pill");
        app.frames(6);
        assert!(app.find("#host-preview").is_some(), "open again");

        app.click("role:button \"Minimize\"");
        app.frames(6);
        preview::set(Some(preview::Preview { id: "inset".into(), name: "Inset".into() }));
        app.frames(6);
        assert!(app.find("#host-preview").is_some(), "another template's preview shows open");
        preview::set(None);
    }

    /// Every notice is a toast of its own, with its own words and buttons:
    /// a switch, signing keys, safe mode at launch; stacked over the
    /// preview's, which stays at the foot; × puts one away.
    #[test]
    fn each_notice_is_a_toast_with_its_own_words_and_actions() {
        preview::set(Some(sleek()));
        let mut app = mount();
        app.ui.scene.borrow_mut().resize(1240., 820.);
        notice(Notice::switched("Ledger", false, true, false));
        app.frames(6);
        let texts = app.texts().join("|");
        assert!(texts.contains("Switched to Ledger") && texts.contains("A chat switched the template. You didn't choose it here."), "{texts}");
        assert!(app.find("role:button \"Use default interface\"").is_some(), "a way back");
        let (switch, preview_card) = (bounds(&mut app, "role:status \"Switched to Ledger\""), bounds(&mut app, "#host-preview"));
        assert!(switch.y + switch.height <= preview_card.y, "over the preview's: {switch:?} {preview_card:?}");

        // A switch to one in preview: its own toast has Revert.
        let in_preview = Notice::switched("Sleek", false, false, true);
        assert_eq!(in_preview.action, None);
        assert_eq!(in_preview.body, "The template changed without you choosing it here.");
        let back = Notice::switched("", true, false, false);
        assert_eq!((back.title.as_str(), back.action), ("Back to the default interface", None));

        notice(Notice::key("key_added", Some("Wing's iPhone"), "…"));
        notice(Notice::key("own_key_revoked", None, "…"));
        app.frames(6);
        let texts = app.texts().join("|");
        assert!(texts.contains("New signing key") && texts.contains("Added to your account on Wing's iPhone. If it wasn't you, revoke it in Settings → Account."), "{texts}");
        assert!(texts.contains("Signing key revoked"), "{texts}");
        assert!(app.find("role:alert \"Signing key revoked\"").is_some(), "a warning is an alert");
        assert_eq!(Notice::key("key_added", None, "").body, "Added to your account on another device. If it wasn't you, revoke it in Settings → Account.");
        assert_eq!(Notice::key("other", None, "The engine's words.").body, "The engine's words.");

        // Three at most, the newest: the switch has gone.
        notice(Notice::safe_at_launch(preview::SafeReason::Crashed));
        app.frames(6);
        let texts = app.texts().join("|");
        assert!(texts.contains("Opened with the default interface") && !texts.contains("Switched to Ledger"), "{texts}");
        assert!(texts.contains("Previewing Sleek"), "the preview's stays: {texts}");

        // × puts one away.
        let before = app.texts().join("|");
        app.click("role:alert \"Signing key revoked\" >> role:button \"Dismiss\"");
        app.frames(6);
        let after = app.texts().join("|");
        assert!(before.contains("Signing key revoked") && !after.contains("Signing key revoked"), "{after}");
        assert!(!host_texts(&mut app).contains('…'), "no ellipses: {}", host_texts(&mut app));
        NOTICES.with(|n| n.borrow().as_ref().map(|s| s.set(Vec::new())));
        preview::set(None);
    }

    /// Light and dark: the toast takes the theme's kind, in its own
    /// colours (never the theme's tokens).
    #[test]
    fn the_toast_follows_light_and_dark_in_its_own_colours() {
        preview::set(Some(sleek()));
        let mut app = mount();
        let themes = zgx::theme::Themes::of(&app.ui.runtime);
        let fill = |app: &mut TestApp| {
            let node = app.find("#host-preview").expect("the toast");
            match app.ui.scene.borrow().kind(node) {
                zgx::scene::NodeKind::Panel { quad, .. } => Some(quad.fill),
                _ => None,
            }
        };
        for (name, ink) in [("zeron-dark", DARK), ("zeron-light", LIGHT), ("zeron-dark", DARK)] {
            themes.set(name).unwrap_or_else(|e| panic!("{name}: {e:?}"));
            app.frames(6);
            assert_eq!(fill(&mut app), Some(ink.card), "{name}");
        }
        preview::set(None);
    }

    /// One column in the window's bottom-right corner, 16 in from its
    /// edges, whatever the page shows: the host's toasts at its foot, the
    /// app's (ds/toast) stacked over them, none on another; the app's come
    /// down to the corner once the host's are gone. At 1240 and 900 wide,
    /// on a session (its composer docked at the foot).
    #[test]
    fn the_host_s_and_the_app_s_toasts_share_the_window_s_corner() {
        preview::set(Some(sleek()));
        let mut app = mount();
        app.ui.scene.borrow_mut().resize(1240., 820.);
        app.frames(20);
        app.click("#shell-sidebar >> role:button \"Fix the build\"");
        app.frames(20);
        notice(Notice::key("key_added", Some("Wing's iPhone"), ""));
        assert!(app.host.run_command("fixture.toast") && app.host.run_command("fixture.toast"));
        for (w, h) in [(1240., 820.), (900., 600.)] {
            app.ui.scene.borrow_mut().resize(w, h);
            app.frames(20);
            let host = bounds(&mut app, "#host-toasts");
            assert!((host.x + host.width - (w - OFF_CORNER)).abs() < 0.5, "{w}: 16 off the end: {host:?}");
            assert!((host.y + host.height - (h - OFF_CORNER)).abs() < 0.5, "{w}: 16 off the foot: {host:?}");
            let (preview_card, key) = (card_of(&mut app, "Previewing Sleek"), card_of(&mut app, "New signing key"));
            assert!(preview_card.y > key.y, "{w}: the preview's lowest");
            let ds = [card_of(&mut app, "Pushed to origin (1)"), card_of(&mut app, "Pushed to origin (2)")];
            for card in ds {
                assert!((card.x + card.width - (w - OFF_CORNER)).abs() < 0.5, "{w}: the same column: {card:?}");
                assert!(card.y + card.height <= host.y - GAP + 0.5, "{w}: the app's over the host's: {card:?} {host:?}");
                assert!(!overlap(card, preview_card) && !overlap(card, key), "{w}: {card:?}");
            }
            assert!(!overlap(ds[0], ds[1]), "{w}: {ds:?}");
        }
        // The host's put away: the app's come down to the corner.
        preview::set(None);
        NOTICES.with(|n| n.borrow().as_ref().map(|s| s.set(Vec::new())));
        app.frames(20);
        let ds = card_of(&mut app, "Pushed to origin (2)");
        assert!((ds.y + ds.height - (600. - OFF_CORNER)).abs() < 0.5, "the app's at the corner: {ds:?}");
        app.host.run_command("fixture.toasts_clear");
    }

    /// No script can cover the toasts or take their clicks: a modal over
    /// the whole window (its dim and an opaque panel) and a layer, opened
    /// after the toast, still draw under it, and a click on Keep reaches
    /// the host, while the modal holds focus.
    #[test]
    fn a_script_s_modal_or_layer_can_t_cover_the_toast_or_take_its_clicks() {
        let dir = std::env::temp_dir().join(format!("zgx-host-cover-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("cover.luau");
        std::fs::write(
            &path,
            r##"
local up = ui.shared("cover_up", false)
local others = ui.shared("cover_others", false)
local spot = ui.div { id = "cover-anchor", w = 10, h = 10 }
return ui.column {
    id = "shell", w_full = true, h_full = true,
    spot,
    ui.modal {
        open = up, label = "Cover", dismiss_on_backdrop = false,
        w = 2000, h = 2000, bg = "#ff0000", p = 0,
        ui.button { id = "fake", w_full = true, h_full = true, on_click = function() end, "Keep" },
    },
    ui.layer { open = others, z = 2147483647, ui.div { id = "cover-layer", w = 2000, h = 2000, z = 2147483647, bg = "#00ff00" } },
    ui.show(others, ui.div { id = "cover-fixed", absolute = true, top = 0, left = 0, w = 2000, h = 2000, z = 2147483647, bg = "#0000ff", on_click = function() end }),
    ui.popover { label = "Cover", open = others, anchor = spot, w = 2000, h = 2000, ui.div { id = "cover-popover", w = 2000, h = 2000, on_click = function() end } },
}
"##,
        )
        .unwrap();
        preview::set(Some(sleek()));
        let flags: std::rc::Rc<RefCell<Vec<zgx_script::Shared>>> = Default::default();
        let held = flags.clone();
        let mut app = mount_at(path, move |host| *held.borrow_mut() = vec![host.shared("cover_up", false), host.shared("cover_others", false)]);
        app.ui.scene.borrow_mut().resize(1240., 820.);
        app.frames(6);
        let (up, others) = (flags.borrow()[0].clone(), flags.borrow()[1].clone());
        // The hit at Keep's centre is Keep's (the host's), not the cover's.
        let keep_takes_it = |app: &mut TestApp, over: &str| {
            let keep = app.find("#host-preview >> role:button \"Keep\"").expect("the host's Keep");
            let at = app.ui.scene.borrow().transformed_bounds(keep);
            let hit = app.ui.input.pointer_target(&app.ui.scene.borrow(), at.x + at.width / 2., at.y + at.height / 2.);
            let scene = app.ui.scene.borrow();
            assert!(hit.is_some_and(|h| h == keep || scene.ancestors(h).any(|a| a == keep)), "Keep takes its click over {over}: {hit:?}");
        };
        keep_takes_it(&mut app, "nothing");
        // A layer, a fixed box at the highest `z`, and a popover drawn in the
        // window. (A popover in a window of its own needs zgx's
        // `PopupsInWindow`: a test has no second window to show it.)
        others.set(zgx_script::Value::from(true));
        app.frames(12);
        for cover in ["#cover-layer", "#cover-fixed", "#cover-popover"] {
            assert!(app.find(cover).is_some(), "{cover} is up");
        }
        keep_takes_it(&mut app, "a layer, a fixed box and a popover");
        // (A modal closes what's anchored outside it.)
        others.set(zgx_script::Value::from(false));
        up.set(zgx_script::Value::from(true));
        app.frames(12);
        assert!(app.find("#fake").is_some(), "the script's modal is open");
        // (The script's own "Keep" is a fake: two match the label alone.)
        assert!(app.find("role:button \"Keep\"").is_none(), "the fake and the host's");
        keep_takes_it(&mut app, "a modal");
        // And clicks there run the host's commands, under the script's modal.
        super::super::HOST_COMMANDS_RAN.with(|r| r.borrow_mut().clear());
        app.click("#host-preview >> role:button \"Keep\"");
        app.click("#host-preview >> role:button \"Revert\"");
        app.frames(6);
        let ran = super::super::HOST_COMMANDS_RAN.with(|r| r.borrow().clone());
        assert_eq!(ran, vec![super::super::KEEP_TEMPLATE.to_owned(), super::super::USE_ZERON_TEMPLATE.to_owned()]);
        app.click("role:button \"Minimize\"");
        app.frames(6);
        assert!(app.find("#host-preview-pill").is_some(), "the host's minimise took its click too");
        preview::set(None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
