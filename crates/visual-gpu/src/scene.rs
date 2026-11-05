//! `ui.scene` on the GPU: physically based shading, soft shadows, glass that
//! refracts and splits light, and bloom, rendered offscreen and read back.
//!
//! Passes, per frame:
//! 1. shadow: the key light's depth over the geometry (orthographic);
//! 2. main: the environment map behind everything, when the scene asks for
//!    it; opaque surfaces (textured or not) into a multisampled HDR target
//!    with depth (GGX specular, sky/ground ambient, key light with PCF
//!    shadows, a fill light, glow); other see-through surfaces blended over
//!    them unbent, far to near; sprites (flares, glints) adding their light
//!    behind nearer surfaces; then lines and points unlit; resolved;
//! 3. glass: transmissive surfaces over that, refracting the resolved image
//!    (see-through surfaces and sprites behind it included) per color
//!    channel (dispersion) under a Fresnel reflection;
//! 4. bloom: bright parts downsampled and blurred;
//! 5. composite: bloom added, a fine depth-edge outline, tonemapped to sRGB
//!    straight alpha, transparent wherever nothing was drawn.

use wgpu::util::DeviceExt;
use zeron_visual::{Camera, CameraFrame, Color, Scene, V3};

use crate::textures::{Loaded, Space};
use crate::{EDGE_FADE, FORMAT, Frame, gpu};

const HDR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const SAMPLES: u32 = 4;
const SHADOW_SIZE: u32 = 2048;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Vertex {
    pos: [f32; 3],
    normal: [f32; 3],
    /// The face's own normal, for culling closed solids.
    face: [f32; 3],
    /// Linear color, straight alpha.
    color: [f32; 4],
    /// Roughness, metalness, glow, glass.
    mat: [f32; 4],
    /// Ior, dispersion, two-sided, kind (0 surface, 1 line, 2 point disc,
    /// 3 sprite).
    opt: [f32; 4],
    /// A surface's texture coordinates; a disc's or sprite's in -1..1.
    uv: [f32; 2],
}

impl Vertex {
    const ATTRIBUTES: [wgpu::VertexAttribute; 7] = wgpu::vertex_attr_array![
        0 => Float32x3, 1 => Float32x3, 2 => Float32x3, 3 => Float32x4,
        4 => Float32x4, 5 => Float32x4, 6 => Float32x2
    ];

    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Globals {
    view_proj: [[f32; 4]; 4],
    light_view_proj: [[f32; 4]; 4],
    eye: [f32; 4],
    /// Toward the key light.
    light: [f32; 4],
    sky: [f32; 4],
    ground: [f32; 4],
    /// Width, height, 1/width, 1/height (device pixels).
    viewport: [f32; 4],
    /// Near, far, the geometry's radius, unused.
    depth: [f32; 4],
    /// The camera's right, up, and forward, for screen-space refraction.
    right: [f32; 4],
    up: [f32; 4],
    forward: [f32; 4],
    /// The environment map: intensity, present, drawn behind, mip levels.
    env: [f32; 4],
    /// The projection's x and y scale, to turn screen points into rays.
    lens: [f32; 4],
}

fn bytes<T>(value: &T) -> &[u8] {
    // SAFETY: plain-old-data `repr(C)` structs of `f32`s.
    unsafe {
        std::slice::from_raw_parts((value as *const T).cast::<u8>(), std::mem::size_of::<T>())
    }
}

fn slice_bytes<T>(values: &[T]) -> &[u8] {
    // SAFETY: as above, for a slice.
    unsafe {
        std::slice::from_raw_parts(values.as_ptr().cast::<u8>(), std::mem::size_of_val(values))
    }
}

pub(crate) struct Pipelines {
    globals_layout: wgpu::BindGroupLayout,
    material_layout: wgpu::BindGroupLayout,
    glass_layout: wgpu::BindGroupLayout,
    post_layout: wgpu::BindGroupLayout,
    shadow: wgpu::RenderPipeline,
    opaque: wgpu::RenderPipeline,
    overlay: wgpu::RenderPipeline,
    glass: wgpu::RenderPipeline,
    translucent: wgpu::RenderPipeline,
    background: wgpu::RenderPipeline,
    sprite: wgpu::RenderPipeline,
    bright: wgpu::RenderPipeline,
    blur: wgpu::RenderPipeline,
    composite: wgpu::RenderPipeline,
    linear: wgpu::Sampler,
    shadow_sampler: wgpu::Sampler,
    /// Textures repeat and filter across mips, anisotropically.
    material_sampler: wgpu::Sampler,
    /// Equirectangular: wraps around, clamps at the poles.
    env_sampler: wgpu::Sampler,
    white: wgpu::TextureView,
}

const SURFACE: &str = r#"
struct Globals {
    view_proj: mat4x4<f32>,
    light_view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    light: vec4<f32>,
    sky: vec4<f32>,
    ground: vec4<f32>,
    viewport: vec4<f32>,
    depth: vec4<f32>,
    right: vec4<f32>,
    up: vec4<f32>,
    forward: vec4<f32>,
    env: vec4<f32>,
    lens: vec4<f32>,
};
@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var shadow_map: texture_depth_2d;
@group(0) @binding(2) var shadow_sampler: sampler_comparison;
@group(0) @binding(3) var env_map: texture_2d<f32>;
@group(0) @binding(4) var env_sampler: sampler;
@group(1) @binding(0) var material_map: texture_2d<f32>;
@group(1) @binding(1) var material_sampler: sampler;

struct In {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) face: vec3<f32>,
    @location(3) color: vec4<f32>,
    @location(4) mat: vec4<f32>,
    @location(5) opt: vec4<f32>,
    @location(6) uv: vec2<f32>,
};
struct Out {
    @builtin(position) clip: vec4<f32>,
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) face: vec3<f32>,
    @location(3) color: vec4<f32>,
    @location(4) mat: vec4<f32>,
    @location(5) opt: vec4<f32>,
    @location(6) uv: vec2<f32>,
};

@vertex fn vs(v: In) -> Out {
    var o: Out;
    o.clip = g.view_proj * vec4<f32>(v.pos, 1.0);
    o.pos = v.pos;
    o.normal = v.normal;
    o.face = v.face;
    o.color = v.color;
    o.mat = v.mat;
    o.opt = v.opt;
    o.uv = v.uv;
    return o;
}

@vertex fn vs_shadow(v: In) -> @builtin(position) vec4<f32> {
    return g.light_view_proj * vec4<f32>(v.pos, 1.0);
}

const PI: f32 = 3.14159265;

fn shadow(pos: vec3<f32>, n: vec3<f32>) -> f32 {
    let clip = g.light_view_proj * vec4<f32>(pos + n * 0.004 * g.depth.z, 1.0);
    let uv = vec2<f32>(clip.x * 0.5 + 0.5, 0.5 - clip.y * 0.5);
    if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0) { return 1.0; }
    let texel = 1.0 / f32(textureDimensions(shadow_map).x);
    var lit = 0.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            lit += textureSampleCompareLevel(shadow_map, shadow_sampler,
                uv + vec2<f32>(f32(x), f32(y)) * texel * 1.5, clip.z - 0.0015);
        }
    }
    return lit / 9.0;
}

