# Ask: compile a custom fill's pipeline before its first frame

**Priority: high.** The window freezes for 70-360 ms the first time a custom-shaded node draws in a launch, which is in Zeron the moment the first session starts working (its status ring), right when the user is watching.

**Today.** The frame that first draws a node with a custom `bg_shader` fill blocks on the pipeline compile: 361 ms the first time after a build, 73 ms with Metal's shader cache warm. `inspect frames` reports it as `paint`, with few instructions (about 52 M in the 361 ms frame): the main thread is waiting. Sampled main-thread stack:

    GpuRenderer::render_flat
      GpuRenderer::prepare_fill
        wgpu CoreDevice::create_render_pipeline
          naga::front::wgsl::Frontend::parse ...
          MTLCompiler ...

**Repro:** any app whose first frame of a custom fill (a WGSL `fill(input: FillInput)` given as `bg_shader`) comes after launch: draw it on a click, and read `zgx inspect frames` for that frame. Zeron: `ui/ds/status.luau` (`RING`), drawn when a session starts working.

**Ask:** compile a custom fill's pipeline when the script that declares it loads, or on a background thread, drawing a plain fill (or nothing) until it's ready; never synchronously in the frame that first draws it.

**Seen on:** zgx `b3371f8` (Zeron's pin), macOS.
