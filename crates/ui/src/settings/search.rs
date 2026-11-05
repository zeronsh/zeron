//! Settings search: what the field atop the Settings sidebar can find, and
//! how a query ranks it.
//!
//! Pages are hand-built, so the index is a table kept beside them: a line per
//! settings row naming its page, the labeled block it sits in, the title the
//! row shows and the words people search by that the title lacks. The row
//! carries its [`SettingId`] as a [`SearchAnchor`], which is how a hit lands
//! on it ([`reveal`]). Shortcuts list themselves from [`ShortcutId::ALL`], as
//! the Shortcuts page does, and every page is an entry of its own.
//!
//! Ranking follows the palette's thread search: every query word must match
//! some field, and a match scores its field's weight, plus half again when it
//! starts a word. A word that no entry spells may still find a title word it
//! misspells, below every real match.

mod reveal;

use std::ops::Range;
use std::sync::LazyLock;

pub use reveal::{PageReveal, SearchAnchor, clear_reveal, page_reveal, reveal};
#[cfg(test)]
pub(crate) use reveal::{lit, take_anchored};

use super::ShortcutId;
use crate::shell::SettingsSection;

/// Field weights, per query word: the row's title, then the words kept for
/// it alone, then its block and its page, which it shares.
const TITLE: u32 = 100;
const TERMS: u32 = 40;
const GROUP: u32 = 30;
const PAGE: u32 = 20;
/// A title that is the whole query outranks every title that only holds it.
const EXACT: u32 = TITLE;
/// A misspelled title or page word: below every real match.
const TYPO: u32 = 10;

macro_rules! settings_index {
    ($(($id:ident, $section:ident, $group:expr, $title:literal, $terms:literal)),+ $(,)?) => {
        /// A settings row the search can land on: the key its
        /// [`SearchAnchor`] carries.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum SettingId {
            $($id),+
        }

        /// The settings rows, each page top to bottom, pages in nav order.
        const ROWS: &[Row] = &[$(Row {
            id: SettingId::$id,
            section: SettingsSection::$section,
            group: $group,
            title: $title,
            terms: $terms,
        }),+];
    };
}

struct Row {
    id: SettingId,
    section: SettingsSection,
    group: Option<&'static str>,
    title: &'static str,
    terms: &'static str,
}

