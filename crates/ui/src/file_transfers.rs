//! Device file transfers (docs/file-transfer.md): the pure presentation layer
//! behind the desktop's "Send to device…" picker, the titlebar transfers
//! indicator and panel, and the incoming-transfer toast. Rendering lives in
//! `shell/file_transfers.rs`; everything here is data in, strings/flags out.

use std::collections::HashSet;

use zeron_proto::{
    Device, FileTransfer, FileTransferDirection as Direction, FileTransferItemKind as ItemKind,
    FileTransferState as State, FileTransferTransport as Transport,
};

/// A finished transfer keeps the titlebar indicator up this long, so a
/// transfer that completes while the user looks elsewhere is still findable.
pub const RECENT_WINDOW_MS: i64 = 15 * 60 * 1000;

/// A finished transfer's toast lingers this long before it hides itself.
pub const TOAST_LINGER_MS: i64 = 8_000;

/// One row of the transfers panel: a transfer plus the engine that holds it
/// (`None` = this desktop's own engine; `Some(device)` = a remote host whose
/// feed is watched because the user sent from one of its projects).
#[derive(Debug, Clone, PartialEq)]
pub struct TransferRow {
    pub host: Option<String>,
    pub transfer: FileTransfer,
}

/// Merge the local feed with remote hosts' feeds. A transfer between this
/// desktop and a watched host appears in both (same id); the local row wins,
/// since its paths are the ones this machine can open. Newest first.
pub fn merge_feeds(
    local: &[FileTransfer],
    remote: &[(String, Vec<FileTransfer>)],
) -> Vec<TransferRow> {
    let mut seen: HashSet<&str> = local.iter().map(|t| t.id.as_str()).collect();
    let mut rows: Vec<TransferRow> = local
        .iter()
        .map(|transfer| TransferRow {
            host: None,
            transfer: transfer.clone(),
        })
        .collect();
    for (host, feed) in remote {
        for transfer in feed {
            if seen.insert(transfer.id.as_str()) {
                rows.push(TransferRow {
                    host: Some(host.clone()),
                    transfer: transfer.clone(),
                });
            }
        }
    }
    rows.sort_by(|a, b| {
        b.transfer
            .created_at
            .cmp(&a.transfer.created_at)
            .then_with(|| a.transfer.id.cmp(&b.transfer.id))
    });
    rows
}

/// `912 B`, `1.2 KB`, `34 MB`, `1.25 GB` (decimal units, like file managers).
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["KB", "MB", "GB", "TB", "PB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1000.0;
    let mut unit = 0;
    while value >= 1000.0 && unit + 1 < UNITS.len() {
        value /= 1000.0;
        unit += 1;
    }
    let digits = if value >= 100.0 {
        0
    } else if value >= 10.0 || unit == 0 {
        1
    } else {
        2
    };
    let text = format!("{value:.digits$}");
    // "34.0 MB" reads as noise; trim trailing zeros of the fraction.
    let text = if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        text
    };
    format!("{text} {}", UNITS[unit])
}

pub fn format_rate(bytes_per_sec: u64) -> String {
    format!("{}/s", format_bytes(bytes_per_sec))
}

/// Rough time left at the current rate; `None` while idle.
pub fn format_eta(remaining: u64, bytes_per_sec: u64) -> Option<String> {
    if bytes_per_sec == 0 || remaining == 0 {
        return None;
    }
    let secs = remaining.div_ceil(bytes_per_sec);
    Some(if secs < 60 {
        format!("{secs} s left")
    } else if secs < 3600 {
        format!("{} min left", secs.div_ceil(60))
    } else {
        let hours = secs / 3600;
        let minutes = (secs % 3600) / 60;
        if minutes == 0 {
            format!("{hours} h left")
        } else {
            format!("{hours} h {minutes} min left")
        }
    })
}

/// What is being moved: the single item's name, or a count.
pub fn title(transfer: &FileTransfer) -> String {
    match transfer.items.as_slice() {
        [] => match transfer.file_count {
            1 => "1 file".into(),
            n => format!("{n} files"),
        },
        [item] => item.name.clone(),
        items => format!("{} items", items.len()),
    }
}

