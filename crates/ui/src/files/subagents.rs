//! The Subagents section's model, kept pure so its shape and height are
//! testable without a window.
//!
//! Running subagents lead the body as plain rows. Everything that has
//! settled folds into one **Finished** dropdown, closed until opened, that
//! splits into **Completed** and **Failed** lists. Rows are read from the
//! transcript's spawn chips, so nothing here removes or hides one.

use std::collections::{HashMap, HashSet};

use zeron_doc::SubagentStatus;

use super::sections::{INITIAL_ROWS, PAGE_ROWS, SubagentRow};

/// A collapsible group of finished subagents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Group {
    /// Parent of the two categories below.
    Finished,
    Completed,
    Failed,
}

impl Group {
    pub fn label(self) -> &'static str {
        match self {
            Group::Finished => "Finished",
            Group::Completed => "Completed",
            Group::Failed => "Failed",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Group::Finished => "finished",
            Group::Completed => "completed",
            Group::Failed => "failed",
        }
    }
}

/// Open/closed and paging state of the groups. The Finished dropdown starts
/// closed, so the body opens on what is running; the two lists inside it start
/// open, so one click shows every finished subagent.
#[derive(Debug, Default)]
pub(super) struct GroupState {
    /// Groups flipped away from their default.
    flipped: HashSet<Group>,
    /// Rows revealed per category ("Show more" pages this up).
    shown: HashMap<Group, usize>,
}

impl GroupState {
    pub fn is_open(&self, group: Group) -> bool {
        let default_open = group != Group::Finished;
        default_open != self.flipped.contains(&group)
    }

    pub fn toggle(&mut self, group: Group) {
        if !self.flipped.remove(&group) {
            self.flipped.insert(group);
        }
    }

    pub fn shown(&self, group: Group) -> usize {
        self.shown
            .get(&group)
            .copied()
            .unwrap_or(INITIAL_ROWS)
            .max(INITIAL_ROWS)
    }

    pub fn page_up(&mut self, group: Group) {
        let shown = self.shown(group) + PAGE_ROWS;
        self.shown.insert(group, shown);
    }
}

/// The rows the Subagents body draws, in the order it draws them.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct SubagentPlan {
    /// Not settled: running first (longest-running leading), then any row
    /// whose status has not been stamped yet.
    pub active: Vec<SubagentRow>,
    pub completed: Vec<SubagentRow>,
    pub failed: Vec<SubagentRow>,
}

impl SubagentPlan {
    /// Sort `rows` (already in display order) into the three lists.
    pub fn new(rows: Vec<SubagentRow>) -> Self {
        let mut plan = Self::default();
        for row in rows {
            match row.status {
                Some(SubagentStatus::Done) => plan.completed.push(row),
                Some(SubagentStatus::Failed) => plan.failed.push(row),
                _ => plan.active.push(row),
            }
        }
        plan
    }

    /// Subagents streaming right now.
    pub fn running(&self) -> usize {
        self.active
            .iter()
            .filter(|row| row.status == Some(SubagentStatus::Running))
            .count()
    }

    pub fn finished(&self) -> usize {
        self.completed.len() + self.failed.len()
    }

    /// Everything the list would show with all groups open.
    pub fn total(&self) -> usize {
        self.active.len() + self.finished()
    }

    /// The rows behind a group; `Finished` is both categories, completed first.
    pub fn rows(&self, group: Group) -> Vec<&SubagentRow> {
        match group {
            Group::Finished => self.completed.iter().chain(&self.failed).collect(),
            Group::Completed => self.completed.iter().collect(),
            Group::Failed => self.failed.iter().collect(),
        }
    }

