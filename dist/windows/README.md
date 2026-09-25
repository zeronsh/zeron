# Windows icon

`glitch-flow.ico` contains 16, 24, 32, 48, 64, 128, and 256 pixel PNG frames
converted from the Glitch Flow cat artwork with high-quality resampling.

`apps/glitch-flow/build.rs` compiles `glitch-flow.rc` into the Windows executable for
both debug and release builds. Resource ID 1 is required by GPUI's Windows
icon loader. No installer or adjacent image file is needed at runtime.
