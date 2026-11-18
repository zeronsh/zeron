//! Strict SemVer 2.0.0 and the version ranges a plugin's `engines.zeron`
//! names: what the store requires of a version, and how a device decides a
//! plugin is for its Zeron.
//!
//! Implemented twice (here and at the edge, in TypeScript) and held together
//! by `docs/plugin-package-vectors.json` (`vectors.rs`), so the rules are
//! spelled out exactly:
//!
//! - A version is `MAJOR.MINOR.PATCH`, then optionally `-PRERELEASE` and
//!   `+BUILD`, as semver.org 2.0.0 says: numbers without leading zeros,
//!   identifiers of `[0-9A-Za-z-]`, none empty, a numeric pre-release
//!   identifier without leading zeros. No `v` prefix, no spaces. Every
//!   number is at most 2^53 − 1 (so JavaScript compares it exactly), and
//!   the whole is at most [`MAX_LEN`] bytes.
//! - Precedence is semver.org's: major, minor, patch; a pre-release before
//!   its release; pre-release identifiers left to right, numbers before
//!   words, numbers by value, words by ASCII; a shorter list first when all
//!   else is equal. Build metadata is ignored: `1.0.0+a` and `1.0.0+b` are
//!   the same version.
//! - A range is sets joined by `||`, any of which may match; a set is
//!   comparators separated by spaces, all of which must. A comparator is
//!   `>=`, `>`, `<=`, `<`, `=`, `^` or `~` and a full version (spaces
//!   between them allowed), or a bare version (exactly it), or `*`
//!   (anything). `^` and `~` read as npm's: `^1.2.3` is `>=1.2.3
//!   <2.0.0-0`, `^0.2.3` is `>=0.2.3 <0.3.0-0`, `^0.0.3` is `>=0.0.3
//!   <0.0.4-0`, `~1.2.3` is `>=1.2.3 <1.3.0-0`. Pre-releases have no special
//!   treatment: a version matches by precedence alone. No partial versions
//!   (`1.2`, `1.x`), no hyphen ranges.
//!
//! Sideloading keeps `install::newer`'s lenient compare: only the store and
//! `engines.zeron` hold to these.

use std::cmp::Ordering;
use std::fmt;

/// The longest version string accepted.
pub const MAX_LEN: usize = 128;
/// The longest range string accepted.
pub const MAX_RANGE_LEN: usize = 256;
/// The largest number in a version: JavaScript's largest exact integer.
pub const MAX_NUMBER: u64 = (1 << 53) - 1;

/// A pre-release identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ident {
    Number(u64),
    Word(String),
}

impl Ord for Ident {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Ident::Number(a), Ident::Number(b)) => a.cmp(b),
            (Ident::Number(_), Ident::Word(_)) => Ordering::Less,
            (Ident::Word(_), Ident::Number(_)) => Ordering::Greater,
            (Ident::Word(a), Ident::Word(b)) => a.as_bytes().cmp(b.as_bytes()),
        }
    }
}

impl PartialOrd for Ident {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Ident {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ident::Number(n) => write!(f, "{n}"),
            Ident::Word(w) => f.write_str(w),
        }
    }
}

/// A strict SemVer 2.0.0 version. Equality and order are precedence: build
/// metadata is kept (and shown) but never compared.
#[derive(Debug, Clone)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub pre: Vec<Ident>,
    pub build: Vec<String>,
}

impl Version {
    pub fn new(major: u64, minor: u64, patch: u64) -> Self {
        Self {
            major,
            minor,
            patch,
            pre: Vec::new(),
            build: Vec::new(),
        }
    }

