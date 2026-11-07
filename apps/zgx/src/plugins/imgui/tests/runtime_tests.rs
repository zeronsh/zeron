//! Two plugin windows in one process: each has its own context, and they
//! take turns through `suspend`/`activate`.

use dear_imgui_rs::Ui;
use zgx_gpu::wgpu;

use super::support::{headless_gpu, imgui_guard};
use crate::plugins::imgui::render::WgpuRenderer;
use crate::plugins::imgui::runtime::{FrameSpec, WindowContext};
use crate::plugins::imgui::snapshot::{self, OffscreenTarget, Snapshot};

const SIZE: (u32, u32) = (320, 240);
const RED: [f32; 4] = [0.85, 0.25, 0.15, 1.0];
const BLUE: [f32; 4] = [0.15, 0.45, 0.90, 1.0];

/// A panel of `colour` with its name in it, so two contexts visibly differ.
fn panel(ui: &Ui, colour: [f32; 4], name: &str) {
    let draw = ui.get_background_draw_list();
    draw.add_rect([20.0, 20.0], [300.0, 220.0], colour)
        .filled(true)
        .build();
    draw.add_text([32.0, 32.0], [1.0, 1.0, 1.0, 1.0], name);
}

#[test]
fn two_contexts_render_through_suspend_and_activate() {
    let _guard = imgui_guard();
    let gpu = headless_gpu();
    let format = wgpu::TextureFormat::Bgra8Unorm;

    // Both are created suspended: creating the second never disturbs the
    // first, and neither is the process's active context yet.
    let mut first = WindowContext::try_create("plugin A").expect("first context");
    let mut second = WindowContext::try_create("plugin B").expect("second context");
    assert!(!first.is_active());
    assert!(!second.is_active());
    assert_ne!(first.id(), second.id());

    // Only one may be active; the refusal keeps the second's context.
    first.activate().expect("first takes its turn");
    assert!(first.is_active());
    assert!(second.activate().is_err(), "two contexts cannot be active");
    assert!(!second.is_active());
    assert!(
        second.suspend().is_ok(),
        "suspending a suspended window is a no-op"
    );
    first.suspend().expect("first yields");
    assert!(!first.is_active());

    let mut first_renderer = WgpuRenderer::new(gpu.device.clone(), gpu.queue.clone(), format);
    let mut second_renderer = WgpuRenderer::new(gpu.device.clone(), gpu.queue.clone(), format);
    let first_target = OffscreenTarget::new(&gpu.device, SIZE, format);
    let second_target = OffscreenTarget::new(&gpu.device, SIZE, format);
    let spec = FrameSpec::from_device_pixels(SIZE, 1.0);

    let paint = |window: &mut WindowContext,
                 renderer: &mut WgpuRenderer,
                 target: &OffscreenTarget,
                 colour: [f32; 4],
                 name: &str|
     -> Snapshot {
        let prepared = window
            .prepare(spec, renderer.needs_atlas(), |ui| panel(ui, colour, name))
            .expect("a prepared frame");
        let (_, image) = snapshot::render(renderer, &gpu.device, &gpu.queue, target, &prepared)
            .expect("an offscreen frame");
        image
    };

    // A, B, then A again: the first context resumes with its own widgets,
    // style and atlas after the second has drawn.
    let a1 = paint(
        &mut first,
        &mut first_renderer,
        &first_target,
        RED,
        "Plugin A",
    );
    let b1 = paint(
        &mut second,
        &mut second_renderer,
        &second_target,
        BLUE,
        "Plugin B",
    );
    let a2 = paint(
        &mut first,
        &mut first_renderer,
        &first_target,
        RED,
        "Plugin A",
    );

    assert!(!first.is_active() && !second.is_active());
    assert_eq!(first.frames(), 2);
    assert_eq!(second.frames(), 1);

    assert!(a1.non_background_pixels() > 1_000, "A drew nothing");
    assert!(b1.non_background_pixels() > 1_000, "B drew nothing");
    assert!(
        a1.near(60, 60, [217, 64, 38, 255], 12),
        "A's accent is not the one it authored: {:?}",
        a1.pixel(60, 60)
    );
    assert!(
        b1.near(60, 60, [38, 115, 230, 255], 12),
        "B's accent is not the one it authored: {:?}",
        b1.pixel(60, 60)
    );
    assert_ne!(
        a1.pixels(),
        b1.pixels(),
        "two contexts produced the same pixels"
    );
    assert!(
        a2.near(60, 60, [217, 64, 38, 255], 12),
        "A's second frame lost its content after B ran: {:?}",
        a2.pixel(60, 60)
    );

    let a = first_renderer.counters();
    let b = second_renderer.counters();
    assert_eq!((a.live_buffers, a.live_textures), (2, 1));
    assert_eq!((b.live_buffers, b.live_textures), (2, 1));

    let path = std::env::temp_dir()
        .join("zeron-zgx-imgui-tests")
        .join("two-contexts-a.png");
    a2.save_png(&path).expect("the evidence PNG");
    let path_b = std::env::temp_dir()
        .join("zeron-zgx-imgui-tests")
        .join("two-contexts-b.png");
    b1.save_png(&path_b).expect("the evidence PNG");
    println!(
        "imgui two contexts: A {} px / B {} px -> {} and {}",
        a2.non_background_pixels(),
        b1.non_background_pixels(),
        path.display(),
        path_b.display()
    );
}

