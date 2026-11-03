# Todo panel

The agent's checklist is a tray above the composer, not just a `Todo · 2/5 done`
chip in the transcript. It follows the latest checklist of the selected chat and
shows what is being worked on without scrolling.

Screenshots: [`docs/screenshots/todo-panel/`](screenshots/todo-panel/).

## Data

Every harness that plans writes the *whole* list on each update, so the panel
needs no history, only the latest `ToolCall::Todo` part.

| Harness | Source | In-progress |
| --- | --- | --- |
| Claude | `TodoWrite` | `in_progress` |
| OpenCode | `todowrite` | `in_progress` |
| ACP (Gemini, Cursor, Devin, ...) | `plan` update, id `LIVE_PLAN_TOOL_ID` | `in_progress` |
| Codex | `turn/plan/updated` (`update_plan`), id `LIVE_PLAN_TOOL_ID` | `inProgress` |
| Codex | legacy `todoList` item | not exposed |
| Cursor | `updateTodos` | `status` string, when present |

`TodoItem` is `{ text, done, status? }` (`crates/proto/src/agent.rs`).

- `done` is unchanged and stays authoritative for completion.
- `status` (`pending | inProgress | completed`) is additive and serde-defaulted. It
  is written **only for an in-progress item**, so every other item serializes
  byte-for-byte as before and old builds, iOS and the TS edge
  (`edge/src/session-doc`) ignore it. A missing or unrecognised value derives from
  `done`; an unknown future status never fails the whole tool call.
- Use `TodoItem::new(text, status)` to keep `done` and `status` consistent and
  `item.status()` to read the effective value.
- `cancelled` (OpenCode) decodes as pending: it is not done and not being worked on.

The ACP and Codex plan updates reuse one tool id, so the fold refreshes the same
part in place within a segment and appends a new one in the next segment. The
engine's stale-echo filter exempts that id, and "latest part wins" is the panel's
contract (`latest_todo`). An empty list clears the panel.

## Behaviour

`crates/ui/src/todo_panel.rs`, mounted by the composer above the queue tray (one
step narrower than it, which is itself narrower than the composer).

- **Scope.** Todo parts of the chat's own transcript only. Subagent traffic lives
  in separate docs and never reaches it. Hidden when the chat has no todo.
- **Collapsed.** `Todo · 2/5` plus the in-progress item (else the first unfinished
  one); a finished list reads `Todo · 5/5 · All done`.
- **Expanded.** Every item, in the agent's order, with a status glyph: check
  (success), spinner (in progress and the turn is live; a still ring when idle, so a
  stopped run does not look busy), empty ring (pending). Completed items recede,
  the current item is the brightest.
- **Long lists.** More than 6 items fold to a window of 3 with the current item in
  the middle (one before, one after; clamped at the ends; a finished list shows its
  last three). `N earlier` / `N later` rows at the edges reveal the hidden items in
  place and hide them again. Items are never reordered (`todo_panel::rows`).
- **Open state.** Per chat, in memory for the app run (like the right-pane flags in
  `shell::SessionPanels`; the repo keeps no per-chat UI state on disk). Default:
  open while work remains, compact once everything is done, including when a new
  turn starts on an old finished list. When the last item completes and the turn
  is idle, an explicit choice is reset once so the panel tidies itself to the
  compact done state; opening it by hand afterwards sticks.
- **Done state.** Offers a dismiss button; a dismissal holds until the agent writes
  a different list.
- **No focus stealing.** Nothing here takes focus; updates only re-render.
- **Accessibility.** The header, dismiss and fold rows are buttons with
  `aria_label`, tab stops and a focus ring, activated by Enter/Space. The icon-only
  dismiss button has a tooltip, and the header's tooltip names the action it will
  take.
- **Motion.** The list fades in on toggle (`FADE_QUICK`) and the tray fades in on
  mount. Those, and the shared mini glyph spinner, all follow the global
  reduce-motion flag, which also covers the pause-in-background setting.

The transcript chip keeps working: `Todo · 2/5 done`, and its expanded detail marks
the current item `[~]` (`[x]` done, `[ ]` pending).

## Performance

The composer re-renders far more often than the transcript changes, so
`TodoCache` rescans only when the selected chat, `transcript_revision` or the
transcript length changes. The scan walks back from the newest entry and stops at
the first todo.

## Testing

```sh
cargo test --locked -p zeron-proto todo_item_tests
cargo test --locked -p zeron-harness            # normalizers: claude, opencode, acp, codex, cursor
cargo test --locked -p zeron-doc todo
cargo test --locked -p zeron-ui --lib todo_panel
```

Live: `ZERON_HARNESS=mock ZERON_MOCK_TODO=1 ZERON_MOCK_DELAY_MS=900` (see
`CONTRIBUTORS.md`) walks an 8-item list through every state.

## Not done

- Mobile (iOS/Android) panel: a later PR in the stack.
- The `[x]` / `[ ]` rendering in MCP `read_chat` and the mobile transcript does not
  yet distinguish in-progress.
