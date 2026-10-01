# Android: tab switch and screen-open performance

Reported: "Settings -> Sessions takes about 200 ms to open." Reproduced on the
x86_64 emulator (demo workspace, debug build): the Sessions/Settings switch in
the bottom bar cost 150-250 ms of main-thread time per switch, in both
directions. This note records the root causes, the fixes and how to re-measure.

## Root cause

`Home` swapped the page with `when (tab)`: switching **disposed** one tab's
whole composition and **composed the other from scratch**, inside a single
frame. Measured contributors (main-thread stack sampler, `Perf.kt`):

| Contributor | Where | Share of the Sessions cold composition |
| --- | --- | --- |
| `LoadingIndicator` per *working* row: `morphSequence` builds shape-morph tables on every first composition (and the pull-to-refresh indicator built a second set that nothing shows until a pull) | `StatusLabel`, `PullToRefreshBox` | about a third |
| `SegmentedListItem` + `SwipeToDismissBox` per visible row, lazy measure | `SessionsScreen` | about a third |
| Vector icons parsed on first use (`painterResource`), ~20 on Settings | `ZIcon` | 5-15 % |
| Everything above repeated on **every** switch because nothing was retained | `Home` | 100 % |

Two further findings while retaining the pages:

* `RunningPill` (a running subagent's breathing dot) **recomposed every frame**
  (`breath()` returned the animated value into composition), and every
  `LoadingIndicator` is an infinite animation. Kept alive behind the other tab
  these would keep the app rendering at display rate with nothing visible
  (120 frames in 3 s on Settings), so hidden pages must hold still.
* NavHost's default 700 ms fade in/out made every pushed screen (Engine,
  Coding agents, Transfers, Sounds, Search, New session) *start transparent and
  take most of a second to arrive*, regardless of how fast it composed.

## Fixes

* **Both tabs stay composed** (`ui/TabHost.kt`). `TabPage` keeps a page's
  composition and display lists and shows/hides it with a layer alpha, a
  placement z-index and `hideFromAccessibility()` (touches and TalkBack only
  see the shown page). A switch is a property change: no composition, no
  re-record. The page state (`tab`, `ready`) is read only in layer, layout,
  semantics and effect blocks, so a switch recomposes next to nothing (the
  home chrome and the two small page wrappers).
* **The other tab idles in** after the first frames (`HomeTabs`): the tab shown
  composes at once, the hidden one ~350 ms later. A tab asked for before that
  paints a **wireframe** (`SessionsSkeleton` / `SettingsSkeleton`, static, no
  animation) for the one frame it takes. The Sessions list also shows row
  wireframes until the first workspace snapshot instead of a blank page.
* **Hidden pages are quiet.** `collectAsStateWhile()` stops collecting the
  workspace / connectivity / engine / transfers flows 2 s after the page is
  hidden (quick back-and-forth costs no recomposition at all) and re-reads the
  current value synchronously when it is shown. `LocalMotionActive` stops
  looping animations when hidden and starts them one frame after the page
  appears; `breath()` returns a `State` read in a draw block, so `RunningPill`
  no longer recomposes per frame.
* **Spinner staging.** A working row paints a still dot first and takes the
  `LoadingIndicator` one frame later, one spinner per frame (`SpinnerQueue`);
  the pull-to-refresh indicator composes a frame after the page. The costliest
  composition in a row is thus off the first frame.
* Navigation fades are 180 ms in / 120 ms out (was 700/700).

## Numbers

x86_64 emulator, software GL, demo workspace, host load average 15-25 on 16
cores, so absolute values are 2-4x what a phone shows; compare the columns.
`cpu` is main-thread CPU time from the request to the start of the second frame
(steady under load). `wall` includes waiting for the emulator's software
renderer. Produced by `scripts/android/measure-tab-switch.sh --ab -n 8` (the
`[rebuild]` rows run the old behaviour in the same build, interleaved).

| Switch | Before: cpu / wall | After: cpu / wall |
| --- | --- | --- |
| Settings -> Sessions | 97 ms (76-220) / 117 ms | **9 ms** (7-17) / 34 ms |
| Sessions -> Settings | 72 ms (55-125) / 113 ms | **11 ms** (8-17) / 67 ms |
| Cold open of the home page (return from a chat), Sessions | no separate figure: the old switch *was* a cold open, so the rows above plus the chrome | 79 ms / 109 ms |
| Cold open, Settings | as above | 61 ms / 68 ms |
| Frames rendered in 3 s idle on Settings (Sessions kept composed) | 120 (kept alive, animating) | **0** |

The original build (before any fix) measured 184-344 ms wall, 146-211 ms cpu
for Settings -> Sessions on a calmer host: the reported ~200 ms.

Real taps on the nav bar (with haptic and sound feedback attached) measure
16-35 ms cpu in the same build; the remainder is the press ripple, the nav
indicator's colour animation and the feedback calls.

## What could not be verified

* No arm64 phone was available. The fixes are structural (no composition on
  switch, no unseen animation, staged heavy composables), so they help there
  too, but the 16 ms frame budget was only checked in relative terms on the
  emulator. The emulator's wall time is dominated by its software renderer
  (`syncAndDrawFrame` in the sampler), which a phone's GPU does not share.
* Other screens opened from Settings (Engine, Coding agents, Transfers, Sounds,
  Search, New session) cost roughly 45-230 ms cpu over 12 frames including the
  transition itself (`kind route`, below); they were not individually optimised
  beyond the shorter fades. `SegmentedListItem` composition is the next target.

## Re-measuring

```sh
./gradlew :app:installDebug
scripts/android/measure-tab-switch.sh -s emulator-5580 --launch --ab -n 8   # old vs new, interleaved
scripts/android/measure-tab-switch.sh --cold -n 8                           # rebuild the home page
adb logcat -s ZeronPerf                                                      # raw lines
```

Debug-build broadcasts (`adb shell am broadcast -a sh.zeron.android.DEBUG_EVENT -p sh.zeron.android ...`):

| Extras | Effect |
| --- | --- |
| `--es kind tab --es tab sessions\|settings` | switch tab; add `--ez cold true` to rebuild the home page, `--ez retain false` for the old rebuild-per-switch, `--ez sample true [--ei tail 250]` to log a main-thread stack profile of the switch |
| `--es kind route --es route engine\|agents\|transfers\|sounds\|search\|new` | time a screen opening (12 frames) |
| `--es kind profile --ei ms 3000` | stack-sample the main thread for a while (what an idle screen costs) |

`core/Perf.kt` logs per switch: wall and main-thread CPU from request to the
second frame, the per-frame UI-thread time (FrameMetrics: input + animation /
recomposition + measure/layout + draw) of the first frames, and the sampler's
inclusive/self frame counts. Never use `adb exec-out screencap` on the emulator
(it crashes system_server); `adb emu screenrecord` works.
