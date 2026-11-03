//! `ui.shader` on the GPU: buffer passes with feedback, the image pass, then
//! optional bloom and compositing by blend mode, read back as an image.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use wgpu::util::DeviceExt;
use zeron_visual::shader::{Blend, MAX_BUFFERS, ShaderUniforms};

use crate::{EDGE_FADE, FORMAT, Frame, gpu};

/// Passes render in float, so feedback buffers keep precision and values
/// above 1 survive to the bloom.
const HDR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const NOISE_SIZE: u32 = 256;

/// What to run: the image pass, its buffers, and how it composites.
pub struct ShaderSpec<'a> {
    pub image: &'a str,
    pub buffers: &'a [String],
    /// Named float inputs, in slot order.
    pub inputs: &'a [String],
    pub blend: Blend,
    pub bloom: f32,
    /// Images sampled as `iTexture0`-`3`.
    pub textures: &'a [zeron_visual::TextureData],
}

/// One shader's textures between frames: each buffer's previous and
/// current frame, the image, and the post targets. Recreated when the size
/// or the number of buffers changes.
pub struct ShaderState {
    size: (u32, u32),
    buffers: Vec<[wgpu::Texture; 2]>,
    /// Which of each pair holds the latest frame.
    latest: usize,
    image: wgpu::Texture,
    bloom: [wgpu::Texture; 2],
    output: wgpu::Texture,
    staging: Option<wgpu::Buffer>,
}

pub(crate) struct Pipelines {
    layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    fullscreen: wgpu::ShaderModule,
    post_layout: wgpu::BindGroupLayout,
    bright: wgpu::RenderPipeline,
    blur: wgpu::RenderPipeline,
    composite: wgpu::RenderPipeline,
    clamp: wgpu::Sampler,
    repeat: wgpu::Sampler,
    noise: wgpu::Texture,
    empty: wgpu::Texture,
    passes: Mutex<HashMap<String, Result<Arc<wgpu::RenderPipeline>, String>>>,
}

const FULLSCREEN: &str =
    "@vertex fn main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}";

const POST: &str = r#"
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;
// Bright/blur: xy is one texel along the blur. Composite: x bloom strength,
// y blend mode (0 alpha, 1 light, 2 opaque).
@group(0) @binding(2) var<uniform> post: vec4<f32>;
@group(0) @binding(3) var glow: texture_2d<f32>;

struct Out { @builtin(position) clip: vec4<f32>, @location(0) uv: vec2<f32> };

@vertex fn vs(@builtin(vertex_index) i: u32) -> Out {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var o: Out;
    o.clip = vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
    o.uv = vec2<f32>(p.x, 1.0 - p.y);
    return o;
}

fn luma(c: vec3<f32>) -> f32 { return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722)); }

@fragment fn fs_bright(i: Out) -> @location(0) vec4<f32> {
    let c = textureSampleLevel(source, source_sampler, i.uv, 0.0).rgb;
    // Soft knee from 0.6, so bright-but-not-clipped light still glows.
    let l = luma(c);
    let over = smoothstep(0.6, 1.2, l);
    return vec4<f32>(c * over, 1.0);
}

@fragment fn fs_blur(i: Out) -> @location(0) vec4<f32> {
    let w = array<f32, 5>(0.227, 0.194, 0.121, 0.054, 0.016);
    var sum = textureSampleLevel(source, source_sampler, i.uv, 0.0).rgb * w[0];
    for (var k = 1; k < 5; k++) {
        let o = post.xy * f32(k) * 1.6;
        sum += textureSampleLevel(source, source_sampler, i.uv + o, 0.0).rgb * w[k];
        sum += textureSampleLevel(source, source_sampler, i.uv - o, 0.0).rgb * w[k];
    }
    return vec4<f32>(sum, 1.0);
}

