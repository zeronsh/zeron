//! Where plugins can go: Zeron's named elements, each with what it is, where
//! a person would say it is, and what it gives to what's built in it (its
//! `provide`). A plugin places its nodes by selector (`#id`, `.class`, a
//! role and a name), so this is the map agents write selectors from
//! (`describe_elements`) and the words a problem is told in ("…but it's in
//! the title bar"). The app's own chats give these elements their ids,
//! classes and provides; templates may rename them, which is why each
//! plugin's literal selectors are checked against what the window running
//! now declares ("Not shown in this template").

use std::collections::BTreeSet;

/// A named element: its selector, where it is in words, what it is, and the
/// names of what it gives to what's built in it.
pub struct Element {
    pub selector: &'static str,
    pub place: &'static str,
    pub about: &'static str,
    pub provides: &'static [&'static str],
}

const fn e(selector: &'static str, place: &'static str, about: &'static str, provides: &'static [&'static str]) -> Element {
    Element { selector, place, about, provides }
}

/// Zeron's named elements. Everything in the window also has `appearance`
/// and `route` (`#shell`'s). What a place gives is an id or a plain value,
/// never a record: a session's, a message's or a project's id, a file's or
/// a block's token (`tokens`), each found in the plugin's records as far
/// as its grants reach.
pub const ELEMENTS: &[Element] = &[
    e("#shell", "the window", "The main window: everything below is in it.", &["appearance", "route"]),
    e("#sidebar", "the sidebar", "The sidebar's list of sessions.", &["sessions"]),
    e(".session-row", "a sidebar row", "A session's row in the sidebar.", &["session"]),
    e(".session-title", "a sidebar row", "A session row's title.", &[]),
    e(".section-header", "the sidebar", "A project's, device's or section's header in the sidebar.", &["project"]),
    // A project's tile wherever it's drawn (ui/ds/tile.luau's `project`): its
    // box gives `project`, so a replacement draws each project's own.
    e(".project-icon", "a project's icon", "A project's icon, wherever one shows: the sidebar's groups and rows, tabs, the composer's and pickers' project, the panel, the palette. Replace it (`ui.change`) to draw your own; `project` is its project (`{ id }`, and with \"sessions.read\" its name, device and `icon`).", &["project"]),
    e("#sidebar-actions", "the sidebar", "New chat and Search, at the sidebar's top.", &[]),
    e("#titlebar", "the title bar", "The title bar.", &[]),
    e("#titlebar-actions", "the title bar", "The title bar's actions, at its end.", &[]),
    e("#rail", "the sidebar's rail", "The collapsed sidebar's rail.", &[]),
    // (Not Settings: sealed, no plugin's to change or add to.)
    e("#chat-home", "a new chat", "A new chat, before it's sent: no session here.", &[]),
    // What a new chat opens on, when Settings → Appearance's Wallpaper names
    // the plugin replacing it (`ui.change`): Zeron's effect is drawn over it.
    e("#new-chat-background", "a new chat's background", "Behind a new chat, under Zeron's effect: replace it to draw the background. `pointer` is { x, y }, 0 to 1 (0.5 at rest).", &["pointer"]),
    e("#chat-session", "a chat", "A session's page: everything in it has its session.", &["session"]),
    e("#session-header", "a chat's header", "A session page's header.", &[]),
    e("#transcript", "a chat", "A session's transcript.", &[]),
    e(".message", "a message", "A message in the transcript (.message-user, .message-assistant).", &["message"]),
    e(".message-actions", "a message", "A message's action row.", &[]),
    e("#chat-composer", "the message box", "The composer.", &[]),
    e("#composer-toolbar", "the message box", "The composer's toolbar.", &[]),
    e("#files-pane", "the files pane", "The files pane.", &[]),
    e("#file-tree", "the files pane", "The files pane's tree.", &[]),
    e("#files-header", "the files pane", "The files pane's header.", &[]),
    e(".file-row", "a file's row", "A file's or folder's row in the tree.", &["file"]),
    e(".file-tab", "a file's tab", "An open file's tab.", &["file"]),
    e("#file-viewer", "the open file", "The open file.", &["file"]),
    e("#panel", "the side panel", "The side panel.", &[]),
    e("#panel-strip", "the side panel", "The side panel's tabs.", &[]),
    e(".panel-tab", "the side panel", "A tab of the side panel.", &["panel"]),
    e("#panel-empty", "the side panel", "The side panel with nothing open.", &[]),
    // A reply's blocks, as markdown draws them (ui/ds/markdown.luau): a
    // renderer of its own replaces one (`.md-code.lang-mermaid`).
    e(".md-code", "a reply's code block", "A fenced code block, also .lang-<its language> (.md-code.lang-mermaid). `code` is { lang, source }, for a plugin allowed \"messages.read\" or to change these blocks.", &["code"]),
    e(".md-math", "a reply's formula", "Display math ($$…$$ or a ```math fence), drawn or, where it can't be, as code. `tex` is its TeX, for a plugin allowed \"messages.read\" or to change these blocks.", &["tex"]),
    e(".md-table", "a reply's table", "A table in a reply.", &[]),
    e(".md-image", "a reply's picture", "A picture in a reply (its alt text where it isn't shown).", &[]),
    e(".md-quote", "a reply's quote", "A quote in a reply.", &[]),
    e(".md-heading", "a reply's heading", "A heading in a reply.", &[]),
];

/// The names Zeron and every template promise to keep: a plugin placed by
/// one of these keeps working across Zeron's updates and templates. The rest
/// may move.
pub const STABLE: &[&str] = &[
    "#sidebar",
    ".session-row",
    ".project-icon",
    "#session-header",
    ".message",
    "#chat-composer",
    "#new-chat-background",
    ".file-row",
    ".file-tab",
    ".panel-tab",
    ".md-code",
    ".md-math",
    ".md-table",
    ".md-image",
    ".md-quote",
    ".md-heading",
];

/// The selector's last part's id and classes (`#a`, `.b`, `x.c`; a role and
/// name or a text has neither): what must exist for it to match.
pub fn parts(selector: &str) -> (Option<String>, Vec<String>) {
    let last = selector.rsplit(">>").next().unwrap_or(selector).trim();
    if last.starts_with("role:") || last.starts_with('"') || last.starts_with("hook:") {
        return (None, vec![]);
    }
    let mut id = None;
    let mut classes = Vec::new();
    // Each `#name` or `.name` run (a leading tag name, `row`, is neither).
    let mut mark: Option<char> = None;
    let mut word = String::new();
    let mut flush = |mark: Option<char>, word: &mut String| {
        match mark {
            Some('#') if !word.is_empty() => id = Some(std::mem::take(word)),
            Some('.') if !word.is_empty() => classes.push(std::mem::take(word)),
            _ => word.clear(),
        }
    };
    for c in last.chars() {
        if c == '#' || c == '.' {
            flush(mark, &mut word);
            mark = Some(c);
        } else {
            word.push(c);
        }
    }
    flush(mark, &mut word);
    (id, classes)
}

/// Where `selector` is in words: its element's place, or "part of Zeron".
pub fn place(selector: &str) -> &'static str {
    let last = selector.rsplit(">>").next().unwrap_or(selector).trim();
    ELEMENTS
        .iter()
        .find(|e| e.selector == last || last.starts_with(e.selector))
        .map(|e| e.place)
        .unwrap_or("part of Zeron")
}

