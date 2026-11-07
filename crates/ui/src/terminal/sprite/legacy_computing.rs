//! Port of Ghostty's src/font/sprite/draw/symbols_for_legacy_computing.zig.
//! Symbols for Legacy Computing | U+1FB00...U+1FBFF
//! https://en.wikipedia.org/wiki/Symbols_for_Legacy_Computing
//!
//! 🬀 🬁 🬂 🬃 🬄 🬅 🬆 🬇 🬈 🬉 🬊 🬋 🬌 🬍 🬎 🬏
//! 🬐 🬑 🬒 🬓 🬔 🬕 🬖 🬗 🬘 🬙 🬚 🬛 🬜 🬝 🬞 🬟
//! 🬠 🬡 🬢 🬣 🬤 🬥 🬦 🬧 🬨 🬩 🬪 🬫 🬬 🬭 🬮 🬯
//! 🬰 🬱 🬲 🬳 🬴 🬵 🬶 🬷 🬸 🬹 🬺 🬻 🬼 🬽 🬾 🬿
//! 🭀 🭁 🭂 🭃 🭄 🭅 🭆 🭇 🭈 🭉 🭊 🭋 🭌 🭍 🭎 🭏
//! 🭐 🭑 🭒 🭓 🭔 🭕 🭖 🭗 🭘 🭙 🭚 🭛 🭜 🭝 🭞 🭟
//! 🭠 🭡 🭢 🭣 🭤 🭥 🭦 🭧 🭨 🭩 🭪 🭫 🭬 🭭 🭮 🭯
//! 🭰 🭱 🭲 🭳 🭴 🭵 🭶 🭷 🭸 🭹 🭺 🭻 🭼 🭽 🭾 🭿
//! 🮀 🮁 🮂 🮃 🮄 🮅 🮆 🮇 🮈 🮉 🮊 🮋 🮌 🮍 🮎 🮏
//! 🮐 🮑 🮒   🮔 🮕 🮖 🮗 🮘 🮙 🮚 🮛 🮜 🮝 🮞 🮟
//! 🮠 🮡 🮢 🮣 🮤 🮥 🮦 🮧 🮨 🮩 🮪 🮫 🮬 🮭 🮮 🮯
//! 🮽 🮾 🮿 🯎 🯏
//! 🯐 🯑 🯒 🯓 🯔 🯕 🯖 🯗 🯘 🯙 🯚 🯛 🯜 🯝 🯞 🯟
//! 🯠 🯡 🯢 🯣 🯤 🯥 🯦 🯧 🯨 🯩 🯪 🯫 🯬 🯭 🯮 🯯

use tiny_skia::PathBuilder;

use super::Metrics;
use super::block::{block, block_shade, full_block_shade};
use super::box_drawing::{self, Lines, Style};
use super::canvas::{self, Canvas, ON, Rect, pt};
use super::common::{
    Alignment, Corner, Edge, FIVE_EIGHTHS, Fraction, HALF, Horizontal, ONE_EIGHTH, ONE_QUARTER,
    ONE_THIRD, Quads, SEVEN_EIGHTHS, Shade, THREE_EIGHTHS, THREE_QUARTERS, TWO_THIRDS, Thickness,
    Vertical, fill,
};
use super::geometric_shapes::corner_triangle_shade;

/// Which of the ten outline points of a smooth mosaic are on its path, in
/// path order (counter-clockwise from the top left).
#[derive(Clone, Copy, Debug)]
struct SmoothMosaic {
    tl: bool,
    ul: bool,
    ll: bool,
    bl: bool,
    bc: bool,
    br: bool,
    lr: bool,
    ur: bool,
    tr: bool,
    tc: bool,
}

impl SmoothMosaic {
    /// From a 3×4 picture of the glyph's points: `#` is on the outline,
    /// anything else is not. A point between two set corners of the same
    /// edge is dropped (it would lie on a straight line anyway).
    fn from(rows: [&str; 4]) -> SmoothMosaic {
        let p: Vec<bool> = rows
            .iter()
            .flat_map(|r| r.bytes().map(|b| b == b'#'))
            .collect();
        debug_assert_eq!(p.len(), 12);
        // Ghostty's index i into "abc\ndef\n…" is row i/4, column i%4.
        let h = |i: usize| p[(i / 4) * 3 + i % 4];
        SmoothMosaic {
            tl: h(0),
            ul: h(4) && (!h(0) || !h(8)),
            ll: h(8) && (!h(4) || !h(12)),
            bl: h(12),
            bc: h(13) && (!h(12) || !h(14)),
            br: h(14),
            lr: h(10) && (!h(14) || !h(6)),
            ur: h(6) && (!h(10) || !h(2)),
            tr: h(2),
            tc: h(1) && (!h(2) || !h(0)),
        }
    }
}

