//! The generative new-thread background: the user's Luau `ui.shader`,
//! rendered at the background's size and painted through the same composer
//! mask, fades, and crossfade as an image. It moves (time, spin, feedback
//! buffers) but takes no input: it is art, not a control, so `iMouse`
//! stays at rest and nothing is clickable.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{App, Context, RenderImage, Task};
use zeron_visual::{Node, NodeKind, ShaderNode};

use crate::shader_player::{ShaderPlayer, ShaderProgram, SharedShaderPlayer};

/// Seeds the script's randomness, so the check and the background agree.
pub(crate) const GENERATIVE_ID: &str = "new-thread-generative";
/// Frames per second while the art moves.
const FPS: f64 = 30.0;
/// Frames render at one pixel per logical pixel: sharp on a 1x display, half
/// the device's on a 2x one, where the art's soft fades hide it, and every
/// frame is read back from the GPU.
const MAX_DENSITY: f32 = 1.0;
/// The most pixels in a frame (about 2560 by 1600); bigger backgrounds scale
/// down to it.
const MAX_PIXELS: f32 = 4_096_000.0;
/// When the art's color is sampled for "Match wallpaper colors".
const SAMPLE_TIME: f32 = 2.0;

/// The shader a script shows as the background: the first in its tree.
pub(crate) fn first_shader(node: &Node) -> Option<&ShaderNode> {
    if let NodeKind::Shader(shader) = &node.kind {
        return Some(shader);
    }
    zeron_visual::children(node).find_map(first_shader)
}

fn program(shader: &ShaderNode) -> ShaderProgram {
    ShaderProgram {
        code: shader.code.clone(),
        buffers: shader.buffers.clone(),
        inputs: shader.inputs.iter().map(|(name, _)| name.clone()).collect(),
        blend: shader.blend,
        bloom: shader.bloom,
        // A background has no assets; the check rejects a script naming any.
        textures: Vec::new(),
        // The background's own mask fades it.
        fade_edges: false,
    }
}

fn inputs(shader: &ShaderNode) -> [[f32; 4]; zeron_visual::shader::MAX_INPUTS / 4] {
    let mut inputs = [[0.0f32; 4]; zeron_visual::shader::MAX_INPUTS / 4];
    for (i, (_, value)) in shader.inputs.iter().enumerate() {
        inputs[i / 4][i % 4] = *value;
    }
    inputs
}

fn uniforms(
    shader: &ShaderNode,
    size: (u32, u32),
    density: f32,
    time: f32,
    delta: f32,
    frame: u32,
) -> zeron_visual::shader::ShaderUniforms {
    zeron_visual::shader::ShaderUniforms {
        resolution: [size.0 as f32, size.1 as f32, density, 0.0],
        // At rest: the background takes no pointer.
        mouse: [0.0, 0.0, -1.0, -1.0],
        camera: [
            shader.yaw + shader.spin * time,
            shader.pitch,
            shader.zoom,
            0.0,
        ],
        time: [time, delta, frame as f32, 0.0],
        inputs: inputs(shader),
        channels: Default::default(),
    }
}

/// Check a script for the generative background the way a visual is checked,
/// and require a shader to show. On success, the color of one of its frames
/// (none without a GPU). Blocking: run it off the main thread.
pub(crate) fn check(script: &str) -> Result<Option<zeron_theme::Color>, String> {
    let seed = zeron_visual::seed_for(GENERATIVE_ID);
    zeron_visual::check(script, seed).map_err(|error| error.message)?;
    let tree = zeron_visual::Visual::load(script, seed)
        .and_then(|mut visual| visual.render())
        .map_err(|error| error.message)?;
    let shader = first_shader(&tree).ok_or_else(|| {
        "a generative background is a ui.shader: return one from the view function".to_owned()
    })?;
    if !shader.textures.is_empty() {
        return Err("a generative background has no assets: remove the shader's textures".into());
    }
    let size = (96, 54);
    let sample = zeron_visual_gpu::render_shader(
        &zeron_visual_gpu::ShaderSpec {
            image: &shader.code,
            buffers: &shader.buffers,
            inputs: &program(shader).inputs,
            blend: shader.blend,
            bloom: shader.bloom,
            textures: &[],
            fade_edges: false,
        },
        &uniforms(shader, size, 1.0, SAMPLE_TIME, 0.0, 0),
        size.0,
        size.1,
        &mut None,
    );
    Ok(sample.ok().and_then(|frame| {
        crate::settings::wallpaper_colors::extract(
            frame.bgra.chunks_exact(4).map(|p| [p[2], p[1], p[0], p[3]]),
        )
    }))
}

/// A running generative background.
pub(crate) struct GenerativeBackground {
    script: String,
    /// Identifies this art to the background's crossfade.
    key: std::path::PathBuf,
    shader: Option<Arc<ShaderNode>>,
    program: Option<Arc<ShaderProgram>>,
    player: SharedShaderPlayer,
    started: Instant,
    /// The art can't show here (no GPU, or a script that stopped loading);
    /// the image shows instead. A failed frame is not retried every frame.
    failed: Rc<Cell<bool>>,
    /// Wakes the owner when the next frame is due.
    wake: Option<Task<()>>,
}