fn ggx(n_h: f32, rough: f32) -> f32 {
    let a = rough * rough;
    let a2 = a * a;
    let d = n_h * n_h * (a2 - 1.0) + 1.0;
    return a2 / (PI * d * d);
}

fn smith(n_v: f32, n_l: f32, rough: f32) -> f32 {
    let k = (rough + 1.0) * (rough + 1.0) / 8.0;
    return (n_v / (n_v * (1.0 - k) + k)) * (n_l / (n_l * (1.0 - k) + k));
}

fn fresnel(cos_t: f32, f0: vec3<f32>) -> vec3<f32> {
    return f0 + (1.0 - f0) * pow(1.0 - cos_t, 5.0);
}

fn env_uv(dir: vec3<f32>) -> vec2<f32> {
    let d = normalize(dir);
    return vec2<f32>(atan2(d.x, -d.z) / (2.0 * PI) + 0.5, acos(clamp(d.y, -1.0, 1.0)) / PI);
}

// The light arriving from `dir`, as blurred as a surface of roughness
// `rough` sees it: the environment map when there is one, else a sky over
// a ground.
fn environment_at(dir: vec3<f32>, rough: f32) -> vec3<f32> {
    if (g.env.y > 0.5) {
        let level = clamp(rough, 0.0, 1.0) * max(g.env.w - 3.0, 0.0);
        return textureSampleLevel(env_map, env_sampler, env_uv(dir), level).rgb * g.env.x;
    }
    return mix(g.ground.rgb, g.sky.rgb, smoothstep(-0.3, 0.6, dir.y));
}

fn environment(dir: vec3<f32>) -> vec3<f32> {
    return environment_at(dir, 0.0);
}

// Diffuse light from around `n`: the map's coarsest useful mip.
fn irradiance(n: vec3<f32>) -> vec3<f32> {
    if (g.env.y > 0.5) {
        let level = max(g.env.w - 3.0, 0.0);
        return textureSampleLevel(env_map, env_sampler, env_uv(n), level).rgb * g.env.x;
    }
    return environment(n);
}

// The shading normal turned toward the camera; w is 0 on the far side of a
// closed solid, which is not drawn.
fn facing(i: Out, v: vec3<f32>) -> vec4<f32> {
    var n = normalize(i.normal);
    if (i.opt.z < 0.5 && dot(i.face, v) < 0.0) { return vec4<f32>(0.0); }
    if (dot(n, v) < 0.0) { n = -n; }
    return vec4<f32>(n, 1.0);
}

fn shade(i: Out, albedo: vec3<f32>, n: vec3<f32>, v: vec3<f32>) -> vec3<f32> {
    let rough = clamp(i.mat.x, 0.04, 1.0);
    let metal = i.mat.y;
    let l = normalize(g.light.xyz);
    let h = normalize(l + v);
    let n_l = max(dot(n, l), 0.0);
    let n_v = max(dot(n, v), 0.001);
    let n_h = max(dot(n, h), 0.0);
    let f0 = mix(vec3<f32>(0.04), albedo, metal);
    let f = fresnel(max(dot(h, v), 0.0), f0);
    let spec = ggx(n_h, rough) * smith(n_v, n_l, rough) * f / max(4.0 * n_v * n_l, 0.001);
    let kd = (vec3<f32>(1.0) - f) * (1.0 - metal);
    let key = (kd * albedo / PI + spec) * n_l * 3.2 * shadow(i.pos, n);
    let fill_dir = normalize(vec3<f32>(-l.x, 0.35, -l.z));
    let fill = kd * albedo / PI * max(dot(n, fill_dir), 0.0) * 0.9;
    let r = reflect(-v, n);
    let env_f = fresnel(n_v, f0);
    var ambient = kd * albedo * environment(n) * 0.85
        + env_f * environment(r) * (1.0 - rough * 0.85) * 0.8;
    if (g.env.y > 0.5) {
        ambient = kd * albedo * irradiance(n) + env_f * environment_at(r, rough);
    }
    return key + fill + ambient + albedo * i.mat.z;
}

@fragment fn fs_opaque(i: Out) -> @location(0) vec4<f32> {
    // Sampled first, while every fragment is still running (derivatives).
    let tex = textureSample(material_map, material_sampler, i.uv);
    let v = normalize(g.eye.xyz - i.pos);
    let f = facing(i, v);
    // A texture's clear parts are cut out.
    if (f.w == 0.0 || tex.a < 0.5) { discard; }
    return vec4<f32>(shade(i, i.color.rgb * tex.rgb, f.xyz, v), 1.0);
}

// A see-through surface that isn't glass: lit like a solid one and laid over
// what's behind it, unbent (premultiplied).
@fragment fn fs_translucent(i: Out) -> @location(0) vec4<f32> {
    let tex = textureSample(material_map, material_sampler, i.uv);
    let v = normalize(g.eye.xyz - i.pos);
    let f = facing(i, v);
    let a = clamp(i.color.a * tex.a, 0.0, 1.0);
    if (f.w == 0.0 || a <= 0.0) { discard; }
    return vec4<f32>(shade(i, i.color.rgb * tex.rgb, f.xyz, v) * a, a);
}

struct Screen { @builtin(position) clip: vec4<f32>, @location(0) ndc: vec2<f32> };

@vertex fn vs_background(@builtin(vertex_index) k: u32) -> Screen {
    let p = vec2<f32>(f32((k << 1u) & 2u), f32(k & 2u)) * 2.0 - 1.0;
    var o: Screen;
    o.clip = vec4<f32>(p, 0.5, 1.0);
    o.ndc = p;
    return o;
}

// The environment map seen straight on, behind the scene.
@fragment fn fs_background(i: Screen) -> @location(0) vec4<f32> {
    let dir = g.forward.xyz + g.right.xyz * (i.ndc.x / g.lens.x) + g.up.xyz * (i.ndc.y / g.lens.y);
    return vec4<f32>(environment(dir), 1.0);
}

// A sprite's light: its image, or a soft round glow (mat.x 0).
@fragment fn fs_sprite(i: Out) -> @location(0) vec4<f32> {
    let tex = textureSample(material_map, material_sampler, vec2<f32>(i.uv.x * 0.5 + 0.5, 0.5 - i.uv.y * 0.5));
    let d2 = dot(i.uv, i.uv);
    var shape = tex.rgb * tex.a;
    if (i.mat.x < 0.5) {
        let soft = exp(-d2 * 5.0) * 0.7 + exp(-d2 * 48.0) * 0.8;
        shape = vec3<f32>(soft * (1.0 - smoothstep(0.7, 1.0, d2)));
    }
    let c = shape * i.color.rgb * i.color.a * i.mat.z;
    return vec4<f32>(c, clamp(max(c.r, max(c.g, c.b)), 0.0, 1.0));
}

@fragment fn fs_overlay(i: Out) -> @location(0) vec4<f32> {
    if (i.opt.w > 1.5 && dot(i.uv, i.uv) > 1.0) { discard; }
    let c = i.color.rgb * (1.0 + i.mat.z);
    return vec4<f32>(c * i.color.a, i.color.a);
}
"#;

const GLASS: &str = r#"
@group(2) @binding(0) var behind: texture_2d<f32>;
@group(2) @binding(1) var behind_sampler: sampler;