/// The ids and classes zgx's report of the window names (mounted now, or
/// declared in its source and not showing), and the report as agents read
/// it: each part of the window that gives something (`provide`), what it
/// gives and what's named in it. (Private parts list nothing inside.)
pub fn from_report(report: &zgx_script::ElementsReport) -> (BTreeSet<String>, BTreeSet<String>, serde_json::Value) {
    let (mut ids, mut classes) = (BTreeSet::new(), BTreeSet::new());
    fn group(g: &zgx_script::ElementGroup, ids: &mut BTreeSet<String>, classes: &mut BTreeSet<String>) -> serde_json::Value {
        let elements: Vec<serde_json::Value> = g
            .elements
            .iter()
            .map(|e| {
                ids.extend(e.id.clone());
                classes.extend(e.classes.iter().cloned());
                let mut out = serde_json::json!({ "classes": e.classes, "showing": e.mounted });
                if let Some(id) = &e.id {
                    out["id"] = serde_json::json!(format!("#{id}"));
                }
                if e.private {
                    out["private"] = serde_json::json!(true);
                }
                if e.count > 1 {
                    out["count"] = serde_json::json!(e.count);
                }
                out
            })
            .collect();
        let groups: Vec<serde_json::Value> = g.groups.iter().map(|g| group(g, ids, classes)).collect();
        serde_json::json!({
            "selector": g.selector,
            "provides": g.provides,
            "inherited": g.inherited,
            "count": g.count,
            "elements": elements,
            "groups": groups,
        })
    }
    let tree = group(&report.root, &mut ids, &mut classes);
    (ids, classes, tree)
}

