//! Charts as vector paint in the chart's own coordinates: the desktop's
//! domains, ticks, label thinning and marks (`crates/ui/src/visuals.rs`).
//! The legend band above the plot doubles as the readout while a finger
//! scrubs the plot (the desktop's hover).

use zeron_visual::{Align, Chart, ChartKind, TextSize, Weight};

use super::Rgba;
use super::paint::{Face, Paint, TextCtx};
use super::palette::Palette;

/// Gaps between the plot and its axis labels.
const AXIS_GAP: f32 = 8.0;
/// Between the legend band and the plot.
pub(crate) const LEGEND_GAP: f32 = 8.0;
const LEGEND_GAP_X: f32 = 14.0;
const LEGEND_GAP_Y: f32 = 2.0;
const SWATCH: f32 = 8.0;
/// A line or area series' swatch is a short stroke, like its mark.
const SWATCH_LINE: f32 = 12.0;
/// Bars are at most this wide, so a few categories don't paint slabs.
const BAR_MAX: f32 = 44.0;
/// Bars round only at their value end; the baseline stays square.
const BAR_RADIUS: f32 = 3.0;
const LINE_WIDTH: f32 = 2.0;
/// An area's fill at the top of the series, fading to clear at its baseline.
const AREA_ALPHA: f32 = 0.3;
/// Steps in the area's fade (mobile paint has no gradients).
const AREA_STEPS: usize = 7;
/// Gridlines sit back; the zero baseline is the one firm rule.
const GRID_ALPHA: f32 = 0.6;
/// Bars outside the hovered category recede to this.
const DIM: f32 = 0.4;
/// Line and area series ending in a labelled value, at most.
const END_LABELS_MAX: usize = 4;
const END_GAP: f32 = 8.0;

/// What a finger over the plot picks out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Hover {
    pub category: Option<usize>,
    pub point: Option<(usize, usize)>,
}

fn domain(lo: f64, hi: f64, include_zero: bool) -> (f64, f64, Vec<f64>) {
    let (mut lo, mut hi) = (lo, hi);
    if include_zero {
        lo = lo.min(0.0);
        hi = hi.max(0.0);
    }
    if !lo.is_finite() || !hi.is_finite() {
        return (0.0, 1.0, vec![0.0, 1.0]);
    }
    if (hi - lo).abs() < f64::EPSILON {
        hi = lo + lo.abs().max(1.0);
    }
    let ticks = zeron_visual::nice_ticks(lo, hi, 4);
    let step = if ticks.len() > 1 {
        ticks[1] - ticks[0]
    } else {
        (hi - lo).max(1.0)
    };
    let lo = (lo / step).floor() * step;
    let hi = (hi / step).ceil() * step;
    (lo, hi, zeron_visual::nice_ticks(lo, hi, 4))
}

fn value_range(chart: &Chart) -> (f64, f64) {
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    if chart.kind == ChartKind::Scatter {
        for (_, y) in chart.series.iter().flat_map(|s| &s.points) {
            lo = lo.min(*y);
            hi = hi.max(*y);
        }
    } else if chart.stacked {
        let n = chart
            .series
            .iter()
            .map(|s| s.values.len())
            .max()
            .unwrap_or(0);
        for i in 0..n {
            let (mut pos, mut neg) = (0.0, 0.0);
            for s in &chart.series {
                match s.values.get(i).copied().flatten() {
                    Some(v) if v >= 0.0 => pos += v,
                    Some(v) => neg += v,
                    None => {}
                }
            }
            lo = lo.min(neg);
            hi = hi.max(pos);
        }
    } else {
        for v in chart.series.iter().flat_map(|s| s.values.iter().flatten()) {
            lo = lo.min(*v);
            hi = hi.max(*v);
        }
    }
    (chart.y_min.unwrap_or(lo), chart.y_max.unwrap_or(hi))
}

fn categories(chart: &Chart) -> usize {
    chart
        .series
        .iter()
        .map(|s| s.values.len())
        .max()
        .unwrap_or(0)
        .max(chart.x.len())
}

