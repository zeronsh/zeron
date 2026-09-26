# File tree actions and workspace moves

Right-click a file or folder in the explorer to open the shared context menu.
The same menu opens with the Menu key or Shift+F10 on the selected row.

- **Add to chat** inserts a workspace reference into the current draft and focuses
  the composer. It keeps existing text and does not send a message.
- **Copy path** copies the full path using the owning workspace's root and path
  format, including when the workspace belongs to another device.
- **Rename…** edits the name in the row. Enter submits; Escape or losing focus
  before submission cancels. F2 also opens the inline editor. A rename never
  overwrites another entry.
- **Delete…** asks for permanent deletion. Cancel is initially selected; use
  Tab/arrow keys to choose the destructive action. Deleting a folder includes
  its current contents. Open buffers remain available for recovery; autosave
  cannot recreate deleted files. The Delete key opens the same confirmation.

Drag an entry onto a folder to move it. Dropping on a file targets that file's
parent directory. The root row and the empty space below the list target the
workspace root. A drop in the current parent is a no-op; a folder cannot move
into itself or a descendant. Existing destinations are never overwritten or
merged. The source remains visible until the host confirms the operation.

Drag from the row on macOS, Linux, and Windows. The Windows drag threshold
protects ordinary clicks from small pointer jitter. Search results and file
tabs continue to support references into the conversation; only tree-originated
drags can move entries.

Holding a compatible drag over a closed folder opens it after 650 ms. Dragging
near the top or bottom of the list scrolls it. Escape, leaving the tree,
switching sessions, and closing the explorer clear the feedback and timers.
Folders opened by hover stay open. The chat column retains its own drop zone,
so a gesture ending there adds a reference instead of moving the entry. OS file
attachments continue through the existing composer attachment pipeline.

## Workspace and editor consistency

Operations run through the owning engine, locally or remotely. Directory pages
announce the host's mutation capabilities and checkout identity; older hosts
remain usable for navigation, Copy path and Add to chat. Mutation requests
carry the expected checkout and an opaque metadata revision. A stale source
must be refreshed before another explicit attempt. Symlinks cannot be renamed,
moved or deleted through these actions, and traversal through symlink parents
is rejected. The workspace root and `.git` paths are protected.

Move and rename use one RPC. Writes acquire a shared checkout gate before their
existing per-file lock; structural operations acquire the exclusive gate.
The UI pauses affected autosaves and waits for in-flight saves. It remaps open
editors, tabs and comment paths while retaining unsaved text. Response and
semantic watcher events share an operation ID so receiving both is harmless.
A semantic event can reconcile a successful operation whose RPC reply was lost.
Transport failures are never retried automatically.

A recursive delete can fail after deleting some children. The response reports
that condition and the explorer refreshes the surviving contents. Cancellation
of a request after a native filesystem call starts is not an undo operation.
Metadata revisions are not recursive directory snapshots, and operations do
not provide transactions against arbitrary external processes. Cross-volume
moves return an error instead of falling back to copying and deleting.

Native moves use Linux `RENAME_NOREPLACE`, macOS `RENAME_EXCL`, and Windows
`MoveFileExW` without replacement or cross-volume copy flags. See the
[Linux rename manual](https://man7.org/linux/man-pages/man2/rename.2.html),
[Apple's exclusive rename capability](https://developer.apple.com/documentation/foundation/urlresourcevalues/volumesupportsexclusiverenaming),
and [Microsoft's MoveFileExW contract](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-movefileexw).
Case-only aliases on case-insensitive Unix volumes use a unique temporary entry
with no-replacement rollback. Distinct case-similar hard links are collisions.

## Verification

Automated coverage includes protocol compatibility, filesystem mutation and
collision tests, real RPC/watch integration, two engines forwarding through a
relay into a plain folder, tree model relocation, dirty-buffer reconciliation,
menus, inline rename, cancellation, and timed hover expansion. A GPUI fixture
renders the production tree beside `Shell::render_main` to test both directions
between the tree and the real conversation drop zone.

Commands from the repository root:

```sh
cargo test --release --locked -p zeron-proto --lib
cargo test --release --locked -p zeron-engine --lib workspace_files::
cargo test --release --locked -p zeron-engine --lib rpc::
cargo test --release --locked -p zeron-engine --test workspace_files
cargo test --release --locked -p zeron-engine --test device_routing workspace_entry_mutations_are_forwarded
cargo test --release --locked -p zeron-ui --lib -- --test-threads=1
cargo check --release --locked -p zeron
```

This implementation was exercised on Linux with GPUI's test backend. Native
macOS/Windows interaction, Windows junctions, and the case-insensitive-volume
fallback require verification on those hosts. A Windows-specific regression is
included for row-click jitter and intentional row dragging. A desktop visual
review of themes, narrow windows and pointer feel is still required; headless
tests do not establish native rendering quality.

Validated on 2026-09-23: 1,274 UI tests, 45 protocol tests, 37 workspace engine
tests, 18 RPC tests (one ignored), four workspace RPC integration tests, and the
two-engine forwarding test passed. The application release check also passed.
An initial full UI run hit a sidebar scheduler failure; its isolated rerun and
the subsequent complete suite passed. Formatting checks passed for every Rust
file changed here; the workspace-wide formatting check still reports existing
differences in unrelated files.
