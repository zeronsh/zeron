//! One plugin window's Dear ImGui context.
//!
//! Dear ImGui has one process-global current context, so a plugin window
//! keeps its context *suspended* between frames and activates it for its own
//! turn: `WindowContext::suspend`/`activate` around the frame that builds
//! the UI. Two windows can then render in the same process, one after the
//! other, each with its own widgets, style and font atlas.
//!
//! The context is created suspended, so creating window B never disturbs
//! window A's active turn. `with_active` is the same round trip as a scope,
//! and `prepare`/`render` are the two halves of the frame pump: the first
//! copies the finished frame off the context (the update phase, where Luau
//! will run later), the second replays it on the GPU (the paint phase, which
//! never touches the context).

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

use dear_imgui_rs::fonts::FontSource;
use dear_imgui_rs::{
    Context, ContextId, FontId, LegacyFontAtlas, SuspendedContext, TextureStatus, Ui,
};
use zgx_gpu::wgpu;

use super::render::{
    AtlasImage, DrawStats, PreparedFrame, RenderError, RenderStats, RenderTarget, WgpuRenderer,
};

/// Seconds a frame is told has passed when the caller has no clock.
pub const DEFAULT_DELTA_TIME: f32 = 1.0 / 60.0;

/// What a frame is built and drawn at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameSpec {
    /// The window's size in logical points (Dear ImGui's `io.DisplaySize`).
    pub display_size: [f32; 2],
    /// Device pixels per point (`io.DisplayFramebufferScale`).
    pub framebuffer_scale: f32,
    /// The target's size in device pixels.
    pub target_size: (u32, u32),
    /// Seconds since the previous frame.
    pub delta_time: f32,
}

impl FrameSpec {
    /// A frame for a `size`-pixel target drawn at `scale` device pixels per
    /// point, sixty frames a second.
    pub fn from_device_pixels(size: (u32, u32), scale: f32) -> Self {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        Self {
            display_size: [size.0 as f32 / scale, size.1 as f32 / scale],
            framebuffer_scale: scale,
            target_size: size,
            delta_time: DEFAULT_DELTA_TIME,
        }
    }

    pub fn with_delta_time(mut self, delta_time: f32) -> Self {
        self.delta_time = delta_time;
        self
    }
}

/// Why a window could not build or draw a frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImGuiHostError {
    /// Dear ImGui refused a context operation (creation, activation, a
    /// conflicting context).
    Context(String),
    /// The frame could not be drawn.
    Render(RenderError),
}

impl ImGuiHostError {
    fn context(message: impl Into<String>) -> Self {
        Self::Context(message.into())
    }
}

impl fmt::Display for ImGuiHostError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Context(message) => formatter.write_str(message),
            Self::Render(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for ImGuiHostError {}

impl From<RenderError> for ImGuiHostError {
    fn from(error: RenderError) -> Self {
        Self::Render(error)
    }
}

/// One frame's outcome: what it drew and what it cost.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameOutcome {
    pub draw: DrawStats,
    pub render: RenderStats,
}

/// A plugin window's context, and the bookkeeping a frame needs.
///
/// Exactly one context may be active at a time, so a window that is not
/// taking its turn holds a [`SuspendedContext`]. Dropping the window drops
/// the context, its font atlas and everything it owned.
pub struct WindowContext {
    label: String,
    slot: Slot,
    frames: u64,
    /// Size of the atlas last prepared for painting.
    atlas_size: Option<(u32, u32)>,
    /// Whether a first (full) atlas upload has been prepared.
    atlas_created: bool,
    /// The fonts a plugin loaded, added to this context's atlas once: a
    /// font id only means something inside the context that made it.
    fonts: Rc<RefCell<HashMap<u64, FontId>>>,
}

enum Slot {
    Active(Context),
    Suspended(SuspendedContext),
    /// Only while a turn is being exchanged; never observable.
    Empty,
}