settings_index! {
    (SendMessagesWith, General, None, "Send messages with",
        "enter return submit modifier keyboard composer"),
    (CompactMode, General, None, "Compact mode",
        "collapse thinking tools transcript dense density"),
    (CompactModelPicker, General, None, "Compact model picker",
        "effort reasoning slider model list"),
    (EscapeStopsAgent, General, None, "Stop agent with Escape",
        "esc interrupt cancel halt run keyboard"),
    (PullRequestDestination, General, Some("Links"), "Open pull requests in",
        "pr pull request badges board browser github"),
    (ThreadNaming, General, None, "Thread naming",
        "titles rename sessions model"),
    (WorktreeLocation, General, Some("Git"), "Custom worktree location",
        "folder directory path checkout branches"),
    (CommitCredit, General, Some("Git"), "Credit Zeron in commits",
        "co-author coauthor attribution bot commit message"),
    (ColorScheme, Appearance, None, "Color scheme",
        "system light dark mode"),
    (LightTheme, Appearance, None, "Light theme",
        "palette colors"),
    (DarkTheme, Appearance, None, "Dark theme",
        "palette colors"),
    (AccentColor, Appearance, None, "Accent color",
        "tint highlight swatch"),
    (WallpaperColors, Appearance, None, "Match wallpaper colors",
        "tint accent surface desktop"),
    (Glass, Appearance, Some("Material and background"), "Glass",
        "frosted translucent opaque transparency blur vibrancy surface"),
    (NewThreadBackground, Appearance, Some("Material and background"), "New thread background",
        "image picture wallpaper composer"),
    (WallpaperFolder, Appearance, Some("Material and background"), "Wallpaper folder",
        "random shuffle images directory"),
    (BackgroundEffect, Appearance, Some("Material and background"), "Background effect",
        "blur dim image"),
    (ReduceMotion, Appearance, Some("Motion"), "Reduce motion",
        "animations accessibility"),
    (PauseAnimations, Appearance, Some("Motion"), "Pause animations in background",
        "focus unfocused inactive window battery"),
    (SmoothScrolling, Appearance, Some("Motion"), "Smooth scrolling",
        "mouse wheel glide ease"),
    (ThemeLibrary, Appearance, None, "Theme library",
        "import add custom themes"),
    (InterfaceFont, Appearance, Some("Fonts and layout"), "Interface font",
        "typeface text size menus conversations"),
    (TerminalFont, Appearance, Some("Fonts and layout"), "Terminal font",
        "monospace typeface size"),
    (CodeFont, Appearance, Some("Fonts and layout"), "Code & diff font",
        "monospace typeface editor files size"),
    (ConversationWidth, Appearance, Some("Fonts and layout"), "Conversation width",
        "transcript messages composer column maximum"),
    (RenderMath, Appearance, Some("Fonts and layout"), "Render math",
        "latex formulas equations markdown typeset"),
    (MathFont, Appearance, Some("Fonts and layout"), "Math font",
        "latex formulas typeface"),
    (MathSize, Appearance, Some("Fonts and layout"), "Math size",
        "latex formulas scale"),
    (DesktopNotifications, Notifications, Some("Desktop"), "Desktop notifications",
        "alerts banners system"),
    (AgentUpdates, Notifications, Some("Desktop"), "Agent updates",
        "cli banner new version"),
    (BackgroundOnly, Notifications, Some("Desktop"), "Only when in the background",
        "focus unfocused"),
    (SessionSounds, Notifications, Some("Sounds"), "Session sounds",
        "audio chime mute"),
    (TaskCompletedSound, Notifications, Some("Sounds"), "Task completed",
        "done finished audio sound"),
    (InputRequiredSound, Notifications, Some("Sounds"), "Input required",
        "question waiting approval audio sound"),
    (ErrorSound, Notifications, Some("Sounds"), "Errors and disconnections",
        "failure offline audio sound"),
    (CodexVoiceDevice, Voice, None, "Codex voice device",
        "remote host machine"),
    (CodexVoiceStyle, Voice, None, "Codex voice style",
        "speech tone"),
    (Dictation, Voice, None, "Dictation",
        "speech to text transcribe talk"),
    (Microphone, Voice, None, "Microphone",
        "input audio mic"),
    (SpeechModel, Voice, None, "Speech model",
        "parakeet download remove"),
    (SkillDollar, Harnesses, Some("Completion"), "Use $ for skills",
        "dollar skill picker composer autocomplete"),
    (SkillSlash, Harnesses, Some("Completion"), "Separate / commands",
        "slash menu skills autocomplete"),
    (NativeSubagents, Harnesses, Some("Delegation"), "Native subagents",
        "subagent delegate side chats tasks spawn"),
    (UpdatePolicy, Harnesses, Some("Updates"), "Update policy",
        "automatic upgrade version cli"),
    (CloudUsage, Cloud, None, "Usage",
        "hours consumption range"),
    (CloudGitHub, Cloud, None, "GitHub",
        "connect repositories account"),
    (Credits, Billing, None, "Credits",
        "balance"),
    (Plan, Billing, None, "Plan",
        "subscription upgrade manage stripe"),
    (ExtraCredits, Billing, None, "Extra credits",
        "buy packs top up"),
    (Autosave, Files, None, "Autosave",
        "save automatically"),
    (AutosaveDelay, Files, None, "Autosave delay",
        "debounce wait"),
    (WordWrap, Files, None, "Word wrap",
        "soft wrap long lines editor"),
    (ShowAllFiles, Files, None, "Show hidden and ignored files",
        "dotfiles gitignore tree"),
    (CaptureAppshots, Appshots, None, "Capture Appshots",
        "screenshot screen"),
    (AppshotSound, Appshots, None, "Capture sound",
        "shutter audio"),
    (AppshotDestination, Appshots, None, "Destination",
        "session composer where"),
    (WindowCapture, Appshots, None, "Window capture",
        "screen recording permission"),
    (ApplicationText, Appshots, None, "Application text",
        "accessibility semantic permission"),
}

impl SettingId {
    /// The row a hit lands on while this one is hidden: a sub-setting shows
    /// only once its parent is on (or, for a background effect, set).
    pub fn parent(self) -> Option<SettingId> {
        match self {
            SettingId::BackgroundEffect => Some(SettingId::NewThreadBackground),
            SettingId::Microphone | SettingId::SpeechModel => Some(SettingId::Dictation),
            SettingId::AutosaveDelay => Some(SettingId::Autosave),
            _ => None,
        }
    }

