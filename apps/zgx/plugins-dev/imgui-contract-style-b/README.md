# imgui-contract-style-b

The second styling fixture: a 720 × 480 window that sets no style at all, so
it must wear Zeron's own look whatever `imgui-contract-style` sets in the same
session (VAL-STYLE-014). Its manifest asks for `imgui.windows`.

| Window id | Title | Size |
| --- | --- | --- |
| `style-b` | `Contract Style B` | 720 × 480 |
| `actions` | `Probe Actions` | 420 × 1000 |

It draws `Zeron 123`, a `Save` button and nothing else, and logs, per frame:

```
style-b frame=<n> theme=<dark|light> background=#… text=#… frame_rounding=… padding=…
```

`probe_status` answers the same look as JSON. Nothing here can be driven: the
row is that no style crosses into this window, so there is nothing to set.

Run it beside the styling fixture (see that fixture's README).