impl WindowContext {
    /// Creates a context for `label`, suspended, with its font atlas built.
    ///
    /// Fails when another context is active (the caller should suspend it)
    /// or when the process has contexts on another thread.
    pub fn try_create(label: impl Into<String>) -> Result<Self, ImGuiHostError> {
        let suspended = SuspendedContext::try_create()
            .map_err(|error| ImGuiHostError::context(error.to_string()))?;
        let mut window = Self {
            label: label.into(),
            slot: Slot::Suspended(suspended),
            frames: 0,
            atlas_size: None,
            atlas_created: false,
            fonts: Rc::default(),
        };
        // Legacy renderer mode: this host uploads the atlas itself, so the
        // context must not use the managed-texture protocol.
        window.with_active(|context| {
            context
                .set_ini_filename::<std::path::PathBuf>(None)
                .map_err(|error| ImGuiHostError::context(error.to_string()))?;
            context
                .font_atlas()
                .try_claim_legacy_renderer()
                .map_err(|error| ImGuiHostError::context(error.to_string()))?
                .build();
            Ok::<_, ImGuiHostError>(())
        })??;
        Ok(window)
    }

    /// A context for `label`, panicking if Dear ImGui refuses one.
    pub fn create(label: impl Into<String>) -> Self {
        Self::try_create(label).unwrap_or_else(|error| panic!("plugin imgui window: {error}"))
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    /// The context's process-unique identity.
    pub fn id(&self) -> ContextId {
        match &self.slot {
            Slot::Active(context) => context.id(),
            Slot::Suspended(context) => context.id(),
            Slot::Empty => unreachable!("a window context is never left empty"),
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(self.slot, Slot::Active(_))
    }

    /// Frames this window has prepared.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Suspends the context so another window can take its turn. A no-op if
    /// it is already suspended.
    pub fn suspend(&mut self) -> Result<(), ImGuiHostError> {
        if matches!(self.slot, Slot::Suspended(_)) {
            return Ok(());
        }
        let current = std::mem::replace(&mut self.slot, Slot::Empty);
        let Slot::Active(context) = current else {
            unreachable!("a window context is never left empty");
        };
        match context.suspend() {
            Ok(suspended) => {
                self.slot = Slot::Suspended(suspended);
                Ok(())
            }
            Err(error) => {
                let reason = error.reason();
                self.slot = Slot::Active(error.into_owner());
                Err(ImGuiHostError::context(reason.to_string()))
            }
        }
    }

    /// Activates the context for its turn. A no-op if it is already active,
    /// and an error (with the context kept) when another one is active.
    pub fn activate(&mut self) -> Result<(), ImGuiHostError> {
        if matches!(self.slot, Slot::Active(_)) {
            return Ok(());
        }
        let current = std::mem::replace(&mut self.slot, Slot::Empty);
        let Slot::Suspended(context) = current else {
            unreachable!("a window context is never left empty");
        };
        match context.activate() {
            Ok(active) => {
                self.slot = Slot::Active(active);
                Ok(())
            }
            Err(error) => {
                let reason = error.reason();
                self.slot = Slot::Suspended(error.into_owner());
                Err(ImGuiHostError::context(reason.to_string()))
            }
        }
    }

    /// Runs `f` with the context active.
    ///
    /// A suspended window goes through the binding's guarded scope, which
    /// ends an open frame and clears the current context even when `f`
    /// panics. A window the caller already activated is left active on
    /// return, and on unwind too — [`build_frame`](Self::build_frame) does
    /// not use this path for exactly that reason, taking a [`Turn`] instead.
    pub fn with_active<R>(
        &mut self,
        f: impl FnOnce(&mut Context) -> R,
    ) -> Result<R, ImGuiHostError> {
        match &mut self.slot {
            Slot::Active(context) => Ok(f(context)),
            Slot::Suspended(context) => match context
                .try_with_active(|context| Ok::<_, std::convert::Infallible>(f(context)))
            {
                Ok(value) => Ok(value),
                Err(error) => Err(ImGuiHostError::context(error.to_string())),
            },
            Slot::Empty => unreachable!("a window context is never left empty"),
        }
    }

    /// Builds and finishes a frame, returning it for painting.
    ///
    /// `want_atlas` copies the font atlas into the prepared frame; the
    /// renderer asks for it when it has none (its first frame, or after a
    /// device rebuild). The atlas is also copied whenever Dear ImGui reports
    /// it changed, so a new glyph or font size reaches the GPU.
    pub fn prepare(
        &mut self,
        spec: FrameSpec,
        want_atlas: bool,
        build: impl FnOnce(&Ui),
    ) -> Result<PreparedFrame, ImGuiHostError> {
        self.build_frame(
            spec,
            want_atlas,
            |_| {},
            |ui| {
                // SAFETY: `build_frame` keeps the context alive and its frame
                // open for exactly this closure, on this thread.
                build(unsafe { &*ui })
            },
        )
    }

    /// The font ids this context's atlas holds, by the plugin's font id: the
    /// one a frame pushes before it draws.
    pub fn font_ids(&self) -> Rc<RefCell<HashMap<u64, FontId>>> {
        Rc::clone(&self.fonts)
    }

    /// Adds a plugin's font to this context's atlas, once, and answers its
    /// id. The atlas is rebuilt after, which is what makes the glyphs
    /// available to this frame's text.
    pub fn ensure_font(
        context: &mut Context,
        fonts: &Rc<RefCell<HashMap<u64, FontId>>>,
        id: u64,
        bytes: &[u8],
        size: f32,
    ) -> Option<FontId> {
        if let Some(font) = fonts.borrow().get(&id) {
            return Some(*font);
        }
        let atlas = context.font_atlas();
        // SAFETY: the bytes are a complete font file the plugin shipped and
        // the atlas reads them only while building, which happens before
        // this returns; they outlive the call in the plugin's own style.
        let source = unsafe { FontSource::ttf_data_with_size(bytes, size) };
        let font = atlas.add_font(&[source]);
        atlas.try_claim_legacy_renderer().ok()?.build();
        fonts.borrow_mut().insert(id, font);
        Some(font)
    }

    /// Builds one whole frame: activates the context, runs `setup` (the
    /// plugin's style and font, which must be in place before the frame
    /// opens), sets the frame's size and delta time, opens the frame, runs
    /// `draw` while it is open, then ends it, copies its draw data and
    /// suspends the context again.
    ///
    /// `draw` gets the frame's [`Ui`] as a pointer rather than a borrow: the
    /// live path runs the plugin's script from inside this closure, and the
    /// script's widget calls come back into the same open frame through that
    /// pointer while this borrow of the context stands. Nothing outside this
    /// call may hold the pointer.
    pub fn build_frame(
        &mut self,
        spec: FrameSpec,
        want_atlas: bool,
        setup: impl FnOnce(&mut Context),
        draw: impl FnOnce(*const Ui),
    ) -> Result<PreparedFrame, ImGuiHostError> {
        let last_size = self.atlas_size;
        let created = self.atlas_created;
        // The turn takes the active context out of the slot and its `Drop`
        // suspends it again — including while a panic unwinds. Dear ImGui has
        // one process-wide current context, so a draw that panics must not
        // leave this one active: every later window's `activate` would fail
        // and all plugin windows would stay frozen until restart.
        let mut turn = Turn::take(self)?;
        let built = turn.with_context(move |context| {
            setup(context);
            let io = context.io_mut();
            io.set_display_size(spec.display_size);
            io.set_display_framebuffer_scale([spec.framebuffer_scale; 2]);
            io.set_delta_time(spec.delta_time);
            let token = context
                .try_begin_frame()
                .map_err(|error| ImGuiHostError::context(error.to_string()))?;
            draw(token.ui() as *const Ui);
            // `render_legacy` ends the frame and hands back its draw data;
            // the copy is what later phases need, so it is dropped at once.
            let prepared = {
                let frame = token.render_legacy();
                PreparedFrame::capture(&frame, spec.target_size)
            };
            // Legacy renderer mode, claimed for the read: the lease is
            // released at the end of this block, the mode stays.
            let (mut atlas_size, mut atlas_created) = (last_size, created);
            let mut prepared = prepared;
            if let Ok(atlas) = context.font_atlas().try_claim_legacy_renderer()
                && (want_atlas || atlas_changed(&atlas, last_size, created))
                && let Some(image) = AtlasImage::capture(&atlas)
            {
                atlas_size = Some((image.width(), image.height()));
                atlas_created = true;
                prepared = prepared.with_atlas(Some(image));
            }
            Ok::<_, ImGuiHostError>((prepared, atlas_size, atlas_created))
        });
        // Always leave the context suspended for the next window's turn,
        // even when the frame failed. The guard has already done it if the
        // closure panicked.
        turn.finish()?;
        let (prepared, atlas_size, atlas_created) = built?;
        self.atlas_size = atlas_size;
        self.atlas_created = atlas_created;
        self.frames += 1;
        Ok(prepared)
    }

    /// Prepares a frame and draws it in one step.
    ///
    /// The direct path, for tests and snapshots; a live window separates the
    /// two halves so the GPU paint never waits on script.
    pub fn render(
        &mut self,
        renderer: &mut WgpuRenderer,
        spec: FrameSpec,
        encoder: &mut wgpu::CommandEncoder,
        target: RenderTarget<'_>,
        build: impl FnOnce(&Ui),
    ) -> Result<FrameOutcome, ImGuiHostError> {
        let want_atlas = renderer.needs_atlas();
        let prepared = self.prepare(spec, want_atlas, build)?;
        let draw = prepared.stats();
        let render = renderer.render(encoder, target, &prepared)?;
        Ok(FrameOutcome { draw, render })
    }
}

/// One window's turn: the context, taken out of its slot and active.
///
/// Whatever ends the turn — a finished frame, an error, or a panic inside
/// the draw closure — the context goes back into the slot suspended. That is
/// not bookkeeping: Dear ImGui has one process-wide current context, so a
/// context left active on unwind makes every later window's `activate` fail
/// and freezes every plugin window until the app restarts.
struct Turn<'a> {
    slot: &'a mut Slot,
    context: Option<Context>,
}

impl<'a> Turn<'a> {
    /// Activates `window`'s context and takes it for this turn.
    fn take(window: &'a mut WindowContext) -> Result<Self, ImGuiHostError> {
        window.activate()?;
        let current = std::mem::replace(&mut window.slot, Slot::Empty);
        let Slot::Active(context) = current else {
            unreachable!("activate() leaves an active context in the slot");
        };
        Ok(Self {
            slot: &mut window.slot,
            context: Some(context),
        })
    }

