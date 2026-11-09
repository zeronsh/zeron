//! Sixel images (DCS `q`): six-pixel-high bands of colour registers, as
//! the VT340 drew them and img2sixel, chafa and lsix write them.

/// Colour registers a picture may define (xterm's and foot's).
const REGISTERS: usize = 1024;
/// The widest or tallest image taken.
const MAX_SIDE: usize = 10_000;

/// The VT340's sixteen colours, in percent, the rest black.
const VT340: [[u8; 3]; 16] = [
    [0, 0, 0],
    [20, 20, 80],
    [80, 13, 13],
    [20, 80, 20],
    [80, 20, 80],
    [20, 80, 80],
    [80, 80, 20],
    [53, 53, 53],
    [26, 26, 26],
    [33, 33, 60],
    [60, 26, 26],
    [33, 60, 33],
    [60, 33, 60],
    [33, 60, 60],
    [60, 60, 33],
    [80, 80, 80],
];

/// An image: its size, and RGBA pixels.
#[derive(Debug, PartialEq)]
pub(crate) struct Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// Decode `body` (the parameters, `q`, then the data), leaving the pixels
/// no sixel sets `background` (an RGBA colour), unless the second
/// parameter is 1, which leaves them clear. None when it draws nothing or
/// is too large: more than `MAX_SIDE` a side, or `max_bytes` of pixels.
pub(crate) fn decode(body: &[u8], background: [u8; 4], max_bytes: usize) -> Option<Image> {
    let at = body.iter().position(|&b| b == b'q')?;
    let clear = body[..at].split(|&b| b == b';').nth(1) == Some(b"1");
    let data = &body[at + 1..];
    // Once for the size (the raster attributes', or the sixels' reach),
    // again to draw.
    let mut width = 0;
    let mut height = 0;
    walk(data, &mut [0; REGISTERS], |event| match event {
        Event::Raster(w, h) => {
            width = width.max(w);
            height = height.max(h);
        }
        Event::Sixels {
            x, y, bits, count, ..
        } => {
            width = width.max(x + count);
            height = height.max(y + 8 - bits.leading_zeros() as usize);
        }
    });
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return None;
    }
    if width * height * 4 > max_bytes {
        return None;
    }
    let fill = if clear { [0; 4] } else { background };
    let mut pixels: Vec<u8> = fill.repeat(width * height);
    let mut registers = [0; REGISTERS];
    for (register, [r, g, b]) in registers.iter_mut().zip(VT340) {
        *register = rgb(percent(r), percent(g), percent(b));
    }
    walk(data, &mut registers, |event| {
        if let Event::Sixels {
            x,
            y,
            bits,
            count,
            color,
        } = event
        {
            for bit in 0..6 {
                if bits & (1 << bit) == 0 || y + bit >= height {
                    continue;
                }
                let row = (y + bit) * width;
                let run = &mut pixels[(row + x) * 4..(row + x + count) * 4];
                for pixel in run.as_chunks_mut::<4>().0 {
                    *pixel = color.to_be_bytes();
                }
            }
        }
    });
    Some(Image {
        width: width as u32,
        height: height as u32,
        pixels,
    })
}

enum Event {
    /// Raster attributes: the image's width and height.
    Raster(usize, usize),
    /// `count` columns of the same six pixels from `x`, `y` (bit 0 the
    /// top), in `color` (RGBA).
    Sixels {
        x: usize,
        y: usize,
        bits: u8,
        count: usize,
        color: u32,
    },
}

/// Read the data, defining colours in `registers` as it goes and telling
/// `event` what it draws.
fn walk(data: &[u8], registers: &mut [u32; REGISTERS], mut event: impl FnMut(Event)) {
    let (mut x, mut y, mut color) = (0, 0, registers[0]);
    let mut i = 0;
    // The numbers after an introducer, as many as it has.
    let numbers = |i: &mut usize, out: &mut [usize]| {
        let mut n = 0;
        while *i < data.len() {
            match data[*i] {
                b'0'..=b'9' => {
                    if n < out.len() {
                        out[n] = (out[n] * 10 + usize::from(data[*i] - b'0')).min(1 << 20);
                    }
                }
                b';' => n += 1,
                _ => break,
            }
            *i += 1;
        }
    };
    while i < data.len() {
        let byte = data[i];
        i += 1;
        match byte {
            b'?'..=b'~' => {
                let bits = byte - b'?';
                if bits != 0 && x < MAX_SIDE && y < MAX_SIDE {
                    event(Event::Sixels {
                        x,
                        y,
                        bits,
                        count: 1,
                        color,
                    });
                }
                x += 1;
            }
            b'!' => {
                let mut count = [0];
                numbers(&mut i, &mut count);
                let count = count[0].max(1);
                if let Some(&byte @ b'?'..=b'~') = data.get(i) {
                    i += 1;
                    let bits = byte - b'?';
                    if bits != 0 && x < MAX_SIDE && y < MAX_SIDE {
                        let count = count.min(MAX_SIDE - x);
                        event(Event::Sixels {
                            x,
                            y,
                            bits,
                            count,
                            color,
                        });
                    }
                    x += count;
                }
            }
            b'"' => {
                let mut raster = [0; 4];
                numbers(&mut i, &mut raster);
                event(Event::Raster(raster[2], raster[3]));
            }
            b'#' => {
                let mut fields = [0; 5];
                let start = i;
                numbers(&mut i, &mut fields);
                let register = fields[0].min(REGISTERS - 1);
                let defined = data[start..i].iter().filter(|&&b| b == b';').count() >= 4;
                if defined {
                    let [_, space, a, b, c] = fields;
                    registers[register] = match space {
                        1 => hls(a, b, c),
                        _ => rgb(percent(a), percent(b), percent(c)),
                    };
                }
                color = registers[register];
            }
            b'$' => x = 0,
            b'-' => {
                x = 0;
                y += 6;
            }
            _ => {}
        }
    }
}

