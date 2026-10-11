These screenshots show the real GPUI shell with isolated sample chats from
`crates/ui/examples/command-palette-fixture.rs`. The fixture uses temporary
storage and does not start an engine or connect to an account, so it shows no
Files or In Files results.

Run `cargo run -p zeron-ui --example command-palette-fixture` on a desktop, then
press Cmd+K (Ctrl+K on Linux/Windows). Set `ZERON_PALETTE_LIGHT=1` for light mode.

## Tabs and sections

The palette searches threads, commands and the focused chat's workspace. Tabs
under the search field narrow it: **All**, **Threads**, **Commands** and
**Files**. Cmd+1…4 (Ctrl+1…4) select them while the palette is open — the
bindings live in the palette's `CommandPalette` key context, where they outrank
the same keys' session jumps; with the palette closed those keys jump to
sessions as before. Tab and Shift+Tab cycle the tabs. The palette always opens
on All.

With an empty query, All shows:

1. **Recent Files** — up to five files the focused chat opened in editor tabs,
   newest first, with the parent folder in muted text.
2. **Quick Actions** — New chat, New project, Open settings, the theme switch
   and, with a focused unarchived chat, Archive thread, each with its shortcut.
3. **Recent Threads** — up to five other unarchived top-level threads by
   latest activity.

With a query, All shows **Threads** (five), **Commands**, **Files** (five) and
**In Files** (five); empty sections are omitted. The Threads tab lists up to 50
threads, the Files tab up to 50 file names and 100 content matches (three per
file).

- Threads are ranked in the UI (`crates/ui/src/shell/thread_search.rs`): every
  query word must match the title (weight 100), project (40), branch (30),
  device or pull request (20); a match starting a word earns half its weight
  again; archived threads score half; ties go to recent activity. Side chats
  and subagent workers never appear.
- Files and In Files query the focused chat's host (`SearchWorkspaceFiles` at
  once, `SearchWorkspaceContent` after an 80 ms pause) once the query has two
  characters. Their headers read "Searching…" while a request runs and
  "Indexing…" while the host is still building its index; a host too old for
  content search says so in the In Files header.
- Answers to superseded queries are dropped, and the highlighted row is kept by
  identity, so results arriving never move the selection.
- Enter opens the row: a thread, a command (the theme switch keeps the palette
  open), a file in an editor tab, a folder revealed in the files tree, or a
  content match at its line.

Footer hints: ↑ ↓ Navigate, ↵ Open, Cmd/Ctrl+1…4 Tabs, Esc Close.

`tabs-empty-state.png`, `tabs-thread-search.png` and
`tabs-commands-no-results.png` show the redesign (captured under Xvfb, so
without the backdrop blur). The other images predate the tabs.

## Earlier verification

Verified interactively on Linux/X11 before the tabs:

- First-open typing searches the palette, leaving the composer untouched.
- Matching text keeps a compact rounded code wash and code text color while preserving the row font and original text spacing. This applies to action labels and chat metadata.
- Palette chat hover leaves the sidebar pixels unchanged (including Archive state).
- Chat rows use the sidebar height calculation and 2px gap; action rows are 32px.
- Up/Down wraps and scrolls the selected result into view; Enter opens it.
- New chat, New project, and Open settings reach their respective screens.
- Escape, a second Ctrl+K, and clicking outside dismiss the palette.
- No matches produces the empty state.

The palette uses the composer's frosted tint and 16px backdrop blur, with its
opaque fallback when frost is disabled. The platform-specific Cmd key binding
has not been exercised on macOS in this Linux environment.