/// Sextants
pub fn draw_1fb00_1fb3b(
    cp: u32,
    canvas: &mut Canvas,
    _width: u32,
    _height: u32,
    metrics: &Metrics,
) {
    use Fraction::{Half, One, OneThird, TwoThirds, Zero};
    let idx = cp - 0x1fb00;
    // Bits, low to high: tl, tr, ml, mr, bl, br. The codepoints skip the
    // empty pattern and the two half blocks.
    let sex = idx + (idx / 0x14) + 1;
    let bit = |n: u32| sex & (1 << n) != 0;
    if bit(0) {
        fill(metrics, canvas, Zero, Half, Zero, OneThird);
    }
    if bit(1) {
        fill(metrics, canvas, Half, One, Zero, OneThird);
    }
    if bit(2) {
        fill(metrics, canvas, Zero, Half, OneThird, TwoThirds);
    }
    if bit(3) {
        fill(metrics, canvas, Half, One, OneThird, TwoThirds);
    }
    if bit(4) {
        fill(metrics, canvas, Zero, Half, TwoThirds, One);
    }
    if bit(5) {
        fill(metrics, canvas, Half, One, TwoThirds, One);
    }
}

/// Smooth Mosaics
pub fn draw_1fb3c_1fb67(
    cp: u32,
    canvas: &mut Canvas,
    _width: u32,
    _height: u32,
    metrics: &Metrics,
) {
    // Hand written lookup table for these shapes since there's no
    // mathematical pattern in the codepoints.
    let rows: [&str; 4] = match cp {
        0x1fb3c => ["...", "...", "#..", "##."],  // 🬼
        0x1fb3d => ["...", "...", "#\\.", "###"], // 🬽
        0x1fb3e => ["...", "#..", "#\\.", "##."], // 🬾
        0x1fb3f => ["...", "#..", "##.", "###"],  // 🬿
        0x1fb40 => ["#..", "#..", "##.", "##."],  // 🭀
        0x1fb41 => ["/##", "###", "###", "###"],  // 🭁
        0x1fb42 => ["./#", "###", "###", "###"],  // 🭂
        0x1fb43 => [".##", ".##", "###", "###"],  // 🭃
        0x1fb44 => ["..#", ".##", "###", "###"],  // 🭄
        0x1fb45 => [".##", ".##", ".##", "###"],  // 🭅
        0x1fb46 => ["...", "./#", "###", "###"],  // 🭆
        0x1fb47 => ["...", "...", "..#", ".##"],  // 🭇
        0x1fb48 => ["...", "...", "./#", "###"],  // 🭈
        0x1fb49 => ["...", "..#", "./#", ".##"],  // 🭉
        0x1fb4a => ["...", "..#", ".##", "###"],  // 🭊
        0x1fb4b => ["..#", "..#", ".##", ".##"],  // 🭋
        0x1fb4c => ["##\\", "###", "###", "###"], // 🭌
        0x1fb4d => ["#\\.", "###", "###", "###"], // 🭍
        0x1fb4e => ["##.", "##.", "###", "###"],  // 🭎
        0x1fb4f => ["#..", "##.", "###", "###"],  // 🭏
        0x1fb50 => ["##.", "##.", "##.", "###"],  // 🭐
        0x1fb51 => ["...", "#\\.", "###", "###"], // 🭑
        0x1fb52 => ["###", "###", "###", "\\##"], // 🭒
        0x1fb53 => ["###", "###", "###", ".\\#"], // 🭓
        0x1fb54 => ["###", "###", ".##", ".##"],  // 🭔
        0x1fb55 => ["###", "###", ".##", "..#"],  // 🭕
        0x1fb56 => ["###", ".##", ".##", ".##"],  // 🭖
        0x1fb57 => ["##.", "#..", "...", "..."],  // 🭗
        0x1fb58 => ["###", "#/.", "...", "..."],  // 🭘
        0x1fb59 => ["##.", "#/.", "#..", "..."],  // 🭙
        0x1fb5a => ["###", "##.", "#..", "..."],  // 🭚
        0x1fb5b => ["##.", "##.", "#..", "#.."],  // 🭛
        0x1fb5c => ["###", "###", "#/.", "..."],  // 🭜
        0x1fb5d => ["###", "###", "###", "##/"],  // 🭝
        0x1fb5e => ["###", "###", "###", "#/."],  // 🭞
        0x1fb5f => ["###", "###", "##.", "##."],  // 🭟
        0x1fb60 => ["###", "###", "##.", "#.."],  // 🭠
        0x1fb61 => ["###", "##.", "##.", "##."],  // 🭡
        0x1fb62 => [".##", "..#", "...", "..."],  // 🭢
        0x1fb63 => ["###", ".\\#", "...", "..."], // 🭣
        0x1fb64 => [".##", ".\\#", "..#", "..."], // 🭤
        0x1fb65 => ["###", ".##", "..#", "..."],  // 🭥
        0x1fb66 => [".##", ".##", "..#", "..#"],  // 🭦
        0x1fb67 => ["###", "###", ".\\#", "..."], // 🭧
        _ => return,
    };
    let mosaic = SmoothMosaic::from(rows);

    let top = 0.0;
    let upper = Fraction::OneThird.float(metrics.cell_height);
    let lower = Fraction::TwoThirds.float(metrics.cell_height);
    let bottom = metrics.cell_height as f64;
    let left = 0.0;
    let center = Fraction::Half.float(metrics.cell_width);
    let right = metrics.cell_width as f64;

    let points = [
        (mosaic.tl, left, top),
        (mosaic.ul, left, upper),
        (mosaic.ll, left, lower),
        (mosaic.bl, left, bottom),
        (mosaic.bc, center, bottom),
        (mosaic.br, right, bottom),
        (mosaic.lr, right, lower),
        (mosaic.ur, right, upper),
        (mosaic.tr, right, top),
        (mosaic.tc, center, top),
    ];
    // z2d starts the path at the first lineTo; tiny-skia needs a moveTo.
    let mut pb = PathBuilder::new();
    for (x, y) in points
        .iter()
        .filter(|p| p.0)
        .map(|p| (p.1 as f32, p.2 as f32))
    {
        if pb.is_empty() {
            pb.move_to(x, y);
        } else {
            pb.line_to(x, y);
        }
    }
    pb.close();
    if let Some(path) = pb.finish() {
        canvas.fill_path(&path, ON);
    }
}

