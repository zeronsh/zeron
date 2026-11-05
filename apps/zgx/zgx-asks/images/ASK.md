# Ask: pictures in a lightbox (GIFs, pixelated, copying a file, a larger draw while it decodes)

**Who needs it:** Zeron's image lightbox (`apps/zgx/ui/ds/lightbox.luau`). A sent message's pictures, a reply's, and the composer's chips open in it. It grows out of the thumbnail, zooms to 1:1 and past it, copies the picture, and pages through a message's set.

All at zgx `b3371f8b`. `zgx test apps/zgx/zgx-asks/images` runs the repro in `tests/repro.test.luau`.

## 1. A GIF in `ui.image` is a runtime error

```text
app.luau: runtime error: app.luau:15: zgx-asks/images/blink.gif: The image format Gif is not supported
```

This happens with zgx-gpu's `gif` feature on (the CLI's default build, and Zeron's). A `src` file is loaded as a kept file (`decode_image_kept`), and that path reads it through the `image` crate. zgx-gpu depends on `image` with `default-features = false` and turns on only `image/png`; `gif` is `dep:gif`. So `image` can't read a GIF's header, and the script errors. In Zeron that's any `.gif` someone attaches.

**Asks:**
- (a) GIFs load through `ui.image` whenever the `gif` feature is on.
- (b) They animate there, as `animated_image` does in Rust: pause off-screen, keep the last frame when finite. Today nothing in zgx-script reaches `compose_animation`.
- (c) A picture that fails to decode stays an error the script can see (a `fallback`, an `on_error`) rather than a script error that blocks the window's reloads.

## 2. `image_rendering = "pixelated"` draws smooth

`app.luau` draws a 4 × 4 checker PNG at 64 × 64 with `image_rendering = "pixelated"`. It should be 16 hard squares.

With `zgx run --inspect` and `zgx inspect snapshot '#checker'`, each square's edge ramps over several pixels instead. In Zeron's lightbox, a 1 px checker at 300 % reads 46 → 202 → 46 in smooth steps. The same happens with no transform, clip or layer above the image, and with a literal or a function value.

zgx-gpu's `image_source` takes the nearest sampler only when `nearest_bind(image)` finds the image's own texture. Our guess is that file images don't have one there (an atlas, or a kept file's upload), so the draw silently falls back to linear.

**Ask:** pixelated samples nearest for every `ui.image`, kept files included. The lightbox asks for it from 100 % at whole multiples and from 300 % at any scale, so 1:1 on a Retina screen is crisp, as Preview and Figma show it.

## 3. Copying a picture from its file

`ui.clipboard.write_image` takes a `Bitmap`, and a script can't make one from a file. So Zeron's host added an `image.copy { path }` action (`src/images.rs`): `zgx_gpu::assets::decode_image` off the main thread, then `PlatformService::set_clipboard_image`.

**Ask:** either of these, in order of preference:
- `ui.clipboard.write_image(src: string)`: a path, or any `ui.image` `src`, decoded off the main thread, settling with whether the clipboard took it.
- `ui.load_bitmap(src: string) -> Promise<Bitmap>`, the same decode, for a script to hand to `write_image` (or crop or recolour first).

## 4. Drawn larger while its larger pixels decode, it isn't drawn at all

A kept file drawn larger than its texture asks for its pixels at the new size. While they decode, `upload_image_at` returns `IMAGE_PENDING`, and the image isn't drawn that frame (lib.rs ~10790), even though a smaller texture of it is there.

The lightbox draws the thumbnail's picture larger on its first frame, so it went blank for a frame or more until the decode landed. We now draw a `el:snapshot()` of the thumbnail under it as a stand-in.

Still open on zgx `0e6c32f` (`zgx-gpu` `ensure_image`: a texture smaller than the drawn size returns `IMAGE_PENDING` and the image is skipped). It also made zooming flicker: each frame of a zoom in draws the picture larger, so it blinked out frame after frame until its pixels decoded (Nov 2, user report). Zeron now holds an `el:snapshot()` of the stage, as a zoom begins, under the picture, scaled with the zoom (ds/lightbox `hold`; tests/lightbox "zooming never leaves a frame without the picture"). A fix here would let both stand-ins go.

