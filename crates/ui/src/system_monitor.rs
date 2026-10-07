//! The Monitor surface: live CPU, memory, GPU, network and disk usage of the
//! device a chat runs on, over `WatchSystemStats`.
//!
//! The view owns its subscription. It samples only while it is on screen: the
//! watch pauses once the surface has gone unrendered for a few frames and
//! resumes on the next render, so a Monitor tab left in the background costs
//! the host nothing.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use gpui::{
    Context, Entity, Hsla, IntoElement, PathBuilder, Render, SharedString, Task, Window, canvas,
    div, point, prelude::*, px,
};
use zeron_proto::{GpuStats, ProcessStats, SystemStats, capabilities};
use zeron_rpc::{RpcError, methods};

use crate::icons::{self, icon};
use crate::popover;
use crate::settings::accounts::{usage_color, usage_level};
use crate::settings::widgets::{self, PageScroll};
use crate::state::AppState;
use crate::theme::Theme;

/// Samples kept per series, and the graph's fixed x-scale: newest at the
/// right edge, so a fresh graph fills in from the right.
const HISTORY_LEN: usize = 30;
const SAMPLE_INTERVAL_MS: u64 = 2000;
/// Unrendered for this long → stop sampling. Frames arrive every
/// [`SAMPLE_INTERVAL_MS`], so a visible surface re-arms this each frame.
const PAUSE_AFTER: Duration = Duration::from_millis(SAMPLE_INTERVAL_MS * 2 + 500);
const PROCESS_ROWS: usize = 8;
/// A rate graph never scales below this, so idle noise doesn't read as load.
const RATE_FLOOR: f32 = 100_000.0;
const GRAPH_HEIGHT: f32 = 44.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProcessSort {
    Cpu,
    Memory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Link {
    Connecting,
    Live,
    /// The device's engine predates `WatchSystemStats`.
    Unsupported,
    Lost(SharedString),
}

#[derive(Default)]
struct History {
    cpu: VecDeque<f32>,
    memory: VecDeque<f32>,
    gpu: VecDeque<f32>,
    net_in: VecDeque<f32>,
    net_out: VecDeque<f32>,
    disk_read: VecDeque<f32>,
    disk_write: VecDeque<f32>,
}

fn push_capped(series: &mut VecDeque<f32>, value: f32) {
    if series.len() == HISTORY_LEN {
        series.pop_front();
    }
    series.push_back(value);
}

impl History {
    fn record(&mut self, stats: &SystemStats) {
        push_capped(&mut self.cpu, stats.cpu.total_percent);
        push_capped(
            &mut self.memory,
            fraction(stats.memory.used_bytes, stats.memory.total_bytes) * 100.0,
        );
        if let Some(gpu) = stats.gpus.first() {
            push_capped(&mut self.gpu, gpu.utilization_percent.unwrap_or(0.0));
        }
        push_capped(&mut self.net_in, stats.network.received_per_sec as f32);
        push_capped(&mut self.net_out, stats.network.transmitted_per_sec as f32);
        push_capped(
            &mut self.disk_read,
            stats.disks.read_per_sec.unwrap_or(0) as f32,
        );
        push_capped(
            &mut self.disk_write,
            stats.disks.write_per_sec.unwrap_or(0) as f32,
        );
    }
}

pub struct SystemMonitor {
    state: Entity<AppState>,
    /// `targetDeviceId` for a device other than the connected engine's.
    target: Option<String>,
    stats: Option<SystemStats>,
    history: History,
    link: Link,
    cores_open: bool,
    sort: ProcessSort,
    scroll: PageScroll,
    last_render: Instant,
    watch: Option<Task<()>>,
}

impl SystemMonitor {
    pub fn new(state: Entity<AppState>, target: Option<String>) -> Self {
        Self {
            state,
            target,
            stats: None,
            history: History::default(),
            link: Link::Connecting,
            cores_open: false,
            sort: ProcessSort::Cpu,
            scroll: PageScroll::default(),
            last_render: Instant::now(),
            watch: None,
        }
    }

    /// The device this monitor reads, for the tab title.
    pub fn target(&self) -> Option<&str> {
        self.target.as_deref()
    }

    /// Start (or resume) sampling. Idempotent; cheap enough to call per render.
    pub fn ensure_watching(&mut self, cx: &mut Context<Self>) {
        self.last_render = Instant::now();
        if self.watch.is_some() {
            return;
        }
        let (engine, supported) = {
            let state = self.state.read(cx);
            let device = self
                .target
                .clone()
                .or_else(|| state.engine().map(|e| e.engine_info().device_id.clone()));
            let supported = device
                .as_deref()
                .is_none_or(|device| state.device_supports(device, capabilities::SYSTEM_STATS_V1));
            (state.engine().cloned(), supported)
        };
        if !supported {
            self.link = Link::Unsupported;
            return;
        }
        let Some(engine) = engine else {
            return;
        };
        if self.stats.is_none() {
            self.link = Link::Connecting;
        }
        let target = self.target.clone();
        self.watch = Some(cx.spawn(async move |this, cx| {
            let mut retry = 1;
            loop {
                let mut params = serde_json::json!({ "intervalMs": SAMPLE_INTERVAL_MS });
                if let Some(target) = &target {
                    params["targetDeviceId"] = serde_json::json!(target);
                }
                let failure = match engine
                    .client()
                    .subscribe_checked(methods::WATCH_SYSTEM_STATS, params)
                    .await
                {
                    Ok(mut stream) => {
                        while let Some(value) = stream.recv().await {
                            let Ok(stats) = serde_json::from_value::<SystemStats>(value) else {
                                continue;
                            };
                            retry = 1;
                            match this.update(cx, |monitor, cx| monitor.accept(stats, cx)) {
                                Ok(true) => {}
                                // Paused (or the view is gone): leave the stream.
                                Ok(false) | Err(_) => return,
                            }
                        }
                        None
                    }
                    Err(RpcError::UnknownMethod(_)) => {
                        let _ = this.update(cx, |monitor, cx| {
                            monitor.link = Link::Unsupported;
                            monitor.watch = None;
                            cx.notify();
                        });
                        return;
                    }
                    Err(error) => Some(error.to_string()),
                };
                let message = failure.unwrap_or_else(|| "Connection closed".into());
                if this
                    .update(cx, |monitor, cx| {
                        monitor.link = Link::Lost(message.into());
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
                cx.background_executor()
                    .timer(Duration::from_secs(retry))
                    .await;
                retry = (retry * 2).min(15);
                // Don't keep redialing a device for a surface nobody sees.
                if this
                    .update(cx, |monitor, _| monitor.pause_if_hidden())
                    .unwrap_or(true)
                {
                    return;
                }
            }
        }));
    }

    /// Nobody has looked at this surface lately: drop the watch (and with it
    /// the subscription) until the next render re-arms it.
    fn pause_if_hidden(&mut self) -> bool {
        let hidden = self.last_render.elapsed() > PAUSE_AFTER;
        if hidden {
            self.watch = None;
        }
        hidden
    }

    /// Take a frame. `false` tells the watch to stop because nobody is looking.
    fn accept(&mut self, stats: SystemStats, cx: &mut Context<Self>) -> bool {
        if self.pause_if_hidden() {
            return false;
        }
        self.history.record(&stats);
        self.stats = Some(stats);
        self.link = Link::Live;
        cx.notify();
        true
    }

    #[cfg(feature = "monitor-fixture")]
    pub fn fixture_view(
        &mut self,
        cores_open: bool,
        by_memory: bool,
        scroll_y: f32,
        cx: &mut Context<Self>,
    ) {
        self.scroll.scroll.set_offset(point(px(0.0), px(-scroll_y)));
        self.cores_open = cores_open;
        self.sort = if by_memory {
            ProcessSort::Memory
        } else {
            ProcessSort::Cpu
        };
        cx.notify();
    }

    fn on_scroll_hovered(&mut self, hovered: &bool, _: &mut Window, cx: &mut Context<Self>) {
        if self.scroll.set_list_hovered(*hovered) {
            cx.notify();
        }
    }
}

impl popover::ScrollRailHost for SystemMonitor {
    fn rail_bar(&mut self) -> &mut popover::MenuScrollbarState {
        self.scroll.rail_bar()
    }

    fn rail_scroll(&self) -> Option<gpui::ScrollHandle> {
        self.scroll.rail_scroll()
    }
}

// ─── formatting (pure) ──────────────────────────────────────────────────────

fn fraction(part: u64, whole: u64) -> f32 {
    if whole == 0 {
        0.0
    } else {
        (part as f64 / whole as f64).clamp(0.0, 1.0) as f32
    }
}

fn scaled(bytes: u64, base: f64, units: &[&str]) -> String {
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= base && unit + 1 < units.len() {
        value /= base;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", units[0])
    } else if value >= 100.0 {
        format!("{value:.0} {}", units[unit])
    } else {
        format!("{value:.1} {}", units[unit])
    }
}

/// Memory as the OS reports it: binary multiples, as Activity Monitor shows.
fn format_memory(bytes: u64) -> String {
    scaled(bytes, 1024.0, &["B", "KB", "MB", "GB", "TB"])
}

/// Disk capacity and throughput in decimal multiples, as Finder and network
/// tools show them.
fn format_size(bytes: u64) -> String {
    scaled(bytes, 1000.0, &["B", "KB", "MB", "GB", "TB"])
}

fn format_rate(bytes_per_sec: u64) -> String {
    format!("{}/s", format_size(bytes_per_sec))
}

fn format_percent(percent: f32) -> String {
    format!("{}%", percent.round() as u32)
}

fn format_uptime(secs: u64) -> String {
    let (days, hours, minutes) = (secs / 86_400, secs / 3600 % 24, secs / 60 % 60);
    match (days, hours) {
        (0, 0) => format!("{minutes}m"),
        (0, _) => format!("{hours}h {minutes}m"),
        _ => format!("{days}d {hours}h"),
    }
}

/// The rows of the process list under `sort`, heaviest first.
fn sorted_processes(processes: &[ProcessStats], sort: ProcessSort) -> Vec<&ProcessStats> {
    let mut rows: Vec<&ProcessStats> = processes.iter().collect();
    rows.sort_by(|a, b| match sort {
        ProcessSort::Cpu => b
            .cpu_percent
            .partial_cmp(&a.cpu_percent)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.memory_bytes.cmp(&a.memory_bytes)),
        ProcessSort::Memory => b.memory_bytes.cmp(&a.memory_bytes).then(
            b.cpu_percent
                .partial_cmp(&a.cpu_percent)
                .unwrap_or(std::cmp::Ordering::Equal),
        ),
    });
    rows.truncate(PROCESS_ROWS);
    rows
}

// ─── building blocks ────────────────────────────────────────────────────────

fn text_size(px_size: f32) -> gpui::Rems {
    crate::typography::ui_rems(px_size)
}

/// One resource's block: the settings pages' faint fill, no outline.
fn card(theme: &Theme) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .p(px(14.0))
        .rounded(px(12.0))
        .bg(widgets::block_fill(theme))
}

fn card_header(theme: &Theme, title: &'static str, detail: impl Into<SharedString>) -> gpui::Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap(px(12.0))
        .child(
            div()
                .flex_none()
                .text_size(text_size(13.0))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(theme.text)
                .child(title),
        )
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_size(text_size(11.5))
                .text_color(theme.text_muted)
                .child(detail.into()),
        )
}

