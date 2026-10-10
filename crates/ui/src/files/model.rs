use std::collections::{HashMap, HashSet};

use zeron_proto::{
    WorkspaceDirectoryPage, WorkspaceEntry, WorkspaceEntryKind, WorkspaceFileSearchMatch,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirectoryLoadState {
    Unloaded,
    Loading {
        cursor: Option<String>,
    },
    Loaded {
        next_cursor: Option<String>,
    },
    Error {
        message: String,
        cursor: Option<String>,
    },
}

#[derive(Debug, Clone)]
pub struct TreeNode {
    pub entry: WorkspaceEntry,
    pub children: Vec<String>,
    pub load: DirectoryLoadState,
    pub stale: bool,
    pub has_loaded: bool,
}

impl TreeNode {
    fn new(entry: WorkspaceEntry) -> Self {
        let load = if entry.kind == WorkspaceEntryKind::Directory {
            DirectoryLoadState::Unloaded
        } else {
            DirectoryLoadState::Loaded { next_cursor: None }
        };
        Self {
            entry,
            children: Vec::new(),
            load,
            stale: false,
            has_loaded: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VisibleRowKind {
    Entry,
    Loading { directory: String },
    Empty { directory: String },
    Error { directory: String, message: String },
    LoadMore { directory: String, cursor: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisibleTreeRow {
    pub path: String,
    pub depth: usize,
    pub kind: VisibleRowKind,
}

impl VisibleTreeRow {
    pub fn selectable(&self) -> bool {
        matches!(
            self.kind,
            VisibleRowKind::Entry | VisibleRowKind::LoadMore { .. }
        )
    }
}

#[derive(Debug, Clone)]
pub struct FileTreeModel {
    nodes: HashMap<String, TreeNode>,
    expanded: HashSet<String>,
    visible_rows: Vec<VisibleTreeRow>,
    /// The keyboard cursor; it need not be part of `selection`.
    selected: Option<String>,
    selection: HashSet<String>,
    /// Where Shift ranges start; follows plain and Ctrl/Cmd clicks.
    anchor: Option<String>,
    include_ignored: bool,
    generation: u64,
    listing_children: HashMap<String, HashSet<String>>,
}

impl Default for FileTreeModel {
    fn default() -> Self {
        Self::new()
    }
}

impl FileTreeModel {
    pub fn new() -> Self {
        Self::with_include_ignored(false)
    }

    pub fn with_include_ignored(include_ignored: bool) -> Self {
        let mut nodes = HashMap::new();
        nodes.insert(String::new(), TreeNode::new(root_entry()));
        Self {
            nodes,
            expanded: HashSet::from([String::new()]),
            visible_rows: Vec::new(),
            selected: None,
            selection: HashSet::new(),
            anchor: None,
            include_ignored,
            generation: 0,
            listing_children: HashMap::new(),
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn include_ignored(&self) -> bool {
        self.include_ignored
    }

    pub fn visible_rows(&self) -> &[VisibleTreeRow] {
        &self.visible_rows
    }

    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    pub fn is_selected(&self, path: &str) -> bool {
        self.selection.contains(path)
    }

    pub fn selection_len(&self) -> usize {
        self.selection.len()
    }

    /// Selected entries in visible order.
    pub fn selected_paths(&self) -> Vec<String> {
        self.visible_rows
            .iter()
            .filter(|row| row.kind == VisibleRowKind::Entry && self.selection.contains(&row.path))
            .map(|row| row.path.clone())
            .collect()
    }

    /// Selected entries without those already covered by a selected ancestor.
    pub fn selection_roots(&self) -> Vec<String> {
        self.selected_paths()
            .into_iter()
            .filter(|path| {
                !self
                    .selection
                    .iter()
                    .any(|other| is_descendant(path, other))
            })
            .collect()
    }

    pub fn node(&self, path: &str) -> Option<&TreeNode> {
        self.nodes.get(path)
    }

    pub fn is_expanded(&self, path: &str) -> bool {
        self.expanded.contains(path)
    }

    pub fn expanded_directories(&self) -> Vec<String> {
        let mut paths = self
            .expanded
            .iter()
            .filter(|path| !path.is_empty())
            .cloned()
            .collect::<Vec<_>>();
        paths.sort_by_key(|path| (path.matches('/').count(), path.clone()));
        paths
    }

    pub fn is_directory_loaded(&self, path: &str) -> bool {
        self.nodes.get(path).is_some_and(|node| {
            node.entry.kind == WorkspaceEntryKind::Directory
                && matches!(node.load, DirectoryLoadState::Loaded { .. })
        })
    }

    pub fn reset(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.nodes.clear();
        self.listing_children.clear();
        self.nodes
            .insert(String::new(), TreeNode::new(root_entry()));
        self.expanded.clear();
        self.expanded.insert(String::new());
        self.visible_rows.clear();
        self.set_single(None);
        self.generation
    }

    pub fn set_include_ignored(&mut self, include_ignored: bool) -> bool {
        if self.include_ignored == include_ignored {
            return false;
        }
        self.include_ignored = include_ignored;
        self.reset();
        true
    }

    pub fn begin_load(&mut self, directory: &str, cursor: Option<String>, generation: u64) -> bool {
        if generation != self.generation {
            return false;
        }
        let Some(node) = self.nodes.get_mut(directory) else {
            return false;
        };
        if matches!(node.load, DirectoryLoadState::Loading { .. }) {
            return false;
        }
        node.load = DirectoryLoadState::Loading { cursor };
        node.stale = false;
        self.rebuild_visible_rows();
        true
    }

    pub fn fail_load(
        &mut self,
        directory: &str,
        cursor: Option<String>,
        message: impl Into<String>,
        generation: u64,
    ) -> bool {
        if generation != self.generation {
            return false;
        }
        let Some(node) = self.nodes.get_mut(directory) else {
            return false;
        };
        node.load = DirectoryLoadState::Error {
            message: message.into(),
            cursor,
        };
        self.rebuild_visible_rows();
        true
    }

    pub fn apply_page(&mut self, page: WorkspaceDirectoryPage, generation: u64) -> bool {
        if generation != self.generation {
            return false;
        }
        let directory = page.directory;
        let Some(parent) = self.nodes.get(&directory) else {
            return false;
        };
        if parent.entry.kind != WorkspaceEntryKind::Directory {
            return false;
        }
        let parent_ignored = parent.entry.ignored;

        let append = matches!(parent.load, DirectoryLoadState::Loading { cursor: Some(_) });
        if !append {
            self.listing_children
                .insert(directory.clone(), HashSet::new());
        }
        let seen = self.listing_children.entry(directory.clone()).or_default();
        seen.extend(
            page.entries
                .iter()
                .filter(|entry| is_direct_child(&entry.path, &directory))
                .map(|entry| entry.path.clone()),
        );
        if page.next_cursor.is_none() {
            let incoming = self.listing_children.remove(&directory).unwrap_or_default();
            let previous = parent.children.clone();
            for child in previous {
                // A partial listing cannot establish that an unseen child was deleted.
                if !incoming.contains(&child) {
                    self.remove_subtree(&child);
                }
            }
            self.nodes
                .get_mut(&directory)
                .unwrap()
                .children
                .retain(|path| incoming.contains(path));
        }

        let mut known_children = self
            .nodes
            .get(&directory)
            .map(|node| node.children.iter().cloned().collect::<HashSet<_>>())
            .unwrap_or_default();
        for mut entry in page.entries {
            if !is_direct_child(&entry.path, &directory) {
                continue;
            }
            entry.ignored |= parent_ignored;
            let path = entry.path.clone();
            if self
                .nodes
                .get(&path)
                .is_some_and(|node| node.entry.kind != entry.kind)
            {
                self.remove_subtree(&path);
            }
            self.nodes
                .entry(path.clone())
                .and_modify(|node| node.entry = entry.clone())
                .or_insert_with(|| TreeNode::new(entry));
            if let Some(parent) = self.nodes.get_mut(&directory)
                && known_children.insert(path.clone())
            {
                parent.children.push(path);
            }
        }

        let mut children = self
            .nodes
            .get(&directory)
            .map(|node| node.children.clone())
            .unwrap_or_default();
        children.sort_by(|left, right| compare_paths(&self.nodes, left, right));
        if let Some(parent) = self.nodes.get_mut(&directory) {
            parent.children = children;
            parent.load = DirectoryLoadState::Loaded {
                next_cursor: page.next_cursor,
            };
            parent.stale = false;
            parent.has_loaded = true;
        }
        self.rebuild_visible_rows();
        true
    }

    pub fn toggle_expanded(&mut self, path: &str) -> bool {
        let Some(node) = self.nodes.get(path) else {
            return false;
        };
        if node.entry.kind != WorkspaceEntryKind::Directory {
            return false;
        }
        if !self.expanded.remove(path) {
            self.expanded.insert(path.to_string());
        } else if self
            .selected
            .as_deref()
            .is_some_and(|selected| is_descendant(selected, path))
        {
            let folder = (!path.is_empty()).then(|| path.to_string());
            if let Some(folder) = &folder {
                self.selection.insert(folder.clone());
            }
            self.selected = folder;
        }
        self.rebuild_visible_rows();
        true
    }

    pub fn expand(&mut self, path: &str) -> bool {
        if self
            .nodes
            .get(path)
            .is_none_or(|node| node.entry.kind != WorkspaceEntryKind::Directory)
        {
            return false;
        }
        let changed = self.expanded.insert(path.to_string());
        if changed {
            self.rebuild_visible_rows();
        }
        changed
    }

    pub fn select(&mut self, path: impl Into<String>) -> bool {
        let path = path.into();
        if !self.nodes.contains_key(&path) {
            return false;
        }
        self.set_single(Some(path));
        true
    }

    /// Ctrl/Cmd+click: flip one entry without disturbing the rest.
    pub fn toggle_selected(&mut self, path: impl Into<String>) -> bool {
        let path = path.into();
        if path.is_empty() || !self.nodes.contains_key(&path) {
            return false;
        }
        if !self.selection.remove(&path) {
            self.selection.insert(path.clone());
        }
        self.anchor = Some(path.clone());
        self.selected = Some(path);
        true
    }

    /// Shift+click: select the visible entries between the anchor and `path`.
    pub fn select_range_to(&mut self, path: &str, additive: bool) -> bool {
        let entries = self
            .visible_rows
            .iter()
            .filter(|row| row.kind == VisibleRowKind::Entry)
            .map(|row| row.path.as_str())
            .collect::<Vec<_>>();
        let Some(end) = entries.iter().position(|candidate| *candidate == path) else {
            return false;
        };
        let start = self
            .anchor
            .as_deref()
            .or(self.selected.as_deref())
            .and_then(|anchor| entries.iter().position(|candidate| *candidate == anchor))
            .unwrap_or(end);
        let range = entries[start.min(end)..=start.max(end)]
            .iter()
            .map(|path| path.to_string())
            .collect::<Vec<_>>();
        if !additive {
            self.selection.clear();
        }
        self.selection.extend(range);
        self.anchor = Some(entries[start].to_string());
        self.selected = Some(path.to_string());
        true
    }

    /// Shift+arrow: move the cursor to the next entry and extend the range.
    pub fn extend_selection(&mut self, delta: isize) -> Option<&str> {
        let entries = self
            .visible_rows
            .iter()
            .filter(|row| row.kind == VisibleRowKind::Entry)
            .map(|row| row.path.clone())
            .collect::<Vec<_>>();
        let current = self
            .selected
            .as_deref()
            .and_then(|path| entries.iter().position(|candidate| candidate == path));
        let next = match current {
            Some(index) => index
                .saturating_add_signed(delta)
                .min(entries.len().checked_sub(1)?),
            None if delta.is_negative() => entries.len().checked_sub(1)?,
            None => 0,
        };
        let target = entries.get(next)?.clone();
        if self.anchor.is_none() {
            self.anchor = Some(entries[current.unwrap_or(next)].clone());
        }
        self.select_range_to(&target, false);
        self.selected.as_deref()
    }

    /// A click on empty space: nothing stays selected. The cursor goes too,
    /// so Delete or F2 cannot act on a row that no longer looks selected.
    pub fn clear_selection(&mut self) -> bool {
        if self.selection.is_empty() && self.selected.is_none() {
            return false;
        }
        self.set_single(None);
        true
    }

    /// Escape: keep only the cursor.
    pub fn collapse_selection(&mut self) -> bool {
        if self.selection.len() <= 1
            && self
                .selected
                .as_ref()
                .is_none_or(|path| self.selection.contains(path))
        {
            return false;
        }
        self.set_single(self.selected.clone());
        true
    }

    pub fn select_next(&mut self) -> Option<&str> {
        self.move_selection(1)
    }

    pub fn select_previous(&mut self) -> Option<&str> {
        self.move_selection(-1)
    }

    pub fn select_parent(&mut self) -> Option<&str> {
        let selected = self.selected.clone()?;
        let parent = parent_path(&selected)?;
        if parent.is_empty() {
            return self.selected.as_deref();
        }
        self.set_single(Some(parent));
        self.selected.as_deref()
    }

    pub fn select_first_child(&mut self) -> Option<&str> {
        let selected = self.selected.clone()?;
        let first = self.nodes.get(&selected)?.children.first()?.clone();
        self.set_single(Some(first));
        self.selected.as_deref()
    }

    pub fn invalidate_directory(&mut self, directory: &str) -> bool {
        let Some(node) = self.nodes.get_mut(directory) else {
            return false;
        };
        if node.entry.kind != WorkspaceEntryKind::Directory {
            return false;
        }
        node.stale = true;
        true
    }

    pub fn invalidate_all_directories(&mut self) {
        for node in self.nodes.values_mut() {
            if node.entry.kind == WorkspaceEntryKind::Directory {
                node.stale = true;
            }
        }
    }

    /// Re-key a confirmed move without throwing away expansion or selection.
    pub fn relocate_subtree(
        &mut self,
        old: &str,
        new: &str,
        entry: Option<WorkspaceEntry>,
    ) -> bool {
        if old.is_empty() || old == new || is_descendant(new, old) {
            return false;
        }
        let remap = |path: &str| -> String {
            if path == old {
                new.to_string()
            } else if is_descendant(path, old) {
                format!("{new}{}", &path[old.len()..])
            } else {
                path.to_string()
            }
        };
        if !self.nodes.contains_key(old) {
            return false;
        }
        let old_parent = parent_path(old).unwrap();
        let new_parent = parent_path(new).unwrap();
        if let Some(parent) = self.nodes.get_mut(&old_parent) {
            parent.children.retain(|p| p != old);
        }
        let nodes = std::mem::take(&mut self.nodes);
        for (path, mut node) in nodes {
            let next = remap(&path);
            node.entry.path = next.clone();
            if path == old {
                node.entry.name = new.rsplit('/').next().unwrap_or(new).into();
            }
            node.children = node.children.iter().map(|p| remap(p)).collect();
            if is_descendant(&path, old) || path == old {
                node.stale = true;
                if node.entry.kind == WorkspaceEntryKind::Directory {
                    node.load = DirectoryLoadState::Unloaded;
                }
            }
            self.nodes.insert(next, node);
        }
        if let Some(entry) = entry {
            if let Some(node) = self.nodes.get_mut(new) {
                node.entry = entry;
            }
        }
        self.expanded = self.expanded.iter().map(|p| remap(p)).collect();
        self.selected = self.selected.as_deref().map(remap);
        self.selection = self.selection.iter().map(|p| remap(p)).collect();
        self.anchor = self.anchor.as_deref().map(remap);
        let mut ancestor = Some(new_parent.clone());
        while let Some(path) = ancestor {
            self.expanded.insert(path.clone());
            ancestor = parent_path(&path);
        }
        self.listing_children.clear();
        if let Some(parent) = self.nodes.get_mut(&new_parent) {
            if !parent.children.iter().any(|p| p == new) {
                parent.children.push(new.into());
            }
        }
        for parent in [&old_parent, &new_parent] {
            if let Some(node) = self.nodes.get_mut(parent) {
                node.stale = true;
                node.load = DirectoryLoadState::Unloaded;
                let mut children = std::mem::take(&mut node.children);
                children.sort_by(|a, b| compare_paths(&self.nodes, a, b));
                self.nodes.get_mut(parent).unwrap().children = children;
            }
        }
        self.rebuild_visible_rows();
        true
    }

    /// Invalidate asynchronous listings without resetting the visible tree.
    pub fn invalidate_loads(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.listing_children.clear();
        for node in self.nodes.values_mut() {
            if matches!(node.load, DirectoryLoadState::Loading { .. }) {
                node.load = DirectoryLoadState::Unloaded;
                node.stale = true;
            }
        }
    }

    pub fn remove(&mut self, path: &str) -> bool {
        if path.is_empty() || !self.nodes.contains_key(path) {
            return false;
        }
        if let Some(parent) = parent_path(path).and_then(|path| self.nodes.get_mut(&path)) {
            parent.children.retain(|child| child != path);
        }
        self.remove_subtree(path);
        if self
            .selected
            .as_deref()
            .is_some_and(|selected| selected == path || is_descendant(selected, path))
        {
            let parent = parent_path(path).filter(|parent| !parent.is_empty());
            self.selected = parent.clone();
            if let Some(parent) = parent
                && self
                    .selection
                    .iter()
                    .all(|selected| contains(path, selected))
            {
                self.selection.insert(parent);
            }
        }
        self.rebuild_visible_rows();
        true
    }

    pub fn reveal_search_match(&mut self, result: &WorkspaceFileSearchMatch) -> Vec<String> {
        let mut ancestors = Vec::new();
        let mut current = parent_path(&result.path);
        while let Some(path) = current {
            if path.is_empty() {
                break;
            }
            ancestors.push(path.clone());
            current = parent_path(&path);
        }
        ancestors.reverse();
        ancestors
    }

    fn move_selection(&mut self, delta: isize) -> Option<&str> {
        let selectable = self
            .visible_rows
            .iter()
            .filter(|row| row.selectable())
            .map(|row| row.path.as_str())
            .collect::<Vec<_>>();
        if selectable.is_empty() {
            self.set_single(None);
            return None;
        }
        let current = self
            .selected
            .as_deref()
            .and_then(|path| selectable.iter().position(|candidate| *candidate == path));
        let next = match (current, delta.is_negative()) {
            (Some(index), true) => index.saturating_sub(delta.unsigned_abs()),
            (Some(index), false) => (index + delta as usize).min(selectable.len() - 1),
            (None, true) => selectable.len() - 1,
            (None, false) => 0,
        };
        self.set_single(Some(selectable[next].to_string()));
        self.selected.as_deref()
    }

    fn set_single(&mut self, path: Option<String>) {
        self.selection.clear();
        self.selection.extend(path.clone());
        self.anchor = path.clone();
        self.selected = path;
    }

    fn remove_subtree(&mut self, path: &str) {
        let children = self
            .nodes
            .get(path)
            .map(|node| node.children.clone())
            .unwrap_or_default();
        for child in children {
            self.remove_subtree(&child);
        }
        self.nodes.remove(path);
        self.listing_children.remove(path);
        self.expanded.remove(path);
    }

    fn rebuild_visible_rows(&mut self) {
        let mut rows = Vec::new();
        let mut visited = HashSet::from([String::new()]);
        self.append_directory_rows("", 0, &mut rows, &mut visited);
        self.visible_rows = rows;
        if self
            .selected
            .as_deref()
            .is_some_and(|selected| !self.visible_rows.iter().any(|row| row.path == selected))
        {
            self.selected = None;
        }
        // Like VS Code, entries hidden by a collapse or removal leave the selection.
        let visible = self
            .visible_rows
            .iter()
            .filter(|row| row.kind == VisibleRowKind::Entry)
            .map(|row| row.path.as_str())
            .collect::<HashSet<_>>();
        self.selection
            .retain(|path| visible.contains(path.as_str()));
        if self
            .anchor
            .as_deref()
            .is_some_and(|anchor| !visible.contains(anchor))
        {
            self.anchor = None;
        }
    }

    fn append_directory_rows(
        &self,
        directory: &str,
        depth: usize,
        rows: &mut Vec<VisibleTreeRow>,
        visited: &mut HashSet<String>,
    ) {
        let Some(node) = self.nodes.get(directory) else {
            return;
        };
        for child_path in &node.children {
            if !visited.insert(child_path.clone()) {
                continue;
            }
            let Some(child) = self.nodes.get(child_path) else {
                continue;
            };
            rows.push(VisibleTreeRow {
                path: child_path.clone(),
                depth,
                kind: VisibleRowKind::Entry,
            });
            if child.entry.kind == WorkspaceEntryKind::Directory
                && self.expanded.contains(child_path)
            {
                self.append_directory_rows(child_path, depth + 1, rows, visited);
            }
        }

        if !self.expanded.contains(directory) {
            return;
        }
        match &node.load {
            DirectoryLoadState::Unloaded => {}
            DirectoryLoadState::Loading { cursor: None } if node.has_loaded => {
                if node.children.is_empty() {
                    rows.push(VisibleTreeRow {
                        path: synthetic_path(directory, "empty"),
                        depth,
                        kind: VisibleRowKind::Empty {
                            directory: directory.to_string(),
                        },
                    });
                }
            }
            DirectoryLoadState::Loading { .. } => rows.push(VisibleTreeRow {
                path: synthetic_path(directory, "loading"),
                depth,
                kind: VisibleRowKind::Loading {
                    directory: directory.to_string(),
                },
            }),
            DirectoryLoadState::Loaded { next_cursor } => {
                if node.children.is_empty() {
                    rows.push(VisibleTreeRow {
                        path: synthetic_path(directory, "empty"),
                        depth,
                        kind: VisibleRowKind::Empty {
                            directory: directory.to_string(),
                        },
                    });
                }
                if let Some(cursor) = next_cursor {
                    rows.push(VisibleTreeRow {
                        path: synthetic_path(directory, "more"),
                        depth,
                        kind: VisibleRowKind::LoadMore {
                            directory: directory.to_string(),
                            cursor: cursor.clone(),
                        },
                    });
                }
            }
            DirectoryLoadState::Error { message, .. } => rows.push(VisibleTreeRow {
                path: synthetic_path(directory, "error"),
                depth,
                kind: VisibleRowKind::Error {
                    directory: directory.to_string(),
                    message: message.clone(),
                },
            }),
        }
    }
}

fn root_entry() -> WorkspaceEntry {
    WorkspaceEntry {
        mutation_revision: None,
        path: String::new(),
        name: String::new(),
        kind: WorkspaceEntryKind::Directory,
        size: None,
        modified_at: None,
        ignored: false,
        read_only: false,
    }
}

fn compare_paths(nodes: &HashMap<String, TreeNode>, left: &str, right: &str) -> std::cmp::Ordering {
    let left = &nodes[left].entry;
    let right = &nodes[right].entry;
    entry_rank(left.kind)
        .cmp(&entry_rank(right.kind))
        .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
        .then_with(|| left.path.cmp(&right.path))
}

fn entry_rank(kind: WorkspaceEntryKind) -> u8 {
    match kind {
        WorkspaceEntryKind::Directory => 0,
        WorkspaceEntryKind::File => 1,
        WorkspaceEntryKind::Symlink => 2,
    }
}

pub fn parent_path(path: &str) -> Option<String> {
    path.rsplit_once('/')
        .map(|(parent, _)| parent.to_string())
        .or_else(|| (!path.is_empty()).then(String::new))
}

fn is_descendant(candidate: &str, ancestor: &str) -> bool {
    if ancestor.is_empty() {
        return !candidate.is_empty();
    }
    candidate
        .strip_prefix(ancestor)
        .is_some_and(|suffix| suffix.starts_with('/'))
}

fn contains(ancestor: &str, path: &str) -> bool {
    path == ancestor || is_descendant(path, ancestor)
}

fn is_direct_child(candidate: &str, directory: &str) -> bool {
    parent_path(candidate).as_deref() == Some(directory)
}

fn synthetic_path(directory: &str, kind: &str) -> String {
    format!("{directory}\0{kind}")
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;

    fn entry(path: &str, kind: WorkspaceEntryKind) -> WorkspaceEntry {
        WorkspaceEntry {
            mutation_revision: None,
            path: path.into(),
            name: path.rsplit('/').next().unwrap_or(path).into(),
            kind,
            size: (kind == WorkspaceEntryKind::File).then_some(12),
            modified_at: Some(Utc::now()),
            ignored: false,
            read_only: kind == WorkspaceEntryKind::Symlink,
        }
    }

    fn page(
        directory: &str,
        entries: Vec<WorkspaceEntry>,
        next_cursor: Option<&str>,
    ) -> WorkspaceDirectoryPage {
        WorkspaceDirectoryPage {
            checkout_id: None,
            mutation_capabilities: None,
            directory: directory.into(),
            entries,
            next_cursor: next_cursor.map(str::to_string),
            truncated: next_cursor.is_some(),
        }
    }

    #[test]
    fn relocation_keeps_descendants_selection_and_expansion() {
        let mut tree = FileTreeModel::new();
        tree.begin_load("", None, tree.generation());
        tree.apply_page(
            page(
                "",
                vec![
                    entry("a", WorkspaceEntryKind::Directory),
                    entry("b", WorkspaceEntryKind::Directory),
                ],
                None,
            ),
            tree.generation(),
        );
        tree.expand("a");
        tree.begin_load("a", None, tree.generation());
        tree.apply_page(
            page("a", vec![entry("a/child", WorkspaceEntryKind::File)], None),
            tree.generation(),
        );
        tree.select("a/child");
        assert!(tree.relocate_subtree("a", "b/a", None));
        assert!(tree.is_expanded("b/a"));
        assert_eq!(tree.selected(), Some("b/a/child"));
        assert!(tree.node("a").is_none());
        assert!(tree.node("b/a/child").is_some());
        assert!(!tree.relocate_subtree("a", "b/a", None));
    }

    #[test]
    fn background_refresh_keeps_loaded_rows_even_when_empty_or_failed() {
        let mut tree = FileTreeModel::new();
        tree.apply_page(page("", vec![], None), 0);
        let empty = tree.visible_rows().to_vec();
        tree.begin_load("", None, 0);
        assert_eq!(tree.visible_rows(), empty);
        assert!(tree.node("").unwrap().has_loaded);
        tree.apply_page(
            page("", vec![entry("src", WorkspaceEntryKind::Directory)], None),
            0,
        );
        let loaded = tree.visible_rows().to_vec();
        tree.begin_load("", None, 0);
        assert_eq!(tree.visible_rows(), loaded);
        tree.fail_load("", None, "offline", 0);
        assert_eq!(tree.visible_rows()[0].path, "src");
        assert!(tree.node("").unwrap().has_loaded);
    }

    #[test]
    fn changes_during_load_are_remembered_and_collapsed_caches_become_stale() {
        let mut tree = FileTreeModel::new();
        tree.apply_page(
            page("", vec![entry("src", WorkspaceEntryKind::Directory)], None),
            0,
        );
        tree.begin_load("", None, 0);
        tree.invalidate_all_directories();
        assert!(tree.node("").unwrap().stale);
        assert!(tree.node("src").unwrap().stale);
        assert!(!tree.begin_load("", None, 0));
        assert!(tree.node("").unwrap().stale);
        tree.apply_page(
            page("", vec![entry("src", WorkspaceEntryKind::Directory)], None),
            0,
        );
        assert!(tree.node("src").unwrap().stale);
        tree.expand("src");
        assert!(tree.begin_load("src", None, 0));
        assert!(!tree.node("src").unwrap().stale);
    }

    #[test]
    fn refreshing_a_directory_preserves_descendants_expansion_and_selection() {
        let mut tree = FileTreeModel::new();
        tree.apply_page(
            page("", vec![entry("src", WorkspaceEntryKind::Directory)], None),
            0,
        );
        tree.expand("src");
        tree.apply_page(
            page(
                "src",
                vec![entry("src/lib.rs", WorkspaceEntryKind::File)],
                None,
            ),
            0,
        );
        tree.select("src/lib.rs");
        tree.begin_load("", None, 0);
        assert!(
            tree.visible_rows()
                .iter()
                .any(|row| row.path == "src/lib.rs")
        );
        tree.apply_page(
            page("", vec![entry("src", WorkspaceEntryKind::Directory)], None),
            0,
        );
        assert!(tree.is_expanded("src"));
        assert!(tree.is_directory_loaded("src"));
        assert_eq!(tree.selected(), Some("src/lib.rs"));
    }

    #[test]
    fn paginated_refresh_prunes_missing_children_only_after_the_last_page() {
        let mut tree = FileTreeModel::new();
        tree.apply_page(
            page(
                "",
                vec![
                    entry("a", WorkspaceEntryKind::Directory),
                    entry("z", WorkspaceEntryKind::Directory),
                    entry("removed", WorkspaceEntryKind::File),
                ],
                None,
            ),
            0,
        );
        tree.expand("z");
        tree.apply_page(
            page("z", vec![entry("z/child", WorkspaceEntryKind::File)], None),
            0,
        );
        tree.select("z/child");
        tree.begin_load("", None, 0);
        tree.apply_page(
            page(
                "",
                vec![entry("a", WorkspaceEntryKind::Directory)],
                Some("next"),
            ),
            0,
        );
        assert!(tree.node("z/child").is_some());
        assert!(tree.node("removed").is_some());
        tree.begin_load("", Some("next".into()), 0);
        tree.apply_page(
            page("", vec![entry("z", WorkspaceEntryKind::Directory)], None),
            0,
        );
        assert!(tree.node("a").is_some());
        assert!(tree.node("removed").is_none());
        assert!(tree.is_expanded("z"));
        assert_eq!(tree.selected(), Some("z/child"));
    }

    #[test]
    fn refresh_removes_deleted_subtrees_and_handles_directory_becoming_file() {
        let mut tree = FileTreeModel::new();
        tree.apply_page(
            page("", vec![entry("src", WorkspaceEntryKind::Directory)], None),
            0,
        );
        tree.expand("src");
        tree.apply_page(
            page(
                "src",
                vec![entry("src/lib.rs", WorkspaceEntryKind::File)],
                None,
            ),
            0,
        );
        tree.select("src/lib.rs");
        tree.begin_load("", None, 0);
        tree.apply_page(
            page("", vec![entry("src", WorkspaceEntryKind::File)], None),
            0,
        );
        assert!(tree.node("src/lib.rs").is_none());
        assert!(!tree.is_expanded("src"));
        assert_eq!(tree.selected(), None);
        assert_eq!(
            tree.node("src").unwrap().entry.kind,
            WorkspaceEntryKind::File
        );
        tree.begin_load("", None, 0);
        tree.apply_page(page("", vec![], None), 0);
        assert!(tree.node("src").is_none());
    }

    #[test]
    fn root_page_is_sorted_and_flattened() {
        let mut tree = FileTreeModel::new();
        let generation = tree.generation();
        tree.begin_load("", None, generation);
        tree.apply_page(
            page(
                "",
                vec![
                    entry("z.rs", WorkspaceEntryKind::File),
                    entry("src", WorkspaceEntryKind::Directory),
                    entry("a.rs", WorkspaceEntryKind::File),
                    entry("link", WorkspaceEntryKind::Symlink),
                ],
                None,
            ),
            generation,
        );
        assert_eq!(
            tree.visible_rows
                .iter()
                .filter(|row| matches!(row.kind, VisibleRowKind::Entry))
                .map(|row| row.path.as_str())
                .collect::<Vec<_>>(),
            ["src", "a.rs", "z.rs", "link"]
        );
    }

    #[test]
    fn expanded_directories_add_rows_at_the_correct_depth() {
        let mut tree = FileTreeModel::new();
        let generation = tree.generation();
        tree.begin_load("", None, generation);
        tree.apply_page(
            page("", vec![entry("src", WorkspaceEntryKind::Directory)], None),
            generation,
        );
        tree.expand("src");
        tree.begin_load("src", None, generation);
        tree.apply_page(
            page(
                "src",
                vec![entry("src/lib.rs", WorkspaceEntryKind::File)],
                None,
            ),
            generation,
        );
        let row = tree
            .visible_rows
            .iter()
            .find(|row| row.path == "src/lib.rs")
            .unwrap();
        assert_eq!(row.depth, 1);
    }

    #[test]
    fn ignored_directories_propagate_ignored_state_to_all_descendants() {
        let mut tree = FileTreeModel::new();
        let generation = tree.generation();
        let mut ignored_directory = entry("target", WorkspaceEntryKind::Directory);
        ignored_directory.ignored = true;

        tree.begin_load("", None, generation);
        tree.apply_page(page("", vec![ignored_directory], None), generation);
        tree.begin_load("target", None, generation);
        tree.apply_page(
            page(
                "target",
                vec![
                    entry("target/cache", WorkspaceEntryKind::Directory),
                    entry("target/output.bin", WorkspaceEntryKind::File),
                ],
                None,
            ),
            generation,
        );
        tree.begin_load("target/cache", None, generation);
        tree.apply_page(
            page(
                "target/cache",
                vec![entry("target/cache/nested.bin", WorkspaceEntryKind::File)],
                None,
            ),
            generation,
        );

        assert!(tree.node("target/cache").unwrap().entry.ignored);
        assert!(tree.node("target/output.bin").unwrap().entry.ignored);
        assert!(tree.node("target/cache/nested.bin").unwrap().entry.ignored);
    }

    #[test]
    fn collapsing_an_ancestor_moves_hidden_selection_to_it() {
        let mut tree = FileTreeModel::new();
        let generation = tree.generation();
        tree.begin_load("", None, generation);
        tree.apply_page(
            page("", vec![entry("src", WorkspaceEntryKind::Directory)], None),
            generation,
        );
        tree.expand("src");
        tree.begin_load("src", None, generation);
        tree.apply_page(
            page(
                "src",
                vec![entry("src/lib.rs", WorkspaceEntryKind::File)],
                None,
            ),
            generation,
        );
        tree.select("src/lib.rs");
        tree.toggle_expanded("src");
        assert_eq!(tree.selected(), Some("src"));
    }

    #[test]
    fn pagination_appends_and_deduplicates_entries() {
        let mut tree = FileTreeModel::new();
        let generation = tree.generation();
        tree.begin_load("", None, generation);
        tree.apply_page(
            page(
                "",
                vec![entry("a.rs", WorkspaceEntryKind::File)],
                Some("next"),
            ),
            generation,
        );
        tree.begin_load("", Some("next".into()), generation);
        tree.apply_page(
            page(
                "",
                vec![
                    entry("a.rs", WorkspaceEntryKind::File),
                    entry("b.rs", WorkspaceEntryKind::File),
                ],
                None,
            ),
            generation,
        );
        let root = tree.node("").unwrap();
        assert_eq!(root.children, ["a.rs", "b.rs"]);
    }

    #[test]
    fn directory_pages_reject_self_parents_and_ancestors() {
        let mut tree = FileTreeModel::new();
        let generation = tree.generation();
        tree.begin_load("", None, generation);
        tree.apply_page(
            page("", vec![entry("src", WorkspaceEntryKind::Directory)], None),
            generation,
        );
        tree.begin_load("src", None, generation);

        assert!(tree.apply_page(
            page(
                "src",
                vec![
                    entry("src", WorkspaceEntryKind::Directory),
                    entry("", WorkspaceEntryKind::Directory),
                    entry("src/lib.rs", WorkspaceEntryKind::File),
                ],
                None,
            ),
            generation,
        ));

        assert_eq!(tree.node("src").unwrap().children, ["src/lib.rs"]);
        assert_eq!(tree.node("").unwrap().children, ["src"]);
    }

    #[test]
    fn directory_pages_reject_non_direct_descendants() {
        let mut tree = FileTreeModel::new();
        let generation = tree.generation();
        tree.begin_load("", None, generation);

        assert!(tree.apply_page(
            page(
                "",
                vec![
                    entry("src", WorkspaceEntryKind::Directory),
                    entry("src/nested.rs", WorkspaceEntryKind::File),
                    entry("README.md", WorkspaceEntryKind::File),
                ],
                None,
            ),
            generation,
        ));

        assert_eq!(tree.node("").unwrap().children, ["src", "README.md"]);
        assert!(tree.node("src/nested.rs").is_none());
    }

    #[test]
    fn visible_rows_stop_at_cycles_in_a_corrupted_model() {
        let mut tree = FileTreeModel::new();
        tree.nodes.insert(
            "loop".into(),
            TreeNode::new(entry("loop", WorkspaceEntryKind::Directory)),
        );
        tree.nodes.get_mut("").unwrap().children.push("loop".into());
        tree.nodes
            .get_mut("loop")
            .unwrap()
            .children
            .push("loop".into());
        tree.expanded.insert("loop".into());

        tree.rebuild_visible_rows();

        assert_eq!(
            tree.visible_rows()
                .iter()
                .filter(|row| matches!(row.kind, VisibleRowKind::Entry))
                .map(|row| row.path.as_str())
                .collect::<Vec<_>>(),
            ["loop"]
        );
    }

    #[test]
    fn stale_generation_cannot_replace_the_tree() {
        let mut tree = FileTreeModel::new();
        let stale = tree.generation();
        tree.reset();
        assert!(!tree.apply_page(
            page("", vec![entry("stale.rs", WorkspaceEntryKind::File)], None),
            stale,
        ));
        assert!(tree.node("stale.rs").is_none());
    }

    #[test]
    fn removing_a_directory_removes_all_descendants() {
        let mut tree = FileTreeModel::new();
        let generation = tree.generation();
        tree.begin_load("", None, generation);
        tree.apply_page(
            page("", vec![entry("src", WorkspaceEntryKind::Directory)], None),
            generation,
        );
        tree.expand("src");
        tree.begin_load("src", None, generation);
        tree.apply_page(
            page(
                "src",
                vec![entry("src/lib.rs", WorkspaceEntryKind::File)],
                None,
            ),
            generation,
        );
        assert!(tree.remove("src"));
        assert!(tree.node("src").is_none());
        assert!(tree.node("src/lib.rs").is_none());
    }

    #[test]
    fn empty_error_and_load_more_rows_follow_load_state() {
        let mut tree = FileTreeModel::new();
        let generation = tree.generation();
        tree.begin_load("", None, generation);
        assert!(matches!(
            tree.visible_rows.last().map(|row| &row.kind),
            Some(VisibleRowKind::Loading { .. })
        ));
        tree.fail_load("", None, "offline", generation);
        assert!(matches!(
            tree.visible_rows.last().map(|row| &row.kind),
            Some(VisibleRowKind::Error { .. })
        ));
        tree.begin_load("", None, generation);
        tree.apply_page(page("", Vec::new(), None), generation);
        assert!(matches!(
            tree.visible_rows.last().map(|row| &row.kind),
            Some(VisibleRowKind::Empty { .. })
        ));
    }

    #[test]
    fn keyboard_selection_follows_visible_rows() {
        let mut tree = FileTreeModel::new();
        let generation = tree.generation();
        tree.begin_load("", None, generation);
        tree.apply_page(
            page(
                "",
                vec![
                    entry("src", WorkspaceEntryKind::Directory),
                    entry("README.md", WorkspaceEntryKind::File),
                ],
                None,
            ),
            generation,
        );
        assert_eq!(tree.select_next(), Some("src"));
        assert_eq!(tree.select_next(), Some("README.md"));
        assert_eq!(tree.select_previous(), Some("src"));
    }

    #[test]
    fn an_inflight_directory_load_is_not_duplicated() {
        let mut tree = FileTreeModel::new();
        let generation = tree.generation();
        assert!(tree.begin_load("", None, generation));
        assert!(!tree.begin_load("", None, generation));
    }

    #[test]
    fn a_large_monorepo_page_stays_flat_and_addressable() {
        let mut tree = FileTreeModel::new();
        let generation = tree.generation();
        tree.begin_load("", None, generation);
        let entries = (0..10_000)
            .map(|index| entry(&format!("package-{index:05}.rs"), WorkspaceEntryKind::File))
            .chain([
                entry("arquitectura rápida.md", WorkspaceEntryKind::File),
                entry("packages", WorkspaceEntryKind::Directory),
            ])
            .collect();
        assert!(tree.apply_page(page("", entries, None), generation));
        assert_eq!(tree.node("").unwrap().children.len(), 10_002);
        assert_eq!(tree.visible_rows().len(), 10_002);
        assert!(tree.node("arquitectura rápida.md").is_some());
        assert_eq!(tree.visible_rows()[0].path, "packages");
    }

    #[test]
    fn reset_discards_loaded_content_and_invalidates_inflight_pages() {
        let mut tree = FileTreeModel::new();
        let stale_generation = tree.generation();
        tree.begin_load("", None, stale_generation);
        tree.apply_page(
            page(
                "",
                vec![entry("secret.env", WorkspaceEntryKind::File)],
                None,
            ),
            stale_generation,
        );
        let current_generation = tree.reset();
        assert_ne!(current_generation, stale_generation);
        assert!(tree.node("secret.env").is_none());
        assert!(tree.visible_rows().is_empty());
        assert!(!tree.apply_page(
            page("", vec![entry("stale.env", WorkspaceEntryKind::File)], None),
            stale_generation,
        ));
    }

    fn selection_tree() -> FileTreeModel {
        let mut tree = FileTreeModel::new();
        tree.apply_page(
            page(
                "",
                vec![
                    entry("src", WorkspaceEntryKind::Directory),
                    entry("a.txt", WorkspaceEntryKind::File),
                    entry("b.txt", WorkspaceEntryKind::File),
                    entry("c.txt", WorkspaceEntryKind::File),
                ],
                None,
            ),
            0,
        );
        tree.expand("src");
        tree.apply_page(
            page(
                "src",
                vec![
                    entry("src/lib.rs", WorkspaceEntryKind::File),
                    entry("src/main.rs", WorkspaceEntryKind::File),
                ],
                None,
            ),
            0,
        );
        tree
    }

    #[test]
    fn toggles_and_ranges_follow_the_anchor() {
        let mut tree = selection_tree();
        tree.select("a.txt");
        assert_eq!(tree.selected_paths(), ["a.txt"]);
        tree.toggle_selected("c.txt");
        assert_eq!(tree.selected_paths(), ["a.txt", "c.txt"]);
        assert_eq!(tree.selected(), Some("c.txt"));
        tree.toggle_selected("a.txt");
        assert_eq!(tree.selected_paths(), ["c.txt"]);
        assert_eq!(tree.selected(), Some("a.txt"));

        // The last Ctrl/Cmd click anchors the range, upward included.
        tree.select_range_to("src/lib.rs", false);
        assert_eq!(
            tree.selected_paths(),
            ["src/lib.rs", "src/main.rs", "a.txt"]
        );
        // A new Shift target replaces the range from the same anchor.
        tree.select_range_to("b.txt", false);
        assert_eq!(tree.selected_paths(), ["a.txt", "b.txt"]);
        tree.toggle_selected("src");
        tree.select_range_to("src/lib.rs", true);
        assert_eq!(
            tree.selected_paths(),
            ["src", "src/lib.rs", "a.txt", "b.txt"]
        );
        assert_eq!(tree.selection_roots(), ["src", "a.txt", "b.txt"]);

        assert!(tree.collapse_selection());
        assert_eq!(tree.selected_paths(), ["src/lib.rs"]);
        assert!(!tree.collapse_selection());

        assert!(tree.clear_selection());
        assert!(tree.selected_paths().is_empty());
        assert_eq!(tree.selected(), None);
        assert!(!tree.clear_selection());
        // With no cursor or anchor, the next Shift target selects only itself.
        tree.select_range_to("b.txt", false);
        assert_eq!(tree.selected_paths(), ["b.txt"]);
    }

    #[test]
    fn shift_arrows_extend_and_shrink_from_the_anchor() {
        let mut tree = selection_tree();
        tree.select("a.txt");
        tree.extend_selection(1);
        tree.extend_selection(1);
        assert_eq!(tree.selected_paths(), ["a.txt", "b.txt", "c.txt"]);
        assert_eq!(tree.extend_selection(1), Some("c.txt"));
        tree.extend_selection(-1);
        tree.extend_selection(-1);
        tree.extend_selection(-1);
        assert_eq!(tree.selected_paths(), ["src/main.rs", "a.txt"]);
        tree.select_next();
        assert_eq!(tree.selected_paths(), ["a.txt"]);
    }

    #[test]
    fn hidden_removed_and_moved_entries_update_the_selection() {
        let mut tree = selection_tree();
        tree.select("src/lib.rs");
        tree.toggle_selected("src/main.rs");
        tree.toggle_selected("b.txt");
        tree.toggle_expanded("src");
        assert_eq!(tree.selected_paths(), ["b.txt"]);
        assert_eq!(tree.selected(), Some("b.txt"));
        // A hidden cursor moves to the collapsed folder, which joins the selection.
        tree.toggle_expanded("src");
        tree.toggle_selected("src/lib.rs");
        tree.toggle_expanded("src");
        assert_eq!(tree.selected_paths(), ["src", "b.txt"]);
        assert_eq!(tree.selected(), Some("src"));

        tree.toggle_expanded("src");
        tree.select("src/lib.rs");
        tree.toggle_selected("c.txt");
        tree.relocate_subtree("c.txt", "src/c.txt", None);
        assert_eq!(tree.selected_paths(), ["src/c.txt", "src/lib.rs"]);
        tree.remove("src/lib.rs");
        assert_eq!(tree.selected_paths(), ["src/c.txt"]);
        tree.remove("src");
        assert!(tree.selected_paths().is_empty());
        assert_eq!(tree.selection_len(), 0);
    }
}
