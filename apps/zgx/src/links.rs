//! `zeron://` links the system hands the app, as Zeron.app opens them
//! (crates/ui/src/lib.rs, state.rs `open_deep_link`), in the formats both
//! apps share (crates/notify):
//!   zeron://notification/<target>   a clicked banner (the engine posts them
//!                                   with the app as `ZERON_APP`): a chat, or
//!                                   agent updates (Settings → Providers)
//!   zeron://open/chat/<id>?workspace=<locator>
//!                                   a chat's link, opened only in the
//!                                   workspace it came from
//!   zeron://server?address=<address>
//!                                   a self-hosted server's invite: its
//!                                   dialog, on the address (never a switch
//!                                   without a click)
//!
//! What to open goes to the scripts as shared `app.link`,
//! `{ seq, chat?, settings?, server? }` (`seq` moving with each), which the
//! shell's route follows (ui/screens/shell/route.luau), and setup's for an
//! invite (ui/screens/onboarding/init.luau). The beta registers for them
//! (`sh.zeron.app`'s `zeron` scheme, src/main.rs); a launch with a link
//! (`zeron <url>`) opens it too.

use std::cell::{Cell, RefCell};

use zgx_script::{ScriptHost, Shared, Value};

/// Where a link goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Chat(String),
    Settings(&'static str),
    /// An invite's address.
    Server(String),
}

/// What `url` asks for: the target and, for a conversation link, the
/// workspace it belongs to. None for anything else.
pub fn parse(url: &str) -> Option<(Target, Option<String>)> {
    if let Ok(address) = zeron_notify::links::parse_server_invite(url) {
        return Some((Target::Server(address), None));
    }
    if let Some(target) = zeron_notify::link_target(url) {
        return Some(if target == zeron_notify::AGENT_UPDATES_TARGET {
            (Target::Settings("providers"), None)
        } else {
            (Target::Chat(target.to_owned()), None)
        });
    }
    let link = zeron_notify::links::parse_conversation_link(url).ok()?;
    Some((Target::Chat(link.chat_id), Some(link.workspace)))
}

thread_local! {
    static WINDOWS: RefCell<Vec<Shared>> = const { RefCell::new(Vec::new()) };
    static SEQ: Cell<u64> = const { Cell::new(0) };
    static LAST: RefCell<Value> = RefCell::new(Value::record([("seq", Value::from(0.0))]));
    /// An invite that came with no window to take it (a launch by the
    /// link, the intro still playing): the next window's, once.
    static PENDING: RefCell<Option<Value>> = const { RefCell::new(None) };
}

/// A window's `app.link`: an invite that waited for one, else the last
/// link (a chat opened from setup is the main window's too).
pub fn install(host: &ScriptHost) {
    let first = PENDING
        .with(|p| p.borrow_mut().take())
        .unwrap_or_else(|| LAST.with(|l| l.borrow().clone()));
    WINDOWS.with(|w| w.borrow_mut().push(host.shared("app.link", first)));
}

/// Open `url`: a banner's target at once; a chat's link once the engine says
/// which workspace is open, and only if it's the link's.
pub fn open(url: &str) {
    let Some((target, workspace)) = parse(url) else {
        eprintln!("zeron-zgx: links: not a Zeron link: {url}");
        return;
    };
    let Some(workspace) = workspace else {
        tell(target);
        return;
    };
    crate::engine::get().spawn(async move {
        match crate::sessions::current_locator().await {
            Some(current) if current == workspace => tell(target),
            Some(_) => eprintln!("zeron-zgx: links: this conversation link belongs to another workspace"),
            None => eprintln!("zeron-zgx: links: the workspace isn't known yet; the link wasn't opened"),
        }
    });
}

/// Banner clicks, taken in this process. The engine posts Zeron's banners,
/// but macOS hands a click to the app its bundle names (Zeron.app's
/// interface, this process), never to the engine that posted it, so the
/// engine's own route (`open -a … zeron://notification/…`) never runs. As
/// Zeron.app's interface always has (crates/ui `notify::on_click`), the
/// click is answered here: Zeron comes forward, on the banner's chat.
pub fn take_banner_clicks() {
    zeron_notify::on_click(|target| {
        activate();
        clicked(&target);
    });
}