fn big_value(text: impl Into<SharedString>, color: Hsla) -> gpui::Div {
    div()
        .text_size(text_size(22.0))
        .line_height(px(26.0))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(color)
        .child(text.into())
}

/// A label on the left and its value on the right, both 12px.
fn stat_row(
    theme: &Theme,
    label: impl Into<SharedString>,
    value: impl Into<SharedString>,
) -> gpui::Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap(px(12.0))
        .text_size(text_size(12.0))
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_color(theme.text_muted)
                .child(label.into()),
        )
        .child(div().flex_none().text_color(theme.text).child(value.into()))
}

/// The accounts page's 4px meter: a wash track, a rounded fill, and the same
/// amber and red thresholds.
fn meter(theme: &Theme, fraction: f32) -> gpui::Div {
    let level = usage_level(fraction);
    let fill = usage_color(level, theme).opacity(match level {
        crate::settings::accounts::UsageLevel::Normal => 0.8,
        _ => 0.9,
    });
    div()
        .w_full()
        .h(px(4.0))
        .flex_none()
        .rounded_full()
        .overflow_hidden()
        .bg(theme.wash(0.08))
        .when(fraction > 0.0, |el| {
            el.child(
                div()
                    .h_full()
                    .w(gpui::relative(fraction.max(0.015)))
                    .rounded_full()
                    .bg(fill),
            )
        })
}