**Ask:** draw the texture it has, stretched (soft), until the larger one is ready, as a browser shows a progressive image. Or tell the script when a `src` has drawn (`on_load`) so it can hold a transition until then.

## 5. Small: a click at a point in tests and in `zgx inspect`

`zgx inspect drag x,y x,y+1` (and the harness's `drag`) adds a 16 pt jitter to cross the drag threshold, so it can't stand in for a click. `click` takes only nodes, and the harness refuses one under a scrim (`is covered at … by …`).

So "a click beside the picture closes the lightbox" can't be tested without moving the person's real pointer.

**Ask:** `click "x,y"` in both, dispatched as a press and release at that point with no movement.

## 6. A missing file in `ui.image` is a script error

```text
sent_pictures_fixture.luau: runtime error: sent_pictures_fixture.luau:42: /host/uploads/shot.png: No such file or directory (os error 2)
```

A picture's file can go at any time: a sent image deleted, a kept copy pruned from the temp dir, a wallpaper's folder moved. Today `ui.image` raises a script error for it. That shows in the window, and in the gallery it stops every window's reloads. It isn't a fault in the script.

Zeron now asks the host first (`image.exists`, a stat, through `ui.call_sync`) and hands `ui.image` an empty `src` for a file that's gone. That's `ds/image_guard`, used by every image built from a path: sent and reply pictures, chips, the lightbox, the file viewer, wallpapers, template previews and avatars. But a check before the load races a deletion between the check and the decode.

**Ask:** a missing or undecodable file draws nothing (as an empty `src` does) and is reported, never raised:
- an `on_error(err)` on `ui.image` (HTML's `onerror`), or
- the `fallback` node used for local files too, not only remote ones.

Either lets a part show its own "couldn't be shown" state without asking the filesystem first.

## 7. A picture's size before it's drawn

A reply's picture holds its place while it loads, at its final size, so nothing jumps when it lands. The only way a script learns a file's size is to lay out a copy at its own size, unseen, and read `on_resize`. That decodes and uploads the whole picture at full resolution just to measure it: a 4000 × 3000 photo costs 48 MB of texture for one number. Zeron's host now reads the header instead (`image.size`, src/images.rs, `image::ImageReader::into_dimensions`), and ds/reply_picture falls back to the copy only where there's no host.

**Ask:** `ui.image_size(src) -> (w, h)?` from the header alone (sync, cheap), or an `on_load({ w, h })` on `ui.image` that fires before the first draw. Either one lets a design system hold a picture's place without a host of its own.

(GIFs, 1 above, matter here too: a reply naming a `.gif` stays a link in Zeron until `ui.image` can show one.)

## 8. With a `fallback`, `fit` is lost: the picture is squeezed into the box

Seen on zgx `0e6c32f`. A wide picture cropped into a small box:

```luau
ui.overlay { w = 56, h = 40, overflow = "hidden",
  ui.image { src = "wide.png", fit = "cover", w_full = true, h_full = true,
    fallback = ui.div {} } }
```

Without `fallback` it's cropped: the picture node is laid out at 160 × 40 (its own 4:1) and the box clips it. With `fallback` it's drawn at 56 × 40, the box's 1.4:1: squeezed. `lower_image` (crates/zgx-script/src/lower.rs) wraps the picture in a `compose::switch` when there's a `fallback`, and the node's styles, `fit` among them, go to the switch rather than to the `picture()` inside, which keeps `ObjectFit::Fill`. In Zeron, every attachment chip in the composer, every dock thumbnail and every avatar photo showed squeezed (Nov 2, user report).

`src/picture_aspect_tests.rs` measures it: the picture node's box against its pixels' shape, within 1%. `aspect-cover-fallback` in `ui/picture_aspect_fixture.luau` is this repro.

**Ask:** a `ui.image` with a `fallback` draws its picture with its own `fit`, `image_rendering` and position, as one without does.

Zeron's stopgap: no `fallback` on a cropped picture. The composer's chip and the dock's thumbnail show their glyph from `on_error`, and an avatar keeps its waves under the face.

**Fixed** in zgx `a2c9cf1e6` (#1417), Zeron's pin since Nov 2: the stopgap is gone, and `src/picture_aspect_tests.rs` checks the cropped picture with a `fallback` too.
