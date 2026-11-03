//! `ui.shader`: a fragment shader the desktop runs on the GPU, in the
//! convention agents already know from Shadertoy. The script supplies
//! `void mainImage(out vec4 fragColor, in vec2 fragCoord)`; Zeron wraps it in
//! a complete GLSL program with the usual inputs and validates it here, so a
//! compile error reaches the agent (with its line) before the reader sees it.
//!
//! Inputs, as in Shadertoy: `iResolution` (vec3, pixels), `iTime` and
//! `iTimeDelta` (seconds), `iFrame` (int), `iMouse` (vec4: xy while the
//! button is down, zw where it went down, negative z/w once released;
//! origin bottom-left). Zeron adds `iCamera` (vec4: yaw, pitch, zoom, 0),
//! which the reader orbits by dragging when `orbit = true`, and named number
//! inputs the script binds to its state (`inputs = { speed = s.speed }`
//! makes `speed` a float in the shader).
//!
//! Like Shadertoy's buffers, up to four extra passes (`buffers = { A, B,
//! ... }`) render first, each into its own float texture that every pass
//! reads as `iChannel0`-`iChannel3`: a later pass sees an earlier one's
//! current frame, and a pass sees its own previous frame (feedback: trails,
//! simulations, accumulation). `iChannelResolution[i]` is each channel's
//! size, and `iNoise` is a tiling 256x256 texture of independent random RGBA.
//!
//! Image assets shipped with the visual (`textures = { "rock.jpg" }`) are
//! `iTexture0`-`iTexture3`, repeating, with mipmaps.

/// At most this many named inputs (four vec4 slots).
pub const MAX_INPUTS: usize = 16;
/// Longest shader source a script may pass.
pub const MAX_SHADER_BYTES: usize = 64 * 1024;
/// Buffer passes before the image pass.
pub const MAX_BUFFERS: usize = 4;
/// Image assets a shader may sample.
pub const MAX_TEXTURES: usize = 4;

/// How a shader's output sits on the chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Blend {
    /// The alpha the shader writes (0 shows the chat through).
    Alpha,
    /// The output is light: brightness is opacity, so black is clear. For
    /// beams, glows, particles, and anything drawn as emitted light. The
    /// default: shader code habitually writes alpha 1 over a black clear
    /// color, which would otherwise be a black box on the chat.
    #[default]
    Light,
    /// Fully opaque, as on Shadertoy.
    Opaque,
}

impl Blend {
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "alpha" => Self::Alpha,
            "light" | "add" | "additive" => Self::Light,
            "opaque" => Self::Opaque,
            _ => return None,
        })
    }
}

/// What a shader frame is drawn with, in the uniform block's layout.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
#[repr(C)]
pub struct ShaderUniforms {
    /// Width, height, pixel ratio, unused.
    pub resolution: [f32; 4],
    pub mouse: [f32; 4],
    /// Yaw, pitch, zoom, unused.
    pub camera: [f32; 4],
    /// Time, time delta, frame, unused.
    pub time: [f32; 4],
    pub inputs: [[f32; 4]; MAX_INPUTS / 4],
    /// Each channel's width, height, 1, unused.
    pub channels: [[f32; 4]; 4],
}

impl ShaderUniforms {
    pub fn as_bytes(&self) -> &[u8] {
        // SAFETY: plain `f32` arrays with no padding.
        unsafe {
            std::slice::from_raw_parts(
                (self as *const Self).cast::<u8>(),
                std::mem::size_of::<Self>(),
            )
        }
    }
}

