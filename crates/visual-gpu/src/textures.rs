//! Image textures for scenes and shaders: decoded, mipmapped, and uploaded
//! once per asset, then shared by every frame and every visual that uses
//! the same bytes.

use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, Weak};

use zeron_visual::TextureData;
use zeron_visual::image::{Image, Pixels};

/// What the cache may hold on the GPU before it drops the least recently
/// used.
const BUDGET_BYTES: usize = 256 * 1024 * 1024;

/// How a texture's 8-bit values are read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Space {
    /// Colors as images store them (sRGB), sampled as linear light.
    Color,
    /// Values as stored, as Shadertoy samples them (bottom row first).
    Raw,
}

/// A texture ready to bind.
#[derive(Clone)]
pub(crate) struct Loaded {
    pub(crate) view: wgpu::TextureView,
    pub(crate) mips: u32,
}

/// The bytes' content and how they're read: the same image uploads once,
/// whichever visual (or re-parse of one) brings it.
type Key = (u64, usize, Space);

struct Entry {
    key: Key,
    loaded: Result<Loaded, String>,
    /// GPU bytes (failures count a little, so they're evicted too).
    size: usize,
    used: u64,
}

/// An `Arc` already hashed, while it lives: hashing once per allocation, not
/// per frame.
struct Alias {
    bytes: Weak<[u8]>,
    hash: u64,
}

#[derive(Default)]
struct State {
    entries: Vec<Entry>,
    aliases: Vec<Alias>,
    clock: u64,
}

#[derive(Default)]
pub(crate) struct Textures {
    state: Mutex<State>,
}

fn bytes_of(data: &TextureData) -> &Arc<[u8]> {
    match data {
        TextureData::Encoded(bytes) => bytes,
        TextureData::Rgba { pixels, .. } => pixels,
    }
}

fn content_hash(bytes: &[u8]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

impl Textures {
    /// `data` on the GPU, decoding and uploading it on first use. Decoding
    /// runs outside the lock, so a large image doesn't hold up other renders.
    pub(crate) fn get(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        data: &TextureData,
        space: Space,
    ) -> Result<Loaded, String> {
        let bytes = bytes_of(data);
        let known = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.aliases.retain(|a| a.bytes.strong_count() > 0);
            state
                .aliases
                .iter()
                .find(|a| std::ptr::addr_eq(a.bytes.as_ptr(), Arc::as_ptr(bytes)))
                .map(|a| a.hash)
        };
        let hash = known.unwrap_or_else(|| content_hash(bytes));
        let key = (hash, bytes.len(), space);
        {
            let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let state = &mut *guard;
            if known.is_none() {
                state.aliases.push(Alias {
                    bytes: Arc::downgrade(bytes),
                    hash,
                });
            }
            state.clock += 1;
            if let Some(entry) = state.entries.iter_mut().find(|e| e.key == key) {
                entry.used = state.clock;
                return entry.loaded.clone();
            }
        }
        let loaded = zeron_visual::image::decode_texture(data)
            .and_then(|image| upload(device, queue, &image, space));
        if let Err(error) = &loaded {
            tracing::warn!(%error, "visual texture unusable");
        }
        let size = loaded.as_ref().map_or(1024, |(_, size)| *size);
        let loaded = loaded.map(|(loaded, _)| loaded);
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let state = &mut *guard;
        state.clock += 1;
        if let Some(entry) = state.entries.iter_mut().find(|e| e.key == key) {
            // Another render uploaded it meanwhile.
            entry.used = state.clock;
            return entry.loaded.clone();
        }
        state.entries.push(Entry {
            key,
            loaded: loaded.clone(),
            size,
            used: state.clock,
        });
        while state.entries.iter().map(|e| e.size).sum::<usize>() > BUDGET_BYTES
            && state.entries.len() > 1
        {
            let oldest = state
                .entries
                .iter()
                .enumerate()
                .min_by_key(|(_, e)| e.used)
                .map(|(i, _)| i)
                .expect("not empty");
            state.entries.swap_remove(oldest);
        }
        loaded
    }
}

/// An `f32` as IEEE half-precision bits (finite light values only).
fn half(value: f32) -> u16 {
    let value = if value.is_finite() {
        value.clamp(0.0, 65504.0)
    } else {
        0.0
    };
    let bits = value.to_bits();
    let exponent = ((bits >> 23) & 0xff) as i32 - 127 + 15;
    let mantissa = bits & 0x7f_ffff;
    if exponent <= 0 {
        // Too small for a normal half: flush to zero.
        0
    } else {
        ((exponent as u16) << 10) | (mantissa >> 13) as u16
    }
}

