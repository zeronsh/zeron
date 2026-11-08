# Composer frame validation (C9)

The structural work C1–C8 is implemented. The reported intermittent glitch
has not been reproduced or established as fixed. Validation below applies to
the exercised sequences; macOS Core Animation/Metal and native IME remain open.

## Automated layout/input coverage

The directed Rust suite passes **30 tests**, including the Luau geometry and
motion cases it invokes. It covers editor identity, focus, byte selection,
compact/wrapped text, empty/final lines, interrupted/reversed motion, scrolling
near maximum height, headers with cargo/questions, stale revisions/context
acknowledgements, cleanup and animation idle. C9 adds dictation layout toggles
and a new-chat/session matrix across dark/light themes, frosted/opaque surfaces,
resize and the host's logical zoom reflow at 90%, 100%, 125%. This models layout,
not device-scale GPU rendering. IME tests inject composition events, without a
native input method or real voice recognition.

```sh
cargo test --locked --manifest-path apps/zgx/Cargo.toml --bin zeron-zgx composer
cargo test --locked --manifest-path apps/zgx/Cargo.toml --bin zeron-zgx zeron_v2
```

The standalone Luau runner (`zgx test apps/zgx/ui composer`) passed 63 cases
and failed one existing side-chat alert case: `Open Alerts` is clipped/hidden.
The same case fails with the identical bounds against archived C4 UI using the
same runner. Three additional new-chat checks (foot overflow, send after growth,
resize without overshoot) passed individually. Existing material/design lint
warnings are visible in those runs.

All five v2 checks passed. The beta build also passed `cargo build --locked
--manifest-path apps/zgx/Cargo.toml --bin zeron-zgx --features beta`.

The broad application suite was also run (four test threads): **546 passed,
7 failed, 20 ignored** before the two new C9 tests were added. Each of these
seven failures was rerun in a separate checkout at C4 (`7b1ca7a4`) with the
original framework pin; all seven fail there too:

- `editor_menu::tests::each_item_acts_on_the_selection_the_menu_opened_on_the_first_time`
- `editor_scroll_tests::a_sideways_swipe_brings_back_what_the_caret_scrolled_away`
- `fit_tests::a_window_resize_moves_everything_one_way`
- `menu_bar_tests::two_plugins_with_one_key_the_first_keeps_it`
- `shortcut_tests::a_rebound_shortcut_takes_effect_at_once`
- `shortcut_tests::the_shortcuts_reach_their_commands_from_any_focus`
- `split_tests::a_dragged_sidebar_edge_builds_no_list`

The suite is therefore not globally green; these pre-existing failures are
separate from composer acceptance. Framework checks and their two baseline
failures are recorded in [composer-editor-api.md](composer-editor-api.md).

## Real presented frames

The repeatable Linux runner is `scripts/composer_frames.py`. It uses an isolated
Xvfb display and fixture-local draft, with separate config/data and no engine
spawn. Build the development binary for the fixture CLI; beta deliberately
omits that CLI. The fixture uses the same composer and framework as beta.

```sh
cargo build --locked --manifest-path apps/zgx/Cargo.toml --bin zeron-zgx
python3 apps/zgx/scripts/composer_frames.py --binary apps/zgx/target/debug/zeron-zgx --output .personal/composer-c9/normal
python3 apps/zgx/scripts/composer_frames.py --binary apps/zgx/target/debug/zeron-zgx --output .personal/composer-c9/damage --damage-check
```

Requires Python, Xvfb, xdotool and ffmpeg. Each run records 12 seconds at 60 fps
(1280×900 capture, 1200×800 content), Zeron dark, 100% zoom, default frosted
surfaces. The sequence covers short/10-line/80-line drafts, rapid interruptions,
attachment header changes, wrap/unwrap, scrolling followed by character input,
and reduced-motion snaps. The runner records event times, per-frame ink counts,
process CPU samples and damage-check diagnostics, and fails on a missing text
region or reported stale pixels.

| Run | Captured frames | Minimum text-region ink pixels | Suspect frames | Damage mismatches |
| --- | ---: | ---: | ---: | ---: |
| Normal | 720 | 95 | 0 | N/A |
| Diagnostic partial/full comparison | 720 | 95 | 0 | 0 |

[Normal report](results/composer-2026-11-08/normal.json),
[diagnostic report](results/composer-2026-11-08/damage.json),
[normal contact sheet](results/composer-2026-11-08/normal.png),
[diagnostic contact sheet](results/composer-2026-11-08/damage.png).
The contact sheets were visually inspected; full recordings remain locally
under `.personal/composer-c9/` to avoid committing video binaries.

The fixed text region excludes toolbar, footer and controls. Its >50 bright
pixel criterion cannot be satisfied by the caret alone, so normal blink is
not a failure or a substitute for text. This detects whole-region absence;
it can miss local missing glyphs, a brief clipped line or header text masking
a missing draft. Exact partial/full pixel comparison adds coverage of damage
errors; it does not prove the scene was correct in the first place.

`ZGX_DAMAGE_CHECK` forces a comparison repaint and may alter the symptom.
The normal recording is therefore a separate run. Recording at 60 fps can
also miss a shorter presentation glitch. Three 0.6-second idle CPU samples
were 0%, approximately 1.67%, 0% in each run (one clock tick in the middle
interval). Blink and engine reconnects remain valid background work; these
short samples are not a performance benchmark. Controller tests separately
assert no animation frame requests after completion/unmount.

## Remaining platform evidence

macOS and Windows presentation, real native IME, native zoom/device-scale
changes and the original intermittent reproduction need the target device.
Follow [composer-characterization.md](composer-characterization.md), including
macOS runs with native caret on and `ZGX_NATIVE_CARET=0`. Retain both normal and
partial/full diagnostic recordings, including platform, window size, theme,
zoom, transparency and exact input sequence. Passing headless or Linux checks
must not be described as verification of Core Animation/Metal presentation.
