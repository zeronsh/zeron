# Native composer stack evidence

Synthetic native GPUI fixture at clean source `e3961051f127f39938931652eea4ee434f031a08`, dev profile, `appshots-fixture` feature. One Codex harness is selected. Both images are 440×520 logical pixels (880×1040 raster), frosted material, standard motion. No real conversations or credentials are included.

- `dark-minimum.png`: goal/plan/task activity, queued follow-ups, durable pending question and answer editor coexist at the minimum size.
- `light-question-scrolled.png`: actual wheel events reach the last question choice while the surrounding stack remains present. Independent wheel assertions also reach the final queued row and final activity task without moving sibling trays.

These images establish the shown layouts and scroll reachability, not smoothness or live provider end-to-end behavior. Native keyboard assertions cover Tab, Enter, Escape and Space on activity disclosure. The fixture source reproduces these states; supply an output directory, with `ZERON_FIXTURE_SURFACE=opaque` or `ZERON_FIXTURE_REDUCE_MOTION=1` for additional variants.

The normal requested 840×960 window was clamped by the test machine to 840×816; minimum size was not clamped. Current native offscreen profiling could not complete because the build reached the workspace storage floor, so this evidence makes no current frame-time or idle-CPU claim.
