# imgui-contract-style

The validation contract's styling fixture: one 720 × 480 window that wears one
screen's style at a time and prints `imgui.style()` for the frame it drew,
plus a `Probe Actions` window that selects the screen. Its manifest asks for
`imgui.windows`.

| Window id | Title | Size |
| --- | --- | --- |
| `style` | `Contract Style` | 720 × 480 |
| `actions` | `Probe Actions` | 420 × 1000 |

Every screen draws the same content after its style: `Zeron 123` text, a
separator, two selectable rows (one selected), a button, a checkbox, a 50%
progress bar, and a bordered child region. The log says, per frame:

```
style frame=<n> theme=<dark|light> dpi=<n> frame_rounding=… window_rounding=… padding=<x>x<y> spacing=<x>x<y> border=… text=… font=<path|nil>
tokens $background=#… $text.primary=#… … $focus.ring=#…
```

(the 15 tokens of the contract, in its order).

| Screen | Style it wears | Rows |
| --- | --- | --- |
| `defaults` | none: Zeron's own | VAL-STYLE-001, VAL-STYLE-002 |
| `light` | `theme = "light"` | VAL-STYLE-004 |
| `colours` | `$background #101820`, `$text.primary #ffcc00`, `$border #3355ff`, `$selection #552288` | VAL-STYLE-006 |
| `rounding` | `frame_rounding = 8` | VAL-STYLE-007 |
| `window_rounding` | `window_rounding = 14` | VAL-STYLE-008 |
| `padding` | `window_padding = 24 × 24` | VAL-STYLE-009 |
| `spacing` | `item_spacing = 20 × 14` | VAL-STYLE-010 |
| `border` | `border_size = 3` | VAL-STYLE-011 |
| `text_size` | `text_size = 22` | VAL-STYLE-012 |
| `font` | its own `assets/InstrumentSerif-Regular.ttf` at 20 points | VAL-STYLE-013 |
| `reset` | `reset_style()`, back to Zeron's | VAL-STYLE-006's unset check |

`assets/InstrumentSerif-Regular.ttf` is shipped under the contract's name; the
file is Geist Medium, because a font that looks nothing like the default face
is what the row measures.

Drive it with the buttons in `Probe Actions`, with
`probe_run { action = "padding" }`, or read the look with `probe_status`
(theme, tokens, metrics, font). Run it beside `imgui-contract-style-b` for
VAL-STYLE-014:

```
ZERON_ZGX_DEV_PLUGINS="apps/zgx/plugins-dev/imgui-contract-style;apps/zgx/plugins-dev/imgui-contract-style-b" \
ZERON_ZGX_PLUGIN_LOG=1 apps/zgx/target/debug/zeron-zgx.exe gallery
```
