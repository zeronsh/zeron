//! File links written as inline code.
//!
//! Agents name files in code spans — "all under `dir/`:" followed by bare
//! `SOURCES.md`, `DESCRIPTION.txt` — which the block parser has no link for.
//! A span whose text resolves to an existing file on this device is rewritten
//! into the Markdown link it stands for, shown under its file name because
//! the span's text was the path rather than a label the author wrote; spans
//! with nothing behind them keep the inline-code look. Where two different
//! paths in one part end in the same name, each shows as many trailing
//! components as it takes to tell them apart.
//!
//! The rewrite walks a whole text part in document order, once per
//! (link-roots revision, part): every probe is memoized for the revision, and
//! the linked tree is reused across frames. A directory span is not a link,
//! but it becomes the context a later bare name in the same part resolves
//! against.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use super::parser::{Block, BlockTree, InlineRun, TopBlock};
use crate::workspace_links::{
    FileLinkRoot, InlineCodePath, PathProbes, resolve_inline_code_path, without_location,
};

/// How many text parts stay memoized, least recently used evicted first. A
/// viewport can hold many short parts at once, and a streaming reply
/// re-parses into a new tree on every commit, so stale streamed trees age out
/// while every part on screen stays cached across frames.
const CACHED_PARTS: usize = 32;

/// Text parts already rewritten, keyed by their source tree and reset when
/// the file-link roots change.
#[derive(Default)]
pub(crate) struct InlineCodeLinkCache {
    revision: u64,
    probes: PathProbes,
    parts: Vec<LinkedPart>,
}

struct LinkedPart {
    /// Keeps the source tree alive so its address cannot be reused by a
    /// different part while this entry is cached.
    source: Arc<BlockTree>,
    linked: Arc<BlockTree>,
}

impl InlineCodeLinkCache {
    /// Drop every memo when the file-link roots change — the only input that
    /// can stale a resolved link. Reports whether anything was dropped, so
    /// the caller can also invalidate presentation caches holding the old
    /// styling.
    pub(crate) fn set_revision(&mut self, revision: u64) -> bool {
        if self.revision == revision {
            return false;
        }
        self.revision = revision;
        self.probes = PathProbes::default();
        self.parts.clear();
        true
    }

    /// `tree` with every inline code span that names an existing file
    /// rewritten into the link it stands for. `source_local` gates the whole
    /// walk: a remote chat's text names files on its own device, which must
    /// not be probed here.
    pub(crate) fn linked_tree(
        &mut self,
        tree: &Arc<BlockTree>,
        roots: &[FileLinkRoot],
        source_local: bool,
    ) -> Arc<BlockTree> {
        if !source_local {
            return tree.clone();
        }
        if let Some(ix) = self
            .parts
            .iter()
            .position(|part| Arc::ptr_eq(&part.source, tree))
        {
            // Most recently used last.
            let part = self.parts.remove(ix);
            let linked = part.linked.clone();
            self.parts.push(part);
            return linked;
        }
        let linked = Arc::new(link_tree(tree, roots, &mut self.probes));
        if self.parts.len() >= CACHED_PARTS {
            self.parts.remove(0);
        }
        self.parts.push(LinkedPart {
            source: tree.clone(),
            linked: linked.clone(),
        });
        linked
    }
}

/// A code span's text → the label it shows once it links.
type Labels<'a> = HashMap<&'a str, &'a str>;

fn link_tree(tree: &BlockTree, roots: &[FileLinkRoot], probes: &mut PathProbes) -> BlockTree {
    let mut spans = Vec::new();
    for top in &tree.blocks {
        code_spans(&top.block, &mut spans);
    }
    let labels = span_labels(&spans);
    let mut dirs = Vec::new();
    let blocks = tree
        .blocks
        .iter()
        .map(
            |top| match link_block(&top.block, roots, probes, &mut dirs, &labels) {
                Some(block) => Arc::new(TopBlock {
                    range: top.range.clone(),
                    block,
                }),
                None => top.clone(),
            },
        )
        .collect();
    BlockTree { blocks }
}