pub fn draw_1fb68_1fb6f(
    cp: u32,
    canvas: &mut Canvas,
    _width: u32,
    _height: u32,
    metrics: &Metrics,
) {
    let (edge, inverted) = match cp {
        0x1fb68 => (Edge::Left, true),    // 🭨
        0x1fb69 => (Edge::Top, true),     // 🭩
        0x1fb6a => (Edge::Right, true),   // 🭪
        0x1fb6b => (Edge::Bottom, true),  // 🭫
        0x1fb6c => (Edge::Left, false),   // 🭬
        0x1fb6d => (Edge::Top, false),    // 🭭
        0x1fb6e => (Edge::Right, false),  // 🭮
        0x1fb6f => (Edge::Bottom, false), // 🭯
        _ => return,
    };
    edge_triangle(metrics, canvas, edge);
    if inverted {
        canvas.invert();
        clip_to_cell(canvas);
    }
}

/// Vertical one eighth blocks
pub fn draw_1fb70_1fb75(
    cp: u32,
    canvas: &mut Canvas,
    _width: u32,
    _height: u32,
    metrics: &Metrics,
) {
    let n = (cp + 1 - 0x1fb70) as usize;
    fill(
        metrics,
        canvas,
        Fraction::EIGHTHS[n],
        Fraction::EIGHTHS[n + 1],
        Fraction::Zero,
        Fraction::One,
    );
}

/// Horizontal one eighth blocks
pub fn draw_1fb76_1fb7b(
    cp: u32,
    canvas: &mut Canvas,
    _width: u32,
    _height: u32,
    metrics: &Metrics,
) {
    let n = (cp + 1 - 0x1fb76) as usize;
    fill(
        metrics,
        canvas,
        Fraction::Zero,
        Fraction::One,
        Fraction::EIGHTHS[n],
        Fraction::EIGHTHS[n + 1],
    );
}

