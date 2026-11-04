# What Zeron asks of zgx

Every open ask from Zeron's zgx app, one folder per topic: an `ASK.md` (who
needs it, what fails today, the ask) and, where one exists, a minimal repro
(`zgx run <folder>/repro` or `zgx test <folder>`, with `ZGX_CHECK=0`). Most
were seen on zgx `b3371f8` on macOS, unless the folder says otherwise; Zeron's
pin is now zgx `0.1.10` (`c85c72c6`) (0.1.9, its fixes, Linux popups and glass blur, full screen to scripts, terminal colours from the theme, lists that scroll with the view around them, an app-owned network policy for plugins, the host's top layer, and the module, slot and shortcut rules for plugins). Each open ask is assigned to a zgx lane,
which reports its SHA when it lands. Ranked by what users of Zeron would notice.

## High: users see it

| Ask | What users see today |
|---|---|
| [shader-compile-first-draw](shader-compile-first-draw/ASK.md) | The window freezes 70-360 ms when the first session of a launch starts working (its status ring's shader compiles in the frame). |
| [inline-node](inline-node/ASK.md) | Inline math (and inline chips, emoji images) can't sit in a paragraph's line. |
| [windows-caption-buttons](windows-caption-buttons/ASK.md) | Windows' caption glyphs stay the system's light or dark (invisible on a light Zeron on a dark system), come from an icon font (tofu on Windows 10), disagree with their hit test when the window is zoomed, are fixed at 32 tall, and Maximise shows on a fixed-size window. |
| [windows-popup-backdrop](windows-popup-backdrop/ASK.md) | A popup's glass on Windows blurs nothing: a wallpaper's dither shows sharp through the model picker. Zeron draws the menus' glass solid there meanwhile. |
| [windows-transparency](windows-transparency/ASK.md) | Windows' Transparency effects aren't reported (`ui.prefers.reduced_transparency` is always false there). Zeron watches the registry meanwhile. |

## Medium: users meet it now and then, or a feature waits on it

| Ask | What it holds back |
|---|---|
| [nested-presence-placement](nested-presence-placement/ASK.md) | A plugin's `replace` misses a slot that is a `ui.presence` branch's root (nested or not), silently: Parallax scenes stopped drawing a new chat's background. Zeron keeps plugin places a child inside the branch meanwhile. |
| [i18n-message-props](i18n-message-props/ASK.md) | After a language change the menu bar keeps the old language until relaunch, tooltips until rebuilt; plugins can't ship their own translations. |
| [atomic-selection](atomic-selection/ASK.md) | A selection that starts inside a formula copies half its TeX. |
| [web-snapshot](web-snapshot/ASK.md) | The browser panel can't attach a picture of the page. |
| [markdown-offsets](markdown-offsets/ASK.md) | Review comments on a Markdown preview can't anchor to the block that holds their line. |

## Developers of Zeron (and of any zgx app)

| Ask | Cost today |
|---|---|
| [reload-cost](reload-cost/ASK.md) | Mostly fixed (#1284: only changed files compile, same-bytes saves don't reload); the whole-window reload per save remains (speed-of-light lane). A partial reload (`ZGX_RELOAD=partial`, `b93f745b`) is in the pin `d81d2c33`, opt-in until it's proved on Zeron's app and templates; not adopted yet. Was: | Every save reloads the whole window and recompiles every module (77% of a reload); a same-bytes rewrite reloads too. A 50-70% CPU burst per save. Zeron side (Nov 2): one reload per save, no `zeron-catalog` pseudo-plugin (`ScriptHost::i18n`, `96856669b`), no top-level router; launch scripts compiled ahead (`compile_ahead`, `94baa20f`). |

## Low

| Ask | |
|---|---|
| [editor-menu-items](editor-menu-items/ASK.md) | Zeron's own text menu (ds/editor_menu) has no spelling guesses: zgx's `editor_spell::menu_items` is crate-private. Ask: a public way to get the guesses for the word at a point and apply one. |
| [plugin-time](plugin-time/ASK.md) | A slow plugin can't be timed or blamed. |
| [tooltip-function](tooltip-function/ASK.md) | A tooltip can't follow a state in place. |

## Done upstream

| Ask | |
|---|---|
| [host-guard](host-guard/ASK.md) | `ScriptHost::guard` and its kinds (#1461, #1463, #1468). In the pin; adopted: src/plugins/preview.rs `guard`. |
| [host-top-layer](host-top-layer/ASK.md) | `View::top_layer` (e379b925) and `PopupsInWindow` (#1455). In the pin; adopted: src/plugins/host_bar.rs. A plugin's own popup in a window of its own is still over it: `PopupsInWindow` is provided around a plugin by zgx, and follows decision controls alone. |
| Inherited token colours | Text taking its colour from a token around it follows every theme switch (#1439, `d81d2c33`): button labels kept the first switch's colour after a second (setup's Continue and Back on Make it yours). In the pin; no Zeron workaround was taken; tests/onboarding_continue_ink.rs guards the setup footer. |
| Token corners and sides | A token `rounded` or `border` takes its single corners or sides, as a number does (#1415, `b72e15db`). In the pin `d81d2c33`; adopted: the composer's tray is `rounded` with `rounded_tl`/`rounded_tr` = 0 again, not a `corners` function (tests/composer_tray.rs). |
| [modal-over-popups](modal-over-popups/ASK.md) | A modal opening closes every anchored popup outside it, popup windows included (#1419, `fbc62abe`). In the pin `f81d5e99`; adopted: ds/popups is gone (glass's `Popups.track`, the `Popups.modal` wrapping in the command palette, dialog and lightbox, the `popups.dismiss` host action); src/popups_tests.rs stays as the guard. |
| [menu-keeps-selection](menu-keeps-selection/ASK.md) | An editor keeps drawing its selection, inactive, while a context menu that took its focus shows (#1418, `00597c48`). In the pin `f81d5e99`; nothing to change in Zeron (src/editor_menu.rs's `ready()` still puts the menu's selection back for its actions). |
| [look-up-line-editor](look-up-line-editor/ASK.md) | `EditorHandle::look_up` works in an editor shown a line at a time (#1418, `6f8ad524`). In the pin `f81d5e99`; adopted: src/editor_menu.rs's `look_up_by_lines` is gone, Look Up calls zgx's own (editor_menu::tests looks up "world" over the word). |
| [popup-window-zoom](popup-window-zoom/ASK.md) | Popup windows open at their trigger, sized and hit right, in a zoomed window, a zoom set at launch too (#1420, `f81d5e99`). In the pin; no Zeron workaround; src/placement_tests is the scene-level guard (a live check at 90%: a right-click in the composer opens its menu at the click). |
| [popup-layer-shadow](popup-layer-shadow/ASK.md) | A popup finds its card through clear layers: a card with its own shadow turns the window server's off, a translucent one gets AppKit's material cut to its corners (#1390, `b75d8e74f`). In the pin `0d42292ef`; adopted: ds/hover_card frames every node tooltip (the context card, ds.Tooltip) in the menus' own glass (`glass.style(…, "menu")`), and tests/hover_cards.rs requires it. A dismissed hover card leaves no empty glass (`dda319ccd`). |
| [chip-span](chip-span/ASK.md) | Real chip spans (`d6582b326`, `972b05d8f`). In the pin `dda319ccd`; adopted: ds/markdown `mention_chip` is a chip span (no transparent underline, no thin spaces), so a copied message has the names alone (src/chip_copy_tests). |
| [list-in-easing-height](list-in-easing-height/ASK.md) | A `ui.list` in a box whose height eases lays out (`78dcac2a4`). In the pin `dda319ccd`; adopted: ds/session_row `Fold` eases the box with the list straight in it, the held full-height inner box gone. |
| [list-row-focus-reload](list-row-focus-reload/ASK.md) | A reload keeps focus where it was, a list's row included (#1389, `798aac781`). In the pin; adopted: ds/file_tree's own refocus (`keys`, files/workspace's `tree_keys`) is gone; src/palette_focus_tests `a_save_keeps_the_file_trees_row_focused`. Also fixes Escape out of ⌘K focusing "a random control" after a save (the first Tab stop); a save while a modal is open is #1408 (`7e8de187`, in `0e6c32f4`; `a_save_while_the_palette_is_open_keeps_focus_to_give_back` un-ignored). |
| [bare-modifier-keys](bare-modifier-keys/ASK.md) | `ui.modifiers` (`816543a2`). In Zeron's pin `2e22d99d3`; adopted: the sidebar's ⌘1–9 hints read `ui.modifiers.meta`, and src/command_hold.rs (AppKit's monitor) and its NSEvent/block2 features are gone. Kept, small: the shell's commands tell the hints a key went down (a command's key goes to its menu item, never an `on_key`). Follow-up in the ask: a web page with focus may not report ⌘ held. `ui.modifiers` follows ⌘ while a web page or terminal has focus too (`554642d79`, in `dda319ccd`; checked natively by zgx, not by hand here). |
| [layer-redraw](layer-redraw/ASK.md) | A layer rastered while a picture decoded draws again (`2e22d99d3`). In the pin; adopted: ds/wallpaper's invisible style nudge on load is gone (the 320 ms crossfade stays, as design). |
| [markdown-pictures](markdown-pictures/ASK.md) | A lone picture its own `img` block, one among words an inline run, no empty `p` (`97859035`). In the pin; adopted: ds/markdown_pictures is gone, find's blocks come from the same parse, and a picture among a reply's words splits its paragraph around it as the shipping app's does. |
| [focus-scroll](focus-scroll/ASK.md) | Focus scrolls into view, `scroll_padding` (`54364a881`). In the pin; adopted: the transcript keeps a focused row clear of the title fade and the composer (tests/transcript_focus_scroll). Follow-ups in the ask: `scroll_padding` can't follow a state (the composer's height), and two transcript cases. Follow-ups all in the pin `dda319ccd`: `scroll_padding` follows a function (`b5ee4983a`; the transcript and side chats pass the composer's live height, `foot_rest` gone), Tab enters and walks a list past its built rows (`f7a8b003b`; tests/transcript_focus_scroll walks after a chat switch), a `math.huge` offset is a list's end (`beff48c5b`). |
| [runtime-command-keys](runtime-command-keys/ASK.md) | `ui.command { key = state }` and `ui.shortcut_owner` (`faf571b26`). In the pin; adopted: Settings → Shortcuts rebinds at once, refusing a key another command holds. `capture_keys` (`c4cb8c857`) adopted too: the recorder takes every key while it listens, so a ⌘ key another command holds records (and is refused, naming its owner). |
| [inspect](inspect/ASK.md) | Interaction summaries, scripted drags and per-subtree layout attribution: zgx `0421365f2` (#1365) and `91e49539f` (#1375). Needs a pin bump past `91e49539f`. |
| [images](images/ASK.md) | `on_error`, `fallback` (local files too), `on_load({ w, h })` and `ui.image_size` (`84a14cd86`); pixelated (`e3a6555b6`); a GIF's first frame (`f6506b6e6`). In Zeron's pin `f6506b6e6`; adopted: ds/image_guard and the `image.exists`/`image.size` host calls are gone, reply pictures, the lightbox, chips, avatars and the GitHub plugin draw through them, and a GIF in a reply shows its first frame. A GIF plays (`260f0d33a`, in `dda319ccd`), in replies and the lightbox, still while the system reduces motion; src/gif_tests. Open: Zeron's own Reduce motion setting can't hold a GIF still (`ui.image` has no pause); and 4, a picture drawn larger isn't drawn at all until its larger pixels decode (it made the lightbox's zoom flicker; Zeron holds a snapshot under the picture meanwhile). 8 (a `fallback` losing `fit`) fixed in `a2c9cf1e6` (#1417), in the pin; adopted: the composer chip, the dock thumbnail and the avatar have their `fallback` back (src/picture_aspect_tests). |
| [remote-image-retry](remote-image-retry/ASK.md) | A failed remote picture is asked for again while shown, 5 s doubling to 5 min (`2d803ad0c`). In the pin; adopted: the host-in-capitals retry and its probes are gone (ds/people, ds/markdown, the GitHub plugin). |
| [plugin-window-size](plugin-window-size/ASK.md) | Plugin VMs have `ui.window.size` (#1370, `f61333725`). In the pin; adopted: `zeron.window` (src/plugins `share_window_size`, its grant) is gone; ds/lightbox reads `ui.window.size` alone. |
| [glyph-raster-in-paint](glyph-raster-in-paint/ASK.md) | Doesn't reproduce on Zeron's pin `f6506b6e6` (release, cold atlas): streaming a reply, paint max 2.2 ms, p95 0.3 ms; every streamed frame under 5 ms. Closed with zgx's render lane. (A long reply landing whole still takes 13.9 ms, mostly our script: an app-side follow-up.) |
| [markdown-parse-cost](markdown-parse-cost/ASK.md) | `ui.markdown(text, { key, lazy, first })` (#1380, `0dff42bf5`), in Zeron's pin. Adopted: ds/markdown's windowed path parses lazily (pending blocks held by `hint`/`lines`, a find's block `first`); a 400-block reply's landing frame 39.6M → 21.5M instructions (`ui/tests/lazy_markdown.test.luau`). |
| [fullscreen-toolbar-strip](fullscreen-toolbar-strip/ASK.md) | Fixed on zgx main `465858d8` (#1413): the inset toolbar hidden in full screen. In Zeron's pin `0e6c32f4`; checked in code (hidden on will-enter, shown on did-exit, only zgx's inset toolbar); the full-screen look is for a person to check. A failed entry into full screen once left the toolbar hidden: #1414 (`33e02879`) gives it back 2 s later, in the pin `a2c9cf1e6`. |
| [stale-paint-mounted-mid-scroll](stale-paint-mounted-mid-scroll/ASK.md) | A move that repaints also damages where the content was (#1368, `c43b4e5ee`). In Zeron's pin `f6506b6e6`; the app's 90 ms hover rest is gone (ds/transcript), `ui/tests/hover_lane.test.luau` kept. |
| [focus-restore](focus-restore/ASK.md) | Fixed in zgx `f3b6b53e`; in Zeron's pin `aaa304eb2`, focus test un-ignored. |
| [cjk-font-fallback](cjk-font-fallback/ASK.md) | Fixed on zgx main (`f9e2dbce`, PingFang at Regular). In Zeron's pin `03270165c`. |
| [stranded-stretch](stranded-stretch/ASK.md) | Fixed on zgx main `03270165c`; `zgx inspect node` reports scroll offset vs range and any stretch. In Zeron's pin. |
| [span-context-menu](span-context-menu/ASK.md) | `ui.span { context_menu }` and `ui.rich { context_menu = fn(e) … e.span … }` (#1326, `413e6d23`). In Zeron's pin `03270165c`; adopted: every link's menu by its kind (ds/links, ds/menu_at; tests/link_menus). |
| [href-cancel](href-cancel/ASK.md) | A span's `on_click` returning true keeps its href from opening (#1326, `413e6d23`). In the pin; adopted: a web link keeps its href while "Open links in Zeron" takes its click (ds/links `take`). |
| [show-root-placement](show-root-placement/ASK.md) | Before/after at a `ui.show`/`ui.switch`/component root, presence roots too (#1264 `16eef680`, #1291). In the pin. |
| [plugin-packages](plugin-packages/ASK.md) | `Capability::Package(name)`, `grant_all_plugins`, compiled once per process (#1286, #1287). In the pin; not yet adopted. |
| [plugin-shortcut-dropped](plugin-shortcut-dropped/ASK.md) | `ScriptHost::dropped_shortcuts()` and `contributions()` report it (#1264). In the pin; not yet surfaced. |

To add one: a folder named for the topic, an `ASK.md` in this shape, a
repro that fails today, the zgx commit it was seen on, and a row here.