/// `block` with every resolved code span rewritten, or `None` when nothing
/// inside it changed (the caller then shares the original block).
fn link_block(
    block: &Block,
    roots: &[FileLinkRoot],
    probes: &mut PathProbes,
    dirs: &mut Vec<PathBuf>,
    labels: &Labels<'_>,
) -> Option<Block> {
    match block {
        Block::Paragraph { runs } => {
            link_runs(runs, roots, probes, dirs, labels).map(|runs| Block::Paragraph { runs })
        }
        Block::Heading { level, runs } => {
            link_runs(runs, roots, probes, dirs, labels).map(|runs| Block::Heading {
                level: *level,
                runs,
            })
        }
        Block::BlockQuote { children } => link_blocks(children, roots, probes, dirs, labels)
            .map(|children| Block::BlockQuote { children }),
        Block::List {
            ordered_start,
            items,
        } => {
            let mut changed = false;
            let items = items
                .iter()
                .map(|item| {
                    let mut linked = Vec::with_capacity(item.len());
                    for child in item {
                        match link_block(child, roots, probes, dirs, labels) {
                            Some(block) => {
                                changed = true;
                                linked.push(block);
                            }
                            None => linked.push(child.clone()),
                        }
                    }
                    linked
                })
                .collect();
            changed.then_some(Block::List {
                ordered_start: *ordered_start,
                items,
            })
        }
        Block::Table {
            header,
            rows,
            align,
        } => {
            let mut changed = false;
            let mut link_cell =
                |cell: &[InlineRun]| match link_runs(cell, roots, probes, dirs, labels) {
                    Some(runs) => {
                        changed = true;
                        runs
                    }
                    None => cell.to_vec(),
                };
            let header = header.iter().map(|cell| link_cell(cell)).collect();
            let rows = rows
                .iter()
                .map(|row| row.iter().map(|cell| link_cell(cell)).collect())
                .collect();
            changed.then_some(Block::Table {
                header,
                rows,
                align: align.clone(),
            })
        }
        Block::CodeBlock { .. } | Block::Rule => None,
    }
}

fn link_blocks(
    blocks: &[Block],
    roots: &[FileLinkRoot],
    probes: &mut PathProbes,
    dirs: &mut Vec<PathBuf>,
    labels: &Labels<'_>,
) -> Option<Vec<Block>> {
    let mut changed = false;
    let linked = blocks
        .iter()
        .map(
            |block| match link_block(block, roots, probes, dirs, labels) {
                Some(block) => {
                    changed = true;
                    block
                }
                None => block.clone(),
            },
        )
        .collect();
    changed.then_some(linked)
}

/// One block's runs with every resolved code span rewritten, or `None` when
/// none resolved. Runs are visited in order, so a directory span only
/// provides context to the spans after it.
fn link_runs(
    runs: &[InlineRun],
    roots: &[FileLinkRoot],
    probes: &mut PathProbes,
    dirs: &mut Vec<PathBuf>,
    labels: &Labels<'_>,
) -> Option<Vec<InlineRun>> {
    let mut linked: Option<Vec<InlineRun>> = None;
    for (ix, run) in runs.iter().enumerate() {
        if !is_plain_code_span(run) {
            continue;
        }
        match resolve_inline_code_path(&run.text, roots, dirs, probes) {
            Some(InlineCodePath::File(target)) => {
                let linked = linked.get_or_insert_with(|| runs.to_vec());
                linked[ix].style.code = false;
                linked[ix].style.link = Some(target);
                // The span's text is the path, not a label the author wrote:
                // show the file name while the text stays the copy source.
                let label = labels.get(run.text.as_str()).copied();
                linked[ix].style.file_label =
                    Some(label.unwrap_or_else(|| file_name(&run.text)).to_owned());
            }
            Some(InlineCodePath::Directory(dir)) => dirs.push(dir),
            None => {}
        }
    }
    linked
}

