//! Port of Ghostty's src/font/sprite/draw/box.zig.
//! Box Drawing | U+2500...U+257F
//!
//! ─━│┃┄┅┆┇┈┉┊┋┌┍┎┏
//! ┐┑┒┓└┕┖┗┘┙┚┛├┝┞┟
//! ┠┡┢┣┤┥┦┧┨┩┪┫┬┭┮┯
//! ┰┱┲┳┴┵┶┷┸┹┺┻┼┽┾┿
//! ╀╁╂╃╄╅╆╇╈╉╊╋╌╍╎╏
//! ═║╒╓╔╕╖╗╘╙╚╛╜╝╞╟
//! ╠╡╢╣╤╥╦╧╨╩╪╫╬╭╮╯
//! ╰╱╲╳╴╵╶╷╸╹╺╻╼╽╾╿

use tiny_skia::PathBuilder;

use super::Metrics;
use super::canvas::{self, Canvas, ON, pt};
use super::common::{Corner, Thickness, hline, hline_middle, vline, vline_middle};

pub fn draw_2500_257f(cp: u32, canvas: &mut Canvas, _width: u32, _height: u32, metrics: &Metrics) {
    use Style::{Double as D, Heavy as H, Light as L, None as N};
    let m = metrics;
    // Lines in (up, right, down, left) order.
    let l = |up, right, down, left| Lines {
        up,
        right,
        down,
        left,
    };
    let light = Thickness::Light.height(m.box_thickness);
    let heavy = Thickness::Heavy.height(m.box_thickness);
    let gap = light.max(4);
    let lines = match cp {
        0x2500 => l(N, L, N, L),
        0x2501 => l(N, H, N, H),
        0x2502 => l(L, N, L, N),
        0x2503 => l(H, N, H, N),
        0x2504 => return dash_horizontal(m, canvas, 3, light, gap),
        0x2505 => return dash_horizontal(m, canvas, 3, heavy, gap),
        0x2506 => return dash_vertical(m, canvas, 3, light, gap),
        0x2507 => return dash_vertical(m, canvas, 3, heavy, gap),
        0x2508 => return dash_horizontal(m, canvas, 4, light, gap),
        0x2509 => return dash_horizontal(m, canvas, 4, heavy, gap),
        0x250a => return dash_vertical(m, canvas, 4, light, gap),
        0x250b => return dash_vertical(m, canvas, 4, heavy, gap),
        0x250c => l(N, L, L, N),
        0x250d => l(N, H, L, N),
        0x250e => l(N, L, H, N),
        0x250f => l(N, H, H, N),

        0x2510 => l(N, N, L, L),
        0x2511 => l(N, N, L, H),
        0x2512 => l(N, N, H, L),
        0x2513 => l(N, N, H, H),
        0x2514 => l(L, L, N, N),
        0x2515 => l(L, H, N, N),
        0x2516 => l(H, L, N, N),
        0x2517 => l(H, H, N, N),
        0x2518 => l(L, N, N, L),
        0x2519 => l(L, N, N, H),
        0x251a => l(H, N, N, L),
        0x251b => l(H, N, N, H),
        0x251c => l(L, L, L, N),
        0x251d => l(L, H, L, N),
        0x251e => l(H, L, L, N),
        0x251f => l(L, L, H, N),

        0x2520 => l(H, L, H, N),
        0x2521 => l(H, H, L, N),
        0x2522 => l(L, H, H, N),
        0x2523 => l(H, H, H, N),
        0x2524 => l(L, N, L, L),
        0x2525 => l(L, N, L, H),
        0x2526 => l(H, N, L, L),
        0x2527 => l(L, N, H, L),
        0x2528 => l(H, N, H, L),
        0x2529 => l(H, N, L, H),
        0x252a => l(L, N, H, H),
        0x252b => l(H, N, H, H),
        0x252c => l(N, L, L, L),
        0x252d => l(N, L, L, H),
        0x252e => l(N, H, L, L),
        0x252f => l(N, H, L, H),

        0x2530 => l(N, L, H, L),
        0x2531 => l(N, L, H, H),
        0x2532 => l(N, H, H, L),
        0x2533 => l(N, H, H, H),
        0x2534 => l(L, L, N, L),
        0x2535 => l(L, L, N, H),
        0x2536 => l(L, H, N, L),
        0x2537 => l(L, H, N, H),
        0x2538 => l(H, L, N, L),
        0x2539 => l(H, L, N, H),
        0x253a => l(H, H, N, L),
        0x253b => l(H, H, N, H),
        0x253c => l(L, L, L, L),
        0x253d => l(L, L, L, H),
        0x253e => l(L, H, L, L),
        0x253f => l(L, H, L, H),

        0x2540 => l(H, L, L, L),
        0x2541 => l(L, L, H, L),
        0x2542 => l(H, L, H, L),
        0x2543 => l(H, L, L, H),
        0x2544 => l(H, H, L, L),
        0x2545 => l(L, L, H, H),
        0x2546 => l(L, H, H, L),
        0x2547 => l(H, H, L, H),
        0x2548 => l(L, H, H, H),
        0x2549 => l(H, L, H, H),
        0x254a => l(H, H, H, L),
        0x254b => l(H, H, H, H),
        0x254c => return dash_horizontal(m, canvas, 2, light, light),
        0x254d => return dash_horizontal(m, canvas, 2, heavy, heavy),
        0x254e => return dash_vertical(m, canvas, 2, light, heavy),
        0x254f => return dash_vertical(m, canvas, 2, heavy, heavy),

        0x2550 => l(N, D, N, D),
        0x2551 => l(D, N, D, N),
        0x2552 => l(N, D, L, N),
        0x2553 => l(N, L, D, N),
        0x2554 => l(N, D, D, N),
        0x2555 => l(N, N, L, D),
        0x2556 => l(N, N, D, L),
        0x2557 => l(N, N, D, D),
        0x2558 => l(L, D, N, N),
        0x2559 => l(D, L, N, N),
        0x255a => l(D, D, N, N),
        0x255b => l(L, N, N, D),
        0x255c => l(D, N, N, L),
        0x255d => l(D, N, N, D),
        0x255e => l(L, D, L, N),
        0x255f => l(D, L, D, N),

        0x2560 => l(D, D, D, N),
        0x2561 => l(L, N, L, D),
        0x2562 => l(D, N, D, L),
        0x2563 => l(D, N, D, D),
        0x2564 => l(N, D, L, D),
        0x2565 => l(N, L, D, L),
        0x2566 => l(N, D, D, D),
        0x2567 => l(L, D, N, D),
        0x2568 => l(D, L, N, L),
        0x2569 => l(D, D, N, D),
        0x256a => l(L, D, L, D),
        0x256b => l(D, L, D, L),
        0x256c => l(D, D, D, D),
        0x256d => return arc(m, canvas, Corner::Br, Thickness::Light),
        0x256e => return arc(m, canvas, Corner::Bl, Thickness::Light),
        0x256f => return arc(m, canvas, Corner::Tl, Thickness::Light),

        0x2570 => return arc(m, canvas, Corner::Tr, Thickness::Light),
        0x2571 => return light_diagonal_upper_right_to_lower_left(m, canvas),
        0x2572 => return light_diagonal_upper_left_to_lower_right(m, canvas),
        0x2573 => return light_diagonal_cross(m, canvas),
        0x2574 => l(N, N, N, L),
        0x2575 => l(L, N, N, N),
        0x2576 => l(N, L, N, N),
        0x2577 => l(N, N, L, N),
        0x2578 => l(N, N, N, H),
        0x2579 => l(H, N, N, N),
        0x257a => l(N, H, N, N),
        0x257b => l(N, N, H, N),
        0x257c => l(N, H, N, L),
        0x257d => l(L, N, H, N),
        0x257e => l(N, L, N, H),
        0x257f => l(H, N, L, N),

        _ => return,
    };
    lines_char(m, canvas, lines);
}

