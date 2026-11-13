//! Whether a fragment of a brain dump is a question the person wants
//! answered, read from the person's own words. The Brain dump's router tags
//! fragments with intent `question`; this is the deterministic check over
//! that tag. It only ever takes the tag away:
//!
//! - work asked in a question's shape ("fix X, and can you also check Y?",
//!   "why does X happen? find the root cause and fix it", "could you add
//!   Z?") is a task;
//! - a rhetorical question ("so much better, right?", "what could go
//!   wrong?") is context, nobody's to answer.
//!
//! A fragment it can't place either way keeps what the router said.

use crate::orchestration::RouteIntent;

/// What a fragment of the person's words asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Asked {
    /// A question the person wants answered.
    Direct,
    /// Work, asked for (in a question's shape or not).
    Work,
    /// Only rhetorical questions.
    Rhetorical,
    /// Neither a question nor work it can see.
    Unclear,
}

/// Words that ask for work when a clause starts with them, even with a
/// question mark after ("Look into why zgx eats 2GB?").
const WORK: &[&str] = &[
    "fix", "implement", "add", "remove", "delete", "drop", "make", "create", "write", "rewrite", "refactor",
    "rename", "move", "investigate", "debug", "find", "figure out", "track down", "dig into", "look into",
    "look at", "reproduce", "repro", "wire", "hook up", "set up", "clean up", "migrate", "rebuild",
    "restart", "install", "uninstall", "revert", "replace", "split", "port", "bump", "upgrade", "downgrade",
    "optimize", "speed up", "stop", "kill", "cancel", "send", "open a pr", "file", "patch", "redesign",
    "rework", "hide", "show", "ensure", "go ahead", "start", "kick off", "try",
];

/// Words that ask for work only as a request ("check the logs.", "can you
/// test it?"), not in a bare question ("test results from last night?").
const WORK_SOFT: &[&str] = &[
    "check", "test", "run", "build", "review", "update", "change", "merge", "deploy", "ship", "push",
    "commit", "release", "verify", "audit", "profile", "plan", "use", "keep", "put", "let",
];

/// What a request opens with ("could you", "please").
const ASKING: &[&str] = &[
    "can you", "could you", "would you", "will you", "can we", "could we", "please", "pls", "kindly",
    "i need you to", "i want you to", "go and", "also please",
];

/// What a clause may open with before what it says ("also", "oh and").
const FILLER: &[&str] = &[
    "also", "and", "then", "so", "oh", "ok", "okay", "btw", "by the way", "plus", "and then", "but",
    "quick question:", "question:", "q:", "one more thing:", "one more thing", "anyway", "now",
];

/// Openers of a real question.
const ASKS: &[&str] = &[
    "what", "what's", "whats", "why", "how", "how's", "hows", "when", "where", "which", "who", "whom", "whose",
    "is", "are", "was", "were", "do", "does", "did", "can", "could", "should", "would", "will", "have", "has",
    "had", "isn't", "aren't", "wasn't", "don't", "doesn't", "didn't", "won't", "shouldn't", "any idea",
    "i wonder", "wondering", "tell me", "explain", "remind me what", "remind me how", "remind me why",
    "any news", "status of", "what about", "i want to know", "i'd like to know", "i'm curious", "curious",
];

/// Questions nobody is asked to answer (in a sentence, lowercase).
const RHETORICAL: &[&str] = &[
    ", right?", "who cares", "who knows", "why not?", "what could go wrong", "how hard can it be",
    "who even", "you know?", "am i right", "go figure", "guess what", "what else is new", "why bother",
    ", isn't it?", "don't you think", "isn't that nice", ", no?", ", eh?", ", huh?", "what gives",
    "how cool is that", "how about that", "who would have thought", "who would've thought",
];