fn size_summary(transfer: &FileTransfer) -> String {
    let bytes = format_bytes(transfer.total_bytes);
    let folders = transfer
        .items
        .iter()
        .any(|item| item.kind == ItemKind::Folder);
    if folders || transfer.items.len() > 1 {
        let files = match transfer.file_count {
            1 => "1 file".to_string(),
            n => format!("{n} files"),
        };
        format!("{files}, {bytes}")
    } else {
        bytes
    }
}

/// One sentence naming what moves where: the toast's title line.
pub fn headline(transfer: &FileTransfer) -> String {
    let what = title(transfer);
    let peer = &transfer.peer_device_name;
    match (transfer.direction, transfer.state) {
        (Direction::Incoming, State::AwaitingAcceptance) => {
            format!("{peer} wants to send {what}")
        }
        (Direction::Incoming, State::Completed) => format!("Received {what} from {peer}"),
        (Direction::Outgoing, State::Completed) => format!("Sent {what} to {peer}"),
        (Direction::Incoming, _) => format!("Receiving {what} from {peer}"),
        (Direction::Outgoing, _) => format!("Sending {what} to {peer}"),
    }
}

/// The toast's detail line: like [`status_line`], minus what its headline
/// already says.
pub fn toast_detail(transfer: &FileTransfer) -> String {
    match (transfer.direction, transfer.state) {
        (Direction::Incoming, State::AwaitingAcceptance) => size_summary(transfer),
        _ => status_line(transfer),
    }
}

/// The row's detail line: progress while live, the outcome once finished.
pub fn status_line(transfer: &FileTransfer) -> String {
    let peer = &transfer.peer_device_name;
    let progress = || {
        format!(
            "{} of {}",
            format_bytes(transfer.done_bytes.min(transfer.total_bytes)),
            format_bytes(transfer.total_bytes)
        )
    };
    match transfer.state {
        State::Preparing => "Preparing…".into(),
        State::Connecting => format!("Connecting to {peer}…"),
        State::AwaitingAcceptance => match transfer.direction {
            Direction::Incoming => format!("{} · waiting for you", size_summary(transfer)),
            Direction::Outgoing => format!("Waiting for {peer} to accept"),
        },
        State::Transferring => {
            let mut parts = vec![progress()];
            if transfer.bytes_per_sec > 0 {
                parts.push(format_rate(transfer.bytes_per_sec));
            }
            if let Some(eta) = format_eta(
                transfer.total_bytes.saturating_sub(transfer.done_bytes),
                transfer.bytes_per_sec,
            ) {
                parts.push(eta);
            }
            parts.join(" · ")
        }
        State::Verifying => "Verifying…".into(),
        State::Reconnecting => format!("Reconnecting · {}", progress()),
        State::Completed => {
            let mut line = size_summary(transfer);
            if transfer.skipped > 0 {
                line.push_str(&format!(" · {} skipped", transfer.skipped));
            }
            line
        }
        State::Failed => transfer
            .error
            .clone()
            .filter(|error| !error.trim().is_empty())
            .unwrap_or_else(|| "Failed".into()),
        State::Cancelled => "Cancelled".into(),
        State::Declined => match transfer.direction {
            Direction::Incoming => "Declined".into(),
            Direction::Outgoing => format!("{peer} declined"),
        },
    }
}

/// The peer line of a panel row: direction and device, plus the transport
/// while bytes move.
pub fn peer_line(transfer: &FileTransfer) -> String {
    let peer = &transfer.peer_device_name;
    let base = match transfer.direction {
        Direction::Incoming => format!("From {peer}"),
        Direction::Outgoing => format!("To {peer}"),
    };
    match (transfer.transport, shows_progress(transfer)) {
        (Some(Transport::P2p), true) => format!("{base} · direct"),
        (Some(Transport::Relay), true) => format!("{base} · relay"),
        _ => base,
    }
}

