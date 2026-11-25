//! What the main window runs: Zeron's own interface, with the active template
//! laid over it. A template is a whole app (its `app.luau`, `screens/`,
//! `ds/`, `assets/`…, like a WordPress theme), and any file it doesn't have
//! is Zeron's, as a child theme inherits.
//!
//! zgx fixes a host's app file when the host is made, and keys each module's
//! state by its path from the app's folder (`screens/shell/sidebar.luau::…`).
//! So the main window always runs from one folder, `<data>/templates/.live/`,
//! and switching templates rewrites what's in it, then reloads: no new window,
//! and state kept wherever the two have the same module.
//!
//! ```text
//! <data>/templates/
//!   .zeron/        Zeron's own, as this build has it (`sync_zeron`), with
//!                  zeron-template.json, .hashes.json, .catalog.json
//!   .live/         .zeron with the active template on top (`compose`)
//!   .active        the active template's id ("zeron", Zeron's own)
//!   .status.json   what failed, per template
//!   <id>/          a template: zeron-template.json, any of .zeron's files
//! ```
//!
//! Files are copied, never linked, and rewritten only where their bytes
//! differ (zgx reloads for any file whose time moves). Some paths are never a
//! template's, whatever it holds (`NEVER_TEMPLATED`): Zeron's own always.

use std::{
    collections::{BTreeMap, BTreeSet},
    hash::{DefaultHasher, Hash, Hasher},
    path::{Component, Path, PathBuf},
};

pub const ZERON: &str = ".zeron";
pub const LIVE: &str = ".live";
pub const ACTIVE: &str = ".active";
pub const STATUS: &str = ".status.json";
/// Zeron's own template's id.
pub const BUILTIN: &str = "zeron";
/// This Zeron's version (the root workspace's, from build.rs): what `.zeron`
/// is stamped with, and what a template made from it records.
pub const ZERON_VERSION: &str = env!("ZERON_VERSION");

/// Which of a template's files are what: the engine's and the app's one answer
/// (zeron-plugin-guard `template`).
pub use zeron_plugin_guard::template::{meta as template_meta, plain, protected as never_templated};
use zeron_plugin_guard::template::{composed, of_the_app};

/// A template's tokens: never laid over Zeron's, but beside them as
/// `TEMPLATE_TOKENS` (Zeron ships a stub there), which Zeron's run after
/// their own, so whatever the template doesn't name is the running Zeron's.
/// (No template has a file of its own there: zeron-plugin-guard's
/// `NEVER_TEMPLATED`, the folder of that name too.)
pub const TOKENS: &str = "ds/tokens.luau";
pub const TEMPLATE_TOKENS: &str = "ds/tokens_template.luau";

/// Whether `rel` is where Zeron lays a template's tokens, or would stand for
/// Zeron's own, as a disk that folds names has it: `TEMPLATE_TOKENS`, or
/// anything under a folder named as it or `TOKENS` (whose init.luau Luau
/// takes in place of the file beside it, so `ds/tokens/init.luau` would skip
/// Zeron's tokens whole). Never a template's own, whatever the guard says:
/// the app's own refusal beside it, for a template's folder edited by hand.
pub fn laid_by_zeron(rel: &str) -> bool {
    let rel = zeron_plugin_guard::path::fold(rel);
    let under = |file: &str| rel.strip_prefix(&zeron_plugin_guard::path::fold(file.trim_end_matches(".luau"))).is_some_and(|rest| rest.starts_with('/'));
    let theme = |scheme: &str| {
        let at = format!("ds/themes/template-{scheme}.json");
        rel == zeron_plugin_guard::path::fold(&at) || rel.strip_prefix(&zeron_plugin_guard::path::fold(&at)).is_some_and(|rest| rest.starts_with('/'))
    };
    rel == zeron_plugin_guard::path::fold(TEMPLATE_TOKENS) || under(TEMPLATE_TOKENS) || under(TOKENS) || theme("dark") || theme("light")
}

/// Zeron's own files, as a disk that folds names has them, and the folders
/// they're in: what a template's file can't be laid across (`clashes`).
pub struct ZeronShape {
    files: BTreeSet<String>,
    folders: BTreeSet<String>,
}

impl ZeronShape {
    pub fn of<'a>(rels: impl IntoIterator<Item = &'a String>) -> Self {
        let files: BTreeSet<String> = rels.into_iter().map(|rel| zeron_plugin_guard::path::fold(rel)).collect();
        let folders = files.iter().flat_map(|rel| rel.match_indices('/').map(move |(at, _)| rel[..at].to_string())).collect();
        Self { files, folders }
    }

    /// Whether a template's file `rel` can't be laid beside Zeron's: a
    /// folder of Zeron's has its name (`screens`), or its path goes through
    /// a file of Zeron's (`ds/button.luau/x.luau`). Laid, it would fail
    /// part-way, the files before it written.
    pub fn clashes(&self, rel: &str) -> bool {
        let rel = zeron_plugin_guard::path::fold(rel);
        self.folders.contains(&rel) || rel.match_indices('/').any(|(at, _)| self.files.contains(&rel[..at]))
    }
}

/// What `template_status` says of a theme file that doesn't read as a theme.
pub const UNREAD_THEME: &str = "Not used: this file doesn't read as a theme.";

/// What `template_status` says of a file `clashes` left out.
pub const CLASHES: &str = "Not used: Zeron has a folder of this name, or a file where this path has a folder.";

pub fn hash(bytes: &[u8]) -> String {
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// Every regular file under `dir` (no links followed), by its path from `dir`,
/// that `keep` keeps. Dot folders are skipped.
pub fn files(dir: &Path, keep: impl Fn(&str) -> bool) -> BTreeMap<String, PathBuf> {
    let mut out = BTreeMap::new();
    walk(dir, keep, |rel, path, _| {
        out.insert(rel, path);
    });
    out
}

/// A path under a folder, as a package names it: `/` between its parts. Only
/// Windows separates them with `\`; elsewhere it's a character in a name,
/// which no package path has (`path::is_safe`), so such a file is no file of
/// it: `screens\sidebar.luau` is never Zeron's `screens/sidebar.luau`.
pub fn rel_path(rel: &Path) -> String {
    let rel = rel.to_string_lossy();
    if cfg!(windows) { rel.replace('\\', "/") } else { rel.into_owned() }
}

/// Each regular file under `dir` that `keep` keeps, as [`files`] finds them,
/// handed to `each` with its folder entry (which has its size and time
/// written: on Windows without opening the file, which `fs::metadata` does).
fn walk(dir: &Path, keep: impl Fn(&str) -> bool, mut each: impl FnMut(String, PathBuf, &std::fs::DirEntry)) {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(at) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&at) else { continue };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else { continue };
            let path = entry.path();
            let Ok(rel) = path.strip_prefix(dir) else { continue };
            let rel = rel_path(rel);
            if kind.is_dir() {
                if !entry.file_name().to_string_lossy().starts_with('.') {
                    stack.push(path);
                }
            } else if kind.is_file() && plain(&rel) && zeron_plugin_guard::path::is_safe(&rel) && keep(&rel) {
                each(rel, path, &entry);
            }
        }
    }
}

/// Writes `bytes` to `path` unless it holds them already (through a file
/// beside it renamed over it, so nothing reads half a file): whether it wrote.
pub fn put(path: &Path, bytes: &[u8]) -> std::io::Result<bool> {
    if std::fs::read(path).is_ok_and(|now| now == bytes) {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.tmp"));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(true)
}

/// Makes `to` hold exactly `want` (by path from `to`: where each comes from),
/// touching only what differs and leaving `keep`'s paths alone: how many
/// files changed.
pub fn mirror(to: &Path, want: &BTreeMap<String, PathBuf>, keep: impl Fn(&str) -> bool) -> std::io::Result<usize> {
    mirror_reading(to, want, keep, |_, _| {})
}

/// [`mirror`], handing each wanted file's bytes to `read` as they're read.
///
/// The files are read and written on several threads: on Windows, the
/// antivirus scans each file as it's first read and as it's written, one at
/// a time on the thread doing it, so a thousand files laid down one by one
/// (a first launch, Zeron's own copied twice over) took six seconds before
/// the window could open, and a few at once take a fraction of that.
fn mirror_reading(
    to: &Path,
    want: &BTreeMap<String, PathBuf>,
    keep: impl Fn(&str) -> bool,
    read: impl Fn(&str, &[u8]) + Sync,
) -> std::io::Result<usize> {
    let changed = std::sync::atomic::AtomicUsize::new(0);
    let jobs: Vec<(&String, &PathBuf)> = want.iter().collect();
    in_parallel(&jobs, |&(rel, from)| {
        let bytes = std::fs::read(from)?;
        read(rel, &bytes);
        if put(&to.join(rel), &bytes)? {
            changed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        Ok(())
    })?;
    let mut changed = changed.into_inner();
    let mut held: BTreeSet<String> = BTreeSet::new();
    // (A file under another spelling of a wanted path is that file where
    // the disk folds names: kept, never taken for a stray.)
    let spelt: BTreeMap<String, &String> = want.keys().map(|rel| (zeron_plugin_guard::path::fold(rel), rel)).collect();
    let wanted_as = |rel: &str, path: &Path| spelt.get(&zeron_plugin_guard::path::fold(rel)).is_some_and(|w| same_file(&to.join(w), path));
    for (rel, path) in files(to, |_| true) {
        if !want.contains_key(&rel) && !keep(&rel) && !wanted_as(&rel, &path) {
            std::fs::remove_file(path)?;
            changed += 1;
        } else {
            held.insert(rel);
        }
    }
    // Then the folders nothing wanted or kept is in. Luau takes a folder's
    // init.luau over the file beside it, even when there's none, so one left
    // by an older lay-down (`ds/tokens_template/`) would stand for the module
    // of that name. Anyone can write here (it's a folder like any), so what
    // they may have left is never deleted: a folder empty once the strays
    // are gone is removed; one that still holds something (a dot file, a
    // `.git`, a name no listing reads) stays as it is, unless it stands for
    // a wanted module, and then it's moved aside whole (`SET_ASIDE`).
    let folders: BTreeMap<String, &str> = want
        .keys()
        .map(String::as_str)
        .chain(held.iter().map(String::as_str))
        .flat_map(|rel| rel.match_indices('/').map(move |(at, _)| &rel[..at]))
        .map(|folder| (zeron_plugin_guard::path::fold(folder), folder))
        .collect();
    let modules: BTreeSet<String> = want.keys().filter_map(|rel| rel.strip_suffix(".luau")).map(zeron_plugin_guard::path::fold).collect();
    changed += settle_folders(to, to, &folders, &modules)?;
    Ok(changed)
}

/// Whether `a` and `b` are one file (or folder) on the disk: one of another
/// spelling where the disk folds names.
fn same_file(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        matches!((std::fs::metadata(a), std::fs::metadata(b)), (Ok(x), Ok(y)) if x.dev() == y.dev() && x.ino() == y.ino())
    }
    #[cfg(not(unix))]
    {
        matches!((std::fs::canonicalize(a), std::fs::canonicalize(b)), (Ok(x), Ok(y)) if x == y)
    }
}

/// Where a lay-down moves a folder that stands for a module and still holds
/// something: `<to>/.zeron-set-aside/<seconds>/<its path>`, never read.
pub const SET_ASIDE: &str = ".zeron-set-aside";

/// Under `dir` (in `to`), each folder (not a dot folder) none of `folders`:
/// removed if empty once its own are settled, moved to [`SET_ASIDE`] if it
/// still holds something and stands for one of `modules` (folded paths
/// without `.luau`), else left. How many it removed or moved.
fn settle_folders(to: &Path, dir: &Path, folders: &BTreeMap<String, &str>, modules: &BTreeSet<String>) -> std::io::Result<usize> {
    let mut changed = 0;
    let Ok(entries) = std::fs::read_dir(dir) else { return Ok(0) };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else { continue };
        if entry.file_name().to_string_lossy().starts_with('.') || !(kind.is_dir() || kind.is_symlink()) {
            continue;
        }
        let path = entry.path();
        let Ok(rel) = path.strip_prefix(to) else { continue };
        let rel = rel_path(rel);
        // A link: zgx follows it, so one by a module's name stands for that
        // module as a folder would. Moved aside (the link, never what it's
        // to); any other left. (One at a wanted file's path is replaced as
        // that file is laid.)
        if kind.is_symlink() {
            if modules.contains(&zeron_plugin_guard::path::fold(&rel)) {
                set_aside(to, &path, &rel, "a link")?;
                changed += 1;
            }
            continue;
        }
        changed += settle_folders(to, &path, folders, modules)?;
        // (Wanted: by its name, or under another spelling of one that is
        // the same folder where the disk folds names.)
        if folders.get(&zeron_plugin_guard::path::fold(&rel)).is_some_and(|w| *w == rel || same_file(&to.join(w), &path)) {
            continue;
        }
        if std::fs::remove_dir(&path).is_ok() {
            changed += 1;
        } else if modules.contains(&zeron_plugin_guard::path::fold(&rel)) {
            set_aside(to, &path, &rel, "it still held files")?;
            changed += 1;
        }
    }
    Ok(changed)
}

