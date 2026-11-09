//! The kitty graphics protocol (APC `G`): pictures a program sends in the
//! terminal's output, kept by number and placed on the screen's cells.
//!
//! Pictures come in the escape codes themselves (`t=d`), whole or in
//! chunks, as RGB, RGBA or PNG, zlib-compressed or not. Reading files or
//! shared memory the program names (`t=f`, `t=t`, `t=s`) is refused, as are
//! animation and relative or Unicode-placeholder placements: a program
//! asking (`a=q`) is told, and sends the pixels in the codes instead. The
//! screen model keeps where each placement is (`Term::place`) and moves it
//! with its lines; the pictures' pixels are here, shared with the view.
//! Sixel images (`crate::sixel`) are kept and placed here too.

use alacritty_terminal::{
    Term,
    event::EventListener,
    grid::Dimensions,
    index::{Column, Line},
    term::graphics::Placement,
    vte::ansi::Handler,
};
use std::{
    collections::HashMap,
    io::Read,
    sync::{Arc, Mutex},
};
use zgx::image::ImageData;

/// The pictures a session holds, by id: the session's thread adds and
/// removes them, the view draws them.
pub(crate) type Images = Mutex<HashMap<u32, Arc<ImageData>>>;

/// Pixels held at most, decoded, before the oldest pictures go (kitty's
/// and Ghostty's quota).
const QUOTA: usize = if cfg!(test) { 1 << 20 } else { 320 << 20 };
/// The most one transmission may send, encoded (a 4K RGBA picture is about
/// 33 MB).
const MAX_DATA: usize = 64 << 20;
/// The widest or tallest picture taken (and none larger than the quota).
const MAX_SIDE: u32 = 10_000;

/// A command's keys (`a=T,f=100,i=7`), each a number or a letter.
#[derive(Clone, Debug, Default, PartialEq)]
struct Keys {
    action: u8,
    medium: u8,
    format: u32,
    compression: u8,
    more: bool,
    quiet: u8,
    id: u32,
    number: u32,
    placement: u32,
    width: u32,
    height: u32,
    source: [u32; 4],
    offset: [u32; 2],
    columns: u32,
    rows: u32,
    z: i32,
    stay: bool,
    delete: u8,
    /// Keys asking for what this doesn't do: relative or Unicode
    /// placeholder placement.
    unsupported: bool,
}

fn keys(control: &[u8]) -> Option<Keys> {
    let mut keys = Keys {
        action: b't',
        medium: b'd',
        format: 32,
        delete: b'a',
        ..Keys::default()
    };
    for pair in control
        .split(|&b| b == b',')
        .filter(|pair| !pair.is_empty())
    {
        let [key, b'=', value @ ..] = pair else {
            return None;
        };
        let letter = || match value {
            [letter] => Some(*letter),
            _ => None,
        };
        let number = || std::str::from_utf8(value).ok()?.parse::<u32>().ok();
        match key {
            b'a' => keys.action = letter()?,
            b't' => keys.medium = letter()?,
            b'f' => keys.format = number()?,
            b'o' => keys.compression = letter()?,
            b'm' => keys.more = number()? == 1,
            b'q' => keys.quiet = number()? as u8,
            b'i' => keys.id = number()?,
            b'I' => keys.number = number()?,
            b'p' => keys.placement = number()?,
            b's' => keys.width = number()?,
            b'v' => keys.height = number()?,
            b'x' => keys.source[0] = number()?,
            b'y' => keys.source[1] = number()?,
            b'w' => keys.source[2] = number()?,
            b'h' => keys.source[3] = number()?,
            b'X' => keys.offset[0] = number()?,
            b'Y' => keys.offset[1] = number()?,
            b'c' => keys.columns = number()?,
            b'r' => keys.rows = number()?,
            b'z' => keys.z = std::str::from_utf8(value).ok()?.parse().ok()?,
            b'C' => keys.stay = number()? == 1,
            b'd' => keys.delete = letter()?,
            b'U' => keys.unsupported |= number()? != 0,
            b'P' | b'Q' | b'H' | b'V' => keys.unsupported = true,
            // Sizes and offsets of files, and animation's keys.
            _ => {}
        }
    }
    Some(keys)
}

/// A picture still arriving in chunks.
struct Loading {
    keys: Keys,
    data: Vec<u8>,
    /// A chunk's base64 left over when it didn't end on a group of four.
    tail: Vec<u8>,
}

/// What the session knows of the pictures it holds, beside their pixels.
struct Held {
    number: u32,
    /// A sixel image's, which nothing names: it goes once covered.
    sixel: bool,
    bytes: usize,
    /// When it arrived, to let the oldest go first.
    serial: u64,
}