    /// A row each provider shows in its expanded details on Providers.
    pub fn per_provider(self) -> bool {
        matches!(
            self,
            SettingId::SkillDollar
                | SettingId::SkillSlash
                | SettingId::NativeSubagents
                | SettingId::UpdatePolicy
        )
    }
}

/// Shortcuts recorded on a feature page rather than on Shortcuts.
const FEATURE_SHORTCUTS: [(ShortcutId, SettingsSection, &str, &str); 2] = [
    (
        ShortcutId::ToggleDictation,
        SettingsSection::Voice,
        "Dictation shortcut",
        "hold to dictate talk keybinding hotkey",
    ),
    (
        ShortcutId::CaptureAppshot,
        SettingsSection::Appshots,
        "Global shortcut",
        "capture appshot screenshot preferred keybinding hotkey",
    ),
];

/// Where a hit lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// A settings row.
    Setting(SettingId),
    /// A shortcut's row: on Shortcuts, or on the feature page that owns it.
    Shortcut(ShortcutId),
    /// A page as a whole, opened at its top.
    Page(SettingsSection),
}

impl Target {
    /// Where a hit lands while this row is hidden. A provider's own rows show
    /// only once it is expanded: until then, the provider list, to pick one.
    fn fallback(self) -> Option<Target> {
        match self {
            Target::Setting(id) if id.per_provider() => {
                Some(Target::Page(SettingsSection::Harnesses))
            }
            Target::Setting(id) => id.parent().map(Target::Setting),
            Target::Shortcut(_) | Target::Page(_) => None,
        }
    }
}

impl From<SettingId> for Target {
    fn from(id: SettingId) -> Self {
        Target::Setting(id)
    }
}

impl From<ShortcutId> for Target {
    fn from(id: ShortcutId) -> Self {
        Target::Shortcut(id)
    }
}

/// One thing the search can return.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Entry {
    pub target: Target,
    pub section: SettingsSection,
    /// The labeled block the row sits in ("Motion"), for the result's trail.
    pub group: Option<&'static str>,
    pub title: &'static str,
    /// Words people search by that the title lacks.
    pub terms: &'static str,
}

/// A ranked result: the entry and where the query shows in its title.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub entry: Entry,
    /// Byte ranges of [`Entry::title`] to highlight, sorted and disjoint.
    pub ranges: Vec<Range<usize>>,
}

/// An entry with its fields lowercased once. Every indexed string is ASCII
/// (a test holds it to that), so lowercasing keeps byte offsets and a match
/// in the folded title is a range of the shown one.
struct Indexed {
    entry: Entry,
    title: String,
    group: String,
    page: String,
    terms: String,
}

/// Every entry in nav order: each page, its rows top to bottom, then the
/// shortcuts it records.
static INDEX: LazyLock<Vec<Indexed>> = LazyLock::new(|| {
    let mut entries = Vec::new();
    for section in SettingsSection::ALL {
        entries.push(Entry {
            target: Target::Page(section),
            section,
            group: None,
            title: section.label(),
            terms: page_terms(section),
        });
        entries.extend(
            ROWS.iter()
                .filter(|row| row.section == section)
                .map(|row| Entry {
                    target: Target::Setting(row.id),
                    section,
                    group: row.group,
                    title: row.title,
                    terms: row.terms,
                }),
        );
        entries.extend(shortcut_entries(section));
    }
    entries
        .into_iter()
        .map(|entry| Indexed {
            entry,
            title: entry.title.to_ascii_lowercase(),
            group: entry.group.unwrap_or_default().to_ascii_lowercase(),
            page: entry.section.label().to_ascii_lowercase(),
            terms: entry.terms.to_ascii_lowercase(),
        })
        .collect()
});

/// The shortcuts whose rows `section` shows.
fn shortcut_entries(section: SettingsSection) -> Vec<Entry> {
    let featured = FEATURE_SHORTCUTS
        .into_iter()
        .filter(|(id, page, ..)| *page == section && id.available())
        .map(|(id, section, title, terms)| Entry {
            target: Target::Shortcut(id),
            section,
            group: None,
            title,
            terms,
        });
    let listed = ShortcutId::ALL
        .into_iter()
        .filter(|id| {
            section == SettingsSection::Shortcuts
                && id.available()
                && !FEATURE_SHORTCUTS
                    .iter()
                    .any(|(featured, ..)| featured == id)
        })
        .map(|id| Entry {
            target: Target::Shortcut(id),
            section,
            group: Some(super::shortcuts::group(id)),
            title: id.label(),
            terms: "shortcut keybinding hotkey",
        });
    featured.chain(listed).collect()
}