fn level_color(theme: &Theme, percent: f32) -> Hsla {
    usage_color(usage_level(percent / 100.0), theme)
}

/// A filled line graph. Every series shares the scale `0..max`, and the
/// x-scale is fixed at [`HISTORY_LEN`] samples with the newest at the right.
fn sparkline(theme: &Theme, series: Vec<(Vec<f32>, Hsla)>, max: f32) -> impl IntoElement {
    let max = max.max(f32::EPSILON);
    div()
        .w_full()
        .h(px(GRAPH_HEIGHT))
        .flex_none()
        .rounded(px(8.0))
        .overflow_hidden()
        .bg(theme.wash(0.04))
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    let width = f32::from(bounds.size.width);
                    let height = f32::from(bounds.size.height);
                    let step = width / (HISTORY_LEN - 1) as f32;
                    // Keep the 1.5px stroke inside the box at 0 and at max.
                    let inset = 1.5;
                    for (values, color) in &series {
                        if values.len() < 2 {
                            continue;
                        }
                        let last = values.len() - 1;
                        let at = |i: usize, v: f32| {
                            point(
                                bounds.origin.x + px(width - step * (last - i) as f32),
                                bounds.origin.y
                                    + px(height
                                        - inset
                                        - (v / max).clamp(0.0, 1.0) * (height - 2.0 * inset)),
                            )
                        };
                        let floor = bounds.origin.y + px(height);
                        let mut area = PathBuilder::fill();
                        area.move_to(point(at(0, 0.0).x, floor));
                        for (i, v) in values.iter().enumerate() {
                            area.line_to(at(i, *v));
                        }
                        area.line_to(point(at(last, 0.0).x, floor));
                        area.close();
                        if let Ok(path) = area.build() {
                            window.paint_path(path, color.opacity(0.16));
                        }
                        let mut line = PathBuilder::stroke(px(1.5));
                        for (i, v) in values.iter().enumerate() {
                            if i == 0 {
                                line.move_to(at(i, *v));
                            } else {
                                line.line_to(at(i, *v));
                            }
                        }
                        if let Ok(path) = line.build() {
                            window.paint_path(path, *color);
                        }
                    }
                },
            )
            .size_full(),
        )
}

