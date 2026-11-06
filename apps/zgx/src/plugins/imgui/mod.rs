//! Native Dear ImGui windows for Luau plugins — the native core.
//!
//! A plugin window is one Dear ImGui context plus one wgpu renderer, kept in
//! this module and never exposed to Luau: a script asks for widgets, the host
//! builds them, and the GPU replays what Dear ImGui drew. Nothing here hands
//! out a pointer, an OS handle or a `wgpu` object.
//!
//! This is the foundation the rest of the feature builds on:
//!
//! - [`runtime::WindowContext`] — one context per window, suspended between
//!   frames and activated for its turn (`suspend`/`activate`), because Dear
//!   ImGui allows one active context at a time.
//! - [`render::WgpuRenderer`] — the wgpu backend, built from the device,
//!   queue and format a `ViewportFrame` brings, with buffers and textures
//!   that survive frame to frame.
//! - [`snapshot::OffscreenTarget`] — rendering without a window, and the
//!   pixel readback tests and diagnostics snapshots use.
//! - [`permission`] — the `imgui.windows` grant every host action passes
//!   through, and the sentence a refused plugin gets.
//!
//! The frame is split the way the window and script layers need it:
//! [`runtime::WindowContext::prepare`] copies a finished frame off the
//! context (the update phase, where Luau will run), and
//! [`render::WgpuRenderer::render`] replays it (the paint phase, which never
//! touches the context). [`render::ImguiViewport`] is the zgx `Viewport` a
//! plugin window draws through.
//!
//! Not here yet: the native window, the Luau bindings and the frame pump.
//! They call this module, not the other way round, which is why the native
//! API is allowed to have no caller in the app yet: the tests below exercise
//! every part of it, and the next milestone wires it to a window.
#![allow(dead_code)]

pub mod permission;
pub mod render;
pub mod runtime;
pub mod snapshot;

#[cfg(test)]
mod tests;