@fragment fn fs_composite(i: Out) -> @location(0) vec4<f32> {
    // Passes store rows bottom-up (Shadertoy's origin); turn them upright.
    let uv = vec2<f32>(i.uv.x, 1.0 - i.uv.y);
    let c = textureSampleLevel(source, source_sampler, uv, 0.0);
    let g = textureSampleLevel(glow, source_sampler, uv, 0.0).rgb * post.x;
    let rgb = clamp(c.rgb + g, vec3<f32>(0.0), vec3<f32>(1.0));
    // An opaque shader is a deliberate box; the others fade at the edges.
    if (post.y > 1.5) {
        return vec4<f32>(rgb, 1.0);
    }
    let fade = edge_fade(i.clip.xy, vec2<f32>(textureDimensions(source)));
    if (post.y > 0.5) {
        let a = max(rgb.r, max(rgb.g, rgb.b));
        if (a <= 0.0001) { return vec4<f32>(0.0); }
        return vec4<f32>(rgb / a, a * fade);
    }
    // The shader's color is straight over its alpha; the glow is light over
    // nothing. Add them premultiplied, then return straight alpha.
    let a = clamp(c.a + luma(g), 0.0, 1.0);
    if (a <= 0.0001) { return vec4<f32>(0.0); }
    let straight = clamp((clamp(c.rgb, vec3<f32>(0.0), vec3<f32>(1.0)) * c.a + g) / a, vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(straight, a * fade);
}
"#;

impl Pipelines {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let texture_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let sampler_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let uniform_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shader inputs"),
            entries: &[
                uniform_entry(0),
                texture_entry(1),
                texture_entry(2),
                texture_entry(3),
                texture_entry(4),
                texture_entry(5),
                sampler_entry(6),
                sampler_entry(7),
                texture_entry(8),
                texture_entry(9),
                texture_entry(10),
                texture_entry(11),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shader"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let fullscreen = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fullscreen"),
            source: wgpu::ShaderSource::Wgsl(FULLSCREEN.into()),
        });
        let post_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shader post"),
            entries: &[
                texture_entry(0),
                sampler_entry(1),
                uniform_entry(2),
                texture_entry(3),
            ],
        });
        let post_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shader post"),
            bind_group_layouts: &[Some(&post_layout)],
            immediate_size: 0,
        });
        let post = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shader post"),
            source: wgpu::ShaderSource::Wgsl(format!("{EDGE_FADE}{POST}").into()),
        });
        let post_pipeline = |label: &str, entry: &str, format| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&post_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &post,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &post,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let sampler = |address| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("shader channel"),
                address_mode_u: address,
                address_mode_v: address,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                // Image textures have mips; channels and noise have one level.
                mipmap_filter: wgpu::MipmapFilterMode::Linear,
                ..Default::default()
            })
        };
        // Independent random RGBA per texel, fixed so a shader looks the
        // same on every device.
        let mut state = 0x2545_f491_u32;
        let noise_bytes: Vec<u8> = (0..NOISE_SIZE * NOISE_SIZE * 4)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                (state >> 24) as u8
            })
            .collect();
        let noise = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("shader noise"),
                size: wgpu::Extent3d {
                    width: NOISE_SIZE,
                    height: NOISE_SIZE,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &noise_bytes,
        );
        let empty = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("shader empty channel"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &[0, 0, 0, 0],
        );
        Self {
            bright: post_pipeline("shader bloom bright", "fs_bright", HDR),
            blur: post_pipeline("shader bloom blur", "fs_blur", HDR),
            composite: post_pipeline("shader composite", "fs_composite", FORMAT),
            layout,
            pipeline_layout,
            fullscreen,
            post_layout,
            clamp: sampler(wgpu::AddressMode::ClampToEdge),
            repeat: sampler(wgpu::AddressMode::Repeat),
            noise,
            empty,
            passes: Mutex::new(HashMap::new()),
        }
    }

    /// The pipeline for one complete fragment program, compiled once.
    fn pass(
        &self,
        device: &wgpu::Device,
        program: &str,
    ) -> Result<Arc<wgpu::RenderPipeline>, String> {
        let mut passes = self.passes.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(cached) = passes.get(program) {
            return cached.clone();
        }
        let compiled = crate::guarded(device, || {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("visual shader"),
                source: wgpu::ShaderSource::Glsl {
                    shader: program.into(),
                    stage: wgpu::naga::ShaderStage::Fragment,
                    defines: &[],
                },
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("visual shader"),
                layout: Some(&self.pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &self.fullscreen,
                    entry_point: Some("main"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: HDR,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        });
        let result = compiled
            .map(Arc::new)
            .map_err(|error| format!("shader: {error}"));
        // Keep the cache bounded; a session rarely holds more than a few.
        if passes.len() > 64 {
            passes.clear();
        }
        passes.insert(program.to_owned(), result.clone());
        result
    }
}

fn texture(
    device: &wgpu::Device,
    label: &str,
    size: (u32, u32),
    format: wgpu::TextureFormat,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

impl ShaderState {
    fn new(device: &wgpu::Device, size: (u32, u32), buffers: usize) -> Self {
        let half = ((size.0 / 2).max(1), (size.1 / 2).max(1));
        Self {
            size,
            buffers: (0..buffers)
                .map(|_| {
                    [
                        texture(device, "shader buffer", size, HDR),
                        texture(device, "shader buffer", size, HDR),
                    ]
                })
                .collect(),
            latest: 0,
            image: texture(device, "shader image", size, HDR),
            bloom: [
                texture(device, "shader bloom", half, HDR),
                texture(device, "shader bloom", half, HDR),
            ],
            output: texture(device, "shader output", size, FORMAT),
            staging: None,
        }
    }
}

/// Run one frame of a `ui.shader` at `width` by `height` device pixels.
/// `state` carries the buffers between frames; pass the same one each frame.
pub fn render_shader(
    spec: &ShaderSpec,
    uniforms: &ShaderUniforms,
    width: u32,
    height: u32,
    state: &mut Option<ShaderState>,
) -> Result<Frame, String> {
    let gpu = gpu().ok_or_else(|| {
        format!(
            "this device has no GPU for shaders ({})",
            crate::unavailable_reason().unwrap_or_default()
        )
    })?;
    let width = width.clamp(1, 8192);
    let height = height.clamp(1, 8192);
    let buffers = spec.buffers.len().min(MAX_BUFFERS);
    let device = &gpu.device;
    let pipelines = gpu
        .shaders
        .get_or_init(|| Pipelines::new(device, &gpu.queue));
    // Compile every pass first, so an error leaves the state untouched.
    let mut programs = Vec::with_capacity(buffers + 1);
    for code in spec
        .buffers
        .iter()
        .take(buffers)
        .map(String::as_str)
        .chain([spec.image])
    {
        let (program, _) = zeron_visual::shader::program(code, spec.inputs);
        programs.push(pipelines.pass(device, &program)?);
    }
    let fresh = state
        .as_ref()
        .is_none_or(|s| s.size != (width, height) || s.buffers.len() != buffers);
    if fresh {
        *state = Some(ShaderState::new(device, (width, height), buffers));
    }
    let state = state.as_mut().expect("just made");
    let (read, write) = (state.latest, 1 - state.latest);

    let mut uniforms = *uniforms;
    for channel in &mut uniforms.channels {
        *channel = [width as f32, height as f32, 1.0, 0.0];
    }
    let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("shader inputs"),
        contents: uniforms.as_bytes(),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let view = |t: &wgpu::Texture| t.create_view(&Default::default());
    let empty = view(&pipelines.empty);
    let noise = view(&pipelines.noise);
    let images: Vec<wgpu::TextureView> = (0..zeron_visual::shader::MAX_TEXTURES)
        .map(|i| {
            spec.textures
                .get(i)
                .and_then(|data| {
                    gpu.textures
                        .get(device, &gpu.queue, data, crate::textures::Space::Raw)
                        .ok()
                })
                .map_or_else(|| empty.clone(), |loaded| loaded.view)
        })
        .collect();
    // Channel `j` as pass `pass` sees it: an earlier buffer's frame from
    // this frame, its own and later ones' from the last.
    let channel = |pass: usize, j: usize| -> wgpu::TextureView {
        if j >= buffers {
            return empty.clone();
        }
        let frame = if j < pass { write } else { read };
        view(&state.buffers[j][frame])
    };
    let mut encoder = device.create_command_encoder(&Default::default());
    for (pass, pipeline) in programs.iter().enumerate() {
        let channels: Vec<wgpu::TextureView> = (0..4).map(|j| channel(pass, j)).collect();
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shader inputs"),
            layout: &pipelines.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&channels[0]),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&channels[1]),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&channels[2]),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&channels[3]),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(&noise),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::Sampler(&pipelines.clamp),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::Sampler(&pipelines.repeat),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: wgpu::BindingResource::TextureView(&images[0]),
                },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: wgpu::BindingResource::TextureView(&images[1]),
                },
                wgpu::BindGroupEntry {
                    binding: 10,
                    resource: wgpu::BindingResource::TextureView(&images[2]),
                },
                wgpu::BindGroupEntry {
                    binding: 11,
                    resource: wgpu::BindingResource::TextureView(&images[3]),
                },
            ],
        });
        let target = if pass < buffers {
            view(&state.buffers[pass][write])
        } else {
            view(&state.image)
        };
        let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("shader pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        rp.set_pipeline(pipeline);
        rp.set_bind_group(0, &bind, &[]);
        rp.draw(0..3, 0..1);
    }

    // Post: bloom, then composite by blend mode.
    let post_uniform = |values: [f32; 4]| {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("shader post"),
            contents: uniform_bytes(&values),
            usage: wgpu::BufferUsages::UNIFORM,
        })
    };
    let post_bind = |source: &wgpu::Texture, values: &wgpu::Buffer, glow: &wgpu::TextureView| {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shader post"),
            layout: &pipelines.post_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view(source)),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&pipelines.clamp),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: values.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(glow),
                },
            ],
        })
    };
    let fullscreen = |encoder: &mut wgpu::CommandEncoder,
                      target: &wgpu::Texture,
                      pipeline: &wgpu::RenderPipeline,
                      bind: &wgpu::BindGroup| {
        let target = view(target);
        let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("shader post"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        rp.set_pipeline(pipeline);
        rp.set_bind_group(0, bind, &[]);
        rp.draw(0..3, 0..1);
    };
    let glow = if spec.bloom > 0.0 {
        let half = state.bloom[0].size();
        let zero = post_uniform([0.0; 4]);
        let step_x = post_uniform([1.0 / half.width as f32, 0.0, 0.0, 0.0]);
        let step_y = post_uniform([0.0, 1.0 / half.height as f32, 0.0, 0.0]);
        fullscreen(
            &mut encoder,
            &state.bloom[0],
            &pipelines.bright,
            &post_bind(&state.image, &zero, &empty),
        );
        for _ in 0..3 {
            fullscreen(
                &mut encoder,
                &state.bloom[1],
                &pipelines.blur,
                &post_bind(&state.bloom[0], &step_x, &empty),
            );
            fullscreen(
                &mut encoder,
                &state.bloom[0],
                &pipelines.blur,
                &post_bind(&state.bloom[1], &step_y, &empty),
            );
        }
        view(&state.bloom[0])
    } else {
        empty.clone()
    };
    let mode = match spec.blend {
        Blend::Alpha => 0.0,
        Blend::Light => 1.0,
        Blend::Opaque => 2.0,
    };
    let composite = post_uniform([spec.bloom, mode, 0.0, 0.0]);
    fullscreen(
        &mut encoder,
        &state.output,
        &pipelines.composite,
        &post_bind(&state.image, &composite, &glow),
    );
    let frame = gpu.read_back(encoder, &state.output, &mut state.staging);
    state.latest = write;
    Ok(frame)
}