pub fn draw_1fb7c_1fb97(cp: u32, canvas: &mut Canvas, width: u32, height: u32, metrics: &Metrics) {
    let m = metrics;
    match cp {
        // '🭼' LEFT AND LOWER ONE EIGHTH BLOCK
        0x1fb7c => {
            block(m, canvas, Alignment::LEFT, ONE_EIGHTH, 1.0);
            block(m, canvas, Alignment::LOWER, 1.0, ONE_EIGHTH);
        }
        // '🭽' LEFT AND UPPER ONE EIGHTH BLOCK
        0x1fb7d => {
            block(m, canvas, Alignment::LEFT, ONE_EIGHTH, 1.0);
            block(m, canvas, Alignment::UPPER, 1.0, ONE_EIGHTH);
        }
        // '🭾' RIGHT AND UPPER ONE EIGHTH BLOCK
        0x1fb7e => {
            block(m, canvas, Alignment::RIGHT, ONE_EIGHTH, 1.0);
            block(m, canvas, Alignment::UPPER, 1.0, ONE_EIGHTH);
        }
        // '🭿' RIGHT AND LOWER ONE EIGHTH BLOCK
        0x1fb7f => {
            block(m, canvas, Alignment::RIGHT, ONE_EIGHTH, 1.0);
            block(m, canvas, Alignment::LOWER, 1.0, ONE_EIGHTH);
        }
        // '🮀' UPPER AND LOWER ONE EIGHTH BLOCK
        0x1fb80 => {
            block(m, canvas, Alignment::UPPER, 1.0, ONE_EIGHTH);
            block(m, canvas, Alignment::LOWER, 1.0, ONE_EIGHTH);
        }
        // '🮁' Horizontal One Eighth Block 1358: the horizontal eighth
        // blocks at rows 1, 3, 5 and 8 (0x1fb75 is just before that range
        // and gives row 0).
        0x1fb81 => {
            for k in [1, 3, 5, 8] {
                draw_1fb76_1fb7b(0x1fb74 + k, canvas, width, height, m);
            }
        }

        // '🮂' UPPER ONE QUARTER BLOCK
        0x1fb82 => block(m, canvas, Alignment::UPPER, 1.0, ONE_QUARTER),
        // '🮃' UPPER THREE EIGHTHS BLOCK
        0x1fb83 => block(m, canvas, Alignment::UPPER, 1.0, THREE_EIGHTHS),
        // '🮄' UPPER FIVE EIGHTHS BLOCK
        0x1fb84 => block(m, canvas, Alignment::UPPER, 1.0, FIVE_EIGHTHS),
        // '🮅' UPPER THREE QUARTERS BLOCK
        0x1fb85 => block(m, canvas, Alignment::UPPER, 1.0, THREE_QUARTERS),
        // '🮆' UPPER SEVEN EIGHTHS BLOCK
        0x1fb86 => block(m, canvas, Alignment::UPPER, 1.0, SEVEN_EIGHTHS),

        // '🮇' RIGHT ONE QUARTER BLOCK
        0x1fb87 => block(m, canvas, Alignment::RIGHT, ONE_QUARTER, 1.0),
        // '🮈' RIGHT THREE EIGHTHS BLOCK
        0x1fb88 => block(m, canvas, Alignment::RIGHT, THREE_EIGHTHS, 1.0),
        // '🮉' RIGHT FIVE EIGHTHS BLOCK
        0x1fb89 => block(m, canvas, Alignment::RIGHT, FIVE_EIGHTHS, 1.0),
        // '🮊' RIGHT THREE QUARTERS BLOCK
        0x1fb8a => block(m, canvas, Alignment::RIGHT, THREE_QUARTERS, 1.0),
        // '🮋' RIGHT SEVEN EIGHTHS BLOCK
        0x1fb8b => block(m, canvas, Alignment::RIGHT, SEVEN_EIGHTHS, 1.0),

        // '🮌'
        0x1fb8c => block_shade(m, canvas, Alignment::LEFT, HALF, 1.0, Shade::Medium),
        // '🮍'
        0x1fb8d => block_shade(m, canvas, Alignment::RIGHT, HALF, 1.0, Shade::Medium),
        // '🮎'
        0x1fb8e => block_shade(m, canvas, Alignment::UPPER, 1.0, HALF, Shade::Medium),
        // '🮏'
        0x1fb8f => block_shade(m, canvas, Alignment::LOWER, 1.0, HALF, Shade::Medium),

        // '🮐'
        0x1fb90 => full_block_shade(m, canvas, Shade::Medium),
        // '🮑'
        0x1fb91 => {
            full_block_shade(m, canvas, Shade::Medium);
            block(m, canvas, Alignment::UPPER, 1.0, HALF);
        }
        // '🮒'
        0x1fb92 => {
            full_block_shade(m, canvas, Shade::Medium);
            block(m, canvas, Alignment::LOWER, 1.0, HALF);
        }
        // Unallocated (a hole in the block): an empty glyph.
        0x1fb93 => {}
        // '🮔'
        0x1fb94 => {
            full_block_shade(m, canvas, Shade::Medium);
            block(m, canvas, Alignment::RIGHT, HALF, 1.0);
        }
        // '🮕'
        0x1fb95 => checkerboard_fill(m, canvas, 0),
        // '🮖'
        0x1fb96 => checkerboard_fill(m, canvas, 1),
        // '🮗'
        0x1fb97 => {
            canvas.box_(
                0,
                (height / 4) as i32,
                width as i32,
                (2 * height / 4) as i32,
                ON,
            );
            canvas.box_(0, (3 * height / 4) as i32, width as i32, height as i32, ON);
        }
        _ => {}
    }
}

