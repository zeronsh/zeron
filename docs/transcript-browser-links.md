# Transcript web links

Click an HTTP(S) link in an agent message to open and select a **new Browser tab** in that conversation. The right panel opens if necessary. Links in a subagent transcript belong to its containing conversation. A pending activation is discarded if the selected conversation has changed.

Right-click a link for **Open in Zeron**, **Open in external browser**, or **Copy link address**. The same menu includes **Open links in Zeron**, which controls normal click and Enter/Space activation and is enabled by default. The three explicit actions remain available regardless of that preference. Tab and Shift+Tab move through links and other controls when an input or completion menu does not consume the key. Enter or Space activates a focused link. Shift+F10 opens its action menu; arrows or Tab move through the actions, and Escape dismisses the menu.

Hover or keyboard focus shows the full destination, including when the Markdown label says something different. The compact tooltip fits the destination text up to 360 px, wraps long destinations, and scrolls within the window. Copy link address is available in the context menu. Opening that menu hides the tooltip and cancels pending hover disclosure until the menu closes. Inspecting a link makes no network requests. Scrolling the transcript, changing conversations, or removing a link dismisses its disclosure.

Long web links are shortened with an ellipsis according to the actual available text width, including inside table cells. Navigation, destination disclosure, and Copy link address retain the complete URL. Text selection uses the original label: selecting an entire shortened URL copies the full URL, and selecting its visible prefix copies that prefix. Selecting across the ellipsis includes the omitted text. Labels, styles, and selection positions update when the window width or streaming content changes.

Only explicit HTTP(S) web addresses without embedded credentials, control characters, whitespace, backslashes, or malformed percent escapes can navigate. Rejected links never fall through to the OS opener. Workspace/file links keep their file-opening behavior. Markdown file previews also open web links in a new Browser tab belonging to the file's conversation, with the same explicit external/copy actions. File previews retain their local-file, anchor, and mail-link handling; their labels are not visually truncated.

macOS uses its integrated WebKit browser. Linux uses WebKitGTK and requires the [Linux browser runtime packages](reference/linux-browser.md). If that runtime is unavailable, the Browser tab displays an error; its external-browser button and the link's external action remain explicit alternatives. Platforms without an integrated implementation use the system browser for valid web links.

## Reproducible validation

The fixture uses synthetic chat data, temporary app storage, and a local HTTP server. Its Markdown corpus includes short and long URLs, misleading labels, rejected destinations, Unicode, and tables:

- [Fixture Markdown](../crates/ui/examples/browser-fixture/transcript-links.md)
- [Native scenario](../crates/ui/examples/browser-fixture/transcript_links.rs)

```sh
cargo test -p zeron-ui
cargo build -p zeron
cargo build -p zeron-ui --example browser-fixture --features browser-fixture
ZERON_TRANSCRIPT_LINK_FIXTURE_ONLY=1 \
  BROWSER_FIXTURE_BINARY=target/debug/examples/browser-fixture \
  scripts/test-linux-browser.sh /tmp/zeron-transcript-links
```

If FFmpeg lacks `libx264`, add `BROWSER_FIXTURE_VIDEO_CODEC=mpeg4` to the runner environment.

The Linux runner exercises X11 and nested Wayland with native pointer input. It captures hover disclosure, focus disclosure over a live Browser, the context menu, and successful navigation; it also checks complete clipboard content, independent tabs, and native visibility after closing. Unit and rendered tests cover external routing without launching the system browser, stale sessions, rejected destinations, keyboard navigation, selection drags, Unicode, resizing, tables, and streaming offset maps.

On macOS, build the same example and run it with the release application's Info.plist:

```sh
ZERON_TRANSCRIPT_LINK_FIXTURE_ONLY=1 \
  scripts/run-macos-browser-fixture.sh target/debug/examples/browser-fixture \
  /tmp/zeron-transcript-links-macos
```

Native runtime-failure validation can be run on Linux by placing an empty `libwebkit2gtk-4.1.so.0` in a temporary directory, setting `LD_LIBRARY_PATH` to that directory, and setting `ZERON_LINK_FIXTURE_MISSING_RUNTIME=1` for the fixture. This simulates the dynamic loader's failure without changing installed packages. The main application must remain usable and expose the error and external-browser control.

## Validation recorded for this change

- `cargo test -p zeron-ui`: 916 tests passed, plus doc tests.
- `cargo build -p zeron`: passed.
- `cargo check -p zeron`: passed.
- Rustfmt checks passed for the modified library modules and the new native scenario; shell syntax and whitespace checks passed. The existing browser fixture's formatting was preserved.
- Linux X11: native input, clipboard, navigation, tab ownership/close, and visual disclosure over WebKitGTK verified.
- Linux Wayland: native pointer, clipboard, keyboard actions, navigation, and tab close assertions passed in nested Weston. Captures did not reliably reflect updated frames, including with software Vulkan and an explicit frame request; visual verification of Wayland overlays remains outstanding.
- Missing WebKitGTK: the simulated loader failure passed on X11 and Wayland; X11 captures show the runtime error and the external-open control.
- macOS: unavailable in this environment; the native scenario and runner are provided, but macOS visual verification remains outstanding.
