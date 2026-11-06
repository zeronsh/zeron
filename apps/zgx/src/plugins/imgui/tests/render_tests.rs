//! The renderer against a real GPU: pixels, device rebuilds, resource reuse
//! and drawing into a `ViewportFrame`.

use dear_imgui_rs::{Condition, Ui};
use zgx_gpu::viewport::{Viewport, ViewportFrame};
use zgx_gpu::wgpu;

use super::support::{TestGpu, block_on, headless_gpu, imgui_guard};
use crate::plugins::imgui::render::{ImguiViewport, WgpuRenderer};
use crate::plugins::imgui::runtime::{FrameSpec, WindowContext};
use crate::plugins::imgui::snapshot::{self, OffscreenTarget, Snapshot};

const SIZE: (u32, u32) = (800, 600);
const CLEAR: [u8; 4] = [26, 28, 33, 255];

fn spec(size: (u32, u32)) -> FrameSpec {
    FrameSpec::from_device_pixels(size, 1.0)
}

/// A filled rectangle, text and a window: enough of what a plugin draws that
/// a blank frame cannot pass.
fn probe(ui: &Ui, frame: u32) {
    let draw = ui.get_background_draw_list();
    draw.add_rect([40.0, 40.0], [240.0, 140.0], [0.90, 0.15, 0.15, 1.0])
        .filled(true)
        .build();
    draw.add_text(
        [40.0, 300.0],
        [1.0, 1.0, 1.0, 1.0],
        "Dear ImGui renders this text",
    );
    ui.window("probe")
        .position([32.0, 360.0], Condition::Always)
        .size([320.0, 120.0], Condition::Always)
        .build(|| {
            ui.text(format!("frame {frame}"));
            let _ = ui.button("Click me");
        });
}

/// Pixels in `(left, top)..(right, bottom)` that differ from `background`.
fn differing_pixels(
    image: &Snapshot,
    left: u32,
    top: u32,
    right: u32,
    bottom: u32,
    background: [u8; 4],
) -> usize {
    let mut count = 0;
    for y in top..bottom.min(image.height()) {
        for x in left..right.min(image.width()) {
            if image.pixel(x, y) != Some(background) {
                count += 1;
            }
        }
    }
    count
}

/// A temporary directory for evidence PNGs, kept out of the repository.
fn evidence_dir() -> std::path::PathBuf {
    std::env::temp_dir().join("zeron-zgx-imgui-tests")
}

#[test]
fn offscreen_frame_draws_text_and_a_filled_rect() {
    let _guard = imgui_guard();
    let gpu = headless_gpu();
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut window = WindowContext::try_create("render probe").expect("a context");
    let mut renderer = WgpuRenderer::new(gpu.device.clone(), gpu.queue.clone(), format);
    let target = OffscreenTarget::new(&gpu.device, SIZE, format);
    let scope = gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);

    // The first frame bakes the glyphs; the second draws them from the atlas
    // uploaded with the first. Three frames is the spike's proven minimum for
    // a settled picture.
    let mut image = None;
    for frame in 0..3 {
        let prepared = window
            .prepare(spec(SIZE), true, |ui| probe(ui, frame))
            .expect("a prepared frame");
        assert!(
            prepared.atlas().is_some(),
            "frame {frame} should carry the atlas"
        );
        let (_, snapshot) =
            snapshot::render(&mut renderer, &gpu.device, &gpu.queue, &target, &prepared)
                .expect("an offscreen frame");
        image = Some(snapshot);
    }
    if let Some(error) = block_on(scope.pop()) {
        panic!("wgpu validation error while drawing Dear ImGui: {error}");
    }
    let image = image.expect("three frames were drawn");

    let path = evidence_dir().join("offscreen-text-and-rect.png");
    image.save_png(&path).expect("the evidence PNG");
    println!(
        "imgui offscreen: {} non-background px, {} colours -> {}",
        image.non_background_pixels(),
        image.distinct_colours(),
        path.display()
    );

    assert_eq!(image.width(), SIZE.0);
    assert_eq!(image.height(), SIZE.1);
    assert!(
        image.non_background_pixels() > 8_000,
        "expected a painted frame, got {} non-background pixels",
        image.non_background_pixels()
    );
    assert!(
        image.distinct_colours() > 10,
        "expected antialiased text and shaded chrome, got {} distinct colours",
        image.distinct_colours()
    );
    assert!(
        image.near(100, 80, [230, 38, 38, 255], 12),
        "the filled rectangle is not the colour authored: {:?}",
        image.pixel(100, 80)
    );
    let text = differing_pixels(&image, 40, 295, 340, 320, CLEAR);
    assert!(
        text > 100,
        "expected glyphs on the background draw list, got {text} differing pixels"
    );

    let counters = renderer.counters();
    assert_eq!(counters.live_buffers, 2, "vertices and indices");
    assert_eq!(counters.live_textures, 1, "the font atlas");
    assert_eq!(window.frames(), 3);
    assert!(!window.is_active(), "a frame leaves the context suspended");
}

