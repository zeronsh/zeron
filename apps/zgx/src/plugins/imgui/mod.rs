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
//! - [`frame`] — the frame pump: one script-driven frame per shown window,
//!   the per-callback CPU budget, and the errors that keep the plugin loaded.
//! - [`input`] — the window's own pointer events → Dear ImGui's IO, so a
//!   widget the plugin drew answers the pointer over it.
//!
//! The frame is split the way the window and script layers need it:
//! [`runtime::WindowContext::prepare`] copies a finished frame off the
//! context (the update phase, where Luau will run), and
//! [`render::WgpuRenderer::render`] replays it (the paint phase, which never
//! touches the context). [`render::ImguiViewport`] is the zgx `Viewport` a
//! plugin window draws through.
//!
//! Not here yet: the rest of the widget bindings. They call this module, not
//! the other way round: [`window::Windows`] makes the native windows,
//! [`PluginWindow`] is what the pump prepares frames for, and [`luau`] serves
//! the window ops to a plugin's script.

#![allow(dead_code)]

pub mod frame;
pub mod input;
pub mod luau;
pub mod permission;
pub mod render;
pub mod runtime;
pub mod snapshot;
pub mod window;

/// The host's windows: one per `(plugin, window id)`, made through the
/// app's `WindowFactory` (see [`window`]).
pub use window::{CloseReason, Windows as ImGuiWindows};

#[cfg(test)]
mod tests;