/// Whether the progress bar shows (live states that move bytes).
pub fn shows_progress(transfer: &FileTransfer) -> bool {
    matches!(
        transfer.state,
        State::Transferring | State::Verifying | State::Reconnecting
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowAction {
    Accept,
    Decline,
    Cancel,
    /// Reveal the received item (or its destination folder) on this machine.
    ShowInFolder,
    /// Open the single received file with the system handler.
    Open,
    /// Drop the finished row from the history.
    Dismiss,
}

/// The actions a row offers. Opening needs a path on THIS machine, so remote
/// hosts' rows never offer it.
pub fn actions(row: &TransferRow) -> Vec<RowAction> {
    let transfer = &row.transfer;
    match transfer.state {
        State::AwaitingAcceptance if transfer.direction == Direction::Incoming => {
            vec![RowAction::Decline, RowAction::Accept]
        }
        state if !state.is_terminal() => vec![RowAction::Cancel],
        State::Completed if row.host.is_none() && transfer.direction == Direction::Incoming => {
            let mut actions = Vec::new();
            if open_target(transfer).is_some() {
                actions.push(RowAction::Open);
            }
            if reveal_target(transfer).is_some() {
                actions.push(RowAction::ShowInFolder);
            }
            actions.push(RowAction::Dismiss);
            actions
        }
        _ => vec![RowAction::Dismiss],
    }
}

pub fn action_label(action: RowAction) -> &'static str {
    match action {
        RowAction::Accept => "Accept",
        RowAction::Decline => "Decline",
        RowAction::Cancel => "Cancel",
        RowAction::ShowInFolder => "Show in folder",
        RowAction::Open => "Open",
        RowAction::Dismiss => "Remove",
    }
}

/// What "Show in folder" reveals: the one received item, or the destination
/// folder that holds several.
pub fn reveal_target(transfer: &FileTransfer) -> Option<String> {
    match transfer.items.as_slice() {
        [item] => item.path.clone().or_else(|| transfer.destination.clone()),
        _ => transfer.destination.clone(),
    }
}

/// What "Open" opens: a single received file (folders are revealed instead).
pub fn open_target(transfer: &FileTransfer) -> Option<String> {
    match transfer.items.as_slice() {
        [item] if item.kind == ItemKind::File => item.path.clone(),
        _ => None,
    }
}

/// The titlebar indicator's summary; `None` hides it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IndicatorSummary {
    pub live: usize,
    /// Incoming transfers waiting for this user's decision.
    pub awaiting: usize,
    /// Aggregate progress of live transfers that move bytes.
    pub fraction: Option<f32>,
    /// The newest recently finished transfer failed.
    pub failed: bool,
}

pub fn indicator(rows: &[TransferRow], now_ms: i64) -> Option<IndicatorSummary> {
    let live: Vec<&FileTransfer> = rows
        .iter()
        .map(|row| &row.transfer)
        .filter(|t| t.is_live())
        .collect();
    let recent = rows.iter().map(|row| &row.transfer).find(|t| {
        !t.is_live() && now_ms - t.finished_at.unwrap_or(t.updated_at) <= RECENT_WINDOW_MS
    });
    if live.is_empty() && recent.is_none() {
        return None;
    }
    let awaiting = live
        .iter()
        .filter(|t| t.direction == Direction::Incoming && t.state == State::AwaitingAcceptance)
        .count();
    let (done, total) =
        live.iter()
            .filter(|t| shows_progress(t))
            .fold((0u64, 0u64), |(done, total), t| {
                (
                    done + t.done_bytes.min(t.total_bytes),
                    total + t.total_bytes,
                )
            });
    Some(IndicatorSummary {
        live: live.len(),
        awaiting,
        fraction: (total > 0).then(|| (done as f64 / total as f64) as f32),
        failed: live.is_empty() && recent.is_some_and(|t| matches!(t.state, State::Failed)),
    })
}

/// The indicator's tooltip / accessible label.
pub fn indicator_label(summary: &IndicatorSummary) -> String {
    if summary.awaiting > 0 {
        return match summary.awaiting {
            1 => "A device wants to send you files".into(),
            n => format!("{n} devices want to send you files"),
        };
    }
    match (summary.live, summary.fraction) {
        (0, _) if summary.failed => "File transfer failed".into(),
        (0, _) => "File transfers".into(),
        (n, Some(fraction)) => format!(
            "{} file transfer{} · {}%",
            n,
            if n == 1 { "" } else { "s" },
            (fraction * 100.0).floor() as u32
        ),
        (n, None) => format!("{n} file transfer{}", if n == 1 { "" } else { "s" }),
    }
}