fn x_extent(chart: &Chart) -> (f64, f64) {
    chart
        .series
        .iter()
        .flat_map(|s| &s.points)
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), (x, _)| {
            (lo.min(*x), hi.max(*x))
        })
}

fn xs() -> Face {
    Face::new(TextSize::Xs, Weight::Regular)
}

fn sm(weight: Weight) -> Face {
    Face::new(TextSize::Sm, weight)
}

fn measure(ctx: &mut TextCtx, text: &str, face: Face) -> f32 {
    ctx.line(text, face, Rgba::default()).natural()
}

fn show_legend(chart: &Chart) -> bool {
    chart.legend && chart.series.len() > 1 && chart.series.iter().any(|s| !s.name.is_empty())
}

fn lines(chart: &Chart) -> bool {
    matches!(chart.kind, ChartKind::Line | ChartKind::Area)
}

fn swatch_width(chart: &Chart) -> f32 {
    if lines(chart) { SWATCH_LINE } else { SWATCH }
}

/// The value each line or area series ends on, labelled past the plot's
/// right edge (none for dense or crowded charts).
fn end_values(chart: &Chart) -> Vec<(usize, f64)> {
    if !lines(chart) || chart.series.len() > END_LABELS_MAX || categories(chart) < 2 {
        return Vec::new();
    }
    chart
        .series
        .iter()
        .enumerate()
        .filter_map(|(s, series)| Some((s, series.values.last().copied().flatten()?)))
        .collect()
}

/// A monotone cubic through `points` (Fritsch-Carlson), sampled as a
/// polyline: smooth, and never overshooting the data between points.
pub(crate) fn monotone(points: &[[f32; 2]]) -> Vec<[f32; 2]> {
    let n = points.len();
    if !(3..=120).contains(&n) {
        return points.to_vec();
    }
    let d: Vec<f32> = points
        .windows(2)
        .map(|w| (w[1][1] - w[0][1]) / (w[1][0] - w[0][0]).max(1e-3))
        .collect();
    let mut m = vec![0.0f32; n];
    m[0] = d[0];
    m[n - 1] = d[n - 2];
    for k in 1..n - 1 {
        m[k] = if d[k - 1] * d[k] <= 0.0 {
            0.0
        } else {
            (d[k - 1] + d[k]) / 2.0
        };
    }
    for k in 0..n - 1 {
        if d[k] == 0.0 {
            m[k] = 0.0;
            m[k + 1] = 0.0;
            continue;
        }
        let (a, b) = (m[k] / d[k], m[k + 1] / d[k]);
        let s = a * a + b * b;
        if s > 9.0 {
            let t = 3.0 / s.sqrt();
            m[k] = t * a * d[k];
            m[k + 1] = t * b * d[k];
        }
    }
    let mut out = vec![points[0]];
    for k in 0..n - 1 {
        let ([x0, y0], [x1, y1]) = (points[k], points[k + 1]);
        let h = x1 - x0;
        let steps = (h / 4.0).clamp(2.0, 12.0) as usize;
        for i in 1..=steps {
            let t = i as f32 / steps as f32;
            let (t2, t3) = (t * t, t * t * t);
            let y = (2.0 * t3 - 3.0 * t2 + 1.0) * y0
                + (t3 - 2.0 * t2 + t) * h * m[k]
                + (-2.0 * t3 + 3.0 * t2) * y1
                + (t3 - t2) * h * m[k + 1];
            out.push([x0 + h * t, y]);
        }
    }
    out
}

/// `polygon` clipped to y <= `limit` (Sutherland-Hodgman; fine for the
/// concave outline of an area).
fn clip_above(polygon: &[[f32; 2]], limit: f32) -> Vec<[f32; 2]> {
    let mut out = Vec::with_capacity(polygon.len() + 4);
    for (i, &a) in polygon.iter().enumerate() {
        let b = polygon[(i + 1) % polygon.len()];
        let (a_in, b_in) = (a[1] <= limit, b[1] <= limit);
        if a_in {
            out.push(a);
        }
        if a_in != b_in {
            let t = (limit - a[1]) / (b[1] - a[1]);
            out.push([a[0] + (b[0] - a[0]) * t, limit]);
        }
    }
    out
}

