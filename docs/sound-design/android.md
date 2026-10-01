# Android: sounds and haptics

The phone's sensory layer: how Zeron for Android feels and sounds. It reuses the
desktop's notification family (`done`, `request`, `attention`, see
[README.md](README.md)) for session events, promotes five of the desktop
auditions, and adds thirty-one small interface cues in the same "rounded pressure
pulse" language (fifteen for the interface itself, five for thinking power and fast
mode, eleven provider motifs). Code: `apps/android/app/src/main/java/sh/zeron/android/feedback/`.
Asset QA: [android-audit.md](android-audit.md).

## Principles

- **Restraint.** Feedback answers a person or announces a one-shot event. There
  is none on scrolling, none per recomposition, none on typing, none for a state
  that merely redraws. Interface cues sit 2 to 4 dB under the session chimes
  (active RMS, at any slider position; see the audit) and last 34 to 176 ms
  against 520 to 650, so they read as far quieter; the shortest are under 40 ms.
- **Consistency.** One vocabulary (`Feedback.kt`: `Haptic`, `Cue`) names the
  *moment*, never the vibration. A tap is always `Select` + `Tap`, a choice
  `Select` + `Select`, an irreversible step `Heavy` + `Delete`. Screens never
  build waveforms.
- **Meaning by pitch.** Rising means opening, turning on, adding, refreshing.
  Falling means closing, turning off, removing, failing. Lower and heavier means
  more destructive (Delete is the lowest cue). The audit checks the directions
  numerically (Open rises, Close falls, ToggleOn rises, ToggleOff falls).
- **Haptic and sound as a pair.** Every cue has a partner haptic that lands on
  the same instant and is just as large (a soft tap, a click, a rising pair, a
  thud). Either channel is complete without the other: silent mode keeps
  haptics, haptics off keeps sounds.
- **One key.** Everything is built from the C major pentatonic scale (C D E G A),
  which cannot clash with itself or with the desktop chimes. The slider detent is
  one pure 880 Hz tick resampled up a pentatonic ladder, so dragging "plays"
  the scale.
- **Accessibility.** One master switch above two independent ones; haptic
  strength scales; the in-app switches are the single source of truth (the phone's
  own touch-sound and touch-vibration settings are not consulted, see "When it
  plays"); nothing is the only carrier of information (every cue accompanies visible
  state).
- **Battery and calm.** Sounds are preloaded in a `SoundPool` (no decoding on
  press, no audio focus, nothing held open). Haptics are short; light ones are
  rate-limited so a fast drag never buzzes continuously.

## When it plays

