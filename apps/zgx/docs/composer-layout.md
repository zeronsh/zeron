# Composer target geometry

`ui/ds/composer_layout.luau` is the pure calculation for the pill, text
viewport and control positions. It reads no UI state, theme, time or refs,
and mutates neither its input nor expansion history. It centralizes the
compact line size, border, insets, text min/max and hysteresis thresholds.

## Inputs and outputs

`calculate(input, previous_text_expanded)` receives available pill width,
the complete last text-metric sample, measured trailing controls/header,
resolved theme corner and header gap, and the explicit mode flags
(expanded, cargo, asking, dictating, multiline). A missing sample and an
unknown width have explicit fallback behavior. As in C2, the integration
keeps the last complete sample while a new request is stale.

The result contains:

- Effective `mode`/`expanded` and `next_text_expanded` for the next input.
- `natural_height`, before the text box's minimum/maximum, and `height`,
  the limited target pill height including header, actions and borders.
- The text rectangle: left/right/top insets, width and viewport height.
- Leading/trailing control anchors and the dictation track's anchors.
- The tray chip inset, aligned with the same resolved control corner.

Coordinates are local to the surface's content box. The text width excludes
the surface border and both text insets; it never supplies the width used
to measure text during an animation. `measurement(width)` shares the
expanded-width calculation with C2's provider without creating a dependency
cycle between measuring content and calculating the resulting height.
The view retains its radius, typography, voice-track height and decoration
tokens; the calculation receives resolved values where numeric geometry
depends on theme tokens. `control.nest` still resolves special pill radii.

## Explicit hysteresis

Text expands above its compact capacity and contracts only below the
existing smaller threshold. The pure function receives the previous text
decision and returns the next one. No computed callback mutates a captured
`was_open` variable anymore.

The **effective mode** and **text history** are intentionally distinct.
As before, a question, cargo, dictation or forced expanded form opens the
composer without overwriting text hysteresis. Unknown width preserves the
previous text history too. Explicit multiline text does update it once
width is known. This distinction preserves behavior after forced modes end.

`composer.luau` gathers the inputs in one place, invokes the calculation
in an effect, and commits the next history plus `target_geometry` in one
batch. A scalar selector takes each destination from this single result.
Unchanged destinations retain their values.

## Target, presented and published sizes

`target_geometry` describes the destination, not an intermediate frame.
The `presented` helper still applies the existing independent scalar
animations, easing, duration and snap policy. Coordinating the animations
belongs to C4; this change does not implement that policy.

`natural_height` is not the box's displayed height and is not published to
the transcript. `screens/chat/composer.luau` continues to publish the actual
laid-out dock height through `shared.dock_h`, as before. Page placement,
footer folding and deck/tray behavior retain their own responsibilities.
The footer simply obtains its existing chip alignment from the shared
geometry result rather than repeating a control-inset calculation.

## Validation

```sh
cargo test --locked --manifest-path apps/zgx/Cargo.toml --bin zeron-zgx composer
```

`ui/tests/composer_layout.test.luau` tests eight cases through the Rust
test runner: golden compact geometry, both exact hysteresis boundaries,
forced modes/history, missing measurements, natural vs limited height,
cargo/questions with headers, theme corners, and deterministic calculation
against frozen inputs. Expected sizes are examples and invariants, rather
than a second implementation of the layout formulas.

An integration test adds/removes a real cargo header and question, checking
editor identity, focus, selection, draft retention, header/text separation
and return to compact geometry. C1/C2 characterization and metric tests
remain part of the same targeted suite. These are headless checks; they do
not establish that the original visual glitch is fixed.

The C1 before/after trace comparison against C2 covered 786 samples. After
normalizing editor node IDs, all observed values matched, including target
rectangles and intermediate presented geometry, focus, selection and draft.