/// Join a workspace-relative tree path onto the host's absolute workspace
/// root, in the host's own separator style. `None` when the root is not an
/// absolute host path (a projectless chat rooted at an unexpanded `~`).
pub fn host_path(root: &str, relative: &str) -> Option<String> {
    let root = root.trim();
    let bytes = root.as_bytes();
    let windows = root.starts_with("\\\\")
        || (bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'\\' | b'/'));
    if !(root.starts_with('/') || windows) {
        return None;
    }
    let separator = if windows && !root.contains('/') {
        '\\'
    } else {
        '/'
    };
    let relative = relative.trim_matches('/');
    if relative.is_empty() {
        return Some(root.to_string());
    }
    if relative
        .split('/')
        .any(|part| part.is_empty() || matches!(part, "." | ".."))
    {
        return None;
    }
    let mut path = root.trim_end_matches(['/', '\\']).to_string();
    path.push(separator);
    if separator == '\\' {
        path.push_str(&relative.replace('/', "\\"));
    } else {
        path.push_str(relative);
    }
    Some(path)
}

/// One row of the "Send to device…" picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendTarget {
    pub id: String,
    pub name: String,
    pub platform: String,
    pub online: bool,
    /// Runs an engine that speaks the transfer protocol.
    pub supported: bool,
}

impl SendTarget {
    pub fn selectable(&self) -> bool {
        self.online && self.supported
    }
}

/// The account's other engines a `host` can send to: everything but the
/// sender itself, viewer-only devices dropped, receivable and online first,
/// then by name.
pub fn send_targets(
    devices: &[Device],
    host: &str,
    online: impl Fn(&Device) -> bool,
) -> Vec<SendTarget> {
    let mut targets: Vec<SendTarget> = devices
        .iter()
        .filter(|device| device.id != host)
        // Viewer-only rows (phones without an engine, the web app) have
        // nothing that could receive.
        .filter(|device| device.is_execution_host() && device.platform != "web")
        .map(|device| SendTarget {
            id: device.id.clone(),
            name: device.name.clone(),
            platform: device.platform.clone(),
            online: online(device),
            supported: device.supports(zeron_proto::capabilities::FILE_TRANSFER_V1),
        })
        .collect();
    targets.sort_by_key(|target| {
        (
            !target.selectable(),
            target.name.to_lowercase(),
            target.id.clone(),
        )
    });
    targets
}

/// The Zeron glyph for a device's platform.
pub fn platform_icon(platform: &str) -> &'static str {
    match platform {
        "macos" | "darwin" => crate::icons::LAPTOP,
        "ios" | "android" => crate::icons::SMARTPHONE,
        "linux" | "windows" => crate::icons::MONITOR,
        _ => crate::icons::REMOTE_SERVER,
    }
}

/// What the shell should announce about the incoming feed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IncomingNotice {
    /// A transfer from another device started (or already finished) arriving.
    Arrived(String),
    /// A transfer waits for this user to accept or decline it.
    NeedsAcceptance(String),
}

/// Edge detector over successive feed snapshots. The first snapshot primes it
/// silently (boot, reconnect) except for transfers still waiting on a
/// decision — those deserve a prompt whenever the app starts.
#[derive(Debug, Default)]
pub struct IncomingNotices {
    seen: HashSet<String>,
    asked: HashSet<String>,
    primed: bool,
}

impl IncomingNotices {
    pub fn observe(&mut self, rows: &[FileTransfer]) -> Vec<IncomingNotice> {
        let mut notices = Vec::new();
        let mut present = HashSet::new();
        // Oldest first, so several arrivals announce in order.
        for transfer in rows
            .iter()
            .rev()
            .filter(|t| t.direction == Direction::Incoming)
        {
            present.insert(transfer.id.clone());
            let new = self.seen.insert(transfer.id.clone());
            if transfer.state == State::AwaitingAcceptance {
                if self.asked.insert(transfer.id.clone()) {
                    notices.push(IncomingNotice::NeedsAcceptance(transfer.id.clone()));
                }
            } else if new && self.primed {
                notices.push(IncomingNotice::Arrived(transfer.id.clone()));
            }
        }
        self.seen.retain(|id| present.contains(id));
        self.asked.retain(|id| present.contains(id));
        self.primed = true;
        notices
    }