/// A clicked banner's target (a chat, or agent updates), opened as its
/// `zeron://notification/` link is.
fn clicked(target: &str) {
    open(&zeron_notify::link(target));
}

/// This app to the front: a banner was clicked. (Main thread: the click
/// comes through the AppKit delegate.)
#[cfg(target_os = "macos")]
fn activate() {
    use objc::{class, msg_send, sel, sel_impl};
    unsafe {
        let app: *mut objc::runtime::Object = msg_send![class!(NSApplication), sharedApplication];
        if !app.is_null() {
            let _: () = msg_send![app, activateIgnoringOtherApps: true];
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn activate() {}

fn tell(target: Target) {
    let seq = SEQ.with(|s| {
        s.set(s.get() + 1);
        s.get()
    });
    let mut fields = vec![("seq", Value::from(seq as f64))];
    // (An invite is the windows' open now, or the next one's: never handed
    // to a later one as the last link, which would ask again.)
    let invite = matches!(target, Target::Server(_));
    match target {
        Target::Chat(id) => fields.push(("chat", Value::from(id.as_str()))),
        Target::Settings(page) => fields.push(("settings", Value::from(page))),
        Target::Server(address) => fields.push(("server", Value::from(address.as_str()))),
    }
    let value = Value::record(fields);
    if !invite {
        LAST.with(|l| *l.borrow_mut() = value.clone());
    } else if WINDOWS.with(|w| w.borrow().is_empty()) {
        PENDING.with(|p| *p.borrow_mut() = Some(value));
        return;
    }
    WINDOWS.with(|w| {
        for shared in w.borrow().iter() {
            shared.set(value.clone());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: &Value, key: &str) -> Option<String> {
        match value.field(key) {
            Some(Value::Str(s)) => Some(s.to_string()),
            Some(Value::Text(s)) => Some(s.to_string()),
            _ => None,
        }
    }

    fn last() -> (Option<String>, Option<String>) {
        LAST.with(|l| {
            let l = l.borrow();
            (text(&l, "chat"), text(&l, "settings"))
        })
    }

    /// A banner clicked in this process (macOS hands the click to the app,
    /// not the engine that posted it) opens its chat as its link would.
    #[test]
    fn a_clicked_banner_opens_its_chat() {
        clicked("chat-42");
        assert_eq!(last(), (Some("chat-42".into()), None));
        clicked(zeron_notify::AGENT_UPDATES_TARGET);
        assert_eq!(last(), (None, Some("providers".into())));
    }

    #[test]
    fn banners_and_chat_links_open_where_zeron_s_do() {
        assert_eq!(parse(&zeron_notify::link("chat-1")), Some((Target::Chat("chat-1".into()), None)));
        assert_eq!(
            parse(&zeron_notify::link(zeron_notify::AGENT_UPDATES_TARGET)),
            Some((Target::Settings("providers"), None))
        );
        let link = zeron_notify::links::conversation_link("c 1", "abc");
        assert_eq!(parse(&link), Some((Target::Chat("c 1".into()), Some("abc".into()))));
        assert_eq!(parse("https://example.com"), None);
        assert_eq!(parse("zeron://open/chat/id"), None);
    }

    #[test]
    fn an_invite_names_its_server() {
        let link = zeron_notify::links::server_invite_link("https://zeron.acme.com");
        assert_eq!(parse(&link), Some((Target::Server("https://zeron.acme.com".into()), None)));
        assert_eq!(parse("zeron://server?address="), None);
    }

    /// Clicked before any window can take it (a launch by the link), an
    /// invite waits for the first; it never stands as the last link, which
    /// every later window is handed (setup's, then the main one's).
    #[test]
    fn an_invite_waits_for_a_window_and_is_never_handed_on() {
        clicked("chat-7");
        open(&zeron_notify::links::server_invite_link("zeron.acme.com"));
        assert_eq!(last(), (Some("chat-7".into()), None));
        let waiting = PENDING.with(|p| p.borrow_mut().take()).expect("kept for the next window");
        assert_eq!(text(&waiting, "server").as_deref(), Some("zeron.acme.com"));
    }
}