    /// Row-sized slots the body occupies: each visible row, each group
    /// header, and a "Show more" row wherever a category is paged.
    pub fn slots(&self, groups: &GroupState) -> usize {
        let mut slots = self.active.len();
        if self.finished() == 0 {
            return slots;
        }
        slots += 1;
        if !groups.is_open(Group::Finished) {
            return slots;
        }
        for group in [Group::Completed, Group::Failed] {
            let count = self.rows(group).len();
            if count == 0 {
                continue;
            }
            slots += 1;
            if groups.is_open(group) {
                let shown = groups.shown(group);
                slots += count.min(shown) + usize::from(count > shown);
            }
        }
        slots
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;

    fn row(id: &str, status: Option<SubagentStatus>) -> SubagentRow {
        SubagentRow {
            doc_id: id.into(),
            title: id.to_owned().into(),
            status,
            spawned_at: Utc::now(),
        }
    }

    fn mixed() -> Vec<SubagentRow> {
        vec![
            row("run", Some(SubagentStatus::Running)),
            row("new", None),
            row("ok-1", Some(SubagentStatus::Done)),
            row("bad", Some(SubagentStatus::Failed)),
            row("ok-2", Some(SubagentStatus::Done)),
        ]
    }

    fn ids(rows: &[SubagentRow]) -> Vec<&str> {
        rows.iter().map(|r| r.doc_id.as_str()).collect()
    }

    /// Groups as the section first draws them, with Finished opened.
    fn opened() -> GroupState {
        let mut groups = GroupState::default();
        groups.toggle(Group::Finished);
        groups
    }

    #[test]
    fn settled_rows_fold_into_lists_and_the_rest_stay_on_top() {
        let plan = SubagentPlan::new(mixed());
        assert_eq!(ids(&plan.active), ["run", "new"]);
        assert_eq!(ids(&plan.completed), ["ok-1", "ok-2"]);
        assert_eq!(ids(&plan.failed), ["bad"]);
        assert_eq!((plan.running(), plan.finished(), plan.total()), (1, 3, 5));
    }

    #[test]
    fn finished_starts_closed_and_its_lists_start_open() {
        let groups = GroupState::default();
        assert!(!groups.is_open(Group::Finished));
        assert!(groups.is_open(Group::Completed));
        assert!(groups.is_open(Group::Failed));
    }

    #[test]
    fn slots_follow_what_is_open() {
        let plan = SubagentPlan::new(mixed());
        let mut groups = GroupState::default();
        // Closed: 2 active + the Finished header.
        assert_eq!(plan.slots(&groups), 2 + 1);
        groups.toggle(Group::Finished);
        // + (Completed header + 2) + (Failed header + 1)
        assert_eq!(plan.slots(&groups), 2 + 1 + 3 + 2);
        groups.toggle(Group::Failed);
        assert_eq!(plan.slots(&groups), 2 + 1 + 3 + 1);
        // Closing the parent keeps each list's own state for the next open.
        groups.toggle(Group::Finished);
        assert_eq!(plan.slots(&groups), 2 + 1);
        groups.toggle(Group::Finished);
        assert_eq!(plan.slots(&groups), 2 + 1 + 3 + 1);
    }

    #[test]
    fn nothing_finished_means_no_group_chrome() {
        let rows = vec![row("run", Some(SubagentStatus::Running))];
        let plan = SubagentPlan::new(rows);
        assert_eq!(plan.slots(&opened()), 1);
        assert_eq!(SubagentPlan::default().slots(&opened()), 0);
    }

    #[test]
    fn a_long_list_pages_and_counts_its_show_more_row() {
        let rows: Vec<_> = (0..25)
            .map(|i| row(&format!("ok-{i}"), Some(SubagentStatus::Done)))
            .collect();
        let plan = SubagentPlan::new(rows);
        let mut groups = opened();
        // Finished header + Completed header + 10 rows + "Show more".
        assert_eq!(plan.slots(&groups), 1 + 1 + INITIAL_ROWS + 1);
        groups.page_up(Group::Completed);
        assert_eq!(plan.slots(&groups), 1 + 1 + INITIAL_ROWS + PAGE_ROWS + 1);
        groups.page_up(Group::Completed);
        // All 25 shown: no "Show more" left.
        assert_eq!(plan.slots(&groups), 1 + 1 + 25);
    }
}
