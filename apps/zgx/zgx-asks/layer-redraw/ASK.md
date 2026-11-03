# Ask: a cached layer draws again when what's in it changes

**Priority: high.** A new chat's wallpaper under an effect (Scanlines, Dither, ASCII, Halftone) came up blank after its picture changed, until something else made it draw.

**Seen on:** zgx `f6506b6e6` (Zeron's pin), macOS 26.

**Today.** A node with `layer_shader` is cached as a layer (zgx-gpu `prepare_layer`), its raster kept while `scene.layer_revision(id)` is unchanged. The revision moves only as a style inside it changes (`invalidate_with`, `invalidate_placed`). Not when:

- nodes come into it or leave it (a new picture mounted inside): the layer keeps its old raster;
- a picture inside it finishes loading: the layer keeps the raster drawn before it had loaded;
- the layer itself is made anew on a node id just freed: its revision starts at 0 again, and a cached raster of the old node with the same revision, step and factor is taken as its own (the cache is keyed by node id).

A zoom change (a new raster step) draws it right at once, which is how we found it.

**Ask:** bump a layer's revision when its subtree's children change and when an image inside it becomes ready; drop a layer's cache when its node is freed (or key the cache so a reused id can't match).

**Meanwhile (Zeron):** ds/wallpaper builds a layer per effect and art, the art in a plain wrapper that fades in on mount (its opacity frames are style changes inside the layer, so it draws as the art arrives) over the layer leaving; `Picture` changes an invisible style as it loads. `ui/tests/wallpaper.test.luau` pins that shape. (Keeping one layer and crossfading the art inside it with a nested `ui.presence` hid the plugin's place from `ui.change`: [nested-presence-placement](../nested-presence-placement/ASK.md).)

**Repro:** in Zeron's `ui/composer_place_fixture.luau` with ds/wallpaper before Nov 2 (git history), set `test_place_effect` to "scanlines", then `test_place_picture` to "plate-3-sunrise": `zgx inspect snapshot '#place-shot'` shows no art; `zgx inspect zoom 110%` brings it back.