/// `path` (`rel` in `to`), which stood for the module `rel.luau`, moved
/// whole to [`SET_ASIDE`] and said in the log (`why`).
fn set_aside(to: &Path, path: &Path, rel: &str, why: &str) -> std::io::Result<()> {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let mut aside = to.join(SET_ASIDE).join(secs.to_string()).join(rel);
    for n in 1.. {
        if aside.symlink_metadata().is_err() {
            break;
        }
        aside = to.join(SET_ASIDE).join(format!("{secs}-{n}")).join(rel);
    }
    if let Some(parent) = aside.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(path, &aside)?;
    eprintln!("zeron-zgx: {rel} stood for the module {rel}.luau ({why}): moved to {}", aside.display());
    Ok(())
}

/// The most threads [`in_parallel`] uses: past this, laying files down
/// went no faster on Windows (Defender's own scanning was the limit).
const MAX_FILE_THREADS: usize = 8;

/// `work` for each of `jobs`, on up to [`MAX_FILE_THREADS`] threads, each
/// taking the next job as it finishes one: the first error, once every
/// thread has stopped (no new job is started after an error).
fn in_parallel<T: Sync>(jobs: &[T], work: impl Fn(&T) -> std::io::Result<()> + Sync) -> std::io::Result<()> {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(MAX_FILE_THREADS).min(jobs.len());
    if threads <= 1 {
        return jobs.iter().try_for_each(work);
    }
    let (next, failed, error) = (AtomicUsize::new(0), AtomicBool::new(false), std::sync::Mutex::new(None));
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                while !failed.load(Ordering::Relaxed) {
                    let Some(job) = jobs.get(next.fetch_add(1, Ordering::Relaxed)) else { break };
                    if let Err(e) = work(job) {
                        failed.store(true, Ordering::Relaxed);
                        error.lock().unwrap_or_else(|p| p.into_inner()).get_or_insert(e);
                    }
                }
            });
        }
    });
    match error.into_inner().unwrap_or_else(|p| p.into_inner()) {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// `.zeron`, as `ui/` has it now: the app's files, their hashes, the catalog
/// of screens (what each draws, the hooks it declares) and Zeron's manifest;
/// and the official templates beside it, as this build ships them
/// (`official::install`). How many of `.zeron`'s files changed.
pub fn sync_zeron(ui: &Path, templates: &Path) -> std::io::Result<usize> {
    let dir = templates.join(ZERON);
    let want = files(ui, of_the_app);
    // Each file's hash from the bytes the copy read (`ui/` read once).
    let hashes = std::sync::Mutex::new(BTreeMap::new());
    let changed = mirror_reading(&dir, &want, |rel| rel == "zeron-template.json", |rel, bytes| {
        hashes.lock().unwrap_or_else(|p| p.into_inner()).insert(rel.to_string(), hash(bytes));
    })?;
    let hashes: BTreeMap<String, String> = hashes.into_inner().unwrap_or_else(|p| p.into_inner());
    let json = |v: serde_json::Value| serde_json::to_vec_pretty(&v).unwrap_or_default();
    put(&dir.join(".hashes.json"), &json(serde_json::json!(hashes)))?;
    put(&dir.join(".catalog.json"), &json(super::templates::catalog(ui)))?;
    put(
        &dir.join("zeron-template.json"),
        &json(serde_json::json!({
            "id": BUILTIN,
            "name": super::official::DEFAULT_NAME,
            "description": super::official::DEFAULT_DESCRIPTION,
            "author": "Zeron",
            "version": ZERON_VERSION,
            "zeron_version": ZERON_VERSION,
        })),
    )?;
    super::official::install(ui, templates);
    Ok(changed)
}

/// The active template's id, from `.active` (Zeron's own if none, or if it's
/// gone or isn't a plain id), and Zeron's own whatever it says in safe mode
/// (`preview::safe`): no template re-selects itself from there. And Zeron's
/// own for one Zeron took down, or can't check (`blocks`).
pub fn active(templates: &Path) -> String {
    if super::preview::safe() {
        return BUILTIN.to_string();
    }
    let id = chosen(templates);
    if super::blocks::blocked(templates, &id).is_some() { BUILTIN.to_string() } else { id }
}

/// What `.active` names, safe mode or not (the Templates page shows it).
pub fn chosen(templates: &Path) -> String {
    let id = std::fs::read_to_string(templates.join(ACTIVE)).unwrap_or_default();
    let id = id.trim();
    let ok = !id.is_empty()
        && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && templates.join(id).join("zeron-template.json").is_file();
    if ok { id.to_string() } else { BUILTIN.to_string() }
}

pub fn set_active(templates: &Path, id: &str) -> std::io::Result<bool> {
    put(&templates.join(ACTIVE), id.as_bytes())
}

/// What `.live` holds: each file's source, and which came from the template.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Overlay {
    pub template: String,
    /// Files the template supplies (its own, or its version of Zeron's), by
    /// their paths in `.live`.
    pub from_template: BTreeSet<String>,
    /// A template file laid at another path in `.live` than its own (its
    /// `ds/tokens.luau`, as `ds/tokens_template.luau`): `.live`'s → its own.
    pub laid_as: BTreeMap<String, String>,
    /// The template's files left out for their path (`ZeronShape::clashes`).
    pub clashing: BTreeSet<String>,
    /// Its theme files laid at `template-*.json` (`.live`'s path → scheme),
    /// renamed and set over the palette as they're laid (`compose`).
    pub themes: BTreeMap<String, String>,
    /// Its theme files left out as they don't read as a theme (its own paths).
    pub unread_themes: BTreeSet<String>,
}

/// A template's theme file for `scheme` (`ds/themes/zeron-dark.json`), as a
/// disk that folds names has it: where Zeron lays it (`template-dark.json`).
pub fn template_theme(rel: &str) -> Option<(&'static str, String)> {
    let rel = zeron_plugin_guard::path::fold(rel);
    ["dark", "light"].into_iter().find(|kind| rel == zeron_plugin_guard::path::fold(&format!("ds/themes/zeron-{kind}.json"))).map(|kind| (kind, format!("ds/themes/template-{kind}.json")))
}

/// What's in a template's theme file `source`, as Zeron lays it: named
/// `template-{scheme}`, of that scheme, over the palette worn
/// (`palette-{scheme}`, ds/palettes `wear`), its tokens as they are. Read as
/// zgx reads a theme (comments and all), or why not.
pub fn laid_theme(source: &Path, scheme: &str) -> Result<Vec<u8>, String> {
    let text = std::fs::read_to_string(source).map_err(|e| e.to_string())?;
    let theme = zgx::theme::Theme::from_json(&text).map_err(|e| e.to_string())?;
    let kind = zgx::theme::ThemeKind::parse(scheme).ok_or("no such scheme")?;
    let mut out = zgx::theme::Theme::new(format!("template-{scheme}"), kind).extending(format!("palette-{scheme}"));
    for (token, expr) in theme.tokens() {
        out = out.with_expr(token, expr).map_err(|e| e.to_string())?;
    }
    Ok(out.to_json().into_bytes())
}

/// Where `compose` keeps a template's theme files as it lays them (`laid_theme`):
/// the host's own, beside the templates.
pub const LAID_THEMES: &str = ".laid-themes";

/// `.live` as `.zeron` with template `id` on top, leaving out the template's
/// `failed` files (Zeron's show in their place) and the paths never templated.
/// Zeron's own alone for `BUILTIN`, or `whole_failed`. How many files changed.
pub fn compose(templates: &Path, id: &str, failed: &BTreeSet<String>, whole_failed: bool) -> std::io::Result<(usize, Overlay)> {
    let (mut want, mut overlay) = plan(templates, id, failed, whole_failed);
    // Its theme files as Zeron lays them (renamed, over the palette); one
    // that doesn't read as a theme left out, Zeron's empty one in its place.
    for (laid, scheme) in overlay.themes.clone() {
        let Some(source) = want.get(&laid).cloned() else { continue };
        match laid_theme(&source, &scheme) {
            Ok(bytes) => {
                let at = templates.join(LAID_THEMES).join(format!("template-{scheme}.json"));
                if let Some(dir) = at.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                put(&at, &bytes)?;
                want.insert(laid, at);
            }
            Err(_) => {
                want.insert(laid.clone(), templates.join(ZERON).join(&laid));
                overlay.from_template.remove(&laid);
                overlay.themes.remove(&laid);
                if let Some(own) = overlay.laid_as.remove(&laid) {
                    overlay.unread_themes.insert(own);
                }
            }
        }
    }
    let changed = mirror(&templates.join(LIVE), &want, |_| false)?;
    Ok((changed, overlay))
}