/// Upper Left to Lower Right Fill
/// 🮘
pub fn draw_1fb98(_cp: u32, canvas: &mut Canvas, _width: u32, _height: u32, metrics: &Metrics) {
    diagonal_fill(metrics, canvas, false);
}

/// Upper Right to Lower Left Fill
/// 🮙
pub fn draw_1fb99(_cp: u32, canvas: &mut Canvas, _width: u32, _height: u32, metrics: &Metrics) {
    diagonal_fill(metrics, canvas, true);
}

/// The shared body of `draw1FB98` and `draw1FB99`: parallel diagonals
/// across the cell. Like Ghostty, this doesn't align for most cell sizes.
fn diagonal_fill(metrics: &Metrics, canvas: &mut Canvas, upper_right: bool) {
    clip_to_cell(canvas);

    let thick_px = Thickness::Light.height(metrics.box_thickness);
    let line_count = metrics.cell_width / (2 * thick_px);

    let float_width = metrics.cell_width as f64;
    let float_height = metrics.cell_height as f64;
    let float_thick = thick_px as f64;
    let stride = (float_width / line_count as f64).round();

    for i in 0..line_count * 2 + 1 {
        let i = i as i32 - line_count as i32;
        let near_x = i as f64 * stride;
        let far_x = float_width + near_x;
        let (top_x, bottom_x) = if upper_right {
            (far_x, near_x)
        } else {
            (near_x, far_x)
        };
        canvas.line(pt(top_x, 0.0), pt(bottom_x, float_height), float_thick, ON);
    }
}

pub fn draw_1fb9a_1fb9f(
    cp: u32,
    canvas: &mut Canvas,
    _width: u32,
    _height: u32,
    metrics: &Metrics,
) {
    match cp {
        // '🮚'
        0x1fb9a => {
            edge_triangle(metrics, canvas, Edge::Top);
            edge_triangle(metrics, canvas, Edge::Bottom);
        }
        // '🮛'
        0x1fb9b => {
            edge_triangle(metrics, canvas, Edge::Left);
            edge_triangle(metrics, canvas, Edge::Right);
        }
        // '🮜'
        0x1fb9c => corner_triangle_shade(metrics, canvas, Corner::Tl, Shade::Medium),
        // '🮝'
        0x1fb9d => corner_triangle_shade(metrics, canvas, Corner::Tr, Shade::Medium),
        // '🮞'
        0x1fb9e => corner_triangle_shade(metrics, canvas, Corner::Br, Shade::Medium),
        // '🮟'
        0x1fb9f => corner_triangle_shade(metrics, canvas, Corner::Bl, Shade::Medium),
        _ => {}
    }
}

