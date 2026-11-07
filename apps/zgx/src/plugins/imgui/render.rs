//! One plugin window's GPU drawing.
//!
//! Dear ImGui hands a finished frame to a renderer as draw lists: vertices,
//! indices and clip rectangles. This module replays them with the wgpu zgx
//! already draws with, into whatever target a caller has — an offscreen
//! texture in tests and snapshots, a [`ViewportFrame`]'s target on a window.
//!
//! ## Why not `dear-imgui-wgpu`'s renderer
//!
//! Its managed path allocates vertex, index and uniform buffers for every
//! render epoch, and an epoch is one native frame: an animating plugin
//! window would make and free three GPU buffers sixty times a second. The
//! mission forbids per-frame GPU churn (`VAL-PERF-011`), so this renderer
//! replays `Context::render_legacy()`'s draw data and owns the upload
//! buffers itself; they only ever grow. `dear-imgui-wgpu` stays a declared
//! dependency because the ADR pins it and later milestones share its texture
//! types, but nothing here uses its managed renderer.
//!
//! ## What is kept between frames
//!
//! Two buffers (vertices and indices), one pipeline per target format, one
//! sampler, and the font-atlas texture with its bind group. Everything else
//! is written into them. [`ResourceCounters`] records every creation and
//! release, so a test — or `zgx inspect` later — can prove that a steady
//! workload creates nothing.

use std::borrow::Cow;
use std::cell::Cell;
use std::fmt;
use std::rc::Rc;

use dear_imgui_rs::render::{DrawCmd, DrawData};
use dear_imgui_rs::{LegacyFontAtlas, TextureFormat as AtlasFormat};
use zgx_gpu::viewport::{Viewport, ViewportFrame};
use zgx_gpu::wgpu;

/// Bytes of one Dear ImGui vertex: position (2 × f32), UV (2 × f32), colour
/// (4 × unorm8). Taken from `DrawVert`, whose layout the binding pins.
const VERTEX_BYTES: usize = 20;
/// Bytes of one index (`DrawIdx` is `u16`).
const INDEX_BYTES: usize = 2;
/// The first vertex buffer holds at least this much, so simple frames never
/// grow it a second time.
const MIN_VERTEX_BYTES: usize = 16 * 1024;
/// The same for indices.
const MIN_INDEX_BYTES: usize = 16 * 1024;

const SHADER: &str = include_str!("imgui.wgsl");

// ---------------------------------------------------------------------------------------------
// Counters
// ---------------------------------------------------------------------------------------------

/// GPU objects a [`WgpuRenderer`] has made and released, and how many are
/// live now.
///
/// "Destroyed" means the renderer let the object go; wgpu frees it once the
/// GPU has finished with it. The counters exist for the resource assertions
/// in `validation-contract.md` (`VAL-PERF-010`, `VAL-PERF-011`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResourceCounters {
    pub buffers_created: u64,
    pub buffers_destroyed: u64,
    pub textures_created: u64,
    pub textures_destroyed: u64,
    pub live_buffers: u32,
    pub live_textures: u32,
}

/// Creation/removal of one more buffer.
fn buffer_created(counters: &Cell<ResourceCounters>) {
    let mut value = counters.get();
    value.buffers_created += 1;
    value.live_buffers += 1;
    counters.set(value);
}

fn buffer_destroyed(counters: &Cell<ResourceCounters>) {
    let mut value = counters.get();
    value.buffers_destroyed += 1;
    value.live_buffers = value.live_buffers.saturating_sub(1);
    counters.set(value);
}

fn texture_created(counters: &Cell<ResourceCounters>) {
    let mut value = counters.get();
    value.textures_created += 1;
    value.live_textures += 1;
    counters.set(value);
}

fn texture_destroyed(counters: &Cell<ResourceCounters>) {
    let mut value = counters.get();
    value.textures_destroyed += 1;
    value.live_textures = value.live_textures.saturating_sub(1);
    counters.set(value);
}

/// A buffer the renderer owns, whose release the counters see.
struct TrackedBuffer {
    buffer: wgpu::Buffer,
    counters: Rc<Cell<ResourceCounters>>,
}

impl TrackedBuffer {
    fn new(
        device: &wgpu::Device,
        descriptor: wgpu::BufferDescriptor<'_>,
        counters: &Rc<Cell<ResourceCounters>>,
    ) -> Self {
        let buffer = device.create_buffer(&descriptor);
        buffer_created(counters);
        Self {
            buffer,
            counters: Rc::clone(counters),
        }
    }
}

