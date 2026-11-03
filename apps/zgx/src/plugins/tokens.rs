//! What the app's elements give plugins of a file or a reply's block, as
//! ids: a file's row gives `file = <token>`, never its path; a code block
//! `code = <token>`, never its source. zgx copies whatever an element
//! provides into any plugin VM that asks (`ui.use_context`), whatever it
//! was allowed, so what's provided is only ever an id, and a plugin finds
//! what's behind it here, through its own function, holding only what its
//! records say it may read:
//!
//! - `plugin.<id>.file(token)`: `{ path, name }` with "files.read"
//!   (`files_read` in its records), else nothing;
//! - `plugin.<id>.block(token)`: a block's `{ lang, source }` (or a
//!   formula's TeX) with "messages.read", or for a plugin allowed to change
//!   how those blocks show (`blocks_read`), else nothing.
//!
//! The app's scripts make tokens with `zeron.file_token(path)` and
//! `zeron.block_token(value)` (`ui.call_sync`), which no plugin reaches
//! (never exposed). A token is a keyed hash, its key this run's own: the
//! same file the same token, all run long; nothing in it says the path.

use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    hash::{BuildHasher, RandomState},
};

use zgx_script::{ScriptHost, Shared, Value};

/// How much is remembered, by weight (a file one each, a block its
/// bytes: a block streaming in is a new one with each part), the oldest
/// going first.
const FILES: usize = 50_000;
const BLOCKS: usize = 16 << 20;

/// Tokens made, oldest first, and what each stands for and weighs.
struct Bounded {
    map: HashMap<String, (Value, usize)>,
    order: VecDeque<String>,
    weight: usize,
    most: usize,
}

impl Bounded {
    fn new(most: usize) -> Self {
        Self { map: HashMap::new(), order: VecDeque::new(), weight: 0, most }
    }
    fn keep(&mut self, token: &str, value: Value, weight: usize) {
        if self.map.contains_key(token) {
            return;
        }
        self.map.insert(token.to_string(), (value, weight));
        self.order.push_back(token.to_string());
        self.weight += weight;
        while self.weight > self.most && self.order.len() > 1 {
            if let Some((_, w)) = self.order.pop_front().and_then(|old| self.map.remove(&old)) {
                self.weight -= w;
            }
        }
    }
    fn get(&self, token: &str) -> Option<&Value> {
        self.map.get(token).map(|(v, _)| v)
    }
}

struct Tokens {
    key: RandomState,
    files: RefCell<Bounded>,
    blocks: RefCell<Bounded>,
}

thread_local! {
    static TOKENS: Tokens = Tokens {
        key: RandomState::new(),
        files: RefCell::new(Bounded::new(FILES)),
        blocks: RefCell::new(Bounded::new(BLOCKS)),
    };
}

/// The token for the file at `path`.
pub fn file(path: &str) -> String {
    TOKENS.with(|t| {
        let token = format!("f{:016x}", t.key.hash_one(("file", path)));
        t.files.borrow_mut().keep(&token, Value::from(path), 1);
        token
    })
}

/// The token for a block's `value` (`{ lang, source }`, or TeX).
pub fn block(value: &Value) -> String {
    TOKENS.with(|t| {
        let json = value.to_json();
        let token = format!("b{:016x}", t.key.hash_one(("block", &json)));
        t.blocks.borrow_mut().keep(&token, value.clone(), json.len());
        token
    })
}

/// The path behind a file's token, if it's one made this run.
pub fn path_of(token: &str) -> Option<String> {
    TOKENS.with(|t| t.files.borrow().get(token).map(crate::auth::text))
}

/// The block behind a block's token.
pub fn block_of(token: &str) -> Option<Value> {
    TOKENS.with(|t| t.blocks.borrow().get(token).cloned())
}

/// The app's: making tokens (`zeron.file_token`, `zeron.block_token`).
/// Never exposed, so no plugin calls them.
pub fn install(host: &ScriptHost) {
    host.function("zeron.file_token", |path: Value| match path {
        Value::Str(path) => Value::from(file(&path).as_str()),
        _ => Value::Nil,
    });
    host.function("zeron.block_token", |value: Value| match value {
        Value::Nil => Value::Nil,
        value => Value::from(block(&value).as_str()),
    });
}

/// Plugin `id`'s: finding what's behind a token, as far as `records` (its
/// `plugin.<id>.records`) says it may read.
pub fn serve(host: &ScriptHost, id: &str, records: &Shared) {
    let may = |records: &Shared, key: &str| records.signal().with_untracked(|r| r.field(key).is_some_and(Value::truthy));
    let r = records.clone();
    host.function(format!("plugin.{id}.file"), move |token: Value| {
        let Some(path) = path_of(&crate::auth::text(&token)).filter(|_| may(&r, "files_read")) else {
            return Value::Nil;
        };
        let name = path.rsplit('/').next().unwrap_or(&path).to_string();
        Value::record([("path", Value::from(path.as_str())), ("name", Value::from(name.as_str()))])
    });
    let r = records.clone();
    host.function(format!("plugin.{id}.block"), move |token: Value| {
        match block_of(&crate::auth::text(&token)) {
            Some(block) if may(&r, "blocks_read") => block,
            _ => Value::Nil,
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_says_nothing_of_its_path_and_finds_it_again() {
        let a = file("/Users/ada/secret/plan.md");
        assert_eq!(a, file("/Users/ada/secret/plan.md"));
        assert_ne!(a, file("/Users/ada/secret/plan2.md"));
        assert!(!a.contains("plan") && a.len() == 17, "{a}");
        assert_eq!(path_of(&a).as_deref(), Some("/Users/ada/secret/plan.md"));
        assert_eq!(path_of("/Users/ada/secret/plan.md"), None);
        let b = block(&Value::from("x^2"));
        assert_eq!(block_of(&b).map(|v| crate::auth::text(&v)).as_deref(), Some("x^2"));
    }

    #[test]
    fn the_oldest_go_once_there_are_too_many() {
        let mut kept = Bounded::new(2);
        for t in ["a", "b", "a", "c"] {
            kept.keep(t, Value::from(t), 1);
        }
        assert_eq!(kept.weight, 2);
        assert!(!kept.map.contains_key("a") && kept.map.contains_key("b") && kept.map.contains_key("c"));
    }
}
