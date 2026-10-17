# Content references

The shared Markdown parser recognizes complete `:codex-file-citation{...}`
single-line directives outside code, HTML, and existing links. `path` must be an absolute
file path; `purpose`, when present, must be `source` or `output`. Unknown or
incomplete directives remain ordinary text.

References become ordinary file links with a filename label. Existing transcript
file navigation resolves them in the originating chat's device and checkout,
including read-only absolute files outside the checkout. This does not install
skills, alter harness output, or rewrite the stored transcript. Copying retains
the destination path, following the existing abbreviated file-link behavior.

Compatibility spelling belongs in `zeron-markdown::content_reference`, shared
by desktop and mobile. File reads and navigation remain host responsibilities.
Arbitrary HTML in Markdown is still literal text.

## HTML previews

Complete Codex `visualize` markers with an absolute `.html`/`.htm` path become
**Preview · filename** links. Clicking opens a session-owned Browser tab in the
right panel. The shared decoder produces a typed `HtmlPreview`; the browser
accepts this request without inspecting the current harness. Additional content
formats can decode to the same request without changing the browser or CRDT.
Unknown modes, malformed JSON, code examples and partial markers stay literal.

Files are read by `WorkspaceFilesClient` through the originating chat's engine
and target device, including read-only paths outside its checkout. Closing the
tab cancels pending delivery. Reload reads the file again; **Open source file**
uses normal session-owned file navigation. Missing, binary, truncated or
oversized files report errors inside the tab.

The browser loads an in-memory document with native WebKit/WebView2 APIs, with
no temporary HTTP server or `file:` navigation. Each preview has an ephemeral
browser context, separate from website login profiles. Content runs in a
scripts-only sandboxed `srcdoc` frame with an opaque origin and a restrictive
CSP: inline scripts/styles and embedded images/fonts work; network requests,
filesystem access, forms, popups and host APIs are unavailable. References to
relative assets or external libraries therefore require a self-contained export.
The final encoded document is capped below WebView2's 2 MiB limit on all hosts.

The optional Codex compatibility runtime is independently implemented here;
no proprietary plugin assets are bundled. It provides navigation for
`.viz-carousel > section[data-variant]`; ordinary buttons and layout logic run
from the original HTML. This preview path is independent of native Luau visuals.

Native regression scenario (macOS):

```sh
cargo build --locked -p zeron-ui --example browser-fixture --features browser-fixture
ZERON_HTML_PREVIEW_FIXTURE_ONLY=1 bash scripts/run-macos-browser-fixture.sh \
  target/debug/examples/browser-fixture /tmp/zeron-html-preview
```

On Linux, pass the same environment flag and binary to
`scripts/test-linux-browser.sh`. The fixture checks inline script execution,
previous/next layout navigation, parent DOM/storage isolation, and blocked
network requests. `ZERON_HTML_PREVIEW_FILE=/absolute/example.html` additionally
captures a local example without committing its contents.

Recorded on macOS: 53 shared Markdown tests, all 1,779 UI library tests with
`--test-threads=1`, the application compile check, and the native HTML fixture
passed. The default parallel UI runner aborts in macOS HIToolbox's concurrent
keyboard-input-source access; its serial run passes. The HTML fixture uses
WebKit snapshots on macOS, without requiring screen recording permission.
Linux/WebView2 implementations are included; their native execution remains
to be verified on those platforms. Existing file-link routing uses POSIX paths.

Validation: `cargo test --locked -p zeron-markdown`. Coverage includes punctuation
and Unicode in paths, code examples, malformed directives, source offsets after
references, and full/incremental parity at every character boundary.
