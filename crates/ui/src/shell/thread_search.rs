//! Scored thread search for the command palette, modeled on the mobile
//! workspace view's `score_row` (`zeron_client`): every query word must match
//! some field; a match scores its field's weight, plus half again when it
//! starts a word. Archived threads score half, and ties go to the most
//! recently active thread.

/// Field weights: a title hit outranks any number of metadata hits.
const TITLE: u32 = 100;
const PROJECT: u32 = 40;
const BRANCH: u32 = 30;
const DEVICE: u32 = 20;
const PULL_REQUEST: u32 = 20;

/// The searchable text of one thread.
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct ThreadFields<'a> {
    pub title: &'a str,
    pub project: &'a str,
    pub branch: &'a str,
    pub device: &'a str,
    /// Number, title and refs of the thread's pull request.
    pub pull_request: &'a str,
    pub archived: bool,
}

/// Lowercased whitespace-separated query words.
pub(super) fn search_terms(query: &str) -> Vec<String> {
    query.split_whitespace().map(str::to_lowercase).collect()
}

/// `None` when some term matches no field (or there are no terms).
pub(super) fn score_thread(fields: &ThreadFields, terms: &[String]) -> Option<u32> {
    if terms.is_empty() {
        return None;
    }
    let haystacks = [
        (fields.title.to_lowercase(), TITLE),
        (fields.project.to_lowercase(), PROJECT),
        (fields.branch.to_lowercase(), BRANCH),
        (fields.device.to_lowercase(), DEVICE),
        (fields.pull_request.to_lowercase(), PULL_REQUEST),
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
    best
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
}
