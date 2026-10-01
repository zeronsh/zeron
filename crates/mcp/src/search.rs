//! Content search across rendered transcripts.
//!
//! Two modes share one shape. `words` tokenizes the query and scores every
//! message per term: an exact word beats a substring, which beats a
//! typo-tolerant match, and rare terms count for more than ones that appear in
//! most chats. `regex` is the ripgrep-style escape hatch. Both keep a
//! snippet around the strongest hit so an agent can decide whether to
//! `read_chat` the whole thing.

use regex::{Regex, RegexBuilder};

use crate::transcript::RenderedMessage;

const SNIPPET_BEFORE: usize = 100;
const SNIPPET_AFTER: usize = 160;
/// Words longer than this are never fuzzy-compared (a pasted hash or blob).
const MAX_FUZZY_WORD: usize = 40;

/// A word in a text: byte range plus its lowercase form.
struct Word {
    start: usize,
    end: usize,
    lower: String,
}

fn words(text: &str) -> Vec<Word> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, c) in text.char_indices() {
        if c.is_alphanumeric() {
            start.get_or_insert(i);
        } else if let Some(s) = start.take() {
            out.push(Word {
                start: s,
                end: i,
                lower: text[s..i].to_lowercase(),
            });
        }
    }
    if let Some(s) = start {
        out.push(Word {
            start: s,
            end: text.len(),
            lower: text[s..].to_lowercase(),
        });
    }
    out
}

/// Distinct lowercase query terms, in order.
pub fn query_terms(query: &str) -> Vec<String> {
    let mut terms: Vec<String> = Vec::new();
    for word in words(query) {
        if !terms.contains(&word.lower) {
            terms.push(word.lower);
        }
    }
    terms
}

/// How many single-character edits a term of this length may absorb.
fn edit_budget(len: usize) -> usize {
    match len {
        0..=3 => 0,
        4..=7 => 1,
        _ => 2,
    }
}

