//! One projection for local watchers, the synced registry, sidebar, and MCP.
//! Native execution state stays separate from terminal-reported activity.

use super::*;
use zeron_proto::TerminalActivity;

impl SessionsEngine {
    pub(crate) fn track_terminal_activity(
        &self,
        terminal_owner: crate::Terminals,
        workspace: crate::WorkspaceHost,
    ) {
        if self.inner.activity_tx.get().is_some() {
            return;
        }
        let mut chats = workspace.watch_chats();
        terminal_owner.refresh_seen(&chats.borrow());
        let mut terminals = terminal_owner.watch_activity();
        let mut native = self.inner.sessions_tx.subscribe();
        let initial = project(
            &self.inner.device_id,
            &native.borrow(),
            &terminals.borrow(),
            Utc::now(),
        );
        let (tx, _) = watch::channel(initial);
        if self.inner.activity_tx.set(tx.clone()).is_err() {
            return;
        }
        let inner = Arc::downgrade(&self.inner);
        let task = tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(10));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            tick.tick().await;
            let mut previous = Vec::<Session>::new();
            loop {
                let Some(inner) = inner.upgrade() else { break };
                terminal_owner.refresh_seen(&chats.borrow_and_update());
                let now = Utc::now();
                let previous_by_id: HashMap<_, _> = previous
                    .iter()
                    .map(|row| (row.chat_id.as_str(), row))
                    .collect();
                let mut current = project(
                    &inner.device_id,
                    &native.borrow_and_update(),
                    &terminals.borrow_and_update(),
                    now,
                );
                // Unrelated native transitions must not turn terminal leases
                // into per-event registry writes. Refresh them at most every
                // ten seconds unless an actual program report changed.
                for row in &mut current {
                    if let Some(old) = previous_by_id.get(row.chat_id.as_str())
                        && old.terminal_activity == row.terminal_activity
                        && row
                            .terminal_activity
                            .as_ref()
                            .is_some_and(|a| a.report.state.active())
                        && (now - old.updated_at).num_milliseconds() < 10_000
                    {
                        let native_at = native
                            .borrow()
                            .iter()
                            .find(|s| s.chat_id == row.chat_id)
                            .map(|s| s.updated_at);
                        row.updated_at = native_at.unwrap_or(old.updated_at).max(old.updated_at);
                    }
                }
                // This task owns registry publication once installed, avoiding
                // a native row racing a terminal projection and erasing it.
                // A row whose chat hasn't reached this workspace yet is held
                // back, and stays unpublished so it is written once the chat
                // arrives (the chats watch wakes this loop) — not at the
                // chat's next transition.
                let mut unpublished = std::collections::HashSet::new();
                for row in &current {
                    if previous_by_id
                        .get(row.chat_id.as_str())
                        .is_none_or(|old| *old != row)
                    {
                        if workspace.chat(&row.chat_id).ok().flatten().is_some() {
                            workspace.record_session(row);
                        } else {
                            unpublished.insert(row.chat_id.clone());
                        }
                    }
                }
                let current_ids: std::collections::HashSet<_> =
                    current.iter().map(|row| row.chat_id.as_str()).collect();
                for old in &previous {
                    if !current_ids.contains(old.chat_id.as_str())
                        && workspace.chat(&old.chat_id).ok().flatten().is_some()
                    {
                        let mut cleared = old.clone();
                        cleared.terminal_activity = None;
                        cleared.status = SessionStatus::Idle;
                        cleared.updated_at = Utc::now();
                        workspace.record_session(&cleared);
                    }
                }
                tx.send_if_modified(|rows| {
                    if *rows == current {
                        false
                    } else {
                        *rows = current.clone();
                        true
                    }
                });
                previous = current
                    .into_iter()
                    .filter(|row| !unpublished.contains(&row.chat_id))
                    .collect();
                drop(inner);
                tokio::select! {
                    changed = native.changed() => if changed.is_err() { break },
                    changed = terminals.changed() => if changed.is_err() { break },
                    changed = chats.changed() => if changed.is_err() { break },
                    _ = tick.tick() => {},
                }
            }
        });
        *lock(&self.inner.activity_task) = Some(task);
    }

    /// Drain the projection before the registry's final snapshot. Shutdown
    /// closes PTYs first; no live terminal lease may survive an engine restart.
    pub(crate) async fn stop_activity_projection(&self) {
        let task = lock(&self.inner.activity_task).take();
        if let Some(task) = task {
            task.abort();
            let _ = task.await;
        }
        let Some(tx) = self.inner.activity_tx.get() else {
            return;
        };
        let mut final_rows = self.inner.sessions_tx.borrow().clone();
        for old in tx.borrow().iter() {
            if !final_rows.iter().any(|row| row.chat_id == old.chat_id) {
                let mut cleared = old.clone();
                cleared.terminal_activity = None;
                cleared.status = SessionStatus::Idle;
                cleared.updated_at = Utc::now();
                final_rows.push(cleared);
            }
        }
        final_rows.sort_by(|a, b| a.chat_id.cmp(&b.chat_id));
        if let Some(workspace) = self.inner.workspace() {
            for row in &final_rows {
                if workspace.chat(&row.chat_id).ok().flatten().is_some() {
                    workspace.record_session(row);
                }
            }
        }
        tx.send_replace(final_rows);
    }
}

