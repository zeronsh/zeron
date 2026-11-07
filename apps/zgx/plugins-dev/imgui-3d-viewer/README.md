# imgui-3d-viewer

A real 3D viewer: twenty-seven cubes in a lattice, drawn by the GPU with a
depth buffer, lit per face, under a title bar the plugin draws itself
(`titlebar = "hidden"`, no OS chrome).

| Window id | Title | Size |
| --- | --- | --- |
| `viewer` | `3D Viewer` | 900 × 620 |

What it shows:

- `imgui.mesh` builds one cube — 24 vertices, 36 indices, outward normals —
  and `ui.scene3d` places 27 of them in a 3×3×3 lattice. The host draws them
  in a depth-tested pass, so cubes occlude each other correctly and each face
  is lit by the key and fill lights.
- every cube turns on its own and the lattice orbits, so it reads as a turning
  cloud; the cubes' tints come from the window's own `$accent`,
  `$accent.hover` and `$text.primary`, so a chosen accent recolours it.
- the title bar is the plugin's: a drag strip (`invisible_button` +
  `begin_drag`, double click to maximise), and `_`, `[]`, `x` buttons for
  minimise, maximise and close, all through the window handle.
- drag anywhere in the scene to orbit (the spin stops when you do), click to
  start it again, `Reset` for the starting angle, and the `distance` slider
  backs the camera off from 3.5 to 16.
- the footer reads the cube count, yaw, pitch and the frame number.

`viewer_status` answers yaw, pitch, distance, spin, the cube count and the
frame count as JSON.

Run it:

```
ZERON_ZGX_DEV_PLUGINS=apps/zgx/plugins-dev/imgui-3d-viewer   apps/zgx/target/debug/zeron-zgx.exe gallery
```