pub fn draw_1fba0_1fbae(
    cp: u32,
    canvas: &mut Canvas,
    _width: u32,
    _height: u32,
    metrics: &Metrics,
) {
    let q = |tl, tr, bl, br| Quads { tl, tr, bl, br };
    let corners = match cp {
        0x1fba0 => q(true, false, false, false), // 🮠
        0x1fba1 => q(false, true, false, false), // 🮡
        0x1fba2 => q(false, false, true, false), // 🮢
        0x1fba3 => q(false, false, false, true), // 🮣
        0x1fba4 => q(true, false, true, false),  // 🮤
        0x1fba5 => q(false, true, false, true),  // 🮥
        0x1fba6 => q(false, false, true, true),  // 🮦
        0x1fba7 => q(true, true, false, false),  // 🮧
        0x1fba8 => q(true, false, false, true),  // 🮨
        0x1fba9 => q(false, true, true, false),  // 🮩
        0x1fbaa => q(false, true, true, true),   // 🮪
        0x1fbab => q(true, false, true, true),   // 🮫
        0x1fbac => q(true, true, false, true),   // 🮬
        0x1fbad => q(true, true, true, false),   // 🮭
        0x1fbae => q(true, true, true, true),    // 🮮
        _ => return,
    };
    corner_diagonal_lines(metrics, canvas, corners);
}

/// 🮯
pub fn draw_1fbaf(_cp: u32, canvas: &mut Canvas, _width: u32, _height: u32, metrics: &Metrics) {
    box_drawing::lines_char(
        metrics,
        canvas,
        Lines {
            up: Style::Heavy,
            down: Style::Heavy,
            left: Style::Light,
            right: Style::Light,
        },
    );
}

/// 🮽
pub fn draw_1fbbd(_cp: u32, canvas: &mut Canvas, _width: u32, _height: u32, metrics: &Metrics) {
    box_drawing::light_diagonal_cross(metrics, canvas);
    canvas.invert();
    clip_to_cell(canvas);
}

/// 🮾
pub fn draw_1fbbe(_cp: u32, canvas: &mut Canvas, _width: u32, _height: u32, metrics: &Metrics) {
    corner_diagonal_lines(
        metrics,
        canvas,
        Quads {
            br: true,
            ..Quads::default()
        },
    );
    canvas.invert();
    clip_to_cell(canvas);
}

/// 🮿
pub fn draw_1fbbf(_cp: u32, canvas: &mut Canvas, _width: u32, _height: u32, metrics: &Metrics) {
    corner_diagonal_lines(
        metrics,
        canvas,
        Quads {
            tl: true,
            tr: true,
            bl: true,
            br: true,
        },
    );
    canvas.invert();
    clip_to_cell(canvas);
}

/// 🯎
pub fn draw_1fbce(_cp: u32, canvas: &mut Canvas, _width: u32, _height: u32, metrics: &Metrics) {
    block(metrics, canvas, Alignment::LEFT, TWO_THIRDS, 1.0);
}

/// 🯏
pub fn draw_1fbcf(_cp: u32, canvas: &mut Canvas, _width: u32, _height: u32, metrics: &Metrics) {
    block(metrics, canvas, Alignment::LEFT, ONE_THIRD, 1.0);
}