/// What a page is also called, and what it holds that has no row of its own.
fn page_terms(section: SettingsSection) -> &'static str {
    match section {
        SettingsSection::General => "preferences composer conversations behavior",
        SettingsSection::Appearance => "theme look colors fonts",
        SettingsSection::Notifications => "alerts sounds banners",
        SettingsSection::Voice => "speech dictation microphone codex",
        SettingsSection::Shortcuts => "keyboard keybindings hotkeys keys",
        SettingsSection::Harnesses | SettingsSection::Agents => {
            "harnesses agents cli claude code codex cursor devin grok hermes pi opencode \
             antigravity muse install enable disable update policy accounts sign in login"
        }
        SettingsSection::Devices => "machines computers remote this device other devices rename",
        SettingsSection::Cloud => "machine remote github repositories usage",
        SettingsSection::Billing => "plan credits subscription payment stripe",
        SettingsSection::Files => "editor autosave wrap hidden",
        SettingsSection::Appshots => "screenshots capture permissions",
        SettingsSection::Archived => "archive unarchive restore delete sessions threads",
    }
}

/// What a hit on `section` could land on that no anchor in `anchored`
/// carries: neither its row nor, while that is hidden, its stand-in.
#[cfg(test)]
pub(crate) fn unanchored(section: SettingsSection, anchored: &[Target]) -> Vec<Target> {
    INDEX
        .iter()
        .map(|indexed| indexed.entry)
        .filter(|entry| entry.section == section && !matches!(entry.target, Target::Page(_)))
        .map(|entry| entry.target)
        .filter(|target| {
            !anchored.contains(target)
                && target
                    .fallback()
                    .is_none_or(|fallback| !anchored.contains(&fallback))
        })
        .collect()
}

/// The rows each provider shows in its expanded details.
#[cfg(test)]
pub(crate) fn per_provider_rows() -> Vec<Target> {
    ROWS.iter()
        .filter(|row| row.id.per_provider())
        .map(|row| Target::Setting(row.id))
        .collect()
}

/// The row titles drawn on `section` that no entry holds: settings the
/// search cannot find. (Shortcuts builds General's rows too, so a title may
/// be held on another page.) A page with no entries of its own lists what
/// it holds (devices, archived sessions) and is not checked.
#[cfg(test)]
pub(crate) fn unindexed(section: SettingsSection, drawn: &[gpui::SharedString]) -> Vec<String> {
    let rows = |indexed: &&Indexed| !matches!(indexed.entry.target, Target::Page(_));
    if !INDEX
        .iter()
        .filter(rows)
        .any(|indexed| indexed.entry.section == section)
    {
        return Vec::new();
    }
    // An entry may say more than its row ("Dictation shortcut" for Voice's
    // "Shortcut"), never less.
    drawn
        .iter()
        .map(|title| title.to_ascii_lowercase())
        .filter(|title| {
            !INDEX
                .iter()
                .filter(rows)
                .any(|indexed| indexed.title.contains(title.as_str()))
        })
        .collect()
}

/// The hits for `query`, best first; none for a blank query. `visible` says
/// which pages the nav shows (Cloud and Billing follow the account).
pub fn search(query: &str, visible: impl Fn(SettingsSection) -> bool) -> Vec<Hit> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() {
        return Vec::new();
    }
    let candidates: Vec<&Indexed> = INDEX
        .iter()
        .filter(|indexed| visible(indexed.entry.section))
        .collect();
    // Typo matching is a fallback: a word some entry spells means what it
    // says, and near spellings of it would only add noise.
    let misspelled: Vec<bool> = words
        .iter()
        .map(|word| !candidates.iter().any(|indexed| indexed.spells(word)))
        .collect();
    let whole = words.join(" ");
    let mut hits: Vec<(u32, bool, Hit)> = candidates
        .into_iter()
        .filter_map(|indexed| {
            let (score, hit) = indexed.score(&words, &misspelled, &whole)?;
            Some((score, matches!(hit.entry.target, Target::Page(_)), hit))
        })
        .collect();
    // Equal scores: a page before the rows it holds, then nav order (the sort
    // is stable).
    hits.sort_by(|(a, a_page, _), (b, b_page, _)| b.cmp(a).then(b_page.cmp(a_page)));
    hits.into_iter().map(|(.., hit)| hit).collect()
}