/// A session's pictures, and the transmission under way.
#[derive(Default)]
pub(crate) struct Graphics {
    held: HashMap<u32, Held>,
    /// Boxed: a session holds its `Graphics` whether pictures come or not.
    loading: Option<Box<Loading>>,
    bytes: usize,
    serial: u64,
    /// The id the last picture sent without one was given, counting down
    /// from the top (programs number theirs from 1).
    given: u32,
}

impl Graphics {
    /// Carry out the command `body` (what follows APC `G`) on `term`, whose
    /// cells are `cell` pixels; the answer to write back, if any.
    pub(crate) fn command<T: EventListener>(
        &mut self,
        body: &[u8],
        term: &mut Term<T>,
        cell: (u16, u16),
        images: &Images,
    ) -> Option<Vec<u8>> {
        let (control, payload) = match body.iter().position(|&b| b == b';') {
            Some(at) => (&body[..at], &body[at + 1..]),
            None => (body, &[][..]),
        };
        let Some(keys) = keys(control) else {
            return reply(&Keys::default(), Err("EINVAL:bad control data"));
        };
        // A chunk after the first names nothing but whether more follow.
        if let Some(loading) = &mut self.loading {
            if keys.action == b't' && keys.id == 0 && keys.number == 0 {
                if let Err(error) = append(loading, payload, keys.more) {
                    let keys = self.loading.take().unwrap().keys;
                    return reply(&keys, Err(error));
                }
                if keys.more {
                    return None;
                }
                let mut loading = self.loading.take().unwrap();
                if keys.quiet != 0 {
                    loading.keys.quiet = keys.quiet;
                }
                return self.loaded(*loading, term, cell, images);
            }
            self.loading = None;
        }
        match keys.action {
            b't' | b'T' | b'q' => {
                if keys.medium != b'd' {
                    return reply(&keys, Err("EINVAL:only pixels in the codes are taken"));
                }
                if keys.unsupported {
                    return reply(&keys, Err("EINVAL:unsupported placement"));
                }
                let more = keys.more;
                let mut loading = Loading {
                    keys,
                    data: Vec::new(),
                    tail: Vec::new(),
                };
                if let Err(error) = append(&mut loading, payload, more) {
                    return reply(&loading.keys, Err(error));
                }
                if loading.keys.more {
                    self.loading = Some(Box::new(loading));
                    return None;
                }
                self.loaded(loading, term, cell, images)
            }
            b'p' => {
                if keys.unsupported {
                    return reply(&keys, Err("EINVAL:unsupported placement"));
                }
                let id = match (keys.id, keys.number) {
                    (0, 0) => return reply(&keys, Err("EINVAL:no picture named")),
                    (0, number) => self.newest(number),
                    (id, _) => Some(id),
                };
                let image = id.and_then(|id| Some((id, images.lock().unwrap().get(&id)?.clone())));
                let Some((id, image)) = image else {
                    return reply(&keys, Err("ENOENT:no such picture"));
                };
                place(&keys, id, &image, term, cell);
                reply(&Keys { id, ..keys }, Ok(()))
            }
            b'd' => {
                self.delete(&keys, term, images);
                None
            }
            _ => reply(&keys, Err("EINVAL:unsupported action")),
        }
    }

    /// A transmission complete: decode it, keep it (unless only asked
    /// about), and place it when asked to.
    fn loaded<T: EventListener>(
        &mut self,
        loading: Loading,
        term: &mut Term<T>,
        cell: (u16, u16),
        images: &Images,
    ) -> Option<Vec<u8>> {
        let keys = loading.keys;
        let image = match decode(&keys, loading.data) {
            Ok(image) => image,
            Err(error) => return reply(&keys, Err(error)),
        };
        if keys.action == b'q' {
            return reply(&keys, Ok(()));
        }
        let id = match keys.id {
            0 => self.give_id(images),
            id => id,
        };
        let image = Arc::new(image);
        self.keep(id, keys.number, false, image.clone(), term, images);
        if keys.action == b'T' {
            place(&keys, id, &image, term, cell);
        }
        // A picture sent without an id or a number isn't answered.
        let keys = Keys {
            id: if keys.id == 0 && keys.number != 0 {
                id
            } else {
                keys.id
            },
            ..keys
        };
        reply(&keys, Ok(()))
    }

