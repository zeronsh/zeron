// A plugin's 3D scene: solid triangles, depth-tested, lit per pixel.
//
// The CPU already multiplied the model, view and projection matrices into the
// positions, so they arrive in clip space and the vertex shader passes them
// through. Normals come in world space, the light sits in the world with them,
// and the fragment shader shades; the depth buffer decides what hides what.

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_main(
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
) -> VertexOut {
    var out: VertexOut;
    out.position = vec4<f32>(position, 1.0);
    out.normal = normal;
    out.color = color;
    return out;
}

// A key light up and to the left, a dimmer fill from the opposite side, and
// enough ambient that no face is ever black.
const KEY: vec3<f32> = vec3<f32>(0.42, 0.82, 0.52);
const FILL: vec3<f32> = vec3<f32>(-0.55, -0.15, -0.35);
const AMBIENT: f32 = 0.34;

fn shaded(in: VertexOut) -> vec4<f32> {
    let normal = normalize(in.normal);
    let key = max(dot(normal, normalize(KEY)), 0.0);
    let fill = max(dot(normal, normalize(FILL)), 0.0) * 0.35;
    let light = AMBIENT + key * 0.78 + fill;
    let alpha = in.color.a;
    // Premultiplied, the way the rest of a plugin window's pixels are.
    return vec4<f32>(in.color.rgb * light * alpha, alpha);
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    return shaded(in);
}

// An sRGB target encodes what the shader writes and the colours are sRGB
// already, so decode here and the hardware's encode brings them back.
@fragment
fn fs_srgb(in: VertexOut) -> @location(0) vec4<f32> {
    let color = shaded(in);
    return vec4<f32>(pow(color.rgb, vec3<f32>(2.2)), color.a);
}