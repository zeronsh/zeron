# Coordinated composer transitions

`ui/ds/composer_motion.luau` owns one origin, destination and progress
animation per composer. All view bindings read its complete `frame`
snapshot. Selecting height, text or control positions starts no additional
animation. The duration remains 220 ms with `out(3)` easing.

## Retargeting and lifecycle

A new visual destination captures the currently presented snapshot as its
origin, cancels the old clip, and starts a single new progress animation.
It never starts from an unfinished transition's old destination. Reversing
direction before any frame, when already at the new destination, settles
without creating an unnecessary animation.

Changes to natural content height above the viewport cap, text-hysteresis
metadata, or auxiliary constraints do not restart a transition when the
visual destination is unchanged. The destination metadata is still
updated; the same progress continues with the latest constraints.

Reduce motion is tracked, including changes during a run. It cancels the
clip and snaps all geometry together. A changed `props.snap` does the same
for context switches. C6 keeps that snap until the current metric revision
is valid and the destination has been laid out; its deferred acknowledgement
checks context and revision instead of waiting 400 ms.

Cleanup cancels the clip and marks the controller inactive. Completion
callbacks check their generation and whether the controller is alive,
so an obsolete callback cannot revive a replaced or unmounted run.

## Width and viewport policy

Interpolating the editor's left/right insets would change its wrapping
width on every frame. This controller instead uses two discrete widths
for a mode change at a fixed surface/control width:

1. On expansion, retain compact width until the vertical envelope holds
   a complete line and its bottom has cleared both control groups.
2. Switch to expanded width once. Position and height continue on the
   same vertical trajectory; the width switch does not also jump them.
3. On contraction, take compact width immediately and keep animating
   the vertical envelope. Compact controls stay beside the text.

Snapping to full expanded width immediately at compact height would put
text under the buttons. Conversely, moving the text to its final top only
at the width switch produced a vertical jump during the initial visual
check. The final policy animates the vertical envelope throughout and
delays only the width switch until it is safe.

The viewport is derived from that frame's surface height, top and bottom
reservation, including borders. This preserves the clip relationship
through interruptions; sharing progress alone would not guarantee it.
The editor retains responsibility for caret reveal, scroll and shaping.
The controller never edits the draft, selection or editor scroll offset.

The natural header is measured inside a separate clip whose presented
height follows the same progress. The animated clip's height is never
fed back into the natural header measurement. Layout constraints and
compact/control sizes still come from `composer_layout.luau`.

## Validation

```sh
cargo test --locked --manifest-path apps/zgx/Cargo.toml --bin zeron-zgx composer
```

The targeted suite has 25 Rust tests, including bridges to eight layout
and four transition cases in Luau. It checks:

- A single discrete width change in both pure geometry and the real editor;
  continuous vertical geometry and clip/control bounds around that change.
- Retargeting growth and reversing shrink from the presented snapshot.
- Unchanged destinations retaining progress, including content above the cap.
- Context and Reduce motion snaps, including cancellation during a run.
- Unmount/remount, frozen disposed progress and no remaining frame requests.
- Real editor identity, focus and selection, typing during growth, actual
  scrolling at the cap, and caret reveal after typing into that scrolled draft.

For manual visual checks, the preview uses the real design-system composer
with local drafts and header data. Host engine/account updates cannot
replace those inputs:

```sh
composer_preview_run=$(mktemp -d)
ZERON_ZGX_ENGINE_SPAWN=0 ZERON_ZGX_ENGINE_PORT=27963 \
ZERON_ZGX_ENGINE_DATA="$composer_preview_run/engine" \
cargo run --locked --manifest-path apps/zgx/Cargo.toml --bin zeron-zgx -- fixture=composer_transition_preview.luau --inspect
```

Use Short, 10 lines and 80 lines quickly to interrupt both directions.
Header toggles a real attachment. Wrap supplies a long line. Presets focus
the editor at the end; scroll up and type to check caret reveal. Repeat
with Reduce motion enabled and at different window widths/zoom levels.

A local Linux X11/Xvfb run used 1200 × 800 content, 100% zoom, Zeron dark
and default frosted surfaces. A 12-second recording at 60 fps captured
720 frames, covering rapid interruptions, header changes, wrapping,
scroll/edit and Reduce motion. Transition samples and final captures were
visually inspected. An automated check found no entirely empty selected
text region across the 720 frames (minimum 95 grayscale pixels above 120).
This check excludes toolbar/footer/buttons but can miss local painting
errors; it is not a pixel-accurate rendering test. Local evidence is in
`.personal/composer-c4/` (ignored by Git).

macOS Core Animation/Metal presentation was not exercised here. This work
does not establish a reproduction or confirmed fix of the original glitch.
