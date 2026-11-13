# Composer

The compact and expanded composers use the same `ds.Composer` component and
retain the same textarea while changing mode. The component owns text sizing
and internal geometry; the chat screen owns placement on the page. Main-chat,
side-chat and v2 consumers share the sizing contract.

## Structure and ownership

| Module | Responsibility |
| --- | --- |
| [`ds/composer.luau`](../ui/ds/composer.luau) | Assemble the editor, header, controls, footer and trays; resolve theme inputs; publish sizes. |
| [`ds/composer_metrics.luau`](../ui/ds/composer_metrics.luau) | Adapt the editor's natural text metrics into coherent, revisioned samples. |
| [`ds/composer_layout.luau`](../ui/ds/composer_layout.luau) | Calculate the complete target geometry and explicit text-expansion history. |
| [`ds/composer_motion.luau`](../ui/ds/composer_motion.luau) | Present one geometry snapshot per animation frame and handle interruption/snap. |
| [`screens/chat/composer.luau`](../ui/screens/chat/composer.luau) | Bind session state and place the main composer; retain the model-picker anchor and animate new-chat sends. |
| [`screens/chat/composer_place.luau`](../ui/screens/chat/composer_place.luau) | Calculate page-placement offsets from published sizes. |
| [`screens/chat/dock.luau`](../ui/screens/chat/dock.luau) | Dock at the page's foot and calculate transcript clearance. |
| [`screens/chat/lane_tab.luau`](../ui/screens/chat/lane_tab.luau) | Bind a side chat and publish its presented composer height. |

The data flow is editor metrics → target geometry → presented geometry →
view and published sizes → page placement/transcript clearance. Footer folding
and deck/tray layout keep their own responsibilities; their measurements feed
the size contract without supplying text-layout rules.

## Modes and visible layout

In compact mode, one line of text sits between the leading attachment control
and trailing controls. Text exceeding the available compact width expands the
composer; an explicit newline also expands it when width is known. New-chat
placement forces the expanded form. Attachments, an active question and
dictation also force expansion.

Expanded text occupies the width above the controls, with 16-point left/right
insets. Attachments and questions contribute a measured header above the text.
The text viewport grows with its natural wrapped content up to its limit;
additional content scrolls inside the editor. The editor manages caret reveal
and scrolling. Its native placeholder follows IME composition; a press on blank
composer space focuses the draft without a second focus change from the press
layer. Footer chips sit below the surface, and trays/deck content keep
their existing layout and interaction behavior.

The pure `Layout.calculate(input, previous_text_expanded)` receives available
width, the last text sample, measured controls/header, resolved theme geometry
and mode flags. It returns mode, natural/limited surface height, text rectangle,
control anchors, tray inset and the next text-expansion decision. It reads no
UI state or time and does not mutate its inputs or hidden history.

The current geometry constants are local logical points:

| Constraint | Value |
| --- | ---: |
| Surface border | 1 per side |
| Compact inner height / total surface height | 47 / 49 |
| Compact text viewport / fallback line height | 22 |
| Expanded text side insets | 16 per side |
| Expanded text top/bottom reservation without a header | 16 / 4 |
| Expanded text box, including that top/bottom reservation | 76–260 |
| Expanded text viewport | 56–240 |
| Leading / trailing control height | 28 / 32 |
| Text-contraction hysteresis | 32 |

Expanded total height adds the controls' reserved area, borders and any header
adjustment to the limited text box. The controls reserve at least 42 points;
the resolved control corner can increase that reservation. Header gap, corner,
typography and decoration still use the theme's tokens. `natural_height` is
computed before the text-box clamp; it is not a displayed height or transcript
reservation.

Text expansion uses hysteresis: compact text expands above its capacity and
an expanded draft contracts only at or below capacity minus 32 points. The
previous decision is an explicit input. Forced expansion does not overwrite
text history. Unknown width preserves that history; with known width but no
sample, the previous text decision supplies the fallback. The view commits the
next decision and target geometry together in a batch.

## Text metrics

`Metrics.use { value, width, editor, keep? }` returns a computed `result` and
creates no measurement nodes. The visible textarea supplies
`Element:text_metrics(content_width)` (Rust: `EditorHandle::text_metrics`).
It uses the editor's plain/rich shaping, spans, fonts and fallback, includes a
final empty line and displayed IME composition, and excludes placeholder,
padding, border and gutter.

The requested expanded content width is the pill width minus both borders
and both 16-point insets. It does not follow the animated textarea's insets,
height or scroll, avoiding feedback from an intermediate viewport into the
next target geometry.

| Result field | Meaning |
| --- | --- |
| `revision`, `width` | Current request generation and expanded content width. |
| `status` | `pending` before any sample, `stale` while retaining an older sample, `ready` for the current request. |
| `sample.unwrapped_width` | Natural width used for the compact-mode decision. |
| `sample.wrapped_height`, `sample.line_height` | Natural wrapped height at the requested width and base line height. |
| `sample.width`, `sample.revision` | Width and request generation represented by the complete sample. |

