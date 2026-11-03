//! Markdown's blocks as places for plugins: a reply (ui/md_blocks_fixture.luau)
//! with plugins-dev/md-hello loaded as the bridge loads a snapshot, or not.
//! Each ```hello block is the plugin's card, built from the `code` its block
//! gives, block by block and as a block streams in; with the plugin off,
//! each is the code block it was.

use std::path::PathBuf;

use serde_json::json;
use zgx_script::{ScriptHost, testing::TestApp};

use super::{Item, expose_own, expose_surface, json_value, records};

fn md_hello(status: &str) -> Item {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("plugins-dev").join("md-hello");
    let manifest: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("zeron-plugin.json")).unwrap()).unwrap();
    Item {
        json: json!({
            "id": "md-hello", "name": manifest["name"], "status": status, "entry": manifest["entry"],
            "snapshot": dir, "permissions": manifest["permissions"], "ui": manifest["ui"],
        }),
    }
}

/// The reply, with `items` loaded (those that are on).
fn reply(items: Vec<Item>) -> TestApp {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/md_blocks_fixture.luau");
    let app = TestApp::new(move |runtime| {
        let host = ScriptHost::new(runtime, path);
        expose_surface(&host);
        let mut plugins = Vec::new();
        for item in &items {
            expose_own(&host, item.id());
            // Its records, as the bridge publishes them: allowed to change
            // how its blocks show, it reads what they hold.
            let records = records::records_with(&item.permissions(), &item.change(), &Default::default());
            host.shared(&format!("plugin.{}.records", item.id()), zgx_script::Value::Nil).set(json_value(&records));
            for name in ["problem", "result", "ack", "cost"] {
                host.action(format!("plugin.{}.{name}", item.id()), |_| {});
            }
            plugins.extend(item.plugin());
        }
        host.set_plugins(plugins);
        host
    });
    app.ui.scene.borrow_mut().resize(900., 1200.);
    app.frames(30);
    app
}

fn count(tree: &str, what: &str) -> usize {
    tree.matches(what).count()
}

#[test]
fn a_plugin_replaces_each_block_of_its_language_and_reads_its_source() {
    let mut app = reply(vec![md_hello("on")]);
    assert_eq!(app.error(), None, "the reply with the plugin failed");
    let tree = app.tree();
    // Both ```hello blocks are its cards, each with its own source.
    assert_eq!(count(&tree, "text \"Hello\""), 2, "{tree}");
    assert!(tree.contains("\"first block\"") && tree.contains("\"second\""), "{tree}");
    // The Lua block is still a code block (the only one with Copy left).
    assert_eq!(count(&tree, "button \"Copy code\""), 1, "{tree}");
    assert!(tree.contains("print(1)"), "{tree}");
}

#[test]
fn a_block_streaming_in_is_drawn_again_as_it_grows() {
    let mut app = reply(vec![md_hello("on")]);
    app.host.run_command("stream");
    app.frames(20);
    let tree = app.tree();
    assert!(tree.contains("\"second block, streaming\""), "{tree}");
    assert!(tree.contains("\"first block\""), "the block before it is as it was: {tree}");
    assert_eq!(count(&tree, "text \"Hello\""), 2, "{tree}");
}

#[test]
fn with_the_plugin_off_each_block_is_the_code_block_it_was() {
    for items in [vec![], vec![md_hello("off")]] {
        let mut app = reply(items);
        assert_eq!(app.error(), None);
        let tree = app.tree();
        assert_eq!(count(&tree, "text \"Hello\""), 0, "{tree}");
        assert_eq!(count(&tree, "button \"Copy code\""), 3, "lua and both hello blocks: {tree}");
        assert!(tree.contains("first block") && tree.contains("second"), "{tree}");
    }
}
