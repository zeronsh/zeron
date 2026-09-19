# Browser on Linux

The sidebar browser requires the distribution's WebKitGTK 4.1 and JSON-GLib runtime packages, including when using a prebuilt Zeron release. The Linux installer does not currently install or validate these dependencies; install them separately before opening a browser tab.

On Ubuntu or Debian:

```sh
sudo apt install libwebkit2gtk-4.1-0 libjson-glib-1.0-0
```

On Fedora:

```sh
sudo dnf install webkit2gtk4.1 json-glib
```

On NixOS, the flake package (`nix profile install github:zeronsh/zeron`) builds the helper against WebKitGTK from nixpkgs and keeps it in the package closure, so nothing needs to be installed separately.

Zeron starts its browser helper when a page is first opened. The main application does not link to GTK or WebKit, so other app features remain available if the browser runtime is missing. WebKit runs in a separate process and uses an ephemeral website-data context shared by the open tabs.

The helper sends live offscreen frames to GPUI, which draws the page alongside the rest of the app. Both X11 and Wayland use this path, including clipping, sidebar transitions, tooltips, and frosted overlays. It uses CPU-addressable frames rather than embedding a separate native browser window. Animated pages therefore incur frame-copy and texture-upload work.

For development, install the development packages in addition to the normal GPUI build dependencies. These provide the `webkit2gtk-4.1` and `json-glib-1.0` pkg-config modules used to compile the helper.

On Ubuntu or Debian:

```sh
sudo apt install libwebkit2gtk-4.1-dev libjson-glib-dev
```

On Fedora:

```sh
sudo dnf install webkit2gtk4.1-devel json-glib-devel
```

The build embeds the small helper executable, which is extracted to the user's cache directory when needed. WebKit itself stays system-managed and receives security updates through the distribution.

HTTP(S) links in chat open new Browser tabs in their conversation. If the runtime cannot start, the tab shows the browser error; use **Open in external browser** from the transcript link's context menu or the Browser toolbar's external-open button. See [transcript link interactions and fixtures](../transcript-browser-links.md).