@fragment fn fs_glass(i: Out) -> @location(0) vec4<f32> {
    let tex = textureSample(material_map, material_sampler, i.uv);
    let v = normalize(g.eye.xyz - i.pos);
    let f = facing(i, v);
    if (f.w == 0.0) { discard; }
    let n = f.xyz;
    let ior = max(i.opt.x, 1.0);
    let spread = i.opt.y * 0.09;
    let screen = i.clip.xy * g.viewport.zw;
    let glass = clamp(i.mat.w, 0.0, 1.0);
    // Bend what's behind by the surface's tilt across the view, a little
    // more for each shorter wavelength; partial glass bends less.
    let view_n = vec2<f32>(dot(n, g.right.xyz), -dot(n, g.up.xyz));
    var seen = vec3<f32>(0.0);
    var cover = 0.0;
    let iors = vec3<f32>(ior - spread, ior, ior + spread);
    for (var c = 0; c < 3; c++) {
        let bend = (1.0 - 1.0 / iors[c]) * 0.34 * glass;
        let s = textureSampleLevel(behind, behind_sampler, screen - view_n * bend, 0.0);
        seen[c] = s[c];
        cover = max(cover, s.a);
    }
    let albedo = i.color.rgb * tex.rgb;
    // With an environment, what isn't covered behind shows the map, bent
    // through the glass per channel.
    if (g.env.y > 0.5) {
        var bent = vec3<f32>(0.0);
        for (var c = 0; c < 3; c++) {
            var t = refract(-v, n, 1.0 / iors[c]);
            if (dot(t, t) < 1e-4) { t = reflect(-v, n); }
            bent[c] = environment(t)[c];
        }
        seen = seen + bent * (1.0 - cover);
        cover = 1.0;
    }
    let tint = mix(vec3<f32>(1.0), albedo, 0.35);
    let f0 = pow((ior - 1.0) / (ior + 1.0), 2.0);
    let n_v = max(dot(n, v), 0.0);
    let fr = f0 + (1.0 - f0) * pow(1.0 - n_v, 5.0);
    let r = reflect(-v, n);
    let l = normalize(g.light.xyz);
    let glint = pow(max(dot(r, l), 0.0), 220.0) * 6.0;
    let reflection = environment(r) * 1.2 + vec3<f32>(glint);
    let through = seen * tint;
    // Where nothing stands behind the glass, it shows its reflection and a
    // faint body tint over the chat.
    let body = 0.12 + 0.25 * (1.0 - glass);
    let alpha = clamp(max(cover, fr + body + glint), 0.0, 1.0);
    let color = through + (reflection - through) * fr + albedo * body * (1.0 - cover)
        + albedo * i.mat.z;
    return vec4<f32>(color, alpha);
}
"#;

const POST: &str = r#"
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;
@group(0) @binding(2) var<uniform> post: vec4<f32>;
@group(0) @binding(3) var bloom: texture_2d<f32>;
@group(0) @binding(4) var depth: texture_depth_multisampled_2d;

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
    let c = textureSampleLevel(source, source_sampler, i.uv, 0.0);
    let over = max(luma(c.rgb) - 1.0, 0.0);
    return vec4<f32>(c.rgb * (over / max(luma(c.rgb), 0.0001)), 1.0);
}

// post.xy: one texel step along the blur direction.
@fragment fn fs_blur(i: Out) -> @location(0) vec4<f32> {
    let w = array<f32, 5>(0.227, 0.194, 0.121, 0.054, 0.016);
    var sum = textureSampleLevel(source, source_sampler, i.uv, 0.0).rgb * w[0];
    for (var k = 1; k < 5; k++) {
        let o = post.xy * f32(k) * 1.5;
        sum += textureSampleLevel(source, source_sampler, i.uv + o, 0.0).rgb * w[k];
        sum += textureSampleLevel(source, source_sampler, i.uv - o, 0.0).rgb * w[k];
    }
    return vec4<f32>(sum, 1.0);
}

fn aces(x: vec3<f32>) -> vec3<f32> {
    let a = 2.51; let b = 0.03; let c = 2.43; let d = 0.59; let e = 0.14;
    return clamp((x * (a * x + b)) / (x * (c * x + d) + e), vec3<f32>(0.0), vec3<f32>(1.0));
}

