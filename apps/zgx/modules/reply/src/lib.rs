//! A long settled reply's blocks and tools, kept out of its row: the app's
//! own code for it (src/chat.rs `set_aside`, `reply_run`, `reply_blocks`,
//! `reply_run_of`), moved here line for line, as a native module.
//!
//! The app calls `chat.reply_set_aside` as it builds a row and
//! `chat.reply_release` as it lets one go; the transcript's scripts call
//! the other three, as they did.

use std::{cell::RefCell, rc::Rc};

use rustc_hash::FxHashMap;
use zgx_module::{Host, Ref, Value};

struct Aside {
    /// Each run: its key (its first block's id) and its blocks.
    runs: Vec<(Value, Value)>,
    blocks: Value,
}

/// More blocks than this, a settled reply's are set aside (ds/transcript's
/// LONG_REPLY, past which it windows them).
const LONG_REPLY: usize = 48;
/// ds/transcript's runs: a block whose id falls on RUN starts one, and none
/// runs past RUN_MOST.
const RUN: u64 = 32;
const RUN_MOST: usize = 64;

thread_local! {
    /// The long replies set aside, by their entry's id: the latest built of
    /// each, while the app holds it.
    static ASIDE: RefCell<FxHashMap<Box<str>, std::rc::Weak<Aside>>> = RefCell::default();
    /// What the app holds, by the number it was given for it.
    static HELD: RefCell<FxHashMap<u64, Rc<Aside>>> = RefCell::default();
    static NEXT: std::cell::Cell<u64> = const { std::cell::Cell::new(1) };
}

/// Whether block `id` starts a run (ds/transcript's `starts_run`: the same
/// sum, so both cut a reply into the same runs).
fn starts_run(id: &str) -> bool {
    id.bytes().fold(0u64, |h, b| (h * 31 + u64::from(b)) % 1_000_003) % RUN == 0
}

/// What `set_aside` reads of a block: asked of the runtime in one call
/// (`Block::of`), then read here.
struct Block<'a> {
    id: Option<Ref<'a>>,
    kind: Option<Ref<'a>>,
    summary: Option<Ref<'a>>,
    live: Option<Ref<'a>>,
    text: Option<Ref<'a>>,
    tools: Option<Ref<'a>>,
}
impl<'a> Block<'a> {
    fn of(block: Ref<'a>) -> Self {
        let [id, kind, summary, live, text, tools] =
            block.get(["id", "kind", "summary", "live", "text", "tools"]);
        Block { id, kind, summary, live, text, tools }
    }
    fn tools(&self) -> usize {
        self.tools.map_or(0, |t| if t.is_list() { t.len() } else { 0 })
    }
}

/// About how tall a block will be before it's laid out (ds/transcript's
/// `guess_block`, the same arithmetic).
fn guess_block(block: &Block) -> f64 {
    match block.kind.and_then(Ref::str) {
        Some("text") => {
            let (mut h, mut chars, mut code, mut fenced) = (0f64, 0usize, 0usize, false);
            let paragraph = |h: &mut f64, chars: &mut usize| {
                if *chars > 0 {
                    *h += (*chars as f64 / 85.).ceil() * 24. + 16.;
                    *chars = 0;
                }
            };
            let text = block.text.and_then(Ref::str).unwrap_or("");
            for line in text.split('\n') {
                if line.trim_start().starts_with("```") {
                    if fenced {
                        h += code as f64 * 19. + 56. + 16.;
                        code = 0;
                    } else {
                        paragraph(&mut h, &mut chars);
                    }
                    fenced = !fenced;
                } else if fenced {
                    code += 1;
                } else if line.trim().is_empty() {
                    paragraph(&mut h, &mut chars);
                } else {
                    chars += line.len() + 1;
                }
            }
            paragraph(&mut h, &mut chars);
            (h - 16.).max(24.)
        }
        Some("tools" | "work") => {
            let live = block.live.and_then(Ref::bool) == Some(true);
            if live { 26. * block.tools().max(1) as f64 } else { 26. }
        }
        _ => 240.,
    }
}

/// What a block holds, summed: its id, its kind, its words' length, its
/// tools' count and line.
fn block_sum(block: &Block) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = rustc_hash::FxHasher::default();
    for value in [block.id, block.kind, block.summary, block.live] {
        match (value.and_then(Ref::str), value.and_then(Ref::bool)) {
            (Some(s), _) => s.hash(&mut h),
            (_, Some(b)) => b.hash(&mut h),
            _ => 0u8.hash(&mut h),
        }
    }
    match block.text.and_then(Ref::str) {
        Some(s) => s.len().hash(&mut h),
        None => 0usize.hash(&mut h),
    }
    block.tools().hash(&mut h);
    h.finish()
}

fn text_of(value: &Value) -> &str {
    value.get().str().unwrap_or("")
}

/// What `set_aside` reads of a block, read once: one pass over the reply's
/// blocks, each block's fields asked of the runtime together.
struct Read<'a> {
    id: Option<Ref<'a>>,
    starts: bool,
    guess: f64,
    sum: u64,
}