    /// A new runtime starts a fresh feed: prime again.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Rows grouped by the engine that holds them, for "Clear finished".
pub fn hosts_with_finished(rows: &[TransferRow]) -> Vec<Option<String>> {
    let mut hosts = Vec::new();
    for row in rows.iter().filter(|row| !row.transfer.is_live()) {
        if !hosts.contains(&row.host) {
            hosts.push(row.host.clone());
        }
    }
    hosts
}

#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;
    use zeron_proto::FileTransferItem;

    pub fn transfer(id: &str, direction: Direction, state: State) -> FileTransfer {
        FileTransfer {
            id: id.into(),
            direction,
            peer_device_id: "phone".into(),
            peer_device_name: "Pixel 8".into(),
            state,
            transport: Some(Transport::P2p),
            items: vec![FileTransferItem {
                name: "app-release.apk".into(),
                kind: ItemKind::File,
                size: 48_300_000,
                file_count: 1,
                path: Some("/home/me/Zeron Transfers/Pixel 8/app-release.apk".into()),
            }],
            file_count: 1,
            total_bytes: 48_300_000,
            done_bytes: 0,
            bytes_per_sec: 0,
            destination: Some("/home/me/Zeron Transfers/Pixel 8".into()),
            skipped: 0,
            error: None,
            created_at: 1_000,
            updated_at: 1_000,
            finished_at: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::transfer;
    use super::*;
    use zeron_proto::FileTransferItem;

    #[test]
    fn bytes_and_rates_read_like_a_file_manager() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(999), "999 B");
        assert_eq!(format_bytes(1_000), "1 KB");
        assert_eq!(format_bytes(1_234), "1.2 KB");
        assert_eq!(format_bytes(4_830_000), "4.83 MB");
        assert_eq!(format_bytes(48_300_000), "48.3 MB");
        assert_eq!(format_bytes(340_000_000), "340 MB");
        assert_eq!(format_bytes(1_250_000_000), "1.25 GB");
        assert_eq!(format_rate(12_300_000), "12.3 MB/s");
        assert_eq!(format_eta(0, 10), None);
        assert_eq!(format_eta(100, 0), None);
        assert_eq!(
            format_eta(30_000_000, 1_000_000).as_deref(),
            Some("30 s left")
        );
        assert_eq!(
            format_eta(90_000_000, 1_000_000).as_deref(),
            Some("2 min left")
        );
        assert_eq!(
            format_eta(3_900_000_000, 1_000_000).as_deref(),
            Some("1 h 5 min left")
        );
    }

    #[test]
    fn lines_describe_each_state_from_both_sides() {
        let mut t = transfer("a", Direction::Incoming, State::AwaitingAcceptance);
        assert_eq!(headline(&t), "Pixel 8 wants to send app-release.apk");
        assert_eq!(status_line(&t), "48.3 MB · waiting for you");
        assert_eq!(toast_detail(&t), "48.3 MB");
        // Nothing moves yet, so no transport is named.
        assert_eq!(peer_line(&t), "From Pixel 8");
        t.state = State::Transferring;
        t.done_bytes = 12_000_000;
        t.bytes_per_sec = 6_000_000;
        assert_eq!(headline(&t), "Receiving app-release.apk from Pixel 8");
        assert_eq!(status_line(&t), "12 MB of 48.3 MB · 6 MB/s · 7 s left");
        assert_eq!(peer_line(&t), "From Pixel 8 · direct");
        assert!(shows_progress(&t));
        t.state = State::Completed;
        assert_eq!(headline(&t), "Received app-release.apk from Pixel 8");
        assert_eq!(peer_line(&t), "From Pixel 8");
        assert!(!shows_progress(&t));

        let mut out = transfer("b", Direction::Outgoing, State::Declined);
        assert_eq!(status_line(&out), "Pixel 8 declined");
        out.state = State::Failed;
        out.error = Some("Pixel 8 is out of space".into());
        assert_eq!(status_line(&out), "Pixel 8 is out of space");
        out.error = None;
        assert_eq!(status_line(&out), "Failed");
        out.state = State::AwaitingAcceptance;
        assert_eq!(status_line(&out), "Waiting for Pixel 8 to accept");

        let mut folder = transfer("c", Direction::Outgoing, State::Completed);
        folder.items[0].kind = ItemKind::Folder;
        folder.items[0].name = "build".into();
        folder.file_count = 12;
        folder.skipped = 1;
        assert_eq!(status_line(&folder), "12 files, 48.3 MB · 1 skipped");
        folder.items.push(FileTransferItem {
            name: "notes.md".into(),
            kind: ItemKind::File,
            size: 10,
            file_count: 1,
            path: None,
        });
        assert_eq!(title(&folder), "2 items");
    }

