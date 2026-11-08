# Composer editor-metrics provider

The framework revision `ce91055fba63352fa126a44a5714749d94f2380e` had no
public editor natural-metrics API. Element width/height describe the imposed
viewport; Canvas text measurement cannot reproduce editor spans or IME.

C7 adds `Element:text_metrics(content_width)` / `EditorHandle::text_metrics`.
The result contains requested width, unwrapped width, wrapped height, base line
height, revision and exactness. It includes the displayed IME composition and
final empty lines, using editor rich/plain shaping and span-highlighter routes.
Padding, borders, placeholder and gutter are excluded. Invalid widths,
passwords, unmounted elements and non-editors return nil.

A reactive reader follows content and typography after layout, independently
of viewport size, focus, selection, scroll and caret blink. Each exact first
query per revision/width visits all hard lines, with retained shaping caches;
unchanged queries are constant-time. Large full-document queries are opt-in
and may be expensive after edits. The composer keeps its requested expanded
width stable through animation.

Framework commit: `f01ab87f` on
[jsgrrchg/zgx, refactor/composer-editor-metrics](https://github.com/jsgrrchg/zgx/tree/refactor/composer-editor-metrics).
Push to the original `zeronsh/zgx` was denied (403). The implementation is
published in the existing public fork so the application can pin an accessible,
immutable commit. When upstream integrates it, the pin can return to upstream.
No PR was opened.

Framework validation: 1,386 core tests passed; all 412 script UI tests passed,
including reactive metrics. Workspace Clippy with warnings denied, Rustfmt,
StyLua 2.6, and lint/typecheck of 125 example scripts passed. Two existing
failures remain in the broader script suite: Monitor's memory threshold
(reproduced at the unmodified base) and an unregistered `list_keys_read` test.
Generated definitions consistency checks pass after regeneration. Details and
the seven-run long-draft cache experiment are committed in the framework at
`docs/results/editor-natural-metrics-2026-11-08/README.md`.

C8 consumes this capability only through `ds/composer_metrics`. It invalidates
the old persisted plain-text samples, removes both invisible measurement nodes
and keeps the complete-sample/revision/status contract used by geometry and
motion. Its stale/initial samples are explicit; zero is not a fresh empty draft.
These structural changes do not establish a reproduction or confirmed fix of
the reported visual glitch. Native presentation is validated separately in C9.