fn percent(n: impl Into<usize>) -> u8 {
    (n.into().min(100) * 255 / 100) as u8
}

fn rgb(r: u8, g: u8, b: u8) -> u32 {
    u32::from_be_bytes([r, g, b, 255])
}

/// DEC's hue, lightness and saturation (hue 0 blue, 120 red, 240 green).
fn hls(hue: usize, lightness: usize, saturation: usize) -> u32 {
    let h = ((hue + 240) % 360) as f32 / 60.;
    let l = lightness.min(100) as f32 / 100.;
    let s = saturation.min(100) as f32 / 100.;
    let chroma = (1. - (2. * l - 1.).abs()) * s;
    let x = chroma * (1. - (h % 2. - 1.).abs());
    let (r, g, b) = match h as u32 {
        0 => (chroma, x, 0.),
        1 => (x, chroma, 0.),
        2 => (0., chroma, x),
        3 => (0., x, chroma),
        4 => (x, 0., chroma),
        _ => (chroma, 0., x),
    };
    let m = l - chroma / 2.;
    let byte = |v: f32| ((v + m).clamp(0., 1.) * 255.).round() as u8;
    rgb(byte(r), byte(g), byte(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLACK: [u8; 4] = [0, 0, 0, 255];

    fn pixel(image: &Image, x: usize, y: usize) -> [u8; 4] {
        let at = (y * image.width as usize + x) * 4;
        image.pixels[at..at + 4].try_into().unwrap()
    }

    /// Bands of six pixels in colours defined as RGB or HLS, repeats, and
    /// carriage returns and new lines between bands.
    #[test]
    fn sixels_draw_bands_in_their_colours() {
        // A red column of six, two green columns of the top three, then on
        // the next band a blue (HLS 0°) pixel over the first column.
        let image = decode(
            b"0;1;0q#1;2;100;0;0~#2;2;0;100;0!2F-#3;1;0;50;100@",
            BLACK,
            1 << 20,
        )
        .unwrap();
        assert_eq!((image.width, image.height), (3, 7));
        assert_eq!(pixel(&image, 0, 5), [255, 0, 0, 255]);
        assert_eq!(pixel(&image, 2, 2), [0, 255, 0, 255]);
        // Left clear (the second parameter 1): no sixel set it.
        assert_eq!(pixel(&image, 2, 3), [0, 0, 0, 0]);
        assert_eq!(pixel(&image, 0, 6), [0, 0, 255, 255]);
        // `$` returns to the band's start, drawing over it.
        let image = decode(b"q#1;2;100;0;0~$#2;2;0;0;100?", BLACK, 1 << 20).unwrap();
        assert_eq!(image.width, 1);
        assert_eq!(pixel(&image, 0, 0), [255, 0, 0, 255]);
    }

    /// Raster attributes size the image; pixels no sixel sets are the
    /// background unless asked to be clear; registers not defined are the
    /// VT340's.
    #[test]
    fn raster_attributes_and_the_background() {
        let image = decode(b"q\"1;1;4;8#2~", [9, 9, 9, 255], 1 << 20).unwrap();
        assert_eq!((image.width, image.height), (4, 8));
        assert_eq!(pixel(&image, 0, 0), [204, 33, 33, 255]);
        assert_eq!(pixel(&image, 3, 7), [9, 9, 9, 255]);
    }

    /// An image drawing nothing, or larger than allowed, is none.
    #[test]
    fn empty_and_huge_images_are_none() {
        assert_eq!(decode(b"q", BLACK, 1 << 20), None);
        assert_eq!(decode(b"q#0;2;0;0;0", BLACK, 1 << 20), None);
        assert_eq!(
            decode(b"q!20000~", BLACK, usize::MAX).map(|i| i.width),
            Some(10_000)
        );
        assert_eq!(decode(b"q\"1;1;100;100~", BLACK, 1000), None);
    }
}