#[test]
fn a_window_context_tears_down_and_leaves_the_process_ready() {
    let _guard = imgui_guard();
    let gpu = headless_gpu();
    let format = wgpu::TextureFormat::Bgra8Unorm;
    let spec = FrameSpec::from_device_pixels(SIZE, 1.0);
    let mut renderer = WgpuRenderer::new(gpu.device.clone(), gpu.queue.clone(), format);
    let target = OffscreenTarget::new(&gpu.device, SIZE, format);

    {
        let mut window = WindowContext::try_create("torn down").expect("a context");
        // A turn before the drop: the window goes back to suspended, and
        // dropping it takes the context and its atlas with it.
        let prepared = window
            .prepare(spec, true, |ui| panel(ui, RED, "Torn down"))
            .expect("a prepared frame");
        let (_, image) =
            snapshot::render(&mut renderer, &gpu.device, &gpu.queue, &target, &prepared)
                .expect("an offscreen frame");
        assert!(image.non_background_pixels() > 1_000);
        assert!(!window.is_active());
    }

    // The process is ready for the next window: the process-global current
    // context was cleared with the drop, and nothing the dropped one owned is
    // needed by the next.
    let mut next = WindowContext::try_create("after teardown").expect("a context after teardown");
    let prepared = next
        .prepare(spec, renderer.needs_atlas(), |ui| panel(ui, BLUE, "After"))
        .expect("the next window draws");
    let (_, image) = snapshot::render(&mut renderer, &gpu.device, &gpu.queue, &target, &prepared)
        .expect("an offscreen frame");
    assert!(!next.is_active());
    assert!(
        image.near(60, 60, [38, 115, 230, 255], 12),
        "the window after teardown lost its content: {:?}",
        image.pixel(60, 60)
    );
    assert_eq!(next.frames(), 1);
}

#[test]
fn a_panic_in_one_windows_draw_leaves_its_context_suspended() {
    let _guard = imgui_guard();
    let gpu = headless_gpu();
    let format = wgpu::TextureFormat::Bgra8Unorm;
    let mut first = WindowContext::try_create("panicking window").expect("first context");
    let mut second = WindowContext::try_create("next window").expect("second context");
    let spec = FrameSpec::from_device_pixels(SIZE, 1.0);

    // A draw that panics unwinds through the frame. The context must be
    // suspended when the panic reaches the caller, because Dear ImGui has
    // one process-wide current context: left active, every later window's
    // `activate` fails and all plugin windows stay frozen until restart.
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = first.build_frame(spec, false, |_| panic!("the draw closure failed"));
    }));
    assert!(panic.is_err(), "the draw's panic must reach the caller");
    assert!(
        !first.is_active(),
        "a panicking draw left its context active"
    );

    // The next window takes its turn normally, in the same process.
    let mut renderer = WgpuRenderer::new(gpu.device.clone(), gpu.queue.clone(), format);
    let target = OffscreenTarget::new(&gpu.device, SIZE, format);
    let prepared = second
        .prepare(spec, renderer.needs_atlas(), |ui| {
            panel(ui, BLUE, "Plugin B")
        })
        .expect("the next window draws after another window's panic");
    let (_, image) = snapshot::render(&mut renderer, &gpu.device, &gpu.queue, &target, &prepared)
        .expect("an offscreen frame");
    assert!(!second.is_active());
    assert!(
        image.non_background_pixels() > 1_000,
        "the second window drew nothing"
    );
    assert!(
        image.near(60, 60, [38, 115, 230, 255], 12),
        "the second window's accent is missing: {:?}",
        image.pixel(60, 60)
    );

    // And the window whose draw panicked is itself usable afterwards.
    let recovered = first
        .prepare(spec, true, |ui| panel(ui, RED, "Plugin A"))
        .expect("the panicking window recovers");
    let (_, image) = snapshot::render(&mut renderer, &gpu.device, &gpu.queue, &target, &recovered)
        .expect("the recovered window draws");
    assert!(!first.is_active());
    assert!(
        image.near(60, 60, [217, 64, 38, 255], 12),
        "the recovered window lost its content: {:?}",
        image.pixel(60, 60)
    );

    let path = std::env::temp_dir()
        .join("zeron-zgx-imgui-tests")
        .join("panic-recovery.png");
    image.save_png(&path).expect("the evidence PNG");
    println!(
        "imgui panic recovery: {} non-background px -> {}",
        image.non_background_pixels(),
        path.display()
    );
}