fn series(values: &VecDeque<f32>) -> Vec<f32> {
    values.iter().copied().collect()
}

fn rate_scale(all: &[&VecDeque<f32>]) -> f32 {
    all.iter()
        .flat_map(|s| s.iter().copied())
        .fold(RATE_FLOOR, f32::max)
}

// ─── sections ───────────────────────────────────────────────────────────────

impl SystemMonitor {
    fn cpu_card(&self, stats: &SystemStats, theme: &Theme, cx: &mut Context<Self>) -> gpui::Div {
        let cpu = &stats.cpu;
        let color = level_color(theme, cpu.total_percent);
        let detail = match cpu.physical_cores {
            Some(physical) if physical as usize != cpu.per_core_percent.len() => format!(
                "{} · {physical} cores, {} threads",
                cpu.brand,
                cpu.per_core_percent.len()
            ),
            _ => format!("{} · {} cores", cpu.brand, cpu.per_core_percent.len()),
        };
        let open = self.cores_open;
        card(theme)
            .child(card_header(theme, "CPU", detail))
            .child(big_value(format_percent(cpu.total_percent), color))
            .child(sparkline(
                theme,
                vec![(series(&self.history.cpu), color)],
                100.0,
            ))
            .when_some(cpu.load_average, |el, load| {
                el.child(stat_row(
                    theme,
                    "Load average",
                    format!("{:.2}  {:.2}  {:.2}", load[0], load[1], load[2]),
                ))
            })
            .child(
                div()
                    .id("system-monitor-cores-toggle")
                    .role(gpui::Role::Button)
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .h(px(28.0))
                    .px(px(8.0))
                    .mx(px(-8.0))
                    .rounded(px(8.0))
                    .cursor_pointer()
                    .text_size(text_size(12.0))
                    .text_color(theme.text_muted)
                    .hover(|s| s.bg(theme.glass_hover()).text_color(theme.text))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.cores_open = !this.cores_open;
                        cx.notify();
                    }))
                    .child("Usage by core")
                    .child(
                        icon(if open {
                            icons::ALT_ARROW_UP
                        } else {
                            icons::ALT_ARROW_DOWN
                        })
                        .size(px(12.0))
                        .text_color(theme.text_muted),
                    ),
            )
            .when(open, |el| el.child(core_grid(&cpu.per_core_percent, theme)))
    }

    fn memory_card(&self, stats: &SystemStats, theme: &Theme) -> gpui::Div {
        let memory = &stats.memory;
        let used = fraction(memory.used_bytes, memory.total_bytes);
        let color = level_color(theme, used * 100.0);
        card(theme)
            .child(card_header(
                theme,
                "Memory",
                format!(
                    "{} of {}",
                    format_memory(memory.used_bytes),
                    format_memory(memory.total_bytes)
                ),
            ))
            .child(big_value(format_percent(used * 100.0), color))
            .child(sparkline(
                theme,
                vec![(series(&self.history.memory), color)],
                100.0,
            ))
            .child(meter(theme, used))
            .child(stat_row(
                theme,
                "Available",
                format_memory(memory.total_bytes.saturating_sub(memory.used_bytes)),
            ))
            .when(memory.swap_total_bytes > 0, |el| {
                el.child(stat_row(
                    theme,
                    "Swap",
                    format!(
                        "{} of {}",
                        format_memory(memory.swap_used_bytes),
                        format_memory(memory.swap_total_bytes)
                    ),
                ))
            })
    }

    fn gpu_card(&self, gpus: &[GpuStats], theme: &Theme) -> gpui::Div {
        let mut block = card(theme);
        if gpus.is_empty() {
            return block
                .child(card_header(theme, "GPU", ""))
                .child(muted_note(theme, "Not reported on this device."));
        }
        for (ix, gpu) in gpus.iter().enumerate() {
            let percent = gpu.utilization_percent;
            let color = level_color(theme, percent.unwrap_or(0.0));
            block = block
                .child(card_header(
                    theme,
                    if ix == 0 { "GPU" } else { "" },
                    gpu.name.clone(),
                ))
                .child(big_value(
                    percent.map(format_percent).unwrap_or_else(|| "–".into()),
                    color,
                ));
            // Only the first GPU is graphed; the rest keep their readout.
            if ix == 0 {
                block = block.child(sparkline(
                    theme,
                    vec![(series(&self.history.gpu), color)],
                    100.0,
                ));
            }
            match (gpu.memory_used_bytes, gpu.memory_total_bytes) {
                (Some(used), Some(total)) => {
                    block = block
                        .child(meter(theme, fraction(used, total)))
                        .child(stat_row(
                            theme,
                            "Video memory",
                            format!("{} of {}", format_memory(used), format_memory(total)),
                        ));
                }
                (Some(used), None) => {
                    block =
                        block.child(stat_row(theme, "Shared memory in use", format_memory(used)));
                }
                _ => {}
            }
            if let Some(celsius) = gpu.temperature_celsius {
                block = block.child(stat_row(theme, "Temperature", format!("{celsius:.0} °C")));
            }
        }
        block
    }

    fn network_card(&self, stats: &SystemStats, theme: &Theme) -> gpui::Div {
        let network = &stats.network;
        let down = theme.accent;
        let up = theme.text_faint;
        let scale = rate_scale(&[&self.history.net_in, &self.history.net_out]);
        let mut block = card(theme)
            .child(card_header(
                theme,
                "Network",
                format!("Graph scale {}", format_rate(scale as u64)),
            ))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(20.0))
                    .child(rate_readout(
                        "Download",
                        network.received_per_sec,
                        down,
                        theme,
                    ))
                    .child(rate_readout(
                        "Upload",
                        network.transmitted_per_sec,
                        up,
                        theme,
                    )),
            )
            .child(sparkline(
                theme,
                vec![
                    (series(&self.history.net_in), down),
                    (series(&self.history.net_out), up),
                ],
                scale,
            ));
        for interface in network.interfaces.iter().take(3) {
            block = block.child(stat_row(
                theme,
                interface.name.clone(),
                format!(
                    "↓ {}  ↑ {}",
                    format_size(interface.total_received_bytes),
                    format_size(interface.total_transmitted_bytes)
                ),
            ));
        }
        block
    }

    fn disk_card(&self, stats: &SystemStats, theme: &Theme) -> gpui::Div {
        let disks = &stats.disks;
        let read = theme.accent;
        let write = theme.text_faint;
        let scale = rate_scale(&[&self.history.disk_read, &self.history.disk_write]);
        let mut block = card(theme).child(card_header(theme, "Disk", ""));
        if let (Some(r), Some(w)) = (disks.read_per_sec, disks.write_per_sec) {
            block = block
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap(px(20.0))
                        .child(rate_readout("Read", r, read, theme))
                        .child(rate_readout("Write", w, write, theme)),
                )
                .child(sparkline(
                    theme,
                    vec![
                        (series(&self.history.disk_read), read),
                        (series(&self.history.disk_write), write),
                    ],
                    scale,
                ));
        }
        for volume in &disks.volumes {
            let used = volume.total_bytes.saturating_sub(volume.available_bytes);
            block = block.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(stat_row(
                        theme,
                        volume.name.clone(),
                        format!(
                            "{} free of {}",
                            format_size(volume.available_bytes),
                            format_size(volume.total_bytes)
                        ),
                    ))
                    .child(meter(theme, fraction(used, volume.total_bytes))),
            );
        }
        block
    }

    fn process_card(
        &self,
        stats: &SystemStats,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let rows = sorted_processes(&stats.processes, self.sort);
        let heading = |id: &'static str, label: &'static str, sort: ProcessSort, width: f32| {
            let active = self.sort == sort;
            div()
                .id(id)
                .role(gpui::Role::Button)
                .w(px(width))
                .flex_none()
                .text_right()
                .cursor_pointer()
                .text_color(if active { theme.text } else { theme.text_muted })
                .hover(|s| s.text_color(theme.text))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.sort != sort {
                        this.sort = sort;
                        cx.notify();
                    }
                }))
                .child(label)
        };
        let mut block = card(theme)
            .child(card_header(
                theme,
                "Processes",
                match self.sort {
                    ProcessSort::Cpu => "Sorted by CPU",
                    ProcessSort::Memory => "Sorted by memory",
                },
            ))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .text_size(text_size(11.5))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(theme.text_muted)
                            .child("Name"),
                    )
                    .child(heading(
                        "system-monitor-sort-cpu",
                        "CPU",
                        ProcessSort::Cpu,
                        48.0,
                    ))
                    .child(heading(
                        "system-monitor-sort-memory",
                        "Memory",
                        ProcessSort::Memory,
                        64.0,
                    )),
            );
        for process in rows {
            block = block.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .h(px(22.0))
                    .text_size(text_size(12.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(theme.text)
                            .child(SharedString::from(process.name.clone())),
                    )
                    .child(
                        div()
                            .w(px(48.0))
                            .flex_none()
                            .text_right()
                            .text_color(if self.sort == ProcessSort::Cpu {
                                theme.text
                            } else {
                                theme.text_muted
                            })
                            .child(format!("{:.1}%", process.cpu_percent)),
                    )
                    .child(
                        div()
                            .w(px(64.0))
                            .flex_none()
                            .text_right()
                            .text_color(if self.sort == ProcessSort::Memory {
                                theme.text
                            } else {
                                theme.text_muted
                            })
                            .child(format_memory(process.memory_bytes)),
                    ),
            );
        }
        block
    }
}