impl Indexed {
    fn fields(&self) -> [(&str, u32); 4] {
        [
            (&self.title, TITLE),
            (&self.terms, TERMS),
            (&self.group, GROUP),
            (&self.page, PAGE),
        ]
    }

    fn spells(&self, word: &str) -> bool {
        self.fields().iter().any(|(field, _)| field.contains(word))
    }

    /// `None` when some word matches nothing here.
    fn score(&self, words: &[String], misspelled: &[bool], whole: &str) -> Option<(u32, Hit)> {
        let mut total = 0;
        let mut ranges = Vec::new();
        for (word, &misspelled) in words.iter().zip(misspelled) {
            let exact = self
                .fields()
                .iter()
                .filter_map(|(field, weight)| field_score(field, word, *weight))
                .max();
            total += match exact {
                Some(score) => {
                    ranges.extend(title_match(&self.title, word));
                    score
                }
                None if misspelled => {
                    let in_title = misspelled_word(&self.title, word);
                    if in_title.is_none() && misspelled_word(&self.page, word).is_none() {
                        return None;
                    }
                    ranges.extend(in_title);
                    TYPO
                }
                None => return None,
            };
        }
        if self.title == whole {
            total += EXACT;
        }
        let hit = Hit {
            entry: self.entry,
            ranges: merged(ranges),
        };
        Some((total, hit))
    }
}

/// `weight` for a match, plus half again when one starts a word.
fn field_score(field: &str, word: &str, weight: u32) -> Option<u32> {
    let mut best = None;
    for (at, _) in field.match_indices(word) {
        if starts_word(field, at) {
            return Some(weight + weight / 2);
        }
        best = Some(weight);
    }
    best
}

fn starts_word(field: &str, at: usize) -> bool {
    field[..at]
        .chars()
        .next_back()
        .is_none_or(|previous| !previous.is_alphanumeric())
}

/// Where `word` shows in `title`: where it starts a word, else where it
/// first appears.
fn title_match(title: &str, word: &str) -> Option<Range<usize>> {
    let mut first = None;
    for (at, _) in title.match_indices(word) {
        if starts_word(title, at) {
            return Some(at..at + word.len());
        }
        first.get_or_insert(at..at + word.len());
    }
    first
}

/// The word of `field` that `word` misspells, whole or as far as it is
/// typed: within one edit for up to seven letters, two from eight on.
fn misspelled_word(field: &str, word: &str) -> Option<Range<usize>> {
    let budget = match word.len() {
        0..=3 => return None,
        4..=7 => 1,
        _ => 2,
    };
    let typed = word.as_bytes();
    let mut start = None;
    for (at, byte) in field.bytes().chain(*b" ").enumerate() {
        match (byte.is_ascii_alphanumeric(), start) {
            (true, None) => start = Some(at),
            (false, Some(from)) => {
                start = None;
                let candidate = &field.as_bytes()[from..at];
                // The whole word, or its start as long as the typed word
                // give or take a letter.
                let lengths = [
                    candidate.len(),
                    typed.len() - 1,
                    typed.len(),
                    typed.len() + 1,
                ];
                if lengths.into_iter().any(|len| {
                    len <= candidate.len()
                        && edit_distance(typed, &candidate[..len], budget).is_some()
                }) {
                    return Some(from..at);
                }
            }
            _ => {}
        }
    }
    None
}

/// Optimal string alignment distance (a swap of neighbors counts once), or
/// `None` once it exceeds `budget`.
fn edit_distance(a: &[u8], b: &[u8], budget: usize) -> Option<usize> {
    if a.len().abs_diff(b.len()) > budget {
        return None;
    }
    // Three rolling rows: two back (for swaps), the last, and the current.
    let width = b.len() + 1;
    let mut rows = vec![0usize; 3 * width];
    let (mut before, mut last, mut current) = (0, width, 2 * width);
    for (j, cell) in rows[last..last + width].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        rows[current] = i;
        let mut row_min = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut cell = (rows[last + j] + 1)
                .min(rows[current + j - 1] + 1)
                .min(rows[last + j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                cell = cell.min(rows[before + j - 2] + 1);
            }
            rows[current + j] = cell;
            row_min = row_min.min(cell);
        }
        if row_min > budget {
            return None;
        }
        (before, last, current) = (last, current, before);
    }
    let distance = rows[last + b.len()];
    (distance <= budget).then_some(distance)
}