/// What `compose` lays down, each file's source by its path in `.live`, and
/// the overlay that is: folders listed, nothing read.
pub fn plan(templates: &Path, id: &str, failed: &BTreeSet<String>, whole_failed: bool) -> (BTreeMap<String, PathBuf>, Overlay) {
    let mut want = files(&templates.join(ZERON), |rel| !template_meta(rel));
    let shape = ZeronShape::of(want.keys());
    let mut from_template = BTreeSet::new();
    let mut laid_as = BTreeMap::new();
    let mut clashing = BTreeSet::new();
    let mut themes = BTreeMap::new();
    if id != BUILTIN && !whole_failed {
        // Each file is written under one spelling: a template's `App.luau`
        // stands in for Zeron's `app.luau` (one file on a case-insensitive
        // filesystem), never both written, whichever sorts last winning.
        let mut spelled: BTreeMap<String, String> = want.keys().map(|rel| (zeron_plugin_guard::path::fold(rel), rel.clone())).collect();
        // Zeron's spelling of each of its folders and files: a template's
        // file is laid under them (`Screens/x.luau` as `screens/x.luau`), so
        // one spelling is written of each.
        let zeron_spelling: BTreeMap<String, String> = want
            .keys()
            .flat_map(|rel| rel.match_indices('/').map(|(at, _)| rel[..at].to_string()).chain([rel.clone()]))
            .map(|p| (zeron_plugin_guard::path::fold(&p), p))
            .collect();
        let respelled = |rel: &str| {
            let mut out = String::new();
            let mut theirs = true;
            for part in rel.split('/') {
                let next = if out.is_empty() { part.to_string() } else { format!("{out}/{part}") };
                out = match theirs.then(|| zeron_spelling.get(&zeron_plugin_guard::path::fold(&next))).flatten() {
                    Some(zeron_s) => zeron_s.clone(),
                    None => {
                        theirs = false;
                        next
                    }
                };
            }
            out
        };
        for (rel, path) in files(&templates.join(id), |_| true) {
            if !composed(&rel) || failed.contains(&rel) || laid_by_zeron(&rel) {
                continue;
            }
            if shape.clashes(&rel) {
                clashing.insert(rel);
                continue;
            }
            // Its theme files beside Zeron's, never over them: laid as
            // template-*.json, over the palette worn and under one you chose
            // (`compose` renames them), so a token it doesn't name is Zeron's.
            if let Some((scheme, laid)) = template_theme(&rel) {
                from_template.insert(laid.clone());
                laid_as.insert(laid.clone(), rel);
                themes.insert(laid.clone(), scheme.to_string());
                want.insert(laid, path);
                continue;
            }
            // Its tokens beside Zeron's, never over them: Zeron's run first
            // and then its (ds/tokens.luau), so a token it doesn't name is
            // this Zeron's, the ones added since it was made too.
            if zeron_plugin_guard::path::fold(&rel) == zeron_plugin_guard::path::fold(TOKENS) {
                from_template.insert(TEMPLATE_TOKENS.to_string());
                laid_as.insert(TEMPLATE_TOKENS.to_string(), rel);
                want.insert(TEMPLATE_TOKENS.to_string(), path);
                continue;
            }
            let laid = respelled(&rel);
            if let Some(zeron_s) = spelled.insert(zeron_plugin_guard::path::fold(&laid), laid.clone())
                && zeron_s != laid
            {
                want.remove(&zeron_s);
                from_template.remove(&zeron_s);
            }
            if laid != rel {
                laid_as.insert(laid.clone(), rel);
            }
            from_template.insert(laid.clone());
            want.insert(laid, path);
        }
    }
    (want, Overlay { template: id.to_string(), from_template, laid_as, clashing, themes, unread_themes: BTreeSet::new() })
}

/// Beside the templates: what a launch last laid `.live` down from, and
/// `.live` as that left it (`LaunchStamp`).
pub const LAUNCH_STAMP: &str = ".launch-stamp.json";

/// What a launch lays `.live` down from, as it stands (each file's path,
/// size and time written; nothing read): this Zeron, `ui/`, the active
/// template, and `.live` itself. A launch that finds the one the last left
/// needs neither `sync_zeron` nor `compose`, which read every file of
/// Zeron's (twice) to write none: a quarter of a second of a warm launch
/// spent before the window, more from a cold disk. Anything changed since
/// (an update, a template edited or switched, `.live` written by anyone)
/// stamps differently, and the launch lays everything down as before.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchStamp {
    pub zeron: String,
    pub ui: String,
    pub id: String,
    pub template: String,
    pub live: String,
}

impl LaunchStamp {
    /// `ui/` and template `id` under `templates` as they stand now.
    pub fn now(ui: &Path, templates: &Path, id: &str) -> Self {
        Self {
            zeron: ZERON_VERSION.to_string(),
            ui: ui_stamp(ui),
            id: id.to_string(),
            template: if id == BUILTIN { String::new() } else { template_stamp(&templates.join(id)) },
            live: listed_stamp(&templates.join(LIVE), |_| true),
        }
    }

    /// The one the last launch left, if any.
    pub fn read(templates: &Path) -> Option<Self> {
        let v = read_json(&templates.join(LAUNCH_STAMP));
        let field = |k: &str| v[k].as_str().map(str::to_string);
        Some(Self { zeron: field("zeron")?, ui: field("ui")?, id: field("id")?, template: field("template")?, live: field("live")? })
    }

    pub fn write(&self, templates: &Path) -> std::io::Result<bool> {
        let v = serde_json::json!({
            "zeron": self.zeron, "ui": self.ui, "id": self.id, "template": self.template, "live": self.live,
        });
        put(&templates.join(LAUNCH_STAMP), &serde_json::to_vec_pretty(&v).unwrap_or_default())
    }

    /// Whether `.live` can run as it is: laid from just this, and whole
    /// (Zeron's copy and its hashes there, the official templates too).
    pub fn still_laid(&self, ui: &Path, templates: &Path) -> bool {
        Self::read(templates).as_ref() == Some(self)
            && templates.join(LIVE).join("app.luau").is_file()
            && templates.join(ZERON).join(".hashes.json").is_file()
            && !super::official::missing(ui, templates)
    }
}

/// The files of `.live` an error names, first mention first: each
/// `<path>.luau:<line>`, its path from the app's folder (as zgx names the
/// app's modules) or through `.live` (as given, or as the system resolves
/// it: macOS's `/var/…` is `/private/var/…` to scripts). The VM's own name
/// before "runtime error" is who ran it, not where it failed, and is skipped.
///
/// Windows names them with `\` (`screens\card.luau:3`, `C:\…\.live\…`):
/// read as `/`, the way `files` names a template's files, so a template
/// file that breaks there falls back like anywhere. (A token stops at a `:`,
/// so `.live`'s spellings are matched without their drive, or `\\?\`.)
pub fn named(error: &str, templates: &Path) -> Vec<String> {
    let live = templates.join(LIVE);
    let spelt = |path: &Path| {
        let text = path.to_string_lossy().replace('\\', "/");
        let text = text.strip_prefix("//?/").unwrap_or(&text);
        let text = match text.as_bytes() {
            [drive, b':', ..] if drive.is_ascii_alphabetic() => &text[2..],
            _ => text,
        };
        format!("{text}/")
    };
    let mut spellings = vec![spelt(&live)];
    if let Ok(real) = std::fs::canonicalize(&live) {
        spellings.push(spelt(&real));
    }
    let body = match error.find(": runtime error: ") {
        Some(at) => &error[at + ": runtime error: ".len()..],
        None => error,
    };
    let is_path = |c: char| c.is_ascii_alphanumeric() || matches!(c, '/' | '\\' | '.' | '_' | '-' | '@');
    let mut out: Vec<String> = Vec::new();
    let mut from = 0;
    while let Some(at) = body[from..].find(".luau:") {
        let end = from + at + ".luau".len();
        let start = body[..end].rfind(|c: char| !is_path(c)).map(|i| i + 1).unwrap_or(0);
        let token = body[start..end].replace('\\', "/");
        let rel = spellings
            .iter()
            .find_map(|s| token.find(s.as_str()).map(|i| &token[i + s.len()..]))
            .unwrap_or(&token);
        if plain(rel) && live.join(rel).is_file() && !out.iter().any(|r| r == rel) {
            out.push(rel.to_string());
        }
        from = end;
    }
    out
}

/// Whom an error is on: the first file it names that the template supplies
/// (a template file required by Zeron's, or Zeron's called from the
/// template's), else none (Zeron's own error). By the template's own path
/// (its tokens' `ds/tokens.luau`, though `.live` has them beside Zeron's).
pub fn blame(error: &str, templates: &Path, overlay: &Overlay) -> Option<String> {
    let rel = named(error, templates).into_iter().find(|rel| overlay.from_template.contains(rel))?;
    Some(overlay.laid_as.get(&rel).cloned().unwrap_or(rel))
}

/// A file as it stands, without reading it: its size and the time it was
/// last written. A save changes it, so a failure recorded against it is
/// tried again once the file is edited.
pub fn file_stamp(path: &Path) -> String {
    std::fs::metadata(path).map(|meta| stamp_of(&meta)).unwrap_or_default()
}

fn stamp_of(meta: &std::fs::Metadata) -> String {
    let at = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).unwrap_or_default();
    format!("{}:{}", meta.len(), at.as_nanos())
}

/// Everything in `dir` that `keep` keeps, as it stands (each file's path,
/// size and time written; nothing read): a template or `ui/` edited reads
/// as new. A stat each, where reading the bytes was most of a look's time.
pub fn stamp(dir: &Path, keep: impl Fn(&str) -> bool) -> String {
    stamped(dir, keep, |_, path| file_stamp(path))
}

/// [`stamp`], each file's size and time as its folder lists them: for the
/// folders only Zeron's installer and this app write (`ui/`, `.live`), whose
/// files are replaced whole. On Windows, asking each file opened and closed
/// it (40 µs a file): an eighth of a second of every launch, for `ui/` and
/// `.live`'s thousand files each. (NTFS can list a file edited through
/// another of its links with its old size and time: so not for templates,
/// which anyone edits.)
pub fn listed_stamp(dir: &Path, keep: impl Fn(&str) -> bool) -> String {
    stamped(dir, keep, |entry, path| entry.metadata().map(|meta| stamp_of(&meta)).unwrap_or_else(|_| file_stamp(path)))
}

fn stamped(dir: &Path, keep: impl Fn(&str) -> bool, stamp: impl Fn(&std::fs::DirEntry, &Path) -> String) -> String {
    let mut stamps = BTreeMap::new();
    walk(dir, keep, |rel, path, entry| {
        stamps.insert(rel, stamp(entry, &path));
    });
    let mut h = DefaultHasher::new();
    for (rel, stamp) in stamps {
        rel.hash(&mut h);
        stamp.hash(&mut h);
    }
    format!("{:016x}", h.finish())
}

/// A template's folder as it stands: what a failure of the whole template
/// is recorded against.
pub fn template_stamp(dir: &Path) -> String {
    stamp(dir, |_| true)
}

/// `ui/`'s app files (and the official templates') as they stand:
/// unchanged, `.zeron` needn't be copied again.
pub fn ui_stamp(ui: &Path) -> String {
    listed_stamp(ui, synced)
}

/// What of `ui/` a sync copies: the app's files, and the official templates'.
fn synced(rel: &str) -> bool {
    of_the_app(rel) || super::official::source_file(rel)
}

/// What a look found, off the UI thread: `.zeron` brought up to date (only
/// when `ui/` changed since the last look), the active template, and its
/// files as they stand.
#[derive(Debug)]
pub struct Survey {
    pub ui_stamp: String,
    /// `.zeron` copied again: how many files changed (none when `ui/` hadn't),
    /// or why it failed.
    pub synced: Result<usize, String>,
    pub id: String,
    pub template_stamp: String,
    /// Each of the active template's files, by its path, as it stands.
    pub files: Vec<(String, String)>,
}

