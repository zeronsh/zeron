# Browser on Linux

The desktop app (`apps/zgx`) renders browser tabs through `zgx-desktop`'s
`webkitgtk` feature (`ui.webview`), loading system **WebKitGTK 4.1**
(`libwebkit2gtk-4.1`) at runtime. The binary does not link WebKitGTK directly,
so headless servers and desktops without WebKitGTK still run normally — browser
tabs show a placeholder when the runtime is absent, and links can still be
opened externally.

At startup, `apps/zgx/src/main.rs` defaults `WEBKIT_DMABUF_RENDERER_FORCE_SHM=1`
so offscreen GTK 3 web views return frames through shared memory without
requiring a GDK GL context.

## Runtime dependencies

The Linux installer does not install or validate system WebKitGTK packages;
install the distribution's WebKitGTK 4.1 runtime to enable embedded browser tabs:

- **Ubuntu / Debian:**
  ```sh
  sudo apt install libwebkit2gtk-4.1-0
  ```
- **Fedora:**
  ```sh
  sudo dnf install webkit2gtk4.1
  ```

WebKit itself stays system-managed and receives security updates through the
distribution.

## Building from source

Development builds on Linux install `libwebkit2gtk-4.1-dev` and
`libjson-glib-dev` alongside the other desktop build libraries listed in
[`CONTRIBUTORS.md`](../../CONTRIBUTORS.md).

## Links

HTTP(S) links in chat open new Browser tabs in their conversation's companion
panel. If the WebKitGTK runtime is unavailable, use **Open in external browser**
from the link's context menu or the browser toolbar.