fn merged(mut ranges: Vec<Range<usize>>) -> Vec<Range<usize>> {
    ranges.sort_by_key(|range| range.start);
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
    for range in ranges {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    fn titles(query: &str) -> Vec<&'static str> {
        search(query, |_| true)
            .into_iter()
            .map(|hit| hit.entry.title)
            .collect()
    }

    fn top(query: &str) -> Target {
        search(query, |_| true)
            .first()
            .unwrap_or_else(|| panic!("no hit for {query:?}"))
            .entry
            .target
    }

    fn marked(hit: &Hit) -> Vec<&'static str> {
        let title = hit.entry.title;
        hit.ranges
            .iter()
            .map(|range| &title[range.clone()])
            .collect()
    }

    #[test]
    fn the_index_is_ascii_unique_and_complete() {
        let mut targets = Vec::new();
        for indexed in INDEX.iter() {
            let entry = indexed.entry;
            for text in [entry.title, entry.terms, entry.group.unwrap_or_default()] {
                assert!(text.is_ascii(), "{text:?}");
            }
            assert!(!entry.title.is_empty(), "{entry:?}");
            assert!(!targets.contains(&entry.target), "listed twice: {entry:?}");
            targets.push(entry.target);
        }
        for row in ROWS {
            assert!(targets.contains(&Target::Setting(row.id)), "{:?}", row.id);
        }
        for section in SettingsSection::ALL {
            assert!(targets.contains(&Target::Page(section)), "{section:?}");
        }
        // Every shortcut the Shortcuts page or a feature page records.
        for id in ShortcutId::ALL.into_iter().filter(|id| id.available()) {
            assert!(targets.contains(&Target::Shortcut(id)), "{id:?}");
        }
    }

    #[test]
    fn blank_queries_find_nothing() {
        assert!(search("", |_| true).is_empty());
        assert!(search("   ", |_| true).is_empty());
    }

    #[test]
    fn every_word_must_match_some_field() {
        assert_eq!(
            top("motion reduce"),
            Target::Setting(SettingId::ReduceMotion)
        );
        // The page's name is a field like any other.
        assert_eq!(top("appearance glass"), Target::Setting(SettingId::Glass));
        assert!(search("glass sounds", |_| true).is_empty());
    }

    #[test]
    fn titles_outrank_groups_pages_and_terms() {
        // "Motion" is one row's title and the others' group.
        assert_eq!(
            titles("motion")[..2],
            ["Reduce motion", "Pause animations in background"]
        );
        // A page's own name finds the page, then the rows it holds.
        let notifications = search("notifications", |_| true);
        assert_eq!(
            notifications[0].entry.target,
            Target::Page(SettingsSection::Notifications)
        );
        assert!(
            notifications[1..]
                .iter()
                .all(|hit| hit.entry.section == SettingsSection::Notifications)
        );
        // Terms find what the title does not say.
        assert_eq!(top("frosted"), Target::Setting(SettingId::Glass));
        assert_eq!(top("latex"), Target::Setting(SettingId::RenderMath));
        assert_eq!(
            top("mouse wheel"),
            Target::Setting(SettingId::SmoothScrolling)
        );
        // A row's own words outrank the block it shares: "size" is the font
        // rows' word, while "font" only names Math size's block.
        assert_eq!(top("font size"), Target::Setting(SettingId::InterfaceFont));
    }

    #[test]
    fn word_starts_whole_titles_and_pages_rank_first() {
        assert_eq!(top("wrap"), Target::Setting(SettingId::WordWrap));
        // The exact title beats titles that only hold it.
        assert_eq!(top("autosave"), Target::Setting(SettingId::Autosave));
        // Equal scores put the page ahead of the rows titled like it.
        assert_eq!(top("shortcut"), Target::Page(SettingsSection::Shortcuts));
    }

    #[test]
    fn ties_keep_nav_order() {
        let hits = titles("theme");
        let light = hits.iter().position(|title| *title == "Light theme");
        let dark = hits.iter().position(|title| *title == "Dark theme");
        assert!(light.unwrap() < dark.unwrap());
    }

    #[test]
    fn shortcuts_are_found_by_name_under_their_pages() {
        assert_eq!(
            search("left sidebar", |_| true)[0].entry,
            Entry {
                target: Target::Shortcut(ShortcutId::ToggleSidebar),
                section: SettingsSection::Shortcuts,
                group: Some("Panels"),
                title: "Toggle left sidebar",
                terms: "shortcut keybinding hotkey",
            }
        );
        // Dictation's shortcut is recorded on Voice.
        let dictate = &search("dictate", |_| true)[0].entry;
        assert_eq!(
            (dictate.target, dictate.section),
            (
                Target::Shortcut(ShortcutId::ToggleDictation),
                SettingsSection::Voice
            )
        );
    }

    #[test]
    fn a_providers_own_rows_land_on_the_list_until_it_is_expanded() {
        assert_eq!(
            top("subagents"),
            Target::Setting(SettingId::NativeSubagents)
        );
        assert_eq!(
            top("update policy"),
            Target::Setting(SettingId::UpdatePolicy)
        );
        assert_eq!(
            Target::Setting(SettingId::NativeSubagents).fallback(),
            Some(Target::Page(SettingsSection::Harnesses))
        );
    }

    #[test]
    fn hidden_pages_are_not_searched() {
        let signed_out = |section: SettingsSection| {
            !matches!(section, SettingsSection::Cloud | SettingsSection::Billing)
        };
        assert_eq!(top("credits"), Target::Setting(SettingId::Credits));
        // Signed out, Billing's credits are gone: only General's near
        // spelling ("Credit Zeron in commits") is left.
        let hits = search("credits", signed_out);
        assert!(!hits.is_empty() && hits.iter().all(|hit| signed_out(hit.entry.section)));
        assert_eq!(
            hits[0].entry.target,
            Target::Setting(SettingId::CommitCredit)
        );
    }

    #[test]
    fn highlights_mark_the_query_in_the_title() {
        assert_eq!(marked(&search("wrap", |_| true)[0]), ["wrap"]);
        // Each word marks its own place.
        let hit = &search("pause back", |_| true)[0];
        assert_eq!(hit.entry.title, "Pause animations in background");
        assert_eq!(marked(hit), ["Pause", "back"]);
        // Overlapping marks merge.
        assert_eq!(marked(&search("wor word", |_| true)[0]), ["Word"]);
        // A match only in the terms marks nothing.
        assert!(search("frosted", |_| true)[0].ranges.is_empty());
    }

    #[test]
    fn misspelled_words_still_find_their_title() {
        assert_eq!(
            top("notifcations"),
            Target::Page(SettingsSection::Notifications)
        );
        assert_eq!(top("microphnoe"), Target::Setting(SettingId::Microphone));
        assert_eq!(top("dictatoin"), Target::Setting(SettingId::Dictation));
        // A word cut short still counts, as far as it is typed.
        assert_eq!(top("wallpapr"), Target::Setting(SettingId::WallpaperColors));
        let hit = &search("wallpapr folder", |_| true)[0];
        assert_eq!(
            hit.entry.target,
            Target::Setting(SettingId::WallpaperFolder)
        );
        assert_eq!(marked(hit), ["Wallpaper", "folder"]);
    }

    #[test]
    fn typos_never_crowd_real_matches() {
        // "mode" is spelled by real entries, so it never finds "code".
        assert!(!titles("mode").contains(&"Code & diff font"));
        // Short words are never corrected.
        assert!(search("zqx", |_| true).is_empty());
        assert!(search("fnt", |_| true).is_empty());
    }

    #[test]
    fn edit_distance_counts_swaps_once_and_stops_at_the_budget() {
        assert_eq!(edit_distance(b"theme", b"theme", 1), Some(0));
        assert_eq!(edit_distance(b"thmee", b"theme", 1), Some(1));
        assert_eq!(edit_distance(b"them", b"theme", 1), Some(1));
        assert_eq!(edit_distance(b"tehem", b"theme", 1), None);
        assert_eq!(edit_distance(b"tehem", b"theme", 2), Some(2));
        assert_eq!(edit_distance(b"abc", b"abcdef", 2), None);
        assert_eq!(edit_distance(b"", b"ab", 2), Some(2));
    }
}
