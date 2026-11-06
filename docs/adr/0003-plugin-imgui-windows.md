# ADR 0003: Give Luau plugins native Dear ImGui windows

- Status: Accepted
- Date: 2026-11-06

## Context

Zeron's Luau plugins can add nodes, tabs, and pages to Zeron's interface, but they cannot
draw an interface of their own. Plugin authors need real desktop tools (monitors, consoles,
inspectors) with immediate-mode widgets, continuous state, tables, and custom drawing that
the declarative `ui.*` surface does not express. Zeron's plugin system is otherwise complete:
manifests, signatures, consent, grants, sandbox, limits, storage, and lifecycle already exist,
and a parallel plugin runtime would duplicate all of it.

The environment constrains the solution. `apps/zgx` is the only plugin host in the repository
and draws through a pinned `zgx` framework that creates native windows at runtime and renders
GPU views, but exposes neither winit windows nor its event loop. Luau closures cannot be
handed to Rust. The production engine, the gpui app, and the framework checkout are off limits.

## Decision

### Target: `apps/zgx`, the only plugin host

The feature ships in `apps/zgx` and reuses its existing plugin bridge, consent, grants,
storage, and script limits. It is the only place plugins run: the gpui app (`apps/zeron`)
loads none, and `crates/engine/src/plugins` is a headless Luau service subsystem with no UI
host. Building native windows anywhere else would mean a second plugin manager, a second
consent flow, and a second window runtime for no user-visible gain. The ImGui host lives in
`apps/zgx/src/plugins/imgui/` as a separate module reached through a small host API, so the
bridge keeps discovery, grants, reloads, and storage while containing no drawing code.

### Binding and renderer: `dear-imgui-rs` 0.22 with `dear-imgui-wgpu` 0.22

Plugins draw through `dear-imgui-rs = "=0.22.0"` (Dear ImGui 1.93.1 WIP, docking branch; docking
is compiled in and enabled at runtime) and
`dear-imgui-wgpu = { version = "=0.22.0", default-features = false, features = ["wgpu-29"] }`.
The `wgpu-29` feature selects the renderer's `wgpu 29.0` dependency, which resolves to
`wgpu 29.0.4`, the version `apps/zgx` already links through `zgx`. The app is a separate
cargo workspace with its own lockfile, so both pins are declared and resolved there. The
`&wgpu::Device` and `&wgpu::Queue` in `ViewportFrame` are therefore the same types the renderer
expects: no second device, no version bridge, no copied textures. `default-features` must stay
off because the crate's default is `wgpu-30`; leaving it on would link two wgpu major versions.
Both pins are exact (`=`) because the binding's renderer API and the framework's wgpu move
together; a wgpu bump in `zgx` requires a matching `dear-imgui-wgpu` feature, not a floating
range.

### Window model: framework-native windows, one context per window

One plugin window is one native `zgx` window opened with
`WindowFactory::open(WindowOptions, build)` whose content is a single full-bleed `gpu_view`
implementing `Viewport::render`. The framework supplies the title bar, resize, min/max size,
focus and close buttons, `frame_autosave` geometry, and raw `on_document_event` input; the host
supplies pixels and widget semantics. Every plugin window gets its own ImGui context, because a
crash, a font atlas, or a docking layout must not be shared between plugins. The binding permits
only one *active* context at a time, so the host suspends the previous window's context and
activates the next around each window's turn; renderers and draw data stay bound across that
switch. All ImGui calls happen on the UI thread and no context is shared across threads.

Rejected alternatives:

- **Forking or patching `zgx`.** The public API already provides windows, GPU views, saved
  frames, input, and CPU budgets; the checkout is a pinned dependency this repository does not
  own, and a fork would strand the app on an unreviewed commit and block framework updates.
- **A second event loop.** Running winit or SDL beside the framework's loop duplicates platform
  and GPU lifetimes, breaks input routing between windows, and adds a thread this UI-thread-only
  API must not use.
- **`dear-imgui-winit`.** It needs the winit event loop and window handles the framework never
  exposes; it could only work inside a fork, with the same objections as above.