/// An area's fill fading from the top of its outline to clear at the
/// bottom, as nested translucent layers: each covers the outline above a
/// lower line, so they build up toward the top with no seams between them.
fn fade_fill(out: &mut Paint, outline: &[[f32; 2]], color: Rgba) {
    let (top, bottom) = outline
        .iter()
        .fold((f32::MAX, f32::MIN), |(t, b), p| (t.min(p[1]), b.max(p[1])));
    if bottom - top < 1.0 {
        return;
    }
    // Each layer's alpha, so all of them together make AREA_ALPHA.
    let layer = 1.0 - (1.0 - AREA_ALPHA).powf(1.0 / AREA_STEPS as f32);
    for k in 1..=AREA_STEPS {
        let limit = top + (bottom - top) * k as f32 / (AREA_STEPS + 1) as f32;
        out.fill(clip_above(outline, limit), color.alpha(layer));
    }
}

/// Which end of a bar holds its value (and takes the rounding).
#[derive(Clone, Copy)]
enum End {
    Top,
    Bottom,
    Left,
    Right,
}

/// A bar's outline, rounded at its value end only.
fn bar_outline([x, y, w, h]: [f32; 4], end: End, radius: f32) -> Vec<[f32; 2]> {
    let r = radius.min(w / 2.0).min(h / 2.0).max(0.0);
    // Corners clockwise from the top left; rounded ones get a quarter arc.
    let corners = [
        (
            [x, y],
            matches!(end, End::Top | End::Left),
            [1.0, 1.0],
            180.0,
        ),
        (
            [x + w, y],
            matches!(end, End::Top | End::Right),
            [-1.0, 1.0],
            270.0,
        ),
        (
            [x + w, y + h],
            matches!(end, End::Bottom | End::Right),
            [-1.0, -1.0],
            0.0,
        ),
        (
            [x, y + h],
            matches!(end, End::Bottom | End::Left),
            [1.0, -1.0],
            90.0,
        ),
    ];
    let mut out = Vec::with_capacity(20);
    for ([cx, cy], round, [sx, sy], start) in corners {
        if !round || r < 0.5 {
            out.push([cx, cy]);
            continue;
        }
        let center = [cx + sx * r, cy + sy * r];
        for i in 0..=4 {
            let a = (start + 90.0 * i as f32 / 4.0f32).to_radians();
            out.push([center[0] + r * a.cos(), center[1] + r * a.sin()]);
        }
    }
    out
}

/// Bar layout within one category band: (inset of the first bar, bar width).
fn bar_metrics(band: f32, groups: usize) -> (f32, f32) {
    let groups = groups.max(1) as f32;
    let bar = (band * 0.72 / groups).clamp(1.0, BAR_MAX);
    let inner = bar * groups;
    ((band - inner) / 2.0, bar)
}

/// Legend entries flowed into rows at `width`: (x, row) per entry.
fn flow(widths: &[f32], width: f32) -> Vec<(f32, usize)> {
    let (mut x, mut row) = (0.0f32, 0usize);
    widths
        .iter()
        .map(|w| {
            if x > 0.0 && x + w > width + 0.5 {
                x = 0.0;
                row += 1;
            }
            let at = (x, row);
            x += w + LEGEND_GAP_X;
            at
        })
        .collect()
}

/// The legend band's height at `width` (one line when there's no legend,
/// kept for the readout).
pub(crate) fn legend_height(
    ctx: &mut TextCtx,
    _palette: &Palette,
    chart: &Chart,
    width: f32,
) -> f32 {
    let line = ctx.line_height(TextSize::Sm);
    if !show_legend(chart) {
        return line;
    }
    let widths: Vec<f32> = chart
        .series
        .iter()
        .map(|s| swatch_width(chart) + 6.0 + measure(ctx, &s.name, sm(Weight::Regular)))
        .collect();
    let rows = flow(&widths, width).last().map_or(0, |(_, row)| *row) + 1;
    rows as f32 * line + (rows - 1) as f32 * LEGEND_GAP_Y
}

