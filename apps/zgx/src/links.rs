//! `zeron://` links the system hands the app, as Zeron.app opens them
//! (crates/ui/src/lib.rs, state.rs `open_deep_link`), in the formats both
//! apps share (crates/notify):
//!   zeron://notification/<target>   a clicked banner (the engine posts them
//!                                   with the app as `ZERON_APP`): a chat, or
//!                                   agent updates (Settings → Providers)
//!   zeron://open/chat/<id>?workspace=<locator>
//!                                   a chat's link, opened only in the
//!                                   workspace it came from
//!   zeron://open/plugin/<id>[?version=<v>]
//!                                   a plugin of the store ("Install in
//!                                   Zeron" on its web page): its page in
//!                                   Settings → Plugins, Install ready (for
//!                                   version <v> when it names one). It
//!                                   only ever names a store id, checked as
//!                                   a plugin's id is (never a URL, a path
//!                                   or a file), and a version only as a
//!                                   version reads anywhere (else none);
//!                                   other query keys are ignored. It never
//!                                   installs: the person does, from the
//!                                   page
//!
//! What to open goes to the scripts as shared `app.link`,
//! `{ seq, chat?, settings?, tab?, plugin?, version? }` (`tab`: Settings →
//! Plugins' tab, for the host's own buttons, never from a link; `version`,
//! a plugin link's) (`seq` moving with each), which the
//! shell's route follows (ui/screens/shell/route.luau). The beta registers
//! for them (`sh.zeron.app`'s `zeron` scheme: dist/macos/Info.plist,
//! dist/zeron.desktop, dist/windows/zeron.iss, the whole scheme, so a new
//! path needs nothing new there); a launch with a link (`zeron <url>`)
//! opens it too. A link that arrives before the main window is up (a cold
//! start, or signed out: sign-in and setup come first) is kept, and the
//! main window opens it as it comes up (`LAST`).

use std::cell::{Cell, RefCell};

use zgx_script::{ScriptHost, Shared, Value};

/// Where a link goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Chat(String),
    Settings(&'static str),
    /// A store plugin's page, by its id, with the version a link chose.
    Plugin(String, Option<String>),
    /// A tab of Settings → Plugins ("yours": what a notice's button opens).
    PluginsTab(&'static str),
}

/// What a plugin link starts with.
const PLUGIN_LINK: &str = "zeron://open/plugin/";

/// A store id, as a plugin's id is (crates/engine plugins/manifest.rs
/// `check_id`): `^[a-z0-9][a-z0-9-]{1,39}$`.
fn store_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    (2..=40).contains(&bytes.len())
        && matches!(bytes[0], b'a'..=b'z' | b'0'..=b'9')
        && bytes[1..].iter().all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'-'))
}

/// The store id a plugin link names, `zeron://open/plugin/<id>` (a
/// trailing `/` allowed), and the version its query chooses (`?version=`,
/// as a version reads anywhere, else none; other keys ignored). Anything
/// more in its path (a fragment, another part, an escape) is no plugin
/// link.
fn plugin_link(url: &str) -> Option<(String, Option<String>)> {
    let rest = url.strip_prefix(PLUGIN_LINK)?;
    let (path, query) = match rest.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (rest, None),
    };
    let id = path.strip_suffix('/').unwrap_or(path);
    if !store_id(id) || query.is_some_and(|q| q.contains('#')) {
        return None;
    }
    let version = query
        .into_iter()
        .flat_map(|q| q.split('&'))
        .find_map(|pair| pair.strip_prefix("version="))
        .filter(|v| crate::plugins::consent::version_reads(v))
        .map(str::to_owned);
    Some((id.to_owned(), version))
}