impl Drop for TrackedBuffer {
    fn drop(&mut self) {
        buffer_destroyed(&self.counters);
    }
}

/// A texture the renderer owns, whose release the counters see.
struct TrackedTexture {
    texture: wgpu::Texture,
    counters: Rc<Cell<ResourceCounters>>,
}

impl TrackedTexture {
    fn new(
        device: &wgpu::Device,
        descriptor: wgpu::TextureDescriptor<'_>,
        counters: &Rc<Cell<ResourceCounters>>,
    ) -> Self {
        let texture = device.create_texture(&descriptor);
        texture_created(counters);
        Self {
            texture,
            counters: Rc::clone(counters),
        }
    }
}

impl Drop for TrackedTexture {
    fn drop(&mut self) {
        texture_destroyed(&self.counters);
    }
}

// ---------------------------------------------------------------------------------------------
// CPU-side frame
// ---------------------------------------------------------------------------------------------

/// The font atlas as the paint phase needs it: RGBA8 pixels, rows top to
/// bottom.
///
/// The atlas belongs to the ImGui context, which the paint phase must not
/// touch; a prepared frame carries a copy when the renderer has none (a new
/// renderer, a device rebuild) or the atlas changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AtlasImage {
    width: u32,
    height: u32,
    /// Dear ImGui's texture id, for diagnostics (the renderer binds the one
    /// atlas it owns).
    id: u64,
    pixels: Vec<u8>,
}

impl AtlasImage {
    /// Copies the context's current atlas. `None` before the atlas is built.
    ///
    /// `Alpha8` atlases are widened to RGBA8 so one texture format serves
    /// every atlas; the coverage ends up in the alpha channel either way.
    pub fn capture(atlas: &LegacyFontAtlas<'_>) -> Option<Self> {
        let texture = atlas.tex_data()?;
        let (width, height) = (texture.width(), texture.height());
        if width == 0 || height == 0 {
            return None;
        }
        let pixels = match texture.format() {
            AtlasFormat::RGBA32 => texture.pixels()?.to_vec(),
            AtlasFormat::Alpha8 => {
                let source = texture.pixels()?;
                let mut pixels = Vec::with_capacity(source.len() * 4);
                for alpha in source
                    .iter()
                    .copied()
                    .take((width as usize) * (height as usize))
                {
                    pixels.extend_from_slice(&[255, 255, 255, alpha]);
                }
                pixels
            }
        };
        Some(Self {
            width,
            height,
            id: atlas.texture_id().id(),
            pixels,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }
}

/// What one frame asked the renderer to do, for metrics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DrawStats {
    pub draw_lists: usize,
    pub commands: usize,
    pub vertices: usize,
    pub indices: usize,
    /// Commands dropped before drawing: an empty clip rectangle, or a raw
    /// callback this renderer does not serve. Images arrive with M3.
    pub skipped_commands: usize,
}

/// A finished Dear ImGui frame, copied off the context.
///
/// The paint phase (a `ViewportFrame`, a test's offscreen pass) never needs
/// the context or a live `DrawData`: positions are already in device pixels,
/// the indices are the ones Dear ImGui wrote, and the atlas is attached when
/// this renderer needs it.
#[derive(Clone, Debug, Default)]
pub struct PreparedFrame {
    /// The target size in device pixels this frame was captured for.
    ///
    /// Vertices and clip rectangles are in the coordinates of that target.
    /// The paint target can be a different size — the first paint after a
    /// window resize, or a snapshot at another size — so
    /// [`WgpuRenderer::render`] clips each scissor to the attachment it
    /// actually has instead of asking wgpu for a rectangle outside it.
    target_size: (u32, u32),
    vertices: Vec<u8>,
    indices: Vec<u16>,
    lists: Vec<PreparedList>,
    commands: Vec<PreparedCommand>,
    atlas: Option<AtlasImage>,
    stats: DrawStats,
}

#[derive(Clone, Copy, Debug)]
struct PreparedList {
    /// First vertex of this list, in the shared vertex buffer.
    vtx_offset: usize,
    /// First index of this list, in the shared index buffer.
    idx_offset: usize,
}

#[derive(Clone, Copy, Debug)]
struct PreparedCommand {
    list: usize,
    /// Relative to the list's own vertex buffer (Dear ImGui's `VtxOffset`).
    vtx_offset: usize,
    /// Relative to the list's own index buffer (Dear ImGui's `IdxOffset`).
    idx_offset: usize,
    count: usize,
    /// In device pixels: left, top, width, height.
    clip: [u32; 4],
}

impl PreparedFrame {
    /// Copies `draw_data` for replay at `target_size` device pixels.
    ///
    /// Vertex positions are shifted from Dear ImGui's display space and
    /// scaled, so drawing needs no uniform buffer: the renderer owns exactly
    /// the two upload buffers the contract allows.
    pub fn capture(draw_data: &DrawData, target_size: (u32, u32)) -> Self {
        let display_pos = draw_data.display_pos();
        let scale = draw_data.framebuffer_scale();
        let mut frame = Self {
            target_size,
            stats: DrawStats {
                draw_lists: draw_data.draw_lists_count(),
                ..DrawStats::default()
            },
            ..Self::default()
        };
        for list in draw_data.draw_lists() {
            let vtx_offset = frame.vertices.len() / VERTEX_BYTES;
            let idx_offset = frame.indices.len();
            let list_index = frame.lists.len();
            frame.lists.push(PreparedList {
                vtx_offset,
                idx_offset,
            });

            for vertex in list.vtx_buffer() {
                let position = [
                    (vertex.pos[0] - display_pos[0]) * scale[0],
                    (vertex.pos[1] - display_pos[1]) * scale[1],
                ];
                push_vertex(
                    &mut frame.vertices,
                    to_clip_space(position, target_size),
                    vertex.uv,
                    vertex.col,
                );
            }
            frame.indices.extend_from_slice(list.idx_buffer());

            for command in list.commands() {
                let DrawCmd::Elements { count, cmd_params } = command else {
                    // `SetSampler*` and `ResetRenderState` say nothing about
                    // what to draw; a raw callback needs a native draw list
                    // this process does not hand out.
                    if matches!(command, DrawCmd::RawCallback(_)) {
                        frame.stats.skipped_commands += 1;
                    }
                    continue;
                };
                if count == 0 {
                    continue;
                }
                let clip = clip_to_pixels(cmd_params.clip_rect, display_pos, scale, target_size);
                if clip[2] == 0 || clip[3] == 0 {
                    frame.stats.skipped_commands += 1;
                    continue;
                }
                frame.commands.push(PreparedCommand {
                    list: list_index,
                    vtx_offset: cmd_params.vtx_offset,
                    idx_offset: cmd_params.idx_offset,
                    count,
                    clip,
                });
                frame.stats.commands += 1;
            }

            frame.stats.vertices += list.vtx_buffer().len();
            frame.stats.indices += list.idx_buffer().len();
        }
        frame
    }