fn project(
    device: &str,
    native: &[Session],
    terminals: &[(String, TerminalActivity)],
    now: chrono::DateTime<Utc>,
) -> Vec<Session> {
    let mut rows: std::collections::BTreeMap<_, _> = native
        .iter()
        .map(|row| (row.chat_id.clone(), row.clone()))
        .collect();
    for (chat, activity) in terminals {
        let row = rows.entry(chat.clone()).or_insert_with(|| Session {
            chat_id: chat.clone(),
            device_id: device.into(),
            status: SessionStatus::Idle,
            started_at: None,
            updated_at: activity.updated_at,
            running_subagents: 0,
            background_tasks: Vec::new(),
            last_completed_turn: None,
            last_turn_outcome: None,
            terminal_activity: None,
        });
        row.terminal_activity = Some(activity.clone());
        // OSC 7501 has no heartbeat requirement. The PTY owner supplies the
        // registry lease, even for a quiet blocked program with no viewers.
        row.updated_at = row.updated_at.max(if activity.report.state.active() {
            now
        } else {
            activity.updated_at
        });
    }
    rows.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_proto::{ProgramInputKind, ProgramState, ProgramStatus};

    /// A run can start before its chat's row reaches this workspace (it is
    /// created on another device, or the row is still syncing). Its status
    /// must land in the registry when the row does, not at the next change.
    #[tokio::test]
    async fn status_for_a_chat_whose_row_arrives_later_is_published_then() {
        let dir = tempfile::tempdir().unwrap();
        let core = crate::EngineCore::assemble(
            dir.path(),
            Arc::new(crate::HarnessRegistry::new()),
            zeron_proto::HarnessId::Mock,
            None,
        )
        .unwrap();
        let published = |core: &crate::EngineCore| {
            core.workspace
                .watch_session_rows()
                .borrow()
                .iter()
                .find(|row| row.chat_id == "late")
                .map(|row| row.status)
        };
        core.sessions.set_status("late", SessionStatus::Working, true);
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert_eq!(published(&core), None, "no chat row yet");

        core.workspace
            .create_chat(
                "late",
                None,
                Some(&core.device_id),
                None,
                Some(dir.path().display().to_string()),
            )
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while published(&core) != Some(SessionStatus::Working) {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the held status is published once the chat row exists");
    }

    #[test]
    fn terminal_projection_preserves_native_receipts_and_control_state() {
        let now = Utc::now();
        let native = Session {
            chat_id: "chat".into(),
            device_id: "device".into(),
            status: SessionStatus::Working,
            started_at: Some(now),
            updated_at: now,
            running_subagents: 0,
            background_tasks: Vec::new(),
            last_completed_turn: Some("previous-turn".into()),
            last_turn_outcome: None,
            terminal_activity: None,
        };
        let activity = TerminalActivity {
            terminal_id: "pty".into(),
            updated_at: now,
            report: ProgramStatus {
                id: "worker".into(),
                state: ProgramState::Blocked,
                kind: Some(ProgramInputKind::Permission),
                app: None,
                progress: None,
                title: None,
                message: Some("Approve?".into()),
            },
        };
        let later = now + chrono::Duration::minutes(2);
        let rows = project(
            "device",
            std::slice::from_ref(&native),
            &[("chat".into(), activity.clone())],
            later,
        );
        assert_eq!(rows[0].status, SessionStatus::Working);
        assert_eq!(rows[0].activity_status(), SessionStatus::AwaitingInput);
        assert_eq!(rows[0].last_completed_turn, native.last_completed_turn);
        assert_eq!(rows[0].updated_at, later);
        assert_eq!(rows[0].terminal_activity.as_ref().unwrap().updated_at, now);
        assert_eq!(
            project("device", &[native.clone()], &[], later),
            vec![native]
        );
        let rows = project("device", &[], &[("chat".into(), activity)], later);
        assert_eq!(rows[0].status, SessionStatus::Idle);
        assert_eq!(rows[0].last_completed_turn, None);
    }
}