/// 2000 text items, the workload `VAL-PERF-010` names.
fn heavy_workload(ui: &Ui, items: &[String]) {
    let draw = ui.get_background_draw_list();
    let mut index = 0;
    for row in 0..50 {
        for column in 0..40 {
            let position = [4.0 + column as f32 * 30.0, 4.0 + row as f32 * 14.0];
            draw.add_text(position, [1.0, 1.0, 1.0, 1.0], &items[index]);
            index += 1;
        }
    }
}

fn paint(
    window: &mut WindowContext,
    renderer: &mut WgpuRenderer,
    gpu: &TestGpu,
    target: &OffscreenTarget,
    items: &[String],
) {
    let prepared = window
        .prepare(spec(target.size()), renderer.needs_atlas(), |ui| {
            heavy_workload(ui, items)
        })
        .expect("a prepared frame");
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("imgui churn test"),
        });
    renderer
        .render(&mut encoder, target.render_target(), &prepared)
        .expect("a drawn frame");
    gpu.queue.submit([encoder.finish()]);
}

#[test]
fn steady_state_creates_and_releases_no_gpu_resource() {
    let _guard = imgui_guard();
    let gpu = headless_gpu();
    let size = (1280, 720);
    let format = wgpu::TextureFormat::Bgra8Unorm;
    let mut window = WindowContext::try_create("churn").expect("a context");
    let mut renderer = WgpuRenderer::new(gpu.device.clone(), gpu.queue.clone(), format);
    let target = OffscreenTarget::new(&gpu.device, size, format);
    let items: Vec<String> = (0..2000).map(|index| format!("item {index}")).collect();

    // Warm-up: the pipeline, both buffers at their final size and the atlas
    // are made here.
    for _ in 0..30 {
        paint(&mut window, &mut renderer, &gpu, &target, &items);
    }
    gpu.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("the GPU caught up");
    let at_frame_30 = renderer.counters();
    let frames_at_30 = renderer.frames();
    assert_eq!(at_frame_30.live_buffers, 2, "vertex and index buffers");
    assert_eq!(at_frame_30.live_textures, 1, "the font atlas");

    // 270 more frames: the window `VAL-PERF-011` measures.
    for index in 0..270 {
        paint(&mut window, &mut renderer, &gpu, &target, &items);
        if index % 10 == 9 {
            gpu.device
                .poll(wgpu::PollType::wait_indefinitely())
                .expect("the GPU caught up");
        }
    }
    gpu.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("the GPU caught up");
    let at_frame_300 = renderer.counters();
    let frames_at_300 = renderer.frames();
    println!(
        "imgui churn (frames {}..{}): buffers created {} -> {}, destroyed {} -> {}; \
         textures created {} -> {}, destroyed {} -> {}; live {} buffers, {} textures",
        frames_at_30,
        frames_at_300,
        at_frame_30.buffers_created,
        at_frame_300.buffers_created,
        at_frame_30.buffers_destroyed,
        at_frame_300.buffers_destroyed,
        at_frame_30.textures_created,
        at_frame_300.textures_created,
        at_frame_30.textures_destroyed,
        at_frame_300.textures_destroyed,
        at_frame_300.live_buffers,
        at_frame_300.live_textures,
    );
    assert_eq!(frames_at_300 - frames_at_30, 270);
    assert_eq!(
        at_frame_300.buffers_created, at_frame_30.buffers_created,
        "a steady frame must not create a buffer"
    );
    assert_eq!(
        at_frame_300.buffers_destroyed, at_frame_30.buffers_destroyed,
        "a steady frame must not release a buffer"
    );
    assert_eq!(
        at_frame_300.textures_created, at_frame_30.textures_created,
        "a steady frame must not create a texture"
    );
    assert_eq!(
        at_frame_300.textures_destroyed, at_frame_30.textures_destroyed,
        "a steady frame must not release a texture"
    );
    assert_eq!(at_frame_300.live_buffers, 2);
    assert_eq!(at_frame_300.live_textures, 1);

    // The 270th frame still draws (the counters alone could come from 270
    // empty frames).
    let last = window
        .prepare(spec(size), false, |ui| heavy_workload(ui, &items))
        .expect("a prepared frame");
    let (stats, image) = snapshot::render(&mut renderer, &gpu.device, &gpu.queue, &target, &last)
        .expect("the last frame draws");
    assert!(stats.draw_calls > 0);
    assert!(stats.vertices > 0);
    assert!(image.non_background_pixels() > 10_000);
}