    /// Attaches the atlas the renderer should upload before drawing.
    pub fn with_atlas(mut self, atlas: Option<AtlasImage>) -> Self {
        self.atlas = atlas;
        self
    }

    pub fn atlas(&self) -> Option<&AtlasImage> {
        self.atlas.as_ref()
    }

    pub fn stats(&self) -> DrawStats {
        self.stats
    }

    /// The target size in device pixels this frame was captured for (see the
    /// field).
    pub fn target_size(&self) -> (u32, u32) {
        self.target_size
    }

    pub fn vertex_bytes(&self) -> usize {
        self.vertices.len()
    }

    pub fn index_bytes(&self) -> usize {
        self.indices.len() * INDEX_BYTES
    }

    pub fn commands(&self) -> usize {
        self.commands.len()
    }
}

/// Pixel coordinates (origin top-left, y down) to clip space, so the vertex
/// shader needs no viewport uniform: the frame's vertices already are what
/// the viewport shows.
fn to_clip_space(pixel: [f32; 2], target_size: (u32, u32)) -> [f32; 2] {
    let width = target_size.0.max(1) as f32;
    let height = target_size.1.max(1) as f32;
    [pixel[0] * 2.0 / width - 1.0, 1.0 - pixel[1] * 2.0 / height]
}

fn push_vertex(vertices: &mut Vec<u8>, position: [f32; 2], uv: [f32; 2], color: u32) {
    vertices.extend_from_slice(&position[0].to_ne_bytes());
    vertices.extend_from_slice(&position[1].to_ne_bytes());
    vertices.extend_from_slice(&uv[0].to_ne_bytes());
    vertices.extend_from_slice(&uv[1].to_ne_bytes());
    vertices.extend_from_slice(&color.to_ne_bytes());
}

fn clip_to_pixels(
    clip: [f32; 4],
    display_pos: [f32; 2],
    scale: [f32; 2],
    target_size: (u32, u32),
) -> [u32; 4] {
    let left = ((clip[0] - display_pos[0]) * scale[0]).floor().max(0.0);
    let top = ((clip[1] - display_pos[1]) * scale[1]).floor().max(0.0);
    let right = ((clip[2] - display_pos[0]) * scale[0])
        .ceil()
        .min(target_size.0 as f32);
    let bottom = ((clip[3] - display_pos[1]) * scale[1])
        .ceil()
        .min(target_size.1 as f32);
    if right <= left || bottom <= top {
        return [0, 0, 0, 0];
    }
    [
        left as u32,
        top as u32,
        (right - left) as u32,
        (bottom - top) as u32,
    ]
}

// ---------------------------------------------------------------------------------------------
// Target and errors
// ---------------------------------------------------------------------------------------------

/// What a plugin window shows where no ImGui window covers it. Opaque:
/// zgx composites premultiplied alpha and a plugin window is not a
/// transparent one.
pub const DEFAULT_CLEAR: wgpu::Color = wgpu::Color {
    r: 0.10,
    g: 0.11,
    b: 0.13,
    a: 1.0,
};

/// Where a frame is drawn: a texture view, its format and size in device
/// pixels, and what to clear it to first.
#[derive(Clone, Copy)]
pub struct RenderTarget<'a> {
    pub view: &'a wgpu::TextureView,
    pub format: wgpu::TextureFormat,
    pub size: (u32, u32),
    pub clear: wgpu::Color,
}