/// One edge-to-center line style per side of a traditional box-drawing char.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Lines {
    pub up: Style,
    pub right: Style,
    pub down: Style,
    pub left: Style,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Style {
    #[default]
    None,
    Light,
    Heavy,
    Double,
}

pub fn lines_char(metrics: &Metrics, canvas: &mut Canvas, lines: Lines) {
    use Style::{Double, Heavy, Light, None};
    let light_px = Thickness::Light.height(metrics.box_thickness);
    let heavy_px = Thickness::Heavy.height(metrics.box_thickness);
    let cw = metrics.cell_width;
    let ch = metrics.cell_height;

    // Top/bottom of light horizontal strokes.
    let h_light_top = ch.saturating_sub(light_px) / 2;
    let h_light_bottom = h_light_top.saturating_add(light_px);
    // Top/bottom of heavy horizontal strokes.
    let h_heavy_top = ch.saturating_sub(heavy_px) / 2;
    let h_heavy_bottom = h_heavy_top.saturating_add(heavy_px);
    // Outer edges of the doubled horizontal strokes (inner edges are the light ones).
    let h_double_top = h_light_top.saturating_sub(light_px);
    let h_double_bottom = h_light_bottom.saturating_add(light_px);

    // Left/right of light vertical strokes.
    let v_light_left = cw.saturating_sub(light_px) / 2;
    let v_light_right = v_light_left.saturating_add(light_px);
    // Left/right of heavy vertical strokes.
    let v_heavy_left = cw.saturating_sub(heavy_px) / 2;
    let v_heavy_right = v_heavy_left.saturating_add(heavy_px);
    // Outer edges of the doubled vertical strokes.
    let v_double_left = v_light_left.saturating_sub(light_px);
    let v_double_right = v_light_right.saturating_add(light_px);

    // The bottom of the up line.
    let up_bottom = if lines.left == Heavy || lines.right == Heavy {
        h_heavy_bottom
    } else if lines.left != lines.right || lines.down == lines.up {
        if lines.left == Double || lines.right == Double {
            h_double_bottom
        } else {
            h_light_bottom
        }
    } else if lines.left == None && lines.right == None {
        h_light_bottom
    } else {
        h_light_top
    };

    // The top of the down line.
    let down_top = if lines.left == Heavy || lines.right == Heavy {
        h_heavy_top
    } else if lines.left != lines.right || lines.up == lines.down {
        if lines.left == Double || lines.right == Double {
            h_double_top
        } else {
            h_light_top
        }
    } else if lines.left == None && lines.right == None {
        h_light_top
    } else {
        h_light_bottom
    };

    // The right of the left line.
    let left_right = if lines.up == Heavy || lines.down == Heavy {
        v_heavy_right
    } else if lines.up != lines.down || lines.left == lines.right {
        if lines.up == Double || lines.down == Double {
            v_double_right
        } else {
            v_light_right
        }
    } else if lines.up == None && lines.down == None {
        v_light_right
    } else {
        v_light_left
    };

    // The left of the right line.
    let right_left = if lines.up == Heavy || lines.down == Heavy {
        v_heavy_left
    } else if lines.up != lines.down || lines.right == lines.left {
        if lines.up == Double || lines.down == Double {
            v_double_left
        } else {
            v_light_left
        }
    } else if lines.up == None && lines.down == None {
        v_light_left
    } else {
        v_light_right
    };

    let mut fill = |x0: u32, y0: u32, x1: u32, y1: u32| {
        canvas.box_(x0 as i32, y0 as i32, x1 as i32, y1 as i32, ON);
    };

    match lines.up {
        None => {}
        Light => fill(v_light_left, 0, v_light_right, up_bottom),
        Heavy => fill(v_heavy_left, 0, v_heavy_right, up_bottom),
        Double => {
            let left_bottom = if lines.left == Double {
                h_light_top
            } else {
                up_bottom
            };
            let right_bottom = if lines.right == Double {
                h_light_top
            } else {
                up_bottom
            };
            fill(v_double_left, 0, v_light_left, left_bottom);
            fill(v_light_right, 0, v_double_right, right_bottom);
        }
    }

    match lines.right {
        None => {}
        Light => fill(right_left, h_light_top, cw, h_light_bottom),
        Heavy => fill(right_left, h_heavy_top, cw, h_heavy_bottom),
        Double => {
            let top_left = if lines.up == Double {
                v_light_right
            } else {
                right_left
            };
            let bottom_left = if lines.down == Double {
                v_light_right
            } else {
                right_left
            };
            fill(top_left, h_double_top, cw, h_light_top);
            fill(bottom_left, h_light_bottom, cw, h_double_bottom);
        }
    }

    match lines.down {
        None => {}
        Light => fill(v_light_left, down_top, v_light_right, ch),
        Heavy => fill(v_heavy_left, down_top, v_heavy_right, ch),
        Double => {
            let left_top = if lines.left == Double {
                h_light_bottom
            } else {
                down_top
            };
            let right_top = if lines.right == Double {
                h_light_bottom
            } else {
                down_top
            };
            fill(v_double_left, left_top, v_light_left, ch);
            fill(v_light_right, right_top, v_double_right, ch);
        }
    }

    match lines.left {
        None => {}
        Light => fill(0, h_light_top, left_right, h_light_bottom),
        Heavy => fill(0, h_heavy_top, left_right, h_heavy_bottom),
        Double => {
            let top_right = if lines.up == Double {
                v_light_left
            } else {
                left_right
            };
            let bottom_right = if lines.down == Double {
                v_light_left
            } else {
                left_right
            };
            fill(0, h_double_top, top_right, h_light_top);
            fill(0, h_light_bottom, bottom_right, h_double_bottom);
        }
    }
}