/// Of a plugin's selectors, those nothing in the window declares: the
/// places it won't show (by their words).
pub fn not_shown(selectors: &BTreeSet<String>, ids: &BTreeSet<String>, classes: &BTreeSet<String>) -> Vec<String> {
    let mut out = Vec::new();
    for s in selectors {
        let (id, cls) = parts(s);
        let missing = id.is_some_and(|i| !ids.contains(&i)) || cls.iter().any(|c| !classes.contains(c));
        if missing && !out.contains(&s.clone()) {
            out.push(s.clone());
        }
    }
    out
}

/// The map `describe_elements` gives agents, with whether each element is
/// in the window running now.
pub fn describe(ids: &BTreeSet<String>, classes: &BTreeSet<String>, tree: serde_json::Value) -> serde_json::Value {
    let elements: Vec<serde_json::Value> = ELEMENTS
        .iter()
        .map(|el| {
            let (id, cls) = parts(el.selector);
            let here = id.is_none_or(|i| ids.contains(&i)) && cls.iter().all(|c| classes.contains(c));
            serde_json::json!({
                "selector": el.selector,
                "place": el.place,
                "about": el.about,
                "provides": el.provides,
                "present": here,
                "stable": STABLE.contains(&el.selector),
            })
        })
        .collect();
    serde_json::json!({
        "elements": elements,
        "everywhere": ["appearance", "route"],
        "window": tree,
        "stable": "Elements marked stable keep their names across Zeron's updates, and every template keeps them: place by these where you can. Others may move.",
        "how": "Place a node by selector: zeron.place { at = \".session-row\", after = function() return ui.text { … } end }. \
                Selectors: #id, .class, `#a >> .b` (inside), role:button \"Send\". What's built at a place reads what it gives \
                with zeron.context(name) (nil where absent) or zeron.need(name) (draws nothing there, and Settings says why). \
                Records carry only what the plugin was allowed: sessions.read, messages.read, profile.read, account.email, files.read.",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(v: &[&str]) -> BTreeSet<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_selector_s_id_and_classes() {
        assert_eq!(parts("#titlebar-actions"), (Some("titlebar-actions".into()), vec![]));
        assert_eq!(parts(".session-row"), (None, vec!["session-row".into()]));
        assert_eq!(parts("#sidebar >> .session-row.active"), (None, vec!["session-row".into(), "active".into()]));
        assert_eq!(parts("row.x#y"), (Some("y".into()), vec!["x".into()]));
        assert_eq!(parts(r#"role:button "Send""#), (None, vec![]));
    }

    #[test]
    fn places_are_told_in_words() {
        assert_eq!(place("#titlebar-actions"), "the title bar");
        assert_eq!(place("#sidebar >> .session-row"), "a sidebar row");
        assert_eq!(place("#nowhere"), "part of Zeron");
        assert_eq!(place("#transcript >> .md-code.lang-mermaid"), "a reply's code block");
    }

    #[test]
    fn a_selector_nothing_declares_is_not_shown() {
        let (ids, classes) = (set(&["titlebar-actions"]), set(&["session-row"]));
        let missing = not_shown(&set(&["#titlebar-actions", ".session-row", ".message", "#gone", r#"role:button "Send""#]), &ids, &classes);
        assert_eq!(missing, vec!["#gone".to_string(), ".message".to_string()]);
    }

    #[test]
    fn the_map_says_what_s_here() {
        let map = describe(&set(&["shell", "titlebar-actions"]), &set(&[]), serde_json::json!({}));
        let els = map["elements"].as_array().unwrap();
        let bar = els.iter().find(|e| e["selector"] == "#titlebar-actions").unwrap();
        assert_eq!(bar["present"], true);
        let row = els.iter().find(|e| e["selector"] == ".session-row").unwrap();
        assert_eq!(row["present"], false);
        assert_eq!(row["provides"], serde_json::json!(["session"]));
        assert_eq!((row["stable"].as_bool(), bar["stable"].as_bool()), (Some(true), Some(false)));
        let icon = els.iter().find(|e| e["selector"] == ".project-icon").unwrap();
        assert_eq!((icon["stable"].as_bool(), &icon["provides"]), (Some(true), &serde_json::json!(["project"])));
        for s in STABLE {
            assert!(ELEMENTS.iter().any(|e| e.selector == *s), "{s} is stable but not an element");
        }
    }
}
