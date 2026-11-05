//! Render a `ui.shader` pass headlessly, composited over a chat-like
//! background, to check how it will look:
//!
//! `cargo run -p zeron-visual-gpu --example render_shader -- shader.glsl out.ppm \
//!    [--size 1440x840] [--blend light|alpha|opaque] [--bloom 1] [--time 2] \
//!    [--input name=value ...] [--background r,g,b]`

use zeron_visual::shader::{Blend, ShaderUniforms};
use zeron_visual_gpu::{ShaderSpec, render_shader};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (source, out) = (&args[0], &args[1]);
    let code = std::fs::read_to_string(source).expect("shader source");
    let (mut w, mut h, mut blend, mut bloom, mut time) = (1440u32, 840u32, Blend::Light, 0.0, 2.0);
    let mut inputs: Vec<(String, f32)> = Vec::new();
    let mut background = [0.80f32, 0.83, 0.81];
    let mut i = 2;
    while i < args.len() {
        let value = args.get(i + 1).cloned().unwrap_or_default();
        match args[i].as_str() {
            "--size" => {
                let (a, b) = value.split_once('x').unwrap();
                (w, h) = (a.parse().unwrap(), b.parse().unwrap());
            }
            "--blend" => blend = Blend::parse(&value).unwrap(),
            "--bloom" => bloom = value.parse().unwrap(),
            "--time" => time = value.parse().unwrap(),
            "--input" => {
                let (name, v) = value.split_once('=').unwrap();
                inputs.push((name.to_owned(), v.parse().unwrap()));
            }
            "--background" => {
                let parts: Vec<f32> = value.split(',').map(|p| p.parse().unwrap()).collect();
                background = [parts[0], parts[1], parts[2]];
            }
            other => panic!("unknown flag {other}"),
        }
        i += 2;
    }
    inputs.sort_by(|a, b| a.0.cmp(&b.0));
    let names: Vec<String> = inputs.iter().map(|(n, _)| n.clone()).collect();
    let mut slots = [[0.0f32; 4]; 4];
    for (k, (_, v)) in inputs.iter().enumerate() {
        slots[k / 4][k % 4] = *v;
    }
    let uniforms = ShaderUniforms {
        resolution: [w as f32, h as f32, 2.0, 0.0],
        mouse: [0.0, 0.0, -1.0, -1.0],
        camera: [0.3, 0.45, 1.0, 0.0],
        time: [time, 0.016, 60.0, 0.0],
        inputs: slots,
        channels: Default::default(),
    };
    let spec = ShaderSpec {
        image: &code,
        buffers: &[],
        inputs: &names,
        blend,
        bloom,
        textures: &[],
    };
    let frame = render_shader(&spec, &uniforms, w, h, &mut None).expect("render");
    let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
    for p in frame.bgra.chunks_exact(4) {
        let a = f32::from(p[3]) / 255.0;
        for (ch, bg) in [p[2], p[1], p[0]].into_iter().zip(background) {
            ppm.push((f32::from(ch) * a + bg * 255.0 * (1.0 - a)) as u8);
        }
    }
    std::fs::write(out, ppm).expect("write");
}