/// The complete fragment program around `code`, and how many lines precede
/// the script's own first line (to report errors at its line numbers).
///
/// Passes render rows bottom-up (fragCoord.y is the texture row), so a
/// buffer reads back exactly what it wrote, as on Shadertoy; the desktop
/// turns the final image upright.
pub fn program(code: &str, inputs: &[String]) -> (String, usize) {
    let mut header = String::from(
        "#version 450\n\
         layout(set = 0, binding = 0) uniform ZeronShader {\n\
         \x20   vec4 zeron_resolution;\n\
         \x20   vec4 iMouse;\n\
         \x20   vec4 iCamera;\n\
         \x20   vec4 zeron_time;\n\
         \x20   vec4 zeron_inputs[4];\n\
         \x20   vec4 iChannelResolution[4];\n\
         };\n\
         layout(set = 0, binding = 1) uniform texture2D zeron_channel0;\n\
         layout(set = 0, binding = 2) uniform texture2D zeron_channel1;\n\
         layout(set = 0, binding = 3) uniform texture2D zeron_channel2;\n\
         layout(set = 0, binding = 4) uniform texture2D zeron_channel3;\n\
         layout(set = 0, binding = 5) uniform texture2D zeron_noise;\n\
         layout(set = 0, binding = 6) uniform sampler zeron_clamp;\n\
         layout(set = 0, binding = 7) uniform sampler zeron_repeat;\n\
         layout(set = 0, binding = 8) uniform texture2D zeron_texture0;\n\
         layout(set = 0, binding = 9) uniform texture2D zeron_texture1;\n\
         layout(set = 0, binding = 10) uniform texture2D zeron_texture2;\n\
         layout(set = 0, binding = 11) uniform texture2D zeron_texture3;\n\
         layout(location = 0) out vec4 zeron_color;\n\
         #define iChannel0 sampler2D(zeron_channel0, zeron_clamp)\n\
         #define iChannel1 sampler2D(zeron_channel1, zeron_clamp)\n\
         #define iChannel2 sampler2D(zeron_channel2, zeron_clamp)\n\
         #define iChannel3 sampler2D(zeron_channel3, zeron_clamp)\n\
         #define iNoise sampler2D(zeron_noise, zeron_repeat)\n\
         #define iTexture0 sampler2D(zeron_texture0, zeron_repeat)\n\
         #define iTexture1 sampler2D(zeron_texture1, zeron_repeat)\n\
         #define iTexture2 sampler2D(zeron_texture2, zeron_repeat)\n\
         #define iTexture3 sampler2D(zeron_texture3, zeron_repeat)\n\
         #define iResolution (zeron_resolution.xyz)\n\
         #define iTime (zeron_time.x)\n\
         #define iTimeDelta (zeron_time.y)\n\
         #define iFrame (int(zeron_time.z))\n",
    );
    for (i, name) in inputs.iter().enumerate().take(MAX_INPUTS) {
        let lane = ["x", "y", "z", "w"][i % 4];
        header.push_str(&format!(
            "#define {name} (zeron_inputs[{}].{lane})\n",
            i / 4
        ));
    }
    let lines = header.lines().count();
    // Shadertoy sources often carry an ES precision statement; GLSL 4.50
    // has no use for it.
    let body: String = code
        .lines()
        .map(|line| {
            if line.trim_start().starts_with("precision ") {
                ""
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let program = format!(
        "{header}{body}\n\
         void main() {{\n\
         \x20   vec4 color = vec4(0.0, 0.0, 0.0, 1.0);\n\
         \x20   mainImage(color, gl_FragCoord.xy);\n\
         \x20   zeron_color = color;\n\
         }}\n"
    );
    (program, lines)
}

/// Whether `name` can be a shader input: a GLSL identifier that does not
/// shadow the built-in inputs.
pub fn valid_input_name(name: &str) -> bool {
    let mut chars = name.chars();
    let starts = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
    starts
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !name.starts_with("gl_")
        && !name.starts_with("zeron_")
        && !matches!(
            name,
            "iResolution"
                | "iTime"
                | "iTimeDelta"
                | "iFrame"
                | "iMouse"
                | "iCamera"
                | "iChannel0"
                | "iChannel1"
                | "iChannel2"
                | "iChannel3"
                | "iChannelResolution"
                | "iNoise"
                | "iTexture0"
                | "iTexture1"
                | "iTexture2"
                | "iTexture3"
                | "main"
        )
}

/// Compile `code` as the GPU will, and report the first problem at the
/// script's own line numbers.
pub fn validate(code: &str, inputs: &[String]) -> Result<(), String> {
    if code.len() > MAX_SHADER_BYTES {
        return Err(format!("a shader is at most {MAX_SHADER_BYTES} bytes"));
    }
    if !code.contains("mainImage") {
        return Err(
            "a shader defines void mainImage(out vec4 fragColor, in vec2 fragCoord)".into(),
        );
    }
    let (source, offset) = program(code, inputs);
    let module = naga::front::glsl::Frontend::default()
        .parse(
            &naga::front::glsl::Options::from(naga::ShaderStage::Fragment),
            &source,
        )
        .map_err(|errors| describe(&errors.errors, &source, offset))?;
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .map_err(|error| format!("shader: {}", error.into_inner()))?;
    Ok(())
}

fn describe(errors: &[naga::front::glsl::Error], source: &str, offset: usize) -> String {
    let Some(error) = errors.first() else {
        return "shader: failed to compile".into();
    };
    let line = error.meta.location(source).line_number as usize;
    if line > offset {
        format!("shader line {}: {}", line - offset, error.kind)
    } else {
        format!("shader: {}", error.kind)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RINGS: &str = "void mainImage(out vec4 fragColor, in vec2 fragCoord) {\n\
        vec2 uv = fragCoord / iResolution.xy;\n\
        float d = length(uv - 0.5);\n\
        fragColor = vec4(vec3(0.5 + 0.5 * sin(d * 40.0 - iTime * speed)), 1.0);\n\
    }";

    #[test]
    fn a_shadertoy_style_shader_compiles_with_named_inputs() {
        validate(RINGS, &["speed".into()]).unwrap();
    }

    #[test]
    fn errors_point_at_the_scripts_own_line() {
        let broken = "void mainImage(out vec4 fragColor, in vec2 fragCoord) {\n\
            vec2 uv = fragCoord / iResolution.xy;\n\
            fragColor = vec4(uv, nope, 1.0);\n\
        }";
        let error = validate(broken, &[]).unwrap_err();
        assert!(error.starts_with("shader line 3:"), "{error}");
        // An undeclared input is the same mistake.
        assert!(validate(RINGS, &[]).is_err());
    }

    #[test]
    fn passes_sample_channels_and_noise_as_on_shadertoy() {
        let code = "void mainImage(out vec4 fragColor, in vec2 fragCoord) {\n\
            vec2 uv = fragCoord / iResolution.xy;\n\
            vec4 last = texture(iChannel0, uv);\n\
            vec4 n = texture(iNoise, uv * 4.0);\n\
            vec4 exact = texelFetch(iChannel1, ivec2(fragCoord), 0);\n\
            fragColor = last * 0.98 + n * 0.02 + exact * iChannelResolution[1].x * 0.0;\n\
        }";
        validate(code, &[]).unwrap();
    }

    #[test]
    fn es_precision_lines_are_tolerated() {
        let code = format!("precision highp float;\n{RINGS}");
        validate(&code, &["speed".into()]).unwrap();
    }

    #[test]
    fn input_names_are_identifiers_that_shadow_nothing() {
        assert!(valid_input_name("speed"));
        assert!(valid_input_name("_k2"));
        assert!(!valid_input_name("2k"));
        assert!(!valid_input_name("iTime"));
        assert!(!valid_input_name("gl_Position"));
        assert!(!valid_input_name("zeron_x"));
    }
}
