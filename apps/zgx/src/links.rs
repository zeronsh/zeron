//! `zeron://` links the system hands the app, as Zeron.app opens them
//! (crates/ui/src/lib.rs, state.rs `open_deep_link`), in the formats both
//! apps share (crates/notify):
//!   zeron://notification/<target>   a clicked banner (the engine posts them
//!                                   with the app as `ZERON_APP`): a chat, or
//!                                   agent updates (Settings → Providers)
//!   zeron://open/chat/<id>?workspace=<locator>
//!                                   a chat's link, opened only in the
//!                                   workspace it came from
//!
//! What to open goes to the scripts as shared `app.link`,
//! `{ seq, chat?, settings? }` (`seq` moving with each), which the shell's
//! route follows (ui/screens/shell/route.luau). The beta registers for them
//! (`sh.zeron.app`'s `zeron` scheme, src/main.rs); a launch with a link
//! (`zeron <url>`) opens it too.

use std::cell::{Cell, RefCell};

use zgx_script::{ScriptHost, Shared, Value};

/// Where a link goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Chat(String),
    Settings(&'static str),
}

/// What `url` asks for: the target and, for a conversation link, the
/// workspace it belongs to. None for anything else.
pub fn parse(url: &str) -> Option<(Target, Option<String>)> {
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

/// A window's `app.link`.
pub fn install(host: &ScriptHost) {
    let last = LAST.with(|l| l.borrow().clone());
    WINDOWS.with(|w| w.borrow_mut().push(host.shared("app.link", last)));
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

fn tell(target: Target) {
    let seq = SEQ.with(|s| {
        s.set(s.get() + 1);
        s.get()
    });
    let mut fields = vec![("seq", Value::from(seq as f64))];
    match target {
        Target::Chat(id) => fields.push(("chat", Value::from(id.as_str()))),
        Target::Settings(page) => fields.push(("settings", Value::from(page))),
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
}