/// What `url` asks for: the target and, for a conversation link, the
/// workspace it belongs to. None for anything else.
pub fn parse(url: &str) -> Option<(Target, Option<String>)> {
    if let Some((id, version)) = plugin_link(url) {
        return Some((Target::Plugin(id, version), None));
    }
    if url.starts_with(PLUGIN_LINK) {
        return None;
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
}

/// A window's `app.link`, and `app.link_followed(seq)`: the main window
/// followed that link, so a window made later (after signing out and in)
/// isn't sent to it again.
pub fn install(host: &ScriptHost) {
    let last = LAST.with(|l| l.borrow().clone());
    WINDOWS.with(|w| w.borrow_mut().push(host.shared("app.link", last)));
    host.action("app.link_followed", |seq| {
        if let Value::Num(seq) = seq {
            followed(seq);
        }
    });
}

/// The link numbered `seq` was followed: kept as its number alone.
fn followed(seq: f64) {
    LAST.with(|l| {
        let mut last = l.borrow_mut();
        if matches!(last.field("seq"), Some(Value::Num(n)) if *n == seq) {
            *last = Value::record([("seq", Value::from(seq))]);
        }
    });
}

/// Open `url`: a banner's target at once; a chat's link once the engine says
/// which workspace is open, and only if it's the link's.
pub fn open(url: &str) {
    let Some((target, workspace)) = parse(url) else {
        eprintln!("zeron-zgx: links: not a Zeron link: {url}");
        return;
    };
    let Some(workspace) = workspace else {
        // (A plugin's page, asked from a web page: Zeron comes forward on
        // it, as for a banner. LaunchServices may already have.)
        if matches!(target, Target::Plugin(..)) {
            activate();
        }
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

/// Somewhere in the app the host itself opens (a notice's button): no
/// link was handed in, so nothing to read or check.
pub fn show(target: Target) {
    tell(target);
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
    match target {
        Target::Chat(id) => fields.push(("chat", Value::from(id.as_str()))),
        Target::Settings(page) => fields.push(("settings", Value::from(page))),
        Target::Plugin(id, version) => {
            fields.push(("plugin", Value::from(id.as_str())));
            if let Some(version) = version {
                fields.push(("version", Value::from(version.as_str())));
            }
        }
        Target::PluginsTab(tab) => {
            fields.push(("settings", Value::from("plugins")));
            fields.push(("tab", Value::from(tab)));
        }
    }
    let value = Value::record(fields);
    LAST.with(|l| *l.borrow_mut() = value.clone());
    WINDOWS.with(|w| {
        for shared in w.borrow().iter() {
            shared.set(value.clone());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(k: &str) -> Option<String> {
        LAST.with(|l| match l.borrow().field(k) {
            Some(Value::Str(s)) => Some(s.to_string()),
            Some(Value::Text(s)) => Some(s.to_string()),
            _ => None,
        })
    }

    fn last() -> (Option<String>, Option<String>) {
        (field("chat"), field("settings"))
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

    /// A notice's "Deploy keys" opens Settings → Plugins on Yours.
    #[test]
    fn the_host_opens_a_plugins_tab() {
        show(Target::PluginsTab("yours"));
        assert_eq!(last(), (None, Some("plugins".into())));
        assert_eq!(field("tab").as_deref(), Some("yours"));
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

    /// A plugin's link names a store id and nothing else: its page, never a
    /// URL, a path, a file or anything to run.
    #[test]
    fn a_plugin_link_names_a_store_id_alone() {
        for (url, id) in [
            ("zeron://open/plugin/focus-timer", "focus-timer"),
            ("zeron://open/plugin/focus-timer/", "focus-timer"),
            ("zeron://open/plugin/a1", "a1"),
            ("zeron://open/plugin/9lives", "9lives"),
        ] {
            assert_eq!(parse(url), Some((Target::Plugin(id.into(), None), None)), "{url}");
        }
        let forty = "a".repeat(40);
        assert_eq!(parse(&format!("{PLUGIN_LINK}{forty}")), Some((Target::Plugin(forty.clone(), None), None)));
        for url in [
            "zeron://open/plugin/",
            "zeron://open/plugin/a",
            "zeron://open/plugin/Focus",
            "zeron://open/plugin/-focus",
            "zeron://open/plugin/focus_timer",
            "zeron://open/plugin/focus%2Ftimer",
            "zeron://open/plugin/focus/../../etc",
            "zeron://open/plugin/focus/settings",
            "zeron://open/plugin/focus#install",
            "zeron://open/plugin/focus?version=1.2.0#install",
            "zeron://open/plugin/focus/settings?version=1.2.0",
            "zeron://open/plugin/https://evil.example/x.zeron-plugin",
            "zeron://open/plugin/file:///tmp/x.zeron-plugin",
            "zeron://open/plugin/focus timer",
            "zeron://open/plugin/fócus",
            "zeron://open/plugin//focus",
            "ZERON://open/plugin/focus",
            "zeron://open/plugins/focus",
        ] {
            assert_eq!(parse(url), None, "{url}");
        }
        assert_eq!(parse(&format!("{PLUGIN_LINK}{forty}a")), None, "41 characters");
    }

    /// A plugin link may choose a version (`?version=`): taken only as a
    /// version reads anywhere, else the page opens as with none; any other
    /// key is ignored.
    #[test]
    fn a_plugin_link_may_choose_a_version() {
        let page = |id: &str, version: Option<&str>| Some((Target::Plugin(id.into(), version.map(str::to_owned)), None));
        for (url, version) in [
            ("zeron://open/plugin/focus?version=1.2.0", Some("1.2.0")),
            ("zeron://open/plugin/focus/?version=0.3.0-beta.1", Some("0.3.0-beta.1")),
            ("zeron://open/plugin/focus?install=1&version=1.2.0&from=web", Some("1.2.0")),
            // Other keys alone: none chosen.
            ("zeron://open/plugin/focus?install=1", None),
            ("zeron://open/plugin/focus?", None),
            // Not a version: as if the link had none.
            ("zeron://open/plugin/focus?version=", None),
            ("zeron://open/plugin/focus?version=1.2.0%20", None),
            ("zeron://open/plugin/focus?version=../../etc", None),
            ("zeron://open/plugin/focus?version=1.2.0;rm", None),
            ("zeron://open/plugin/focus?version=https://evil.example", None),
        ] {
            assert_eq!(parse(url), page("focus", version), "{url}");
        }
        let long = format!("zeron://open/plugin/focus?version=1.{}", "0".repeat(40));
        assert_eq!(parse(&long), page("focus", None), "longer than a version");
        // Told to the scripts beside its plugin.
        tell(Target::Plugin("focus".into(), Some("1.2.0".into())));
        assert_eq!((field("plugin").as_deref(), field("version").as_deref()), (Some("focus"), Some("1.2.0")));
        tell(Target::Plugin("focus".into(), None));
        assert_eq!(field("version"), None);
    }

    /// Told to the scripts as `plugin`, and kept for a main window that
    /// comes up later (a cold start, or signed out until set up).
    #[test]
    fn a_plugin_link_opens_its_page_and_is_kept_for_a_window_to_come() {
        tell(Target::Plugin("focus-timer".into(), None));
        assert_eq!(field("plugin").as_deref(), Some("focus-timer"));
        assert_eq!(last(), (None, None));
        // (A window installed now starts from it: `install` reads `LAST`.)
        let kept = LAST.with(|l| l.borrow().clone());
        let Some(Value::Num(seq)) = kept.field("seq").cloned() else { panic!("{kept:?}") };
        assert!(seq >= 1.0);
        // Followed by the main window: a later window isn't sent there again.
        followed(seq - 1.0);
        assert_eq!(field("plugin").as_deref(), Some("focus-timer"), "only the link it followed");
        followed(seq);
        assert_eq!(field("plugin"), None);
        assert!(matches!(LAST.with(|l| l.borrow().field("seq").cloned()), Some(Value::Num(n)) if n == seq));
    }
}