impl GenerativeBackground {
    pub fn new(script: &str) -> Self {
        let seed = zeron_visual::seed_for(GENERATIVE_ID);
        let shader = zeron_visual::Visual::load(script, seed)
            .and_then(|mut visual| visual.render())
            .map_err(|error| tracing::warn!(%error, "generative background failed to load"))
            .ok()
            .and_then(|tree| first_shader(&tree).cloned())
            .map(Arc::new);
        let key = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            script.hash(&mut hasher);
            format!("generative:{:016x}", hasher.finish()).into()
        };
        Self {
            script: script.to_owned(),
            key,
            program: shader.as_deref().map(program).map(Arc::new),
            failed: Rc::new(Cell::new(shader.is_none())),
            shader,
            player: Default::default(),
            started: Instant::now(),
            wake: None,
        }
    }

    pub fn script(&self) -> &str {
        &self.script
    }

    pub fn key(&self) -> &std::path::Path {
        &self.key
    }

    pub fn failed(&self) -> bool {
        self.failed.get()
    }

    /// The newest frame for a background `size` logical pixels big. Starts
    /// the next frame when it's due, or wakes the owner (`cx`'s entity) when
    /// it will be; the owner calls this again only while it paints the
    /// background, so a hidden background stops rendering. `still` (reduced
    /// motion, or paused in the background) holds the time last shown.
    pub fn frame<T: 'static>(
        &mut self,
        size: (f32, f32),
        scale_factor: f32,
        still: bool,
        cx: &mut Context<T>,
    ) -> Option<Arc<RenderImage>> {
        self.wake = None;
        let (Some(shader), Some(program)) = (self.shader.clone(), self.program.clone()) else {
            return None;
        };
        let density = scale_factor.min(MAX_DENSITY);
        let mut pixels = (size.0 * density, size.1 * density);
        let area = pixels.0 * pixels.1;
        if area > MAX_PIXELS {
            let shrink = (MAX_PIXELS / area).sqrt();
            pixels = (pixels.0 * shrink, pixels.1 * shrink);
        }
        let pixels = (
            pixels.0.round().max(1.0) as u32,
            pixels.1.round().max(1.0) as u32,
        );
        let moving = !still && (shader.animate || shader.spin != 0.0 || !shader.buffers.is_empty());
        let now = self.started.elapsed().as_secs_f64();
        let player = self.player.borrow();
        let shown = player.image();
        let time = if moving {
            now as f32
        } else {
            player.shown().map_or(now as f32, |(u, _)| u.time[0])
        };
        let current = player.shown().is_some_and(|(u, _)| {
            u.resolution[..2] == [pixels.0 as f32, pixels.1 as f32]
                && (!moving || u.time[0] == time)
        });
        if self.failed.get() || player.rendering() || current {
            return shown;
        }
        let first = player.frames() == 0;
        let wait = if first {
            0.0
        } else {
            1.0 / FPS - (now - player.last_time())
        };
        if wait > 0.0 {
            self.wake = Some(cx.spawn(async move |entity, cx| {
                cx.background_executor()
                    .timer(Duration::from_secs_f64(wait))
                    .await;
                let _ = entity.update(cx, |_, cx| cx.notify());
            }));
            return shown;
        }
        let delta = if first {
            0.0
        } else {
            (now - player.last_time()) as f32
        };
        let uniforms = uniforms(&shader, pixels, density, time, delta, player.frames());
        drop(player);
        let (entity, failed) = (cx.entity().downgrade(), self.failed.clone());
        ShaderPlayer::render(
            &self.player,
            program,
            uniforms,
            pixels,
            now,
            cx,
            move |outcome, cx: &mut App| {
                if let Err(error) = outcome {
                    tracing::warn!(%error, "generative background frame failed");
                    failed.set(true);
                }
                let _ = entity.update(cx, |_, cx| cx.notify());
            },
        );
        shown
    }

    /// Give up the shown frame, for the owner to free with the background.
    pub fn take_image(&mut self) -> Option<Arc<RenderImage>> {
        self.wake = None;
        self.player.borrow_mut().take_image()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHADER: &str =
        "void mainImage(out vec4 c, in vec2 p) { c = vec4(p / iResolution.xy, 0.5, 1.0); }";

    #[test]
    fn the_first_shader_in_the_tree_is_the_background() {
        let script = format!(
            "return function() return ui.column {{ ui.text \"Art\", \
             ui.shader {{ height = 80, code = [[{SHADER}]] }} }} end"
        );
        let tree = zeron_visual::Visual::load(&script, 1)
            .and_then(|mut visual| visual.render())
            .unwrap();
        assert_eq!(first_shader(&tree).unwrap().code, SHADER);
        assert!(check(&script).is_ok());
    }

    #[test]
    fn a_script_without_a_shader_is_not_a_background() {
        let error = check("return function() return ui.text \"Hi\" end").unwrap_err();
        assert!(error.contains("ui.shader"), "{error}");
        let error = check("return function() return ui.nope {} end").unwrap_err();
        assert!(error.contains("visual:1"), "{error}");
    }

    #[test]
    fn the_background_takes_no_pointer_and_spins_on_its_own() {
        let tree = zeron_visual::Visual::load(
            &format!(
                "return function() return ui.shader {{ height = 80, spin = 0.5, \
                 code = [[{SHADER}]] }} end"
            ),
            1,
        )
        .and_then(|mut visual| visual.render())
        .unwrap();
        let shader = first_shader(&tree).unwrap();
        let at = |time| uniforms(shader, (40, 20), 1.0, time, 0.0, 0);
        assert_eq!(at(0.0).mouse, [0.0, 0.0, -1.0, -1.0]);
        assert!((at(2.0).camera[0] - at(0.0).camera[0] - 1.0).abs() < 1e-6);
        assert!(!program(shader).fade_edges);
    }
}