    #[test]
    fn actions_follow_state_direction_and_host() {
        let row = |transfer, host: Option<&str>| TransferRow {
            host: host.map(str::to_owned),
            transfer,
        };
        let awaiting = row(
            transfer("a", Direction::Incoming, State::AwaitingAcceptance),
            None,
        );
        assert_eq!(actions(&awaiting), [RowAction::Decline, RowAction::Accept]);
        let outgoing_wait = row(
            transfer("a", Direction::Outgoing, State::AwaitingAcceptance),
            None,
        );
        assert_eq!(actions(&outgoing_wait), [RowAction::Cancel]);
        let live = row(
            transfer("a", Direction::Incoming, State::Reconnecting),
            None,
        );
        assert_eq!(actions(&live), [RowAction::Cancel]);
        let done = row(transfer("a", Direction::Incoming, State::Completed), None);
        assert_eq!(
            actions(&done),
            [RowAction::Open, RowAction::ShowInFolder, RowAction::Dismiss]
        );
        // A remote host's paths are not openable here.
        let remote = row(
            transfer("a", Direction::Incoming, State::Completed),
            Some("vps"),
        );
        assert_eq!(actions(&remote), [RowAction::Dismiss]);
        let sent = row(transfer("a", Direction::Outgoing, State::Completed), None);
        assert_eq!(actions(&sent), [RowAction::Dismiss]);

        let mut folder = transfer("f", Direction::Incoming, State::Completed);
        folder.items[0].kind = ItemKind::Folder;
        assert_eq!(open_target(&folder), None);
        assert_eq!(
            actions(&row(folder, None)),
            [RowAction::ShowInFolder, RowAction::Dismiss]
        );
    }

    #[test]
    fn indicator_tracks_live_and_recent_transfers() {
        let now = 10 * RECENT_WINDOW_MS;
        assert_eq!(indicator(&[], now), None);
        let row = |transfer| TransferRow {
            host: None,
            transfer,
        };
        let mut old = transfer("old", Direction::Incoming, State::Completed);
        old.finished_at = Some(now - RECENT_WINDOW_MS - 1);
        assert_eq!(indicator(&[row(old.clone())], now), None);

        let mut recent_fail = transfer("f", Direction::Outgoing, State::Failed);
        recent_fail.finished_at = Some(now - 1_000);
        let summary = indicator(&[row(recent_fail.clone()), row(old.clone())], now).unwrap();
        assert!(summary.failed);
        assert_eq!(summary.live, 0);
        assert_eq!(indicator_label(&summary), "File transfer failed");

        let mut a = transfer("a", Direction::Incoming, State::Transferring);
        a.total_bytes = 100;
        a.done_bytes = 50;
        let mut b = transfer("b", Direction::Outgoing, State::Transferring);
        b.total_bytes = 300;
        b.done_bytes = 50;
        let asking = transfer("c", Direction::Incoming, State::AwaitingAcceptance);
        let summary = indicator(&[row(a.clone()), row(b.clone()), row(recent_fail)], now).unwrap();
        assert_eq!(summary.live, 2);
        assert!(!summary.failed);
        assert_eq!(summary.fraction, Some(0.25));
        assert_eq!(indicator_label(&summary), "2 file transfers · 25%");
        let summary = indicator(&[row(a), row(asking)], now).unwrap();
        assert_eq!(summary.awaiting, 1);
        assert_eq!(
            indicator_label(&summary),
            "A device wants to send you files"
        );
    }