    /// `text` as a strict version, or what's wrong with it.
    pub fn parse(text: &str) -> Result<Self, String> {
        let bad = |why: &str| {
            Err(format!(
                "\"{}\" isn't a version (MAJOR.MINOR.PATCH): {why}",
                shorten(text)
            ))
        };
        if text.is_empty() {
            return bad("it's empty");
        }
        if text.len() > MAX_LEN {
            return bad(&format!("it's longer than {MAX_LEN} characters"));
        }
        let (rest, build) = match text.split_once('+') {
            Some((rest, build)) => (rest, Some(build)),
            None => (text, None),
        };
        let (core, pre) = match rest.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (rest, None),
        };
        let parts: Vec<&str> = core.split('.').collect();
        if parts.len() != 3 {
            return bad("it needs three numbers, like 1.0.0");
        }
        let mut numbers = [0u64; 3];
        for (slot, part) in numbers.iter_mut().zip(&parts) {
            *slot = match number(part) {
                Some(n) => n,
                None => return bad("each of its three parts is a number without leading zeros"),
            };
        }
        let pre = match pre {
            None => Vec::new(),
            Some(pre) => {
                let mut idents = Vec::new();
                for ident in pre.split('.') {
                    if !word_chars(ident) {
                        return bad(
                            "its pre-release is dot-separated identifiers of letters, digits and hyphens",
                        );
                    }
                    if ident.bytes().all(|b| b.is_ascii_digit()) {
                        match number(ident) {
                            Some(n) => idents.push(Ident::Number(n)),
                            None => {
                                return bad(
                                    "a number in its pre-release has a leading zero or is too large",
                                );
                            }
                        }
                    } else {
                        idents.push(Ident::Word(ident.to_owned()));
                    }
                }
                idents
            }
        };
        let build = match build {
            None => Vec::new(),
            Some(build) => {
                let idents: Vec<&str> = build.split('.').collect();
                if !idents.iter().all(|i| word_chars(i)) {
                    return bad(
                        "its build metadata is dot-separated identifiers of letters, digits and hyphens",
                    );
                }
                idents.into_iter().map(str::to_owned).collect()
            }
        };
        let [major, minor, patch] = numbers;
        Ok(Self {
            major,
            minor,
            patch,
            pre,
            build,
        })
    }

    pub fn is_prerelease(&self) -> bool {
        !self.pre.is_empty()
    }

    /// The lowest version of `major.minor.patch` (`major.minor.patch-0`): an
    /// exclusive upper bound below every pre-release of it.
    fn floor(major: u64, minor: u64, patch: u64) -> Self {
        Self {
            major,
            minor,
            patch,
            pre: vec![Ident::Number(0)],
            build: Vec::new(),
        }
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                // Identifier by identifier, then the shorter first.
                (false, false) => self.pre.cmp(&other.pre),
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Version {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Version {}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if !self.pre.is_empty() {
            let pre: Vec<String> = self.pre.iter().map(ToString::to_string).collect();
            write!(f, "-{}", pre.join("."))?;
        }
        if !self.build.is_empty() {
            write!(f, "+{}", self.build.join("."))?;
        }
        Ok(())
    }
}

/// `0` or a number without a leading zero, at most [`MAX_NUMBER`].
fn number(text: &str) -> Option<u64> {
    let digits = !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
    if !digits || (text.len() > 1 && text.starts_with('0')) || text.len() > 16 {
        return None;
    }
    text.parse::<u64>().ok().filter(|n| *n <= MAX_NUMBER)
}