/// Cell diagonals.
pub fn draw_1fbd0_1fbdf(
    cp: u32,
    canvas: &mut Canvas,
    _width: u32,
    _height: u32,
    metrics: &Metrics,
) {
    use Alignment as A;
    let m = metrics;
    match cp {
        0x1fbd0 => cell_diagonal(m, canvas, A::RIGHT, A::LOWER_LEFT), // 🯐
        0x1fbd1 => cell_diagonal(m, canvas, A::UPPER_RIGHT, A::LEFT), // 🯑
        0x1fbd2 => cell_diagonal(m, canvas, A::UPPER_LEFT, A::RIGHT), // 🯒
        0x1fbd3 => cell_diagonal(m, canvas, A::LEFT, A::LOWER_RIGHT), // 🯓
        0x1fbd4 => cell_diagonal(m, canvas, A::UPPER_LEFT, A::LOWER), // 🯔
        0x1fbd5 => cell_diagonal(m, canvas, A::UPPER, A::LOWER_RIGHT), // 🯕
        0x1fbd6 => cell_diagonal(m, canvas, A::UPPER_RIGHT, A::LOWER), // 🯖
        0x1fbd7 => cell_diagonal(m, canvas, A::UPPER, A::LOWER_LEFT), // 🯗
        // 🯘
        0x1fbd8 => {
            cell_diagonal(m, canvas, A::UPPER_LEFT, A::CENTER);
            cell_diagonal(m, canvas, A::CENTER, A::UPPER_RIGHT);
        }
        // 🯙
        0x1fbd9 => {
            cell_diagonal(m, canvas, A::UPPER_RIGHT, A::CENTER);
            cell_diagonal(m, canvas, A::CENTER, A::LOWER_RIGHT);
        }
        // 🯚
        0x1fbda => {
            cell_diagonal(m, canvas, A::LOWER_LEFT, A::CENTER);
            cell_diagonal(m, canvas, A::CENTER, A::LOWER_RIGHT);
        }
        // 🯛
        0x1fbdb => {
            cell_diagonal(m, canvas, A::UPPER_LEFT, A::CENTER);
            cell_diagonal(m, canvas, A::CENTER, A::LOWER_LEFT);
        }
        // 🯜
        0x1fbdc => {
            cell_diagonal(m, canvas, A::UPPER_LEFT, A::LOWER);
            cell_diagonal(m, canvas, A::LOWER, A::UPPER_RIGHT);
        }
        // 🯝
        0x1fbdd => {
            cell_diagonal(m, canvas, A::UPPER_RIGHT, A::LEFT);
            cell_diagonal(m, canvas, A::LEFT, A::LOWER_RIGHT);
        }
        // 🯞
        0x1fbde => {
            cell_diagonal(m, canvas, A::LOWER_LEFT, A::UPPER);
            cell_diagonal(m, canvas, A::UPPER, A::LOWER_RIGHT);
        }
        // 🯟
        0x1fbdf => {
            cell_diagonal(m, canvas, A::UPPER_LEFT, A::RIGHT);
            cell_diagonal(m, canvas, A::RIGHT, A::LOWER_LEFT);
        }
        _ => {}
    }
}

pub fn draw_1fbe0_1fbef(
    cp: u32,
    canvas: &mut Canvas,
    _width: u32,
    _height: u32,
    metrics: &Metrics,
) {
    use Alignment as A;
    let m = metrics;
    match cp {
        0x1fbe0 => circle(m, canvas, A::UPPER, false),   // 🯠
        0x1fbe1 => circle(m, canvas, A::RIGHT, false),   // 🯡
        0x1fbe2 => circle(m, canvas, A::LOWER, false),   // 🯢
        0x1fbe3 => circle(m, canvas, A::LEFT, false),    // 🯣
        0x1fbe4 => block(m, canvas, A::UPPER, 0.5, 0.5), // 🯤
        0x1fbe5 => block(m, canvas, A::LOWER, 0.5, 0.5), // 🯥
        0x1fbe6 => block(m, canvas, A::LEFT, 0.5, 0.5),  // 🯦
        0x1fbe7 => block(m, canvas, A::RIGHT, 0.5, 0.5), // 🯧
        0x1fbe8 => circle(m, canvas, A::UPPER, true),    // 🯨
        0x1fbe9 => circle(m, canvas, A::RIGHT, true),    // 🯩
        0x1fbea => circle(m, canvas, A::LOWER, true),    // 🯪
        0x1fbeb => circle(m, canvas, A::LEFT, true),     // 🯫
        0x1fbec => circle(m, canvas, A::UPPER_RIGHT, true), // 🯬
        0x1fbed => circle(m, canvas, A::LOWER_LEFT, true), // 🯭
        0x1fbee => circle(m, canvas, A::LOWER_RIGHT, true), // 🯮
        0x1fbef => circle(m, canvas, A::UPPER_LEFT, true), // 🯯
        _ => {}
    }
}

/// Clip the glyph to its cell so nothing drawn in the padding shows.
fn clip_to_cell(canvas: &mut Canvas) {
    canvas.clip_left = canvas.padding_x;
    canvas.clip_right = canvas.padding_x;
    canvas.clip_top = canvas.padding_y;
    canvas.clip_bottom = canvas.padding_y;
}

/// A triangle from the cell's center to the two corners of `edge`.
fn edge_triangle(metrics: &Metrics, canvas: &mut Canvas, edge: Edge) {
    let upper = 0.0;
    let middle = (metrics.cell_height as f64 / 2.0).round();
    let lower = metrics.cell_height as f64;
    let left = 0.0;
    let center = (metrics.cell_width as f64 / 2.0).round();
    let right = metrics.cell_width as f64;

    let (x0, y0, x1, y1) = match edge {
        Edge::Top => (right, upper, left, upper),
        Edge::Left => (left, upper, left, lower),
        Edge::Bottom => (left, lower, right, lower),
        Edge::Right => (right, lower, right, upper),
    };
    canvas.triangle([pt(center, middle), pt(x0, y0), pt(x1, y1)], ON);
}