- **The older `imgui-rs` binding.** It tracks an older Dear ImGui and has no maintained wgpu 29
  renderer; `dear-imgui-wgpu` 0.22 is the renderer verified against this wgpu version.

The choice was validated by a running spike on Windows: an 800×600 offscreen frame with real
content (48.1% non-background pixels, 70 distinct colours), two contexts rendered interleaved
through suspend/activate, a simulated click returning `true` on its release frame, typed
characters reaching an `input_text` buffer, a clean build in about 29 seconds and no
clang/bindgen requirement.

### Frame pump: script-first, paint-second

Dear ImGui is immediate mode, but Rust cannot call a Luau closure. The only supported way for
the host to make a script run is its existing call queue
(`push_call` → `plugin.<id>.calls` → the SDK's effect loop), so a frame is produced in two
phases. The update phase pushes one frame request per shown window; the SDK drains it and runs
the plugin's `on_render(ui)`, whose widget calls come back as host actions on the open frame and
whose completion closes it. The host then ends the ImGui frame, tessellates the draw data, keeps
it, and invalidates the GPU view. The paint phase only replays that prepared draw data into
`ViewportFrame`; it never runs Luau.

At most one frame is in flight per window, which is both the throttle and the back-pressure: a
slow script delays only its own next frame. Hidden, minimized, and closed windows receive no
request and draw nothing. If `on_render` errors or exceeds its 8 ms CPU budget, the host aborts
that frame, keeps the last good draw data (or an ImGui-drawn error panel), records the error, and
stays responsive; three consecutive failures close the window while leaving the plugin loaded.
Because painting never executes script, a slow plugin cannot stall presentation mid-frame or
stutter Zeron's main window.

### Permission: `imgui.windows`

Native ImGui windows require the new lowercase-dotted permission `imgui.windows`. The existing
`windows` permission is not a substitute: it gates zgx's declarative windows (and the
`clipboard`/`files`/`web` family), which Zeron never follows as plugin-declared windows, while
`imgui.windows` alone unlocks the ImGui host actions and the SDK's `zeron.imgui` table. It is
added to the engine's known-permission list (an unlisted permission marks a plugin broken) and
mapped to a capability in the app's grants, with consent wording that the plugin can create
native application windows and draw in them. Enforcement is doubled: the VM only exposes granted
capabilities, and every ImGui host action checks `Item::may("imgui.windows")` plus the plugin's
enabled status. Consent is per plugin and revocable; revoking it closes that plugin's windows
immediately. No plugin can read another plugin's windows, obtain an OS window handle, a GPU
resource, a C++ object, or draw into Zeron's own window.

### Canonical limits

These are the shipped defaults, defined in one place and enforced by the host:

| Limit | Value |
| --- | --- |
| Windows per plugin | 8 |
| Windows in total | 32 |
| Window size, minimum | 120 × 80 logical points |
| Window size, maximum | 3840 × 2160 logical points (clamped, not refused) |
| Window title | 200 bytes |
| External textures per plugin | 32, each at most 4096 × 4096 |
| Queued UI operations per frame | 4096 |
| Render callback CPU budget per frame | 8 ms |
| Plugin VM memory | 32 MiB (the existing per-plugin script limit) |

A hidden window consumes no frame time; a window that exceeds a dimension is clamped to the
platform and policy maximum rather than refused.

### Lifecycle and teardown

Windows are keyed by `(plugin id, window id)`, and every teardown point goes through the same
close path:

- `Bridge::expose` registers the plugin's ImGui state once and idempotently.
- `Bridge::forget` (plugin removed or replaced) closes its windows and drops saved state.
- `Bridge::plugin_failed` (script error withdraws the plugin) closes its windows; Zeron stays up.
- The window/script host close path closes every remaining window on app shutdown.
- A whole-window script reload closes old windows and drops renderer resources before the new
  script runs, so reload never duplicates a window or leaks a texture.
- Disabling a plugin closes its windows and frees GPU resources while keeping it installed.
- Closing a window never unloads or disables the plugin; reopening restores what was persisted.

### Persistence

