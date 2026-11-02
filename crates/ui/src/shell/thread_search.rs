//! Shared scored search for chat history and the sidebar. Every query word
//! can match a different field, in any order. Exact title hits rank highest;
//! long words tolerate one typo (including a transposition). Archived chats
//! remain searchable, and ties go to the most recently active chat.

/// Field weights: a title hit outranks any number of metadata hits.
const TITLE: u32 = 100;
const PROJECT: u32 = 40;
const BRANCH: u32 = 30;
const DEVICE: u32 = 20;
const PULL_REQUEST: u32 = 20;
const PREVIEW: u32 = 15;
const ID: u32 = 10;

/// The searchable text of one thread.
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct ThreadFields<'a> {
    pub title: &'a str,
    pub project: &'a str,
    pub project_path: &'a str,
    pub preview: &'a str,
    pub id: &'a str,
    pub branch: &'a str,
    pub device: &'a str,
    /// Number, title and refs of the thread's pull request.
    pub pull_request: &'a str,
    pub archived: bool,
}

/// Case-insensitive words, ignoring punctuation and Russian е/ё differences.
pub(super) fn search_terms(query: &str) -> Vec<String> {
    let mut terms = Vec::new();
    for term in query.split(|c: char| !c.is_alphanumeric()) {
        let term = normalize(term);
        if !term.is_empty() && !terms.contains(&term) {
            terms.push(term);
        }
    }
    terms
}

fn normalize(text: &str) -> String {
    text.to_lowercase().replace('ё', "е")
}

/// `None` when some term matches no field (or there are no terms).
pub(super) fn score_thread(fields: &ThreadFields, terms: &[String]) -> Option<u32> {
    if terms.is_empty() {
        return None;
    }
    let haystacks = [
        (normalize(fields.title), TITLE),
        (normalize(fields.project), PROJECT),
        (normalize(fields.project_path), PROJECT),
        (normalize(fields.branch), BRANCH),
        (normalize(fields.device), DEVICE),
        (normalize(fields.pull_request), PULL_REQUEST),
        (normalize(fields.preview), PREVIEW),
        (normalize(fields.id), ID),
    ];
    let mut total = 0;
    for term in terms {
        total += haystacks
            .iter()
            .filter_map(|(haystack, weight)| field_score(haystack, term, *weight))
            .max()?;
    }
    Some(if fields.archived { total / 2 } else { total })
}

fn field_score(haystack: &str, term: &str, weight: u32) -> Option<u32> {
    let mut best = None;
    for (at, _) in haystack.match_indices(term) {
        let starts_word = haystack[..at]
            .chars()
            .next_back()
            .is_none_or(|previous| !previous.is_alphanumeric());
        if starts_word {
            return Some(weight + weight / 2);
        }
        best = Some(weight);
    }
    if best.is_some() {
        return best;
    }
    let needle: Vec<_> = term.chars().collect();
    // Short fuzzy queries produce too many unrelated hits. IDs stay literal.
    if needle.len() < 4 || weight == ID {
        return None;
    }
    for word in haystack.split(|c: char| !c.is_alphanumeric()) {
        let word: Vec<_> = word.chars().take(needle.len() + 1).collect();
        for length in needle.len() - 1..=needle.len() + 1 {
            if length <= word.len() && one_typo(&needle, &word[..length]) {
                return Some(weight / 2);
            }
        }
    }
    None
}

/// One Unicode-character insertion, deletion, substitution or adjacent swap.
fn one_typo(a: &[char], b: &[char]) -> bool {
    if a.len().abs_diff(b.len()) > 1 {
        return false;
    }
    let first = a.iter().zip(b).position(|(a, b)| a != b);
    let Some(i) = first else {
        return true;
    };
    match a.len().cmp(&b.len()) {
        std::cmp::Ordering::Greater => a[i + 1..] == b[i..],
        std::cmp::Ordering::Less => a[i..] == b[i + 1..],
        std::cmp::Ordering::Equal => {
            a[i + 1..] == b[i + 1..]
                || (i + 1 < a.len()
                    && a[i] == b[i + 1]
                    && a[i + 1] == b[i]
                    && a[i + 2..] == b[i + 2..])
        }
    }
}