/// `row` (an entry's, built whole) with its blocks and tools set aside, if
/// it's a settled reply of more than LONG_REPLY blocks: `{ row, hold }`,
/// `hold` what to release it by. Else nil, and the row is as it was.
fn set_aside(row: Ref) -> Value {
    let fields = row.record();
    let settled = fields.get("role").and_then(Ref::str) == Some("assistant")
        && fields.get("streaming").and_then(Ref::bool) == Some(false);
    let (Some(id), Some(blocks)) = (fields.get("id").and_then(Ref::str), fields.get("blocks")) else {
        return Value::NIL;
    };
    if !blocks.is_list() || !settled || blocks.len() <= LONG_REPLY {
        return Value::NIL;
    }
    let read: Vec<Read> = blocks
        .items()
        .map(|block| {
            let block = Block::of(block);
            let id = block.id.filter(|id| id.str().is_some());
            Read {
                id,
                starts: starts_run(id.and_then(Ref::str).unwrap_or("")),
                guess: guess_block(&block),
                sum: block_sum(&block),
            }
        })
        .collect();
    // Cut as ds/transcript cuts them: a run ends before a block that starts
    // one, or once it holds RUN_MOST.
    let mut cuts: Vec<(usize, usize)> = Vec::new();
    let mut from = 0;
    for (i, block) in read.iter().enumerate().skip(1) {
        if i - from >= RUN_MOST || block.starts {
            cuts.push((from, i));
            from = i;
        }
    }
    cuts.push((from, read.len()));
    let mut runs = Vec::with_capacity(cuts.len());
    let mut listed = Vec::with_capacity(cuts.len());
    for (from, to) in cuts {
        let key = read[from].id.map_or_else(|| Value::from(""), Ref::to_owned);
        let guess = read[from..to].iter().map(|b| b.guess + 12.).sum::<f64>() - 12.;
        let sum = read[from..to].iter().fold(0u64, |h, b| h.wrapping_mul(31).wrapping_add(b.sum));
        listed.push(Value::record([
            ("key", key.clone()),
            ("n", Value::from((to - from) as f64)),
            ("guess", Value::from(guess.max(0.))),
            ("sum", Value::from(format!("{sum:x}").as_str())),
        ]));
        runs.push((key, blocks.slice(from, to)));
    }
    let aside = Rc::new(Aside { runs, blocks: blocks.to_owned() });
    let mut kept: Vec<(&str, Value)> =
        fields.iter().filter(|(k, _)| !matches!(*k, "blocks" | "tools")).map(|(k, v)| (k, v.to_owned())).collect();
    kept.push(("runs", Value::list(listed)));
    ASIDE.with_borrow_mut(|held| {
        if held.len() >= 64 {
            held.retain(|_, a| a.strong_count() > 0);
        }
        held.insert(id.into(), Rc::downgrade(&aside));
    });
    let hold = NEXT.replace(NEXT.get() + 1);
    HELD.with_borrow_mut(|held| held.insert(hold, aside));
    Value::record([("row", Value::record(kept)), ("hold", Value::from(hold as f64))])
}

/// The reply set aside as `id`, while something holds it.
fn aside_of(id: &str) -> Option<Rc<Aside>> {
    ASIDE.with_borrow(|held| held.get(id).and_then(std::rc::Weak::upgrade))
}

fn text<'a>(value: Option<Ref<'a>>) -> &'a str {
    value.and_then(Ref::str).unwrap_or("")
}

/// A long reply's run (`{ id, key }`), as `chat.reply_run` hands it to the
/// transcript building it: its blocks; nil once the reply is let go of.
fn reply_run(arg: Ref) -> Value {
    let [id, key] = arg.get(["id", "key"]);
    let key = text(key);
    aside_of(text(id))
        .and_then(|aside| aside.runs.iter().find(|(k, _)| text_of(k) == key).map(|(_, blocks)| blocks.clone()))
        .unwrap_or(Value::NIL)
}

/// A long reply's blocks, all of them (`chat.reply_blocks`, given its id):
/// what a find in the chat reads. Nil once the reply is let go of.
fn reply_blocks(id: Ref) -> Value {
    let Some(id) = id.str() else { return Value::NIL };
    aside_of(id).map_or(Value::NIL, |aside| aside.blocks.clone())
}

/// The key of the run of a long reply (`{ id, block }`) that holds that
/// block (`chat.reply_run_of`), nil if none does.
fn reply_run_of(arg: Ref) -> Value {
    let [id, block] = arg.get(["id", "block"]);
    let block = text(block);
    aside_of(text(id))
        .and_then(|aside| {
            let holds = |b: Ref| b.field("id").and_then(Ref::str) == Some(block);
            aside.runs.iter().find(|(_, blocks)| blocks.get().items().any(holds)).map(|(k, _)| k.clone())
        })
        .unwrap_or(Value::NIL)
}

zgx_module::module!(|host: &Host| {
    host.function("chat.reply_run", reply_run);
    host.function("chat.reply_blocks", reply_blocks);
    host.function("chat.reply_run_of", reply_run_of);
    host.function("chat.reply_set_aside", set_aside);
    host.action("chat.reply_release", |hold| {
        if let Some(hold) = hold.num() {
            HELD.with_borrow_mut(|held| held.remove(&(hold as u64)));
        }
    });
});
