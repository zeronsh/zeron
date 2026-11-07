// Dear ImGui's draw lists, replayed by the plugin window's renderer.
//
// Positions arrive in device pixels (the CPU already applied the display
// offset and framebuffer scale), texture coordinates index the font atlas,
// and the vertex colour is straight-alpha sRGB. The fragment shader outputs
// premultiplied colour, which zgx's compositor expects; the pipeline's blend
// state is ONE / ONE_MINUS_SRC_ALPHA.

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@group(0) @binding(0) var atlas: texture_2d<f32>;
@group(0) @binding(1) var atlas_sampler: sampler;

@vertex
fn vs_main(
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
) -> VertexOut {
    var out: VertexOut;
    out.position = vec4<f32>(position, 0.0, 1.0);
    out.uv = uv;
    out.color = color;
    return out;
}

// The atlas is white with coverage in alpha (Dear ImGui 1.92+), so a text or
// shape quad shades from its colour alone; a plugin's picture carries its own
// colours, which multiply the tint.
fn shaded(in: VertexOut) -> vec4<f32> {
    let texel = textureSample(atlas, atlas_sampler, in.uv);
    let alpha = in.color.a * texel.a;
    return vec4<f32>(in.color.rgb * texel.rgb * alpha, alpha);
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    return shaded(in);
}

// An sRGB target encodes whatever the shader writes. Dear ImGui's colours
// are sRGB already, so decode them here and the hardware's encode brings
// them back as authored.
@fragment
fn fs_srgb(in: VertexOut) -> @location(0) vec4<f32> {
    let color = shaded(in);
    return vec4<f32>(pow(color.rgb, vec3<f32>(2.2)), color.a);
}