| Rule | Detail |
| --- | --- |
| App active | Only while the app is in the foreground **and** the screen is on. Otherwise nothing plays in-app. Events that matter in the background arrive as notifications (below). |
| Master | **Sounds & haptics** is absolute. Off: nothing plays, no sound and no vibration, in the app and in the notifications it posts (they become silent). On: the rows below decide. |
| Haptics | Needs the **Haptics** switch and a vibrator. |
| Sounds | Needs the **Sounds** switch and the category switch (Interface, or Session sounds with Completion / Input required / Errors). |
| Not consulted | With the master on, the phone's *Touch sounds* and *Touch feedback* settings, the ringer mode and Do Not Disturb are **not** read: the user said play, so the app does not second-guess it. The only thing left outside the app is physical: Android itself may still mute the audio stream (silent or vibrate mode and total-silence Do Not Disturb mute the system-sound stream, volume zero is silence); see "If you hear nothing". |
| No audio focus | Cues are `USAGE_GAME` / `CONTENT_TYPE_SONIFICATION` (game sound effects: media volume, capturable by screen recorders) and never take focus, so music keeps playing and nothing ducks. |
| Rate limits | The same cue never stacks inside its own gap (60 to 400 ms by cue); any two cues of equal or lower priority keep 20 ms apart; at most 4 concurrent streams (a more important cue still gets in); the same haptic is coalesced (70 ms for ticks up to 400 ms for alerts); at most 12 light haptics per second; any alert cuts through light ticks. |
| Order | Wherever a sound and a haptic belong to the same moment, the **sound is issued first**, then the haptic, from the same synchronous call (`Feedback.both`, `Feedback.play`, the default tap, previews). See Latency. |
| Default tap | Plain controls answer `Select` + `Tap` *after* their action; if the action asked for feedback of its own (a toggle, a menu item, a navigation's Open cue) the default stays out of the way. Menus and popovers close silently after a chosen item. |

## Haptic mapping

`HapticDesign.kt` walks a ladder per haptic: the platform's own
`performHapticFeedback` constant (OEM tuned, so it feels native) when the
strength is Standard, then a `VibrationEffect.Composition` of primitives (only
those `areAllPrimitivesSupported`), then the designed waveform (amplitude control),
then a predefined effect, then plain on/off timings. minSdk 29, so every API-gated
constant has a fallback.

| Haptic | Moment | Standard (API 34+) | Primitives | Fallback |
| --- | --- | --- | --- | --- |
| `Tick` | slider / detent, terminal key, disclosure | `SEGMENT_FREQUENT_TICK` (`CLOCK_TICK` below 34) | LOW_TICK 0.5 | `EFFECT_TICK` / 10 ms @40 |
| `Select` | tap, tab, choice | `SEGMENT_TICK` (`CONTEXT_CLICK`) | TICK 0.7 | `EFFECT_TICK` |
| `ToggleOn` | switch on | `TOGGLE_ON` | LOW_TICK 0.5 then TICK 0.8 (rising pair) | `EFFECT_CLICK`, two-step waveform |
| `ToggleOff` | switch off | `TOGGLE_OFF` | TICK 0.7 then LOW_TICK 0.4 (falling pair) | `EFFECT_TICK`, two-step waveform |
| `Press` | press-and-hold begins | `GESTURE_START` | CLICK 0.4 | `EFFECT_TICK` |
| `Confirm` | send, save, create, start | `CONFIRM` | CLICK 0.65 | `EFFECT_CLICK` |
| `Success` | turn finished, transfer arrived | designed | QUICK_RISE 0.45, LOW_TICK 0.55 @40 ms | CLICK+TICK / waveform |
| `Attention` | a question, connection lost | designed | TICK 0.85, TICK 0.85 @90 ms | `EFFECT_DOUBLE_CLICK` |
| `Error` | failure, refusal | `REJECT` only as fallback | CLICK 1.0, THUD 0.8 @60 ms | `EFFECT_HEAVY_CLICK` |
| `LongPress` | long press recognised | `LONG_PRESS` | CLICK 0.7 | `EFFECT_CLICK` |
| `Threshold` | swipe or pull crossed its commit point | `GESTURE_THRESHOLD_ACTIVATE` | CLICK 0.55 | `EFFECT_CLICK` |
| `Heavy` | delete, uninstall, discard | designed | THUD 0.85, CLICK 0.5 @30 ms | `EFFECT_HEAVY_CLICK` |
| `Pop` | pinned, starred | designed | TICK 0.8, LOW_TICK 0.35 @25 ms | `EFFECT_TICK` |
| `EffortStep` (level 0..1) | a thinking-power detent | none (platform constants are too faint) | CLICK 0.70 to 1.00; from level 0.2 a THUD 0.25 to 1.00 @10 ms; from 0.75 a second CLICK 1.0 @22 ms. Without THUD: two CLICKs; without CLICK strength: two TICKs | `CLICK` below level 0.4, `HEAVY_CLICK` above; waveform 10 ms @150-255, gap, 10-38 ms @110-255 |
| `Stretch` (level 0..1) | the effort thumb pulled past the end | none | TICK 0.25 to 0.70 (firmer the further it is pulled); from level 0.6 a LOW_TICK drag 0.35 to 0.80 @12 ms | `EFFECT_TICK`, 8 ms @40-150 |
| `Rebound` | the thumb snaps back | designed | THUD 0.9, LOW_TICK 0.35 @30 ms (a firm thump that decays at once) | `HEAVY_CLICK`, 22 ms @230, 12 ms @100, 10 ms @40 |
| `Surge` | the highest power chosen | designed | five rising TICKs (0.20, 0.30, 0.42, 0.56, 0.70, gaps 60, 50, 40, 32 ms), CLICK 0.9 @24, THUD 1.0 @8: about 250 ms, a crescendo with a hard crack | without THUD a CLICK 1.0 closes; clicks only: a CLICK crescendo; waveform 28 ms bursts at 40, 70, 110, 160, 210 then 40 ms @255 |
| `Zip` | the lowest power chosen | designed | TICK 0.9, TICK 0.9 @28 ms (very short, sharp double tick) | CLICK 0.5 twice; waveform 6 ms @200, 18 ms gap, 6 ms @200 |
| `Lightning` | fast mode on | designed | TICK 0.45, TICK 0.80 @31, LOW_TICK 0.35 @17, TICK 0.90 @44, CLICK 1.0 @52: uneven strength and spacing, about 180 ms, ends in a firm crack | five CLICKs of the same shape; waveform of irregular 5-8 ms bursts ending in 24 ms @255 |
| `RailTick` | landing on a provider on the rail | `SEGMENT_FREQUENT_TICK` (`CLOCK_TICK`) | LOW_TICK 0.3 (lighter than `Tick`) | 6 ms @28 |

`EffortStep` and `Stretch` take a level: `Feedback.haptic(h, level)` with `level`
from 0 to 1 (position on the effort scale, distance pulled). `AndroidFeedback`
passes it to `HapticTable.spec(h, level)`; every other haptic ignores it. At its
lightest (level 0) `EffortStep` is already a CLICK 0.70 against the old `Select`
TICK 0.70 and `Tick` LOW_TICK 0.5, and at the top it is a THUD 1.0 under a full
double CLICK: clearly the strongest detent in the set, as asked.

**Strength** (Settings): *Subtle* scales primitives and waveform amplitudes by 0.5,
*Standard* by 1.0 (and prefers the platform constants), *Strong* by 1.5 (clamped
to full). Platform constants have a fixed strength, so Subtle and Strong use the
composition path; hardware with no amplitude control and no primitives cannot
honour Subtle, so it drops only the lightest (scroll-class) haptics rather than
playing them at full strength. On the emulator's vibrator the Strong / Subtle
Confirm click is played at scale 0.97 / 0.32 (`dumpsys vibrator_manager`).

## Sound cues

Files are `res/raw/fx_<name>.wav`. Interface cues are generated
(`scripts/generate-android-sounds.py`, deterministic, mono 16-bit 48 kHz) and
committed. The three session chimes are `fx_chime_done` / `_request` / `_attention`:
the desktop sounds from `crates/ui/assets/sounds` (left untouched) as mono, leading
silence trimmed and mastered loud (see Volume); the in-app cues and the notification
channels both play them. The five promoted cues come from `docs/sound-design/auditions/` (stereo to mono, trimmed,
mastered like the rest). `silence_keepalive.wav` is not a cue (see Latency).
Category = the in-app switch that governs it. "Trigger" lists every place the
cue is wired.

| Cue | Source | ms | Category | Paired haptic | Trigger(s) |
| --- | --- | ---: | --- | --- | --- |
| `Done` | in-app copy of desktop `done.wav` | 513 | Completion | `Success` | a session's turn finished while Zeron is open |
| `Request` | in-app copy of `request.wav` | 593 | Input required | `Attention` | a session is waiting for an answer or approval |
| `Attention` | in-app copy of `attention.wav` | 644 | Errors | `Error` / `Attention` | a session failed; connection lost while a turn runs |
| `Send` | desktop audition 01 | 300 | Interface | `Confirm` | composer send / steer, question answer, queued message "Send now" |
| `Queued` | desktop audition 02 | 480 | Interface | `Confirm` | composer send while a turn is running (queue) |
| `UploadReady` | desktop audition 03 | 740 | Interface | `Success` | Save to Downloads done; agent install / update done; project cloned or created |
| `Reconnected` | desktop audition 09 | 760 | Errors | `Confirm` | connection restored after a loss that was announced |
| `Undo` | desktop audition 10 | 490 | Interface | `Select` | Undo on the "Archived" snackbar |
| `Tap` | generated | 34 | Interface | `Select` | default tap of any plain control; links, tool disclosures, file badges, tree rows |
| `Select` | generated | 58 | Interface | `Select` | tabs, filter pills, theme / strength choice, menu choices, mentions picked, photos attached, file saved, jump to latest, rename confirm, paste in terminal |
| `ToggleOn` / `ToggleOff` | generated | 72 / 66 | Interface | `ToggleOn` / `ToggleOff` | every switch, question option chips, Settings toggles |
| `Open` | generated | 108 | Interface | none (the tap's) | a page pushed, a sheet / dialog / menu / popover opens, a section expands, a terminal tab opens, the demo or the developer door opens |
| `Close` | generated | 100 | Interface | none | back, a sheet / dialog / menu closes (quiet after a chosen item), a section collapses, stop a run, close a terminal tab, remove a staged photo, leave the demo / sign out |
| `Detent` | generated, pitch-shifted | 38 | Interface | `Tick` (effort: `EffortStep`, firmer per level) | effort levels (climbs the ladder per level), the volume slider (ten steps) |
| `Star` / `Unstar` | generated | 108 / 92 | Interface | `Pop` | favorites (the model picker); `Unstar` also unpins a session |
| `Pin` | generated | 84 | Interface | `Pop` | pin a session (list menu, session menu) |
| `Archive` | generated | 112 | Interface | `Confirm` | archive by swipe, menu, session menu or search |
| `Delete` | generated | 124 | Interface | `Heavy` (`Confirm` for light removals) | uninstall confirm, discard edits, sign out of an agent, remove a queued message, remove the wallpaper |
| `Copy` | generated | 76 | Interface | `Confirm` | every copy action: paths, links, transcript, code blocks, terminal selection |
| `Error` | generated | 176 | Interface | `Error` | a refusal or failure: save failed or conflicted, send failed, install / update / save-to-Downloads failed, terminal could not open, toasts |
| `Refresh` | generated | 118 | Interface | `Select` / `Confirm` | pull to refresh, reload, retry, refresh files |
| `Surge` | generated | 490 | Interface | `Surge` | the highest thinking power chosen. A rising swell G4 to C6 with a detuned second voice for shimmer, a pentatonic run of sparkles climbing with it (E5 G5 A5 C6 D6 E6) and a bright C-major bloom (C6 E6 G6 C7) at the top. Quiet start, loudest at about 350 ms |
| `Zip` | generated | 85 | Interface | `Zip` | the lowest thinking power chosen. A fast airy streak of band-passed noise falling from 7.5 to 1.8 kHz with a thin pure zing riding it, G6 to C6. Falling = light |
| `Rebound` | generated | 121 | Interface | `Rebound` | the effort thumb snapped back. A soft elastic boing: G4 whose pitch overshoots 55% and wobbles at 26 Hz while it decays, with a low pulse under the hit |
| `FastOn` | generated | 195 | Interface (trim 0.7) | `Lightning` | fast mode on. Eight tiny signed pulses at uneven times (the crackle, centroid above 2 kHz), a bright G6 to G7 zap with a C6 to C7 shadow, and a small C7 + G6 bloom. Quiet by design |
| `FastOff` | generated | 100 | Interface (trim 0.8) | `ToggleOff` | fast mode off. A soft tick, then the charge draining: G6 falling to C5 over 80 ms, fast decay |
| `ProviderClaude` | generated | 115 | Interface | `RailTick` | warm two-note rising pair, E5 then A5, rounded harmonics, soft attack |
| `ProviderCodex` | generated | 83 | Interface | `RailTick` | crisp bracket-like double tick: two identical hollow clicks (odd harmonics) on D6, 41 ms apart |
| `ProviderCursor` | generated | 108 | Interface | `RailTick` | one glassy blip, G6 bending up to A6, inharmonic partials (2.76, 5.4) |
| `ProviderDevin` | generated | 116 | Interface | `RailTick` | soft pad-like minor third, A4 + C5 over an A3 undertone, slow 22 ms bloom |
| `ProviderGrok` | generated | 116 | Interface | `RailTick` | bright quick fifth, C5 then G5, rich upper harmonics, then three sparkles (A6, C7, E7) |
| `ProviderHermes` | generated | 94 | Interface | `RailTick` | fast flutter up: seven steps of the scale C5 to D6, 9 ms apart, with a breath of air |
| `ProviderPi` | generated | 110 | Interface | `RailTick` | three-note tiny arpeggio on the digits 3-1-4 of pi: E5, C5, G5 (third, root, fourth degree) |
| `ProviderOpenCode` | generated | 117 | Interface | `RailTick` | open, hollow tone: an open fifth D5 + A5 in odd harmonics only, like a wooden pipe |
| `ProviderAntigravity` | generated | 116 | Interface | `RailTick` | floaty upward glide C5 to C6 with a slow wobble, and a quieter echo (G5 to G6) 32 ms later |
| `ProviderFavorites` | generated | 116 | Interface | `RailTick` | twinkle: four bell tones (C7, G6, C7, E7) falling in loudness, inharmonic overtones |
| `ProviderOther` | generated | 59 | Interface | `RailTick` | neutral soft pop: a broad rounded pulse with a short low A4 body, no pitch story |

The provider cues are told apart by structure, not just by pitch: the audit builds
a fingerprint of each (12 spectral bands, a 24-slice loudness envelope, an 8-slice
pitch contour) and asserts that no two are closer than a threshold
(`android-audit.md`, "Provider cues are told apart"). `Cue.forProvider(harness)`
maps a harness id to its motif; anything unknown plays `ProviderOther`.

Detent gets `Haptic.Tick` (on the effort slider `Haptic.EffortStep` with `level = index / (n - 1)`);
pull and swipe thresholds have haptic only (`Threshold`).

Model picker (compact card): the effort bar adds `Stretch` (once on entering the
rubber band, once at the wall), `Rebound` + `Cue.Rebound` on release, and, once the
user has settled on an end (180 ms dwell, or right after release), `Surge` +
`Cue.Surge` at the top and `Zip` + `Cue.Zip` at the bottom. The fast button plays
`Lightning` + `FastOn` when it turns on and `FastOff` + `Tick` when it turns off.
The provider rail answers a tap with `Select` + `Cue.forProvider(harness)`, moving
the finger onto the next provider (or scrolling the list across a section) with
`RailTick` (plus the provider's cue when scrubbing). Arrival and lightning effects
are skipped, not the feedback, when animations are off.

Fast mode's lightning **strikes once**, when fast is switched on: one bolt with forks,
a bright flash, two restrikes and a bloom that fades out over 0.9 s
(`LightningFx.LIFE_SECONDS`), starting on the same frame the `Lightning` haptic and
`FastOn` cue fire. After the fade nothing is drawn or animated (no redraw loop, no
timer); switching fast off and on strikes again, and opening the picker with fast
already on shows nothing. With reduced motion it is one still frame at the flash's
peak for 450 ms. It sits at the bottom of the card and never takes touches.

The effort bar follows the finger exactly: while a finger is down the thumb is under
it (`EffortDrag.follow`), with no stickiness, no speed limit and no inertia. Past the
first or last level it rubber-bands (`EffortFx.damp`). On release the thumb springs to
the nearest level (`SETTLE_*`; `REBOUND_*` after a stretch) and there is no fling. A tap
on the track jumps straight to its level. The thumb is the single state the fill, dots
and halo derive from. `EffortStep` fires when the thumb crosses to the next level
(hysteresis in `EffortScale.snap`), so jitter at a boundary does not re-fire. (A
magnetic-well / speed-limited variant was tried and dropped: it felt sticky.)

Stretch past an end (frames: `docs/media/android/picker-stretch-*-before.png` and
`-after.png`, recorded at animator scale 10): the old bar drew a flat bulge past the
rail and a rectangular, rail-clipped fill whose end was cut at the un-stretched
position, so it showed a lighter coloured block past the thumb and then, as the rebound
spring swung inward, a square-ended fill with the grey track end exposed. The fill is
now a capsule whose right end sits between the thumb's centre and its far edge
(`EffortGeometry.fillRight`), derived from the one thumb position, so it stays round
and under the thumb through stretch and rebound.
Not every vocabulary entry is wired by this layer: `Star`, `Pop` for favorites and
the model chip are used by the model picker, `Press` is reserved.

## Volume

The slider is 0 to 100% in ten steps and **starts at 50%**. 100% is twice as loud
(+6 dB) as 50%. The curve, `FeedbackSettings.gainFor(slider)`: `v = 2 * slider`; for
`v <= 1` the gain is `v^2` (25% reads about -12 dB), above that it is `2^(v - 1)`,
equal dB steps up to gain 2.0 at 100%. Both pieces meet at gain 1 with no jump.

`SoundPool` volume cannot exceed 1.0, so the headroom is in the files: the app plays a
cue at `gain * trim / ASSET_BOOST` (`CueTable.volume`, `ASSET_BOOST` = 2): the default
plays the file at half volume, 100% at full volume.

**Round 3: "2x louder again, default stays 50%".** The previous build's files sat at
active RMS -38 dBFS with peaks only -24 to -30 dBFS (the chimes and promoted cues
-34 to -37 dBFS, peaks -13), so there was real room below full scale. Every `fx_*`
file is now mastered by `scripts/generate-android-sounds.py`:

1. a gentle +3.5 dB peaking boost at 2.6 kHz (Q 0.8), where phone speakers are efficient
   and hearing is most sensitive (not on `Detent`, which SoundPool transposes and which
   must stay a flat 880 Hz sine, nor `Zip` and `Surge`, whose falling / climbing
   direction the boost would blur);
2. a soft-knee tanh limiter (knee -4 dBFS, ceiling **-0.5 dBFS**): peaks are rounded
   rather than clipped, and only the chimes and promoted cues (crest factor 22 to 25 dB)
   reach it;
3. the gain is solved with the limiter in the loop until the active RMS hits the target:
   interface cues -31.98 dBFS (previous -38 + 0 lift + 6.02 slider headroom), the
   promoted cues and chimes their desktop level + 12.02 dB.

Result (`android-audit.md`, "Loudness against the previous build"; all numbers include
the SoundPool volume factor, trims and the cap at 1.0): at the **default 50% every cue
plays as loud as the previous build's 100%** (plain and A-weighted active RMS, within
the audit's 0.5 dB), and the new 100% is another +6.02 dB. Seen from the previous
*default* (what the user had been hearing) that is +6 dB at 50% (twice the amplitude)
and +12 dB at 100%. The audit asserts that no cue is quieter than that for every
cue, volume at most 1.0 at every slider position, no clipping, and a stored baseline of
the previous build that it re-checks against git (`f1d4eff1`) when the history is there.
The mastering changes nothing about timing: onset is still within 1 ms in every file
(the trim threshold sits 0.5 dB above the audit's so 16-bit rounding cannot push it).

Notification channels play the same mastered files (`fx_chime_done`, `_request`,
`_attention`, formerly the untouched desktop WAVs through a Gradle copy, which is gone),
+12 dB at file level; `Notifier.CHANNEL_VERSION` is 2 so existing channels, whose
sounds are immutable, are recreated. The system's notification volume still applies.

**What is not possible.** Digital full scale is the limit: the interface cues still
have 10 to 18 dB of peak room (mastered by loudness, not by peak, as asked: the set is
not squashed against the ceiling), the chimes have none. `SoundPool` volume is capped
at 1.0 and cannot be amplified. `LoudnessEnhancer` cannot be attached to a
`SoundPool`'s internal session. (Playing as `USAGE_GAME` does follow the media volume,
often set higher than the system-sound volume; see "Screen recording".)

**Migration.** `FeedbackSettingsCodec.migrate` runs once, keyed on `feedback.prefs_version`
(now 3), and **resets the stored volume to the new default 50%** (no attempt to preserve
the old loudness: the user asked for louder) and silently deletes the retired
`feedback.ignore_touch_sounds` key. Every other choice is kept. Unit tests cover the
reset from versions 1 and 2, idempotence, malformed values and the dropped key.

## Latency

Reports: sounds are "slightly delayed". The path from finger to ear, and what each
part costs, was traced through `AndroidFeedback`, `SoundBank` and `FeedbackGate`
(the emulator has no audio output, so the costs below are from the platform's
documented behaviour and the code, not a timed capture; the log and the unit tests
prove the changes, only a phone proves the milliseconds).

| Stage | Before | Now |
| --- | --- | --- |
| Default tap | `defaultTap` waits `ClaimTracker.DEFER_MS` = 40 ms after release (so an explicit cue can claim the moment), after the release interaction's own coroutine hop | unchanged on purpose (the wait is what keeps a Tap from stacking on an Open); it is the largest remaining software term, about 2.4 frames at 60 Hz. Explicit cues (`feedbackClickable`, `toggleAction`, `both`) already fire inside the click handler, synchronously |
| Order | haptic first, then sound: the vibrator service is a binder call (about 1 to 3 ms, sometimes tens when the service is busy), and a late sound is the one a person hears as late | sound first everywhere (`both`, `play`, `defaultTap`, `preview`) |
| Gate | per event: `PowerManager.isInteractive`, `AudioManager.getRingerMode` and `getStreamVolume`, `NotificationManager.currentInterruptionFilter`, two `Settings.System` reads, `hasVibrator`: five binder calls and two settings reads on the UI thread | the phone's sound state is no longer read at all (see "When it plays"); what is left, `isInteractive`, is cached for 2 s (`TtlValue`) and dropped at once by the screen on / off broadcasts, registered only while foregrounded. `hasVibrator` is read once |
| Output standby | after a few seconds with no stream the audio output goes to standby, and waking it takes tens of ms on many phones (more over Bluetooth): the first sound after a pause came late | a silent looped stream (`silence_keepalive.wav`) keeps the mixer running; started on a finger-down anywhere (`MainActivity.dispatchTouchEvent` to `AndroidFeedback.onTouchDown`, so it is up before the click that sounds), stopped 12 s after the last touch or when the app leaves the foreground (`WarmPolicy`) |
| First play per stream | `SoundPool` creates a stream's `AudioTrack` at its first `play`: a few ms the first four cues paid | after the last sample loads, four silent (-60 dB) plays of the shortest cue create all four tracks (`SoundBank.prime`). The pool has one stream more than the gate allows, for the keep-alive loop |
| Sample rate | assets are 48 kHz; a mixer at another rate must resample every cue and loses the fast path | `AudioManager.PROPERTY_OUTPUT_SAMPLE_RATE` is read at start (logged: `outputRate=`); at 48 kHz (every current phone) nothing changes; otherwise each asset is resampled once to the output rate with a Catmull-Rom interpolator into `cacheDir/sounds-<rate>-<install time>` and loaded from there |
| Attributes | `USAGE_ASSISTANCE_SONIFICATION` only | `USAGE_GAME` (see "Screen recording") plus `FLAG_LOW_LATENCY` (deprecated since 29, still read by the audio policy, which then selects the fast output; `SoundPool` has no performance-mode API) |
| Leading silence in the files | 1.1 to 8.2 ms before the sound reaches -40 dB re peak (Archive 8.2; the promoted desktop cues 7.3 to 7.5; Open 3.3), measured by the audit | every file is trimmed to a 0.25 ms lead-in with a click-free fade from exact zero; the audit and `LatencyTest` assert onset within 1 ms for every `fx_` file. The desktop originals keep their 7 ms (they are no longer shipped: the notification channels use the trimmed `fx_chime_*`) |
| Loading | `onLoadComplete` was tracked; a cue that was not ready yet was dropped (`NotLoaded`) | unchanged: dropping is right (a late sound is worse than none); loading is one background thread at start |

What is **not** fixed, and cannot be in software: the Bluetooth link's own delay,
phone-specific audio HAL buffering (a fast track is typically 10 to 20 ms), and the
`Detent` ladder, which plays at a playback rate other than 1.0 (so the mixer
resamples it; a pre-rendered set per step would avoid that at the cost of five
more files).

## If you hear nothing

Zeron's sounds are played by the app itself as *game sound effects* (`USAGE_GAME`):
they follow the phone's **Media** volume, the way a game's do. With **Sounds & haptics**
on, the app plays them whatever the phone's *Touch sounds*, ringer or Do Not Disturb
settings say; what is left is physical. Check, in this order:

1. Settings, Sounds & haptics: the top **Sounds & haptics** switch, **Sounds** and
   **Interface sounds** are on, and the page is not greyed out.
2. Raise the phone's **Media volume**: at zero the audio stream itself is silent.
3. Zeron's own **Volume** slider is above 0%.

There is no warning on the page and no override: the switches are the single source
of truth. Haptics are the same: the phone's *Touch feedback* setting is not read.

## Screen recording

Reported on a Samsung: starting the screen recorder silenced Zeron's sounds, on the
phone and in the recording, until it stopped. The cues used to be played as
`USAGE_ASSISTANCE_SONIFICATION`, the OS's *system sound* path (the one for keyboard
clicks and touch sounds). That path cannot be captured by `MediaProjection` playback
capture (only media, game and unknown usages can), and One UI's recorder takes it
over while it records. Cues are now `USAGE_GAME` with `ALLOW_CAPTURE_BY_ALL`: they
play on the phone's speaker during a recording and are part of its "Media sounds"
track. This could not be reproduced here (no Samsung, no audio on the emulators); the
change is based on how the audio policy treats the usage. Side effects: the **Media**
volume now governs them (not the system-sound volume), and the ringer mode no longer
mutes them (the master switch is the single source of truth, as before).

## Where each event goes

| Event | In the foreground | In the background |
| --- | --- | --- |
| Turn finished | `Success` + `Done` (Completion switch) | notification, channel sound `fx_chime_done`, vibration 24-40-30 |
| Question / approval | `Attention` + `Request` (Input switch) | notification, `fx_chime_request`, 18-90-18 |
| Session failed | `Error` + `Attention` (Errors switch) | notification, `fx_chime_attention`, 35-55-45 |
| Connection lost mid-turn | `Attention` + `Attention` | nothing (nobody can hear it) |
| A save to Downloads finished | `Success` + `UploadReady` | notification on the completion channel: the app's default sound is the completion chime (`fx_chime_done`), under the Completion and Haptics switches; progress and failures stay silent |
| Connection restored | `Confirm` + `Reconnected`, only if the loss was announced | nothing |

Session events are derived from workspace snapshots (`SessionTransitions`): the
first snapshot is a baseline, subagent rows are skipped, new rows are baselines,
and the same event for the same session within 2 s is dropped. Stopping a run
yourself is not a completion. Returning to the app re-baselines silently: what
happened while away already had its notification. Periodic (clock) refreshes never
announce, so a status that merely aged out is not an event.

**Notification channels.** Channel sounds are immutable once a channel exists, so
session channels are versioned (`session-<kind>-v2-<s|v|sv|q>`); bumping
`Notifier.CHANNEL_VERSION` migrates (deletes) older ones. One channel exists per
kind and per sound / vibration combination the in-app switches ask for, created
lazily, so turning a chime off in Zeron also turns it off in the notification.
Sounds are `android.resource://sh.zeron.android/raw/fx_*`. Notifications need the
`POST_NOTIFICATIONS` permission (API 33+), asked once after the first message you
send and again from Settings. Android freezes or kills a backgrounded process, so
a session finishing long after you leave is only announced if the connection is
still alive; a push service would be the way to make that reliable (not part of
this change).

## Settings

Settings, then **Sounds & haptics**: the **Sounds & haptics** master at the top (off
greys out everything below and silences sounds, haptics and the notification sounds
and vibration; on lets the switches decide), then **Sounds** (every sound),
**Interface sounds**, a ten-step **Volume** (default 50%, 100% is twice as loud, see
Volume), the **Session sounds** master with independent **Completion**, **Input
required** and **Errors** (mirroring the desktop's Settings, Notifications),
**Background alerts** (notification permission), **Haptics** with **Strength**
(Subtle / Standard / Strong) and a **Try them** list that plays the real cue and
haptic together. Previews ignore rate limits and category switches (hearing a muted
category is their point) but obey the master, Sounds and Haptics switches. Groups of
rows that sit directly under each other are 10 dp apart (`GroupGap`; a section title
has its own larger gap); the Session sounds group and the Background alerts card used
to touch. Everything defaults on.

## Debugging

Debug builds log one line per decision to the `ZeronFeedback` tag: what played and
as what (`haptic Select play ViewConstant(constant=26)`, `cue Detent play vol=0.64
rate=1.33 step=5`) or why it did not (`cue Tap skip: another cue just played`,
`haptic Tick skip: rate limited`). `adb shell dumpsys vibrator_manager` lists recent
vibrations with the primitives played. Session events can be driven without an
agent: `adb shell am broadcast -a sh.zeron.android.DEBUG_EVENT -p sh.zeron.android
--es kind done|input|failed [--ez background true]` (`background` takes the
notification branch). Any vocabulary entry can be fired directly: `--es kind haptic
--es name Surge [--ef level 0.75]` or `--es kind cue --es name ProviderClaude`. The
`ready:` line at start reports `outputRate=`, `assetRate=` and whether assets had to
be resampled.

## Regenerating

```sh
python3 scripts/generate-android-sounds.py          # res/raw/fx_*.wav (+ promoted desktop auditions)
python3 scripts/audit-android-sounds.py             # asserts and rewrites android-audit.md
```

Both are standard-library only and deterministic (bit-identical output). The audit
asserts durations (interface cues at most about 120 ms, event cues at most about
600 ms plus the promoted tails), peaks and RMS bands, no clipping, zero first and
last samples with zero-slope fades, DC offset, rising Open and falling Close,
rising ToggleOn and falling ToggleOff, a rising Surge and falling Zip / FastOff,
onset within 1 ms of the file start for every `fx_` file, that the in-app chimes are
the desktop chimes plus 18.04 dB (previous headroom, slider headroom, the lift), that the provider cues are
structurally distinct, and that interface cues stay 2 to 4 dB under the chimes. The full table is in [android-audit.md](android-audit.md).

## Verification and its limits

JVM tests (`apps/android/app/src/test/java/sh/zeron/android/feedback`) cover the
gate (the absolute master, the switches, that the environment has no system-state
property at all, throttling, stream cap, reads per decision), the haptic planner (a plan for every `Haptic` at
several levels on several simulated devices, strength scaling, the round-2 designs),
the volume curve and its one-time reset, the latency pieces (`LatencyTest`: the TTL cache,
output-rate handling, the resampler, the keep-warm policy, every shipped file starting
within 1 ms), the cue table (every `Cue` has its own file), the settings codec and the
event policy with a recording fake (a finished session fires Done once; a
background event becomes a notification and nothing in-app). On the emulator the
log, `dumpsys vibrator_manager` and `dumpsys notification` prove what fires and
how. **The feel of the haptics and the sound quality of the cues need a human on a
phone**: the emulator has no vibration motor you can feel and no audio output.
