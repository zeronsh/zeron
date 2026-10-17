import SwiftUI
import WidgetKit

struct SnapshotEntry: TimelineEntry {
    let date: Date
    /// nil when signed out.
    let snapshot: WidgetSnapshot?

    /// when the app last published, once that's too long ago to trust live
    /// state (running timers, online hosts); nil while fresh.
    var asOf: Date? {
        guard let snapshot, date >= snapshot.staleAt else { return nil }
        return snapshot.publishedAt
    }
}

/// every widget reads the app's last published snapshot. the app reloads the
/// timelines whenever it changes; the only scheduled entry is the one that
/// marks the snapshot stale if the app doesn't publish again in time.
struct SnapshotProvider: TimelineProvider {
    func placeholder(in context: Context) -> SnapshotEntry {
        SnapshotEntry(date: .now, snapshot: .sample)
    }

    func getSnapshot(in context: Context, completion: @escaping (SnapshotEntry) -> Void) {
        let snapshot = WidgetStore.load()
        completion(SnapshotEntry(date: .now, snapshot: context.isPreview && snapshot == nil ? .sample : snapshot))
    }

    func getTimeline(in context: Context, completion: @escaping (Timeline<SnapshotEntry>) -> Void) {
        let snapshot = WidgetStore.load()
        var entries = [SnapshotEntry(date: .now, snapshot: snapshot)]
        if let snapshot, snapshot.staleAt > .now {
            entries.append(SnapshotEntry(date: snapshot.staleAt, snapshot: snapshot))
        }
        completion(Timeline(entries: entries, policy: .never))
    }
}

extension WidgetSnapshot {
    static let sample = WidgetSnapshot(
        sessions: [
            Session(id: "a", title: "Add home screen widgets", place: "zeron", agent: "Opus", status: .awaiting, since: .now.addingTimeInterval(-240)),
            Session(id: "b", title: "Fix Cursor model defaults", place: "zeron", agent: "GPT-5.5", status: .working, since: .now.addingTimeInterval(-410)),
            Session(id: "c", title: "Release notes for v0.2.113", place: "MacBook Pro", agent: "Sonnet", status: .done, since: .now.addingTimeInterval(-1800)),
        ],
        devices: [
            Device(id: "m", name: "MacBook Pro", platform: "macos", online: true, sessions: 4),
            Device(id: "f", name: "Fedora", platform: "linux", online: false, sessions: 1, lastSeen: .now.addingTimeInterval(-7200)),
        ],
        usage: [
            UsageAccount(
                id: "m/claude", source: "claude-code", agent: "Claude Code", account: "you@example.com", plan: "Max 20×", host: "MacBook Pro", active: true,
                windows: [
                    UsageWindow(label: "Session", used: 0.42, resetsAt: .now.addingTimeInterval(2 * 3600)),
                    UsageWindow(label: "Week", used: 0.78, resetsAt: .now.addingTimeInterval(3 * 86400)),
                ],
                fetchedAt: .now
            ),
            UsageAccount(
                id: "m/codex", source: "codex", agent: "Codex", account: "you@example.com", plan: "ChatGPT Pro", host: "MacBook Pro", active: true,
                windows: [
                    UsageWindow(label: "Session", used: 0.12, resetsAt: .now.addingTimeInterval(4 * 3600)),
                    UsageWindow(label: "Week", used: 0.93, resetsAt: .now.addingTimeInterval(86400)),
                ],
                fetchedAt: .now.addingTimeInterval(-7200)
            ),
            UsageAccount(
                id: "m/pi-claude", source: "pi/anthropic", agent: "Pi · Claude", account: "old@example.com", plan: "Claude Pro", host: "MacBook Pro", active: true,
                windows: [],
                error: "Session expired — it refreshes the next time pi runs"
            ),
        ],
        voiceAvailable: true,
        awaiting: 1,
        failed: 0,
        working: 2,
        done: 1,
        publishedAt: .now
    )

    var summary: String {
        var parts: [String] = []
        if needsYou > 0 { parts.append("\(needsYou) need you") }
        if working > 0 { parts.append("\(working) working") }
        return parts.isEmpty ? "All clear" : parts.joined(separator: " · ")
    }
}

extension Date {
    /// with the weekday once it isn't today, so an old snapshot doesn't read
    /// as this afternoon's.
    var stamp: String {
        Calendar.current.isDateInToday(self)
            ? formatted(date: .omitted, time: .shortened)
            : formatted(.dateTime.weekday(.abbreviated).hour().minute())
    }
}

extension WidgetFamily {
    var isAccessory: Bool {
        self == .accessoryCircular || self == .accessoryRectangular || self == .accessoryInline
    }
}

struct SignedOut: View {
    let compact: Bool

    var body: some View {
        if compact {
            Text("Sign in to Zeron")
                .font(Face.sans(12))
        } else {
            VStack(spacing: 4) {
                Text("Zeron")
                    .font(Face.sans(15))
                Text("Sign in to see your sessions")
                    .font(Face.sans(12))
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
            }
            .widgetURL(ZeronLink.sessions)
        }
    }
}

extension View {
    func zeronWidgetBackground() -> some View {
        containerBackground(for: .widget) { Color(uiColor: .systemBackground) }
    }
}