/// Lines from the middle of the top/bottom edge to the middle of the
/// left/right edge, one per selected corner.
fn corner_diagonal_lines(metrics: &Metrics, canvas: &mut Canvas, corners: Quads) {
    let thick_px = Thickness::Light.height(metrics.box_thickness);

    let float_width = metrics.cell_width as f64;
    let float_height = metrics.cell_height as f64;
    let float_thick = thick_px as f64;
    let center_x = (metrics.cell_width / 2 + metrics.cell_width % 2) as f64;
    let center_y = (metrics.cell_height / 2 + metrics.cell_height % 2) as f64;

    if corners.tl {
        canvas.line(pt(center_x, 0.0), pt(0.0, center_y), float_thick, ON);
    }
    if corners.tr {
        canvas.line(
            pt(center_x, 0.0),
            pt(float_width, center_y),
            float_thick,
            ON,
        );
    }
    if corners.bl {
        canvas.line(
            pt(center_x, float_height),
            pt(0.0, center_y),
            float_thick,
            ON,
        );
    }
    if corners.br {
        canvas.line(
            pt(center_x, float_height),
            pt(float_width, center_y),
            float_thick,
            ON,
        );
    }
}

/// Where an alignment puts a point on the cell's outline or center.
fn anchor(metrics: &Metrics, a: Alignment) -> (f64, f64) {
    let float_width = metrics.cell_width as f64;
    let float_height = metrics.cell_height as f64;
    let x = match a.horizontal {
        Horizontal::Left => 0.0,
        Horizontal::Right => float_width,
        Horizontal::Center => float_width / 2.0,
    };
    let y = match a.vertical {
        Vertical::Top => 0.0,
        Vertical::Bottom => float_height,
        Vertical::Middle => float_height / 2.0,
    };
    (x, y)
}

fn cell_diagonal(metrics: &Metrics, canvas: &mut Canvas, from: Alignment, to: Alignment) {
    let (x0, y0) = anchor(metrics, from);
    let (x1, y1) = anchor(metrics, to);
    canvas.line(
        pt(x0, y0),
        pt(x1, y1),
        Thickness::Light.height(metrics.box_thickness) as f64,
        ON,
    );
}

fn checkerboard_fill(metrics: &Metrics, canvas: &mut Canvas, parity: u32) {
    let float_width = metrics.cell_width as f64;
    let float_height = metrics.cell_height as f64;
    let x_size: u32 = 4;
    let y_size = (4.0 * (float_height / float_width)).round() as u32;
    for x in 0..x_size {
        let x0 = (metrics.cell_width * x) / x_size;
        let x1 = (metrics.cell_width * (x + 1)) / x_size;
        for y in 0..y_size {
            let y0 = (metrics.cell_height * y) / y_size;
            let y1 = (metrics.cell_height * (y + 1)) / y_size;
            if (x + y) % 2 == parity {
                canvas.rect(
                    Rect {
                        x: x0 as i32,
                        y: y0 as i32,
                        width: x1.saturating_sub(x0) as i32,
                        height: y1.saturating_sub(y0) as i32,
                    },
                    ON,
                );
            }
        }
    }
}

/// A circle of diameter min(cell width, height) centered on `position`,
/// filled or stroked with a light line inside its edge; clipped to the cell.
pub fn circle(metrics: &Metrics, canvas: &mut Canvas, position: Alignment, filled: bool) {
    clip_to_cell(canvas);

    let (x, y) = anchor(metrics, position);
    let r = 0.5 * (metrics.cell_width as f64).min(metrics.cell_height as f64);
    let line_width = Thickness::Light.height(metrics.box_thickness) as f64;

    let mut pb = PathBuilder::new();
    if filled {
        pb.push_circle(x as f32, y as f32, r as f32);
        if let Some(path) = pb.finish() {
            canvas.fill_path(&path, ON);
        }
    } else {
        pb.push_circle(x as f32, y as f32, (r - line_width / 2.0) as f32);
        if let Some(path) = pb.finish() {
            canvas.stroke_path(&path, &canvas::stroke(line_width), ON);
        }
    }
}