/// The corner overshoot that keeps a diagonal's slope: (slope_x, slope_y).
fn diagonal_slope(metrics: &Metrics) -> (f64, f64, f64, f64) {
    let w = metrics.cell_width as f64;
    let h = metrics.cell_height as f64;
    (w, h, (w / h).min(1.0), (h / w).min(1.0))
}

pub fn light_diagonal_upper_right_to_lower_left(metrics: &Metrics, canvas: &mut Canvas) {
    let (w, h, sx, sy) = diagonal_slope(metrics);
    canvas.line(
        pt(w + 0.5 * sx, -0.5 * sy),
        pt(-0.5 * sx, h + 0.5 * sy),
        Thickness::Light.height(metrics.box_thickness) as f64,
        ON,
    );
}

pub fn light_diagonal_upper_left_to_lower_right(metrics: &Metrics, canvas: &mut Canvas) {
    let (w, h, sx, sy) = diagonal_slope(metrics);
    canvas.line(
        pt(-0.5 * sx, -0.5 * sy),
        pt(w + 0.5 * sx, h + 0.5 * sy),
        Thickness::Light.height(metrics.box_thickness) as f64,
        ON,
    );
}

pub fn light_diagonal_cross(metrics: &Metrics, canvas: &mut Canvas) {
    light_diagonal_upper_right_to_lower_left(metrics, canvas);
    light_diagonal_upper_left_to_lower_right(metrics, canvas);
}