fn muted_note(theme: &Theme, copy: &'static str) -> gpui::Div {
    div()
        .text_size(text_size(12.0))
        .line_height(px(16.0))
        .text_color(theme.text_muted.opacity(0.7))
        .child(copy)
}

/// A labelled throughput figure whose label carries the graph line's color.
fn rate_readout(label: &'static str, bytes_per_sec: u64, color: Hsla, theme: &Theme) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .text_size(text_size(11.5))
                .text_color(theme.text_muted)
                .child(div().size(px(6.0)).rounded_full().bg(color))
                .child(label),
        )
        .child(
            div()
                .text_size(text_size(15.0))
                .line_height(px(20.0))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme.text)
                .child(format_rate(bytes_per_sec)),
        )
}

/// Two columns of per-core meters, numbered from 1.
fn core_grid(cores: &[f32], theme: &Theme) -> gpui::Div {
    let cell = |ix: usize, percent: f32| {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .h(px(20.0))
            .text_size(text_size(11.5))
            .child(
                div()
                    .w(px(16.0))
                    .flex_none()
                    .text_right()
                    .text_color(theme.text_muted)
                    .child(format!("{}", ix + 1)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(meter(theme, percent / 100.0)),
            )
            .child(
                div()
                    .w(px(34.0))
                    .flex_none()
                    .text_right()
                    .text_color(
                        if usage_level(percent / 100.0)
                            == crate::settings::accounts::UsageLevel::Normal
                        {
                            theme.text_muted
                        } else {
                            level_color(theme, percent)
                        },
                    )
                    .child(format_percent(percent)),
            )
    };
    let mut grid = div().flex().flex_row().gap(px(16.0));
    let half = cores.len().div_ceil(2);
    for (column_ix, column) in cores.chunks(half.max(1)).enumerate() {
        let first = column_ix * half;
        grid = grid.child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .children(
                    column
                        .iter()
                        .enumerate()
                        .map(|(i, percent)| cell(first + i, *percent)),
                ),
        );
    }
    grid
}

