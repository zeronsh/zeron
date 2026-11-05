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
        .map(|s| SWATCH + 6.0 + measure(ctx, &s.name, sm(Weight::Regular)))
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
    let right = if horizontal || scatter { 12.0 } else { 4.0 };
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
            item.swatch.map_or(0.0, |_| SWATCH + 6.0)
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
            out.rect(at, top + (line - SWATCH) / 2.0, SWATCH, SWATCH, 2.0, swatch);
            at += SWATCH + 6.0;
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
    let (grid, muted) = (palette.border, palette.muted);

    // Gridlines and value ticks.
    for tick in &plan.ticks {
        let label = chart.format.format(*tick);
        if horizontal {
            let x = vx(*tick);
            out.stroke([[x, oy], [x, oy + ph]], false, 1.0, None, grid);
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
            out.stroke([[ox, y], [ox + pw, y]], false, 1.0, None, grid);
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
            if let Some(i) = hover.category {
                let start = if horizontal { oy } else { ox } + band * i as f32;
                if horizontal {
                    out.rect(ox, start, pw, band, 4.0, palette.wash);
                } else {
                    out.rect(start, oy, band, ph, 4.0, palette.wash);
                }
            }
            let groups = if chart.stacked {
                1
            } else {
                chart.series.len().max(1)
            };
            let inner = band * 0.72;
            let bar = (inner / groups as f32).max(1.0);
            for i in 0..n {
                let start = band * i as f32 + (band - inner) / 2.0;
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
                        (0.0f64.clamp(plan.lo, plan.hi), v)
                    };
                    let offset = if chart.stacked { 0.0 } else { bar * s as f32 };
                    let gap = if groups > 1 && !chart.stacked {
                        1.0
                    } else {
                        0.0
                    };
                    let radius = if chart.stacked { 0.0 } else { 2.0 };
                    let color = palette.color(series.color);
                    if horizontal {
                        let y = oy + start + offset;
                        let (a, b) = (vx(from), vx(to));
                        out.rect(a.min(b), y, (a - b).abs(), bar - gap, radius, color);
                    } else {
                        let x = ox + start + offset;
                        let (a, b) = (vy(from), vy(to));
                        out.rect(x, a.min(b), bar - gap, (a - b).abs(), radius, color);
                    }
                }
            }
            for (i, label) in chart.x.iter().enumerate().take(n) {
                if i % plan.label_stride != 0 {
                    continue;
                }
                let center = band * (i as f32 + 0.5);
                if horizontal {
                    out.label(
                        ctx,
                        label,
                        xs(),
                        muted,
                        [ox - AXIS_GAP, oy + center],
                        Align::End,
                        Align::Center,
                    );
                } else {
                    out.label(
                        ctx,
                        label,
                        xs(),
                        muted,
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
            let base = vy(0.0f64.clamp(plan.lo, plan.hi));
            for series in &chart.series {
                let color = palette.color(series.color);
                // Gaps split a series into runs.
                let mut runs: Vec<Vec<[f32; 2]>> = vec![Vec::new()];
                for (i, v) in series.values.iter().enumerate() {
                    match v {
                        Some(v) => runs.last_mut().expect("a run").push([x_at(i), vy(*v)]),
                        None => runs.push(Vec::new()),
                    }
                }
                for run in runs.iter().filter(|r| !r.is_empty()) {
                    if chart.kind == ChartKind::Area && run.len() > 1 {
                        let mut area = run.clone();
                        area.push([run[run.len() - 1][0], base]);
                        area.push([run[0][0], base]);
                        out.fill(area, color.alpha(0.22));
                    }
                    if run.len() == 1 {
                        out.dot(run[0], 2.5, color);
                    } else {
                        out.stroke(run.iter().copied(), false, 1.75, None, color);
                    }
                }
                if let Some(i) = hover.category
                    && let Some(Some(v)) = series.values.get(i)
                {
                    out.dot([x_at(i), vy(*v)], 3.5, color);
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
                out.label(
                    ctx,
                    label,
                    xs(),
                    muted,
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
                    let r = if hover.point == Some((s, ix)) {
                        5.0
                    } else {
                        3.0
                    };
                    out.dot([sx(*x), vy(*y)], r, color);
                }
            }
        }
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