struct Plan {
    /// x, y, w, h in chart coordinates.
    plot: [f32; 4],
    lo: f64,
    hi: f64,
    ticks: Vec<f64>,
    x_lo: f64,
    x_hi: f64,
    x_ticks: Vec<f64>,
    label_stride: usize,
}

fn plan(ctx: &mut TextCtx, chart: &Chart, width: f32, legend: f32) -> Plan {
    let tick_line = ctx.line_height(TextSize::Xs);
    let scatter = chart.kind == ChartKind::Scatter;
    let horizontal = chart.kind == ChartKind::Bar && chart.horizontal;
    let (lo, hi) = value_range(chart);
    let (lo, hi, ticks) = domain(lo, hi, !scatter);
    let (x_lo, x_hi, x_ticks) = if scatter {
        let (a, b) = x_extent(chart);
        domain(a, b, false)
    } else {
        (0.0, 1.0, Vec::new())
    };
    let n = categories(chart).max(1);
    // The left gutter fits the widest value tick, or the category labels on
    // a horizontal bar chart.
    let left = if horizontal {
        chart
            .x
            .iter()
            .map(|l| measure(ctx, l, xs()))
            .fold(0.0, f32::max)
            .min(width * 0.35)
    } else {
        ticks
            .iter()
            .map(|t| measure(ctx, &chart.format.format(*t), xs()))
            .fold(0.0, f32::max)
    } + AXIS_GAP;
    let top = legend + LEGEND_GAP + tick_line / 2.0;
    let bottom = tick_line + AXIS_GAP / 2.0;
    // Room past the plot for the values line series end on.
    let ends = end_values(chart)
        .iter()
        .map(|(_, v)| {
            measure(
                ctx,
                &chart.format.format(*v),
                Face::new(TextSize::Xs, Weight::Medium),
            )
        })
        .fold(0.0, f32::max);
    let right = if ends > 0.0 {
        (ends + END_GAP).min(width * 0.3)
    } else if horizontal || scatter {
        12.0
    } else {
        4.0
    };
    let plot = [
        left,
        top,
        (width - left - right).max(0.0),
        (legend + LEGEND_GAP + chart.height - bottom - top).max(0.0),
    ];
    // Thin the category labels until they don't collide.
    let label_stride = if horizontal || scatter {
        1
    } else {
        let widest = chart
            .x
            .iter()
            .map(|l| measure(ctx, l, xs()))
            .fold(0.0, f32::max)
            + 12.0;
        let band = plot[2] / n as f32;
        ((widest / band.max(1.0)).ceil() as usize).max(1)
    };
    Plan {
        plot,
        lo,
        hi,
        ticks,
        x_lo,
        x_hi,
        x_ticks,
        label_stride,
    }
}

/// What's under (x, y) in chart coordinates.
pub(crate) fn hover_at(
    ctx: &mut TextCtx,
    chart: &Chart,
    width: f32,
    legend: f32,
    [x, y]: [f32; 2],
) -> Hover {
    let plan = plan(ctx, chart, width, legend);
    let [px, py, pw, ph] = plan.plot;
    if pw <= 0.0 || ph <= 0.0 {
        return Hover::default();
    }
    let n = categories(chart);
    if chart.kind == ChartKind::Scatter {
        let mut best: Option<(f32, usize, usize)> = None;
        for (s, series) in chart.series.iter().enumerate() {
            for (p, (vx, vy)) in series.points.iter().enumerate() {
                let sx = px + pw * ((vx - plan.x_lo) / (plan.x_hi - plan.x_lo)) as f32;
                let sy = py + ph * (1.0 - ((vy - plan.lo) / (plan.hi - plan.lo)) as f32);
                // A fingertip, not a cursor: a wider catch than the desktop's.
                let d = (sx - x).hypot(sy - y);
                if d < 24.0 && best.is_none_or(|(bd, _, _)| d < bd) {
                    best = Some((d, s, p));
                }
            }
        }
        return Hover {
            category: None,
            point: best.map(|(_, s, p)| (s, p)),
        };
    }
    let along = if chart.horizontal && chart.kind == ChartKind::Bar {
        (y - py) / ph
    } else {
        (x - px) / pw
    };
    let ix = if chart.kind == ChartKind::Bar || n <= 1 {
        (along * n as f32).floor()
    } else {
        (along * (n - 1) as f32).round()
    };
    Hover {
        category: (0.0..n as f32).contains(&ix).then_some(ix as usize),
        point: None,
    }
}