fn to_srgb(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

const NEIGHBOURS = array<vec2<i32>, 4>(vec2<i32>(1, 0), vec2<i32>(-1, 0), vec2<i32>(0, 1), vec2<i32>(0, -1));

// post.x: bloom strength.
@fragment fn fs_composite(i: Out) -> @location(0) vec4<f32> {
    let c = textureSampleLevel(source, source_sampler, i.uv, 0.0);
    let glow = textureSampleLevel(bloom, source_sampler, i.uv, 0.0).rgb * post.x;
    // A fine outline where depth jumps: silhouettes and folds.
    let size = vec2<i32>(textureDimensions(depth));
    let at = clamp(vec2<i32>(i.clip.xy), vec2<i32>(0), size - 1);
    let z = textureLoad(depth, at, 0);
    var edge = 0.0;
    if (z < 1.0) {
        for (var k = 0; k < 4; k++) {
            let zn = textureLoad(depth, clamp(at + NEIGHBOURS[k], vec2<i32>(0), size - 1), 0);
            if (zn >= 1.0 || abs(zn - z) > 0.004 * (1.0 - z) + 0.0004) { edge = 1.0; }
        }
    }
    let alpha = clamp(c.a + luma(glow), 0.0, 1.0);
    if (alpha <= 0.0001) { return vec4<f32>(0.0); }
    var straight = (c.rgb + glow) / alpha;
    straight = straight * (1.0 - 0.18 * edge);
    let fade = edge_fade(i.clip.xy, vec2<f32>(size));
    return vec4<f32>(to_srgb(aces(straight)), alpha * fade);
}
"#;

impl Pipelines {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let uniform = |binding, visibility| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let texture = |binding, sample_type, multisampled| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled,
            },
            count: None,
        };
        let sampler = |binding, kind| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(kind),
            count: None,
        };
        let float = wgpu::TextureSampleType::Float { filterable: true };
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene globals"),
            entries: &[
                uniform(0, wgpu::ShaderStages::VERTEX_FRAGMENT),
                texture(1, wgpu::TextureSampleType::Depth, false),
                sampler(2, wgpu::SamplerBindingType::Comparison),
                texture(3, float, false),
                sampler(4, wgpu::SamplerBindingType::Filtering),
            ],
        });
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene material"),
            entries: &[
                texture(0, float, false),
                sampler(1, wgpu::SamplerBindingType::Filtering),
            ],
        });
        let glass_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene behind glass"),
            entries: &[
                texture(0, float, false),
                sampler(1, wgpu::SamplerBindingType::Filtering),
            ],
        });
        let post_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene post"),
            entries: &[
                texture(0, float, false),
                sampler(1, wgpu::SamplerBindingType::Filtering),
                uniform(2, wgpu::ShaderStages::FRAGMENT),
                texture(3, float, false),
                texture(4, wgpu::TextureSampleType::Depth, true),
            ],
        });
        let surface = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene surfaces"),
            source: wgpu::ShaderSource::Wgsl(SURFACE.into()),
        });
        let glass_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene glass"),
            source: wgpu::ShaderSource::Wgsl(format!("{SURFACE}{GLASS}").into()),
        });
        let post = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene post"),
            source: wgpu::ShaderSource::Wgsl(format!("{EDGE_FADE}{POST}").into()),
        });
        let layout = |layouts: &[Option<&wgpu::BindGroupLayout>]| {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None,
                bind_group_layouts: layouts,
                immediate_size: 0,
            })
        };
        let shadow_layout = layout(&[Some(&globals_layout)]);
        let surface_layout = layout(&[Some(&globals_layout), Some(&material_layout)]);
        let glass_pipeline_layout = layout(&[
            Some(&globals_layout),
            Some(&material_layout),
            Some(&glass_layout),
        ]);
        let post_pipeline_layout = layout(&[Some(&post_layout)]);
        let msaa = wgpu::MultisampleState {
            count: SAMPLES,
            ..Default::default()
        };
        let depth = |write: bool, bias: wgpu::DepthBiasState| {
            Some(wgpu::DepthStencilState {
                format: DEPTH,
                depth_write_enabled: Some(write),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias,
            })
        };
        let surface_pipeline =
            |label: &str,
             module: &wgpu::ShaderModule,
             layout: &wgpu::PipelineLayout,
             fragment: &str,
             blend: Option<wgpu::BlendState>,
             depth_stencil: Option<wgpu::DepthStencilState>| {
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(label),
                    layout: Some(layout),
                    vertex: wgpu::VertexState {
                        module,
                        entry_point: Some("vs"),
                        compilation_options: Default::default(),
                        buffers: &[Vertex::layout()],
                    },
                    primitive: wgpu::PrimitiveState::default(),
                    depth_stencil,
                    multisample: msaa,
                    fragment: Some(wgpu::FragmentState {
                        module,
                        entry_point: Some(fragment),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: HDR,
                            blend,
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    multiview_mask: None,
                    cache: None,
                })
            };
        let opaque = surface_pipeline(
            "scene opaque",
            &surface,
            &surface_layout,
            "fs_opaque",
            None,
            depth(true, Default::default()),
        );
        let overlay = surface_pipeline(
            "scene lines and points",
            &surface,
            &surface_layout,
            "fs_overlay",
            Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
            depth(
                false,
                wgpu::DepthBiasState {
                    constant: -4,
                    slope_scale: -1.0,
                    clamp: 0.0,
                },
            ),
        );
        let glass = surface_pipeline(
            "scene glass",
            &glass_module,
            &glass_pipeline_layout,
            "fs_glass",
            None,
            depth(true, Default::default()),
        );
        // Drawn far to near, so it tests depth but doesn't write it.
        let translucent = surface_pipeline(
            "scene translucent",
            &surface,
            &surface_layout,
            "fs_translucent",
            Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
            depth(false, Default::default()),
        );
        let sprite = surface_pipeline(
            "scene sprites",
            &surface,
            &surface_layout,
            "fs_sprite",
            Some(wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::One,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::One,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
            }),
            depth(false, Default::default()),
        );
        let background = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("scene environment"),
            layout: Some(&surface_layout),
            vertex: wgpu::VertexState {
                module: &surface,
                entry_point: Some("vs_background"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: msaa,
            fragment: Some(wgpu::FragmentState {
                module: &surface,
                entry_point: Some("fs_background"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: HDR,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let shadow = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("scene shadow"),
            layout: Some(&shadow_layout),
            vertex: wgpu::VertexState {
                module: &surface,
                entry_point: Some("vs_shadow"),
                compilation_options: Default::default(),
                buffers: &[Vertex::layout()],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: wgpu::DepthBiasState {
                    constant: 2,
                    slope_scale: 2.0,
                    clamp: 0.0,
                },
            }),
            multisample: Default::default(),
            fragment: None,
            multiview_mask: None,
            cache: None,
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
        Self {
            bright: post_pipeline("scene bloom bright", "fs_bright", HDR),
            blur: post_pipeline("scene bloom blur", "fs_blur", HDR),
            composite: post_pipeline("scene composite", "fs_composite", FORMAT),
            globals_layout,
            material_layout,
            glass_layout,
            post_layout,
            shadow,
            opaque,
            overlay,
            glass,
            translucent,
            background,
            sprite,
            material_sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("scene material"),
                address_mode_u: wgpu::AddressMode::Repeat,
                address_mode_v: wgpu::AddressMode::Repeat,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Linear,
                anisotropy_clamp: 8,
                ..Default::default()
            }),
            env_sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("scene environment"),
                address_mode_u: wgpu::AddressMode::Repeat,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Linear,
                ..Default::default()
            }),
            white: crate::textures::solid(device, queue, [255; 4]),
            linear: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("scene linear"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                ..Default::default()
            }),
            shadow_sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("scene shadow"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                compare: Some(wgpu::CompareFunction::LessEqual),
                ..Default::default()
            }),
        }
    }
}

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn scale(a: V3, s: f32) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn normalize(a: V3) -> V3 {
    let len = dot(a, a).sqrt();
    if len < 1e-9 {
        [0.0, 1.0, 0.0]
    } else {
        scale(a, 1.0 / len)
    }
}

fn to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear(rgba: [f32; 4]) -> [f32; 4] {
    [
        to_linear(rgba[0]),
        to_linear(rgba[1]),
        to_linear(rgba[2]),
        rgba[3],
    ]
}

/// Rows of a matrix into WGSL's column-major layout.
fn columns(rows: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut cols = [[0.0; 4]; 4];
    for (i, row) in rows.iter().enumerate() {
        for (j, value) in row.iter().enumerate() {
            cols[j][i] = *value;
        }
    }
    cols
}

/// The world-space key light, from above and to the side, so shadows fall
/// toward the viewer's default angle and ground the geometry.
const KEY_LIGHT: V3 = [-0.45, 0.85, 0.35];

/// A run of vertices drawn with one texture (`None`: untextured).
#[derive(Debug, Clone, PartialEq)]
struct Batch {
    texture: Option<u32>,
    range: std::ops::Range<u32>,
}

/// Append `vertices` to `out`, as part of the last batch when it shares
/// the texture.
fn push_batch(
    out: &mut Vec<Vertex>,
    batches: &mut Vec<Batch>,
    texture: Option<u32>,
    vertices: &[Vertex],
) {
    let start = out.len() as u32;
    out.extend_from_slice(vertices);
    let end = out.len() as u32;
    match batches.last_mut() {
        Some(last) if last.texture == texture && last.range.end == start => last.range.end = end,
        _ => batches.push(Batch {
            texture,
            range: start..end,
        }),
    }
}

struct Geometry {
    /// Opaque surfaces; the first `shadowed` vertices cast shadows (glowing
    /// surfaces, which are lights, come after and don't).
    opaque: Vec<Vertex>,
    opaque_batches: Vec<Batch>,
    shadowed: usize,
    glass: Vec<Vertex>,
    glass_batches: Vec<Batch>,
    /// See-through surfaces that aren't glass, far to near.
    translucent: Vec<Vertex>,
    translucent_batches: Vec<Batch>,
    overlay: Vec<Vertex>,
    sprites: Vec<Vertex>,
    sprite_batches: Vec<Batch>,
}