/// Levenshtein distance, giving up once it exceeds `max`.
fn edit_distance(a: &[char], b: &[char], max: usize) -> Option<usize> {
    if a.len().abs_diff(b.len()) > max {
        return None;
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur.push((prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        if cur.iter().all(|&d| d > max) {
            return None;
        }
        prev = cur;
    }
    (prev[b.len()] <= max).then_some(prev[b.len()])
}

/// Match strength of one word against one term: 0 for no match.
fn term_weight(term: &str, term_chars: &[char], word: &str) -> f32 {
    if word == term {
        return 1.0;
    }
    if term_chars.len() >= 3 && word.contains(term) {
        return 0.8;
    }
    let budget = edit_budget(term_chars.len());
    if budget == 0 || word.chars().count() > MAX_FUZZY_WORD {
        return 0.0;
    }
    let word_chars: Vec<char> = word.chars().collect();
    match edit_distance(term_chars, &word_chars, budget) {
        Some(1) => 0.6,
        Some(_) => 0.5,
        None => 0.0,
    }
}

/// What a message (or title) contributes for each query term.
struct TermHits {
    weights: Vec<f32>,
    /// Byte range of the single strongest matching word.
    anchor: Option<(usize, usize)>,
}

fn score_text(terms: &[(String, Vec<char>)], text: &str) -> TermHits {
    let mut weights = vec![0.0_f32; terms.len()];
    let mut anchor: Option<(usize, usize, f32)> = None;
    for word in words(text) {
        for (i, (term, chars)) in terms.iter().enumerate() {
            let w = term_weight(term, chars, &word.lower);
            if w > weights[i] {
                weights[i] = w;
            }
            if w > 0.0 && anchor.is_none_or(|(_, _, best)| w > best) {
                anchor = Some((word.start, word.end, w));
            }
        }
    }
    TermHits {
        weights,
        anchor: anchor.map(|(s, e, _)| (s, e)),
    }
}

/// The text of a rendered message that search looks at.
fn searchable(message: &RenderedMessage) -> String {
    let mut blob = message.text.clone();
    for extra in message
        .reasoning
        .iter()
        .chain(&message.tools)
        .chain(&message.errors)
    {
        if !blob.is_empty() {
            blob.push('\n');
        }
        blob.push_str(extra);
    }
    blob
}

/// A window of `text` around the byte range `[start, end)`, on one line.
fn snippet(text: &str, start: usize, end: usize) -> String {
    let head: Vec<(usize, char)> = text[..start].char_indices().collect();
    let from = head
        .len()
        .checked_sub(SNIPPET_BEFORE)
        .map_or(0, |i| head[i].0);
    let to = text[end..]
        .char_indices()
        .nth(SNIPPET_AFTER)
        .map_or(text.len(), |(i, _)| end + i);
    let flat = text[from..to]
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "{}{flat}{}",
        if from > 0 { "…" } else { "" },
        if to < text.len() { "…" } else { "" }
    )
}

/// One message worth showing.
pub struct MessageHit {
    /// Index into the chat's rendered messages, oldest first.
    pub index: usize,
    pub score: f32,
    /// Query terms this message matched (empty in regex mode).
    pub matched: Vec<String>,
    pub snippet: String,
}

/// Everything one chat's transcript contributes, before cross-chat weighting.
pub struct ChatScan {
    /// Best weight per term across the title and every message.
    pub coverage: Vec<f32>,
    pub title_match: bool,
    /// Messages with at least one hit, unsorted.
    hits: Vec<RawHit>,
    /// Regex mode: total matches, capped per message.
    pub regex_matches: usize,
}

struct RawHit {
    index: usize,
    weights: Vec<f32>,
    /// Regex mode: matches in this message.
    count: usize,
    blob: String,
    anchor: (usize, usize),
}

pub enum Matcher {
    Words(Vec<(String, Vec<char>)>),
    Regex(Regex),
}

impl Matcher {
    pub fn words(query: &str) -> Result<Self, String> {
        let terms = query_terms(query);
        if terms.is_empty() {
            return Err("query has no searchable words".into());
        }
        Ok(Self::Words(
            terms
                .into_iter()
                .map(|t| {
                    let chars = t.chars().collect();
                    (t, chars)
                })
                .collect(),
        ))
    }

    pub fn regex(pattern: &str, case_sensitive: bool) -> Result<Self, String> {
        RegexBuilder::new(pattern)
            .case_insensitive(!case_sensitive)
            .size_limit(1 << 20)
            .build()
            .map(Self::Regex)
            .map_err(|e| format!("invalid regex: {e}"))
    }

    pub fn term_count(&self) -> usize {
        match self {
            Self::Words(terms) => terms.len(),
            Self::Regex(_) => 1,
        }
    }

    /// Scan one chat. `messages` pairs each message to search with its index
    /// in the full transcript, so callers can filter without losing places.
    pub fn scan<'a>(
        &self,
        title: Option<&str>,
        messages: impl IntoIterator<Item = (usize, &'a RenderedMessage)>,
    ) -> ChatScan {
        let mut scan = ChatScan {
            coverage: vec![0.0; self.term_count()],
            title_match: false,
            hits: Vec::new(),
            regex_matches: 0,
        };
        match self {
            Self::Words(terms) => {
                if let Some(title) = title {
                    let hits = score_text(terms, title);
                    scan.title_match = hits.weights.iter().any(|&w| w > 0.0);
                    merge_max(&mut scan.coverage, &hits.weights);
                }
                for (index, message) in messages {
                    let blob = searchable(message);
                    let hits = score_text(terms, &blob);
                    let Some(anchor) = hits.anchor else { continue };
                    merge_max(&mut scan.coverage, &hits.weights);
                    scan.hits.push(RawHit {
                        index,
                        weights: hits.weights,
                        count: 0,
                        blob,
                        anchor,
                    });
                }
            }
            Self::Regex(re) => {
                if let Some(title) = title
                    && re.is_match(title)
                {
                    scan.title_match = true;
                    scan.coverage[0] = 1.0;
                    scan.regex_matches += 1;
                }
                for (index, message) in messages {
                    let blob = searchable(message);
                    let mut found = re.find_iter(&blob);
                    let Some(first) = found.next() else { continue };
                    let count = 1 + found.take(4).count();
                    scan.coverage[0] = 1.0;
                    scan.regex_matches += count;
                    scan.hits.push(RawHit {
                        index,
                        weights: vec![1.0],
                        count,
                        anchor: (first.start(), first.end()),
                        blob,
                    });
                }
            }
        }
        scan
    }
}

fn merge_max(into: &mut [f32], from: &[f32]) {
    for (a, b) in into.iter_mut().zip(from) {
        *a = a.max(*b);
    }
}

/// A chat that made the cut.
pub struct ChatResult {
    /// Index into the scans this came from.
    pub scan: usize,
    pub score: f32,
    pub title_match: bool,
    pub matched: Vec<String>,
    pub hits: Vec<MessageHit>,
}

/// Rank chats across a set of scans. `min_match` is the share of the query
/// (weighted by term rarity) a chat must cover to be listed.
pub fn rank(
    matcher: &Matcher,
    scans: &[ChatScan],
    min_match: f32,
    limit: usize,
    snippets: usize,
) -> (usize, Vec<ChatResult>) {
    // Rare terms matter more: a word in every chat says little.
    let idf: Vec<f32> = match matcher {
        Matcher::Words(terms) => (0..terms.len())
            .map(|t| {
                let df = scans.iter().filter(|s| s.coverage[t] > 0.0).count().max(1);
                (1.0 + scans.len() as f32 / df as f32).ln()
            })
            .collect(),
        Matcher::Regex(_) => vec![1.0],
    };
    let idf_sum: f32 = idf.iter().sum();
    let weighted = |weights: &[f32]| -> f32 {
        weights.iter().zip(&idf).map(|(w, i)| w * i).sum::<f32>() / idf_sum
    };

    let mut results: Vec<ChatResult> = Vec::new();
    for (i, scan) in scans.iter().enumerate() {
        let coverage = weighted(&scan.coverage);
        if coverage < min_match || coverage <= 0.0 {
            continue;
        }
        let mut hits: Vec<MessageHit> = scan
            .hits
            .iter()
            .map(|raw| {
                let score = match matcher {
                    Matcher::Words(_) => weighted(&raw.weights),
                    Matcher::Regex(_) => raw.count as f32,
                };
                let matched = match matcher {
                    Matcher::Words(terms) => terms
                        .iter()
                        .zip(&raw.weights)
                        .filter(|(_, w)| **w > 0.0)
                        .map(|((t, _), _)| t.clone())
                        .collect(),
                    Matcher::Regex(_) => Vec::new(),
                };
                MessageHit {
                    index: raw.index,
                    score,
                    matched,
                    snippet: snippet(&raw.blob, raw.anchor.0, raw.anchor.1),
                }
            })
            .collect();
        // Best first; among equals the newer message.
        hits.sort_by(|a, b| b.score.total_cmp(&a.score).then(b.index.cmp(&a.index)));
        let best = hits.first().map_or(0.0, |h| h.score);
        let score = match matcher {
            Matcher::Words(_) => 0.5 * coverage + 0.5 * best + f32::from(scan.title_match) * 0.25,
            Matcher::Regex(_) => scan.regex_matches as f32,
        };
        hits.truncate(snippets);
        let matched = match matcher {
            Matcher::Words(terms) => terms
                .iter()
                .zip(&scan.coverage)
                .filter(|(_, w)| **w > 0.0)
                .map(|((t, _), _)| t.clone())
                .collect(),
            Matcher::Regex(_) => Vec::new(),
        };
        results.push(ChatResult {
            scan: i,
            score,
            title_match: scan.title_match,
            matched,
            hits,
        });
    }
    let total = results.len();
    results.sort_by(|a, b| b.score.total_cmp(&a.score));
    results.truncate(limit);
    (total, results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_doc::MessageRole;

    fn msg(text: &str) -> RenderedMessage {
        RenderedMessage {
            id: "m".into(),
            role: MessageRole::Assistant,
            created_at: 0,
            created_at_iso: String::new(),
            device_id: "d".into(),
            status: None,
            text: text.into(),
            reasoning: None,
            tools: Vec::new(),
            pending_input: None,
            errors: Vec::new(),
        }
    }

    fn scan(m: &Matcher, title: &str, texts: &[&str]) -> ChatScan {
        let messages: Vec<_> = texts.iter().map(|t| msg(t)).collect();
        m.scan(Some(title), messages.iter().enumerate())
    }

    #[test]
    fn terms_split_on_punctuation_and_dedupe() {
        assert_eq!(
            query_terms("read_chat, Read-Chat!"),
            vec!["read".to_string(), "chat".to_string()]
        );
    }

    #[test]
    fn weights_rank_exact_over_substring_over_typo() {
        let chars = |s: &str| s.chars().collect::<Vec<_>>();
        let w = |term: &str, word: &str| term_weight(term, &chars(term), word);
        assert_eq!(w("proot", "proot"), 1.0);
        assert_eq!(w("proot", "prootfs"), 0.8);
        assert_eq!(w("proot", "prot"), 0.6);
        assert_eq!(w("environment", "enviromet"), 0.5);
        assert_eq!(w("cat", "car"), 0.0, "short terms are exact-only");
        assert_eq!(w("rootfs", "kernel"), 0.0);
    }

    #[test]
    fn typos_and_partial_queries_find_the_chat() {
        let m = Matcher::words("androd proot rootfs").unwrap();
        let scans = vec![
            scan(
                &m,
                "Android runtime",
                &["mounting the rootfs under proot", "done"],
            ),
            scan(&m, "Wallpapers", &["shuffle wallpapers with preloading"]),
        ];
        let (total, ranked) = rank(&m, &scans, 0.5, 10, 3);
        assert_eq!(total, 1);
        assert_eq!(ranked[0].scan, 0);
        assert!(ranked[0].matched.contains(&"androd".to_string()));
        assert_eq!(ranked[0].hits[0].index, 0);
        assert!(ranked[0].hits[0].snippet.contains("rootfs"));
    }

    #[test]
    fn min_match_filters_and_terms_can_span_messages() {
        let m = Matcher::words("reaper zebra").unwrap();
        let scans = vec![scan(&m, "t", &["the reaper kills sessions", "unrelated"])];
        // One of two terms: 0.5 coverage passes the default cut, not a strict one.
        assert_eq!(rank(&m, &scans, 0.5, 10, 3).0, 1);
        assert_eq!(rank(&m, &scans, 0.9, 10, 3).0, 0);
    }

    #[test]
    fn rare_terms_outweigh_common_ones() {
        let m = Matcher::words("session unicorn").unwrap();
        let scans = vec![
            scan(&m, "a", &["session session"]),
            scan(&m, "b", &["session"]),
            scan(&m, "c", &["session unicorn"]),
        ];
        let (_, ranked) = rank(&m, &scans, 0.0, 10, 1);
        assert_eq!(ranked[0].scan, 2);
    }

    #[test]
    fn title_only_matches_count() {
        let m = Matcher::words("wallpaper").unwrap();
        let scans = vec![scan(&m, "Wallpaper shuffle", &["nothing here"])];
        let (_, ranked) = rank(&m, &scans, 0.5, 10, 3);
        assert!(ranked[0].title_match);
        assert!(ranked[0].hits.is_empty());
    }

    #[test]
    fn regex_mode_counts_matches_and_honors_case() {
        let m = Matcher::regex(r"HELD_\w+", true).unwrap();
        let scans = vec![
            scan(
                &m,
                "a",
                &["const HELD_DONE_SETTLE = 5s", "and HELD_X again"],
            ),
            scan(&m, "b", &["held_done_settle lowercase"]),
        ];
        let (total, ranked) = rank(&m, &scans, 0.0, 10, 3);
        assert_eq!(total, 1);
        assert_eq!(ranked[0].scan, 0);
        assert_eq!(ranked[0].hits.len(), 2);
        assert!(Matcher::regex("(", false).is_err());
    }

    #[test]
    fn snippets_stay_on_char_boundaries_and_one_line() {
        let text = format!("{}needle\n\n{}", "é".repeat(300), "ü".repeat(300));
        let start = text.find("needle").unwrap();
        let s = snippet(&text, start, start + 6);
        assert!(s.starts_with('…') && s.ends_with('…'));
        assert!(s.contains("needle") && !s.contains('\n'));
        assert_eq!(snippet("short needle", 6, 12), "short needle");
    }
}
