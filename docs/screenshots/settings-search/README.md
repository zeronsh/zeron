# Settings search

A field under Back in the Settings sidebar finds any setting on any page. These
frames are the production Shell drawn offscreen by
`crates/ui/examples/settings-search-fixture.rs`, with temporary settings and no
engine or account: Cloud and Billing stay hidden, so their rows are not
searched here. ⌘F and Enter go through the window's real key dispatch; the
queries are set on the field.

```bash
ZERON_OPEN_ROUTE=settings/general cargo run -p zeron-ui \
  --example settings-search-fixture --features appshots-fixture -- <output-dir> [name]
```

`ZERON_PALETTE_LIGHT=1` selects the light palette, `ZERON_FIXTURE_FROSTED=1`
the frosted surface and `ZERON_FIXTURE_SIZE=<w>x<h>` the window size.

## Behavior

- ⌘F (Ctrl+F) focuses the field from anywhere in Settings and selects its
  query. Its hint shows while the field is idle, unless one of the user's
  shortcuts is on ⌘F and takes the key first.
- The index (`crates/ui/src/settings/search.rs`) lists the 53 settings rows by
  page, block, title and the words people search by; every shortcut where it
  is recorded (the Shortcuts page, Dictation on Voice, the global capture on
  Appshots); and every page.
- Every query word must match a field: title 100, the row's own terms 40, its
  block 30, its page 20, half again at a word start; a title that is the whole
  query outranks the rest. A word nothing spells may find a title or page word
  one edit off (two from eight letters, never under four), below every real
  match. Ties keep pages first, then nav order.
- Hits replace the section list: the page's glyph, the title with the match
  highlighted, `Page · Block` beneath, and a shortcut's binding. ↑↓ move, Enter
  or a click opens, and the field keeps the keyboard for the next pick. Escape
  clears the query, then closes Settings.
- A pick opens the page and glides the row a third of the way down
  (`motion::SCROLL_GLIDE`), then washes it with the accent: 150 ms in, 900 ms
  held, 600 ms out. Reduced motion jumps, and holds the wash without fades.
  Scrolling mid-glide hands the page back. A row hidden behind its toggle
  (Microphone, Autosave delay…) lands on the toggle; a row that has not laid
  out 1.5 s after the pick is let go.
- Leaving Settings by any route clears the query.

## Frames

| File | State |
|---|---|
| `idle.png` | The field under Back, with its ⌘F hint |
| `theme.png` | `theme`: three titles, then the Appearance page |
| `typo.png` | `notifcations`: the misspelled page and its rows |
| `shortcuts.png` | `sidebar`: shortcut hits with their bindings |
| `no-results.png` | `teleport`: nothing matches |
| `landed.png` | `math size`, Enter: Appearance glided to its last row, washed |
| `light-theme.png`, `light-landed.png` | The same in the light palette |

## Verification

- `cargo test -p zeron-ui --lib -- settings::search shell::settings_search`
  covers ranking, typos, highlights and hidden pages; the landing math, glide,
  hand-back, reduced motion, fallback, seek limit and window scoping; and, on
  the real Shell, the keyboard flow end to end, shortcut hits, the keyboard
  kept across a pick (Appshots, a click), Enter on the clear button, and
  leaving Settings by any route.
- `the_index_and_the_pages_agree` draws every page reachable without an
  account: each row a page draws is in the index, and each indexed row is
  anchored on its page.

## Limits

- Static frames say nothing of the glide's smoothness.
- Cloud and Billing rows are indexed and anchored, but draw only with an
  account: neither these frames nor the tests reach them.
- The hits are listbox options marked selected, but the keyboard stays in the
  input, which is not their ancestor, so gpui does not announce the active hit
  to screen readers. The ⌘K palette shares this.