    /// A sixel image (DCS `q`, `body` its parameters, `q` and data): kept
    /// and placed at the cursor over the cells its pixels need, its pixels
    /// that no sixel sets `background` unless asked to be clear. Pictures
    /// it covers wholly go, as the cells it takes are its; the cursor goes
    /// to the line of its last band of six pixels (the VT340's, as xterm,
    /// foot and WezTerm have it), in the column it was in, scrolling as a
    /// line feed does.
    pub(crate) fn sixel<T: EventListener>(
        &mut self,
        body: &[u8],
        term: &mut Term<T>,
        cell: (u16, u16),
        background: [u8; 3],
        images: &Images,
    ) {
        let [r, g, b] = background;
        let Some(decoded) = crate::sixel::decode(body, [r, g, b, 255], QUOTA) else {
            return;
        };
        let (width, height) = (decoded.width, decoded.height);
        let Ok(image) = ImageData::new(width, height, decoded.pixels) else {
            return;
        };
        let id = self.give_id(images);
        let point = term.grid().cursor.point;
        let cells = |pixels: u32, cell: u16| match cell {
            0 => 1,
            cell => pixels.div_ceil(u32::from(cell)) as usize,
        };
        let (columns, lines) = (cells(width, cell.0), cells(height, cell.1));
        let rows = point.line..point.line + lines;
        let across = point.column..point.column + columns;
        let mut covered = Vec::new();
        term.retain_placements(|p| {
            let lines = p.line_range();
            let inside = rows.start <= lines.start
                && lines.end <= rows.end
                && across.start <= p.point.column
                && p.point.column + p.columns <= across.end;
            if inside {
                covered.push(p.image);
            }
            !inside
        });
        for covered in covered {
            let sixel = self.held.get(&covered).is_some_and(|held| held.sixel);
            if sixel && !term.placements().iter().any(|p| p.image == covered) {
                self.forget(covered, images);
            }
        }
        self.keep(id, 0, true, Arc::new(image), term, images);
        term.place(Placement {
            image: id,
            id: 0,
            point,
            lines,
            columns,
            z: 0,
            source: [0, 0, width, height],
            offset: [0, 0],
            size: [width, height],
        });
        let last_band = (height - 1) / 6 * 6;
        let down = match cell.1 {
            0 => 0,
            cell => last_band / u32::from(cell),
        };
        for _ in 0..down {
            term.linefeed();
        }
    }

    /// An id no picture has, from the top down.
    fn give_id(&mut self, images: &Images) -> u32 {
        let images = images.lock().unwrap();
        loop {
            self.given = self.given.wrapping_sub(1);
            if self.given != 0 && !images.contains_key(&self.given) {
                return self.given;
            }
        }
    }

    /// The newest picture sent under `number`.
    fn newest(&self, number: u32) -> Option<u32> {
        self.held
            .iter()
            .filter(|(_, held)| held.number == number)
            .max_by_key(|(_, held)| held.serial)
            .map(|(id, _)| *id)
    }

    fn keep<T: EventListener>(
        &mut self,
        id: u32,
        number: u32,
        sixel: bool,
        image: Arc<ImageData>,
        term: &mut Term<T>,
        images: &Images,
    ) {
        let bytes = image.pixels().len();
        self.serial += 1;
        let held = Held {
            number,
            sixel,
            bytes,
            serial: self.serial,
        };
        if let Some(old) = self.held.insert(id, held) {
            self.bytes -= old.bytes;
        }
        self.bytes += bytes;
        images.lock().unwrap().insert(id, image);
        // A picture sent again under its id replaces it where placed: its
        // placements now show the new one, and the view draws them afresh.
        if term.placements().iter().any(|p| p.image == id) {
            term.mark_fully_damaged();
        }
        // Over the quota, the oldest go: first those not on the screen.
        while self.bytes > QUOTA {
            let placed = |id: &u32| term.placements().iter().any(|p| p.image == *id);
            let Some(oldest) = self
                .held
                .iter()
                .filter(|(other, _)| **other != id)
                .min_by_key(|(other, held)| (placed(other), held.serial))
                .map(|(id, _)| *id)
            else {
                break;
            };
            self.forget(oldest, images);
            term.retain_placements(|p| p.image != oldest);
        }
    }

    fn forget(&mut self, id: u32, images: &Images) {
        if let Some(held) = self.held.remove(&id) {
            self.bytes -= held.bytes;
        }
        images.lock().unwrap().remove(&id);
        if self.held.is_empty() {
            *self = Graphics {
                given: self.given,
                ..Graphics::default()
            };
            *images.lock().unwrap() = HashMap::new();
        }
    }