/// The whole chart (legend band and plot) at `width`.
pub(crate) fn paint(
    ctx: &mut TextCtx,
    palette: &Palette,
    chart: &Chart,
    width: f32,
    legend: f32,
    hover: Hover,
) -> Paint {
    let mut out = Paint::default();
    paint_legend(&mut out, ctx, palette, chart, width, legend, hover);
    let plan = plan(ctx, chart, width, legend);
    paint_plot(&mut out, ctx, palette, chart, &plan, hover);
    out
}

/// One entry in the legend band: a series (swatch, name, value while
/// scrubbing) or a readout line.
struct LegendItem {
    swatch: Option<Rgba>,
    name: String,
    value: Option<String>,
    weight: Weight,
    color: Rgba,
}

impl LegendItem {
    fn text(name: String, weight: Weight, color: Rgba) -> Self {
        Self {
            swatch: None,
            name,
            value: None,
            weight,
            color,
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_legend(
    out: &mut Paint,
    ctx: &mut TextCtx,
    palette: &Palette,
    chart: &Chart,
    width: f32,
    band: f32,
    hover: Hover,
) {
    let line = ctx.line_height(TextSize::Sm);
    // Readout first (the hovered category, or the hovered point's values),
    // then series entries with their values while scrubbing.
    let mut items: Vec<LegendItem> = Vec::new();
    if let Some(label) = hover.category.and_then(|i| chart.x.get(i)) {
        items.push(LegendItem::text(
            label.clone(),
            Weight::Medium,
            palette.text,
        ));
    }
    if let Some((s, p)) = hover.point {
        let (x, y) = chart.series[s].points[p];
        let text = format!("{} · {}", chart.x_format.format(x), chart.format.format(y));
        items.push(LegendItem::text(text, Weight::Regular, palette.text));
    }
    if show_legend(chart) || hover.category.is_some() {
        for series in &chart.series {
            items.push(LegendItem {
                swatch: Some(palette.color(series.color)),
                name: series.name.clone(),
                value: hover
                    .category
                    .and_then(|i| series.values.get(i).copied().flatten())
                    .map(|v| chart.format.format(v)),
                weight: Weight::Regular,
                color: palette.muted,
            });
        }
    }
    let widths: Vec<f32> = items
        .iter()
        .map(|item| {
            item.swatch.map_or(0.0, |_| swatch_width(chart) + 6.0)
                + measure(ctx, &item.name, sm(item.weight))
                + item
                    .value
                    .as_ref()
                    .map_or(0.0, |v| 6.0 + measure(ctx, v, sm(Weight::Regular)))
        })
        .collect();
    let rows = ((band + LEGEND_GAP_Y) / (line + LEGEND_GAP_Y))
        .floor()
        .max(1.0) as usize;
    for ((x, row), item) in flow(&widths, width).into_iter().zip(&items) {
        // What doesn't fit the band is dropped (the band's height is fixed).
        if row >= rows {
            break;
        }
        let top = row as f32 * (line + LEGEND_GAP_Y);
        let mut at = x;
        if let Some(swatch) = item.swatch {
            match chart.kind {
                ChartKind::Line | ChartKind::Area => {
                    out.rect(at, top + line / 2.0 - 1.5, SWATCH_LINE, 3.0, 1.5, swatch)
                }
                ChartKind::Scatter => {
                    out.dot([at + SWATCH / 2.0, top + line / 2.0], SWATCH / 2.0, swatch)
                }
                ChartKind::Bar => {
                    out.rect(at, top + (line - SWATCH) / 2.0, SWATCH, SWATCH, 2.0, swatch)
                }
            }
            at += swatch_width(chart) + 6.0;
        }
        at += out.label(
            ctx,
            &item.name,
            sm(item.weight),
            item.color,
            [at, top],
            Align::Start,
            Align::Start,
        );
        if let Some(value) = &item.value {
            out.label(
                ctx,
                value,
                sm(Weight::Regular),
                palette.text,
                [at + 6.0, top],
                Align::Start,
                Align::Start,
            );
        }
    }
}

fn paint_plot(
    out: &mut Paint,
    ctx: &mut TextCtx,
    palette: &Palette,
    chart: &Chart,
    plan: &Plan,
    hover: Hover,
) {
    let [ox, oy, pw, ph] = plan.plot;
    if pw <= 4.0 || ph <= 4.0 {
        return;
    }
    let horizontal = chart.kind == ChartKind::Bar && chart.horizontal;
    let n = categories(chart).max(1);
    let value_t = |v: f64| ((v - plan.lo) / (plan.hi - plan.lo)) as f32;
    let vy = |v: f64| oy + ph * (1.0 - value_t(v));
    let vx = |v: f64| ox + pw * value_t(v);
    let (grid, muted) = (palette.border.alpha(GRID_ALPHA), palette.muted);
    let zero = 0.0f64.clamp(plan.lo, plan.hi);

    // Gridlines and value ticks; the zero line is drawn over the marks.
    for tick in &plan.ticks {
        let label = chart.format.format(*tick);
        let rule = if *tick == zero && chart.kind != ChartKind::Scatter {
            Rgba::default()
        } else {
            grid
        };
        if horizontal {
            let x = vx(*tick);
            out.stroke([[x, oy], [x, oy + ph]], false, 1.0, None, rule);
            out.label(
                ctx,
                &label,
                xs(),
                muted,
                [x, oy + ph + AXIS_GAP / 2.0],
                Align::Center,
                Align::Start,
            );
        } else {
            let y = vy(*tick);
            out.stroke([[ox, y], [ox + pw, y]], false, 1.0, None, rule);
            out.label(
                ctx,
                &label,
                xs(),
                muted,
                [ox - AXIS_GAP, y],
                Align::End,
                Align::Center,
            );
        }
    }

    match chart.kind {
        ChartKind::Bar => {
            let band = if horizontal { ph } else { pw } / n as f32;
            let groups = if chart.stacked {
                1
            } else {
                chart.series.len().max(1)
            };
            let (inset, bar) = bar_metrics(band, groups);
            let gap = if groups > 1 {
                2.0f32.min(bar / 4.0)
            } else {
                0.0
            };
            for i in 0..n {
                let start = band * i as f32 + inset;
                let dim = hover.category.is_some_and(|h| h != i);
                // The outermost segment of each stack takes the rounding.
                let (top_pos, top_neg) = if chart.stacked {
                    let last = |positive: bool| {
                        chart.series.iter().rposition(|s| {
                            s.values
                                .get(i)
                                .copied()
                                .flatten()
                                .is_some_and(|v| (v >= 0.0) == positive && v != 0.0)
                        })
                    };
                    (last(true), last(false))
                } else {
                    (None, None)
                };
                let (mut pos, mut neg) = (0.0f64, 0.0f64);
                for (s, series) in chart.series.iter().enumerate() {
                    let Some(v) = series.values.get(i).copied().flatten() else {
                        continue;
                    };
                    let (from, to) = if chart.stacked {
                        if v >= 0.0 {
                            pos += v;
                            (pos - v, pos)
                        } else {
                            neg += v;
                            (neg - v, neg)
                        }
                    } else {
                        (zero, v)
                    };
                    let offset = if chart.stacked { 0.0 } else { bar * s as f32 };
                    let rounded = !chart.stacked || Some(s) == top_pos || Some(s) == top_neg;
                    let radius = if rounded { BAR_RADIUS } else { 0.0 };
                    // Stacked segments part with a sliver at their value end.
                    let seam = if chart.stacked && !rounded { 1.0 } else { 0.0 };
                    let mut color = palette.color(series.color);
                    if dim {
                        color = color.alpha(DIM);
                    }
                    let up = to >= from;
                    let rect = if horizontal {
                        let y = oy + start + offset;
                        let (a, b) = (vx(from), vx(to));
                        let w = ((a - b).abs() - seam).max(0.0);
                        let x = if up { a } else { a - w };
                        let end = if up { End::Right } else { End::Left };
                        ([x, y, w, bar - gap], end)
                    } else {
                        let x = ox + start + offset;
                        let (a, b) = (vy(from), vy(to));
                        let h = ((a - b).abs() - seam).max(0.0);
                        let y = if up { a - h } else { a };
                        let end = if up { End::Top } else { End::Bottom };
                        ([x, y, bar - gap, h], end)
                    };
                    let (frame, end) = rect;
                    if frame[2] <= 0.0 || frame[3] <= 0.0 {
                        continue;
                    }
                    if radius > 0.0 {
                        out.fill(bar_outline(frame, end, radius), color);
                    } else {
                        out.rect(frame[0], frame[1], frame[2], frame[3], 0.0, color);
                    }
                }
            }
            baseline(
                out,
                palette,
                horizontal,
                [ox, oy, pw, ph],
                vx(zero),
                vy(zero),
            );
            for (i, label) in chart.x.iter().enumerate().take(n) {
                if i % plan.label_stride != 0 {
                    continue;
                }
                let center = band * (i as f32 + 0.5);
                let color = if hover.category == Some(i) {
                    palette.text
                } else {
                    muted
                };
                if horizontal {
                    out.label(
                        ctx,
                        label,
                        xs(),
                        color,
                        [ox - AXIS_GAP, oy + center],
                        Align::End,
                        Align::Center,
                    );
                } else {
                    out.label(
                        ctx,
                        label,
                        xs(),
                        color,
                        [ox + center, oy + ph + AXIS_GAP / 2.0],
                        Align::Center,
                        Align::Start,
                    );
                }
            }
        }
        ChartKind::Line | ChartKind::Area => {
            let x_at = |i: usize| {
                if n <= 1 {
                    ox + pw / 2.0
                } else {
                    ox + pw * i as f32 / (n - 1) as f32
                }
            };
            if let Some(i) = hover.category {
                let x = x_at(i);
                out.stroke(
                    [[x, oy], [x, oy + ph]],
                    false,
                    1.0,
                    None,
                    palette.border_strong,
                );
            }
            let base = vy(zero);
            // Fills first, so no series' fill covers another's line.
            let runs: Vec<Vec<Vec<[f32; 2]>>> = chart
                .series
                .iter()
                .map(|series| {
                    // Gaps split a series into runs.
                    let mut runs: Vec<Vec<[f32; 2]>> = vec![Vec::new()];
                    for (i, v) in series.values.iter().enumerate() {
                        match v {
                            Some(v) => runs.last_mut().expect("a run").push([x_at(i), vy(*v)]),
                            None => runs.push(Vec::new()),
                        }
                    }
                    runs.retain(|r| !r.is_empty());
                    runs
                })
                .collect();
            if chart.kind == ChartKind::Area {
                for (series, runs) in chart.series.iter().zip(&runs) {
                    let color = palette.color(series.color);
                    for run in runs.iter().filter(|r| r.len() > 1) {
                        let mut outline = monotone(run);
                        outline.push([run[run.len() - 1][0], base]);
                        outline.push([run[0][0], base]);
                        fade_fill(out, &outline, color);
                    }
                }
            }
            if chart.kind == ChartKind::Area || plan.lo < 0.0 {
                baseline(out, palette, false, [ox, oy, pw, ph], 0.0, base);
            }
            for (series, runs) in chart.series.iter().zip(&runs) {
                let color = palette.color(series.color);
                for run in runs {
                    if run.len() == 1 {
                        out.dot(run[0], 2.5, color);
                    } else {
                        out.stroke(monotone(run), false, LINE_WIDTH, None, color);
                    }
                }
            }
            // Each series ends on a dot and its value; the hovered category
            // gets a ringed dot per series.
            let ends = end_values(chart);
            let line = ctx.line_height(TextSize::Xs);
            let mut labels: Vec<(f32, usize, f64)> =
                ends.iter().map(|&(s, v)| (vy(v), s, v)).collect();
            labels.sort_by(|a, b| a.0.total_cmp(&b.0));
            // Nudge apart labels that would overlap, then back inside the plot.
            for k in 1..labels.len() {
                labels[k].0 = labels[k].0.max(labels[k - 1].0 + line);
            }
            if let Some(last) = labels.last() {
                let over = (last.0 - (oy + ph)).max(0.0);
                for label in &mut labels {
                    label.0 -= over;
                }
            }
            for (y, s, v) in labels {
                let color = palette.color(chart.series[s].color);
                if hover.category.is_none() {
                    out.dot([ox + pw, vy(v)], 3.0, color);
                }
                out.label(
                    ctx,
                    &chart.format.format(v),
                    Face::new(TextSize::Xs, Weight::Medium),
                    color,
                    [ox + pw + END_GAP, y],
                    Align::Start,
                    Align::Center,
                );
            }
            if let Some(i) = hover.category {
                for series in &chart.series {
                    if let Some(Some(v)) = series.values.get(i) {
                        let color = palette.color(series.color);
                        out.dot([x_at(i), vy(*v)], 7.0, color.alpha(0.2));
                        out.dot([x_at(i), vy(*v)], 3.5, color);
                    }
                }
            }
            for (i, label) in chart.x.iter().enumerate().take(n) {
                if i % plan.label_stride != 0 {
                    continue;
                }
                let align = if n > 1 && i == 0 {
                    Align::Start
                } else if n > 1 && i == n - 1 {
                    Align::End
                } else {
                    Align::Center
                };
                let color = if hover.category == Some(i) {
                    palette.text
                } else {
                    muted
                };
                out.label(
                    ctx,
                    label,
                    xs(),
                    color,
                    [x_at(i), oy + ph + AXIS_GAP / 2.0],
                    align,
                    Align::Start,
                );
            }
        }
        ChartKind::Scatter => {
            let sx = |x: f64| ox + pw * ((x - plan.x_lo) / (plan.x_hi - plan.x_lo)) as f32;
            for tick in &plan.x_ticks {
                let x = sx(*tick);
                out.stroke([[x, oy], [x, oy + ph]], false, 1.0, None, grid);
                out.label(
                    ctx,
                    &chart.x_format.format(*tick),
                    xs(),
                    muted,
                    [x, oy + ph + AXIS_GAP / 2.0],
                    Align::Center,
                    Align::Start,
                );
            }
            for (s, series) in chart.series.iter().enumerate() {
                let color = palette.color(series.color);
                for (ix, (x, y)) in series.points.iter().enumerate() {
                    let at = [sx(*x), vy(*y)];
                    if hover.point == Some((s, ix)) {
                        out.dot(at, 9.0, color.alpha(0.2));
                        out.dot(at, 5.0, color);
                    } else {
                        let faded = hover.point.is_some_and(|(hs, _)| hs != s);
                        out.dot(at, 3.5, color.alpha(if faded { DIM } else { 0.9 }));
                    }
                }
            }
        }
    }
}

/// The zero line: the one firm rule, over the marks it anchors.
fn baseline(
    out: &mut Paint,
    palette: &Palette,
    horizontal: bool,
    [ox, oy, pw, ph]: [f32; 4],
    x: f32,
    y: f32,
) {
    if horizontal {
        out.stroke(
            [[x, oy], [x, oy + ph]],
            false,
            1.0,
            None,
            palette.border_strong,
        );
    } else {
        out.stroke(
            [[ox, y], [ox + pw, y]],
            false,
            1.0,
            None,
            palette.border_strong,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domains_include_zero_for_bars_and_land_on_ticks() {
        let (lo, hi, ticks) = domain(12.0, 87.0, true);
        assert_eq!((lo, hi), (0.0, 100.0));
        assert_eq!(ticks.first(), Some(&0.0));
        assert_eq!(ticks.last(), Some(&100.0));
        let (lo, hi, _) = domain(-3.0, 7.0, false);
        assert!(lo <= -3.0 && hi >= 7.0);
    }

    #[test]
    fn legend_entries_flow_onto_new_rows() {
        let at = flow(&[40.0, 40.0, 40.0], 100.0);
        assert_eq!(at, vec![(0.0, 0), (54.0, 0), (0.0, 1)]);
    }
}