pub fn arc(metrics: &Metrics, canvas: &mut Canvas, corner: Corner, thickness: Thickness) {
    let thick_px = thickness.height(metrics.box_thickness);
    let float_width = metrics.cell_width as f64;
    let float_height = metrics.cell_height as f64;
    let float_thick = thick_px as f64;
    let center_x = (metrics.cell_width.saturating_sub(thick_px) / 2) as f64 + float_thick / 2.0;
    let center_y = (metrics.cell_height.saturating_sub(thick_px) / 2) as f64 + float_thick / 2.0;

    let r = float_width.min(float_height) / 2.0;

    // Fraction away from the center to place the middle control points.
    let s = 0.25;

    // Vertical end (top or bottom edge) and horizontal direction.
    let (y_edge, dy) = match corner {
        Corner::Tl | Corner::Tr => (0.0, -1.0),
        Corner::Bl | Corner::Br => (float_height, 1.0),
    };
    let (x_edge, dx) = match corner {
        Corner::Tl | Corner::Bl => (0.0, -1.0),
        Corner::Tr | Corner::Br => (float_width, 1.0),
    };

    let mut pb = PathBuilder::new();
    pb.move_to(center_x as f32, y_edge as f32);
    pb.line_to(center_x as f32, (center_y + dy * r) as f32);
    pb.cubic_to(
        center_x as f32,
        (center_y + dy * s * r) as f32,
        (center_x + dx * s * r) as f32,
        center_y as f32,
        (center_x + dx * r) as f32,
        center_y as f32,
    );
    pb.line_to(x_edge as f32, center_y as f32);
    if let Some(path) = pb.finish() {
        canvas.stroke_path(&path, &canvas::stroke(float_thick), ON);
    }
}