fn geometry(scene: &Scene, frame: &CameraFrame, rgba: &dyn Fn(Color) -> [f32; 4]) -> Geometry {
    use std::collections::BTreeMap;
    let mut lit: BTreeMap<Option<u32>, Vec<Vertex>> = BTreeMap::new();
    let mut glowing: BTreeMap<Option<u32>, Vec<Vertex>> = BTreeMap::new();
    let mut out = Geometry {
        opaque: Vec::new(),
        opaque_batches: Vec::new(),
        shadowed: 0,
        glass: Vec::new(),
        glass_batches: Vec::new(),
        translucent: Vec::new(),
        translucent_batches: Vec::new(),
        overlay: Vec::new(),
        sprites: Vec::new(),
        sprite_batches: Vec::new(),
    };
    let mut glass_faces: Vec<(f32, Option<u32>, Vec<Vertex>)> = Vec::new();
    let mut translucent_faces: Vec<(f32, Option<u32>, Vec<Vertex>)> = Vec::new();
    for face in scene.face_views() {
        let m = face.material;
        let color = linear(rgba(face.color));
        // A texture needs coordinates to be sampled by.
        let texture = face.texture.filter(|_| face.uvs.is_some());
        let vertex = |i: usize| Vertex {
            pos: face.corners[i],
            normal: face.normals.map_or(face.normal, |n| n[i]),
            face: face.normal,
            color,
            mat: [m.roughness, m.metalness, m.glow, m.glass],
            opt: [
                m.ior,
                m.dispersion,
                if face.two_sided { 1.0 } else { 0.0 },
                0.0,
            ],
            uv: face.uvs.map_or([0.0; 2], |uv| uv[i]),
        };
        let mut tris = Vec::with_capacity((face.corners.len() - 2) * 3);
        for i in 1..face.corners.len() - 1 {
            tris.extend([vertex(0), vertex(i), vertex(i + 1)]);
        }
        if m.glass > 0.0 || color[3] < 0.999 {
            let centroid = face.corners.iter().fold([0.0; 3], |acc, c| add(acc, *c));
            let centroid = scale(centroid, 1.0 / face.corners.len() as f32);
            let distance = dot(sub(centroid, frame.eye), frame.forward);
            // Only glass refracts; a faded wall or pane is just see-through.
            if m.glass > 0.0 {
                glass_faces.push((distance, texture, tris));
            } else {
                translucent_faces.push((distance, texture, tris));
            }
        } else if m.glow > 0.0 {
            glowing.entry(texture).or_default().extend(tris);
        } else {
            lit.entry(texture).or_default().extend(tris);
        }
    }
    for (texture, tris) in &lit {
        push_batch(&mut out.opaque, &mut out.opaque_batches, *texture, tris);
    }
    out.shadowed = out.opaque.len();
    for (texture, tris) in &glowing {
        push_batch(&mut out.opaque, &mut out.opaque_batches, *texture, tris);
    }
    // Glass far to near, so nearer panes cover farther ones.
    glass_faces.sort_by(|a, b| b.0.total_cmp(&a.0));
    for (_, texture, tris) in &glass_faces {
        push_batch(&mut out.glass, &mut out.glass_batches, *texture, tris);
    }
    translucent_faces.sort_by(|a, b| b.0.total_cmp(&a.0));
    for (_, texture, tris) in &translucent_faces {
        push_batch(
            &mut out.translucent,
            &mut out.translucent_batches,
            *texture,
            tris,
        );
    }

    // Lines and points face the camera, sized in screen pixels.
    let world_per_pixel =
        |p: V3| dot(sub(p, frame.eye), frame.forward).max(frame.near) / frame.focal;
    let flat = |pos: V3, color: [f32; 4], kind: f32, uv: [f32; 2]| Vertex {
        pos,
        normal: [0.0, 1.0, 0.0],
        face: [0.0, 1.0, 0.0],
        color,
        mat: [1.0, 0.0, 0.0, 0.0],
        opt: [1.0, 0.0, 1.0, kind],
        uv,
    };
    for (a, b, color, width) in scene.line_views() {
        let color = linear(rgba(color));
        let dir = normalize(sub(b, a));
        let side_a = normalize(cross(dir, sub(a, frame.eye)));
        let side_b = normalize(cross(dir, sub(b, frame.eye)));
        let wa = scale(side_a, width.max(1.0) * 0.5 * world_per_pixel(a));
        let wb = scale(side_b, width.max(1.0) * 0.5 * world_per_pixel(b));
        let quad = [add(a, wa), add(b, wb), sub(b, wb), sub(a, wa)];
        for i in [0, 1, 2, 0, 2, 3] {
            out.overlay.push(flat(quad[i], color, 1.0, [0.0; 2]));
        }
    }
    for (at, radius, color) in scene.point_views() {
        let color = linear(rgba(color));
        let r = radius * world_per_pixel(at);
        let (x, y) = (scale(frame.right, r), scale(frame.up, r));
        let corners = [
            (add(sub(at, x), y), [-1.0, 1.0]),
            (add(add(at, x), y), [1.0, 1.0]),
            (sub(add(at, x), y), [1.0, -1.0]),
            (sub(sub(at, x), y), [-1.0, -1.0]),
        ];
        for i in [0, 1, 2, 0, 2, 3] {
            out.overlay
                .push(flat(corners[i].0, color, 2.0, corners[i].1));
        }
    }
    let mut sprites: Vec<_> = scene.sprites().iter().collect();
    sprites.sort_by_key(|sprite| sprite.texture);
    for sprite in sprites {
        let color = linear(rgba(sprite.color));
        let r = sprite.size * 0.5 * world_per_pixel(sprite.at);
        let (x, y) = (scale(frame.right, r), scale(frame.up, r));
        let at = sprite.at;
        let corners = [
            (add(sub(at, x), y), [-1.0, 1.0]),
            (add(add(at, x), y), [1.0, 1.0]),
            (sub(add(at, x), y), [1.0, -1.0]),
            (sub(sub(at, x), y), [-1.0, -1.0]),
        ];
        let quad: Vec<Vertex> = [0, 1, 2, 0, 2, 3]
            .into_iter()
            .map(|i| Vertex {
                mat: [
                    if sprite.texture.is_some() { 1.0 } else { 0.0 },
                    0.0,
                    sprite.glow,
                    0.0,
                ],
                ..flat(corners[i].0, color, 3.0, corners[i].1)
            })
            .collect();
        push_batch(
            &mut out.sprites,
            &mut out.sprite_batches,
            sprite.texture,
            &quad,
        );
    }
    out
}

/// One scene's render targets between frames (about 70 MB at a Retina
/// size): made once, then reused while the size holds.
pub struct SceneState {
    size: (u32, u32),
    shadow_map: wgpu::Texture,
    msaa_color: wgpu::Texture,
    msaa_depth: wgpu::Texture,
    opaque_resolved: wgpu::Texture,
    lit: wgpu::Texture,
    bloom_a: wgpu::Texture,
    bloom_b: wgpu::Texture,
    output: wgpu::Texture,
    dummy_depth: wgpu::Texture,
    staging: Option<wgpu::Buffer>,
}

