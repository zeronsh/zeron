//! Lays a visual's node tree out at a width: the desktop's stack, grid and
//! table rules (`crates/ui/src/visuals.rs`) as arithmetic, so the platform
//! only paints. Controls are drawn here too (one look on every platform);
//! the platform puts touch targets over them.

use zeron_visual::{Align, Axis, HandlerId, Justify, Node, NodeKind, TextSize, Weight};

use super::chart;
use super::paint::{Face, Mark, Paint, Para, TextCtx};
use super::palette::Palette;
use super::{ControlKind, SurfaceGesture, VisualControl};

/// Control metrics, in points.
pub(crate) mod metrics {
    pub const BUTTON_H: f32 = 32.0;
    pub const BUTTON_PAD: f32 = 14.0;
    pub const TAB_H: f32 = 30.0;
    pub const TAB_PAD: f32 = 12.0;
    pub const TAB_GAP: f32 = 2.0;
    pub const TOGGLE_W: f32 = 36.0;
    pub const TOGGLE_H: f32 = 22.0;
    pub const TOGGLE_GAP: f32 = 10.0;
    pub const SLIDER_TRACK: f32 = 4.0;
    pub const SLIDER_THUMB: f32 = 14.0;
    pub const TABLE_CELL_PAD: f32 = 12.0;
    pub const TABLE_CELL_PAD_Y: f32 = 8.0;
}
use metrics::*;