Requests follow draft, width, theme and settled editor metrics, including
uncommitted IME. Deferred publication checks request and editor revisions;
replacement or unmount cancels obsolete callbacks. Width and height publish
together. Zero width of measured text is valid for an empty draft, not an
initialization sentinel; an unknown/non-positive available width cannot
publish a fresh sample.

With `keep`, the last complete sample survives reload but starts stale until
revalidated against the mounted editor. Layout retains the last sample while
waiting; before the first sample it uses its explicit geometry fallback.
The persistence namespace is `editor_text_metrics`.

Framework metrics are exact. The first query per revision/width visits all
hard lines using retained shaping caches; unchanged queries are constant-time.
Full-document queries can be expensive for long drafts after edits. Focus,
selection, scroll, caret blink and viewport size alone do not change the
metric revision. The API is supplied by the immutable ZGX revision pinned in
[`Cargo.toml`](../Cargo.toml), currently from `jsgrrchg/zgx`.

## Geometry transitions and context changes

One controller keeps origin, destination and progress, with a 220 ms `out(3)`
animation. Every view binding reads its complete presented snapshot. A changed
destination cancels the previous animation and starts from the geometry
currently presented. An unchanged visual destination retains progress, even
if natural height above the cap or other metadata changes.

The vertical envelope and controls move together. During expansion, the editor
keeps compact width until a complete line fits above both control groups, then
switches once to the expanded width. During contraction, it takes compact width
immediately. The text top and height continue on the same vertical trajectory;
width is not continuously interpolated during a mode change.

The viewport derives from that frame's surface, top and bottom reservations.
A separate animated clip surrounds the naturally measured header; the clip's
height never feeds back into the header measurement. These transitions retain
the textarea, draft, focus and selection, and do not write editor scroll.

Reduced motion cancels animation and snaps to the destination. A context change
also snaps, holding that policy until the current metrics are ready and the pill
has been laid out at its target height. A deferred acknowledgement verifies
context, revision and destination before ending settlement. Page placement uses
the same context acknowledgement. Cleanup cancels animation/acknowledgement;
obsolete generations cannot update a replacement controller.

## Published sizes and page placement

`Composer.on_sizes` reports:

| Field | Meaning |
| --- | --- |
| `rest` | Surface height calculated with no draft/cargo/question/dictation, retaining the forced expanded flag, plus the currently measured footer and deck. |
| `target` | Target surface height plus measured footer and deck. |
| `presented` | Actual laid-out root height, including footer/deck. |
| `active` | Whether the internal geometry transition is running. |
| `ready` | Current metrics are valid and the pill is laid out at target height. |
| `settling`, `context` | Context snap status and the generation the snapshot belongs to. |

A new chat stands near the page's middle, under the title bar. Its top is based
on `rest`, so typing grows the composer downward without moving that top. If
the presented box would overflow the page's foot, placement lifts it by the
required amount. A session anchors at the bottom with the dock's 8-point gap.
The main chat publishes presented height through `shared.dock_h`; side chats
publish the same actual-height contract for their transcript clearance.

While a new chat's model picker is open, a height compensation keeps its chip
and picker anchored. Closing the picker releases that compensation with a
260 ms `out(3)` animation. Sending from a new chat deliberately glides to the
session dock with the same duration. Other route/split-context changes snap;
reduced motion snaps these page movements too. Sidebar/page resizing follows
layout. The rest height and context settlement use no fixed waiting window.

## Verification and visual diagnostics

Run from the repository root:

```sh
cargo test --locked --manifest-path apps/zgx/Cargo.toml --bin zeron-zgx composer
cargo test --locked --manifest-path apps/zgx/Cargo.toml --bin zeron-zgx zeron_v2
```

The tests cover mode boundaries, headers, metrics/IME, interruption, context
revisions, cleanup, editor identity/focus/selection, max-height scrolling,
dictation layout, theme/transparency and logical resize/zoom reflow. Route-motion
checks compare against a second, reduced-motion shell. Set
`ZERON_ZGX_COMPOSER_TRACE=1` and use `-- --nocapture` to retain per-frame traces.

For a local visual check, build the development binary (beta omits the fixture
CLI) and run the isolated composer preview:

```sh
cargo build --locked --manifest-path apps/zgx/Cargo.toml --bin zeron-zgx
python3 apps/zgx/scripts/composer_frames.py --binary apps/zgx/target/debug/zeron-zgx --output .personal/composer/normal
python3 apps/zgx/scripts/composer_frames.py --binary apps/zgx/target/debug/zeron-zgx --output .personal/composer/damage --damage-check
```

The Linux runner needs Python, Xvfb, xdotool and ffmpeg. It records real frames,
input timing, text-region ink counts and partial/full repaint diagnostics.
The region check can detect whole-region disappearance but can miss local
clipping or missing glyphs. Diagnostic repaints alter timing, so always retain
a separate normal run. On macOS, compare native caret behavior separately with
`ZGX_NATIVE_CARET=0`; record build/platform, window size, zoom, theme,
transparency, display scale/refresh rate and the exact input sequence.

The reported intermittent glitch has not been established as reproduced or
fixed. Headless geometry and Linux captures do not verify macOS/Windows
presentation, native IME or device-scale rendering. Existing broader-suite
failures should be assessed separately from these composer checks.