/// Every mip level of `image`, largest first, as the texture's bytes (one
/// buffer), with the level count and format.
fn levels(image: &Image) -> (Vec<u8>, u32, wgpu::TextureFormat) {
    let (mut w, mut h) = (image.width as usize, image.height as usize);
    let mut mips = 1;
    match &image.pixels {
        Pixels::Rgba8(pixels) => {
            let mut data = pixels.clone();
            let mut level_start = 0;
            while w > 1 || h > 1 {
                let (next, nw, nh) = downsample(&data[level_start..], w, h);
                level_start = data.len();
                data.extend_from_slice(&next);
                (w, h) = (nw, nh);
                mips += 1;
            }
            (data, mips, wgpu::TextureFormat::Rgba8Unorm)
        }
        Pixels::RgbaF32(pixels) => {
            let mut data: Vec<u8> = pixels.iter().flat_map(|v| half(*v).to_le_bytes()).collect();
            let mut level = None::<Vec<f32>>;
            while w > 1 || h > 1 {
                let (next, nw, nh) = downsample(level.as_deref().unwrap_or(pixels), w, h);
                data.extend(next.iter().flat_map(|v| half(*v).to_le_bytes()));
                level = Some(next);
                (w, h) = (nw, nh);
                mips += 1;
            }
            (data, mips, wgpu::TextureFormat::Rgba16Float)
        }
    }
}

/// Half the size, each texel the mean of the 2x2 it covers.
fn downsample<T: Copy + Into<f32> + FromF32>(
    src: &[T],
    w: usize,
    h: usize,
) -> (Vec<T>, usize, usize) {
    let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
    let mut out = Vec::with_capacity(nw * nh * 4);
    for y in 0..nh {
        for x in 0..nw {
            for c in 0..4 {
                let mut sum = 0.0f32;
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let sx = (x * 2 + dx).min(w - 1);
                    let sy = (y * 2 + dy).min(h - 1);
                    sum += src[(sy * w + sx) * 4 + c].into();
                }
                out.push(T::from_f32(sum / 4.0));
            }
        }
    }
    (out, nw, nh)
}

trait FromF32 {
    fn from_f32(value: f32) -> Self;
}
impl FromF32 for u8 {
    fn from_f32(value: f32) -> Self {
        value.round().clamp(0.0, 255.0) as u8
    }
}
impl FromF32 for f32 {
    fn from_f32(value: f32) -> Self {
        value
    }
}

fn upload(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    image: &Image,
    space: Space,
) -> Result<(Loaded, usize), String> {
    use wgpu::util::DeviceExt;
    let largest = device.limits().max_texture_dimension_2d;
    if image.width.max(image.height) > largest {
        return Err(format!(
            "a {}x{} image is larger than this GPU's {largest}px textures",
            image.width, image.height
        ));
    }
    // Shadertoy samples images bottom row first, as its passes store rows.
    let flipped;
    let image = if space == Space::Raw {
        flipped = flip_rows(image);
        &flipped
    } else {
        image
    };
    let (data, mips, format) = levels(image);
    let format = match (format, space) {
        (wgpu::TextureFormat::Rgba8Unorm, Space::Color) => wgpu::TextureFormat::Rgba8UnormSrgb,
        (format, _) => format,
    };
    let size = data.len();
    let texture = crate::guarded(device, || {
        device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("visual texture"),
                size: wgpu::Extent3d {
                    width: image.width,
                    height: image.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: mips,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &data,
        )
    })?;
    Ok((
        Loaded {
            view: texture.create_view(&Default::default()),
            mips,
        },
        size,
    ))
}

/// The image upside down.
fn flip_rows(image: &Image) -> Image {
    fn flip<T: Copy>(pixels: &[T], width: usize) -> Vec<T> {
        pixels
            .chunks_exact(width * 4)
            .rev()
            .flatten()
            .copied()
            .collect()
    }
    let width = image.width as usize;
    Image {
        width: image.width,
        height: image.height,
        pixels: match &image.pixels {
            Pixels::Rgba8(p) => Pixels::Rgba8(flip(p, width)),
            Pixels::RgbaF32(p) => Pixels::RgbaF32(flip(p, width)),
        },
    }
}

/// A 1x1 texture of one color, for slots with nothing to sample.
pub(crate) fn solid(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    rgba: [u8; 4],
) -> wgpu::TextureView {
    use wgpu::util::DeviceExt;
    device
        .create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("visual solid texture"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &rgba,
        )
        .create_view(&Default::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halves_round_trip_light_values() {
        assert_eq!(half(0.0), 0);
        assert_eq!(half(1.0), 0x3c00);
        assert_eq!(half(2.0), 0x4000);
        assert_eq!(half(0.5), 0x3800);
        assert_eq!(half(f32::INFINITY), 0);
        assert_eq!(half(1e9), 0x7bff);
    }

    #[test]
    fn mip_chains_reach_one_texel() {
        let image = Image {
            width: 5,
            height: 2,
            pixels: Pixels::Rgba8(vec![200; 5 * 2 * 4]),
        };
        let (data, mips, _) = levels(&image);
        assert_eq!(mips, 3);
        assert_eq!(data.len(), 5 * 2 * 4 + 2 * 4 + 4);
        assert_eq!(&data[data.len() - 4..], [200; 4]);
    }

    #[test]
    fn raw_images_flip_to_bottom_row_first() {
        let image = Image {
            width: 1,
            height: 2,
            pixels: Pixels::Rgba8(vec![1, 1, 1, 1, 2, 2, 2, 2]),
        };
        let Pixels::Rgba8(pixels) = flip_rows(&image).pixels else {
            unreachable!()
        };
        assert_eq!(pixels, [2, 2, 2, 2, 1, 1, 1, 1]);
    }
}
