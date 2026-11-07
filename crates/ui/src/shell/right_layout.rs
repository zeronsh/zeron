//! The right pane's split layout: a tree of panes, each with its own tabs.
//!
//! A chat's surface host is a single pane until the user drags a tab onto
//! one of its edges. Only then does a [`SplitLayout`] exist for that chat;
//! when closing or moving tabs leaves one pane, the layout dissolves back
//! into the plain tab list. Pure and generic over the tab type so the tree
//! operations are tested without a window.

pub(super) type PaneId = u64;

/// Panes narrower or shorter than this would only show a truncated strip.
pub(super) const MIN_PANE_WIDTH: f32 = 200.0;
pub(super) const MIN_PANE_HEIGHT: f32 = 140.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Axis {
    /// Children laid out left to right.
    Row,
    /// Children laid out top to bottom.
    Column,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Edge {
    Left,
    Right,
    Top,
    Bottom,
}

impl Edge {
    fn axis(self) -> Axis {
        match self {
            Edge::Left | Edge::Right => Axis::Row,
            Edge::Top | Edge::Bottom => Axis::Column,
        }
    }

    fn trailing(self) -> bool {
        matches!(self, Edge::Right | Edge::Bottom)
    }
}

/// Where a tab dragged over a pane lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DropZone {
    /// Into the pane, as its last tab.
    Center,
    /// A new pane on that side of it.
    Edge(Edge),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Node {
    Pane(PaneId),
    Split {
        id: u64,
        axis: Axis,
        children: Vec<Node>,
        /// Fractions of the split's extent, summing to one.
        sizes: Vec<f32>,
    },
}

impl Node {
    fn leaves(&self, out: &mut Vec<PaneId>) {
        match self {
            Node::Pane(id) => out.push(*id),
            Node::Split { children, .. } => {
                for child in children {
                    child.leaves(out);
                }
            }
        }
    }

    fn first_leaf(&self) -> PaneId {
        match self {
            Node::Pane(id) => *id,
            Node::Split { children, .. } => children[0].first_leaf(),
        }
    }

    fn last_leaf(&self) -> PaneId {
        match self {
            Node::Pane(id) => *id,
            Node::Split { children, .. } => children[children.len() - 1].last_leaf(),
        }
    }

    fn rects(&self, rect: Rect, out: &mut Vec<(PaneId, Rect)>) {
        match self {
            Node::Pane(id) => out.push((*id, rect)),
            Node::Split {
                axis,
                children,
                sizes,
                ..
            } => {
                let mut offset = 0.0;
                for (child, size) in children.iter().zip(sizes) {
                    let child_rect = match axis {
                        Axis::Row => Rect {
                            x: rect.x + rect.w * offset,
                            w: rect.w * size,
                            ..rect
                        },
                        Axis::Column => Rect {
                            y: rect.y + rect.h * offset,
                            h: rect.h * size,
                            ..rect
                        },
                    };
                    child.rects(child_rect, out);
                    offset += size;
                }
            }
        }
    }

    /// Put `new` beside `target`. A parent already running along `axis`
    /// takes it as a sibling, halving the target's share; otherwise the
    /// target leaf becomes a two-pane split.
    fn insert_beside(
        &mut self,
        target: PaneId,
        new: PaneId,
        axis: Axis,
        trailing: bool,
        split_id: u64,
    ) -> bool {
        match self {
            Node::Pane(id) if *id == target => {
                let pair = if trailing {
                    vec![Node::Pane(target), Node::Pane(new)]
                } else {
                    vec![Node::Pane(new), Node::Pane(target)]
                };
                *self = Node::Split {
                    id: split_id,
                    axis,
                    children: pair,
                    sizes: vec![0.5, 0.5],
                };
                true
            }
            Node::Pane(_) => false,
            Node::Split {
                axis: own_axis,
                children,
                sizes,
                ..
            } => {
                if *own_axis == axis
                    && let Some(ix) = children
                        .iter()
                        .position(|child| *child == Node::Pane(target))
                {
                    let half = sizes[ix] / 2.0;
                    sizes[ix] = half;
                    let at = if trailing { ix + 1 } else { ix };
                    children.insert(at, Node::Pane(new));
                    sizes.insert(at, half);
                    return true;
                }
                children
                    .iter_mut()
                    .any(|child| child.insert_beside(target, new, axis, trailing, split_id))
            }
        }
    }

    /// Remove the leaf `target`, giving its share to the neighbour before it
    /// (or after it, when it was first). Returns the leaf that inherits the
    /// space, so focus can follow it.
    fn remove(&mut self, target: PaneId) -> Option<PaneId> {
        let Node::Split {
            children, sizes, ..
        } = self
        else {
            return None;
        };
        if let Some(ix) = children
            .iter()
            .position(|child| *child == Node::Pane(target))
        {
            let share = sizes.remove(ix);
            children.remove(ix);
            if children.is_empty() {
                return None;
            }
            let heir = if ix > 0 { ix - 1 } else { 0 };
            sizes[heir] += share;
            return Some(if ix > 0 {
                children[heir].last_leaf()
            } else {
                children[heir].first_leaf()
            });
        }
        children.iter_mut().find_map(|child| child.remove(target))
    }

    /// Collapse single-child splits and flatten a split nested in a parent
    /// running the same way (its sizes scale into the parent's).
    fn normalize(&mut self) {
        let Node::Split {
            axis,
            children,
            sizes,
            ..
        } = self
        else {
            return;
        };
        for child in children.iter_mut() {
            child.normalize();
        }
        let mut flat_children = Vec::with_capacity(children.len());
        let mut flat_sizes = Vec::with_capacity(sizes.len());
        for (child, size) in children.drain(..).zip(sizes.drain(..)) {
            match child {
                Node::Split {
                    axis: child_axis,
                    children: grand,
                    sizes: grand_sizes,
                    ..
                } if child_axis == *axis => {
                    for (g, s) in grand.into_iter().zip(grand_sizes) {
                        flat_children.push(g);
                        flat_sizes.push(s * size);
                    }
                }
                other => {
                    flat_children.push(other);
                    flat_sizes.push(size);
                }
            }
        }
        let total: f32 = flat_sizes.iter().sum();
        if total > 0.0 {
            for size in &mut flat_sizes {
                *size /= total;
            }
        }
        *children = flat_children;
        *sizes = flat_sizes;
        if children.len() == 1 {
            *self = children.pop().expect("one child");
        }
    }

    fn split_mut(&mut self, split: u64) -> Option<(&mut Vec<f32>, Axis)> {
        match self {
            Node::Pane(_) => None,
            Node::Split {
                id,
                axis,
                sizes,
                children,
            } => {
                if *id == split {
                    Some((sizes, *axis))
                } else {
                    children.iter_mut().find_map(|child| child.split_mut(split))
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Pane<T> {
    /// Strip order.
    pub tabs: Vec<T>,
    /// The tab this pane shows; `None` until one is picked, or after the
    /// picked tab closed (the caller resolves a fallback).
    pub active: Option<T>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct SplitLayout<T> {
    root: Node,
    panes: Vec<(PaneId, Pane<T>)>,
    focused: PaneId,
    next_id: u64,
}

impl<T: Copy + PartialEq> SplitLayout<T> {
    /// Split a single pane's `tabs` by moving `moved` to `edge`. `None` when
    /// that would leave the original pane empty.
    pub fn split_off(tabs: Vec<T>, active: Option<T>, moved: T, edge: Edge) -> Option<Self> {
        if tabs.len() < 2 || !tabs.contains(&moved) {
            return None;
        }
        let mut layout = Self {
            root: Node::Pane(1),
            panes: vec![(1, Pane { tabs, active })],
            focused: 1,
            next_id: 2,
        };
        layout.split(moved, 1, edge).then_some(layout)
    }

    pub fn root(&self) -> &Node {
        &self.root
    }

    pub fn focused(&self) -> PaneId {
        self.focused
    }

    #[cfg(test)]
    pub fn pane_count(&self) -> usize {
        self.panes.len()
    }

    pub fn pane(&self, id: PaneId) -> Option<&Pane<T>> {
        self.panes
            .iter()
            .find(|(pane, _)| *pane == id)
            .map(|(_, pane)| pane)
    }

    fn pane_mut(&mut self, id: PaneId) -> Option<&mut Pane<T>> {
        self.panes
            .iter_mut()
            .find(|(pane, _)| *pane == id)
            .map(|(_, pane)| pane)
    }

    /// Pane ids in layout order: left to right, top to bottom.
    pub fn pane_ids(&self) -> Vec<PaneId> {
        let mut ids = Vec::new();
        self.root.leaves(&mut ids);
        ids
    }

    pub fn pane_of(&self, tab: T) -> Option<PaneId> {
        self.panes
            .iter()
            .find(|(_, pane)| pane.tabs.contains(&tab))
            .map(|(id, _)| *id)
    }

    /// Every tab, panes in layout order.
    pub fn tabs(&self) -> Vec<T> {
        self.pane_ids()
            .into_iter()
            .filter_map(|id| self.pane(id))
            .flat_map(|pane| pane.tabs.iter().copied())
            .collect()
    }

    /// Each pane's rectangle in unit space (the host is 1×1).
    pub fn rects(&self) -> Vec<(PaneId, Rect)> {
        let mut out = Vec::new();
        self.root.rects(
            Rect {
                x: 0.0,
                y: 0.0,
                w: 1.0,
                h: 1.0,
            },
            &mut out,
        );
        out
    }

    /// Show `tab` and focus its pane. A tab no pane holds yet (just opened)
    /// joins the focused pane.
    pub fn activate(&mut self, tab: T) {
        let id = match self.pane_of(tab) {
            Some(id) => id,
            None => {
                let focused = self.focused;
                if let Some(pane) = self.pane_mut(focused) {
                    pane.tabs.push(tab);
                }
                focused
            }
        };
        if let Some(pane) = self.pane_mut(id) {
            pane.active = Some(tab);
        }
        self.focused = id;
    }

    /// File a tab no pane holds into the focused pane without showing it.
    /// Returns whether it was new.
    pub fn adopt(&mut self, tab: T) -> bool {
        if self.pane_of(tab).is_some() {
            return false;
        }
        let focused = self.focused;
        self.pane_mut(focused)
            .map(|pane| pane.tabs.push(tab))
            .is_some()
    }

    /// Move `tab` into a new pane on `edge` of `target`, focused. Refused
    /// when it would only empty and recreate the same pane.
    pub fn split(&mut self, tab: T, target: PaneId, edge: Edge) -> bool {
        let Some(origin) = self.pane_of(tab) else {
            return false;
        };
        if self.pane(target).is_none()
            || (origin == target && self.pane(origin).is_some_and(|p| p.tabs.len() == 1))
        {
            return false;
        }
        let new = self.next_id;
        let split_id = self.next_id + 1;
        self.next_id += 2;
        if !self
            .root
            .insert_beside(target, new, edge.axis(), edge.trailing(), split_id)
        {
            return false;
        }
        self.take_tab(origin, tab);
        self.panes.push((
            new,
            Pane {
                tabs: vec![tab],
                active: Some(tab),
            },
        ));
        self.focused = new;
        self.drop_if_empty(origin);
        self.root.normalize();
        true
    }

    /// Move `tab` into `target` at `index` (its end when `None`), shown and
    /// focused. Within one pane this is a reorder.
    pub fn move_to(&mut self, tab: T, target: PaneId, index: Option<usize>) -> bool {
        let Some(origin) = self.pane_of(tab) else {
            return false;
        };
        let Some(pane) = self.pane(target) else {
            return false;
        };
        let at = index.unwrap_or(pane.tabs.len());
        if origin == target {
            let from = pane.tabs.iter().position(|t| *t == tab).expect("held");
            self.reorder(target, from, at.min(pane.tabs.len() - 1));
        } else {
            self.take_tab(origin, tab);
            let pane = self.pane_mut(target).expect("checked");
            let at = at.min(pane.tabs.len());
            pane.tabs.insert(at, tab);
        }
        if let Some(pane) = self.pane_mut(target) {
            pane.active = Some(tab);
        }
        self.focused = target;
        self.drop_if_empty(origin);
        self.root.normalize();
        true
    }

    pub fn reorder(&mut self, id: PaneId, from: usize, to: usize) -> bool {
        let Some(pane) = self.pane_mut(id) else {
            return false;
        };
        if from == to || from >= pane.tabs.len() || to >= pane.tabs.len() {
            return false;
        }
        let tab = pane.tabs.remove(from);
        pane.tabs.insert(to, tab);
        true
    }

    /// Drop tabs that no longer exist, and the panes they emptied. Returns
    /// whether anything changed.
    pub fn retain(&mut self, mut keep: impl FnMut(&T) -> bool) -> bool {
        let mut changed = false;
        for (_, pane) in &mut self.panes {
            let before = pane.tabs.len();
            pane.tabs.retain(|tab| keep(tab));
            if pane.tabs.len() != before {
                changed = true;
                if pane
                    .active
                    .is_some_and(|active| !pane.tabs.contains(&active))
                {
                    pane.active = None;
                }
            }
        }
        let empty: Vec<PaneId> = self
            .panes
            .iter()
            .filter(|(_, pane)| pane.tabs.is_empty())
            .map(|(id, _)| *id)
            .collect();
        for id in empty {
            self.drop_if_empty(id);
        }
        self.root.normalize();
        changed
    }

    /// The only pane left, once the layout has collapsed to one.
    pub fn sole_pane(&self) -> Option<&Pane<T>> {
        match self.panes.as_slice() {
            [(_, pane)] => Some(pane),
            _ => None,
        }
    }

    /// Drag the boundary before child `index` of split `split` to
    /// `fraction` of the split's extent, keeping both neighbours at least
    /// `min` (a fraction too). Returns whether the sizes changed.
    pub fn resize(&mut self, split: u64, index: usize, fraction: f32, min: f32) -> bool {
        let Some((sizes, _)) = self.root.split_mut(split) else {
            return false;
        };
        if index == 0 || index >= sizes.len() {
            return false;
        }
        let start: f32 = sizes[..index - 1].iter().sum();
        let pair = sizes[index - 1] + sizes[index];
        let min = min.min(pair / 2.0);
        let before = (fraction - start).clamp(min, pair - min);
        if (sizes[index - 1] - before).abs() < f32::EPSILON {
            return false;
        }
        sizes[index - 1] = before;
        sizes[index] = pair - before;
        true
    }

    /// Even out one split's children (a divider's double click).
    pub fn balance(&mut self, split: u64) -> bool {
        let Some((sizes, _)) = self.root.split_mut(split) else {
            return false;
        };
        let even = 1.0 / sizes.len() as f32;
        let changed = sizes.iter().any(|size| (size - even).abs() > 1e-4);
        sizes.iter_mut().for_each(|size| *size = even);
        changed
    }

    fn take_tab(&mut self, id: PaneId, tab: T) {
        if let Some(pane) = self.pane_mut(id) {
            pane.tabs.retain(|t| *t != tab);
            if pane.active == Some(tab) {
                pane.active = None;
            }
        }
    }

    fn drop_if_empty(&mut self, id: PaneId) {
        if self.panes.len() <= 1 || self.pane(id).is_none_or(|pane| !pane.tabs.is_empty()) {
            return;
        }
        let heir = self.root.remove(id);
        self.panes.retain(|(pane, _)| *pane != id);
        if self.focused == id {
            self.focused = heir.unwrap_or_else(|| self.panes[0].0);
        }
    }
}

/// Which zone of a `w`×`h` pane the pointer at (`x`, `y`) — relative to the
/// pane's origin — targets. Edge bands take a fifth of the pane, clamped to
/// 48–96px; an axis the pane cannot split along offers no edges on it.
pub(super) fn drop_zone(x: f32, y: f32, w: f32, h: f32) -> DropZone {
    let band = |extent: f32| (extent * 0.2).clamp(48.0, 96.0).min(extent / 3.0);
    let can_row = w >= MIN_PANE_WIDTH * 2.0;
    let can_column = h >= MIN_PANE_HEIGHT * 2.0;
    let mut best: Option<(f32, Edge)> = None;
    let mut consider = |distance: f32, limit: f32, edge: Edge| {
        if distance <= limit && best.is_none_or(|(d, _)| distance / limit < d) {
            best = Some((distance / limit, edge));
        }
    };
    if can_row {
        consider(x, band(w), Edge::Left);
        consider(w - x, band(w), Edge::Right);
    }
    if can_column {
        consider(y, band(h), Edge::Top);
        consider(h - y, band(h), Edge::Bottom);
    }
    best.map_or(DropZone::Center, |(_, edge)| DropZone::Edge(edge))
}

/// Panes whose top edge is the host's: their strips live in the titlebar
/// band, the others carry their own. Sorted left to right; together they
/// tile the host's width.
pub(super) fn top_row(rects: &[(PaneId, Rect)]) -> Vec<(PaneId, Rect)> {
    let mut top: Vec<_> = rects
        .iter()
        .filter(|(_, rect)| rect.y < 1e-4)
        .copied()
        .collect();
    top.sort_by(|a, b| a.1.x.total_cmp(&b.1.x));
    top
}

/// Whether a pane touches the host's left edge (the outer resize seam).
pub(super) fn touches_left(rect: &Rect) -> bool {
    rect.x < 1e-4
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(tabs: &[u32], moved: u32, edge: Edge) -> SplitLayout<u32> {
        SplitLayout::split_off(tabs.to_vec(), tabs.first().copied(), moved, edge).unwrap()
    }

    fn pane_tabs(layout: &SplitLayout<u32>) -> Vec<Vec<u32>> {
        layout
            .pane_ids()
            .into_iter()
            .map(|id| layout.pane(id).unwrap().tabs.clone())
            .collect()
    }

    #[test]
    fn splitting_a_single_pane_moves_the_tab_to_that_side() {
        let right = layout(&[1, 2, 3], 2, Edge::Right);
        assert_eq!(pane_tabs(&right), vec![vec![1, 3], vec![2]]);
        assert_eq!(right.pane(right.focused()).unwrap().tabs, vec![2]);

        let top = layout(&[1, 2, 3], 3, Edge::Top);
        assert_eq!(pane_tabs(&top), vec![vec![3], vec![1, 2]]);
        let rects = top.rects();
        assert_eq!(rects[0].1.h, 0.5);
        assert_eq!(rects[1].1.y, 0.5);
    }

    #[test]
    fn a_lone_tab_cannot_split_its_own_pane() {
        assert!(SplitLayout::split_off(vec![1], Some(1), 1, Edge::Right).is_none());
        let mut layout = layout(&[1, 2], 2, Edge::Right);
        let lone = layout.pane_of(2).unwrap();
        assert!(!layout.split(2, lone, Edge::Bottom));
        assert_eq!(layout.pane_count(), 2);
    }

    #[test]
    fn same_axis_splits_flatten_into_one_row() {
        let mut layout = layout(&[1, 2, 3], 2, Edge::Right);
        let target = layout.pane_of(2).unwrap();
        assert!(layout.split(3, target, Edge::Right));
        let Node::Split {
            axis,
            children,
            sizes,
            ..
        } = layout.root()
        else {
            panic!("split");
        };
        assert_eq!(*axis, Axis::Row);
        assert_eq!(children.len(), 3);
        assert_eq!(sizes, &vec![0.5, 0.25, 0.25]);
        assert_eq!(pane_tabs(&layout), vec![vec![1], vec![2], vec![3]]);
    }

    #[test]
    fn cross_axis_splits_nest() {
        let mut layout = layout(&[1, 2, 3], 2, Edge::Right);
        let target = layout.pane_of(2).unwrap();
        assert!(layout.split(3, target, Edge::Bottom));
        let rects = layout.rects();
        let right_bottom = rects
            .iter()
            .find(|(id, _)| Some(*id) == layout.pane_of(3))
            .unwrap()
            .1;
        assert_eq!(
            right_bottom,
            Rect {
                x: 0.5,
                y: 0.5,
                w: 0.5,
                h: 0.5
            }
        );
        let top = top_row(&rects);
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].0, layout.pane_of(1).unwrap());
    }

    #[test]
    fn moving_the_last_tab_out_collapses_its_pane() {
        let mut layout = layout(&[1, 2, 3], 3, Edge::Right);
        let left = layout.pane_of(1).unwrap();
        assert!(layout.move_to(3, left, Some(0)));
        assert_eq!(layout.pane_count(), 1);
        assert_eq!(layout.sole_pane().unwrap().tabs, vec![3, 1, 2]);
        assert_eq!(layout.root(), &Node::Pane(left));
        assert_eq!(layout.focused(), left);
    }

    #[test]
    fn closing_a_pane_hands_its_space_and_focus_to_a_neighbour() {
        let mut layout = layout(&[1, 2, 3], 2, Edge::Right);
        let middle = layout.pane_of(2).unwrap();
        layout.split(3, middle, Edge::Right);
        assert_eq!(layout.pane_count(), 3);
        // Focus sits on the newest pane (3); closing it hands over to 2.
        layout.retain(|tab| *tab != 3);
        assert_eq!(layout.pane_count(), 2);
        assert_eq!(layout.focused(), middle);
        let Node::Split { sizes, .. } = layout.root() else {
            panic!("split");
        };
        assert_eq!(sizes, &vec![0.5, 0.5]);
        // The first pane hands over to the one after it.
        layout.activate(1);
        layout.retain(|tab| *tab != 1);
        assert_eq!(layout.pane_count(), 1);
        assert_eq!(layout.focused(), middle);
    }

    #[test]
    fn retain_clears_a_closed_active_tab() {
        let mut layout = layout(&[1, 2, 3], 3, Edge::Right);
        let left = layout.pane_of(1).unwrap();
        layout.activate(2);
        assert_eq!(layout.focused(), left);
        assert!(layout.retain(|tab| *tab != 2));
        assert_eq!(layout.pane(left).unwrap().active, None);
        assert!(!layout.retain(|_| true));
    }

    #[test]
    fn activating_a_new_tab_adds_it_to_the_focused_pane() {
        let mut layout = layout(&[1, 2], 2, Edge::Right);
        let right = layout.pane_of(2).unwrap();
        layout.activate(9);
        assert_eq!(layout.pane_of(9), Some(right));
        assert_eq!(layout.pane(right).unwrap().active, Some(9));
        // Activating a tab elsewhere moves focus to its pane.
        layout.activate(1);
        assert_eq!(layout.focused(), layout.pane_of(1).unwrap());
    }

    #[test]
    fn resize_keeps_both_neighbours_above_the_minimum() {
        let mut layout = layout(&[1, 2], 2, Edge::Right);
        let Node::Split { id, .. } = *layout.root() else {
            panic!("split");
        };
        assert!(layout.resize(id, 1, 0.7, 0.2));
        assert!(layout.resize(id, 1, 0.95, 0.2));
        let Node::Split { sizes, .. } = layout.root() else {
            panic!("split");
        };
        assert!((sizes[0] - 0.8).abs() < 1e-5);
        assert!(layout.balance(id));
        assert!(!layout.balance(id));
    }

    #[test]
    fn drop_zones_prefer_the_nearest_edge_the_pane_can_split_along() {
        assert_eq!(
            drop_zone(10.0, 300.0, 600.0, 600.0),
            DropZone::Edge(Edge::Left)
        );
        assert_eq!(
            drop_zone(590.0, 300.0, 600.0, 600.0),
            DropZone::Edge(Edge::Right)
        );
        assert_eq!(
            drop_zone(300.0, 595.0, 600.0, 600.0),
            DropZone::Edge(Edge::Bottom)
        );
        assert_eq!(drop_zone(300.0, 300.0, 600.0, 600.0), DropZone::Center);
        // Too narrow to split into columns: the left band is just the center.
        assert_eq!(drop_zone(10.0, 300.0, 300.0, 600.0), DropZone::Center);
        assert_eq!(
            drop_zone(150.0, 590.0, 300.0, 600.0),
            DropZone::Edge(Edge::Bottom)
        );
    }
}