fn uniform_bytes(values: &[f32; 4]) -> &[u8] {
    // SAFETY: four `f32`s.
    unsafe { std::slice::from_raw_parts(values.as_ptr().cast::<u8>(), 16) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec<'a>(image: &'a str, buffers: &'a [String], blend: Blend) -> ShaderSpec<'a> {
        ShaderSpec {
            image,
            buffers,
            inputs: &[],
            blend,
            bloom: 0.0,
            textures: &[],
        }
    }

    fn px(frame: &Frame, x: u32, y: u32) -> [u8; 4] {
        let o = ((y * frame.width + x) * 4) as usize;
        [
            frame.bgra[o + 2],
            frame.bgra[o + 1],
            frame.bgra[o],
            frame.bgra[o + 3],
        ]
    }

    #[test]
    fn a_shader_renders_its_gradient_upright_with_time() {
        if !crate::available() {
            return;
        }
        let code = "void mainImage(out vec4 fragColor, in vec2 fragCoord) {\n\
            vec2 uv = fragCoord / iResolution.xy;\n\
            fragColor = vec4(uv.x, uv.y, fract(iTime), 1.0);\n\
        }";
        let mut uniforms = ShaderUniforms {
            resolution: [64.0, 32.0, 1.0, 0.0],
            ..Default::default()
        };
        uniforms.time[0] = 0.5;
        let frame = render_shader(
            &spec(code, &[], Blend::Opaque),
            &uniforms,
            64,
            32,
            &mut None,
        )
        .unwrap();
        // Red grows to the right; green grows upward (Shadertoy's origin is
        // bottom-left), so the top row is the greenest.
        assert!(px(&frame, 60, 16)[0] > px(&frame, 4, 16)[0]);
        assert!(px(&frame, 32, 2)[1] > px(&frame, 32, 29)[1]);
        assert!(
            (i32::from(px(&frame, 32, 16)[2]) - 128).abs() < 3,
            "iTime reaches the shader"
        );
    }

    #[test]
    fn image_textures_are_sampled_as_stored() {
        if !crate::available() {
            return;
        }
        let code = "void mainImage(out vec4 fragColor, in vec2 fragCoord) {\n\
            fragColor = texture(iTexture0, fragCoord / iResolution.xy);\n\
        }";
        // Left texel red, right texel a mid blue (stored values, not sRGB).
        let textures = [zeron_visual::TextureData::Rgba {
            width: 2,
            height: 1,
            pixels: vec![255, 0, 0, 255, 0, 0, 128, 255].into(),
        }];
        let spec = ShaderSpec {
            textures: &textures,
            ..spec(code, &[], Blend::Opaque)
        };
        let uniforms = ShaderUniforms {
            resolution: [64.0, 16.0, 1.0, 0.0],
            ..Default::default()
        };
        let frame = render_shader(&spec, &uniforms, 64, 16, &mut None).unwrap();
        // At the texel centers (it repeats, so edges blend with the far side).
        let left = px(&frame, 16, 8);
        assert!(left[0] > 245 && left[2] < 8, "{left:?}");
        let right = px(&frame, 48, 8);
        assert!(
            right[0] < 8 && (i32::from(right[2]) - 128).abs() < 6,
            "{right:?}"
        );
    }

    #[test]
    fn image_textures_show_upright_as_on_shadertoy() {
        if !crate::available() {
            return;
        }
        let code = "void mainImage(out vec4 fragColor, in vec2 fragCoord) {\n\
            fragColor = texture(iTexture0, fragCoord / iResolution.xy);\n\
        }";
        // Top texel red, bottom texel blue, as an image stores them.
        let textures = [zeron_visual::TextureData::Rgba {
            width: 1,
            height: 2,
            pixels: vec![255, 0, 0, 255, 0, 0, 255, 255].into(),
        }];
        let spec = ShaderSpec {
            textures: &textures,
            ..spec(code, &[], Blend::Opaque)
        };
        let uniforms = ShaderUniforms {
            resolution: [16.0, 64.0, 1.0, 0.0],
            ..Default::default()
        };
        let frame = render_shader(&spec, &uniforms, 16, 64, &mut None).unwrap();
        let (top, bottom) = (px(&frame, 8, 16), px(&frame, 8, 48));
        assert!(top[0] > 245 && top[2] < 8, "{top:?}");
        assert!(bottom[2] > 245 && bottom[0] < 8, "{bottom:?}");
    }

    #[test]
    fn light_blend_makes_black_clear_and_light_opaque() {
        if !crate::available() {
            return;
        }
        // A bright disc on black, as light-only art draws it.
        let code = "void mainImage(out vec4 fragColor, in vec2 fragCoord) {\n\
            float d = length(fragCoord - iResolution.xy * 0.5);\n\
            fragColor = vec4(vec3(d < 8.0 ? 1.0 : 0.0), 1.0);\n\
        }";
        let uniforms = ShaderUniforms {
            resolution: [48.0, 48.0, 1.0, 0.0],
            ..Default::default()
        };
        let frame =
            render_shader(&spec(code, &[], Blend::Light), &uniforms, 48, 48, &mut None).unwrap();
        assert_eq!(px(&frame, 2, 2)[3], 0, "black is see-through");
        assert_eq!(px(&frame, 24, 24)[3], 255, "light is solid");
    }

    #[test]
    fn light_fades_out_toward_the_edges_unless_opaque() {
        if !crate::available() {
            return;
        }
        // White everywhere, like a beam running off the frame.
        let code = "void mainImage(out vec4 fragColor, in vec2 fragCoord) {\n\
            fragColor = vec4(1.0);\n\
        }";
        let uniforms = ShaderUniforms {
            resolution: [200.0, 100.0, 1.0, 0.0],
            ..Default::default()
        };
        let render =
            |blend| render_shader(&spec(code, &[], blend), &uniforms, 200, 100, &mut None).unwrap();
        let light = render(Blend::Light);
        // The ramp is 15 px (15% of 100): clear at the edge, solid inside,
        // rising steadily in between.
        let row: Vec<u8> = (0..20).map(|x| px(&light, x, 50)[3]).collect();
        assert!(row[0] < 3, "{row:?}");
        assert_eq!(row[16], 255, "{row:?}");
        assert!(row.windows(2).all(|w| w[0] <= w[1]), "{row:?}");
        assert!(px(&light, 100, 0)[3] < 3, "the top fades too");
        assert!(px(&light, 199, 99)[3] < 3, "and the corners");
        let opaque = render(Blend::Opaque);
        assert_eq!(px(&opaque, 0, 0)[3], 255, "an opaque shader keeps its box");
    }

    #[test]
    fn a_buffer_reads_its_own_previous_frame() {
        if !crate::available() {
            return;
        }
        // Buffer A counts frames by adding 0.25 to what it wrote last time;
        // the image shows it.
        let buffer = vec![
            "void mainImage(out vec4 fragColor, in vec2 fragCoord) {\n\
            vec4 last = texture(iChannel0, fragCoord / iResolution.xy);\n\
            fragColor = vec4(last.r + 0.25, 0.0, 0.0, 1.0);\n\
        }"
            .to_owned(),
        ];
        let image = "void mainImage(out vec4 fragColor, in vec2 fragCoord) {\n\
            fragColor = vec4(texture(iChannel0, fragCoord / iResolution.xy).r, 0.0, 0.0, 1.0);\n\
        }";
        let uniforms = ShaderUniforms {
            resolution: [16.0, 16.0, 1.0, 0.0],
            ..Default::default()
        };
        let mut state = None;
        let mut reds = Vec::new();
        for _ in 0..3 {
            let frame = render_shader(
                &spec(image, &buffer, Blend::Opaque),
                &uniforms,
                16,
                16,
                &mut state,
            )
            .unwrap();
            reds.push(px(&frame, 8, 8)[0]);
        }
        assert!(
            reds[0] < reds[1] && reds[1] < reds[2],
            "accumulates: {reds:?}"
        );
        assert!(
            (i32::from(reds[2]) - 191).abs() < 4,
            "0.75 after three frames: {reds:?}"
        );
    }

    #[test]
    fn a_raymarched_mandelbulb_compiles_and_draws() {
        if !crate::available() {
            return;
        }
        let code = include_str!("../tests/mandelbulb.glsl");
        zeron_visual::shader::validate(code, &[]).unwrap();
        let uniforms = ShaderUniforms {
            resolution: [96.0, 96.0, 1.0, 0.0],
            camera: [0.6, 0.3, 1.0, 0.0],
            ..Default::default()
        };
        let frame =
            render_shader(&spec(code, &[], Blend::Alpha), &uniforms, 96, 96, &mut None).unwrap();
        let lit = frame.bgra.chunks_exact(4).filter(|p| p[3] > 0).count();
        assert!(lit > 96 * 96 / 10, "the fractal covers the middle: {lit}");
        assert!(lit < 96 * 96, "and the sky stays clear");
    }
}