    /// `a=d`: the placements `d` picks go; picked in capitals, the
    /// pictures they showed too, unless still placed.
    fn delete<T: EventListener>(&mut self, keys: &Keys, term: &mut Term<T>, images: &Images) {
        let screen = Line(0)..Line(term.screen_lines() as i32);
        let cursor = term.grid().cursor.point;
        // Cells are counted from 1.
        let (x, y) = (keys.source[0], keys.source[1]);
        let cell = (Column(x.saturating_sub(1) as usize), Line(y as i32 - 1));
        let covers = |p: &Placement, line: Line, column: Column| {
            p.line_range().contains(&line)
                && (p.point.column..p.point.column + p.columns).contains(&column)
        };
        let id = match (keys.id, keys.number) {
            (0, number) if number != 0 => self.newest(number),
            (id, _) => Some(id),
        };
        let picked = |p: &Placement| match keys.delete.to_ascii_lowercase() {
            b'a' => p.line_range().start < screen.end && p.line_range().end > screen.start,
            b'i' | b'n' => Some(p.image) == id && (keys.placement == 0 || p.id == keys.placement),
            b'c' => covers(p, cursor.line, cursor.column),
            b'p' => covers(p, cell.1, cell.0),
            b'q' => covers(p, cell.1, cell.0) && p.z == keys.z,
            b'x' => (p.point.column..p.point.column + p.columns).contains(&cell.0),
            b'y' => p.line_range().contains(&cell.1),
            b'z' => p.z == keys.z,
            b'r' => (x..=y).contains(&p.image),
            _ => false,
        };
        let mut shown = Vec::new();
        term.retain_placements(|p| {
            let go = picked(p);
            if go {
                shown.push(p.image);
            }
            !go
        });
        if keys.delete.is_ascii_uppercase() {
            // Named pictures go even when nothing showed them.
            if matches!(keys.delete, b'I' | b'N') {
                shown.extend(id);
            }
            if keys.delete == b'R' {
                shown.extend(self.held.keys().copied().filter(|id| (x..=y).contains(id)));
            }
            for id in shown {
                if !term.placements().iter().any(|p| p.image == id) {
                    self.forget(id, images);
                }
            }
        }
    }
}

/// Add a chunk of base64 to a transmission, `more` to follow. Programs
/// cut one encoding into chunks (kitty's, every fourth character) or encode
/// each chunk on its own, padded: both arrive whole.
fn append(loading: &mut Loading, payload: &[u8], more: bool) -> Result<(), &'static str> {
    use base64::{Engine, engine};
    const BASE64: engine::GeneralPurpose = engine::GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        engine::GeneralPurposeConfig::new()
            .with_decode_padding_mode(engine::DecodePaddingMode::Indifferent),
    );
    let mut text = std::mem::take(&mut loading.tail);
    text.extend(payload.iter().filter(|b| !b.is_ascii_whitespace()));
    // Padded, a chunk ends its encoding; else what doesn't fill a group of
    // four waits for the next.
    let whole = if !more || text.last() == Some(&b'=') {
        text.len()
    } else {
        text.len() / 4 * 4
    };
    if loading.data.len() + whole / 4 * 3 > MAX_DATA {
        return Err("EFBIG:picture too large");
    }
    BASE64
        .decode_vec(&text[..whole], &mut loading.data)
        .map_err(|_| "EINVAL:bad base64")?;
    text.drain(..whole);
    loading.tail = text;
    Ok(())
}

/// A transmission's pixels, as RGBA.
fn decode(keys: &Keys, data: Vec<u8>) -> Result<ImageData, &'static str> {
    let data = match keys.compression {
        0 => data,
        b'z' => {
            let mut inflated = Vec::new();
            flate2::read::ZlibDecoder::new(&data[..])
                .take(MAX_DATA as u64 * 4 + 1)
                .read_to_end(&mut inflated)
                .map_err(|_| "EINVAL:bad zlib data")?;
            inflated
        }
        _ => return Err("EINVAL:unknown compression"),
    };
    let (width, height, pixels) = match keys.format {
        24 | 32 => {
            let (width, height) = (keys.width, keys.height);
            if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
                return Err("EINVAL:bad size");
            }
            if width as usize * height as usize * 4 > QUOTA {
                return Err("EFBIG:picture too large");
            }
            let channels = (keys.format / 8) as usize;
            let expected = width as usize * height as usize * channels;
            if data.len() < expected {
                return Err("ENODATA:too few pixels");
            }
            let pixels = if channels == 4 {
                let mut data = data;
                data.truncate(expected);
                data
            } else {
                data[..expected]
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .flat_map(|&[r, g, b]| [r, g, b, 255])
                    .collect()
            };
            (width, height, pixels)
        }
        100 => png(&data)?,
        _ => return Err("EINVAL:unknown format"),
    };
    ImageData::new(width, height, pixels).map_err(|_| "EINVAL:bad size")
}

