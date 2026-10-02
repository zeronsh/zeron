//! Call-site identity.
//!
//! The journal keys every host call by `(site, ordinal)`:
//!
//! * the **site** is the call expression's source span, `l1:c1-l2:c2`
//!   (1-based, both ends), so `h.ask(x)` and the `.result()` chained onto it
//!   are different sites;
//! * the **ordinal** counts how many times that site has already run *in its
//!   scope* — the nth iteration of a loop body, the nth call of a helper;
//! * the **scope** is empty on the script's main thread and, inside a `pmap`
//!   / `parallel` worker, the call's own key plus the item index, so two
//!   items running the same code on different threads never share counters
//!   and the key does not depend on thread timing.
//!
//! The static analysis computes the same strings from the AST, so the graph
//! shown at approval names the sites the journal will contain.

use std::collections::HashMap;
use std::sync::Mutex;

use starlark::codemap::ResolvedSpan;

/// `l1:c1-l2:c2` for a resolved (0-based) span.
pub fn site_of(span: &ResolvedSpan) -> String {
    format!(
        "{}:{}-{}:{}",
        span.begin.line + 1,
        span.begin.column + 1,
        span.end.line + 1,
        span.end.column + 1
    )
}

/// The identity a host call is journaled under.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SiteKey {
    /// Scope prefix + call-site span (see the module docs).
    pub site: String,
    pub ordinal: u32,
}

impl std::fmt::Display for SiteKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}#{}", self.site, self.ordinal)
    }
}

/// Hands out ordinals for one scope.
#[derive(Debug, Default)]
pub struct Ordinals {
    scope: String,
    counts: Mutex<HashMap<String, u32>>,
}

impl Ordinals {
    pub fn root() -> Self {
        Self::default()
    }

    /// A worker's counters: `parent` is the key of the `pmap` call, `index`
    /// the item.
    pub fn child(parent: &SiteKey, index: usize) -> Self {
        Self {
            scope: format!("{parent}[{index}]/"),
            counts: Mutex::default(),
        }
    }

    /// The key of the next execution of `site` in this scope.
    pub fn next(&self, site: &str) -> SiteKey {
        let mut counts = self.counts.lock().unwrap_or_else(|e| e.into_inner());
        let slot = counts.entry(site.to_owned()).or_insert(0);
        let ordinal = *slot;
        *slot += 1;
        SiteKey {
            site: format!("{}{site}", self.scope),
            ordinal,
        }
    }

    pub fn scope(&self) -> &str {
        &self.scope
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinals_count_per_site_and_scope() {
        let root = Ordinals::root();
        assert_eq!(root.next("3:5-3:20").ordinal, 0);
        assert_eq!(root.next("3:5-3:20").ordinal, 1);
        assert_eq!(root.next("4:1-4:9").ordinal, 0);
        let parent = root.next("9:1-9:30");
        let a = Ordinals::child(&parent, 0);
        let b = Ordinals::child(&parent, 1);
        let ka = a.next("3:5-3:20");
        let kb = b.next("3:5-3:20");
        assert_ne!(ka, kb);
        assert_eq!(ka.ordinal, 0);
        assert_eq!(kb.ordinal, 0);
        assert_eq!(ka.site, "9:1-9:30#0[0]/3:5-3:20");
        // The same worker index in a re-run produces the same key.
        let again = Ordinals::child(&parent, 0).next("3:5-3:20");
        assert_eq!(again, ka);
    }
}