/// Best first: higher score, then more recent activity, then the caller's
/// order. Keeps at most `limit`.
pub(super) fn rank<T>(mut scored: Vec<(T, u32, i64)>, limit: usize) -> Vec<T> {
    scored.sort_by(|(_, score_a, active_a), (_, score_b, active_b)| {
        score_b.cmp(score_a).then(active_b.cmp(active_a))
    });
    scored.truncate(limit);
    scored.into_iter().map(|(item, _, _)| item).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn title(title: &str) -> ThreadFields<'_> {
        ThreadFields {
            title,
            ..Default::default()
        }
    }

    #[test]
    fn every_word_must_match_some_field() {
        let fields = ThreadFields {
            title: "Fix authentication",
            device: "MacBook",
            ..Default::default()
        };
        assert!(score_thread(&fields, &search_terms("mac auth")).is_some());
        assert!(score_thread(&fields, &search_terms("mac windows")).is_none());
        assert!(score_thread(&fields, &search_terms("   ")).is_none());
    }

    #[test]
    fn fields_are_weighted_title_first() {
        let terms = search_terms("search");
        let in_title = score_thread(&title("search"), &terms).unwrap();
        let in_project = score_thread(
            &ThreadFields {
                project: "search",
                ..Default::default()
            },
            &terms,
        )
        .unwrap();
        let in_branch = score_thread(
            &ThreadFields {
                branch: "search",
                ..Default::default()
            },
            &terms,
        )
        .unwrap();
        let in_pr = score_thread(
            &ThreadFields {
                pull_request: "#12 search",
                ..Default::default()
            },
            &terms,
        )
        .unwrap();
        assert!(in_title > in_project && in_project > in_branch && in_branch > in_pr);
    }

    #[test]
    fn word_starts_earn_a_bonus() {
        let terms = search_terms("cache");
        let start = score_thread(&title("Fix cache eviction"), &terms).unwrap();
        let inside = score_thread(&title("Fix precached rows"), &terms).unwrap();
        assert_eq!(start, 150);
        assert_eq!(inside, 100);
        // A later word start still wins over an earlier mid-word hit.
        assert_eq!(score_thread(&title("precache cache"), &terms).unwrap(), 150);
    }

    #[test]
    fn archived_threads_score_half() {
        let terms = search_terms("deploy");
        let active = score_thread(&title("deploy"), &terms).unwrap();
        let archived = score_thread(
            &ThreadFields {
                archived: true,
                ..title("deploy")
            },
            &terms,
        )
        .unwrap();
        assert_eq!(archived, active / 2);
    }

    #[test]
    fn ties_break_by_recent_activity() {
        let ranked = rank(
            vec![("old", 150, 10), ("new", 150, 20), ("best", 200, 0)],
            2,
        );
        assert_eq!(ranked, vec!["best", "new"]);
    }
    #[test]
    fn words_match_in_any_order_with_punctuation_and_russian_yo() {
        let fields = title("Настройка ёмкого кэша: поиск чатов");
        assert!(score_thread(&fields, &search_terms("ЧАТОВ, емкого поиск")).is_some());
        assert_eq!(search_terms("cache cache #42"), ["cache", "42"]);
    }

    #[test]
    fn preview_paths_and_ids_are_searchable() {
        let fields = ThreadFields {
            title: "New session",
            preview: "Investigate sidebar rendering",
            project_path: "/Users/me/zeron",
            id: "b6a06e76-3729",
            ..Default::default()
        };
        for query in ["sidebar zeron", "b6a06e76", "3729"] {
            assert!(score_thread(&fields, &search_terms(query)).is_some(), "{query}");
        }
        assert!(score_thread(&fields, &search_terms("unrelated")).is_none());
    }

    #[test]
    fn one_typo_is_tolerated_but_exact_titles_win() {
        for (query, text) in [
            ("serach", "Search chats"),
            ("searh", "Search chats"),
            ("searchh", "Search chats"),
            ("searxh", "Search chats"),
            ("поискк", "Поиск чатов"),
            ("чатво", "Поиск чатов"),
            ("authn", "Authentication"),
        ] {
            let terms = search_terms(query);
            let fuzzy = score_thread(&title(text), &terms).unwrap();
            let exact = score_thread(&title(query), &terms).unwrap();
            assert!(exact > fuzzy, "{query}: {text}");
        }
        assert!(score_thread(&title("Cache"), &search_terms("cxe")).is_none());
        assert!(score_thread(&title("Search"), &search_terms("sxxrch")).is_none());
    }

    #[test]
    fn typo_comparison_covers_unicode_and_boundaries() {
        for (a, b, expected) in [
            ("", "", true), ("a", "", true), ("", "ab", false),
            ("ab", "ba", true), ("abcd", "abdc", true),
            ("abcd", "axyd", false), ("поиск", "поис", true),
        ] {
            assert_eq!(one_typo(&a.chars().collect::<Vec<_>>(), &b.chars().collect::<Vec<_>>()), expected);
        }
    }

}
