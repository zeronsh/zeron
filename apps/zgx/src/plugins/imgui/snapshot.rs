//! Offscreen rendering: a frame into a texture of our own, then its pixels.
//!
//! Two callers: tests (assert that a frame really draws text and geometry)
//! and snapshots (a PNG of a window for diagnostics and validation). Both
//! need a real wgpu device and no zgx window, so they work headless and on
//! any platform this app builds for.

use std::collections::HashMap;
use std::path::Path;

use zgx_gpu::wgpu;

use super::render::{PreparedFrame, RenderError, RenderStats, RenderTarget, WgpuRenderer};

/// A texture to draw a frame into and read back from.
pub struct OffscreenTarget {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: (u32, u32),
    format: wgpu::TextureFormat,
}

impl OffscreenTarget {
    /// A render-attachment, copy-source texture of `size`.
    ///
    /// `Rgba8Unorm` and `Bgra8Unorm` (what zgx's `ViewportFrame` uses) are
    /// supported; both read back as RGBA.
    pub fn new(device: &wgpu::Device, size: (u32, u32), format: wgpu::TextureFormat) -> Self {
        assert!(
            matches!(
                format,
                wgpu::TextureFormat::Rgba8Unorm
                    | wgpu::TextureFormat::Rgba8UnormSrgb
                    | wgpu::TextureFormat::Bgra8Unorm
                    | wgpu::TextureFormat::Bgra8UnormSrgb
            ),
            "an ImGui offscreen target is 8-bit RGBA or BGRA, not {format:?}"
        );
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("imgui offscreen target"),
            size: wgpu::Extent3d {
                width: size.0.max(1),
                height: size.1.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            texture,
            view,
            size,
            format,
        }
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    /// The texture's view, for a caller that draws into it directly (a
    /// `ViewportFrame`, in the tests).
    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    pub fn render_target(&self) -> RenderTarget<'_> {
        RenderTarget::new(&self.view, self.format, self.size)
    }

    /// Copies the drawn frame to the CPU: RGBA8, rows top to bottom.
    pub fn read(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Snapshot {
        read_pixels(device, queue, &self.texture, self.size, self.format)
    }
}

/// Draws `frame` into `target`, waits for the GPU and returns the pixels.
pub fn render(
    renderer: &mut WgpuRenderer,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    target: &OffscreenTarget,
    frame: &PreparedFrame,
) -> Result<(RenderStats, Snapshot), RenderError> {
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("imgui offscreen encoder"),
    });
    let stats = renderer.render(&mut encoder, target.render_target(), frame)?;
    queue.submit([encoder.finish()]);
    Ok((stats, target.read(device, queue)))
}

/// One frame's pixels: RGBA8, top row first, same orientation as Dear ImGui
/// screen space.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl Snapshot {
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> Self {
        assert_eq!(
            pixels.len(),
            (width as usize) * (height as usize) * 4,
            "a snapshot needs four bytes per pixel"
        );
        Self {
            width,
            height,
            pixels,
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// The colour at `(x, y)`, or `None` outside the image.
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let offset = ((y as usize) * (self.width as usize) + x as usize) * 4;
        Some([
            self.pixels[offset],
            self.pixels[offset + 1],
            self.pixels[offset + 2],
            self.pixels[offset + 3],
        ])
    }

    /// How many pixels each colour has, largest first.
    fn colour_counts(&self) -> Vec<([u8; 4], usize)> {
        let mut counts: HashMap<[u8; 4], usize> = HashMap::new();
        for pixel in self.pixels.as_chunks::<4>().0 {
            *counts.entry(*pixel).or_insert(0) += 1;
        }
        let mut counts: Vec<([u8; 4], usize)> = counts.into_iter().collect();
        counts.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(&right.0)));
        counts
    }

    /// The most common colour: the cleared background of a frame.
    pub fn background(&self) -> [u8; 4] {
        self.colour_counts()
            .first()
            .map(|(colour, _)| *colour)
            .unwrap_or([0, 0, 0, 0])
    }

    /// Pixels that differ from [`Self::background`].
    pub fn non_background_pixels(&self) -> usize {
        let background = self.background();
        self.pixels
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|pixel| **pixel != background)
            .count()
    }

    /// Distinct colours, a proxy for antialiased text and shaded geometry.
    pub fn distinct_colours(&self) -> usize {
        self.colour_counts().len()
    }

    /// Whether two pixels differ by at most `tolerance` on every channel.
    pub fn near(&self, x: u32, y: u32, expected: [u8; 4], tolerance: i32) -> bool {
        let Some(actual) = self.pixel(x, y) else {
            return false;
        };
        actual
            .iter()
            .zip(expected.iter())
            .all(|(actual, expected)| (*actual as i32 - *expected as i32).abs() <= tolerance)
    }

    /// Writes the frame as an 8-bit RGBA PNG.
    pub fn save_png(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = std::fs::File::create(path)?;
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), self.width, self.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        writer
            .write_image_data(&self.pixels)
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        writer
            .finish()
            .map_err(|error| std::io::Error::other(error.to_string()))
    }
}

/// Copies `texture` into a mappable buffer, waits, and returns its pixels as
/// RGBA8 (BGRA targets are swizzled).
pub fn read_pixels(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    size: (u32, u32),
    format: wgpu::TextureFormat,
) -> Snapshot {
    let (width, height) = (size.0.max(1), size.1.max(1));
    // copy_texture_to_buffer wants row pitches aligned to 256 bytes.
    let unpadded = width * 4;
    let padded =
        unpadded.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("imgui readback"),
        size: u64::from(padded) * u64::from(height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("imgui readback encoder"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("the GPU finished the readback");

    let slice = buffer.slice(..);
    let (sender, receiver) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.send(result);
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("the GPU mapped the readback buffer");
    receiver
        .recv()
        .expect("the readback callback ran")
        .expect("the readback buffer mapped");

    let swizzle = matches!(
        format,
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
    );
    let mut pixels = Vec::with_capacity((unpadded * height) as usize);
    {
        let mapped = slice.get_mapped_range();
        for row in 0..height as usize {
            let start = row * padded as usize;
            let row_pixels = &mapped[start..start + unpadded as usize];
            for pixel in row_pixels.as_chunks::<4>().0 {
                if swizzle {
                    pixels.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
                } else {
                    pixels.extend_from_slice(pixel);
                }
            }
        }
    }
    buffer.unmap();
    Snapshot::new(width, height, pixels)
}