/// Only a plain code span stands for a file: an author's link, image, task
/// marker or chat mention already means something else.
fn is_plain_code_span(run: &InlineRun) -> bool {
    run.style.code
        && run.style.link.is_none()
        && run.style.image.is_none()
        && run.style.task.is_none()
}

/// Every plain code span's text in `block`, in document order.
fn code_spans<'a>(block: &'a Block, out: &mut Vec<&'a str>) {
    let mut push = |runs: &'a [InlineRun]| {
        out.extend(
            runs.iter()
                .filter(|run| is_plain_code_span(run))
                .map(|run| run.text.as_str()),
        )
    };
    match block {
        Block::Paragraph { runs } | Block::Heading { runs, .. } => push(runs),
        Block::Table { header, rows, .. } => {
            header.iter().for_each(|cell| push(cell));
            rows.iter().flatten().for_each(|cell| push(cell));
        }
        Block::BlockQuote { children } => children.iter().for_each(|child| code_spans(child, out)),
        Block::List { items, .. } => items
            .iter()
            .flatten()
            .for_each(|child| code_spans(child, out)),
        Block::CodeBlock { .. } | Block::Rule => {}
    }
}

/// The last `components` path components of `span` (all of it when it has
/// fewer); one component is the file name, with any `:line` suffix.
fn trailing(span: &str, components: usize) -> &str {
    span.rmatch_indices('/')
        .nth(components - 1)
        .map_or(span, |(at, _)| &span[at + 1..])
}

fn file_name(span: &str) -> &str {
    trailing(span, 1)
}

