# Composer focus and layout baseline

This is characterization for the composer refactor, **not a confirmed
reproduction or fix of the intermittent glitch when clicking or typing**.
Production deadline reads delegate to `ds/composer_clock.now`, whose default
is `ui.now`. No layout rules or framework behavior change in this baseline.

## Automated observations

Run from the repository root:

```sh
cargo test --locked --manifest-path apps/zgx/Cargo.toml --bin zeron-zgx route_motion_tests
```

`src/route_motion_tests.rs` mounts the real shell via
`ui/composer_motion_fixture.luau`. Each observed frame records the editor's
node identity, focused node, selection in UTF-8 bytes, committed draft,
transformed field/composer rectangles, ancestor opacity and scene visibility.
Assertion failures include the frame trace for the current interaction.
To retain every observed sample on success too:

```sh
ZERON_ZGX_COMPOSER_TRACE=1 cargo test --locked --manifest-path apps/zgx/Cargo.toml --bin zeron-zgx route_motion_tests -- --nocapture > /tmp/zgx-composer-trace.log 2>&1
```

Trace lines begin with `composer-trace` and include the test name so parallel
tests can be distinguished.

The destination rectangles come from a second shell receiving the same
inputs with Reduce motion enabled. They are an independently laid-out
reference endpoint, not direct access to the animated composer's private
target signals. This avoids reimplementing the composer's geometry formulas
in tests. It also cannot detect a calculation error shared by both shells.

The tests cover:

- New-chat and session composers: focus loss, a real click to regain focus,
  and a selection retained without editing or moving/resizing the field.
- Character-by-character typing through the compact/expanded boundary,
  then backspacing until compact again. The boundary is discovered with
  the current font/layout rather than an assumed character count.
- Multiline growth and shrink in both modes, including more typing while
  growth is still in progress. The test checks that it actually interrupts
  an intermediate geometry, rather than typing after the animation ends.
- New-chat/session route changes: immediate destination geometry, a
  retained editor node, and separate session drafts restored on return.

`TestApp` supplies 60 Hz frames and a controlled timer clock. `ui.now()`
normally uses wall time, so the dedicated fixture replaces `composer_clock.now` with a state
advanced by the tests to the same logical elapsed time. The override is
confined to that fixture's VM and uses `peek()` to avoid making time a new
reactive layout dependency. These tests use no sleeps. The ordinary shell
fixture and the app retain their real clock.

This is headless scene validation. Scene visibility and opacity do **not**
prove that the GPU painted the text or that a presented frame was correct.

## Visual reproduction procedure

Status at implementation: the original visual glitch has not been
reproduced by these headless tests. A valid standalone visual comparison
and the macOS comparison have not been executed in this change. The following is a repeatable diagnostic
procedure, not a claim that these steps reproduce it on every machine.
macOS native-caret/Metal comparison requires a separate run on macOS.

Use the standalone **ZGX runner built from the revision pinned in
`apps/zgx/Cargo.toml`**, with the ordinary shell fixture, not the controlled
clock fixture. Pointer input, timers and presentation then run normally:

```sh
zgx-run --inspect apps/zgx/ui/shell_fixture.luau
```

The standalone runner deliberately omits Zeron's engine host. Launching the
same shell through `zeron-zgx fixture=shell_fixture.luau` is not equivalent:
its host can overwrite the seeded sessions with engine data, or mark the
account signed out after a connection timeout. Do not use that launch as
an isolated visual baseline. Record the actual fonts/fallback used by the
runner, since the Zeron host also registers bundled fonts.

Use a 1200 × 800 logical-point content area at 100% zoom. The fixture window
restores its size, so resize it explicitly and record the actual size.
Start with Zeron dark, transparency on and Reduce motion off. Record display
scale and refresh rate as well. The fixture has made-up sessions and does
not require sending a real agent turn. Its menu command `Fixture: dark`
selects the dark theme (`fixture.dark` through the command/inspect API).

Perform this sequence while recording video at the display's available
frame rate; before/after screenshots alone miss a one-frame glitch:

1. Open **Fix the build**, click its empty composer, click the conversation,
   then click the composer again. Repeat five times without entering text.
2. Type `hello composer`, click at different positions, select `composer`,
   and move the caret with the arrow keys. Do not submit the message.
3. Select all and clear. Type `W` one character at a time until it expands;
   record the character count at that boundary. Delete characters one at a
   time until it folds. Repeat close to the measured boundary.
4. Select all and enter six lines: `one`, `two`, `three`, `four`, `five`,
   `six`. While it is visibly growing, add a seventh line, `seven`.
   Replace the draft with `short` while observing the shrinking field.
5. Open **New chat** and repeat steps 1–4 in the expanded composer.
6. Return to **Fix the build**, then **Write the docs**, give them different
   drafts and switch between them. Check that drafts, focus and geometry
   correspond to the session shown.

Repeat the relevant sequence at 90% zoom, then with transparency off and
with Reduce motion on, changing one condition per run. For a report from
an installed beta, also record its exact version, active template/plugins
and reproduce in that app: a fixture is useful isolation, not identical to
the full live beta environment.

## Diagnostic comparisons

First capture the normal run without either environment variable. On
macOS, compare the same sequence with the native caret disabled:

```sh
ZGX_NATIVE_CARET=0 zgx-run --inspect apps/zgx/ui/shell_fixture.luau
```

Run the partial-vs-full repaint comparison separately, keeping stderr:

```sh
ZGX_DAMAGE_CHECK=1 zgx-run --inspect apps/zgx/ui/shell_fixture.luau 2> /tmp/zgx-composer-damage.log
```

The damage check compares partial output to a full repaint and reports
stale pixels. It forces additional rendering/readback, changes timing and
may mask the glitch. An empty log does not rule out presentation errors;
a changed symptom in this mode is not proof of its cause. Do not enable
either switch permanently or combine them in the first comparison.

## Evidence to attach to each run

Record revision/build and platform, content size, zoom, theme, transparency,
Reduce motion, display scale/refresh rate, active template/plugins, switches
and input sequence (including wrap count). Attach the video with timestamps
and the relevant stderr excerpt. Label each run **observed**, **not
observed**, or **not executed**. If observed, describe what changed without
assuming it disappeared: text, cursor, geometry, background or whole window.

Keep geometry/focus/selection failures separate from pixel or presentation
failures. A passing headless baseline alone does not close the original
visual report.