    #[test]
    fn merged_feeds_prefer_local_rows_and_sort_newest_first() {
        let mut local = transfer("shared", Direction::Incoming, State::Transferring);
        local.created_at = 5;
        let mut remote_copy = local.clone();
        remote_copy.direction = Direction::Outgoing;
        let mut other = transfer("other", Direction::Outgoing, State::Completed);
        other.created_at = 9;
        let rows = merge_feeds(
            std::slice::from_ref(&local),
            &[("vps".into(), vec![remote_copy, other])],
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].transfer.id, "other");
        assert_eq!(rows[0].host.as_deref(), Some("vps"));
        assert_eq!(rows[1].transfer.id, "shared");
        assert_eq!(rows[1].host, None);
        assert_eq!(rows[1].transfer.direction, Direction::Incoming);
        assert_eq!(hosts_with_finished(&rows), [Some("vps".to_string())]);
    }

    #[test]
    fn host_paths_join_in_the_host_style_and_refuse_escapes() {
        assert_eq!(
            host_path("/home/me/app", "build/app.apk").as_deref(),
            Some("/home/me/app/build/app.apk")
        );
        assert_eq!(
            host_path("/home/me/app/", "").as_deref(),
            Some("/home/me/app/")
        );
        assert_eq!(host_path("/", "etc").as_deref(), Some("/etc"));
        assert_eq!(
            host_path("C:\\Users\\me\\app", "src/main.rs").as_deref(),
            Some("C:\\Users\\me\\app\\src\\main.rs")
        );
        assert_eq!(
            host_path("C:/Users/me/app", "src/main.rs").as_deref(),
            Some("C:/Users/me/app/src/main.rs")
        );
        assert_eq!(host_path("~", "notes.md"), None);
        assert_eq!(host_path("relative", "notes.md"), None);
        assert_eq!(host_path("/home/me/app", "../secret"), None);
        assert_eq!(host_path("/home/me/app", "a//b"), None);
    }

    #[test]
    fn send_targets_exclude_the_sender_and_viewers_and_rank_receivers_first() {
        let device = |id: &str, name: &str, caps: &[&str]| -> Device {
            serde_json::from_value(serde_json::json!({
                "id": id, "name": name,
            "platform": match id { "viewer" => "web", "pocket" => "ios", _ => "linux" },
            "lastSeenAt": null,
                "capabilities": caps,
            }))
            .unwrap()
        };
        let ft = zeron_proto::capabilities::FILE_TRANSFER_V1;
        let devices = vec![
            device("host", "Desk", &[ft]),
            device("viewer", "Browser", &[]),
            device("pocket", "iPhone", &[]),
            device("old", "Old laptop", &["harness-updates-v1"]),
            device("phone", "Pixel", &[ft]),
            device("away", "Attic", &[ft]),
        ];
        let targets = send_targets(&devices, "host", |d| d.id != "away");
        let ids: Vec<_> = targets.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["phone", "away", "old"]);
        assert!(targets[0].selectable());
        assert!(!targets[1].selectable() && targets[1].supported);
        assert!(!targets[2].supported);
    }

    #[test]
    fn incoming_notices_prime_silently_but_always_ask() {
        let mut notices = IncomingNotices::default();
        let done = transfer("done", Direction::Incoming, State::Completed);
        let asking = transfer("ask", Direction::Incoming, State::AwaitingAcceptance);
        let outgoing = transfer("out", Direction::Outgoing, State::Transferring);
        assert_eq!(
            notices.observe(&[asking.clone(), done.clone(), outgoing.clone()]),
            [IncomingNotice::NeedsAcceptance("ask".into())]
        );
        // Same snapshot again: nothing new.
        assert!(notices.observe(&[asking.clone(), done.clone()]).is_empty());
        // A new arrival, and the asked one proceeding, announce once.
        let mut accepted = asking.clone();
        accepted.state = State::Transferring;
        let fresh = transfer("fresh", Direction::Incoming, State::Connecting);
        assert_eq!(
            notices.observe(&[fresh.clone(), accepted.clone(), done.clone()]),
            [IncomingNotice::Arrived("fresh".into())]
        );
        // A later confirmation request on a known row still asks.
        let mut asks_late = fresh.clone();
        asks_late.state = State::AwaitingAcceptance;
        assert_eq!(
            notices.observe(&[asks_late, accepted, done]),
            [IncomingNotice::NeedsAcceptance("fresh".into())]
        );
        notices.reset();
        assert!(
            notices
                .observe(&[transfer("x", Direction::Incoming, State::Transferring)])
                .is_empty()
        );
    }
}