impl<'a> RenderTarget<'a> {
    pub fn new(view: &'a wgpu::TextureView, format: wgpu::TextureFormat, size: (u32, u32)) -> Self {
        Self {
            view,
            format,
            size,
            clear: DEFAULT_CLEAR,
        }
    }

    pub fn with_clear(mut self, clear: wgpu::Color) -> Self {
        self.clear = clear;
        self
    }
}

/// Begins the frame's colour pass: `target` cleared, premultiplied alpha.
///
/// A free function, so the pass borrows the encoder (and nothing of the
/// renderer) for as long as it records.
fn begin_pass<'a>(
    encoder: &'a mut wgpu::CommandEncoder,
    target: &RenderTarget<'_>,
) -> wgpu::RenderPass<'a> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("imgui frame"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target.view,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(target.clear),
                store: wgpu::StoreOp::Store,
            },
            depth_slice: None,
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

/// Why a prepared frame could not be drawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenderError {
    /// The frame has commands, but no font atlas reached this renderer (a
    /// device rebuild before the next prepared frame).
    MissingAtlas,
}

impl fmt::Display for RenderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingAtlas => formatter.write_str(
                "the ImGui frame needs its font atlas, and this device has not uploaded one",
            ),
        }
    }
}

impl std::error::Error for RenderError {}

/// What one draw recorded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderStats {
    pub draw_calls: u32,
    pub vertices: u32,
    pub indices: u32,
    pub target: (u32, u32),
}

// ---------------------------------------------------------------------------------------------
// Renderer
// ---------------------------------------------------------------------------------------------

/// The GPU objects of one plugin window's renderer.
struct AtlasTexture {
    /// Dropped last: the bind group borrows the view.
    texture: TrackedTexture,
    view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
    size: (u32, u32),
    id: u64,
}

/// Draws prepared Dear ImGui frames with wgpu.
///
/// One renderer serves one plugin window (and its one context). It owns its
/// GPU objects; `device_changed` drops all of them, and the next frame
/// rebuilds them from the device that frame brings.
pub struct WgpuRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    format: wgpu::TextureFormat,
    // Made with the first atlas and remade after a device change: they
    // belong to a device as much as the buffers do.
    bind_group_layout: Option<wgpu::BindGroupLayout>,
    sampler: Option<wgpu::Sampler>,
    pipeline: Option<wgpu::RenderPipeline>,
    pipeline_format: Option<wgpu::TextureFormat>,
    atlas: Option<AtlasTexture>,
    vertex: Option<(TrackedBuffer, usize)>,
    index: Option<(TrackedBuffer, usize)>,
    counters: Rc<Cell<ResourceCounters>>,
    frames: u64,
}

