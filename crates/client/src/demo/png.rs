//! Tiny dependency-free PNG encoder for demo images (stored deflate blocks —
//! big files, zero code). Produces a soft diagonal gradient with a few
//! "card" rectangles so screenshots show something recognizable.

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &byte in bytes {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in bytes {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

fn zlib_stored(raw: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    let mut blocks = raw.chunks(65_535).peekable();
    if blocks.peek().is_none() {
        out.extend_from_slice(&[1, 0, 0, 0xff, 0xff]);
    }
    while let Some(block) = blocks.next() {
        out.push(u8::from(blocks.peek().is_none()));
        let len = block.len() as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(block);
    }
    out.extend_from_slice(&adler32(raw).to_be_bytes());
    out
}

/// An RGB PNG, `width × height`, tinted by `seed`.
pub(crate) fn gradient(width: u32, height: u32, seed: u32) -> Vec<u8> {
    let tint = [(92u8, 124u8, 250u8), (236, 72, 153), (16, 185, 129)][seed as usize % 3];
    let mut raw = Vec::with_capacity(((width * 3 + 1) * height) as usize);
    for y in 0..height {
        raw.push(0);
        for x in 0..width {
            let t = (x + y) as f32 / (width + height) as f32;
            let card = (y * 5 / height.max(1)) % 2 == 1 && x > width / 12 && x < width - width / 12;
            let mix = |base: u8, accent: u8| {
                let v = base as f32 * (1.0 - t) + accent as f32 * t;
                if card {
                    (v * 0.35 + 255.0 * 0.65) as u8
                } else {
                    v as u8
                }
            };
            raw.push(mix(246, tint.0));
            raw.push(mix(244, tint.1));
            raw.push(mix(238, tint.2));
        }
    }
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &zlib_stored(&raw));
    chunk(&mut out, b"IEND", &[]);
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn png_has_signature_and_iend() {
        let png = super::gradient(8, 4, 0);
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
        assert_eq!(&png[png.len() - 8..png.len() - 4], b"IEND");
        assert_eq!(super::crc32(b"IEND"), 0xae42_6082);
    }
}