pub fn survey(ui: &Path, templates: &Path, last_ui_stamp: Option<&str>) -> Survey {
    let ui_stamp = ui_stamp(ui);
    // (An official template gone, deleted by hand: put back, as `.zeron` is.)
    let current =
        last_ui_stamp == Some(ui_stamp.as_str()) && templates.join(ZERON).join(".hashes.json").is_file() && !super::official::missing(ui, templates);
    let synced = if current { Ok(0) } else { sync_zeron(ui, templates).map_err(|e| e.to_string()) };
    let id = active(templates);
    let (template_stamp, files_) = if id == BUILTIN {
        (String::new(), Vec::new())
    } else {
        let dir = templates.join(&id);
        let list = files(&dir, |_| true).into_iter().map(|(rel, path)| (rel, file_stamp(&path))).collect();
        (template_stamp(&dir), list)
    };
    Survey { ui_stamp, synced, id, template_stamp, files: files_ }
}

/// What `.live` was last laid down from: the same again needs no compose
/// (which reads every file of Zeron's and the template's).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Laid {
    pub ui_stamp: String,
    pub id: String,
    pub template_stamp: String,
    pub whole_failed: bool,
    pub failed: BTreeSet<String>,
}

/// What to watch so a change reaches the window: the templates folder
/// (`.active`, a template made or removed), the active template's files and
/// folders, and Zeron's own `ui/` (a save while developing).
///
/// `ui/` (over a thousand files, most of them art) is watched by its
/// folders, which hear a file made, removed or saved by replacing it, plus
/// its scripts and themes, the files saved in place. The gallery's and the
/// tests' folders aren't the app's and aren't watched.
pub fn watched(templates: &Path, ui: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut files_ = vec![templates.join(ACTIVE)];
    let mut dirs = vec![templates.to_path_buf()];
    let id = active(templates);
    let edited_in_place = |rel: &str| rel.ends_with(".luau") || rel.ends_with(".json") || rel == ".luaurc";
    // (A bundle's interface doesn't change while it runs: it's read again at
    // the next launch.)
    if !in_app_bundle(ui) {
        files_.extend(files(ui, |rel| synced(rel) && edited_in_place(rel)).into_values());
        folders(ui, &mut dirs, synced);
    }
    if id != BUILTIN {
        let dir = templates.join(&id);
        files_.extend(files(&dir, |_| true).into_values());
        folders(&dir, &mut dirs, |_| true);
    }
    (files_, dirs)
}

/// Whether `ui` is what a release shipped: a macOS app bundle's, or this
/// installed version's own folder (Windows' `beta\<version>\ui`, which an
/// update never changes: it installs the next version beside it).
fn in_app_bundle(ui: &Path) -> bool {
    ui.ends_with("Contents/Resources/ui") || crate::bundled_ui().as_deref() == Some(ui)
}

/// `root` and every folder under it that `keep` keeps (by its path from
/// `root`), but dot folders.
fn folders(root: &Path, out: &mut Vec<PathBuf>, keep: impl Fn(&str) -> bool) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(at) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&at) else { continue };
        out.push(at);
        for entry in entries.flatten() {
            let hidden = entry.file_name().to_string_lossy().starts_with('.');
            if hidden || !entry.file_type().is_ok_and(|k| k.is_dir()) {
                continue;
            }
            let path = entry.path();
            let rel = path.strip_prefix(root).map(|r| r.to_string_lossy().replace('\\', "/")).unwrap_or_default();
            if keep(&rel) {
                stack.push(path);
            }
        }
    }
}

/// A template, as its manifest and files say.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Installed {
    pub id: String,
    pub name: String,
    pub description: String,
    pub author: String,
    pub version: String,
    /// The Zeron it was made from (its manifest's `zeron_version`).
    pub zeron_version: String,
    /// It was made from another version of Zeron than this one (its
    /// manifest's `zeron_version` against `.zeron`'s): only a real update
    /// says so, never a save while developing. Which files changed is
    /// `template_status`'s (`.base.json` against `.zeron/.hashes.json`).
    pub out_of_date: bool,
    /// Installed from someone's package: `.imported.json`'s
    /// `{key, author}` (the engine's `template_packages`).
    pub imported: Option<serde_json::Value>,
    /// One of Zeron's official templates, exactly as this build wrote it
    /// (`official::verified`): built in, as the default is.
    pub official: Option<super::official::Official>,
}

/// Whether a template made for Zeron `made_for` is out of date in Zeron
/// `now`: both known, and not the same. One with no version recorded (written
/// by hand) never is, nor one stamped "0.0.0" (what development builds
/// stamped before Zeron's version reached them: no real Zeron's).
pub fn version_out_of_date(made_for: &str, now: &str) -> bool {
    let known = |v: &str| !v.is_empty() && v != "0.0.0";
    let (made_for, now) = (made_for.trim(), now.trim());
    known(made_for) && known(now) && made_for != now
}

fn read_json(path: &Path) -> serde_json::Value {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// The default first, then the official templates in Zeron's order, then
/// every other template, by name.
pub fn installed(templates: &Path) -> Vec<Installed> {
    let zeron = read_json(&templates.join(ZERON).join("zeron-template.json"));
    let now = zeron["zeron_version"].as_str().unwrap_or("").to_string();
    let manifest = |dir: &Path, id: &str| -> Option<Installed> {
        let m = read_json(&dir.join("zeron-template.json"));
        if !m.is_object() {
            return None;
        }
        let text = |k: &str| m[k].as_str().unwrap_or("").to_string();
        let out_of_date = id != BUILTIN && version_out_of_date(&text("zeron_version"), &now);
        Some(Installed {
            id: id.to_string(),
            name: if text("name").is_empty() { id.to_string() } else { text("name") },
            description: text("description"),
            author: text("author"),
            version: text("version"),
            zeron_version: text("zeron_version"),
            out_of_date,
            imported: (id != BUILTIN)
                .then(|| read_json(&dir.join(".imported.json")))
                // (Known by its record, never by its key: one published with
                // a deploy token has none.)
                .filter(|v| v.is_object()),
            official: (id != BUILTIN).then(|| super::official::verified(dir)).flatten(),
        })
    };
    let mut out: Vec<Installed> = manifest(&templates.join(ZERON), BUILTIN).into_iter().collect();
    let mut rest: Vec<Installed> = std::fs::read_dir(templates)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|k| k.is_dir()))
        .filter_map(|e| {
            let id = e.file_name().to_string_lossy().into_owned();
            (!id.starts_with('.') && id != BUILTIN && plain(&id)).then(|| manifest(&e.path(), &id)).flatten()
        })
        .collect();
    rest.sort_by_key(|t| (super::official::order(&t.id, t.official), t.name.to_lowercase()));
    out.extend(rest);
    out
}

/// Packages waiting for you to allow them (the engine keeps each in
/// `.pending/<name>.json`, beside the package), oldest first.
pub fn pending(templates: &Path) -> Vec<serde_json::Value> {
    let dir = templates.join(".pending");
    let mut out: Vec<serde_json::Value> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let file = e.file_name().to_string_lossy().into_owned();
            let name = file.strip_suffix(".json")?.to_string();
            if !plain(&name) || !dir.join(format!("{name}.zeron-template")).is_file() {
                return None;
            }
            let mut about = read_json(&e.path());
            // Where it came from, as the window may show it (never a local
            // path): `from`.
            if let Some(origin) = about.as_object_mut().and_then(|a| a.remove("origin"))
                && let Some(kind) = origin["kind"].as_str()
            {
                let mut from = serde_json::json!({ "kind": kind });
                for key in ["url", "ref"] {
                    if let Some(v) = origin[key].as_str() {
                        from[key] = serde_json::json!(v);
                    }
                }
                about["from"] = from;
            }
            (about["pending"].as_str() == Some(name.as_str())).then_some(about)
        })
        .collect();
    out.sort_by_key(|v| (v["at"].as_i64().unwrap_or(0), v["pending"].as_str().unwrap_or("").to_string()));
    out
}

/// A new template's id from the name it's given: lowercase letters, digits
/// and dashes (a template id), never Zeron's, and free (`<slug>-2`…).
pub fn new_id(templates: &Path, name: &str) -> String {
    let mut slug = String::new();
    for c in name.trim().chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let mut slug: String = slug.trim_end_matches('-').chars().take(30).collect();
    slug = slug.trim_end_matches('-').to_string();
    if slug.len() < 2 || slug == BUILTIN {
        slug = format!("template-{slug}").trim_end_matches('-').to_string();
    }
    let mut id = slug.clone();
    let mut n = 2;
    while id == BUILTIN || templates.join(&id).exists() {
        id = format!("{slug}-{n}");
        n += 1;
    }
    id
}