impl WgpuRenderer {
    /// A renderer for `format` targets on `device`.
    ///
    /// It creates no GPU resource until the first [`Self::render`], so a
    /// hidden window costs nothing.
    pub fn new(device: wgpu::Device, queue: wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        Self {
            device,
            queue,
            format,
            bind_group_layout: None,
            sampler: None,
            pipeline: None,
            pipeline_format: None,
            atlas: None,
            vertex: None,
            index: None,
            counters: Rc::new(Cell::new(ResourceCounters::default())),
            frames: 0,
        }
    }

    /// Whether the next frame must carry the font atlas (first frame, or
    /// after [`Self::device_changed`]).
    pub fn needs_atlas(&self) -> bool {
        self.atlas.is_none()
    }

    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    pub fn counters(&self) -> ResourceCounters {
        self.counters.get()
    }

    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Drops every GPU object this renderer made. The next [`Self::render`]
    /// rebuilds them from the device it is given.
    pub fn device_changed(&mut self) {
        self.pipeline = None;
        self.pipeline_format = None;
        self.atlas = None;
        self.bind_group_layout = None;
        self.sampler = None;
        self.vertex = None;
        self.index = None;
    }

    /// The atlas bindings, made once per device.
    fn ensure_atlas_bindings(&mut self) {
        if self.bind_group_layout.is_some() && self.sampler.is_some() {
            return;
        }
        let layout = self
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("imgui atlas layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });
        let sampler = self.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("imgui atlas sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..wgpu::SamplerDescriptor::default()
        });
        self.bind_group_layout = Some(layout);
        self.sampler = Some(sampler);
    }

    /// Points the renderer at a new device (or the same one after its loss)
    /// and drops what was made with the old one.
    pub fn rebind(&mut self, device: wgpu::Device, queue: wgpu::Queue) {
        self.device_changed();
        self.device = device;
        self.queue = queue;
    }

    /// Clears `target` and draws nothing else.
    pub fn clear(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        target: RenderTarget<'_>,
    ) -> RenderStats {
        let _pass = begin_pass(encoder, &target);
        RenderStats {
            target: target.size,
            ..RenderStats::default()
        }
    }