fn png(data: &[u8]) -> Result<(u32, u32, Vec<u8>), &'static str> {
    let bad = |_| "EBADPNG:bad PNG";
    let mut decoder = png::Decoder::new(std::io::Cursor::new(data));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(bad)?;
    let (width, height) = (reader.info().width, reader.info().height);
    if width > MAX_SIDE || height > MAX_SIDE {
        return Err("EINVAL:bad size");
    }
    // Checked before decoding: a small file may say it's huge.
    if width as usize * height as usize * 4 > QUOTA {
        return Err("EFBIG:picture too large");
    }
    let mut buffer = vec![0; reader.output_buffer_size().ok_or("EINVAL:bad size")?];
    let frame = reader.next_frame(&mut buffer).map_err(bad)?;
    buffer.truncate(frame.buffer_size());
    let pixels = match frame.color_type {
        png::ColorType::Rgba => buffer,
        png::ColorType::Rgb => buffer
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|&[r, g, b]| [r, g, b, 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => buffer
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|&[gray, alpha]| [gray, gray, gray, alpha])
            .collect(),
        png::ColorType::Grayscale => buffer.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return Err("EBADPNG:bad PNG"),
    };
    Ok((width, height, pixels))
}

/// Place `image` at the cursor, as `keys` say, and move the cursor past it
/// unless asked not to (`C=1`): down its lines but the last, scrolling as a
/// line feed does, and across its columns.
fn place<T: EventListener>(
    keys: &Keys,
    id: u32,
    image: &ImageData,
    term: &mut Term<T>,
    cell: (u16, u16),
) {
    let [x, y, w, h] = keys.source;
    let (x, y) = (x.min(image.width()), y.min(image.height()));
    let width = if w == 0 {
        image.width() - x
    } else {
        w.min(image.width() - x)
    };
    let height = if h == 0 {
        image.height() - y
    } else {
        h.min(image.height() - y)
    };
    // Given columns or rows, it stretches to them (one alone keeps its
    // shape); given neither, it covers the cells its pixels need.
    let (cell_width, cell_height) = (u32::from(cell.0), u32::from(cell.1));
    let [offset_x, offset_y] = keys.offset;
    let mut size = [width, height];
    if cell_width != 0 && cell_height != 0 {
        let across = |columns: u32| (columns * cell_width).saturating_sub(offset_x).max(1);
        let down = |rows: u32| (rows * cell_height).saturating_sub(offset_y).max(1);
        size = match (keys.columns, keys.rows) {
            (0, 0) => size,
            (0, rows) => [
                (down(rows) as u64 * u64::from(width) / u64::from(height)) as u32,
                down(rows),
            ],
            (columns, 0) => [
                across(columns),
                (across(columns) as u64 * u64::from(height) / u64::from(width)) as u32,
            ],
            (columns, rows) => [across(columns), down(rows)],
        };
    } else if keys.columns != 0 || keys.rows != 0 {
        // Pixels unknown, it stretches over its cells as they are drawn.
        size = [0, 0];
    }
    let cells = |given: u32, offset: u32, pixels: u32, cell: u32| match (given, cell) {
        (0, 0) => 1,
        (0, cell) => (offset + pixels).div_ceil(cell).max(1),
        (given, _) => given,
    };
    let columns = cells(keys.columns, offset_x, size[0], cell_width) as usize;
    let lines = cells(keys.rows, offset_y, size[1], cell_height) as usize;
    let point = term.grid().cursor.point;
    term.place(Placement {
        image: id,
        id: keys.placement,
        point,
        lines,
        columns,
        z: keys.z,
        source: [x, y, width, height],
        offset: keys.offset,
        size,
    });
    if !keys.stay {
        for _ in 1..lines {
            term.linefeed();
        }
        let last = term.columns() - 1;
        term.goto_col((point.column.0 + columns).min(last));
    }
}