#[test]
fn device_changed_drops_and_rebuilds_every_gpu_object() {
    let _guard = imgui_guard();
    let gpu = headless_gpu();
    let size = (400, 300);
    let format = wgpu::TextureFormat::Bgra8Unorm;
    let mut window = WindowContext::try_create("device change").expect("a context");
    let mut renderer = WgpuRenderer::new(gpu.device.clone(), gpu.queue.clone(), format);
    let target = OffscreenTarget::new(&gpu.device, size, format);

    let draw = |renderer: &mut WgpuRenderer,
                device: &wgpu::Device,
                queue: &wgpu::Queue,
                target: &OffscreenTarget,
                window: &mut WindowContext| {
        let prepared = window
            .prepare(spec(size), renderer.needs_atlas(), |ui| probe(ui, 1))
            .expect("a prepared frame");
        let (_, image) = snapshot::render(renderer, device, queue, target, &prepared)
            .expect("an offscreen frame");
        image
    };

    let first = draw(&mut renderer, &gpu.device, &gpu.queue, &target, &mut window);
    assert!(first.non_background_pixels() > 1_000);
    let before = renderer.counters();
    assert_eq!(before.live_buffers, 2);
    assert_eq!(before.live_textures, 1);

    renderer.device_changed();
    let dropped = renderer.counters();
    assert_eq!(dropped.live_buffers, 0, "every buffer is gone");
    assert_eq!(dropped.live_textures, 0, "every texture is gone");
    assert_eq!(dropped.buffers_destroyed, dropped.buffers_created);
    assert_eq!(dropped.textures_destroyed, dropped.textures_created);
    assert!(renderer.needs_atlas(), "the atlas must be sent again");

    // A genuinely new device: the rebuild must use it, not the old handle.
    let (device, queue) = gpu.second_device();
    renderer.rebind(device.clone(), queue.clone());
    let second_target = OffscreenTarget::new(&device, size, format);
    let second = draw(&mut renderer, &device, &queue, &second_target, &mut window);
    assert!(
        second.non_background_pixels() > 1_000,
        "the rebuilt renderer drew nothing"
    );
    assert!(second.near(100, 80, [230, 38, 38, 255], 12));
    let rebuilt = renderer.counters();
    assert_eq!(rebuilt.live_buffers, 2);
    assert_eq!(rebuilt.live_textures, 1);
    assert!(rebuilt.buffers_created > dropped.buffers_created);
    assert!(rebuilt.textures_created > dropped.textures_created);

    // The view's own path: dropping the renderer and making another one is
    // what `Viewport::device_changed` does, and it must not panic either.
    let mut viewport = ImguiViewport::new();
    viewport.set_prepared(
        window
            .prepare(spec(size), true, |ui| probe(ui, 2))
            .expect("a prepared frame"),
    );
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("viewport device change"),
    });
    {
        let mut frame = ViewportFrame::new(
            &device,
            &queue,
            &mut encoder,
            second_target.view(),
            format,
            size,
            1.0,
        );
        viewport.render(&mut frame);
    }
    queue.submit([encoder.finish()]);
    assert!(viewport.renderer().is_some());
    viewport.device_changed();
    assert!(viewport.renderer().is_none());
}

#[test]
fn a_prepared_frame_renders_through_a_viewport_frame_target() {
    let _guard = imgui_guard();
    let gpu = headless_gpu();
    let format = wgpu::TextureFormat::Bgra8Unorm;
    let mut window = WindowContext::try_create("viewport").expect("a context");
    let renderer = WgpuRenderer::new(gpu.device.clone(), gpu.queue.clone(), format);
    let target = OffscreenTarget::new(&gpu.device, SIZE, format);

    let prepared = window
        .prepare(spec(SIZE), renderer.needs_atlas(), |ui| probe(ui, 1))
        .expect("a prepared frame");

    // The paint half on its own: no context, exactly the `ViewportFrame` zgx
    // hands a view.
    let mut viewport = ImguiViewport::new();
    viewport.set_prepared(prepared);
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("viewport frame"),
        });
    {
        let mut frame = ViewportFrame::new(
            &gpu.device,
            &gpu.queue,
            &mut encoder,
            target.view(),
            format,
            SIZE,
            1.0,
        );
        viewport.render(&mut frame);
    }
    gpu.queue.submit([encoder.finish()]);
    let image = target.read(&gpu.device, &gpu.queue);
    let path = evidence_dir().join("viewport-frame.png");
    image.save_png(&path).expect("the evidence PNG");
    println!(
        "imgui viewport frame: {} non-background px -> {}",
        image.non_background_pixels(),
        path.display()
    );

    assert!(
        image.non_background_pixels() > 8_000,
        "a ViewportFrame target stayed blank"
    );
    assert!(image.near(100, 80, [230, 38, 38, 255], 12));
    let text = differing_pixels(&image, 40, 295, 340, 320, CLEAR);
    assert!(text > 100, "no glyphs reached the ViewportFrame target");
    assert_eq!(viewport.take_error(), None);

    // What the view hands a `ViewportFrame` must be usable as its target,
    // and the view's renderer must be the one built from that frame.
    let renderer = viewport.renderer().expect("the view made a renderer");
    assert_eq!(renderer.format(), format);
    assert_eq!(renderer.counters().live_buffers, 2);
}
