//! One `ui.shader`'s frames: rendered one at a time on a background thread
//! with `zeron_visual_gpu`, the newest shown meanwhile. Agent visuals and
//! the generative new-thread background share it.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{App, RenderImage};
use zeron_visual::shader::{Blend, ShaderUniforms};

/// What a shader runs, apart from each frame's uniforms.
pub(crate) struct ShaderProgram {
    pub code: String,
    pub buffers: Vec<String>,
    /// Named float inputs, in slot order.
    pub inputs: Vec<String>,
    pub blend: Blend,
    pub bloom: f32,
    pub textures: Vec<zeron_visual::TextureData>,
    pub fade_edges: bool,
}

/// A shader's newest finished frame and the GPU state carried between
/// frames (feedback buffers, simulations).
#[derive(Default)]
pub(crate) struct ShaderPlayer {
    /// The newest frame and the uniforms it was drawn with.
    shown: Option<(ShaderUniforms, Arc<RenderImage>)>,
    /// Frames started, and the wall time the last one was started at.
    frame: u32,
    last_time: f64,
    /// Away on the render thread while a frame renders.
    gpu: Option<zeron_visual_gpu::ShaderState>,
    rendering: bool,
}

pub(crate) type SharedShaderPlayer = Rc<RefCell<ShaderPlayer>>;

impl ShaderPlayer {
    pub fn shown(&self) -> Option<&(ShaderUniforms, Arc<RenderImage>)> {
        self.shown.as_ref()
    }

    pub fn image(&self) -> Option<Arc<RenderImage>> {
        self.shown.as_ref().map(|(_, image)| image.clone())
    }

    /// Frames started so far (`iFrame` of the next one).
    pub fn frames(&self) -> u32 {
        self.frame
    }

    /// When the last frame was started, for the next one's `iTimeDelta`.
    pub fn last_time(&self) -> f64 {
        self.last_time
    }

    pub fn rendering(&self) -> bool {
        self.rendering
    }

    /// Give up the shown frame, for its owner to free.
    pub fn take_image(&mut self) -> Option<Arc<RenderImage>> {
        self.shown.take().map(|(_, image)| image)
    }

    /// Render a frame of `program` at `size` device pixels unless one is
    /// already in flight (then this returns false). `time` is the wall time
    /// the frame starts at. When it lands it replaces the shown frame (the
    /// old one is freed) and `landed` runs with the outcome; a player
    /// dropped meanwhile frees the frame instead and `landed` doesn't run.
    pub fn render(
        player: &SharedShaderPlayer,
        program: Arc<ShaderProgram>,
        uniforms: ShaderUniforms,
        size: (u32, u32),
        time: f64,
        cx: &mut App,
        landed: impl FnOnce(Result<(), String>, &mut App) + 'static,
    ) -> bool {
        let mut gpu_state = {
            let mut this = player.borrow_mut();
            if this.rendering {
                return false;
            }
            this.rendering = true;
            this.frame += 1;
            this.last_time = time;
            this.gpu.take()
        };
        let render = cx.background_executor().spawn(async move {
            let rendered = zeron_visual_gpu::render_shader(
                &zeron_visual_gpu::ShaderSpec {
                    image: &program.code,
                    buffers: &program.buffers,
                    inputs: &program.inputs,
                    blend: program.blend,
                    bloom: program.bloom,
                    textures: &program.textures,
                    fade_edges: program.fade_edges,
                },
                &uniforms,
                size.0,
                size.1,
                &mut gpu_state,
            )
            .and_then(|frame| {
                crate::visuals::render_image(frame.width, frame.height, frame.bgra)
                    .ok_or_else(|| "the shader's frame was malformed".to_owned())
            });
            (rendered, gpu_state)
        });
        let player = Rc::downgrade(player);
        cx.spawn(async move |cx| {
            let (rendered, gpu_state) = render.await;
            cx.update(|cx| {
                let Some(player) = player.upgrade() else {
                    if let Ok(image) = rendered {
                        crate::visuals::free_images(vec![image], cx);
                    }
                    return;
                };
                let outcome = {
                    let mut this = player.borrow_mut();
                    this.rendering = false;
                    this.gpu = gpu_state;
                    match rendered {
                        Ok(image) => {
                            let old = this.shown.replace((uniforms, image));
                            drop(this);
                            crate::visuals::free_images(
                                old.map(|(_, image)| image).into_iter().collect(),
                                cx,
                            );
                            Ok(())
                        }
                        Err(error) => Err(error),
                    }
                };
                landed(outcome, cx);
            })
        })
        .detach();
        true
    }
}
