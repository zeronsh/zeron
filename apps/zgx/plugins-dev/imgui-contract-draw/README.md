# imgui-contract-draw

The validation contract's drawing fixture: one 720 × 480 window that draws one
row's geometry at a time, plus a 420 × 1000 `Probe Actions` window whose
buttons select the screen. Its manifest asks for `imgui.windows`.

| Window id | Title | Size |
| --- | --- | --- |
| `draw` | `Contract Drawing` | 720 × 480 |
| `actions` | `Probe Actions` | 420 × 1000 |

Screens, and the rows each serves:

| Screen | Draws | Rows |
| --- | --- | --- |
| `lines` | two lines, one 3 points thick | VAL-DRAW-001 |
| `rect` | an outlined rectangle | VAL-DRAW-002 |
| `rect_filled` | a filled rectangle | VAL-DRAW-003 |
| `rounding` | a rounded rectangle and, 120 points lower, the square one | VAL-DRAW-004 |
| `rounded_filled` | a filled rounded rectangle | VAL-DRAW-005 |
| `circle` | a circle outline | VAL-DRAW-006 |
| `circle_filled` | a filled circle | VAL-DRAW-007 |
| `text` | the same string twice, 80 points apart | VAL-DRAW-008 |
| `image` | `add_image` of `assets/checker.png` at 40,40–104,104 | VAL-DRAW-009 |
| `polygon` | a filled triangle and a closed pentagon outline | VAL-DRAW-010 |
| `clip` | a full-window fill inside a 100,100–200,180 clip | VAL-DRAW-011 |
| `alpha` | opaque blue, then `#ff000080` over it | VAL-DRAW-012 |
| `kept` | a plugin-held list, cleared and refilled each frame, a circle after its 31st | VAL-DRAW-013 |
| `refused` | three refused `add_image` handles, each `pcall` logged, then a green square | VAL-DRAW-014 |
| `off_frame` | nothing; the timer and `probe_run` log the off-frame attempts | VAL-DRAW-015 |
| `handles` | prints `type`/`tostring` for the window, a texture, a draw list and a `color_edit3` result | VAL-DRAW-016 |
| `image_widget` | `imgui.texture_size` logged, then `ui.image{ width = 64, height = 64 }` and a 4-point gap | VAL-ADV-015 |

`assets/checker.png` is a 16 × 16 picture: red top left and bottom right,
blue top right and bottom left, 8 pixels each.

Drive it:

- click a button in the `Probe Actions` window, or
- `probe_run { action = "clip" }` on a `tools` grant, or `probe_run { action = "off_frame" }`
  for the two off-frame attempts, or
- `probe_status` for the current screen, its frame count and the last refusal.

The off-frame attempts also run from a 5-second timer, which logs
`off-frame-ok timer` when both refusals behaved.

Run it (the gallery screen needs no engine and no sign-in):

```
ZERON_ZGX_DEV_PLUGINS=apps/zgx/plugins-dev/imgui-contract-draw \
ZERON_ZGX_PLUGIN_LOG=1 apps/zgx/target/debug/zeron-zgx.exe gallery
```

(`;` separates folders on Windows, `:` elsewhere.)