    fn with_context<R>(&mut self, f: impl FnOnce(&mut Context) -> R) -> R {
        f(self.context.as_mut().expect("the turn's context"))
    }

    /// Suspends the context and returns it to the window's slot.
    fn finish(mut self) -> Result<(), ImGuiHostError> {
        self.suspend()
    }

    fn suspend(&mut self) -> Result<(), ImGuiHostError> {
        let Some(context) = self.context.take() else {
            return Ok(());
        };
        match context.suspend() {
            Ok(suspended) => {
                *self.slot = Slot::Suspended(suspended);
                Ok(())
            }
            Err(error) => {
                let reason = error.reason().to_string();
                *self.slot = Slot::Active(error.into_owner());
                Err(ImGuiHostError::context(reason))
            }
        }
    }
}

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        // On unwind the binding's frame token has already ended the frame, so
        // this normally succeeds. A failure keeps the context in the slot
        // rather than destroying it; panicking inside a drop would abort.
        let _ = self.suspend();
    }
}

/// Whether the context's atlas differs from the one last prepared.
///
/// The size changing is a rebuild; `WantUpdates` is a glyph or bake added
/// since the copy; `WantCreate` matters once, for the first copy. `OK` with
/// the same size means the renderer already has these pixels.
fn atlas_changed(
    atlas: &LegacyFontAtlas<'_>,
    last_size: Option<(u32, u32)>,
    created: bool,
) -> bool {
    let Some(texture) = atlas.tex_data() else {
        return false;
    };
    let size = (texture.width(), texture.height());
    if last_size != Some(size) {
        return true;
    }
    match texture.status() {
        // Destroyed/WantDestroy: the next copy rebuilds the texture with the
        // same bookkeeping; the renderer replaces it if the size differs.
        TextureStatus::WantUpdates | TextureStatus::Destroyed | TextureStatus::WantDestroy => true,
        TextureStatus::WantCreate => !created,
        TextureStatus::OK => false,
    }
}

impl fmt::Debug for WindowContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WindowContext")
            .field("label", &self.label)
            .field("id", &self.id())
            .field("active", &self.is_active())
            .field("frames", &self.frames)
            .finish_non_exhaustive()
    }
}