/// What a surface draws (every frame it changes).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SurfaceNode {
    Canvas(zeron_visual::Canvas),
    Scene(zeron_visual::SceneNode),
    Shader(zeron_visual::ShaderNode),
    Chart {
        chart: Box<zeron_visual::Chart>,
        /// Height of the legend band above the plot.
        legend: f32,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SurfaceSpec {
    /// Stable across renders (node key or tree path).
    pub id: u64,
    pub rect: [f32; 4],
    pub node: SurfaceNode,
}

impl SurfaceSpec {
    pub fn size(&self) -> (f32, f32) {
        (self.rect[2], self.rect[3])
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ActionArg {
    None,
    Bool(bool),
    Text(String),
    Slider { min: f64, max: f64, step: f64 },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Action {
    Call(HandlerId, ActionArg),
    /// Opened by the platform.
    Link,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ControlSpec {
    pub control: VisualControl,
    pub action: Action,
}

/// A tree laid out at one width.
#[derive(Default)]
pub(crate) struct Laid {
    pub height: f32,
    pub paint: Paint,
    pub surfaces: Vec<SurfaceSpec>,
    pub controls: Vec<ControlSpec>,
}

#[derive(Clone, Copy)]
struct LaidMark {
    paint: Mark,
    surfaces: usize,
    controls: usize,
}

impl Laid {
    fn mark(&self) -> LaidMark {
        LaidMark {
            paint: self.paint.mark(),
            surfaces: self.surfaces.len(),
            controls: self.controls.len(),
        }
    }

    /// Move everything added between two marks.
    fn shift(&mut self, from: LaidMark, to: LaidMark, dx: f32, dy: f32) {
        if dx == 0.0 && dy == 0.0 {
            return;
        }
        self.paint.shift_between(from.paint, to.paint, dx, dy);
        for s in &mut self.surfaces[from.surfaces..to.surfaces] {
            s.rect[0] += dx;
            s.rect[1] += dy;
        }
        for c in &mut self.controls[from.controls..to.controls] {
            c.control.x += dx;
            c.control.y += dy;
            if let ControlKind::Slider { track_x, .. } = &mut c.control.kind {
                *track_x += dx;
            }
        }
    }
}

/// Stable 64-bit id for a node path (FNV-1a).
pub(crate) fn stable_id(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

fn node_key(node: &Node, path: &str) -> String {
    match &node.key {
        Some(key) => format!("key:{key}"),
        None => path.to_owned(),
    }
}

/// Lay `tree` out at `width`.
pub(crate) fn lay_out(ctx: &mut TextCtx, palette: &Palette, tree: &Node, width: f32) -> Laid {
    let mut layouter = Layouter {
        ctx,
        palette,
        out: Laid::default(),
    };
    let height = layouter.place(tree, "0", 0.0, 0.0, width);
    let mut out = layouter.out;
    out.height = height;
    out
}

struct Layouter<'a, 'b> {
    ctx: &'a mut TextCtx<'b>,
    palette: &'a Palette,
    out: Laid,
}

#[derive(Clone, Copy, Debug)]
enum Basis {
    Fixed(f32),
    Flex,
    Shrink(f32),
}

impl Layouter<'_, '_> {
    fn text_para(&mut self, text: &zeron_visual::Text) -> Para {
        let base = text
            .color
            .map(|c| self.palette.color(c))
            .unwrap_or(self.palette.text);
        let parts: Vec<(&str, Face, super::Rgba)> = text
            .spans
            .iter()
            .map(|span| {
                let face = Face {
                    size: text.size,
                    weight: span.weight.unwrap_or(text.weight),
                    mono: text.mono || span.mono,
                    italic: text.italic || span.italic,
                };
                let color = span.color.map(|c| self.palette.color(c)).unwrap_or(base);
                (span.text.as_str(), face, color)
            })
            .collect();
        if parts.iter().all(|(s, _, _)| s.is_empty()) {
            // An empty text still holds its line.
            let face = Face::new(text.size, text.weight);
            return self.ctx.para(&[("", face, base)]);
        }
        self.ctx.para(&parts)
    }

    fn plain(&mut self, text: &str, size: TextSize, weight: Weight, color: super::Rgba) -> Para {
        self.ctx.line(text, Face::new(size, weight), color)
    }

    /// Width a node takes unstretched; `None` fills what it's given.
    fn natural(&mut self, node: &Node) -> Option<f32> {
        if let Some(width) = node.width {
            return Some(width);
        }
        match &node.kind {
            NodeKind::Stack(stack) => {
                let gaps = stack.gap * stack.children.len().saturating_sub(1) as f32;
                let mut total: f32 = 0.0;
                for child in &stack.children {
                    let w = self.natural(child)?;
                    total = match stack.axis {
                        Axis::Horizontal => total + w,
                        Axis::Vertical => total.max(w),
                    };
                }
                Some(match stack.axis {
                    Axis::Horizontal => total + gaps,
                    Axis::Vertical => total,
                })
            }
            NodeKind::Text(text) => Some(self.text_para(text).natural()),
            NodeKind::Space(size) => Some(*size),
            NodeKind::Stat {
                value,
                label,
                delta,
                ..
            } => {
                let value_w = self
                    .plain(value, TextSize::Xxl, Weight::Semibold, self.palette.text)
                    .natural();
                let delta_w = delta.as_deref().filter(|d| !d.is_empty()).map_or(0.0, |d| {
                    6.0 + self
                        .plain(d, TextSize::Sm, Weight::Regular, self.palette.text)
                        .natural()
                });
                let label_w = self
                    .plain(label, TextSize::Sm, Weight::Regular, self.palette.text)
                    .natural();
                Some((value_w + delta_w).max(label_w))
            }
            NodeKind::Button { label, .. } => Some(self.button_width(label)),
            NodeKind::Tabs { options, .. } => {
                let widths: Vec<f32> = options.iter().map(|o| self.tab_width(o)).collect();
                Some(widths.iter().sum::<f32>() + TAB_GAP * widths.len().saturating_sub(1) as f32)
            }
            NodeKind::Toggle { label, .. } => Some(
                TOGGLE_W
                    + TOGGLE_GAP
                    + self
                        .plain(label, TextSize::Body, Weight::Regular, self.palette.text)
                        .natural(),
            ),
            NodeKind::Link { label, .. } => Some(
                self.plain(label, TextSize::Body, Weight::Regular, self.palette.accent)
                    .natural(),
            ),
            NodeKind::Grid { .. }
            | NodeKind::Table(_)
            | NodeKind::Rule
            | NodeKind::Chart(_)
            | NodeKind::Canvas(_)
            | NodeKind::Scene(_)
            | NodeKind::Shader(_)
            | NodeKind::Slider { .. } => None,
        }
    }

    fn button_width(&mut self, label: &str) -> f32 {
        self.plain(label, TextSize::Body, Weight::Medium, self.palette.text)
            .natural()
            + BUTTON_PAD * 2.0
    }

    fn tab_width(&mut self, label: &str) -> f32 {
        self.plain(label, TextSize::Body, Weight::Medium, self.palette.text)
            .natural()
            + TAB_PAD * 2.0
    }

    /// Place `node` with its top-left at (x, y) in `width`; returns its height.
    fn place(&mut self, node: &Node, path: &str, x: f32, y: f32, width: f32) -> f32 {
        let key = node_key(node, path);
        match &node.kind {
            NodeKind::Stack(stack) => self.stack(stack, &key, path, x, y, width),
            NodeKind::Grid {
                columns,
                gap,
                children,
            } => {
                let columns = usize::from((*columns).max(1));
                let cell = ((width - gap * (columns - 1) as f32) / columns as f32).max(0.0);
                let mut top = y;
                for (row, chunk) in children.chunks(columns).enumerate() {
                    let mut tallest: f32 = 0.0;
                    for (col, child) in chunk.iter().enumerate() {
                        let ix = row * columns + col;
                        let h = self.place(
                            child,
                            &format!("{path}.{ix}"),
                            x + col as f32 * (cell + gap),
                            top,
                            cell,
                        );
                        tallest = tallest.max(h);
                    }
                    top += tallest + gap;
                }
                (top - y - gap).max(0.0)
            }
            NodeKind::Text(text) => {
                let para = self.text_para(text);
                self.out
                    .paint
                    .para(self.ctx, &para, x, y, width, text.align, text.truncate)
            }
            NodeKind::Rule => {
                self.out
                    .paint
                    .rect(x, y, width, 1.0, 0.0, self.palette.border);
                1.0
            }
            NodeKind::Space(size) => *size,
            NodeKind::Stat {
                value,
                label,
                delta,
                tone,
            } => self.stat(value, label, delta.as_deref(), *tone, x, y),
            NodeKind::Table(table) => self.table(table, path, x, y, width),
            NodeKind::Chart(chart) => {
                let legend = chart::legend_height(self.ctx, self.palette, chart, width);
                let height = legend + chart::LEGEND_GAP + chart.height;
                self.surface(
                    &key,
                    [x, y, width, height],
                    SurfaceNode::Chart {
                        chart: chart.clone(),
                        legend,
                    },
                );
                height
            }
            NodeKind::Canvas(canvas) => {
                self.surface(
                    &key,
                    [x, y, width, canvas.height],
                    SurfaceNode::Canvas(canvas.clone()),
                );
                canvas.height
            }
            NodeKind::Scene(scene) => {
                self.surface(
                    &key,
                    [x, y, width, scene.height],
                    SurfaceNode::Scene(scene.clone()),
                );
                scene.height
            }
            NodeKind::Shader(shader) => {
                self.surface(
                    &key,
                    [x, y, width, shader.height],
                    SurfaceNode::Shader(shader.clone()),
                );
                shader.height
            }
            NodeKind::Button {
                label,
                on_click,
                primary,
            } => self.button(&key, label, *on_click, *primary, x, y),
            NodeKind::Tabs {
                options,
                value,
                on_change,
            } => self.tabs(&key, options, value, *on_change, x, y, width),
            NodeKind::Toggle {
                label,
                value,
                on_change,
            } => self.toggle(&key, label, *value, *on_change, x, y),
            NodeKind::Slider {
                label,
                value,
                min,
                max,
                step,
                on_change,
                format,
            } => {
                let shown = format.format(*value);
                self.slider(
                    &key,
                    label.as_deref(),
                    *value,
                    (*min, *max, *step),
                    *on_change,
                    &shown,
                    x,
                    y,
                    width,
                )
            }
            NodeKind::Link { label, url } => {
                let para = self.plain(label, TextSize::Body, Weight::Regular, self.palette.accent);
                let w = para.natural().min(width);
                let h = self
                    .out
                    .paint
                    .para(self.ctx, &para, x, y, width, Align::Start, false);
                self.control(
                    &key,
                    ControlKind::Link { url: url.clone() },
                    label,
                    [x, y, w, h],
                    Action::Link,
                );
                h
            }
        }
    }

    fn surface(&mut self, key: &str, rect: [f32; 4], node: SurfaceNode) {
        self.out.surfaces.push(SurfaceSpec {
            id: stable_id(key),
            rect,
            node,
        });
    }

    fn control(
        &mut self,
        key: &str,
        kind: ControlKind,
        label: &str,
        [x, y, w, h]: [f32; 4],
        action: Action,
    ) {
        self.out.controls.push(ControlSpec {
            control: VisualControl {
                id: stable_id(key),
                kind,
                x,
                y,
                w,
                h,
                label: label.to_owned(),
            },
            action,
        });
    }

    fn stack(
        &mut self,
        stack: &zeron_visual::Stack,
        key: &str,
        path: &str,
        x: f32,
        y: f32,
        width: f32,
    ) -> f32 {
        // A tappable stack's target goes under its children's (controls
        // stack in order), so a button inside it still gets its taps.
        let slot = self.out.controls.len();
        let height = match stack.axis {
            Axis::Vertical => self.column(stack, path, x, y, width),
            Axis::Horizontal if stack.wrap => self.wrapped_row(stack, path, x, y, width),
            Axis::Horizontal => self.row(stack, path, x, y, width),
        };
        if let Some(handler) = stack.on_click {
            self.control(
                key,
                ControlKind::Tap,
                "",
                [x, y, width, height],
                Action::Call(handler, ActionArg::None),
            );
            let tap = self.out.controls.pop().expect("just pushed");
            self.out.controls.insert(slot, tap);
        }
        height
    }

    fn column(
        &mut self,
        stack: &zeron_visual::Stack,
        path: &str,
        x: f32,
        y: f32,
        width: f32,
    ) -> f32 {
        let mut top = y;
        for (ix, child) in stack.children.iter().enumerate() {
            if ix > 0 {
                top += stack.gap;
            }
            // Controls keep their own width; content stretches unless the
            // stack centers or ends it.
            let intrinsic = matches!(
                child.kind,
                NodeKind::Button { .. }
                    | NodeKind::Toggle { .. }
                    | NodeKind::Link { .. }
                    | NodeKind::Space(_)
                    | NodeKind::Stat { .. }
            );
            let w = match child.width {
                Some(w) => w.min(width),
                None if stack.align == Align::Start && !intrinsic => width,
                None => self.natural(child).map_or(width, |n| n.min(width)),
            };
            let offset = match stack.align {
                Align::Start => 0.0,
                Align::Center => (width - w) / 2.0,
                Align::End => width - w,
            };
            top += self.place(child, &format!("{path}.{ix}"), x + offset, top, w);
        }
        top - y
    }

    fn bases(&mut self, stack: &zeron_visual::Stack) -> Vec<Basis> {
        stack
            .children
            .iter()
            .map(|child| match (child.width, child.grow) {
                (Some(w), _) => Basis::Fixed(w),
                (None, true) => Basis::Flex,
                (None, false) => match self.natural(child) {
                    None => Basis::Flex,
                    Some(w) => Basis::Shrink(w),
                },
            })
            .collect()
    }

    /// Place a line of children at their widths, aligned across the line;
    /// returns the line's height.
    fn line(
        &mut self,
        stack: &zeron_visual::Stack,
        path: &str,
        items: &[(usize, f32, f32)],
        y: f32,
    ) -> f32 {
        let mut placed = Vec::with_capacity(items.len());
        for (ix, left, w) in items {
            let start = self.out.mark();
            let h = self.place(&stack.children[*ix], &format!("{path}.{ix}"), *left, y, *w);
            placed.push((start, self.out.mark(), h));
        }
        let tallest = placed.iter().map(|(_, _, h)| *h).fold(0.0, f32::max);
        for (from, to, h) in placed {
            let dy = match stack.align {
                Align::Start => 0.0,
                Align::Center => (tallest - h) / 2.0,
                Align::End => tallest - h,
            };
            self.out.shift(from, to, 0.0, dy);
        }
        tallest
    }

    /// Lefts for `widths` from x, with justify spreading `leftover`.
    fn justify(stack: &zeron_visual::Stack, x: f32, widths: &[f32], leftover: f32) -> Vec<f32> {
        let n = widths.len();
        let (mut at, between) = match stack.justify {
            Justify::Start => (x, 0.0),
            Justify::Center => (x + leftover / 2.0, 0.0),
            Justify::End => (x + leftover, 0.0),
            Justify::Between if n > 1 => (x, leftover / (n - 1) as f32),
            Justify::Between => (x, 0.0),
        };
        widths
            .iter()
            .map(|w| {
                let left = at;
                at += w + stack.gap + between;
                left
            })
            .collect()
    }

    fn row(&mut self, stack: &zeron_visual::Stack, path: &str, x: f32, y: f32, width: f32) -> f32 {
        let n = stack.children.len();
        if n == 0 {
            return 0.0;
        }
        let bases = self.bases(stack);
        let gaps = stack.gap * (n - 1) as f32;
        let used: f32 = bases
            .iter()
            .map(|b| match b {
                Basis::Fixed(w) | Basis::Shrink(w) => *w,
                Basis::Flex => 0.0,
            })
            .sum();
        let free = width - gaps - used;
        let flexes = bases.iter().filter(|b| matches!(b, Basis::Flex)).count();
        let shrinkable: f32 = bases
            .iter()
            .map(|b| if let Basis::Shrink(w) = b { *w } else { 0.0 })
            .sum();
        let widths: Vec<f32> = bases
            .iter()
            .map(|b| match *b {
                Basis::Fixed(w) => w,
                Basis::Flex if free > 0.0 => free / flexes as f32,
                Basis::Flex => 0.0,
                Basis::Shrink(w) if free < 0.0 && shrinkable > 0.0 => {
                    (w + free * w / shrinkable).max(0.0)
                }
                Basis::Shrink(w) => w,
            })
            .collect();
        let leftover = if flexes == 0 { free.max(0.0) } else { 0.0 };
        let lefts = Self::justify(stack, x, &widths, leftover);
        let items: Vec<(usize, f32, f32)> = (0..n).map(|i| (i, lefts[i], widths[i])).collect();
        self.line(stack, path, &items, y)
    }

    fn wrapped_row(
        &mut self,
        stack: &zeron_visual::Stack,
        path: &str,
        x: f32,
        y: f32,
        width: f32,
    ) -> f32 {
        let bases = self.bases(stack);
        let mut lines: Vec<Vec<(usize, f32, bool)>> = vec![Vec::new()];
        let mut cursor = 0.0;
        for (ix, basis) in bases.iter().enumerate() {
            let (w, flex) = match *basis {
                Basis::Fixed(w) => (w.min(width), false),
                Basis::Shrink(w) => (w.min(width), false),
                Basis::Flex => (
                    self.natural(&stack.children[ix])
                        .unwrap_or(width)
                        .min(width),
                    true,
                ),
            };
            let line = lines.last_mut().expect("one line");
            let next = if line.is_empty() {
                w
            } else {
                cursor + stack.gap + w
            };
            if !line.is_empty() && next > width + 0.5 {
                lines.push(vec![(ix, w, flex)]);
                cursor = w;
            } else {
                line.push((ix, w, flex));
                cursor = next;
            }
        }
        let mut top = y;
        for (n, line) in lines.iter().enumerate() {
            if line.is_empty() {
                continue;
            }
            if n > 0 {
                top += stack.gap;
            }
            let used: f32 =
                line.iter().map(|(_, w, _)| w).sum::<f32>() + stack.gap * (line.len() - 1) as f32;
            let free = (width - used).max(0.0);
            let flexes = line.iter().filter(|(_, _, flex)| *flex).count();
            let widths: Vec<f32> = line
                .iter()
                .map(|(_, w, flex)| {
                    if *flex && flexes > 0 {
                        w + free / flexes as f32
                    } else {
                        *w
                    }
                })
                .collect();
            let leftover = if flexes == 0 { free } else { 0.0 };
            let lefts = Self::justify(stack, x, &widths, leftover);
            let items: Vec<(usize, f32, f32)> = line
                .iter()
                .enumerate()
                .map(|(i, (ix, _, _))| (*ix, lefts[i], widths[i]))
                .collect();
            top += self.line(stack, path, &items, top);
        }
        top - y
    }

    /// A headline figure over a short label: plain type, no tile.
    fn stat(
        &mut self,
        value: &str,
        label: &str,
        delta: Option<&str>,
        tone: Option<zeron_visual::Color>,
        x: f32,
        y: f32,
    ) -> f32 {
        let tone = tone.map(|c| self.palette.color(c));
        let delta = delta.filter(|d| !d.is_empty());
        // A tone marks the change, not the figure; a figure with no change
        // takes the tone itself.
        let value_color = tone
            .filter(|_| delta.is_none())
            .unwrap_or(self.palette.text);
        let figure = self.plain(value, TextSize::Xxl, Weight::Semibold, value_color);
        let value_w = figure.natural();
        let mut height = self.out.paint.para(
            self.ctx,
            &figure,
            x,
            y,
            value_w.max(1.0),
            Align::Start,
            true,
        );
        if let Some(delta) = delta {
            let change = self.plain(
                delta,
                TextSize::Sm,
                Weight::Regular,
                tone.unwrap_or(self.palette.muted),
            );
            // Baseline-aligned with the figure.
            let top = y + figure.base - change.base;
            self.out.paint.para(
                self.ctx,
                &change,
                x + value_w + 6.0,
                top,
                change.natural().max(1.0),
                Align::Start,
                true,
            );
        }
        if !label.is_empty() {
            let caption = self.plain(label, TextSize::Sm, Weight::Regular, self.palette.muted);
            height += 2.0;
            height += self.out.paint.para(
                self.ctx,
                &caption,
                x,
                y + height,
                caption.natural().max(1.0),
                Align::Start,
                false,
            );
        }
        height
    }

    /// The markdown table's frameless treatment: a bold header and hairlines
    /// between rows, no outer box or fills.
    fn table(
        &mut self,
        table: &zeron_visual::TableNode,
        path: &str,
        x: f32,
        y: f32,
        width: f32,
    ) -> f32 {
        let columns = table
            .columns
            .len()
            .max(table.rows.iter().map(Vec::len).max().unwrap_or(0));
        if columns == 0 {
            return 0.0;
        }
        let column = |c: usize| table.columns.get(c).cloned().unwrap_or_default();
        let fixed: f32 = (0..columns).filter_map(|c| column(c).width).sum();
        let flexes = (0..columns).filter(|c| column(*c).width.is_none()).count();
        let share = if flexes > 0 {
            ((width - fixed) / flexes as f32).max(0.0)
        } else {
            0.0
        };
        let widths: Vec<f32> = (0..columns)
            .map(|c| column(c).width.unwrap_or(share))
            .collect();
        let lefts: Vec<f32> = widths
            .iter()
            .scan(x, |at, w| {
                let left = *at;
                *at += w;
                Some(left)
            })
            .collect();
        let hairline = self.palette.text.alpha(0.10);
        let mut top = y;
        // One row: each cell's content aligned in its padded box.
        let row = |this: &mut Self,
                   top: f32,
                   cells: &mut dyn FnMut(&mut Self, usize, f32, f32, f32) -> f32|
         -> f32 {
            let mut tallest: f32 = 0.0;
            for c in 0..columns {
                let inner = (widths[c] - TABLE_CELL_PAD * 2.0).max(0.0);
                tallest = tallest.max(cells(
                    this,
                    c,
                    lefts[c] + TABLE_CELL_PAD,
                    top + TABLE_CELL_PAD_Y,
                    inner,
                ));
            }
            tallest + TABLE_CELL_PAD_Y * 2.0
        };
        if table.columns.iter().any(|c| !c.label.is_empty()) {
            top += row(self, top, &mut |this, c, cx, cy, inner| {
                let col = column(c);
                let para = this.plain(&col.label, TextSize::Body, Weight::Bold, this.palette.text);
                let w = para.natural().min(inner);
                let offset = align_offset(col.align, inner, w);
                this.out.paint.para(
                    this.ctx,
                    &para,
                    cx + offset,
                    cy,
                    w.max(1.0),
                    Align::Start,
                    false,
                )
            });
        }
        for (r, cells) in table.rows.iter().enumerate() {
            if r > 0 || !table.columns.is_empty() {
                self.out.paint.rect(x, top, width, 1.0, 0.0, hairline);
                top += 1.0;
            }
            top += row(self, top, &mut |this, c, cx, cy, inner| {
                let Some(node) = cells.get(c) else {
                    return 0.0;
                };
                let w = this.natural(node).map_or(inner, |n| n.min(inner));
                let offset = align_offset(column(c).align, inner, w);
                this.place(node, &format!("{path}.{r}.{c}"), cx + offset, cy, w)
            });
        }
        top - y
    }

    fn button(
        &mut self,
        key: &str,
        label: &str,
        handler: HandlerId,
        primary: bool,
        x: f32,
        y: f32,
    ) -> f32 {
        let w = self.button_width(label);
        let (fill, ink) = if primary {
            (self.palette.accent, self.palette.on_accent)
        } else {
            (self.palette.wash, self.palette.text)
        };
        self.out.paint.rect(x, y, w, BUTTON_H, 8.0, fill);
        let para = self.plain(label, TextSize::Body, Weight::Medium, ink);
        self.out.paint.para(
            self.ctx,
            &para,
            x + BUTTON_PAD,
            y + (BUTTON_H - para.lh) / 2.0,
            w,
            Align::Start,
            true,
        );
        self.control(
            key,
            ControlKind::Button { primary },
            label,
            [x, y, w, BUTTON_H],
            Action::Call(handler, ActionArg::None),
        );
        BUTTON_H
    }

    #[allow(clippy::too_many_arguments)]
    fn tabs(
        &mut self,
        key: &str,
        options: &[String],
        value: &str,
        handler: HandlerId,
        x: f32,
        y: f32,
        width: f32,
    ) -> f32 {
        let (mut cx, mut top) = (x, y);
        for (ix, option) in options.iter().enumerate() {
            let w = self.tab_width(option);
            if cx > x && cx + w > x + width + 0.5 {
                cx = x;
                top += TAB_H + TAB_GAP;
            }
            let selected = option == value;
            if selected {
                self.out
                    .paint
                    .rect(cx, top, w, TAB_H, 8.0, self.palette.accent.alpha(0.14));
            }
            let (weight, ink) = if selected {
                (Weight::Semibold, self.palette.accent)
            } else {
                (Weight::Medium, self.palette.muted)
            };
            let para = self.plain(option, TextSize::Body, weight, ink);
            self.out.paint.para(
                self.ctx,
                &para,
                cx + TAB_PAD,
                top + (TAB_H - para.lh) / 2.0,
                w,
                Align::Start,
                true,
            );
            self.control(
                &format!("{key}#tab{ix}"),
                ControlKind::Tab { selected },
                option,
                [cx, top, w, TAB_H],
                Action::Call(handler, ActionArg::Text(option.clone())),
            );
            cx += w + TAB_GAP;
        }
        if options.is_empty() {
            0.0
        } else {
            top + TAB_H - y
        }
    }

    fn toggle(
        &mut self,
        key: &str,
        label: &str,
        on: bool,
        handler: HandlerId,
        x: f32,
        y: f32,
    ) -> f32 {
        let para = self.plain(label, TextSize::Body, Weight::Regular, self.palette.text);
        let h = para.lh.max(TOGGLE_H);
        let track_y = y + (h - TOGGLE_H) / 2.0;
        let track = if on {
            self.palette.accent
        } else {
            self.palette.text.alpha(0.16)
        };
        self.out
            .paint
            .rect(x, track_y, TOGGLE_W, TOGGLE_H, TOGGLE_H / 2.0, track);
        let knob = TOGGLE_H - 4.0;
        let knob_x = if on {
            x + TOGGLE_W - knob - 2.0
        } else {
            x + 2.0
        };
        self.out.paint.rect(
            knob_x,
            track_y + 2.0,
            knob,
            knob,
            knob / 2.0,
            self.palette.on_accent,
        );
        let text_x = x + TOGGLE_W + TOGGLE_GAP;
        let text_w = para.natural();
        self.out.paint.para(
            self.ctx,
            &para,
            text_x,
            y + (h - para.lh) / 2.0,
            text_w.max(1.0),
            Align::Start,
            true,
        );
        self.control(
            key,
            ControlKind::Toggle { on },
            label,
            [x, y, TOGGLE_W + TOGGLE_GAP + text_w, h],
            Action::Call(handler, ActionArg::Bool(!on)),
        );
        h
    }

    #[allow(clippy::too_many_arguments)]
    fn slider(
        &mut self,
        key: &str,
        label: Option<&str>,
        value: f64,
        (min, max, step): (f64, f64, f64),
        handler: HandlerId,
        shown: &str,
        x: f32,
        y: f32,
        width: f32,
    ) -> f32 {
        let caption = self.plain(
            label.unwrap_or(""),
            TextSize::Sm,
            Weight::Regular,
            self.palette.muted,
        );
        let readout = self.plain(shown, TextSize::Sm, Weight::Regular, self.palette.text);
        let readout_w = readout.natural();
        self.out.paint.para(
            self.ctx,
            &caption,
            x,
            y,
            (width - readout_w - 12.0).max(1.0),
            Align::Start,
            true,
        );
        self.out.paint.para(
            self.ctx,
            &readout,
            x + width - readout_w,
            y,
            readout_w.max(1.0),
            Align::Start,
            true,
        );
        let row = caption.lh.max(readout.lh);
        let track_top = y + row + 4.0;
        let track_h = SLIDER_THUMB + 4.0;
        let mid = track_top + track_h / 2.0;
        let left = x + SLIDER_THUMB / 2.0;
        let span = (width - SLIDER_THUMB).max(1.0);
        let fraction = if max > min {
            ((value - min) / (max - min)).clamp(0.0, 1.0) as f32
        } else {
            0.0
        };
        let at = left + span * fraction;
        let half = SLIDER_TRACK / 2.0;
        self.out.paint.rect(
            left,
            mid - half,
            span,
            SLIDER_TRACK,
            half,
            self.palette.border,
        );
        self.out.paint.rect(
            left,
            mid - half,
            at - left,
            SLIDER_TRACK,
            half,
            self.palette.accent,
        );
        self.out
            .paint
            .dot([at, mid], SLIDER_THUMB / 2.0, self.palette.text);
        self.control(
            key,
            ControlKind::Slider {
                value,
                min,
                max,
                step,
                fraction,
                track_x: left,
                track_w: span,
                shown: shown.to_owned(),
            },
            label.unwrap_or(""),
            [x, track_top, width, track_h],
            Action::Call(handler, ActionArg::Slider { min, max, step }),
        );
        track_top + track_h - y
    }
}

fn align_offset(align: Align, room: f32, w: f32) -> f32 {
    match align {
        Align::Start => 0.0,
        Align::Center => ((room - w) / 2.0).max(0.0),
        Align::End => (room - w).max(0.0),
    }
}

/// How a surface takes touches (canvas pointer use is learned by drawing).
pub(crate) fn gesture(node: &SurfaceNode, reads_pointer: bool) -> SurfaceGesture {
    match node {
        SurfaceNode::Scene(scene) if scene.orbit => SurfaceGesture::Drag,
        SurfaceNode::Scene(_) => SurfaceGesture::Tap,
        SurfaceNode::Shader(shader)
            if shader.orbit
                || shader.code.contains("iMouse")
                || shader.buffers.iter().any(|b| b.contains("iMouse")) =>
        {
            SurfaceGesture::Drag
        }
        SurfaceNode::Shader(_) => SurfaceGesture::None,
        SurfaceNode::Canvas(_) if reads_pointer => SurfaceGesture::Drag,
        SurfaceNode::Canvas(canvas) if canvas.on_click.is_some() => SurfaceGesture::Tap,
        SurfaceNode::Canvas(_) => SurfaceGesture::None,
        SurfaceNode::Chart { .. } => SurfaceGesture::HorizontalDrag,
    }
}