| Data | Where | Key |
| --- | --- | --- |
| Position, size, maximized | `WindowOptions::frame_autosave` | `plugin.<plugin-id>.<window-id>` |
| Visibility preference | plugin store | `imgui.windows.<window-id>.visible` |
| Docking layout | plugin store (ImGui ini payload) | `imgui.windows.<window-id>.docking` |
| Plugin-defined preferences | plugin store | `imgui.windows.<window-id>.prefs` |

All writes go through the plugin's own store, so isolation is structural rather than a check:
one plugin cannot write another's keys. There is no global `imgui.ini`. Saved geometry is
revalidated by the platform, so a frame saved on a disconnected display does not reopen off
screen; on Wayland only size and maximized state survive, and the docs say so.

### Docking: in-window, opt-in per window

In-window docking is part of the deliverable. It is opt-in per window (`docking = true`), because
most plugin windows are a single panel and should not pay for a dockspace, a tab bar, or layout
machinery. When enabled, the host creates one dockspace over the window's client area with
`ConfigFlags::DOCKING_ENABLE`, lets panels dock into splits and tabs, and saves the layout in the
plugin store keyed by plugin and window. A window that did not opt in shows no docking artefact
and never inherits another plugin's layout.

### Multi-viewport: an attempt with a recorded verdict

Detached ImGui viewports (ImGui panels dragged out into their own OS windows) are not part of
the supported surface. Dear ImGui's viewport support needs a platform backend, and
`dear-imgui-winit` is unusable because the framework exposes no event loop, so the only viable
path is a hand-written `ImGuiPlatformIO` backend mapping each ImGui viewport to a
`WindowFactory` window and GPU view. That attempt is time-boxed for the last milestone and ends
with a recorded verdict: either it is implemented with evidence, or the precise limitation and
its blocker are documented. Known constraints make success uncertain: Wayland forbids clients
placing windows, so drag-to-move would no-op there; each detached viewport costs a full GPU
surface; and viewport windows must not join AppKit tabs. Independent plugin-owned windows remain
the supported feature, and no document or diagnostic may claim multi-viewport works until the
verdict says so.

### Platform matrix

| Platform | Status |
| --- | --- |
| Windows | The primary target: the app is built, run, and validated with real windows on D3D12 and Vulkan, and every feature claim is evidenced here first. |
| Linux | Built and run under WSLg (Wayland and X11), with real windows, input, and snapshots validated as the feature lands. Wayland forbids client-side placement, and saved frames restore only size and maximized state. |
| macOS | Code paths are compile-gated and a CI job builds and tests `apps/zgx`; **not runtime-verified in this environment**, and never claimed as working without CI evidence. |

## Consequences

- Plugins gain native desktop tool windows without a second runtime, permission system, event
  loop, or GPU device; the existing bridge stays the single entry point.
- The build gains a C++ dependency compiled by `cc` (MSVC on Windows, g++ on Linux) with no
  clang or bindgen: about 29 seconds clean, a ~10 MiB static library, and roughly 17 MiB added to
  a debug executable.
- A frame request waits for the SDK effect loop to drain, so interaction latency includes one
  zgx frame. That is accepted for desktop tools and must be checked against real drags and
  scrolling when input lands; if it is not immediate, the pump, not the user, is at fault.
- One frame in flight per window and no pumping for hidden windows bound both CPU and memory;
  buffers, draw lists, and the font atlas are reused rather than reallocated per frame.
- Renderer failures and device loss are contained per window: `device_changed()` drops every
  GPU-owned resource and the next frame rebuilds it. If the binding's managed-texture protocol
  ever conflicts with per-window pumping, `Context::render_legacy()` plus a first-party renderer
  inside `render.rs` is the contained fallback, with no change to the Luau API.
- The mandatory suspend/activate around each window turn means any new ImGui call site must run
  inside the pump; touching a context outside its turn is a defect, not a style choice.
- The dependency pins must move with the framework's wgpu version, and the honest platform
  statements above are part of the contract: no macOS claim before CI runs it, no
  multi-viewport claim before the verdict exists.
