# Markdown file preview

Files opens `.md` and `.markdown` documents in Preview by default (case-insensitive extensions). The Code / Preview selector preserves the user's choice while the document remains open. The preview renders the current buffer, including unsaved edits, using Zeron's Markdown typography and existing toolbar controls. It does not change autosave settings.

By default, HTTP(S) links open a new Browser tab in the file's conversation, from both the Files panel and standalone file tabs. Right-click for **Open in Zeron**, **Open in external browser**, **Copy link address**, or to change **Open links in Zeron**. That preference affects only normal activation; the explicit actions, tooltip, and context menu remain available. Hover or keyboard focus reveals the complete destination. Relative file links and document anchors retain their preview navigation; `mailto:` links use the system mail handler. Web links use the same validation, session checks, platform fallback, and [Linux runtime requirements](reference/linux-browser.md) as [transcript links](transcript-browser-links.md). Preview labels retain their full text.

The document is presented in a centered responsive column capped at 900px for readable line lengths. Code, tables and media share that column; images and Mermaid diagrams are centered at their natural size when narrower. On small panels the column fills the available width with Zeron's standard gutters.

Task lists use the existing GPUI base checkbox with Zeron theme colors and icons. Toggling a task changes only its marker in the current editor buffer, preserving formatting and the normal change event, autosave and editor undo/redo history. Source byte ranges distinguish duplicate and nested tasks. An outdated preview cannot edit a newer buffer, and checkboxes are disabled without an editable document. Chat retains its existing task rendering.

Hovering a Markdown block exposes the same add-comment button used in diffs. The shared inline draft and comment cards appear below the block across the full preview panel width, with their location, input, text and actions aligned to the centered reading column. Each card retains its original file and line reference. Comments use the existing file-review staging, removal and editor anchors; they join the composer without modifying the Markdown. A paragraph, list, table or fence is one comment target, cited at its first source line. Existing notes on inner lines appear below their containing block. Cancel or Escape dismisses the draft. Truncated and non-editable previews cannot start comments, and stale preview offsets are rejected.