/// Sentences, each with whether it ends in '?': split at sentence ends,
/// semicolons and line breaks.
fn sentences(text: &str) -> Vec<(String, bool)> {
    let lower = text.to_lowercase().replace('\u{2019}', "'");
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = lower.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '?' | '!' | ';' | '\n' => {
                if c == '?' || c == '!' {
                    cur.push(c);
                }
                push(&mut out, &cur, c == '?');
                cur.clear();
            }
            '.' if chars.peek().is_none_or(|n| n.is_whitespace()) => {
                push(&mut out, &cur, false);
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    push(&mut out, &cur, false);
    out
}

/// A sentence's pieces where a second request may start: at commas, at
/// "and then", and at "and" before a request ("and can you"); only the
/// last piece keeps the sentence's question mark.
fn pieces(sentence: &str, question: bool) -> Vec<(String, bool)> {
    let mut seps: Vec<String> = vec![",".into(), " and then ".into()];
    for a in ASKING {
        seps.push(format!(" and {a} "));
    }
    let mut out = Vec::new();
    let mut rest = sentence;
    loop {
        let at = seps
            .iter()
            .filter_map(|sep| rest.find(sep.as_str()).map(|i| (i, if sep.starts_with(" and ") && sep != " and then " { 5 } else { sep.len() })))
            .min_by_key(|(i, _)| *i);
        match at {
            Some((i, len)) => {
                out.push((rest[..i].trim().to_owned(), false));
                rest = &rest[i + len..];
            }
            None => {
                out.push((rest.trim().to_owned(), question));
                break;
            }
        }
    }
    out.retain(|(c, _)| !c.trim_matches(|c: char| !c.is_alphanumeric()).is_empty());
    out
}

fn push(out: &mut Vec<(String, bool)>, clause: &str, question: bool) {
    let clause = clause.trim();
    if !clause.is_empty() {
        out.push((clause.to_owned(), question));
    }
}

/// `text` starts with `word` as a whole word (or phrase).
fn opens(text: &str, word: &str) -> bool {
    text.strip_prefix(word)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(|c: char| !c.is_alphanumeric() && c != '\''))
}

/// `clause` past its fillers ("also", "oh and").
fn past_fillers(mut clause: &str) -> &str {
    loop {
        let trimmed = clause.trim_start_matches(|c: char| c.is_whitespace() || c == ',' || c == '-' || c == '—');
        match FILLER.iter().find(|f| opens(trimmed, f)) {
            Some(f) => clause = &trimmed[f.len()..],
            None => return trimmed,
        }
    }
}

/// Whether a piece of a sentence asks for work.
fn is_work(clause: &str, question: bool) -> bool {
    let mut c = past_fillers(clause);
    let mut asking = false;
    loop {
        match ASKING.iter().find(|a| opens(c, a)) {
            Some(a) => {
                asking = true;
                c = past_fillers(&c[a.len()..]);
                c = c.strip_prefix("also ").unwrap_or(c);
            }
            None => break,
        }
    }
    if opens(c, "let's") || opens(c, "lets") {
        return true;
    }
    if WORK.iter().any(|w| opens(c, w)) {
        return true;
    }
    WORK_SOFT.iter().any(|w| opens(c, w)) && (asking || !question)
}

fn is_rhetorical(clause: &str) -> bool {
    RHETORICAL.iter().any(|r| clause.contains(r))
}

fn asks(clause: &str, question: bool) -> bool {
    question || ASKS.iter().any(|a| opens(past_fillers(clause), a))
}

/// What `excerpt` (the person's words) asks for.
pub fn classify(excerpt: &str) -> Asked {
    let parts = sentences(excerpt);
    if parts.iter().any(|(s, q)| pieces(s, *q).iter().any(|(p, pq)| is_work(p, *pq))) {
        return Asked::Work;
    }
    let questions: Vec<_> = parts.iter().filter(|(s, q)| asks(s, *q)).collect();
    if questions.iter().any(|(s, _)| !is_rhetorical(s)) {
        return Asked::Direct;
    }
    if !questions.is_empty() {
        return Asked::Rhetorical;
    }
    Asked::Unclear
}