fn dash_horizontal(
    metrics: &Metrics,
    canvas: &mut Canvas,
    count: u32,
    thick_px: u32,
    desired_gap: u32,
) {
    debug_assert!((2..=4).contains(&count));

    // For N dashes there are N - 1 gaps between them plus half-sized gaps
    // on either side, N gaps in total, so the line tiles evenly.
    let gap_count = count;

    // Without at least 1px for each gap and dash, draw a solid line.
    if metrics.cell_width < count + gap_count {
        hline_middle(metrics, canvas, Thickness::Light);
        return;
    }

    // Gaps never take up more than 50% of the space.
    let gap_width = desired_gap.min(metrics.cell_width / (2 * count)) as i32;
    let total_gap_width = gap_count as i32 * gap_width;
    let total_dash_width = metrics.cell_width as i32 - total_gap_width;
    let dash_width = total_dash_width.div_euclid(count as i32);
    let remaining = total_dash_width.rem_euclid(count as i32);

    // Dashes are centered vertically.
    let y = (metrics.cell_height.saturating_sub(thick_px) / 2) as i32;

    // Start half a gap from the left edge to center the dashes.
    let mut x = gap_width.div_euclid(2);

    // Left-over space goes into dash widths, 1px at a time.
    let mut extra = remaining;

    for _ in 0..count {
        let mut x1 = x + dash_width;
        if extra > 0 {
            extra -= 1;
            x1 += 1;
        }
        hline(canvas, x, x1, y, thick_px);
        x = x1 + gap_width;
    }
}

fn dash_vertical(
    metrics: &Metrics,
    canvas: &mut Canvas,
    count: u32,
    thick_px: u32,
    desired_gap: u32,
) {
    debug_assert!((2..=4).contains(&count));

    // One full-sized extra gap at the bottom (rather than two half-sized
    // ones) so the line joins solid characters cleanly: as many gaps as dashes.
    let gap_count = count;

    // Without at least 1px for each gap and dash, draw a solid line.
    if metrics.cell_height < count + gap_count {
        vline_middle(metrics, canvas, Thickness::Light);
        return;
    }

    // Gaps never take up more than 50% of the space.
    let gap_height = desired_gap.min(metrics.cell_height / (2 * count)) as i32;
    let total_gap_height = gap_count as i32 * gap_height;
    let total_dash_height = metrics.cell_height as i32 - total_gap_height;
    let dash_height = total_dash_height.div_euclid(count as i32);
    let remaining = total_dash_height.rem_euclid(count as i32);

    // Dashes are centered horizontally.
    let x = (metrics.cell_width.saturating_sub(thick_px) / 2) as i32;

    // Start at the top of the cell.
    let mut y = 0;

    // Left-over space goes into dash heights, 1px at a time.
    let mut extra = remaining;

    for _ in 0..count {
        let mut y1 = y + dash_height;
        if extra > 0 {
            extra -= 1;
            y1 += 1;
        }
        vline(canvas, y, y1, x, thick_px);
        y = y1 + gap_height;
    }
}
