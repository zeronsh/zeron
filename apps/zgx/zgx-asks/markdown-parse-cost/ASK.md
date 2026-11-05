# Ask: parse `ui.markdown` off the frame

**Priority: medium.** When a long reply lands whole (a chat opened on one,
a reply arriving from another device), its whole Markdown parse runs in the
frame that shows it. On a loaded Mac that frame goes past a 120 Hz budget.

**Today.** Zeron's transcript windows a long reply (`ds/markdown.luau`, runs
of 16 blocks, only the last run built at first). But to split the reply into
runs it must parse the whole text first, in `ui.markdown`, synchronously.

- A 400-block reply (about 25 KB: headings, paragraphs, lists, code blocks):
  `ui.markdown` takes **2.2 ms** of the landing frame on its own, timed with
  `os.clock` around the call (`Math.prepare`, `Pics.split` and `Pics.merge`
  are under 0.05 ms together).
- The landing frame now takes 39.3M instructions (script 24.2M, update 7.9M,
  layout 4.8M) and 4.0–4.3 ms of work at normal load. About 20M of that is
  fixed whatever is built, and the parse is roughly half of it.
- Under load the same work stretches 2–3×. Before Zeron's windowing change,
  the same frame took 6.1 ms in one run and 14.8 ms in another, with the same
  53M instructions.

**Repro** (from `apps/zgx`, release app, `ZGX_CHECK=0`): run
`./target/release/zeron-zgx fixture=chat_fixture.luau --inspect` with
engine isolation (`ZERON_ZGX_ENGINE_SPAWN=0`, its own `ZERON_ZGX_ENGINE_PORT`,
`ZERON_ZGX_ENGINE_DATA` and `HOME`). Then:
`zgx inspect command go_s6`, `mark s`, `command long_reply`, and
`frames since=s summary=true`. The landing frame is the one whose writes
start with `chat.messages` and include `markdown_window_heights`.

**Ask:** never put a long text's whole parse into one frame. Either:

- **a worker thread:** parse on another thread, handing back the blocks when
  done (a state, as `ui.fetch`'s result is), with the text's previous blocks
  or nothing meanwhile; or
- **incremental, tail first:** parse within a budget per frame, the end of
  the text first, since a transcript opens at its end.

Either way, a caller needs to know how many blocks there will be (or the
text's top-level boundaries) early, so its windowing can lay out held
boxes for the parts not parsed yet.

**Seen on:** zgx `f6506b6e6` (Zeron's pin), release, macOS.

**Done (Nov 2):** `ui.markdown(text, { key, lazy = true, first = i })` in zgx
`0dff42bf5` (#1380), Zeron's pin. Adopted in `ds/markdown.luau`: a windowed
text (a transcript reply) is parsed lazily by its window's key; a pending
block holds its place, sized by `hint` and `lines` (`pending_height`); runs
build as their blocks parse and still window near/far; a find's current
match asks `first` for its block. Measured on a release build (`go_s6`,
`long_reply`, `frames summary`): the landing frame went from 39.6M to 21.5M
instructions (from 5.9–10.4 ms to 2.4–2.6 ms of work). The heaviest frame is
now the one where the end's blocks arrive, at 28M: about 1.1 ms re-converting
the 400-entry list, and 0.8 ms building the last run's blocks. Tests:
`ui/tests/lazy_markdown.test.luau`.