impl Render for SystemMonitor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.last_render = Instant::now();
        let theme = Theme::of(cx).clone();
        let stats = self.stats.clone();

        let body = match (&stats, &self.link) {
            (None, Link::Unsupported) => centered_note(
                &theme,
                "This device's Zeron is too old to report resource usage. Update it to see live stats.",
            ),
            (None, Link::Lost(message)) => {
                centered_note(&theme, format!("Can't reach this device: {message}"))
            }
            (None, _) => centered_note(&theme, "Reading system stats…"),
            (Some(stats), link) => {
                let host = &stats.host;
                let mut line = Vec::new();
                if !host.name.is_empty() {
                    line.push(host.name.clone());
                }
                if !host.os.is_empty() {
                    line.push(host.os.clone());
                }
                line.push(format!("up {}", format_uptime(host.uptime_secs)));
                let stale = match link {
                    Link::Lost(_) => Some("Reconnecting…"),
                    _ => None,
                };
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .p(px(12.0))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .gap(px(12.0))
                            .px(px(4.0))
                            .text_size(text_size(11.5))
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(theme.text_muted)
                                    .child(SharedString::from(line.join(" · "))),
                            )
                            .when_some(stale, |el, note| {
                                el.child(div().flex_none().text_color(theme.warning).child(note))
                            }),
                    )
                    .child(self.cpu_card(stats, &theme, cx))
                    .child(self.memory_card(stats, &theme))
                    .child(self.gpu_card(&stats.gpus, &theme))
                    .child(self.network_card(stats, &theme))
                    .child(self.disk_card(stats, &theme))
                    .child(self.process_card(stats, &theme, cx))
                    .into_any_element()
            }
        };

        let scrollbar = popover::rail(self, "system-monitor-scrollbar", &theme, cx);
        div()
            .id("system-monitor-host")
            .relative()
            .size_full()
            .on_hover(cx.listener(Self::on_scroll_hovered))
            .child(
                crate::edge_fade::edge_faded(
                    16.0,
                    true,
                    true,
                    div()
                        .id("system-monitor")
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll.scroll)
                        .child(body),
                )
                .fade_overflow_y(&self.scroll.scroll),
            )
            .children(scrollbar)
    }
}

