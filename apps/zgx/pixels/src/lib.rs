//! Loops over every pixel or point of a frame of something moving (an
//! agent visual's scene, shader or canvas), optimized even in a debug
//! build of the app (see Cargo.toml).

/// Blue-green-red-alpha pixels as red-green-blue-alpha, in place (a frame
/// read back from crates/visual-gpu, for `zgx::image::ImageData`). A
/// trailing partial pixel is left as it is.
pub fn bgra_to_rgba(pixels: &mut [u8]) {
    for px in pixels.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
}

/// A fill crates/visual made for `g:circle`: its centre and radius. Only
/// its own shape counts (the point count `arc_points` gives that radius, the
/// first at 3 o'clock, every one as far from the centre), so a polygon the
/// script drew stays one.
pub fn circle_of(points: &[[f32; 2]]) -> Option<(f32, f32, f32)> {
    if points.len() < 5 {
        return None;
    }
    let n = points.len() as f32;
    let (cx, cy) = points.iter().fold((0., 0.), |(x, y), p| (x + p[0] / n, y + p[1] / n));
    let r = ((points[0][0] - cx).powi(2) + (points[0][1] - cy).powi(2)).sqrt();
    let segments = ((std::f32::consts::TAU * r.max(1.0)).sqrt() * 2.5).clamp(6.0, 160.0) as usize;
    let starts_east = (points[0][1] - cy).abs() <= r * 0.01 + 0.01 && points[0][0] > cx;
    let round = points.iter().all(|p| (((p[0] - cx).powi(2) + (p[1] - cy).powi(2)).sqrt() - r).abs() <= r * 0.01 + 0.01);
    (points.len() == segments && starts_east && round && r > 0.).then_some((cx, cy, r))
}

/// A polyline cut where its x turns back, into runs whose x only rises or
/// only falls (each a stroke zgx's canvas draws on the GPU, not
/// rasterized): each run's first and last point index, inclusive, the next
/// run starting where the last ended. Repeated points stay with their run.
pub fn monotone_runs(points: &[[f32; 2]]) -> Vec<(usize, usize)> {
    let mut runs = Vec::new();
    if points.len() < 2 {
        return runs;
    }
    let (mut start, mut dir) = (0usize, 0i8);
    for i in 1..points.len() {
        let dx = points[i][0] - points[i - 1][0];
        let step = if dx > 0. { 1 } else if dx < 0. { -1 } else { 0 };
        if step == 0 {
            continue;
        }
        if dir == 0 {
            dir = step;
        } else if step != dir {
            runs.push((start, i - 1));
            start = i - 1;
            dir = step;
        }
    }
    runs.push((start, points.len() - 1));
    runs
}

/// A fill crates/visual made for a square-cornered `g:rect`: x, y, w, h.
pub fn rect_of(points: &[[f32; 2]]) -> Option<(f32, f32, f32, f32)> {
    let [a, b, c, d] = points else { return None };
    (a[1] == b[1] && b[0] == c[0] && c[1] == d[1] && d[0] == a[0] && a[0] < b[0] && b[1] < c[1])
        .then(|| (a[0], a[1], b[0] - a[0], c[1] - b[1]))
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_square_rect_and_its_corners() {
        let r = [[1., 2.], [5., 2.], [5., 9.], [1., 9.]];
        assert_eq!(super::rect_of(&r), Some((1., 2., 4., 7.)));
        assert_eq!(super::rect_of(&r[..3]), None);
    }

    #[test]
    fn runs_split_where_x_turns_back() {
        let pts = [[0., 0.], [1., 1.], [2., 0.], [1., -1.], [0., 0.], [0., 3.], [1., 4.]];
        assert_eq!(super::monotone_runs(&pts), [(0, 2), (2, 5), (5, 6)], "a vertical step stays in its run");
        assert_eq!(super::monotone_runs(&pts[..3]), [(0, 2)]);
    }

    #[test]
    fn swaps_red_and_blue() {
        let mut px = vec![1, 2, 3, 4, 5, 6, 7, 8];
        super::bgra_to_rgba(&mut px);
        assert_eq!(px, [3, 2, 1, 4, 7, 6, 5, 8]);
    }
}
