//! A headless GPU and the guards the ImGui tests share.
//!
//! Dear ImGui keeps one process-global current context and its contexts may
//! only be used from the thread that created them, so tests that create one
//! take [`imgui_guard`] for their whole body: they then run one at a time,
//! each creating and dropping its contexts on its own thread. The lock is
//! also what keeps a `Context` alive while another test would otherwise
//! create a second one.

use std::sync::{Mutex, MutexGuard};

use zgx_gpu::wgpu;

/// The one lock every test in this module holds.
static IMGUI_LOCK: Mutex<()> = Mutex::new(());

/// Serialises tests that touch Dear ImGui (see the module docs).
pub fn imgui_guard() -> MutexGuard<'static, ()> {
    IMGUI_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A real wgpu device with no window and no surface.
pub struct TestGpu {
    /// Kept alive for the device's sake; never read.
    _instance: wgpu::Instance,
    /// The same: requesting the device from an adapter keeps it needed.
    _adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

impl TestGpu {
    /// A second device on the same adapter, for the rebuild test.
    pub fn second_device(&self) -> (wgpu::Device, wgpu::Queue) {
        block_on(
            self._adapter
                .request_device(&wgpu::DeviceDescriptor::default()),
        )
        .expect("a second headless wgpu device")
    }
}

/// A headless device: adapter selection with no surface to match.
pub fn headless_gpu() -> TestGpu {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .expect("a headless wgpu adapter (the ImGui renderer tests need a GPU)");
    let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
        .expect("a headless wgpu device");
    TestGpu {
        _instance: instance,
        _adapter: adapter,
        device,
        queue,
    }
}

/// A future run to its end on this thread (its waker unparks it).
///
/// The app takes no async runtime for this; the same helper the other GPU
/// tests in the app use.
pub fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
    struct Unpark(std::thread::Thread);
    impl std::task::Wake for Unpark {
        fn wake(self: std::sync::Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = std::task::Waker::from(std::sync::Arc::new(Unpark(std::thread::current())));
    let mut context = std::task::Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        if let std::task::Poll::Ready(value) = future.as_mut().poll(&mut context) {
            return value;
        }
        std::thread::park_timeout(std::time::Duration::from_millis(20));
    }
}
