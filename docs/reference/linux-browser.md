# Browser on Linux

Zeron renders the sidebar browser with WPE WebKit when the build includes its helper and the system has its runtime, and with WebKitGTK otherwise. Either engine runs in a helper process that Zeron starts when a page is first opened. The main application links to neither, so other app features remain available if the browser runtime is missing. Both use a persistent website-data context shared by tabs and windows in the same Zeron profile. Persistent cookies, localStorage and IndexedDB survive closing tabs and restarting Zeron. See [browser profiles and login persistence](browser-profiles.md) for storage locations, profile isolation, and regression tests.

GPUI draws the helper's frames alongside the rest of the app rather than embedding a separate native browser window. Both X11 and Wayland use this path, including clipping, sidebar transitions, tooltips, and frosted overlays.

## WPE WebKit

The WPE helper is Zeron's own WPE platform: WebKit renders pages on the GPU GPUI uses, and GPUI shows its frames at the display's refresh rate without copying them through the CPU.

- GPUI's Vulkan device reports the buffer layouts (DRM format modifiers) it can import. WebKit renders into one of those that EGL can also render, and GPUI copies only each frame's damaged regions from the shared dma-buf into its atlas, on the GPU.
- Where no layout suits both, as when the driver can render none that Vulkan imports, the helper reads frames back with OpenGL ES into shared memory. Without a usable GPU, WebKit renders on the CPU into shared memory.
- GPUI's frame callbacks are the pages' vblank. Like a compositor, GPUI tells WebKit a frame is on screen at most once per display refresh, so pages animate at the display's rate, 60 Hz or 240 Hz, and never render frames the display would not show. A hidden page, or one Zeron replaces with its error, gets no vblanks at all.

If the WPE helper cannot start, for instance because its runtime is missing, Zeron uses WebKitGTK for the rest of the session.

The WPE helper needs WPE WebKit 2.50 or later with the WPE Platform API, JSON-GLib, GBM, EGL, OpenGL ES 2 and xkbcommon. On Arch Linux:

```sh
sudo pacman -S wpewebkit json-glib mesa libglvnd libxkbcommon
```

A build embeds the WPE helper when pkg-config finds `wpe-webkit-2.0 >= 2.50`, `wpe-platform-2.0`, `json-glib-1.0`, `gbm`, `egl`, `glesv2` and `xkbcommon`. Otherwise it prints a warning and the browser uses WebKitGTK. The release workflows install only the WebKitGTK packages, so release builds currently use WebKitGTK.

## WebKitGTK

WebKitGTK requires the distribution's WebKitGTK 4.1 and JSON-GLib runtime packages, including when using a prebuilt Zeron release. The Linux installer does not currently install or validate these dependencies; install them separately before opening a browser tab.

On Ubuntu or Debian:

```sh
sudo apt install libwebkit2gtk-4.1-0 libjson-glib-1.0-0
```

On Fedora:

```sh
sudo dnf install webkit2gtk4.1 json-glib
```

The WebKitGTK helper sends CPU-addressable frames, so animated pages incur frame-copy and texture-upload work.

For development, install the development packages in addition to the normal GPUI build dependencies. These provide the `webkit2gtk-4.1` and `json-glib-1.0` pkg-config modules used to compile the helper.

On Ubuntu or Debian:

```sh
sudo apt install libwebkit2gtk-4.1-dev libjson-glib-dev
```

On Fedora:

```sh
sudo dnf install webkit2gtk4.1-devel json-glib-devel
```

The build embeds the small helper executables, which are extracted to the user's cache directory when needed. WebKit itself stays system-managed and receives security updates through the distribution.

HTTP(S) links in chat open new Browser tabs in their conversation. If the runtime cannot start, the tab shows the browser error; use **Open in external browser** from the transcript link's context menu or the Browser toolbar's external-open button. See [transcript link interactions and fixtures](../transcript-browser-links.md).

## Environment variables

| Variable | Effect |
| --- | --- |
| `ZERON_BROWSER_ENGINE=wpe` or `gtk` | Use that engine, without falling back to the other. |
| `ZERON_BROWSER_DMABUF=0` | WPE WebKit renders on the CPU into shared memory. |
| `ZERON_BROWSER_DMABUF=readback` | WPE WebKit renders on the GPU, and the helper reads its frames back into shared memory. |
| `ZERON_WPE=0` | At build time, leave the WPE helper out. |
| `ZERON_WPE_HELPER=<path>` | Debug builds run this WPE helper instead of the embedded one. |

## Tests and benchmarks

The end-to-end tests run the real WPE helper, so they need its runtime:

```sh
cargo test -p zeron-ui --lib browser::linux -- --include-ignored --test-threads=1
```

They render pages on every render node in every layout its driver renders, read back and in shared memory, and check that pages follow vblanks at 60, 144 and 240 Hz however often GPUI paints. `ZERON_TEST_RENDER_NODE=/dev/dri/renderD128` picks the GPU for the tests that take one; without it they use the first render node that renders, or shared memory when none does.

The browser fixture drives the whole app on a desktop session. It needs an unlocked session with keyboard focus for its clipboard checks, or `ZERON_BROWSER_FIXTURE_UNFOCUSED=1` to skip them. On Wayland, `ZERON_BROWSER_CAPTURE_COMMAND` names a program that captures the fixture's window into the PNG path it is given:

```sh
cargo build -p zeron-ui --example browser-fixture --features browser-fixture
ZERON_BROWSER_ENGINE=wpe ZERON_BROWSER_CAPTURE_COMMAND=./capture.sh \
  target/debug/examples/browser-fixture /tmp/browser-fixture
```

It checks input, clipboard, menus, resizes and captures, and fails if an animating page falls behind the display (WebKitGTK: behind 60 Hz), stops getting vblanks once shown again, or WebKit keeps more than six buffers for one page. On Wayland, GPUI's frames, and so the page's vblanks, wait for the compositor's frame callbacks; the nested weston that `scripts/test-linux-browser.sh` runs answers none for a commit without damage, so its WPE run fails the vblank check, while cage (wlroots) passes it. `ZERON_BROWSER_BENCH=<seconds>` instead measures pages that repaint their whole viewport every frame (a 2D canvas, WebGL 2, scrolling) or a small part of it (a CSS spinner), a 60 fps video when `ZERON_BROWSER_BENCH_VIDEO` names an MP4, how long a press takes to reach a painted frame, and an idle page. It writes to `bench.json` the page's frame rate, when GPUI painted each frame, the bytes copied to show them, and the CPU, memory and NVIDIA GPU memory Zeron and WebKit use. `ZERON_BROWSER_BENCH_WIDTH` sets the browser's width in logical pixels.