/// Whether `excerpt` is a question the person wants answered.
pub fn is_direct(excerpt: &str) -> bool {
    classify(excerpt) == Asked::Direct
}

/// The router's `intent` for a fragment, checked against the person's words
/// (`excerpt`): a "question" that asks for work is a task, one that is only
/// rhetorical is info. Anything else stands.
pub fn settle_intent(intent: RouteIntent, excerpt: &str) -> RouteIntent {
    if intent != RouteIntent::Question {
        return intent;
    }
    match classify(excerpt) {
        Asked::Work => RouteIntent::Task,
        Asked::Rhetorical => RouteIntent::Info,
        Asked::Direct | Asked::Unclear => RouteIntent::Question,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Questions the person wants answered.
    const DIRECT: &[&str] = &[
        // The real one: asked in the Control room, answered in a thread, never shown.
        "Are Inbox and Next sections in the board ever used? Under what conditions does that happen? Because I've never seen a card in Inbox or Next.",
        "how's the auth refactor going?",
        "When does 2.0 ship?",
        "Should we drop the Inbox column, or is it worth keeping?",
        // A work word inside the question isn't work asked for.
        "Why is the fix for the sidebar flicker still not merged?",
        "I asked it to fix the flicker yesterday, did that ever land?",
        "Do we still need the old router? I'm thinking about removing it.",
        "Can you explain how the Brain dump picks a project?",
        "Tell me which devices are online right now.",
        "Quick question: is the Moscow node still on the old kernel?",
        "btw what model does the Brain dump use",
        "Test results from last night?",
        "Is it true the map is drawn every frame, right? I want to know how often it redraws.",
    ];

    /// Work in a question's shape, rhetorical questions, and plain work.
    const NOT_DIRECT: &[(&str, Asked)] = &[
        ("fix the sidebar flicker, and can you also check whether CI is green?", Asked::Work),
        ("why does the build fail on arm64? find the root cause and fix it", Asked::Work),
        ("Could you add a Delete action to board cards?", Asked::Work),
        ("Look into why zgx eats 2GB of memory?", Asked::Work),
        ("Are Inbox and Next ever used? If not, remove them.", Asked::Work),
        ("Ship it already, what could go wrong?", Asked::Work),
        ("Please rebuild the app and tell me when it's done", Asked::Work),
        ("Remember the cache bug? It's back, please fix it.", Asked::Work),
        ("Can you check if the deploy went through?", Asked::Work),
        ("Why not, let's just keep the old layout.", Asked::Work),
        ("The new map is so much better, right?", Asked::Rhetorical),
        ("Who even reads these logs anyway?", Asked::Rhetorical),
        ("What could go wrong?", Asked::Rhetorical),
        ("Make the Progress clocks tick by the minute.", Asked::Work),
        ("The VPN dropped twice overnight.", Asked::Unclear),
    ];

    #[test]
    fn direct_questions_are_marked() {
        for q in DIRECT {
            assert_eq!(classify(q), Asked::Direct, "{q}");
            assert!(is_direct(q), "{q}");
        }
    }

    #[test]
    fn work_and_rhetorical_questions_are_not() {
        for (q, want) in NOT_DIRECT {
            assert_eq!(classify(q), *want, "{q}");
            assert!(!is_direct(q), "{q}");
        }
    }

    #[test]
    fn the_router_tag_is_only_taken_away() {
        use RouteIntent::*;
        assert_eq!(settle_intent(Question, DIRECT[0]), Question);
        assert_eq!(settle_intent(Question, "why does X happen? find the root cause and fix it"), Task);
        assert_eq!(settle_intent(Question, "The new map is so much better, right?"), Info);
        // Can't tell: the router's word stands.
        assert_eq!(settle_intent(Question, "the release date for 2.0"), Question);
        // Never made a question.
        assert_eq!(settle_intent(Task, "When does 2.0 ship?"), Task);
        assert_eq!(settle_intent(Info, "When does 2.0 ship?"), Info);
    }
}