/// The answer to a command, unless it asked for none: kitty answers only
/// commands naming a picture, and `q=1` silences success, `q=2` errors too.
fn reply(keys: &Keys, result: Result<(), &str>) -> Option<Vec<u8>> {
    let quiet = match result {
        Ok(()) => keys.quiet >= 1,
        Err(_) => keys.quiet >= 2,
    };
    if quiet || (keys.id == 0 && keys.number == 0) {
        return None;
    }
    let mut answer = format!("\x1b_Gi={}", keys.id);
    if keys.number != 0 {
        answer += &format!(",I={}", keys.number);
    }
    if keys.placement != 0 {
        answer += &format!(",p={}", keys.placement);
    }
    answer += ";";
    answer += result.err().unwrap_or("OK");
    answer += "\x1b\\";
    Some(answer.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::{
        event::VoidListener,
        index::Point,
        term::{Config, test::TermSize},
    };
    use base64::Engine;

    struct Fixture {
        term: Term<VoidListener>,
        graphics: Graphics,
        images: Images,
    }
    impl Fixture {
        fn new() -> Self {
            Self {
                term: Term::new(Config::default(), &TermSize::new(20, 10), VoidListener),
                graphics: Graphics::default(),
                images: Images::default(),
            }
        }
        /// Run `control;base64(data)`, cells 10 by 20 pixels: the answer.
        fn run(&mut self, control: &str, data: &[u8]) -> String {
            let mut body = control.as_bytes().to_vec();
            if !data.is_empty() {
                body.push(b';');
                body.extend(
                    base64::engine::general_purpose::STANDARD
                        .encode(data)
                        .bytes(),
                );
            }
            let answer = self
                .graphics
                .command(&body, &mut self.term, (10, 20), &self.images);
            String::from_utf8(answer.unwrap_or_default()).unwrap()
        }
        fn placed(&self) -> Vec<(u32, i32, usize, usize, usize)> {
            self.term
                .placements()
                .iter()
                .map(|p| {
                    (
                        p.image,
                        p.point.line.0,
                        p.point.column.0,
                        p.lines,
                        p.columns,
                    )
                })
                .collect()
        }
        fn cursor(&self) -> Point {
            self.term.grid().cursor.point
        }
    }

    fn rgba(width: usize, height: usize) -> Vec<u8> {
        [255, 0, 0, 255].repeat(width * height)
    }

    /// A picture sent and shown at once covers the cells its pixels need,
    /// at the cursor, and the cursor moves past it; one sent, then placed,
    /// is placed where the cursor is then; numbered ones are answered.
    #[test]
    fn a_picture_is_sent_placed_and_answered() {
        let mut fx = Fixture::new();
        // 25 by 30 pixels: 3 columns, 2 lines of 10 by 20.
        assert_eq!(
            fx.run("a=T,f=32,s=25,v=30,i=7", &rgba(25, 30)),
            "\x1b_Gi=7;OK\x1b\\"
        );
        assert_eq!(fx.placed(), [(7, 0, 0, 2, 3)]);
        assert_eq!(fx.cursor(), Point::new(Line(1), Column(3)));
        // Sent quietly, placed later at 4 by 1 cells, the cursor left.
        assert_eq!(fx.run("a=t,f=24,s=2,v=2,i=8,q=1", &[9; 12]), "");
        fx.term.goto(5, 2);
        assert_eq!(
            fx.run("a=p,i=8,c=4,r=1,C=1,p=3", &[]),
            "\x1b_Gi=8,p=3;OK\x1b\\"
        );
        assert_eq!(fx.placed()[1], (8, 5, 2, 1, 4));
        assert_eq!(fx.cursor(), Point::new(Line(5), Column(2)));
        assert_eq!(
            fx.images.lock().unwrap()[&8].pixels(),
            [9, 9, 9, 255].repeat(4)
        );
        // A picture sent without a name is kept but not answered.
        assert_eq!(fx.run("a=T,f=32,s=1,v=1", &rgba(1, 1)), "");
        assert_eq!(fx.images.lock().unwrap().len(), 3);
    }

    /// Given columns and rows, a picture stretches over them; given one,
    /// it keeps its shape.
    #[test]
    fn columns_or_rows_given_stretch_the_picture() {
        let mut fx = Fixture::new();
        fx.run("a=t,f=32,s=40,v=20,i=1,q=1", &rgba(40, 20));
        let sizes = |fx: &mut Fixture, keys: &str| {
            fx.run(&format!("a=p,i=1,C=1,{keys}"), &[]);
            let p = fx.term.placements().last().unwrap().clone();
            (p.size, p.columns, p.lines)
        };
        // Cells 10 by 20: its own 40 by 20 is 4 by 1.
        assert_eq!(sizes(&mut fx, "z=0"), ([40, 20], 4, 1));
        assert_eq!(sizes(&mut fx, "c=2,r=3"), ([20, 60], 2, 3));
        assert_eq!(sizes(&mut fx, "c=8"), ([80, 40], 8, 2));
        assert_eq!(sizes(&mut fx, "r=2,X=4"), ([80, 40], 9, 2));
        // The cell's pixels unknown: over the cells asked for, as drawn.
        fx.graphics
            .command(b"a=p,i=1,C=1,c=3,r=2", &mut fx.term, (0, 0), &fx.images);
        let p = fx.term.placements().last().unwrap();
        assert_eq!((p.size, p.columns, p.lines), ([0, 0], 3, 2));
    }

    /// Sent in chunks (each base64 of its own), compressed or as a PNG, a
    /// picture arrives whole.
    #[test]
    fn chunked_compressed_and_png_pictures_arrive_whole() {
        let mut fx = Fixture::new();
        let pixels: Vec<u8> = (0..4 * 6 * 4).map(|n| n as u8).collect();
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(&mut z, &pixels).unwrap();
        let z = z.finish().unwrap();
        let (first, rest) = z.split_at(z.len() / 2);
        assert_eq!(fx.run("a=t,f=32,o=z,s=4,v=6,i=1,m=1", first), "");
        assert_eq!(fx.run("m=0", rest), "\x1b_Gi=1;OK\x1b\\");
        assert_eq!(fx.images.lock().unwrap()[&1].pixels(), &pixels[..]);

        let mut png = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png, 2, 1);
            encoder.set_color(png::ColorType::Rgb);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[1, 2, 3, 4, 5, 6]).unwrap();
        }
        // Chunks cut inside a group of four base64 characters.
        let text = base64::engine::general_purpose::STANDARD.encode(&png);
        let (a, b) = text.split_at(5);
        let mut body = b"a=t,f=100,I=4,m=1;".to_vec();
        body.extend(a.bytes());
        let none = fx
            .graphics
            .command(&body, &mut fx.term, (10, 20), &fx.images);
        assert_eq!(none, None);
        let answer = fx
            .graphics
            .command(
                format!("m=0;{b}").as_bytes(),
                &mut fx.term,
                (10, 20),
                &fx.images,
            )
            .unwrap();
        let answer = String::from_utf8(answer).unwrap();
        let id: u32 = answer["\x1b_Gi=".len()..answer.find(',').unwrap()]
            .parse()
            .unwrap();
        assert_eq!(answer, format!("\x1b_Gi={id},I=4;OK\x1b\\"));
        assert_eq!(
            fx.images.lock().unwrap()[&id].pixels(),
            [1, 2, 3, 255, 4, 5, 6, 255]
        );
        // Placed by its number.
        assert_eq!(fx.run("a=p,I=4", &[]), format!("\x1b_Gi={id},I=4;OK\x1b\\"));
    }

    /// A program asking what's supported is told, and nothing is kept: the
    /// pixels in the codes are, files and shared memory aren't.
    #[test]
    fn queries_are_answered_and_keep_nothing() {
        let mut fx = Fixture::new();
        assert_eq!(
            fx.run("a=q,i=31,s=1,v=1,f=24", &[0, 0, 0]),
            "\x1b_Gi=31;OK\x1b\\"
        );
        assert!(fx.images.lock().unwrap().is_empty());
        assert!(
            fx.run("a=q,i=32,t=f,s=1,v=1", b"/etc/passwd")
                .contains(";EINVAL:")
        );
        assert!(fx.run("a=q,i=33,t=s,s=1,v=1", b"/shm").contains(";EINVAL:"));
        assert!(
            fx.run("a=q,i=34,s=2,v=2,f=24", &[0; 3])
                .contains(";ENODATA:")
        );
        assert!(fx.run("a=p,i=99", &[]).contains(";ENOENT:"));
        assert_eq!(fx.run("a=p,i=99,q=2", &[]), "");
        assert!(fx.placed().is_empty());
    }

    /// `a=d` drops the placements it picks; in capitals the pictures too,
    /// once nothing shows them.
    #[test]
    fn deletes_pick_placements_and_free_pictures() {
        let mut fx = Fixture::new();
        fx.run("a=t,f=32,s=10,v=20,i=1,q=1", &rgba(10, 20));
        fx.run("a=t,f=32,s=10,v=20,i=2,q=1", &rgba(10, 20));
        for (line, column, image, z) in [(0, 0, 1, 0), (2, 4, 1, 5), (4, 8, 2, -1)] {
            fx.term.goto(line, column);
            fx.run(&format!("a=p,i={image},z={z},C=1"), &[]);
        }
        fx.run("a=d,d=z,z=5", &[]);
        assert_eq!(fx.placed(), [(1, 0, 0, 1, 1), (2, 4, 8, 1, 1)]);
        // At the cell at column 9, line 5 (from 1).
        fx.run("a=d,d=p,x=9,y=5", &[]);
        assert_eq!(fx.placed(), [(1, 0, 0, 1, 1)]);
        assert_eq!(fx.images.lock().unwrap().len(), 2);
        fx.run("a=d,d=I,i=2", &[]);
        assert_eq!(fx.images.lock().unwrap().len(), 1);
        fx.run("a=d,d=A", &[]);
        assert!(fx.placed().is_empty());
        assert!(fx.images.lock().unwrap().is_empty());
        // Once all are gone, nothing is held for them.
        assert_eq!(fx.images.lock().unwrap().capacity(), 0);
        assert_eq!(fx.graphics.held.capacity(), 0);
    }

    /// A sixel image is placed at the cursor over the cells its pixels
    /// need; the cursor goes to the line of its last band, in its column;
    /// another over the same cells takes them, and the first image goes.
    #[test]
    fn a_sixel_image_takes_its_cells() {
        let mut fx = Fixture::new();
        fx.term.goto(1, 2);
        // 20 by 30 pixels (raster attributes), its last band from pixel 24:
        // line 1 of its 2.
        let body = b"0;0;0q\"1;1;20;30#1;2;100;0;0!20~";
        fx.graphics
            .sixel(body, &mut fx.term, (10, 20), [0, 0, 0], &fx.images);
        assert_eq!(fx.placed(), [(fx.placed()[0].0, 1, 2, 2, 2)]);
        assert_eq!(fx.cursor(), Point::new(Line(2), Column(2)));
        let first = fx.placed()[0].0;
        let image = fx.images.lock().unwrap()[&first].clone();
        assert_eq!((image.width(), image.height()), (20, 30));
        // Unset pixels are the background.
        assert_eq!(image.pixels()[..4], [255, 0, 0, 255]);
        assert_eq!(image.pixels()[20 * 29 * 4..][..4], [0, 0, 0, 255]);
        fx.term.goto(1, 2);
        fx.graphics
            .sixel(body, &mut fx.term, (10, 20), [0, 0, 0], &fx.images);
        assert_eq!(fx.placed().len(), 1);
        assert_ne!(fx.placed()[0].0, first);
        assert_eq!(fx.images.lock().unwrap().len(), 1);
    }

    /// Output without pictures, read as a session reads it (the sniffer,
    /// then the parser), holds no room for any: not in the sniffer, the
    /// pictures, what is known of them or the screen's placements.
    #[test]
    fn output_without_pictures_holds_no_room_for_them() {
        use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};
        let mut fx = Fixture::new();
        let mut sniffer = crate::osc::Sniffer::default();
        let mut parser = Processor::<StdSyncHandler>::new();
        let output: &[u8] = b"\x1b[1;31mbold red\x1b[0m\r\n\x1b]0;title\x07\x1b]7;file://h/p\x07\
            \x1b]133;C\x07\x1b[?1049h\x1b[2J\x1b[Halternate\x1b[?1049l\x1bPdcs\x1b\\\
            \x1b_Xother\x1b\\\x1b[3J\x1bc";
        for _ in 0..100 {
            let mut parsed = 0;
            sniffer.feed(output, |end, osc| {
                parser.advance(&mut fx.term, &output[parsed..end]);
                parsed = end;
                if let crate::osc::Osc::Graphics(command) = osc {
                    fx.graphics
                        .command(&command, &mut fx.term, (10, 20), &fx.images);
                }
            });
            parser.advance(&mut fx.term, &output[parsed..]);
        }
        assert_eq!(sniffer.graphics_held(), 0);
        assert_eq!(fx.images.lock().unwrap().capacity(), 0);
        assert_eq!(fx.graphics.held.capacity(), 0);
        assert!(fx.graphics.loading.is_none());
        assert!(fx.term.placements().is_empty());
    }

    /// Over the quota, the oldest pictures go, those not on the screen
    /// first.
    #[test]
    fn over_the_quota_the_oldest_unplaced_pictures_go() {
        let mut fx = Fixture::new();
        // Each 200 by 200: 160 KB; six fit in the tests' 1 MB.
        let big = rgba(200, 200);
        fx.run("a=T,f=32,s=200,v=200,i=1,q=1", &big);
        for id in 2..=7 {
            fx.run(&format!("a=t,f=32,s=200,v=200,i={id},q=1"), &big);
        }
        let mut held: Vec<u32> = fx.images.lock().unwrap().keys().copied().collect();
        held.sort();
        // 2 went, the oldest not placed; 1, placed, stayed.
        assert_eq!(held, [1, 3, 4, 5, 6, 7]);
        assert_eq!(fx.graphics.bytes, 6 * 160_000);
    }
}
