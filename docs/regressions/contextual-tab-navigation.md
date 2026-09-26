# Contextual tab navigation

Ctrl+Tab and Ctrl+Shift+Tab navigate the focused pane on macOS, Linux, and
Windows. macOS Command+Tab remains the system application switcher. Existing
custom `nextSession` / `prevSession` bindings retain their stored keys.

- Main conversation, main composer, explorer, and bottom terminal: cycle the
  visible session list in sidebar order.
- Right pane content and its titlebar tab strip: cycle the session's live
  surface tabs in their displayed, drag-reordered order.
- A pane with zero or one surface tab stays in that pane. Navigation wraps in
  both directions and does not include stale tabs or the empty surface picker.
- Read-only transcripts and diffs retain pane focus after a click or tab switch.
  Inputs retain their own focus while editing. Hover does not choose a pane.
- Closing an active right tab keeps focus on the remaining right content;
  closing the last tab or hiding the pane returns navigation to the main area.
- Shell modals and pickers block navigation. Shortcut recording keeps its
  existing interception behavior. Normal Settings navigation still opens a session.

## Automated checks

`cargo test --locked -p zeron-ui --lib -- --test-threads=1`

The `shell::navigation_tests` suite renders production pane components and
dispatches real configured keystrokes without starting an engine. It covers
repeated cycling, reordering, side-chat and diff focus, tab closure, empty
panes, stale entries, per-session tabs, hiding/reopening, expansion, custom
bindings, modal/picker guards, browser focus, and both terminal locations.
`shell::navigation_focus::tests` exercises a real input's mouse-down blur.
`browser::model::native_shortcut_tests` checks native Tab/BackTab normalization.
Existing shortcut, recording, focus-recovery and tab-mouse tests remain applicable.

## Native desktop checks

These are manual checks; headless GPUI tests do not validate native OS event delivery.
Run on macOS, Linux (X11 and Wayland), and Windows with a real session:

1. Open a side chat, a diff, a file editor, a terminal, and (where supported) a
   browser in the right pane. Repeatedly cycle forward and backward from each.
2. Click and select text in the main transcript, then in a right transcript or
   diff. Check that the shortcut follows the clicked pane and does not clear
   an editor's selection while clicking within that editor.
3. Reorder tabs, close the focused tab, close the last tab, and hide/reopen or
   expand the pane. Verify the next two shortcuts stay in the expected pane.
4. Repeat from the bottom terminal and explorer: these cycle main sessions.
5. Open a modal, picker, or shortcut recorder. Check that session/tab navigation
   does not run behind it. Rebind both navigation shortcuts and repeat.
6. On macOS, focus actual browser page content and press Ctrl+Tab / Ctrl+Shift+Tab;
   repeat from the address input. Check for exactly one transition per press.
