# imgui-3d-viewer

A rotatable cube in a native window. Dear ImGui draws in two dimensions, so
this plugin projects it itself: eight corners, six faces, a rotation about the
vertical then the horizontal axis, a perspective divide, back-face culling and
a painter's sort. Every face is shaded from the accent the window wears, so
the cube changes colour with Zeron's theme and a chosen accent.

| Window id | Title | Size |
| --- | --- | --- |
| `viewer` | `3D Viewer` | 760 × 560 |

Controls:

- drag anywhere in the viewport to turn the cube (the auto-spin stops as soon
  as you do)
- `Spin` / `Stop` toggles the auto-spin, `Reset` puts the cube back at its
  starting angle, and the `zoom` slider scales it from 0.5× to 2×
- the line under the title reads yaw, pitch, zoom and the frame count, so a
  screenshot shows the state it was taken in

`viewer_status` answers the same numbers as JSON for a scripted check.

Run it:

```
ZERON_ZGX_DEV_PLUGINS=apps/zgx/plugins-dev/imgui-3d-viewer \
  apps/zgx/target/debug/zeron-zgx.exe gallery
```

(`;` separates folders on Windows, `:` elsewhere.)