    /// Draws one prepared frame into `target`.
    pub fn render(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        target: RenderTarget<'_>,
        frame: &PreparedFrame,
    ) -> Result<RenderStats, RenderError> {
        if target.format != self.format {
            self.format = target.format;
            self.pipeline = None;
            self.pipeline_format = None;
        }
        if let Some(atlas) = &frame.atlas {
            self.upload_atlas(atlas);
        }
        if !frame.commands.is_empty() && self.atlas.is_none() {
            return Err(RenderError::MissingAtlas);
        }
        self.upload_vertex(frame);
        self.upload_index(frame);
        self.ensure_pipeline();

        self.frames += 1;
        let mut stats = RenderStats {
            target: target.size,
            vertices: frame.stats.vertices as u32,
            indices: frame.stats.indices as u32,
            ..RenderStats::default()
        };

        let mut pass = begin_pass(encoder, &target);
        if frame.commands.is_empty() {
            return Ok(stats);
        }
        let Some(pipeline) = self.pipeline.as_ref() else {
            return Ok(stats);
        };
        let Some(atlas) = self.atlas.as_ref() else {
            return Ok(stats);
        };
        let Some((vertex, _)) = self.vertex.as_ref() else {
            return Ok(stats);
        };
        let Some((index, _)) = self.index.as_ref() else {
            return Ok(stats);
        };
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &atlas.bind_group, &[]);
        for command in &frame.commands {
            let list = frame.lists[command.list];
            let vertex_offset = (list.vtx_offset + command.vtx_offset) * VERTEX_BYTES;
            let index_offset = (list.idx_offset + command.idx_offset) * INDEX_BYTES;
            pass.set_vertex_buffer(0, vertex.buffer.slice(vertex_offset as u64..));
            pass.set_index_buffer(
                index.buffer.slice(index_offset as u64..),
                wgpu::IndexFormat::Uint16,
            );
            // The frame was captured for `frame.target_size`, and the
            // attachment can be smaller — the first paint after a resize,
            // say. wgpu rejects a scissor that leaves the attachment, so
            // clip it to what is actually there; a command entirely outside
            // draws nothing.
            let [x, y, width, height] = command.clip;
            let width = width.min(target.size.0.saturating_sub(x));
            let height = height.min(target.size.1.saturating_sub(y));
            if width == 0 || height == 0 {
                continue;
            }
            pass.set_scissor_rect(x, y, width, height);
            pass.draw_indexed(0..command.count as u32, 0, 0..1);
            stats.draw_calls += 1;
        }
        Ok(stats)
    }

    fn upload_vertex(&mut self, frame: &PreparedFrame) {
        let required = frame.vertices.len();
        // Every frame's bytes go into the buffer: reusing the allocation is
        // what keeps the churn counters flat, but skipping the write would
        // replay frame 1 for every later frame. A buffer is made only when
        // the frame does not fit the one already there.
        let capacity = match self.vertex.as_ref() {
            Some((_, capacity)) if *capacity >= required => *capacity,
            Some((_, capacity)) => (*capacity * 2).max(required.next_power_of_two()),
            None => required.next_power_of_two().max(MIN_VERTEX_BYTES),
        };
        if self.vertex.as_ref().map(|(_, capacity)| *capacity) != Some(capacity) {
            let buffer = TrackedBuffer::new(
                &self.device,
                wgpu::BufferDescriptor {
                    label: Some("imgui vertices"),
                    size: capacity as u64,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                },
                &self.counters,
            );
            self.vertex = Some((buffer, capacity));
        }
        if required > 0 {
            self.queue.write_buffer(
                &self.vertex.as_ref().expect("the buffer is there").0.buffer,
                0,
                &frame.vertices,
            );
        }
    }

    fn upload_index(&mut self, frame: &PreparedFrame) {
        let required = frame.index_bytes();
        // The same invariant as `upload_vertex`: reuse the buffer, but write
        // this frame's indices into it every time.
        let capacity = match self.index.as_ref() {
            Some((_, capacity)) if *capacity >= required => *capacity,
            Some((_, capacity)) => (*capacity * 2).max(required.next_power_of_two()),
            None => required.next_power_of_two().max(MIN_INDEX_BYTES),
        };
        if self.index.as_ref().map(|(_, capacity)| *capacity) != Some(capacity) {
            let buffer = TrackedBuffer::new(
                &self.device,
                wgpu::BufferDescriptor {
                    label: Some("imgui indices"),
                    size: capacity as u64,
                    usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                },
                &self.counters,
            );
            self.index = Some((buffer, capacity));
        }
        if required > 0 {
            // `queue.write_buffer` copies in 4-byte units; an odd index count
            // is padded, and the extra index is never drawn.
            let padded = required.next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT as usize);
            let mut bytes = Vec::with_capacity(padded);
            for index in &frame.indices {
                bytes.extend_from_slice(&index.to_ne_bytes());
            }
            bytes.resize(padded, 0);
            self.queue.write_buffer(
                &self.index.as_ref().expect("the buffer is there").0.buffer,
                0,
                &bytes,
            );
        }
    }

    fn upload_atlas(&mut self, image: &AtlasImage) {
        let size = (image.width(), image.height());
        if let Some(atlas) = self.atlas.as_mut()
            && atlas.size == size
        {
            write_rgba(&self.queue, &atlas.texture.texture, image.pixels(), size);
            atlas.id = image.id();
            return;
        }
        let texture = TrackedTexture::new(
            &self.device,
            wgpu::TextureDescriptor {
                label: Some("imgui font atlas"),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            &self.counters,
        );
        write_rgba(&self.queue, &texture.texture, image.pixels(), size);
        self.ensure_atlas_bindings();
        let layout = self
            .bind_group_layout
            .as_ref()
            .expect("the atlas layout was just made")
            .clone();
        let sampler = self
            .sampler
            .as_ref()
            .expect("the atlas sampler was just made")
            .clone();
        let view = texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("imgui atlas binding"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        self.atlas = Some(AtlasTexture {
            texture,
            view,
            bind_group,
            size,
            id: image.id(),
        });
    }

    fn ensure_pipeline(&mut self) {
        if self.pipeline.is_some() && self.pipeline_format == Some(self.format) {
            return;
        }
        self.ensure_atlas_bindings();
        let module = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("imgui shader"),
                source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(SHADER)),
            });
        let atlas_layout = self
            .bind_group_layout
            .as_ref()
            .expect("the atlas layout was just made")
            .clone();
        let layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("imgui pipeline layout"),
                bind_group_layouts: &[Some(&atlas_layout)],
                immediate_size: 0,
            });
        // Dear ImGui's colours are sRGB. A target whose format is sRGB
        // encodes what the shader writes, so that entry point decodes first
        // and the pixels come out as authored; a unorm target takes them as
        // they are.
        let entry_point = if self.format.is_srgb() {
            "fs_srgb"
        } else {
            "fs_main"
        };
        let pipeline = self
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("imgui pipeline"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: VERTEX_BYTES as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &[
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x2,
                                offset: 0,
                                shader_location: 0,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x2,
                                offset: 8,
                                shader_location: 1,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Unorm8x4,
                                offset: 16,
                                shader_location: 2,
                            },
                        ],
                    }],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(entry_point),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: self.format,
                        // Premultiplied source, as zgx's compositor expects.
                        blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    ..wgpu::PrimitiveState::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            });
        self.pipeline = Some(pipeline);
        self.pipeline_format = Some(self.format);
    }
}