impl SceneState {
    fn new(gpu: &crate::Gpu, (w, h): (u32, u32)) -> Self {
        use wgpu::TextureUsages as U;
        let device = &gpu.device;
        let texture = |label: &str, size: (u32, u32), format, samples, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let sampled = U::RENDER_ATTACHMENT | U::TEXTURE_BINDING;
        let half = ((w / 2).max(1), (h / 2).max(1));
        Self {
            size: (w, h),
            shadow_map: texture(
                "scene shadow map",
                (SHADOW_SIZE, SHADOW_SIZE),
                DEPTH,
                1,
                sampled,
            ),
            msaa_color: texture("scene color", (w, h), HDR, SAMPLES, U::RENDER_ATTACHMENT),
            msaa_depth: texture("scene depth", (w, h), DEPTH, SAMPLES, sampled),
            opaque_resolved: texture("scene behind glass", (w, h), HDR, 1, sampled),
            lit: texture("scene lit", (w, h), HDR, 1, sampled),
            bloom_a: texture("scene bloom", half, HDR, 1, sampled),
            bloom_b: texture("scene bloom", half, HDR, 1, sampled),
            output: gpu.target(w, h, "scene output"),
            dummy_depth: lit_dummy_depth(device),
            staging: None,
        }
    }
}

/// Render `scene` on the GPU for a canvas `width` by `height` logical pixels
/// at `pixel_scale`, reusing `state`'s targets (made or remade as needed).
/// `None` without a GPU (or geometry), so the caller can fall back to the
/// software renderer.
pub fn render_scene(
    scene: &Scene,
    camera: Camera,
    width: f32,
    height: f32,
    pixel_scale: f32,
    rgba: &dyn Fn(Color) -> [f32; 4],
    state: &mut Option<SceneState>,
) -> Option<Frame> {
    let gpu = gpu()?;
    let frame = scene.camera_frame(camera, width, height)?;
    let (w, h) = (
        (width * pixel_scale).round().max(1.0) as u32,
        (height * pixel_scale).round().max(1.0) as u32,
    );
    let pipelines = gpu
        .scene
        .get_or_init(|| Pipelines::new(&gpu.device, &gpu.queue));
    let device = &gpu.device;
    let geometry = geometry(scene, &frame, rgba);
    // Textures, decoded and uploaded once each; one that fails draws plain.
    let loaded: Vec<Option<Loaded>> = scene
        .textures()
        .iter()
        .map(|t| {
            gpu.textures
                .get(device, &gpu.queue, &t.data, Space::Color)
                .ok()
        })
        .collect();
    let environment = scene
        .environment()
        .and_then(|e| Some((e, loaded.get(e.texture as usize)?.clone()?)));

    // Camera: the same framing and projection as the software renderer.
    let distance = dot(sub(frame.center, frame.eye), sub(frame.center, frame.eye)).sqrt();
    let near = frame.near;
    let far = distance + frame.radius * 2.0;
    let (sx, sy) = (frame.focal / (width / 2.0), frame.focal / (height / 2.0));
    let (a, b) = (far / (far - near), -far * near / (far - near));
    let row = |axis: V3, s: f32| {
        [
            axis[0] * s,
            axis[1] * s,
            axis[2] * s,
            -dot(axis, frame.eye) * s,
        ]
    };
    let f = frame.forward;
    let view_proj = columns([
        row(frame.right, sx),
        row(frame.up, sy),
        [f[0] * a, f[1] * a, f[2] * a, -dot(f, frame.eye) * a + b],
        row(f, 1.0),
    ]);
    // The key light's orthographic view, fitted to the geometry.
    let light = normalize(KEY_LIGHT);
    let light_forward = scale(light, -1.0);
    let light_right = normalize(cross(light_forward, [0.0, 0.0, 1.0]));
    let light_up = cross(light_right, light_forward);
    let r = frame.radius * 1.1;
    let c = frame.center;
    let light_view_proj = columns([
        [
            light_right[0] / r,
            light_right[1] / r,
            light_right[2] / r,
            -dot(light_right, c) / r,
        ],
        [
            light_up[0] / r,
            light_up[1] / r,
            light_up[2] / r,
            -dot(light_up, c) / r,
        ],
        [
            light_forward[0] / (2.0 * r),
            light_forward[1] / (2.0 * r),
            light_forward[2] / (2.0 * r),
            (-dot(light_forward, c) + r) / (2.0 * r),
        ],
        [0.0, 0.0, 0.0, 1.0],
    ]);
    let globals = Globals {
        view_proj,
        light_view_proj,
        eye: [frame.eye[0], frame.eye[1], frame.eye[2], 0.0],
        light: [light[0], light[1], light[2], 0.0],
        sky: [0.62, 0.66, 0.74, 0.0],
        ground: [0.26, 0.24, 0.22, 0.0],
        viewport: [w as f32, h as f32, 1.0 / w as f32, 1.0 / h as f32],
        depth: [near, far, frame.radius, 0.0],
        right: [frame.right[0], frame.right[1], frame.right[2], 0.0],
        up: [frame.up[0], frame.up[1], frame.up[2], 0.0],
        forward: [f[0], f[1], f[2], 0.0],
        env: match &environment {
            Some((e, map)) => [
                e.intensity,
                1.0,
                if e.background { 1.0 } else { 0.0 },
                map.mips as f32,
            ],
            None => [0.0; 4],
        },
        lens: [sx, sy, 0.0, 0.0],
    };
    let globals_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("scene globals"),
        contents: bytes(&globals),
        usage: wgpu::BufferUsages::UNIFORM,
    });

    // Targets, kept between frames.
    if state.as_ref().is_none_or(|t| t.size != (w, h)) {
        *state = Some(SceneState::new(gpu, (w, h)));
    }
    let targets = state.as_mut().expect("made above");
    let SceneState {
        shadow_map,
        msaa_color,
        msaa_depth,
        opaque_resolved,
        lit,
        bloom_a,
        bloom_b,
        output,
        dummy_depth,
        staging,
        ..
    } = targets;
    let half = ((w / 2).max(1), (h / 2).max(1));
    let view = |t: &wgpu::Texture| t.create_view(&Default::default());

    let env_view = environment
        .as_ref()
        .map_or(&pipelines.white, |(_, map)| &map.view);
    // Slot 0 is plain white (untextured); texture `i` is slot `i + 1`.
    let material_binds: Vec<wgpu::BindGroup> = std::iter::once(Some(&pipelines.white))
        .chain(loaded.iter().map(|l| l.as_ref().map(|l| &l.view)))
        .map(|view| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("scene material"),
                layout: &pipelines.material_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(
                            view.unwrap_or(&pipelines.white),
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&pipelines.material_sampler),
                    },
                ],
            })
        })
        .collect();
    let material = |texture: Option<u32>| {
        &material_binds[texture
            .map_or(0, |t| t as usize + 1)
            .min(material_binds.len() - 1)]
    };
    let globals_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("scene globals"),
        layout: &pipelines.globals_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: globals_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&view(shadow_map)),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(&pipelines.shadow_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(env_view),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::Sampler(&pipelines.env_sampler),
            },
        ],
    });
    let vertices = |label: &str, data: &[Vertex]| {
        (!data.is_empty()).then(|| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: slice_bytes(data),
                usage: wgpu::BufferUsages::VERTEX,
            })
        })
    };
    let opaque = vertices("scene opaque", &geometry.opaque);
    let glass = vertices("scene glass", &geometry.glass);
    let translucent = vertices("scene translucent", &geometry.translucent);
    let overlay = vertices("scene overlay", &geometry.overlay);
    let sprites = vertices("scene sprites", &geometry.sprites);
    let draw = |pass: &mut wgpu::RenderPass, buffer: &Option<wgpu::Buffer>, count: usize| {
        if let Some(buffer) = buffer {
            pass.set_vertex_buffer(0, buffer.slice(..));
            pass.draw(0..count as u32, 0..1);
        }
    };
    let draw_batches =
        |pass: &mut wgpu::RenderPass, buffer: &Option<wgpu::Buffer>, batches: &[Batch]| {
            if let Some(buffer) = buffer {
                pass.set_vertex_buffer(0, buffer.slice(..));
                for batch in batches {
                    pass.set_bind_group(1, material(batch.texture), &[]);
                    pass.draw(batch.range.clone(), 0..1);
                }
            }
        };

    let mut encoder = device.create_command_encoder(&Default::default());
    // 1. Shadow. Its bind group (with the sampled map) is unused here: the
    // shadow vertex stage only reads the globals.
    let shadow_globals = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("scene shadow globals"),
        layout: &pipelines.globals_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: globals_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&view(dummy_depth)),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(&pipelines.shadow_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(env_view),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::Sampler(&pipelines.env_sampler),
            },
        ],
    });
    {
        let shadow_view = view(shadow_map);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("scene shadow"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &shadow_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&pipelines.shadow);
        pass.set_bind_group(0, &shadow_globals, &[]);
        draw(&mut pass, &opaque, geometry.shadowed);
    }
    let color_view = view(msaa_color);
    let depth_view = view(msaa_depth);
    // 2. Opaque surfaces; see-through ones and sprites over them (so glass
    // refracts them too); then lines and points.
    {
        let resolve = view(opaque_resolved);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("scene main"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &color_view,
                depth_slice: None,
                resolve_target: Some(&resolve),
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_bind_group(0, &globals_bind, &[]);
        pass.set_bind_group(1, material(None), &[]);
        if environment.as_ref().is_some_and(|(e, _)| e.background) {
            pass.set_pipeline(&pipelines.background);
            pass.draw(0..3, 0..1);
        }
        pass.set_pipeline(&pipelines.opaque);
        draw_batches(&mut pass, &opaque, &geometry.opaque_batches);
        pass.set_pipeline(&pipelines.translucent);
        draw_batches(&mut pass, &translucent, &geometry.translucent_batches);
        pass.set_pipeline(&pipelines.sprite);
        draw_batches(&mut pass, &sprites, &geometry.sprite_batches);
        pass.set_bind_group(1, material(None), &[]);
        pass.set_pipeline(&pipelines.overlay);
        draw(&mut pass, &overlay, geometry.overlay.len());
    }
    // 3. Glass, refracting everything resolved behind it.
    let shaded = if glass.is_some() {
        let behind_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene behind glass"),
            layout: &pipelines.glass_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view(opaque_resolved)),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&pipelines.linear),
                },
            ],
        });
        let resolve = view(lit);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("scene glass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &color_view,
                depth_slice: None,
                resolve_target: Some(&resolve),
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_bind_group(0, &globals_bind, &[]);
        pass.set_bind_group(2, &behind_bind, &[]);
        pass.set_pipeline(&pipelines.glass);
        draw_batches(&mut pass, &glass, &geometry.glass_batches);
        &lit
    } else {
        &opaque_resolved
    };

    // 4–5. Bloom and composite.
    let post_uniform = |values: [f32; 4]| {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("scene post"),
            contents: slice_bytes(&values),
            usage: wgpu::BufferUsages::UNIFORM,
        })
    };
    let depth_sample_view = msaa_depth.create_view(&wgpu::TextureViewDescriptor {
        aspect: wgpu::TextureAspect::DepthOnly,
        ..Default::default()
    });
    let post_bind = |source: &wgpu::Texture, uniform: &wgpu::Buffer, bloom: &wgpu::Texture| {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene post"),
            layout: &pipelines.post_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view(source)),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&pipelines.linear),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&view(bloom)),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&depth_sample_view),
                },
            ],
        })
    };
    let fullscreen = |encoder: &mut wgpu::CommandEncoder,
                      target: &wgpu::Texture,
                      pipeline: &wgpu::RenderPipeline,
                      bind: &wgpu::BindGroup| {
        let target = view(target);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("scene post"),
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
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, bind, &[]);
        pass.draw(0..3, 0..1);
    };
    let zero = post_uniform([0.0; 4]);
    let step_x = post_uniform([1.0 / half.0 as f32, 0.0, 0.0, 0.0]);
    let step_y = post_uniform([0.0, 1.0 / half.1 as f32, 0.0, 0.0]);
    fullscreen(
        &mut encoder,
        bloom_a,
        &pipelines.bright,
        &post_bind(shaded, &zero, bloom_b),
    );
    for _ in 0..2 {
        fullscreen(
            &mut encoder,
            bloom_b,
            &pipelines.blur,
            &post_bind(bloom_a, &step_x, &lit_dummy(device)),
        );
        fullscreen(
            &mut encoder,
            bloom_a,
            &pipelines.blur,
            &post_bind(bloom_b, &step_y, &lit_dummy(device)),
        );
    }
    let strength = post_uniform([0.9, 0.0, 0.0, 0.0]);
    fullscreen(
        &mut encoder,
        output,
        &pipelines.composite,
        &post_bind(shaded, &strength, bloom_a),
    );
    Some(gpu.read_back(encoder, output, staging))
}