fn centered_note(theme: &Theme, copy: impl Into<SharedString>) -> gpui::AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .p(px(24.0))
        .text_center()
        .text_size(text_size(12.0))
        .line_height(px(17.0))
        .text_color(theme.text_muted.opacity(0.7))
        .child(copy.into())
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_uses_binary_and_disks_decimal_multiples() {
        assert_eq!(format_memory(512), "512 B");
        assert_eq!(format_memory(16 * 1024 * 1024 * 1024), "16.0 GB");
        assert_eq!(format_memory(25_769_803_776), "24.0 GB");
        assert_eq!(format_size(1_995_218_165_760), "2.0 TB");
        assert_eq!(format_size(999), "999 B");
        assert_eq!(format_size(1_500_000), "1.5 MB");
        assert_eq!(format_size(250_000_000_000), "250 GB");
    }

    #[test]
    fn rates_carry_a_per_second_suffix() {
        assert_eq!(format_rate(0), "0 B/s");
        assert_eq!(format_rate(3_649), "3.6 KB/s");
        assert_eq!(format_rate(14_589_582), "14.6 MB/s");
    }

    #[test]
    fn uptime_reads_at_the_largest_useful_unit() {
        assert_eq!(format_uptime(59), "0m");
        assert_eq!(format_uptime(12 * 60), "12m");
        assert_eq!(format_uptime(5 * 3600 + 12 * 60), "5h 12m");
        assert_eq!(format_uptime(3 * 86_400 + 23 * 3600 + 59 * 60), "3d 23h");
    }

    #[test]
    fn fraction_is_clamped_and_safe_on_zero() {
        assert_eq!(fraction(5, 0), 0.0);
        assert_eq!(fraction(5, 10), 0.5);
        assert_eq!(fraction(20, 10), 1.0);
    }

    #[test]
    fn history_is_capped_and_gpu_only_advances_with_a_gpu() {
        let mut history = History::default();
        let mut stats = SystemStats::default();
        for _ in 0..HISTORY_LEN + 25 {
            history.record(&stats);
        }
        assert_eq!(history.cpu.len(), HISTORY_LEN);
        assert_eq!(history.net_in.len(), HISTORY_LEN);
        assert!(history.gpu.is_empty(), "no GPU, no GPU samples");
        stats.gpus.push(GpuStats {
            utilization_percent: Some(40.0),
            ..Default::default()
        });
        history.record(&stats);
        assert_eq!(history.gpu.back(), Some(&40.0));
    }

    fn process(pid: u32, cpu: f32, memory: u64) -> ProcessStats {
        ProcessStats {
            pid,
            name: format!("p{pid}"),
            cpu_percent: cpu,
            memory_bytes: memory,
        }
    }

    #[test]
    fn process_sort_follows_the_chosen_column() {
        let all: Vec<_> = (0..12)
            .map(|pid| process(pid, pid as f32, 1000 - u64::from(pid)))
            .collect();
        let by_cpu = sorted_processes(&all, ProcessSort::Cpu);
        assert_eq!(by_cpu.len(), PROCESS_ROWS);
        assert_eq!(by_cpu[0].pid, 11);
        let by_memory = sorted_processes(&all, ProcessSort::Memory);
        assert_eq!(by_memory[0].pid, 0);
    }

    #[test]
    fn rate_scale_never_drops_below_the_floor() {
        let quiet: VecDeque<f32> = [10.0, 20.0].into();
        assert_eq!(rate_scale(&[&quiet]), RATE_FLOOR);
        let busy: VecDeque<f32> = [10.0, 5_000_000.0].into();
        assert_eq!(rate_scale(&[&quiet, &busy]), 5_000_000.0);
    }
}