/// Labels for spans whose file name a different path in the same part
/// shares: each grows trailing components until no other path ends the same
/// way (`ui/src/lib.rs` beside `engine/src/lib.rs`), keeping its `:line`
/// suffix. Spans of one path at different lines do not collide. Every other
/// span shows its file name.
fn span_labels<'a>(spans: &[&'a str]) -> Labels<'a> {
    let mut by_name: HashMap<&'a str, Vec<&'a str>> = HashMap::new();
    for &span in spans {
        let path = without_location(span);
        let same = by_name.entry(file_name(path)).or_default();
        if !same.contains(&path) {
            same.push(path);
        }
    }
    let mut labels = Labels::new();
    for &span in spans {
        let path = without_location(span);
        let same = &by_name[file_name(path)];
        if same.len() < 2 {
            continue;
        }
        // Another path ends the same way when it is the tail itself or ends
        // in `/tail`.
        let shared = |tail: &str| {
            same.iter().any(|&other| {
                other != path
                    && other
                        .strip_suffix(tail)
                        .is_some_and(|rest| rest.is_empty() || rest.ends_with('/'))
            })
        };
        let components = (1..)
            .find(|&components| {
                let tail = trailing(path, components);
                tail == path || !shared(tail)
            })
            .unwrap_or(1);
        // The location suffix holds no `/`, so the span's own tail is the
        // path's tail with the suffix still on it.
        labels.insert(span, trailing(span, components));
    }
    labels
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::parser::parse_full;
    use crate::workspace_links::FileLinkRoot;

    struct Fixture {
        _dir: tempfile::TempDir,
        root: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().join("checkout");
            std::fs::create_dir_all(root.join("2026-09-28/Some Title")).unwrap();
            for name in [
                "SOURCES.md",
                "DESCRIPTION.txt",
                "Some Title.txt",
                "Makefile",
            ] {
                std::fs::write(root.join("2026-09-28/Some Title").join(name), "x").unwrap();
            }
            std::fs::write(root.join("top.md"), "x").unwrap();
            std::fs::write(dir.path().join("outside.md"), "x").unwrap();
            Self { _dir: dir, root }
        }

        fn roots(&self, local: bool) -> Vec<FileLinkRoot> {
            vec![FileLinkRoot {
                chat: Some("chat".into()),
                root: self.root.to_string_lossy().into_owned(),
                local,
                own: true,
            }]
        }

        fn tree(&self, source: &str) -> BlockTree {
            parse_full(source)
        }

        fn linked(&self, tree: &BlockTree, roots: &[FileLinkRoot], local: bool) -> BlockTree {
            let tree = Arc::new(tree.clone());
            let mut cache = InlineCodeLinkCache::default();
            cache.set_revision(1);
            let linked = cache.linked_tree(&tree, roots, local);
            BlockTree {
                blocks: linked.blocks.clone(),
            }
        }

        fn target(&self, tree: &BlockTree, text: &str) -> Option<String> {
            runs(tree)
                .into_iter()
                .find(|run| run.text == text)
                .and_then(|run| run.style.link.clone())
        }

        fn file_target(&self, relative: &str) -> String {
            format!(
                "file://{}",
                self.root
                    .join(relative)
                    .to_string_lossy()
                    .replace(' ', "%20")
            )
        }
    }

    fn runs(tree: &BlockTree) -> Vec<InlineRun> {
        let mut out = Vec::new();
        fn walk(block: &Block, out: &mut Vec<InlineRun>) {
            match block {
                Block::Paragraph { runs } | Block::Heading { runs, .. } => out.extend(runs.clone()),
                Block::BlockQuote { children } => children.iter().for_each(|c| walk(c, out)),
                Block::List { items, .. } => {
                    items.iter().flatten().for_each(|child| walk(child, out))
                }
                Block::Table { header, rows, .. } => {
                    header.iter().for_each(|cell| out.extend(cell.clone()));
                    rows.iter()
                        .flatten()
                        .for_each(|cell| out.extend(cell.clone()));
                }
                Block::CodeBlock { .. } | Block::Rule => {}
            }
        }
        for top in &tree.blocks {
            walk(&top.block, &mut out);
        }
        out
    }

    // File links are POSIX paths; Windows spans keep their code look.
    #[cfg(unix)]
    #[test]
    fn a_context_directory_resolves_bare_names_that_follow_it() {
        let fixture = Fixture::new();
        let tree = fixture.tree(
            "Paths (all under `2026-09-28/Some Title/`):\n\
             \n\
             - `SOURCES.md`\n\
             - `DESCRIPTION.txt`\n\
             - `Makefile`\n\
             - `missing.md`\n",
        );
        let linked = fixture.linked(&tree, &fixture.roots(true), true);
        assert_eq!(
            fixture.target(&linked, "SOURCES.md").as_deref(),
            Some(
                fixture
                    .file_target("2026-09-28/Some Title/SOURCES.md")
                    .as_str()
            )
        );
        assert_eq!(
            fixture.target(&linked, "DESCRIPTION.txt").as_deref(),
            Some(
                fixture
                    .file_target("2026-09-28/Some Title/DESCRIPTION.txt")
                    .as_str()
            )
        );
        // No dot in the name, an icon theme would not recognize it — the
        // file still exists, so it links.
        assert_eq!(
            fixture.target(&linked, "Makefile").as_deref(),
            Some(
                fixture
                    .file_target("2026-09-28/Some Title/Makefile")
                    .as_str()
            )
        );
        assert_eq!(fixture.target(&linked, "missing.md"), None);
        // The directory span itself names a directory: context, not a link.
        let dir = runs(&linked)
            .into_iter()
            .find(|run| run.text == "2026-09-28/Some Title/")
            .unwrap();
        assert!(dir.style.code);
        assert!(dir.style.link.is_none());
    }

    #[test]
    fn a_bare_name_before_the_directory_span_stays_plain() {
        let fixture = Fixture::new();
        let tree = fixture.tree("`SOURCES.md` then `2026-09-28/Some Title/`");
        let linked = fixture.linked(&tree, &fixture.roots(true), true);
        assert_eq!(fixture.target(&linked, "SOURCES.md"), None);
    }

    #[cfg(unix)]
    #[test]
    fn absolute_and_root_relative_and_location_spans_link() {
        let fixture = Fixture::new();
        let absolute = fixture.root.join("top.md");
        let tree = fixture.tree(&format!(
            "`{absolute}` and `top.md:12` and `https://example.com/top.md` and `~/nope.md`",
            absolute = absolute.to_string_lossy()
        ));
        let linked = fixture.linked(&tree, &fixture.roots(true), true);
        assert_eq!(
            fixture
                .target(&linked, &absolute.to_string_lossy())
                .as_deref(),
            Some(format!("file://{}", absolute.to_string_lossy()).as_str())
        );
        assert_eq!(
            fixture.target(&linked, "top.md:12").as_deref(),
            Some(
                format!(
                    "file://{}:12",
                    fixture.root.join("top.md").to_string_lossy()
                )
                .as_str()
            )
        );
        assert_eq!(fixture.target(&linked, "https://example.com/top.md"), None);
        assert_eq!(fixture.target(&linked, "~/nope.md"), None);
    }

    #[test]
    fn remote_chats_and_remote_roots_are_never_probed() {
        let fixture = Fixture::new();
        let tree = fixture.tree("`top.md` and `2026-09-28/Some Title/SOURCES.md`");
        // A remote chat's text names files on its own device.
        let remote = fixture.linked(&tree, &fixture.roots(true), false);
        assert_eq!(fixture.target(&remote, "top.md"), None);
        assert_eq!(
            fixture.target(&remote, "2026-09-28/Some Title/SOURCES.md"),
            None
        );
        // A remote root's files are not on this disk.
        let remote_root = fixture.linked(&tree, &fixture.roots(false), true);
        assert_eq!(fixture.target(&remote_root, "top.md"), None);
        assert_eq!(
            fixture.target(&remote_root, "2026-09-28/Some Title/SOURCES.md"),
            None
        );
    }

    #[test]
    fn a_bare_name_only_another_project_has_stays_plain() {
        let fixture = Fixture::new();
        let own = fixture.root.parent().unwrap().join("own");
        std::fs::create_dir_all(&own).unwrap();
        // The linking chat's folder first, then a project that has `top.md`.
        let mut roots = fixture.roots(true);
        roots[0].chat = None;
        roots[0].own = false;
        roots.insert(
            0,
            FileLinkRoot {
                chat: Some("chat".into()),
                root: own.to_string_lossy().into_owned(),
                local: true,
                own: true,
            },
        );
        let linked = fixture.linked(&fixture.tree("see `top.md`"), &roots, true);
        assert_eq!(fixture.target(&linked, "top.md"), None);
        let span = runs(&linked)
            .into_iter()
            .find(|run| run.text == "top.md")
            .unwrap();
        assert!(span.style.code);
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_span_flattens_exactly_like_the_markdown_link_it_stands_for() {
        use crate::markdown::render::flatten_runs;
        use crate::theme::Theme;
        let fixture = Fixture::new();
        let linked = fixture.linked(&fixture.tree("`top.md`"), &fixture.roots(true), true);
        let target = fixture.target(&linked, "top.md").unwrap();
        let theme = Theme::dark();
        let from_code = flatten_runs(&runs(&linked), &theme, false);
        let from_link = flatten_runs(
            &runs(&fixture.tree(&format!("[top.md]({target})"))),
            &theme,
            false,
        );
        assert_eq!(from_code.text, from_link.text);
        assert_eq!(from_code.links, from_link.links);
        assert_eq!(from_code.code_ranges, from_link.code_ranges);
        assert_eq!(from_code.runs, from_link.runs);
    }

    #[cfg(unix)]
    #[test]
    fn a_nested_path_span_shows_its_file_name_and_keeps_the_path_for_copy() {
        use crate::markdown::render::flatten_runs;
        use crate::theme::Theme;
        let fixture = Fixture::new();
        for (source, shown) in [
            ("`2026-09-28/Some Title/SOURCES.md`", "SOURCES.md"),
            ("`top.md:12`", "top.md:12"),
        ] {
            let linked = fixture.linked(&fixture.tree(source), &fixture.roots(true), true);
            let flat = flatten_runs(&runs(&linked), &Theme::dark(), false);
            assert_eq!(flat.text, shown);
            if shown != source.trim_matches('`') {
                // The display name stands in for the path; the raw span stays
                // the copy source.
                let original = flat
                    .original
                    .as_ref()
                    .expect("the raw span stays the copy source");
                assert_eq!(original.text.as_ref(), source.trim_matches('`'));
                assert_eq!(original.offsets.original(0), 0);
                assert_eq!(
                    original.offsets.original(flat.text.len()),
                    original.text.len()
                );
            } else {
                // Already the file name: what shows IS the source text.
                assert!(flat.original.is_none());
            }
            assert_eq!(&flat.text[flat.links[0].0.clone()], shown);
        }
    }

    #[cfg(unix)]
    #[test]
    fn paths_sharing_a_file_name_show_enough_of_the_path_to_differ() {
        use crate::markdown::render::flatten_runs;
        use crate::theme::Theme;
        let fixture = Fixture::new();
        for dir in ["crates/ui/src", "crates/engine/src"] {
            std::fs::create_dir_all(fixture.root.join(dir)).unwrap();
            std::fs::write(fixture.root.join(dir).join("lib.rs"), "x").unwrap();
        }
        let tree = fixture.tree(
            "- `crates/ui/src/lib.rs`\n\
             - `crates/engine/src/lib.rs:40`\n\
             - `crates/engine/src/lib.rs`\n\
             - `2026-09-28/Some Title/SOURCES.md`\n",
        );
        let linked = fixture.linked(&tree, &fixture.roots(true), true);
        let shown: Vec<String> = runs(&linked)
            .iter()
            .map(|run| flatten_runs(std::slice::from_ref(run), &Theme::dark(), false).text)
            .map(|text| text.to_string())
            .collect();
        // Two paths ending in `lib.rs` show as much as tells them apart,
        // line suffix included; a name nothing else shares shows alone.
        assert_eq!(
            shown,
            [
                "ui/src/lib.rs",
                "engine/src/lib.rs:40",
                "engine/src/lib.rs",
                "SOURCES.md"
            ]
        );
    }

    #[test]
    fn span_labels_grow_only_as_far_as_a_collision_needs() {
        let labels = span_labels(&[
            "a/x/mod.rs",
            "b/x/mod.rs",
            "mod.rs",
            "a/x/mod.rs:3",
            "/abs/c/mod.rs",
            "README.md",
            "README.md#L4",
        ]);
        assert_eq!(labels.get("a/x/mod.rs"), Some(&"a/x/mod.rs"));
        assert_eq!(labels.get("a/x/mod.rs:3"), Some(&"a/x/mod.rs:3"));
        assert_eq!(labels.get("b/x/mod.rs"), Some(&"b/x/mod.rs"));
        // A span with nothing more to show stays as written.
        assert_eq!(labels.get("mod.rs"), Some(&"mod.rs"));
        assert_eq!(labels.get("/abs/c/mod.rs"), Some(&"c/mod.rs"));
        // One file at two locations is no collision: the plain file name.
        assert_eq!(labels.get("README.md"), None);
        assert_eq!(labels.get("README.md#L4"), None);
    }

    #[test]
    fn a_later_span_reuses_the_cached_part_for_one_revision() {
        let fixture = Fixture::new();
        let tree = Arc::new(fixture.tree("`top.md`"));
        let roots = fixture.roots(true);
        let mut cache = InlineCodeLinkCache::default();
        assert!(cache.set_revision(7));
        let first = cache.linked_tree(&tree, &roots, true);
        let second = cache.linked_tree(&tree, &roots, true);
        assert!(Arc::ptr_eq(&first, &second));
        // The roots changed: the memo drops, presentation must rebuild.
        assert!(cache.set_revision(8));
        let third = cache.linked_tree(&tree, &roots, true);
        assert!(!Arc::ptr_eq(&first, &third));
        assert!(!cache.set_revision(8));
    }

    #[test]
    fn a_part_in_use_survives_other_parts_filling_the_cache() {
        let fixture = Fixture::new();
        let roots = fixture.roots(true);
        let mut cache = InlineCodeLinkCache::default();
        cache.set_revision(1);
        let kept = Arc::new(fixture.tree("`top.md`"));
        let first = cache.linked_tree(&kept, &roots, true);
        let mut others = Vec::new();
        for ix in 0..CACHED_PARTS * 2 {
            let other = Arc::new(fixture.tree(&format!("part {ix}")));
            cache.linked_tree(&other, &roots, true);
            others.push(other);
            // A part rendered every frame stays the most recent.
            assert!(Arc::ptr_eq(&first, &cache.linked_tree(&kept, &roots, true)));
        }
        assert!(cache.parts.len() <= CACHED_PARTS);
    }

    /// The linked span reaches the same hit testing a Markdown file link
    /// does: a left click activates it, a right click opens the file menu.
    #[cfg(target_os = "linux")]
    mod rendered {
        use super::*;
        use crate::markdown::render::{self, LinkActivation, LinkOutcome, LinkUi};
        use crate::theme::Theme;
        use gpui::{
            Context, MouseButton, Render, TestAppContext, Window, div, point, prelude::*, px,
        };
        use std::rc::Rc;

        struct Scene {
            tree: Arc<BlockTree>,
            roots: Rc<Vec<FileLinkRoot>>,
            activated: Rc<std::cell::RefCell<Vec<LinkActivation>>>,
        }

        impl Render for Scene {
            fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                let mut opts = render::RenderOptions::settled("inline-code-fixture".into());
                let activated = self.activated.clone();
                opts.link = Some(LinkUi {
                    source_session: Some("chat".into()),
                    source_local: true,
                    file_roots: Some(self.roots.clone()),
                    handler: Rc::new(move |activation, _, _| {
                        activated.borrow_mut().push(activation.clone());
                        LinkOutcome::Internal
                    }),
                });
                div()
                    .w(px(320.))
                    .child(render::selection_frame_reset())
                    .child(render::render_tree(
                        &self.tree,
                        &opts,
                        &Theme::of(cx).clone(),
                        window,
                        &|_| None,
                    ))
            }
        }

        #[gpui::test]
        fn a_linked_code_span_clicks_and_menus_like_a_file_link(cx: &mut TestAppContext) {
            let fixture = Fixture::new();
            cx.update(|cx| {
                cx.set_global(Theme::dark());
                crate::settings::init(
                    crate::settings::UiSettings::default(),
                    fixture._dir.path(),
                    cx,
                );
            });
            let roots = Rc::new(fixture.roots(true));
            let source = Arc::new(fixture.tree("`top.md`"));
            let mut cache = InlineCodeLinkCache::default();
            cache.set_revision(1);
            let tree = cache.linked_tree(&source, &roots, true);
            let activated = Rc::<std::cell::RefCell<Vec<LinkActivation>>>::default();
            let (_view, cx) = cx.add_window_view(|_, _| Scene {
                tree,
                roots,
                activated: activated.clone(),
            });
            let position = cx.update(|_, _| {
                let (_, layout, _) = render::selection_test_snapshot("inline-code-fixture:0");
                layout.position_for_index(2).unwrap() + point(px(2.), px(8.))
            });
            let target = fixture.file_target("top.md");
            cx.simulate_click(position, gpui::Modifiers::default());
            let activated = activated.borrow();
            let last = activated.last().expect("the file link activates");
            assert_eq!(last.action, crate::markdown::render::LinkAction::Primary);
            assert_eq!(last.target.original, target);
            drop(activated);
            cx.simulate_mouse_down(position, MouseButton::Right, gpui::Modifiers::default());
            cx.simulate_mouse_up(position, MouseButton::Right, gpui::Modifiers::default());
            for selector in [
                "link-menu-open-zeron",
                "link-menu-open-default",
                "link-menu-show-in-folder",
                "link-menu-copy-path",
            ] {
                assert!(
                    cx.debug_bounds(selector).is_some(),
                    "a linked code span gets the file menu: {selector}"
                );
            }
            assert!(
                cx.debug_bounds("link-menu-open-external").is_none(),
                "the web rows never mount for a file"
            );
        }
    }
}