fn write_rgba(queue: &wgpu::Queue, texture: &wgpu::Texture, pixels: &[u8], size: (u32, u32)) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(size.0 * 4),
            rows_per_image: Some(size.1),
        },
        wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
    );
}

// ---------------------------------------------------------------------------------------------
// The window's view
// ---------------------------------------------------------------------------------------------

/// A plugin window's content box: it draws the last prepared frame.
///
/// This is the paint half of the frame pump. The update half (a
/// [`crate::plugins::imgui::WindowContext`]) prepares a frame outside of
/// any `ViewportFrame`; this view replays it whenever zgx draws the window,
/// without touching the ImGui context or running any script.
pub struct ImguiViewport {
    renderer: Option<WgpuRenderer>,
    prepared: Option<PreparedFrame>,
    clear: wgpu::Color,
    error: Option<RenderError>,
    /// The target size of the last frame painted, in device pixels. The
    /// update phase builds the next frame for it, so content lands on the
    /// window's pixels even at a display scale the app did not predict.
    target: Option<(u32, u32)>,
    /// Frames this view has drawn. zgx calls `render` only while the window
    /// showing it does (a hidden or minimized window draws none), so this is
    /// the window's own render lifecycle, and a hidden window's count stays
    /// where it was.
    frames: u64,
}

impl ImguiViewport {
    pub fn new() -> Self {
        Self {
            renderer: None,
            prepared: None,
            clear: DEFAULT_CLEAR,
            error: None,
            target: None,
            frames: 0,
        }
    }

    /// The size in device pixels of the last painted frame, if any.
    pub fn last_target_size(&self) -> Option<(u32, u32)> {
        self.target
    }

    /// Frames drawn so far (see the field).
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Replaces what the next frame draws.
    pub fn set_prepared(&mut self, frame: PreparedFrame) {
        self.prepared = Some(frame);
    }

    pub fn prepared(&self) -> Option<&PreparedFrame> {
        self.prepared.as_ref()
    }

    pub fn renderer(&self) -> Option<&WgpuRenderer> {
        self.renderer.as_ref()
    }

    pub fn renderer_mut(&mut self) -> Option<&mut WgpuRenderer> {
        self.renderer.as_mut()
    }

    /// The last reason a frame could not be drawn, if any.
    pub fn take_error(&mut self) -> Option<RenderError> {
        self.error.take()
    }

    pub fn with_clear(mut self, clear: wgpu::Color) -> Self {
        self.clear = clear;
        self
    }
}

impl Default for ImguiViewport {
    fn default() -> Self {
        Self::new()
    }
}

impl Viewport for ImguiViewport {
    fn render(&mut self, frame: &mut ViewportFrame<'_>) {
        self.frames += 1;
        self.target = Some(frame.size);
        let target = RenderTarget {
            view: frame.target,
            format: frame.format,
            size: frame.size,
            clear: self.clear,
        };
        let renderer = self.renderer.get_or_insert_with(|| {
            WgpuRenderer::new(frame.device.clone(), frame.queue.clone(), frame.format)
        });
        let Some(prepared) = self.prepared.as_ref() else {
            renderer.clear(frame.encoder, target);
            return;
        };
        if let Err(error) = renderer.render(frame.encoder, target, prepared) {
            // Without an atlas (a device rebuild) the window keeps its
            // background until the next update prepares a frame again.
            renderer.clear(frame.encoder, target);
            self.error = Some(error);
        }
    }

    /// Every GPU object goes with the device; the next `render` makes new
    /// ones with the frame it is given.
    fn device_changed(&mut self) {
        self.renderer = None;
    }
}