/// A duplicate's id and name: `<id>-copy`, `<id>-copy-2`… and "<name> copy".
pub fn copy_name(templates: &Path, from: &str) -> (String, String) {
    let name = installed(templates).into_iter().find(|t| t.id == from).map(|t| t.name).unwrap_or_else(|| from.to_string());
    let stem: String = from.chars().take(30).collect();
    let mut id = format!("{stem}-copy");
    let mut n = 2;
    while templates.join(&id).exists() {
        id = format!("{stem}-copy-{n}");
        n += 1;
    }
    (id, format!("{name} copy"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zgx-live-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &Path, rel: &str, text: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn read(dir: &Path, rel: &str) -> String {
        std::fs::read_to_string(dir.join(rel)).unwrap_or_default()
    }

    fn mtime(path: &Path) -> SystemTime {
        std::fs::metadata(path).unwrap().modified().unwrap()
    }

    /// The files an error names, however the platform spells them: `/`, or
    /// Windows' `\` (as zgx names the app's modules there, and in full
    /// paths through `.live`); what isn't in `.live`, and the VM that ran
    /// it, aren't.
    #[test]
    fn an_error_names_its_files_with_either_separator() {
        let templates = dir("named");
        let live = templates.join(LIVE);
        write(&live, "screens/card.luau", "return 1");
        write(&live, "app.luau", "return 1");
        let names = |error: &str| named(error, &templates);
        assert_eq!(names("app.luau: runtime error: screens/card.luau:3: broke"), vec!["screens/card.luau"]);
        assert_eq!(
            names("app.luau: runtime error: screens\\card.luau:3: broke later\nstack traceback:\n\t[C]: in function 'error'\n\tscreens\\card.luau:3: in function <screens\\card.luau:3>"),
            vec!["screens/card.luau"],
            "Windows' own spelling, named once"
        );
        let full = format!("{}\\screens\\card.luau", live.to_string_lossy().replace('/', "\\"));
        assert_eq!(names(&format!("app.luau: runtime error: {full}:3: broke")), vec!["screens/card.luau"], "through .live, with `\\`");
        assert_eq!(names("app.luau: runtime error: screens\\other.luau:1: not the template's"), Vec::<String>::new());
        assert_eq!(names("screens\\card.luau: runtime error: elsewhere"), Vec::<String>::new(), "the VM that ran it");
        assert_eq!(
            names("app.luau: runtime error: screens\\other.luau:1: from\n\tscreens\\card.luau:3: in function"),
            vec!["screens/card.luau"]
        );
        let _ = std::fs::remove_dir_all(&templates);
    }

    /// A mirror laid down on several threads is the one laid down one file
    /// at a time: every file's bytes, what changed counted once, the ones it
    /// doesn't want removed, and laying it again changes (or rewrites)
    /// nothing.
    #[test]
    fn a_mirror_on_several_threads_lays_every_file_once() {
        let root = dir("mirror-threads");
        let (from, to) = (root.join("from"), root.join("to"));
        for i in 0..300 {
            write(&from, &format!("screens/s{}/f{i}.luau", i % 7), &format!("-- file {i}"));
        }
        write(&to, "screens/stale.luau", "-- not wanted");
        write(&to, "screens/s0/f0.luau", "-- file 0");
        let want = files(&from, |_| true);
        assert_eq!(mirror(&to, &want, |_| false).unwrap(), 299 + 1, "every new file and the stale one, not the one already there");
        for (rel, path) in &want {
            assert_eq!(read(&to, rel), std::fs::read_to_string(path).unwrap(), "{rel}");
        }
        assert!(!to.join("screens/stale.luau").exists());
        let at = mtime(&to.join("screens/s3/f10.luau"));
        assert_eq!(mirror(&to, &want, |_| false).unwrap(), 0);
        assert_eq!(mtime(&to.join("screens/s3/f10.luau")), at, "nothing rewritten");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Several threads stop at the first error, and say it.
    #[test]
    fn work_on_several_threads_says_its_first_error() {
        let ran = std::sync::atomic::AtomicUsize::new(0);
        let jobs: Vec<usize> = (0..64).collect();
        let result = in_parallel(&jobs, |&job| {
            ran.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if job == 5 { Err(std::io::Error::other("job 5")) } else { Ok(()) }
        });
        assert_eq!(result.unwrap_err().to_string(), "job 5");
        assert!(ran.into_inner() <= jobs.len());
        let done = std::sync::atomic::AtomicUsize::new(0);
        in_parallel(&jobs, |_| {
            done.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Ok(())
        })
        .unwrap();
        assert_eq!(done.into_inner(), jobs.len(), "all of them, once");
    }

    /// `.zeron`'s hashes, taken from the bytes its copy read, are each file's.
    #[test]
    fn zeron_s_hashes_are_its_files() {
        let root = dir("hashes");
        let (ui, t) = (root.join("ui"), root.join("templates"));
        write(&ui, "app.luau", "-- app");
        write(&ui, "screens/shell/sidebar.luau", "-- sidebar");
        sync_zeron(&ui, &t).unwrap();
        let hashes = read_json(&t.join(ZERON).join(".hashes.json"));
        assert_eq!(hashes["app.luau"], hash(b"-- app"));
        assert_eq!(hashes["screens/shell/sidebar.luau"], hash(b"-- sidebar"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A folder stamped as its listing says is stamped as asking each file
    /// says (the launch stamps written before read the same), and a file
    /// replaced restamps it.
    #[test]
    fn a_listed_stamp_is_the_files_own() {
        let root = dir("listed-stamp");
        write(&root, "app.luau", "-- app");
        write(&root, "screens/a.luau", "-- a");
        write(&root, ".hidden/x.luau", "-- not stamped");
        assert_eq!(listed_stamp(&root, |_| true), stamp(&root, |_| true));
        let before = listed_stamp(&root, |_| true);
        put(&root.join("screens/a.luau"), b"-- a, changed").unwrap();
        assert_ne!(listed_stamp(&root, |_| true), before);
        assert_eq!(listed_stamp(&root, |_| true), stamp(&root, |_| true));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A templates folder with a small Zeron and a template `t`.
    fn setup(name: &str) -> PathBuf {
        let root = dir(name);
        let t = root.join("templates");
        let z = t.join(ZERON);
        write(&z, "app.luau", "-- zeron app");
        write(&z, "screens/shell/sidebar.luau", "-- zeron sidebar");
        write(&z, "screens/plugins/page.luau", "-- zeron plugins page");
        write(&z, "ds/tokens.luau", "-- zeron tokens");
        write(&z, ".luaurc", "{}");
        write(&z, "zeron-template.json", "{}");
        write(&t, "t/zeron-template.json", r#"{"id":"t"}"#);
        write(&t, "t/screens/shell/sidebar.luau", "-- t sidebar");
        write(&t, "t/screens/plugins/page.luau", "-- t's plugins page, never used");
        write(&t, "t/screens/extra.luau", "-- t only");
        write(&t, "t/.luaurc", r#"{"aliases":{"evil":"/"}}"#);
        t
    }

    #[test]
    fn a_sparse_template_inherits_and_never_replaces_what_s_kept() {
        let t = setup("inherit");
        let (_, overlay) = compose(&t, "t", &BTreeSet::new(), false).unwrap();
        let live = t.join(LIVE);
        assert_eq!(read(&live, "screens/shell/sidebar.luau"), "-- t sidebar");
        assert_eq!(read(&live, "app.luau"), "-- zeron app", "inherited");
        assert_eq!(read(&live, "ds/tokens.luau"), "-- zeron tokens", "inherited");
        assert_eq!(read(&live, "screens/extra.luau"), "-- t only");
        assert_eq!(read(&live, "screens/plugins/page.luau"), "-- zeron plugins page", "never templated");
        assert_eq!(read(&live, ".luaurc"), "{}", "never templated");
        assert!(!live.join("zeron-template.json").exists());
        assert_eq!(
            overlay.from_template,
            BTreeSet::from(["screens/extra.luau".to_string(), "screens/shell/sidebar.luau".to_string()])
        );
    }

    /// The engine installs a change to a template's listing without asking,
    /// so nothing of it is ever laid down, kept by digest or counted. Anything
    /// else under `screenshots/`, however it's spelled, is code a change to
    /// asks about, there as here, and is never laid down either.
    #[test]
    fn a_template_s_listing_is_never_loaded() {
        use super::super::preview::digest;
        let t = setup("listing");
        let dir = t.join("t");
        let listing = ["README.md", "CHANGELOG.md", "preview.png", "screenshots/shot.png", "screenshots/shot.JPG", "screenshots/deep/shot.jpeg"];
        let before = digest(&dir);
        for rel in listing {
            assert!(template_meta(rel) && !zeron_plugin_guard::template::stray_screenshot(rel), "{rel}");
            write(&dir, rel, "return 1");
        }
        assert_eq!(digest(&dir), before, "no part of what's kept");
        let strays = ["screenshots/app.luau", "screenshots/deep/run.luau", "screenshots/png", "screenshots", "Screenshots/shot.png", "SCREENSHOTS/app.luau", "\u{17f}creenshots/app.luau", "screenshots./app.luau"];
        for rel in strays {
            assert!(!template_meta(rel) && zeron_plugin_guard::template::stray_screenshot(rel), "{rel}");
        }
        write(&dir, "screenshots/app.luau", "return 1");
        write(&dir, "screenshots/deep/run.luau", "return 1");
        assert_ne!(digest(&dir), before, "a stray is code");
        let (_, overlay) = compose(&t, "t", &BTreeSet::new(), false).unwrap();
        for rel in listing.iter().chain(&["screenshots/app.luau", "screenshots/deep/run.luau"]) {
            assert!(!t.join(LIVE).join(rel).exists(), "{rel}");
            assert!(!overlay.from_template.contains(*rel), "{rel}");
        }
        for rel in ["screenshots.luau", "screens/screenshots/app.luau", "changelog.md", "docs/CHANGELOG.md"] {
            assert!(!template_meta(rel) && !zeron_plugin_guard::template::stray_screenshot(rel), "{rel}");
        }
        write(&dir, "screenshots.luau", "return 2");
        compose(&t, "t", &BTreeSet::new(), false).unwrap();
        assert!(t.join(LIVE).join("screenshots.luau").is_file());
    }

    /// A stray under `screenshots/` added to a kept template asks, and is
    /// never laid down; a picture or a changelog added asks nothing.
    #[test]
    fn a_stray_added_to_a_kept_template_asks_and_is_never_laid_down() {
        use super::super::preview::{approve_at, approved_at, digest, trusted_at};
        let t = setup("stray");
        let (dir, approvals) = (t.join("t"), t.join("approvals.json"));
        let kept = digest(&dir);
        approve_at(&approvals, "t", &kept).unwrap();
        for rel in ["screenshots/a.png", "screenshots/b.JPEG", "CHANGELOG.md", "README.md", "preview.png"] {
            write(&dir, rel, "x");
            assert!(trusted_at(&t, "t", &approvals), "{rel}");
        }
        for rel in ["screenshots/x.luau", "screenshots/a.svg", "Screenshots/y.luau", "\u{17f}creenshots/z.luau"] {
            write(&dir, rel, "return 1");
            assert!(!trusted_at(&t, "t", &approvals), "{rel} asks");
            assert!(approved_at(&approvals, "t", &kept), "{rel}: the record stays");
            compose(&t, "t", &BTreeSet::new(), false).unwrap();
            assert!(!t.join(LIVE).join(rel).exists(), "{rel}");
            std::fs::remove_file(dir.join(rel)).unwrap();
            assert!(trusted_at(&t, "t", &approvals), "{rel} gone");
        }
    }

    /// A path a disk could open as another file (Windows by its short name,
    /// HFS+ by leaving out what isn't seen) is never laid down, on any system. A template kept with one runs without it and
    /// isn't asked about again; one byte of that file different asks, as ever.
    #[test]
    #[cfg(unix)]
    fn a_backslash_in_a_name_is_never_a_folder_s() {
        let (ui, t) = (crate::ui_dir(), std::env::temp_dir().join(format!("zgx-backslash-{}", std::process::id())));
        let _ = std::fs::remove_dir_all(&t);
        sync_zeron(&ui, &t).unwrap();
        let dir = t.join("odd");
        std::fs::create_dir_all(dir.join("screens")).unwrap();
        std::fs::write(dir.join("zeron-template.json"), r#"{"id":"odd","name":"Odd"}"#).unwrap();
        std::fs::write(dir.join("screens/extra.luau"), "-- t only").unwrap();
        std::fs::write(dir.join("screens\\shell"), "-- not Zeron's").unwrap();
        std::fs::write(dir.join("a:b.luau"), "-- not a package's name").unwrap();
        assert_eq!(files(&dir, |_| true).keys().cloned().collect::<Vec<_>>(), ["screens/extra.luau", "zeron-template.json"]);
        let before = super::super::preview::digest(&dir);
        std::fs::write(dir.join("screens\\shell"), "-- changed").unwrap();
        assert_eq!(super::super::preview::digest(&dir), before, "nor counted");
        let (want, overlay) = plan(&t, "odd", &BTreeSet::new(), false);
        assert!(!want.keys().any(|rel| rel.contains('\\') || rel.contains(':')), "{:?}", overlay.from_template);
        assert!(overlay.from_template.contains("screens/extra.luau"));
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn a_windows_short_name_is_never_laid_down() {
        use super::super::preview::{approve_at, digest, trusted_at};
        let t = setup("short");
        let (dir, approvals) = (t.join("t"), t.join("approvals.json"));
        let short = ["screens/TEMPLA~1/page.luau", "SCREEN~2/plugins/page.luau", "LUAURC~1", "account~0/x.luau", "screens/extra~1.luau", "screens/a~12b.luau", "screens/tem\u{200c}plates/page.luau"];
        for rel in short {
            write(&dir, rel, "-- short");
            assert!(zeron_plugin_guard::template::short_name(rel) || zeron_plugin_guard::template::unseen(rel), "{rel}");
        }
        // Kept with them, as an earlier Zeron did (its digest counts them).
        approve_at(&approvals, "t", &digest(&dir)).unwrap();
        let (_, overlay) = compose(&t, "t", &BTreeSet::new(), false).unwrap();
        for rel in short {
            assert!(!t.join(LIVE).join(rel).exists() && !overlay.from_template.contains(rel), "{rel}");
        }
        assert_eq!(read(&t.join(LIVE), "screens/extra.luau"), "-- t only", "the rest runs");
        assert!(trusted_at(&t, "t", &approvals), "not asked again");
        write(&dir, short[0], "-- shorT");
        assert!(!trusted_at(&t, "t", &approvals));
        for rel in ["screens/a~b.luau", "screens/a~.luau", "screens/~.luau", "screens/a1~.luau"] {
            assert!(!zeron_plugin_guard::template::short_name(rel), "{rel}");
        }
    }

    /// R30: a protected path spelled as a case-insensitive filesystem reads
    /// it the same (`Screens/`, `ſcreens/` with U+017F, a trailing dot) is
    /// still Zeron's, and never laid down over Zeron's own.
    #[test]
    fn a_protected_path_in_another_spelling_is_still_zeron_s() {
        for rel in ["Screens/Plugins/page.luau", "\u{17f}creens/plugins/page.luau", "screens/plugins./page.luau", "ACCOUNT/x.luau", ".LUAURC", "screens\\templates\\page.luau"] {
            assert!(never_templated(rel), "{rel}");
        }
        for rel in ["screens/pluginsx/page.luau", "screens/shell/sidebar.luau", "accounts.luau"] {
            assert!(!never_templated(rel), "{rel}");
        }
        let t = setup("folded");
        write(&t, "t/\u{17f}creens/plugins/page.luau", "-- the template's plugins page");
        write(&t, "t/App.luau", "-- the template's app");
        let (_, overlay) = compose(&t, "t", &BTreeSet::new(), false).unwrap();
        let live = t.join(LIVE);
        assert_eq!(read(&live, "screens/plugins/page.luau"), "-- zeron plugins page");
        assert!(!overlay.from_template.iter().any(|r| r.contains("plugins")), "{:?}", overlay.from_template);
        // One spelling of a file written: Zeron's, with the template's in it
        // (blamed by the template's own name).
        assert!(overlay.from_template.contains("app.luau") && overlay.laid_as["app.luau"] == "App.luau", "{overlay:?}");
        let names: Vec<String> = files(&live, |_| true).into_keys().filter(|r| r.eq_ignore_ascii_case("app.luau")).collect();
        assert_eq!(names.len(), 1, "{names:?}");
        assert_eq!(read(&live, &names[0]), "-- the template's app");
    }

    #[test]
    fn switching_touches_only_the_files_that_differ() {
        let t = setup("touch");
        compose(&t, BUILTIN, &BTreeSet::new(), false).unwrap();
        let live = t.join(LIVE);
        let tokens = mtime(&live.join("ds/tokens.luau"));
        std::thread::sleep(std::time::Duration::from_millis(20));
        let (changed, _) = compose(&t, "t", &BTreeSet::new(), false).unwrap();
        assert_eq!(changed, 2, "the sidebar rewritten, extra added");
        assert_eq!(mtime(&live.join("ds/tokens.luau")), tokens, "an unchanged file isn't touched");
        let (again, _) = compose(&t, "t", &BTreeSet::new(), false).unwrap();
        assert_eq!(again, 0, "nothing differs, nothing written");
        // Back to Zeron: the template's file goes, its version reverts.
        let (back, overlay) = compose(&t, BUILTIN, &BTreeSet::new(), false).unwrap();
        assert_eq!(back, 2);
        assert!(!live.join("screens/extra.luau").exists());
        assert!(overlay.from_template.is_empty());
    }

    #[test]
    fn a_failed_file_shows_zeron_s_and_a_failed_whole_shows_only_zeron() {
        let t = setup("failed");
        let failed = BTreeSet::from(["screens/shell/sidebar.luau".to_string()]);
        let (_, overlay) = compose(&t, "t", &failed, false).unwrap();
        assert_eq!(read(&t.join(LIVE), "screens/shell/sidebar.luau"), "-- zeron sidebar");
        assert_eq!(read(&t.join(LIVE), "screens/extra.luau"), "-- t only", "the rest stays the template's");
        assert!(!overlay.from_template.contains("screens/shell/sidebar.luau"));
        let (_, whole) = compose(&t, "t", &BTreeSet::new(), true).unwrap();
        assert!(whole.from_template.is_empty());
        assert!(!t.join(LIVE).join("screens/extra.luau").exists());
    }

    #[test]
    fn an_error_is_blamed_on_the_template_file_it_names() {
        let t = setup("blame");
        let (_, overlay) = compose(&t, "t", &BTreeSet::new(), false).unwrap();
        let live = t.join(LIVE).display().to_string();
        // Zeron's shell required the template's sidebar, which failed.
        let e = format!("app.luau: runtime error: {live}/app.luau:3: in require\n\t{live}/screens/shell/sidebar.luau:12: attempt to index nil");
        assert_eq!(blame(&e, &t, &overlay).as_deref(), Some("screens/shell/sidebar.luau"));
        // The template's file called Zeron's tokens with something wrong: still the template's.
        let e = format!("{live}/ds/tokens.luau:4: bad argument\nstack traceback:\n\t{live}/screens/extra.luau:9: in function");
        assert_eq!(blame(&e, &t, &overlay).as_deref(), Some("screens/extra.luau"));
        // Zeron's own error: nobody's to drop.
        assert_eq!(blame(&format!("{live}/ds/tokens.luau:4: oops"), &t, &overlay), None);
        // The app's modules are named from its folder; the VM's own name
        // (who ran it) isn't where it failed.
        assert_eq!(blame("app.luau: runtime error: screens/extra.luau:3: broke later", &t, &overlay).as_deref(), Some("screens/extra.luau"));
        assert_eq!(blame("app.luau: runtime error: ds/tokens.luau:3: oops", &t, &overlay), None);
    }

    /// A template's tokens go beside Zeron's, never over them (Zeron's run
    /// theirs first, then its), and an error in them is its `ds/tokens.luau`'s.
    #[test]
    fn a_template_s_tokens_are_laid_beside_zeron_s() {
        let t = setup("tokens");
        write(&t.join(ZERON), "ds/tokens_template.luau", "-- zeron's stub");
        write(&t, "t/ds/tokens.luau", "-- t tokens");
        write(&t, "t/ds/tokens_template.luau", "-- t's, never laid");
        write(&t, "t/ds/tokens_template/init.luau", "-- t's folder, never laid");
        write(&t, "t/ds/tokens/init.luau", "-- t's folder over Zeron's tokens, never laid");
        let (_, overlay) = compose(&t, "t", &BTreeSet::new(), false).unwrap();
        let live = t.join(LIVE);
        assert!(!live.join("ds/tokens_template").exists(), "a template's folder of that name is never laid");
        assert!(!live.join("ds/tokens").exists(), "nor one that would stand for Zeron's tokens");
        assert_eq!(read(&live, "ds/tokens.luau"), "-- zeron tokens");
        assert_eq!(read(&live, "ds/tokens_template.luau"), "-- t tokens");
        assert!(overlay.from_template.contains("ds/tokens_template.luau") && !overlay.from_template.contains("ds/tokens.luau"));

        // Blamed by its own path; Zeron's tokens' own error is nobody's.
        let shown = live.display().to_string();
        let e = format!("app.luau: runtime error: {shown}/ds/tokens.luau:850: in require\n\t{shown}/ds/tokens_template.luau:12: attempt to index nil");
        assert_eq!(blame(&e, &t, &overlay).as_deref(), Some("ds/tokens.luau"));
        assert_eq!(blame("app.luau: runtime error: ds/tokens_template.luau:3: oops", &t, &overlay).as_deref(), Some("ds/tokens.luau"));
        assert_eq!(blame(&format!("{shown}/ds/tokens.luau:4: oops"), &t, &overlay), None);

        // Failed, Zeron's stub shows in its place; Zeron's own are never touched.
        let failed = BTreeSet::from(["ds/tokens.luau".to_string()]);
        let (_, overlay) = compose(&t, "t", &failed, false).unwrap();
        assert_eq!(read(&live, "ds/tokens_template.luau"), "-- zeron's stub");
        assert_eq!(read(&live, "ds/tokens.luau"), "-- zeron tokens");
        assert!(overlay.laid_as.is_empty());

        // Folders `.live` is left with (an older Zeron laid a template's
        // `ds/tokens_template/init.luau`; anyone may have written here):
        // - emptied once the strays go: removed, however deep;
        // - holding something still, and standing for a wanted module (Luau
        //   would take it for `ds/tokens_template.luau`): moved aside whole;
        // - holding something, standing for nothing: left as it is.
        // A dot folder (zgx's own) is never touched.
        write(&live, "ds/tokens_template/init.luau", "-- stray");
        write(&live, "ds/tokens_template/.cache/x", "kept");
        std::fs::create_dir_all(live.join("screens/emptied/deeper")).unwrap();
        write(&live, "notes/.git/HEAD", "mine");
        write(&live, ".zgx/types.d.luau", "");
        let (changed, _) = compose(&t, "t", &failed, false).unwrap();
        assert!(changed >= 3, "{changed}");
        assert!(!live.join("ds/tokens_template").exists() && !live.join("screens/emptied").exists());
        assert_eq!(read(&live, "ds/tokens_template.luau"), "-- zeron's stub");
        let aside: Vec<PathBuf> = std::fs::read_dir(live.join(SET_ASIDE)).unwrap().flatten().map(|e| e.path()).collect();
        assert_eq!(aside.len(), 1, "{aside:?}");
        assert_eq!(read(&aside[0], "ds/tokens_template/.cache/x"), "kept", "moved whole, never deleted");
        assert!(!aside[0].join("ds/tokens_template/init.luau").exists(), "(the stray file went as strays do)");
        assert_eq!(read(&live, "notes/.git/HEAD"), "mine");
        assert!(live.join("screens/shell/sidebar.luau").is_file() && live.join(".zgx/types.d.luau").is_file());
        // Laid again: nothing more moves.
        let (again, _) = compose(&t, "t", &failed, false).unwrap();
        assert_eq!(again, 0);
        assert_eq!(std::fs::read_dir(live.join(SET_ASIDE)).unwrap().count(), 1);

        // Another spelling of either is the same file.
        std::fs::remove_dir_all(t.join("t/ds")).unwrap();
        write(&t, "t/DS/Tokens.luau", "-- t tokens, spelt");
        write(&t, "t/DS/Tokens_Template.luau", "-- t's, never laid");
        let (want, overlay) = plan(&t, "t", &BTreeSet::new(), false);
        assert!(want["ds/tokens.luau"].starts_with(t.join(ZERON)));
        assert_eq!(std::fs::read_to_string(&want["ds/tokens_template.luau"]).unwrap(), "-- t tokens, spelt");
        assert_eq!(want.keys().filter(|k| k.to_lowercase().starts_with("ds/tokens")).count(), 2, "{want:?}");
        assert_eq!(overlay.laid_as["ds/tokens_template.luau"], "DS/Tokens.luau");
    }

    /// A template's files are laid under Zeron's spelling of its folders
    /// and files (one spelling written of each), what's protected left out
    /// in any spelling; and a lay-down keeps Zeron's file where its folder
    /// on the disk is spelt otherwise (the same file where the disk folds
    /// names), never taking it for a stray. By the fold alone: so on either
    /// kind of disk.
    #[test]
    fn a_template_s_files_are_laid_under_zeron_s_spelling() {
        let t = setup("spelling");
        write(&t, "s/zeron-template.json", r#"{"id":"s"}"#);
        write(&t, "s/Screens/shell/x.luau", "-- s's own, beside Zeron's");
        write(&t, "s/SCREENS/plugins/y.luau", "-- protected in any spelling");
        write(&t, "s/Screens/Shell/Sidebar.luau", "-- s's sidebar");
        let (want, overlay) = plan(&t, "s", &BTreeSet::new(), false);
        assert!(want.keys().all(|k| !k.starts_with("Screens") && !k.starts_with("SCREENS")), "{want:?}");
        assert_eq!(std::fs::read_to_string(&want["screens/shell/x.luau"]).unwrap(), "-- s's own, beside Zeron's");
        assert_eq!(std::fs::read_to_string(&want["screens/shell/sidebar.luau"]).unwrap(), "-- s's sidebar");
        assert!(want["screens/plugins/page.luau"].starts_with(t.join(ZERON)));
        assert!(!want.keys().any(|k| k.ends_with("y.luau")));
        assert!(overlay.laid_as["screens/shell/x.luau"].eq_ignore_ascii_case("Screens/shell/x.luau"));

        // `.live`'s folder spelt otherwise on the disk (an older lay-down).
        compose(&t, BUILTIN, &BTreeSet::new(), false).unwrap();
        let live = t.join(LIVE);
        std::fs::rename(live.join("screens"), live.join("Screens.tmp")).unwrap();
        std::fs::rename(live.join("Screens.tmp"), live.join("Screens")).unwrap();
        compose(&t, BUILTIN, &BTreeSet::new(), false).unwrap();
        assert_eq!(read(&live, "screens/shell/sidebar.luau"), "-- zeron sidebar");
        let sidebars = files(&live, |_| true).into_keys().filter(|k| k.eq_ignore_ascii_case("screens/shell/sidebar.luau")).count();
        assert_eq!(sidebars, 1, "one copy, wherever the disk keeps it");
    }

    /// A link in `.live` by a module's name (to a folder with an
    /// init.luau, which zgx would follow) is moved aside, the link itself:
    /// what it's to is never touched; Zeron's file of that name stands. A
    /// link by no module's name is left.
    #[cfg(unix)]
    #[test]
    fn a_link_that_stands_for_a_module_is_moved_aside() {
        let t = setup("link");
        write(&t.join(ZERON), "ds/tokens_template.luau", "-- zeron's stub");
        compose(&t, "t", &BTreeSet::new(), false).unwrap();
        let live = t.join(LIVE);
        let elsewhere = t.parent().unwrap().join("elsewhere");
        write(&elsewhere, "init.luau", "-- not this");
        std::os::unix::fs::symlink(&elsewhere, live.join("ds/tokens_template")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, live.join("notes")).unwrap();
        compose(&t, "t", &BTreeSet::new(), false).unwrap();
        assert!(live.join("ds/tokens_template").symlink_metadata().is_err(), "the link is gone from .live");
        assert_eq!(read(&live, "ds/tokens_template.luau"), "-- zeron's stub");
        let aside: Vec<PathBuf> = std::fs::read_dir(live.join(SET_ASIDE)).unwrap().flatten().map(|e| e.path()).collect();
        assert_eq!(aside.len(), 1);
        assert!(aside[0].join("ds/tokens_template").symlink_metadata().is_ok_and(|m| m.file_type().is_symlink()), "moved, as a link");
        assert_eq!(read(&elsewhere, "init.luau"), "-- not this", "what it's to is untouched");
        assert!(live.join("notes").symlink_metadata().is_ok(), "a link by no module's name is left");
    }

    /// A template's theme files are laid beside Zeron's, renamed and set over
    /// the palette worn (`template-*`, `palette-*`), each naming only its
    /// own tokens (comments and all, read as zgx reads a theme); Zeron's own
    /// stay. One that doesn't read as a theme is left out, Zeron's empty one
    /// in its place, and said.
    #[test]
    fn a_template_s_theme_files_are_laid_over_the_palette() {
        let t = setup("themes");
        let z = t.join(ZERON);
        write(&z, "ds/themes/zeron-dark.json", r#"{ "name": "zeron-dark", "type": "dark", "extends": "zgx-dark", "tokens": { "zeron": { "list": { "gap": 2 } } } }"#);
        write(&z, "ds/themes/template-dark.json", r#"{ "name": "template-dark", "type": "dark", "extends": "palette-dark", "tokens": {} }"#);
        write(&z, "ds/themes/template-light.json", r#"{ "name": "template-light", "type": "light", "extends": "palette-light", "tokens": {} }"#);
        write(&t, "t/ds/themes/zeron-dark.json", "{\n\t// Only what it changes.\n\t\"name\": \"zeron-dark\",\n\t\"type\": \"dark\",\n\t\"extends\": \"zgx-dark\",\n\t\"tokens\": { \"zeron\": { \"selected\": { \"flat\": { \"bg\": \"#123456\" } } } }\n}");
        write(&t, "t/ds/themes/zeron-light.json", "not a theme");
        let (_, overlay) = compose(&t, "t", &BTreeSet::new(), false).unwrap();
        let live = t.join(LIVE);
        let laid = zgx::theme::Theme::from_json(&read(&live, "ds/themes/template-dark.json")).unwrap();
        assert_eq!(laid.to_json(), zgx::theme::Theme::new("template-dark", zgx::theme::ThemeKind::Dark).extending("palette-dark").with_expr(zgx::theme::token("zeron.selected.flat.bg"), "#123456").unwrap().to_json());
        assert!(read(&live, "ds/themes/zeron-dark.json").contains("\"gap\": 2"), "Zeron's own file stays");
        assert_eq!(overlay.laid_as["ds/themes/template-dark.json"], "ds/themes/zeron-dark.json");
        // Not a theme: left out, Zeron's empty one, said.
        assert!(read(&live, "ds/themes/template-light.json").contains("\"tokens\": {}"));
        assert!(overlay.unread_themes.contains("ds/themes/zeron-light.json") && !overlay.from_template.contains("ds/themes/template-light.json"));
        // And a template's own file at those paths is never laid.
        assert!(laid_by_zeron("ds/themes/template-dark.json") && laid_by_zeron("DS/Themes/Template-Light.json") && !laid_by_zeron("ds/themes/zeron-dark.json"));
    }

    /// A template's file laid across Zeron's (a folder of Zeron's by its
    /// name, or its path through a file of Zeron's) is left out, and said;
    /// the rest of the template is laid as ever, and the lay-down doesn't
    /// fail part-way.
    #[test]
    fn a_template_s_file_across_zeron_s_is_left_out_and_said() {
        let t = setup("clash");
        write(&t, "c/zeron-template.json", r#"{"id":"c"}"#);
        write(&t, "c/app.luau/x.luau", "-- through Zeron's file");
        write(&t, "c/screens/shell", "-- where Zeron has a folder");
        write(&t, "c/screens/extra.luau", "-- c's own");
        let (_, overlay) = compose(&t, "c", &BTreeSet::new(), false).expect("laid whole");
        let live = t.join(LIVE);
        assert_eq!(read(&live, "app.luau"), "-- zeron app");
        assert_eq!(read(&live, "screens/shell/sidebar.luau"), "-- zeron sidebar");
        assert_eq!(read(&live, "screens/extra.luau"), "-- c's own");
        let mut clashing: Vec<_> = overlay.clashing.iter().map(|r| r.to_lowercase()).collect();
        clashing.sort();
        assert_eq!(clashing, ["app.luau/x.luau", "screens/shell"]);
        assert!(!overlay.from_template.iter().any(|r| r.starts_with("app.luau/") || r.eq_ignore_ascii_case("screens/shell")));
        // And a file beside, not across: laid.
        let shape = ZeronShape::of([String::from("ds/button.luau"), String::from("screens/a/b.luau")].iter());
        for (rel, clashes) in [("ds/button.luau/x.luau", true), ("screens/a", true), ("SCREENS", true), ("ds/button.luau", false), ("ds/button.luaux/x.luau", false), ("screens/a/c.luau", false), ("screens/ab", false)] {
            assert_eq!(shape.clashes(rel), clashes, "{rel}");
        }
    }

    /// The app's own refusal, whatever the guard: where Zeron lays a
    /// template's tokens, and a folder that would stand for Zeron's, is
    /// never a file of the template's.
    #[test]
    fn where_zeron_lays_a_template_s_tokens_is_never_its_own() {
        for (rel, zeron_s) in [
            ("ds/tokens_template.luau", true),
            ("DS/Tokens_Template.luau", true),
            ("ds/tokens_template/init.luau", true),
            ("ds/Tokens_Template/deep/a.luau", true),
            ("ds/tokens_template./init.luau", true),
            ("ds/tokens/init.luau", true),
            ("DS/Tokens./init.luau", true),
            ("ds/tokens/deep/x.luau", true),
            ("ds/tokens.luau", false),
            ("ds/tokens", false),
            ("ds/tokensx/init.luau", false),
            ("ds/tokens_templates.luau", false),
            ("ds/tokens_templates/init.luau", false),
            ("ds/tokens_template", false),
            ("screens/ds/tokens_template.luau", false),
        ] {
            assert_eq!(laid_by_zeron(rel), zeron_s, "{rel}");
        }
    }

    /// R12: safe mode runs Zeron's own whatever `.active` says, so a
    /// template re-selecting itself (from a timer, a terminal, an agent)
    /// changes nothing until you pick one on the Templates page.
    #[test]
    fn safe_mode_holds_zeron_s_own_whatever_active_says() {
        let t = setup("safe");
        set_active(&t, "t").unwrap();
        super::super::preview::enter_safe_for_test(true);
        assert_eq!(active(&t), BUILTIN);
        assert_eq!(chosen(&t), "t", "what was chosen, still shown");
        set_active(&t, "t").unwrap();
        assert_eq!(active(&t), BUILTIN, "re-selected from a timer: still Zeron's own");
        super::super::preview::enter_safe_for_test(false);
        assert_eq!(active(&t), "t");
    }

    #[test]
    fn the_active_template_is_read_safely() {
        let t = setup("active");
        assert_eq!(active(&t), BUILTIN);
        set_active(&t, "t").unwrap();
        assert_eq!(active(&t), "t");
        set_active(&t, "../../etc").unwrap();
        assert_eq!(active(&t), BUILTIN);
        set_active(&t, "gone").unwrap();
        assert_eq!(active(&t), BUILTIN);
        assert!(!plain("../x") && !plain("/x") && !plain("a/.b") && plain(".luaurc") && plain("screens/x.luau"));
    }

    #[test]
    fn zeron_s_copy_holds_the_app_and_not_the_gallery_or_tests() {
        let root = dir("zeron");
        let ui = Path::new(env!("CARGO_MANIFEST_DIR")).join("ui");
        let t = root.join("templates");
        let first = sync_zeron(&ui, &t).unwrap();
        assert!(first > 10);
        let z = t.join(ZERON);
        assert!(z.join("app.luau").is_file() && z.join("screens/shell/sidebar.luau").is_file() && z.join("ds/init.luau").is_file());
        assert!(!z.join("gallery.luau").exists() && !z.join("tests").exists() && !z.join("demos").exists());
        assert!(!z.join("consent.luau").exists());
        assert!(z.join(".hashes.json").is_file() && z.join(".catalog.json").is_file());
        // Stamped with Zeron's version, the root workspace's (build.rs).
        let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(z.join("zeron-template.json")).unwrap()).unwrap();
        let root = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml")).unwrap();
        assert!(!ZERON_VERSION.is_empty() && root.contains(&format!("version = \"{ZERON_VERSION}\"")));
        assert_eq!(manifest["zeron_version"], ZERON_VERSION);
        assert_eq!(sync_zeron(&ui, &t).unwrap(), 0, "nothing new, nothing written");
        // `.zeron` keeps to the same rule as `.live`: an emptied folder goes,
        // one holding something stays, and one that stands for a module of
        // Zeron's is moved aside whole.
        std::fs::create_dir_all(z.join("screens/gone/deeper")).unwrap();
        write(&z, "screens/gone/stray.luau", "-- stray");
        write(&z, "mine/.notes", "mine");
        write(&z, "screens/shell/.DS_Store", "kept");
        write(&z, "ds/tokens/.swp", "kept");
        assert!(sync_zeron(&ui, &t).unwrap() >= 3);
        assert!(!z.join("screens/gone").exists());
        assert_eq!(read(&z, "mine/.notes"), "mine");
        assert_eq!(read(&z, "screens/shell/.DS_Store"), "kept");
        assert!(!z.join("ds/tokens").exists() && z.join("ds/tokens.luau").is_file());
        let aside = std::fs::read_dir(z.join(SET_ASIDE)).unwrap().flatten().next().unwrap().path();
        assert_eq!(read(&aside, "ds/tokens/.swp"), "kept");
    }

    #[test]
    fn a_survey_copies_zeron_s_own_only_when_ui_changed() {
        let root = dir("survey");
        let ui = root.join("ui");
        write(&ui, "app.luau", "-- app");
        write(&ui, "screens/x.luau", "-- x");
        write(&ui, "demos/d.luau", "-- not the app's");
        let t = root.join("templates");
        let first = survey(&ui, &t, None);
        assert!(first.synced.as_ref().is_ok_and(|n| *n > 0));
        assert_eq!(first.id, BUILTIN);
        // Nothing changed: nothing copied, the same stamp.
        let again = survey(&ui, &t, Some(&first.ui_stamp));
        assert_eq!((again.synced.clone().unwrap(), &again.ui_stamp), (0, &first.ui_stamp));
        // A demo isn't the app's.
        std::thread::sleep(std::time::Duration::from_millis(5));
        write(&ui, "demos/d.luau", "-- changed");
        assert_eq!(survey(&ui, &t, Some(&first.ui_stamp)).ui_stamp, first.ui_stamp);
        // A screen saved: copied again.
        write(&ui, "screens/x.luau", "-- x, saved");
        let saved = survey(&ui, &t, Some(&first.ui_stamp));
        assert_ne!(saved.ui_stamp, first.ui_stamp);
        assert_eq!(saved.synced.unwrap(), 1);
        assert_eq!(read(&t.join(ZERON), "screens/x.luau"), "-- x, saved");
        // A template in use: its files as they stand, and a stamp that follows them.
        write(&t, "mine/zeron-template.json", r#"{"name":"Mine"}"#);
        write(&t, "mine/screens/x.luau", "-- mine");
        set_active(&t, "mine").unwrap();
        let used = survey(&ui, &t, Some(&saved.ui_stamp));
        assert_eq!(used.id, "mine");
        assert!(used.files.iter().any(|(rel, s)| rel == "screens/x.luau" && !s.is_empty()));
        std::thread::sleep(std::time::Duration::from_millis(5));
        write(&t, "mine/screens/x.luau", "-- mine, edited");
        assert_ne!(survey(&ui, &t, Some(&saved.ui_stamp)).template_stamp, used.template_stamp);
    }

    /// A launch lays `.live` down once; the next, finding everything as the
    /// last left it, runs it as it is. Anything changed since lays it down
    /// again: `ui/` (an update), the template switched or edited, `.live`
    /// written by anyone, a file of it gone.
    #[test]
    fn a_launch_runs_live_as_it_is_only_while_nothing_changed() {
        let root = dir("launch-stamp");
        let ui = root.join("ui");
        write(&ui, "app.luau", "-- app");
        write(&ui, "screens/x.luau", "-- x");
        let t = root.join("templates");
        let lay = |id: &str| {
            sync_zeron(&ui, &t).unwrap();
            compose(&t, id, &Default::default(), false).unwrap();
            LaunchStamp::now(&ui, &t, id).write(&t).unwrap();
        };
        let runs_as_is = |id: &str| LaunchStamp::now(&ui, &t, id).still_laid(&ui, &t);
        // Nothing laid yet.
        assert!(!runs_as_is(BUILTIN));
        lay(BUILTIN);
        assert!(runs_as_is(BUILTIN));
        assert!(runs_as_is(BUILTIN), "looking writes nothing");
        // Zeron's own changed (an update): laid again, then as it is.
        std::thread::sleep(std::time::Duration::from_millis(5));
        write(&ui, "screens/x.luau", "-- x, updated");
        assert!(!runs_as_is(BUILTIN));
        lay(BUILTIN);
        assert!(runs_as_is(BUILTIN));
        assert_eq!(read(&t.join(LIVE), "screens/x.luau"), "-- x, updated");
        // `.live` written by someone else, or a file of it gone.
        std::thread::sleep(std::time::Duration::from_millis(5));
        write(&t.join(LIVE), "screens/x.luau", "-- not this Zeron's");
        assert!(!runs_as_is(BUILTIN));
        lay(BUILTIN);
        std::fs::remove_file(t.join(LIVE).join("app.luau")).unwrap();
        assert!(!runs_as_is(BUILTIN));
        lay(BUILTIN);
        assert!(runs_as_is(BUILTIN));
        // Another template in use, then one of its files edited.
        write(&t, "mine/zeron-template.json", r#"{"name":"Mine"}"#);
        write(&t, "mine/screens/x.luau", "-- mine");
        assert!(!runs_as_is("mine"));
        lay("mine");
        assert!(runs_as_is("mine"));
        assert_eq!(read(&t.join(LIVE), "screens/x.luau"), "-- mine");
        std::thread::sleep(std::time::Duration::from_millis(5));
        write(&t, "mine/screens/x.luau", "-- mine, edited");
        assert!(!runs_as_is("mine"));
        // (The overlay a launch that skips the copy still knows.)
        let (_, overlay) = plan(&t, "mine", &Default::default(), false);
        assert!(overlay.from_template.contains("screens/x.luau"));
    }

    #[test]
    fn ui_is_watched_by_its_folders_scripts_and_themes() {
        let ui = Path::new(env!("CARGO_MANIFEST_DIR")).join("ui");
        let t = setup("watched");
        let (files_, dirs) = watched(&t, &ui);
        // (By `/`, as on every platform: Windows' paths say `\`.)
        let rel = |p: &PathBuf| p.strip_prefix(&ui).map(|r| r.to_string_lossy().replace('\\', "/")).unwrap_or_default();
        assert!(files_.iter().any(|p| rel(p) == "app.luau"));
        assert!(files_.iter().any(|p| rel(p).starts_with("ds/themes/") && rel(p).ends_with(".json")));
        assert!(!files_.iter().any(|p| rel(p).ends_with(".svg") || rel(p).ends_with(".webp")), "art is heard by its folder");
        assert!(!files_.iter().chain(&dirs).any(|p| rel(p).starts_with("demos") || rel(p).starts_with("tests")));
        assert!(dirs.iter().any(|p| rel(p) == "assets"));
        // A template in use: all of it.
        set_active(&t, "t").unwrap();
        let (files_, dirs) = watched(&t, &ui);
        assert!(files_.contains(&t.join("t/screens/extra.luau")) && dirs.contains(&t.join("t/screens")));
    }

    #[test]
    fn templates_are_listed_zeron_first_and_out_of_date_is_told() {
        let t = setup("installed");
        std::fs::write(t.join(ZERON).join("zeron-template.json"), r#"{"name":"Zeron","author":"Zeron","zeron_version":"0.3.0"}"#).unwrap();
        std::fs::write(t.join(ZERON).join(".hashes.json"), r#"{"screens/shell/sidebar.luau":"now"}"#).unwrap();
        // Made for an earlier Zeron: out of date.
        write(&t, "t/zeron-template.json", r#"{"name":"Tidy","author":"you","zeron_version":"0.2.9"}"#);
        write(&t, "t/.base.json", r#"{"screens/shell/sidebar.luau":"then"}"#);
        // No version recorded: never out of date.
        write(&t, "a/zeron-template.json", r#"{"name":"Airy"}"#);
        // This Zeron's, though a file it copied has changed since (a save
        // while developing): not out of date.
        write(&t, "b/zeron-template.json", r#"{"name":"Brisk","zeron_version":"0.3.0"}"#);
        write(&t, "b/.base.json", r#"{"screens/shell/sidebar.luau":"then"}"#);
        write(&t, "notatemplate/file.txt", "x");
        let list = installed(&t);
        let ids: Vec<&str> = list.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, vec![BUILTIN, "a", "b", "t"]);
        let stale: Vec<bool> = list.iter().map(|i| i.out_of_date).collect();
        assert_eq!(stale, vec![false, false, false, true]);
        assert!(version_out_of_date("0.2.9", "0.3.0") && !version_out_of_date("", "0.3.0") && !version_out_of_date("0.3.0", ""));
        assert!(!version_out_of_date("0.0.0", "0.3.0"), "a development stamp isn't a version");
        let (id, name) = copy_name(&t, "t");
        assert_eq!((id.as_str(), name.as_str()), ("t-copy", "Tidy copy"));
        assert_eq!(new_id(&t, "My Dark  Theme!"), "my-dark-theme");
        assert_eq!(new_id(&t, "T"), "template-t");
        assert_eq!(new_id(&t, "Zeron"), "template-zeron");
        assert_eq!(new_id(&t, "  "), "template");
        std::fs::create_dir_all(t.join("airy")).unwrap();
        assert_eq!(new_id(&t, "Airy"), "airy-2");
        assert_eq!(list[3].zeron_version, "0.2.9");
    }

    #[test]
    fn a_bundle_s_interface_isn_t_watched_file_by_file() {
        let root = dir("watched-bundle");
        let templates = root.join("templates");
        let ui = root.join("Zeron.app/Contents/Resources/ui");
        write(&ui, "app.luau", "return 1");
        write(&ui, "screens/shell/page.luau", "return 2");
        std::fs::create_dir_all(&templates).unwrap();
        let (files_, dirs) = watched(&templates, &ui);
        assert_eq!(files_, vec![templates.join(ACTIVE)]);
        assert_eq!(dirs, vec![templates.clone()]);
        // A source tree, which is edited as it runs, still is.
        let source = root.join("apps/zgx/ui");
        write(&source, "app.luau", "return 1");
        let (files_, dirs) = watched(&templates, &source);
        assert!(files_.contains(&source.join("app.luau")));
        assert!(dirs.contains(&source));
    }
}
