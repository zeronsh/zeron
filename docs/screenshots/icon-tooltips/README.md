# Tooltips on icon-only buttons

Buttons that show only a glyph and had no tooltip now name what they do on
hover, in the same chip their neighbours use.

Captured on 2026-10-25 from `zeron/add-missing-button-tooltips` at `bdb4e0c4`
(on `main` `c04e3749`): light mode, window 1280×820 logical (2560×1640 px).

A local capture driver, not part of this change, ran the production `Shell` in
an offscreen window that never took focus, against an in-process engine with no
agent harnesses. Synthetic pointer and key events did the clicking and hovering;
after each hover the driver waited past the tooltip delay and saved the whole
window with `render_to_image`. Images are uncropped; the PNGs were re-deflated
losslessly.

Everything shown is synthetic: the `fieldnotes` project and its edits, the
session, the "Studio Mac" device, the account "Ana", and the "Ridge at dawn"
background. The model catalog, pull requests, Billing and Cloud come from faked
engine replies; nothing reached GitHub, Stripe or an account. The voice error is
the app's own message: the start failed before any audio device opened.

| File | Button | Tooltip |
|---|---|---|
| `markdown-add-comment.png` | "+" beside a Markdown table header | Add comment |
| `diff-add-comment.png` | "+" in a diff line's gutter | Add comment |
| `editor-add-comment.png` | "+" in the file editor's gutter | Add comment |
| `editor-show-comment.png` | Comment marker, comment closed | Show comment |
| `editor-hide-comment.png` | Comment marker, comment open | Hide comment |
| `picker-starred-tab.png` | Model picker's star tab | Starred |
| `picker-agent-tab.png` | Model picker's agent tab | Claude Code |
| `picker-add-favorite.png` | A model's star, off | Add to favorites¹ |
| `picker-remove-favorite.png` | A model's star, on | Remove from favorites¹ |
| `compact-picker-back.png` | Compact picker's back arrow | Back¹ |
| `pull-requests-clear-search.png` | × in the board's search | Clear search |
| `pull-request-send-comment.png` | Send in the Activity comment box | Send comment |
| `appearance-accent-swatch.png` | An accent swatch | Orange |
| `add-theme-close.png` | × on Add a theme | Close |
| `background-close.png` | × on Adjust background | Close |
| `background-zoom-out.png` | − beside the zoom slider | Zoom out |
| `background-zoom-in.png` | + beside the zoom slider | Zoom in |
| `billing-fewer-packs.png` | − on the extra-credit packs | Fewer packs |
| `billing-more-packs.png` | + on the extra-credit packs | More packs |
| `cloud-repositories-refresh.png` | Refresh beside Choose on GitHub | Check repositories again |
| `cloud-copy-code.png` | GitHub device-code chip | Copy code |
| `providers-refresh.png` | Refresh beside the device menu | Check for agents and updates |
| `action-icon.png` | An icon choice in Add action | Test |
| `section-dialog-close.png` | × on New section | Close |
| `voice-error-dismiss.png` | × on the voice error | Dismiss |
| `new-project-hidden-folders.png` | "…" in New project's folder trail | Show hidden folders |

¹ Reworded after capture: the full picker's star now names its model, as the
compact picker's does ("Add GPT-5.4 to favorites"), and Back shows its
shortcut ("Back · Esc").

Validation: `cargo test -p zeron-ui --lib -- --test-threads=1` at `bdb4e0c4`
passed (1875 tests, 0 failed), and `git show --check` is clean. A parallel
`cargo test` of the same suite aborts with SIGABRT on macOS without this change
too; CI runs it under nextest, one process per test.