/// One identifier: non-empty, `[0-9A-Za-z-]` only.
fn word_chars(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// `text` cut to a length worth quoting back.
fn shorten(text: &str) -> String {
    let mut out: String = text.chars().take(40).collect();
    if out.len() < text.len() {
        out.push('…');
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Eq,
    Gt,
    Ge,
    Lt,
    Le,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Comparator {
    op: Op,
    version: Version,
}

impl Comparator {
    fn matches(&self, v: &Version) -> bool {
        let order = v.cmp(&self.version);
        match self.op {
            Op::Eq => order == Ordering::Equal,
            Op::Gt => order == Ordering::Greater,
            Op::Ge => order != Ordering::Less,
            Op::Lt => order == Ordering::Less,
            Op::Le => order != Ordering::Greater,
        }
    }
}

/// A range of versions (`engines.zeron`): see the module's rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Range {
    /// Any set may match; every comparator of a set must. An empty set
    /// (`*`) matches anything.
    sets: Vec<Vec<Comparator>>,
}

impl Range {
    pub fn parse(text: &str) -> Result<Self, String> {
        let bad = |why: String| {
            Err(format!(
                "\"{}\" isn't a range of versions: {why}",
                shorten(text)
            ))
        };
        if text.trim().is_empty() {
            return bad("it's empty".into());
        }
        if text.len() > MAX_RANGE_LEN {
            return bad(format!("it's longer than {MAX_RANGE_LEN} characters"));
        }
        if text.chars().any(|c| c.is_whitespace() && c != ' ') {
            return bad("only spaces separate its parts".into());
        }
        let mut sets = Vec::new();
        for set in text.split("||") {
            let tokens: Vec<&str> = set.split(' ').filter(|t| !t.is_empty()).collect();
            if tokens.is_empty() {
                return bad("each side of || needs a comparator".into());
            }
            if tokens == ["*"] {
                sets.push(Vec::new());
                continue;
            }
            let mut comparators = Vec::new();
            let mut tokens = tokens.into_iter();
            while let Some(token) = tokens.next() {
                let (op, rest) = split_op(token);
                // An operator alone is followed by its version (`>= 1.0.0`).
                let version = match (op, rest) {
                    (Some(_), "") => match tokens.next() {
                        Some(next) if split_op(next).0.is_none() => next,
                        _ => return bad(format!("\"{token}\" needs a version after it")),
                    },
                    (_, rest) => rest,
                };
                if version == "*" {
                    return bad("* stands alone".into());
                }
                let version = match Version::parse(version) {
                    Ok(v) => v,
                    Err(e) => return bad(e),
                };
                comparators.extend(expand(op.unwrap_or("="), version));
            }
            sets.push(comparators);
        }
        Ok(Self { sets })
    }

    pub fn matches(&self, version: &Version) -> bool {
        self.sets
            .iter()
            .any(|set| set.iter().all(|c| c.matches(version)))
    }
}

/// `token`'s operator, if it starts with one, and the rest.
fn split_op(token: &str) -> (Option<&str>, &str) {
    for op in [">=", "<=", ">", "<", "=", "^", "~"] {
        if let Some(rest) = token.strip_prefix(op) {
            return (Some(op), rest);
        }
    }
    (None, token)
}

/// `op v` as plain comparators.
fn expand(op: &str, v: Version) -> Vec<Comparator> {
    let c = |op, version| Comparator { op, version };
    match op {
        ">=" => vec![c(Op::Ge, v)],
        ">" => vec![c(Op::Gt, v)],
        "<=" => vec![c(Op::Le, v)],
        "<" => vec![c(Op::Lt, v)],
        "^" => {
            let upper = if v.major > 0 {
                Version::floor(v.major + 1, 0, 0)
            } else if v.minor > 0 {
                Version::floor(0, v.minor + 1, 0)
            } else {
                Version::floor(0, 0, v.patch + 1)
            };
            vec![c(Op::Ge, v), c(Op::Lt, upper)]
        }
        "~" => {
            let upper = Version::floor(v.major, v.minor + 1, 0);
            vec![c(Op::Ge, v), c(Op::Lt, upper)]
        }
        _ => vec![c(Op::Eq, v)],
    }
}

/// This Zeron's version, as `engines.zeron` is matched against.
pub fn zeron() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or_else(|_| Version::new(0, 0, 0))
}

/// Whether a plugin whose `engines.zeron` is `range` (none: any) is for
/// Zeron `version`: `Err` says why not (a range it can't read, or one that
/// leaves this version out).
pub fn accepts(range: Option<&str>, version: &Version) -> Result<(), String> {
    let Some(range) = range else { return Ok(()) };
    let parsed = Range::parse(range)?;
    if parsed.matches(version) {
        Ok(())
    } else {
        Err(format!(
            "It's made for Zeron {range}, and this is Zeron {version}"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn strict_versions_only() {
        for good in [
            "0.0.0",
            "1.2.3",
            "10.20.30",
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-0.3.7",
            "1.0.0-x.7.z.92",
            "1.0.0-x-y-z.--",
            "1.0.0+20130313144700",
            "1.0.0-beta+exp.sha.5114f85",
            "1.0.0+21AF26D3----117B344092BD",
            "1.0.0+001",
            "9007199254740991.0.0",
        ] {
            assert!(Version::parse(good).is_ok(), "{good}");
        }
        for bad in [
            "",
            "1",
            "1.2",
            "1.2.3.4",
            "v1.2.3",
            " 1.2.3",
            "1.2.3 ",
            "01.2.3",
            "1.02.3",
            "1.2.03",
            "1.2.3-",
            "1.2.3+",
            "1.2.3-01",
            "1.2.3-a..b",
            "1.2.3-a_b",
            "1.2.3+a..b",
            "1.2.3-é",
            "9007199254740992.0.0",
            "1.0.0-9007199254740992",
            "-1.0.0",
            "1.0.0-+",
            "a.b.c",
            "1.0.0++a",
        ] {
            assert!(Version::parse(bad).is_err(), "{bad}");
        }
        assert!(Version::parse(&format!("1.0.0-{}", "a".repeat(MAX_LEN))).is_err());
        assert_eq!(v("1.0.0-beta.2+exp.1").to_string(), "1.0.0-beta.2+exp.1");
    }

    #[test]
    fn precedence_is_semver_org_s() {
        // semver.org §11's own example, ascending.
        let order = [
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-alpha.beta",
            "1.0.0-beta",
            "1.0.0-beta.2",
            "1.0.0-beta.11",
            "1.0.0-rc.1",
            "1.0.0",
            "2.0.0",
            "2.1.0",
            "2.1.1",
        ];
        for pair in order.windows(2) {
            assert!(v(pair[0]) < v(pair[1]), "{} < {}", pair[0], pair[1]);
        }
        assert_eq!(v("1.0.0+a"), v("1.0.0+b"));
        assert!(v("1.0.0-B") < v("1.0.0-a"), "ASCII");
        assert!(v("1.0.0-1") < v("1.0.0-a"));
        assert!(v("1.0.0-a.1") < v("1.0.0-a.1.0"));
    }

    #[test]
    fn ranges_read_as_npm_s() {
        let cases: &[(&str, &str, bool)] = &[
            (">=0.4.0", "0.4.0", true),
            (">=0.4.0", "0.3.9", false),
            (">=0.4.0", "0.4.0-beta.1", false),
            (">= 0.4.0 <0.6.0", "0.5.9", true),
            (">= 0.4.0 <0.6.0", "0.6.0", false),
            ("^1.2.3", "1.9.0", true),
            ("^1.2.3", "2.0.0-alpha", false),
            ("^1.2.3", "1.2.2", false),
            ("^0.2.3", "0.2.9", true),
            ("^0.2.3", "0.3.0", false),
            ("^0.0.3", "0.0.3", true),
            ("^0.0.3", "0.0.4", false),
            ("~1.2.3", "1.2.9", true),
            ("~1.2.3", "1.3.0", false),
            ("~1.2.3", "1.3.0-rc.1", false),
            ("1.2.3", "1.2.3", true),
            ("=1.2.3", "1.2.3+build", true),
            ("1.2.3", "1.2.4", false),
            ("*", "0.0.1-alpha", true),
            ("<0.3.0 || >=0.5.0", "0.4.0", false),
            ("<0.3.0 || >=0.5.0", "0.5.1", true),
            ("<0.3.0 || *", "9.9.9", true),
            ("<=1.0.0", "1.0.0", true),
            (">1.0.0", "1.0.0", false),
        ];
        for (range, version, matches) in cases {
            assert_eq!(
                Range::parse(range).unwrap().matches(&v(version)),
                *matches,
                "{range} {version}"
            );
        }
        for bad in [
            "",
            " ",
            "1.2",
            ">=",
            ">= ",
            "1.x",
            "1.2.3 - 2.0.0",
            "||",
            ">=1.0.0 ||",
            "^v1.0.0",
            ">=1.0.0\t<2.0.0",
            ">> 1.0.0",
            "* 1.0.0",
            ">=*",
            "latest",
        ] {
            assert!(Range::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_plugin_is_for_this_zeron_unless_its_range_says_not() {
        let here = v("0.4.2");
        assert!(accepts(None, &here).is_ok());
        assert!(accepts(Some(">=0.4.0"), &here).is_ok());
        assert!(
            accepts(Some(">=0.5.0"), &here)
                .unwrap_err()
                .contains("Zeron >=0.5.0")
        );
        assert!(accepts(Some("soon"), &here).is_err());
        assert!(
            Version::parse(env!("CARGO_PKG_VERSION")).is_ok(),
            "this Zeron's own version is strict"
        );
    }
}