/// A 1×1 texture for bind slots a pass doesn't read (a texture can't be both
/// the target and a binding of one pass).
fn lit_dummy(device: &wgpu::Device) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("scene unused"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: HDR,
        usage: wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

/// As [`lit_dummy`], for the shadow map slot while the shadow map is drawn.
fn lit_dummy_depth(device: &wgpu::Device) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("scene unused depth"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH,
        usage: wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_visual::Token;

    fn palette(color: Color) -> [f32; 4] {
        let [r, g, b] = match color.token {
            Token::Chart(0) => [0.3, 0.6, 0.95],
            Token::Chart(1) => [0.95, 0.45, 0.35],
            Token::Chart(2) => [0.4, 0.85, 0.5],
            Token::Chart(3) => [0.95, 0.8, 0.3],
            _ => [0.75, 0.75, 0.78],
        };
        [r, g, b, color.alpha]
    }

    fn scene(script: &str) -> Scene {
        let mut visual = zeron_visual::Visual::load(script, 1).unwrap();
        let tree = visual.render().unwrap();
        let zeron_visual::NodeKind::Scene(node) = tree.kind else {
            panic!("a scene")
        };
        visual.draw_scene(node.draw, 0.0).unwrap()
    }

    fn room() -> Scene {
        let mut visual = zeron_visual::Visual::load(
            r#"
            return function()
              return ui.scene { height = 300, draw = function(s)
                s:box { at = { 0, -0.05, 0 }, size = { 6, 0.1, 4 }, color = "muted" }
                s:box { at = { -1.6, 0.5, 0 }, size = { 1, 1, 1 }, color = "chart1", roughness = 0.35 }
                s:sphere { at = { 0, 0.6, 0.4 }, radius = 0.6, color = "chart2", metal = 1, roughness = 0.25 }
                s:box { at = { 1.6, 0.6, 0 }, size = { 0.9, 1.2, 0.9 }, color = "chart3", glass = true, dispersion = 1 }
                s:sphere { at = { 1.6, 1.6, -1 }, radius = 0.2, color = "chart4", glow = 4 }
              end }
            end
            "#,
            1,
        )
        .unwrap();
        let tree = visual.render().unwrap();
        let zeron_visual::NodeKind::Scene(node) = tree.kind else {
            panic!("a scene")
        };
        visual.draw_scene(node.draw, 0.0).unwrap()
    }

    fn png(width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 4]) -> std::sync::Arc<[u8]> {
        let mut out = Vec::new();
        image::RgbaImage::from_fn(width, height, |x, y| image::Rgba(pixel(x, y)))
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out.into()
    }

    /// A checkered box, a glass prism, a metal ball under a striped sky
    /// map, and a flare.
    fn gallery() -> Scene {
        let assets = zeron_visual::Assets::new(
            [
                (
                    "checker.png".to_owned(),
                    png(64, 64, |x, y| {
                        if (x / 8 + y / 8) % 2 == 0 {
                            [240, 240, 240, 255]
                        } else {
                            [30, 60, 200, 255]
                        }
                    }),
                ),
                (
                    "sky.png".to_owned(),
                    png(256, 128, |x, y| {
                        let band = (x / 32) % 2 == 0;
                        let up = 255 - (y * 2).min(255) as u8;
                        if band {
                            [255, up, 80, 255]
                        } else {
                            [40, 80, up, 255]
                        }
                    }),
                ),
            ]
            .into_iter()
            .collect(),
        );
        let mut visual = zeron_visual::Visual::load(
            r##"
            return ui.scene { height = 300, draw = function(s)
              s:environment { asset = "sky.png", intensity = 1.2, background = true }
              s:box { at = { -1.4, 0.5, 0 }, size = 1, texture = "checker.png", roughness = 0.6 }
              s:mesh { vertices = { {0.2,0,-0.5}, {1.2,0,-0.5}, {0.7,1.4,-0.5}, {0.2,0,0.5}, {1.2,0,0.5}, {0.7,1.4,0.5} },
                faces = { {1,2,3}, {4,6,5}, {1,4,5,2}, {2,5,6,3}, {3,6,4,1} },
                color = "#ffffff", glass = true, ior = 1.5, dispersion = 1 }
              s:sphere { at = { 2.2, 0.6, 0 }, radius = 0.6, color = "#dddddd", metal = 1, roughness = 0.1 }
              s:sprite { at = { 0.7, 1.6, 0 }, size = 120, color = "#ffd9a0", glow = 2 }
            end }
            "##,
            1,
        )
        .unwrap()
        .with_assets(assets);
        let tree = visual.render().unwrap();
        let zeron_visual::NodeKind::Scene(node) = tree.kind else {
            panic!("a scene")
        };
        visual.draw_scene(node.draw, 0.0).unwrap()
    }

    #[test]
    fn textures_environments_and_sprites_render() {
        if !crate::available() {
            return;
        }
        let scene = gallery();
        let mut state = None;
        let camera = Camera {
            yaw: 0.5,
            pitch: 0.35,
            zoom: 1.0,
        };
        let frame = render_scene(&scene, camera, 720.0, 360.0, 2.0, &palette, &mut state).unwrap();
        if let Ok(path) = std::env::var("ZERON_VISUAL_DUMP_ASSETS") {
            crate::dump_ppm(&frame, &path);
        }
        // The environment stands behind everything: no clear pixels inside
        // the edge fade (15% of the 720 px height), and a faded frame edge.
        let (w, h) = (frame.width as usize, frame.height as usize);
        let band = 110;
        let mut inside = frame.bgra.chunks_exact(4).enumerate().filter(|(i, _)| {
            let (x, y) = (i % w, i / w);
            (band..w - band).contains(&x) && (band..h - band).contains(&y)
        });
        assert!(inside.all(|(_, p)| p[3] == 255));
        assert!(frame.bgra[3] < 3, "the corner fades out");
        // The checker shows both of its colors somewhere.
        let blueish = frame
            .bgra
            .chunks_exact(4)
            .filter(|p| p[0] > 150 && p[2] < 90 && p[1] < 120)
            .count();
        assert!(blueish > 500, "{blueish}");
        // A texture uploads once, however many frames draw it.
        render_scene(&scene, camera, 720.0, 360.0, 2.0, &palette, &mut state).unwrap();
    }

    #[test]
    fn a_room_renders_lit_with_glass_and_glow() {
        if !crate::available() {
            return;
        }
        let scene = room();
        let mut state = None;
        let camera = Camera {
            yaw: 0.6,
            pitch: 0.45,
            zoom: 1.0,
        };
        // The first frame also makes the device and compiles pipelines.
        render_scene(&scene, camera, 720.0, 360.0, 2.0, &palette, &mut state).unwrap();
        let start = std::time::Instant::now();
        let frame = render_scene(&scene, camera, 720.0, 360.0, 2.0, &palette, &mut state).unwrap();
        eprintln!(
            "gpu scene {}x{} in {:?}",
            frame.width,
            frame.height,
            start.elapsed()
        );
        if let Ok(path) = std::env::var("ZERON_VISUAL_DUMP") {
            crate::dump_ppm(&frame, &path);
        }
        assert_eq!((frame.width, frame.height), (1440, 720));
        let drawn = frame.bgra.chunks_exact(4).filter(|p| p[3] > 0).count();
        let total = (frame.width * frame.height) as usize;
        assert!(drawn > total / 8, "geometry fills the frame: {drawn}");
        assert!(drawn < total, "and the background stays clear");
    }

    /// The mean column of the strongly red pixels.
    fn red_center(frame: &Frame) -> f64 {
        let (mut sum, mut count) = (0.0, 0usize);
        for (i, p) in frame.bgra.chunks_exact(4).enumerate() {
            if p[2] > 120 && p[2] as i32 - p[0] as i32 > 50 {
                sum += (i % frame.width as usize) as f64;
                count += 1;
            }
        }
        assert!(count > 1000, "the red box shows: {count}");
        sum / count as f64
    }

    #[test]
    fn translucent_surfaces_show_what_is_behind_them_in_place() {
        if !crate::available() {
            return;
        }
        let camera = Camera {
            yaw: 0.6,
            pitch: 0.3,
            zoom: 1.0,
        };
        let render = |shell: &str| {
            let script = format!(
                r#"return ui.scene {{ height = 300, draw = function(s)
                  s:box {{ at = {{ 0, 0.5, 0 }}, size = 1, color = "chart2" }}
                  {shell}
                end }}"#
            );
            render_scene(
                &scene(&script),
                camera,
                720.0,
                360.0,
                1.0,
                &palette,
                &mut None,
            )
            .unwrap()
        };
        let bare = red_center(&render(""));
        // A faded shell around the box (like a cutaway wall) must not shift
        // it the way glass would.
        let shelled = red_center(&render(
            r#"s:box { at = { 0, 0.6, 0 }, size = 1.8, color = ui.color("muted", 0.3) }"#,
        ));
        assert!(
            (bare - shelled).abs() < 6.0,
            "box moved from {bare} to {shelled}"
        );
    }
}