Workspace-relative images are loaded from the device owning the checkout. HTTP(S) images remain links. HTML and MDX are not executed. Images and diagrams open in the shared centered lightbox with trackpad pinch, Ctrl + wheel zoom and pan; see [image preview and zoom](image-preview.md). Chat renders Mermaid with the same engine; see [Mermaid in chat](#mermaid-in-chat).

Mermaid diagrams use the same fence frame, header metrics, border, background and code actions as ordinary fenced code blocks. Switching between diagram and source replaces the body inside that single frame.

Diagrams are drawn in Zeron's own style rather than the engine's stock theme. The canvas is transparent, so the diagram sits directly on the fence body under any surface treatment. Nodes are cards with 8px corners and a hairline border: white in light mode, one ink step above the fence in dark mode. Groups are a faint wash. Edge and group labels use muted text, and connectors use faint text. Decisions, sequence notes and activations carry a soft tint of the selected accent, and Gantt bars derive their hues from it. Gantt gridlines use the theme border. Colors set explicitly with `style` or `classDef` are kept. The layout uses tighter node and rank spacing than the engine default, so diagrams scaled to the reading column keep larger labels. In the lightbox, the diagram is drawn on a rounded plate in the fence body color so it remains legible over the scrim.

Source fences use the same interaction model as chat code blocks: long lines have an independent horizontal scroll plane and a hover scrollbar, Copy changes to a transient `Copied` confirmation, and the Fit content control wraps lines to the available width. The Fit content choice is the same device-level persisted setting used by chat, so changing it in either surface updates both; horizontal offsets remain local to each fence.

## Mermaid engine decision

The embedded engine is `mermaid-rs-renderer` **0.3.1**, MIT licensed, with default CLI/PNG features disabled. Its interface is isolated in `markdown/mermaid.rs`. Generated SVG is consumed by GPUI's existing SVG renderer.

The six fixtures under `scripts/fixtures/markdown-preview/` were rendered with this version and compared visually against official Mermaid **11.12.0**, with `securityLevel: strict` and `htmlLabels: false`, in headless Chrome. Flowcharts with subgraphs, sequence alternatives, classes and cardinalities, state transitions, ER attributes and Gantt dates retained their content. The corpus includes Spanish labels, long labels and explicit colors. Tests also cover `<br/>` labels and malformed source.

The native layout is not pixel-identical to Mermaid.js. Sequence actors use rectangular boxes in the evaluated output; edge routing, spacing, state-loop labels and default styling differ. This corpus establishes support for these examples, not full Mermaid syntax parity. Unsupported or invalid input remains accessible as source with a diagnostic. Browser rendering was used only for development comparison and is not a product dependency.

The adapter uses Zeron's resolved theme/font. SVG preparation loads the same bundled Geist and Geist Mono faces as the interface and uses bundled Geist as a fallback for unavailable families, including virtual system font names. This prevents unresolved fonts from silently removing labels during text-to-path conversion. Unit tests rasterize the corpus through `gpui::SvgRenderer` in light and dark themes with both bundled families, including the prepared image path used by Files, and check deterministic output. Separate regression tests verify that text produces visible pixels with Geist Mono and unavailable font names. To retain SVG and prepared PNG artifacts while running that test, set `ZERON_MERMAID_ARTIFACTS` to a local output directory.

## Limits and lifecycle

Preview parsing is debounced by 120ms and limited to the first 2 MiB of Markdown, with a visible truncation notice. Text rows are virtualized and their derived render caches retain only the previous viewport. A document admits at most 32 distinct images and 32 distinct Mermaid sources, with a combined 64 MiB media retention budget including estimated decoded texture memory. Excess media displays a limit message; its Markdown/source remains available. Image reads have a 30-second deadline and at most three concurrent jobs. Mermaid layouts are serialized.

Source revisions reject obsolete async results. Image watcher events invalidate both completed and pending loads; changing theme regenerates diagrams. Switching back to Code releases the preview's derived state while preserving the editor and preview scroll position. Closing a preview schedules image asset and atlas eviction. Image/diagram completion remeasures rows with an absolute scroll anchor.

Image reads are limited to 8 MiB, with 384 KiB binary chunks and content-hash validation between chunks. Raster images are decoded with 4096px dimension and 64 MiB allocation limits, then flattened to a static PNG, including the first frame of animations. SVG sources are parsed and reserialized with embedded/external image resolution disabled; the prepared vector source is retained (up to 8 MiB) regardless of its natural dimensions. A bounded outer SVG viewport preserves the complete original coordinate system. Preview rasters adapt to the panel and display density (up to 4×), capped at 1,048,576 pixels and 4096 pixels per side. The lightbox uses its own viewport and up to 2,097,152 pixels within the remaining document memory budget, reusing the preview when there is no room for another raster. CPU pixels, GPU textures and retained SVG bytes are budgeted. An SVG or diagram is accounted at the raster it currently holds, not the largest raster any view could request. A re-raster at a higher width or density happens only when the larger variant fits the remaining budget; otherwise the current raster stays. Enlarged variants are evicted on close, replacement, source changes and preview disposal. Unsupported SVG content may be omitted.

Mermaid source is limited to 16 KiB, 256 lines and 2048 lexical segments; generated SVG is limited to 2 MiB. The native engine has no cooperative cancellation or hard execution deadline. Its CPU work is serialized across previews and runs off the UI thread; obsolete results must be rejected by their owning view. These limits bound admitted work but do not constitute a strict wall-clock guarantee.

Manual validation on macOS and a second physical remote device must be recorded separately from headless Linux tests. Automated tests cannot establish platform-specific focus, GPU rendering or real network behavior by themselves.

## Mermaid in chat

Assistant replies render ```` ```mermaid ```` fences as diagrams, sharing the file preview's engine, fence frame, source toggle, Copy action and lightbox. Inline images in chat keep their existing text rendering.

Streaming never renders a fence that may still be growing. Only blocks that a later row of the same reply follows, or blocks of a completed reply, request a diagram. The streaming tail keeps its source; per-token commits start no render work. Until a diagram is ready, the fence shows its ordinary source, so completion changes the row height at most once. A failed render keeps the source and shows the engine's diagnostic in a warning marker in the fence header.

Rows request their fences while they lay out, so only painted diagrams cost anything. One serialized loop per transcript renders them off the UI thread, choosing its next source between renders and dropping requests whose rows scrolled away. Retained diagrams share a 64 MiB budget and are evicted least recently painted first, with a 64-entry cap; an evicted diagram renders again when its row returns. Diagrams painted in the latest two passes are never evicted. Theme changes discard every diagram, and results computed under the previous theme are rejected. Rasters follow the conversation column width and display density, under the same budget check before a larger re-raster.

A diagram swap remeasures only the rows painting it and uses the same layout signals as other row-height changes. The bottom pin glides to the new end, and the own-turn runway reservation absorbs the change in the same layout without moving the sent prompt. Source toggles keep the stable row identity across streaming completion. The lightbox enlarges the diagram within the memory the retained diagrams leave available and releases that raster when it closes.

## Implementation validation

The implementation was checked on Linux with the following commands:

| Command | Result |
| --- | --- |
| `cargo test --release --locked -p zeron-ui --lib -- --test-threads=1` | 762 passed |
| `cargo test --release --locked -p zeron-engine --lib` | 161 passed |
| `cargo test --release --locked -p zeron-proto -p zeron-rpc` | 47 passed, 1 previously ignored |
| `cargo test --release --locked -p zeron-engine --test workspace_files` | 3 passed |
| `cargo test --release --locked -p zeron-engine --test device_routing workspace_file_surface_proxies_over_the_relay` | 1 passed |
| `cargo check --locked -p zeron` | Passed |

The UI tests cover unsaved content without extra saves, independent selection surfaces, pointer opening and Escape dismissal of the lightbox, obsolete work, media limits and image invalidation. The relay test runs two engines through a test relay and checks remote image reads and checkout identity. The Mermaid corpus produces twenty-four SVG and PNG pairs across light/dark themes and Geist/Geist Mono. Prepared ER (light) and sequence (light/dark) PNGs with Geist Mono were inspected visually after fixing font resolution. Regression tests first reproduced zero visible text pixels with Geist Mono and an unavailable family, then passed after the fix.

Formatting checks pass for all changed Rust files, and `git diff --check` passes. Repository-wide `cargo fmt --all -- --check` reports existing differences in unrelated files; those files were left unchanged. Full native application review on Linux/macOS, HiDPI interaction and a physical remote connection remain manual acceptance checks.

### Browser link follow-up

The file preview's session Browser routing was checked on Linux with `cargo test -p zeron-ui -- --test-threads=1` (918 passed), `cargo build -p zeron`, Rustfmt for changed modules, and `git diff --check`. New tests cover the preview-to-Files event, source-session ownership, complete destinations and clipboard content, rejected URLs, mail handling, preview suspension, and the Shell subscriptions for both Files and standalone file tabs. The first parallel suite run failed the existing `active_reply_text_selection_survives_streaming_and_completion` test; it passed in isolation and in the complete sequential run. This follow-up did not repeat native visual verification on Linux or macOS.

### Mermaid in chat follow-up

Chat Mermaid rendering was checked on Linux with `cargo test -p zeron-ui --lib -- --test-threads=1` (1528 passed), `cargo check -p zeron`, Rustfmt for changed modules and `git diff --check`. Cache unit tests cover single queueing per source, row tracking, dropping requests that scrolled away, theme invalidation of retained and in-flight results, eviction ordering that spares recently painted diagrams, and source toggles following their rows. Transcript tests stream a fence token by token and verify that the tail starts no render. They also verify that the following block renders it once, that the row height changes and the toggle restores the source, and that completion keeps the diagram and toggle. Other transcript tests check that a pinned overflowing stream stays at the bottom through the swap and further streaming. They also check that rendering a completed tail neither moves the own-turn prompt nor opens blank space below the runway, and that the diagram lightbox opens and releases its raster. In one parallel suite run, the existing `markdown_drag_tracks_each_table_column_and_wrapped_cell` and `rendered_truncation_resizes_and_selects_the_original_url` tests failed. Both passed in isolation and in the complete sequential run. Native visual verification on Linux or macOS was not performed.

### Diagram style follow-up

The Zeron diagram style was checked with `cargo test -p zeron-ui --lib -- --test-threads=1` (1531 passed), Rustfmt for changed modules and Clippy, which reports no new warnings in changed code. New unit tests cover the transparent canvas, rounded node corners, accent-tinted default decisions alongside an explicitly styled one that keeps its colors, themed Gantt gridlines, and diagram type detection past front matter and comments. The six-fixture corpus was rendered with `ZERON_MERMAID_ARTIFACTS` in light, dark and Geist Mono variants. It was composited onto the fence body color at the chat column size and inspected visually. Native visual verification in the running application was not performed.
