import SwiftUI
import WidgetKit

/// sessions that need the user, then running ones with their elapsed time.
struct SessionsWidget: Widget {
    var body: some WidgetConfiguration {
        StaticConfiguration(kind: WidgetStore.sessionsKind, provider: SnapshotProvider()) { entry in
            SessionsWidgetView(entry: entry)
                .zeronWidgetBackground()
        }
        .configurationDisplayName("Sessions")
        .description("Sessions waiting on you, and runs in progress with how long they've been going.")
        .supportedFamilies([
            .systemSmall, .systemMedium, .systemLarge,
            .accessoryCircular, .accessoryRectangular, .accessoryInline,
        ])
    }
}

private struct SessionsWidgetView: View {
    @Environment(\.widgetFamily) private var family
    let entry: SnapshotEntry

    var body: some View {
        if let snapshot = entry.snapshot {
            let asOf = entry.asOf

            switch family {
            case .systemSmall: SmallSessions(snapshot: snapshot, asOf: asOf)
            case .systemMedium: ListSessions(snapshot: snapshot, asOf: asOf, limit: 2, compact: true)
            case .systemLarge: ListSessions(snapshot: snapshot, asOf: asOf, limit: 6)
            case .accessoryCircular: CircularSessions(snapshot: snapshot)
            case .accessoryRectangular: RectangularSessions(snapshot: snapshot, asOf: asOf)
            case .accessoryInline: Text(asOf.map { "\(snapshot.summary) · as of \($0.stamp)" } ?? snapshot.summary)
            default: ListSessions(snapshot: snapshot, asOf: asOf, limit: 2, compact: true)
            }
        } else {
            SignedOut(compact: family.isAccessory)
        }
    }
}

private struct SmallSessions: View {
    let snapshot: WidgetSnapshot
    let asOf: Date?

    var body: some View {
        let top = snapshot.sessions.first

        VStack(alignment: .leading, spacing: 4) {
            Headline(snapshot: snapshot, asOf: asOf)

            Spacer(minLength: 6)

            if let top {
                HStack(spacing: 5) {
                    StatusIcon(status: top.status)
                    SessionTime(session: top, asOf: asOf)
                        .font(Face.mono(11))
                        .foregroundStyle(.secondary)
                }

                Text(top.title)
                    .font(Face.sans(13))
                    .lineLimit(2)
                    .privacySensitive()

                Text(top.detail)
                    .font(Face.sans(11))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .privacySensitive()
            } else {
                Text("No sessions running")
                    .font(Face.sans(13))
                    .foregroundStyle(.secondary)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .widgetURL(top.map { ZeronLink.chat($0.id) } ?? ZeronLink.sessions)
    }
}

private struct ListSessions: View {
    let snapshot: WidgetSnapshot
    let asOf: Date?
    let limit: Int
    var compact = false

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .center) {
                Headline(snapshot: snapshot, asOf: asOf, compact: compact)
                Spacer()
                NewSessionButton()
            }
            .fixedSize(horizontal: false, vertical: true)

            if snapshot.sessions.isEmpty {
                Spacer()
                Text("No sessions running")
                    .font(Face.sans(13))
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity)
                Spacer()
            } else {
                sessionRows
                    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .widgetURL(ZeronLink.sessions)
    }

    private var sessionRows: some View {
        let sessions = Array(snapshot.sessions.prefix(limit))
        let remaining = snapshot.needsYou + snapshot.working + snapshot.done - sessions.count

        return VStack(alignment: .leading, spacing: compact ? 6 : 8) {
            ForEach(sessions) { session in
                Link(destination: ZeronLink.chat(session.id)) {
                    SessionLine(session: session, asOf: asOf)
                }
            }

            if remaining > 0 {
                Link(destination: ZeronLink.sessions) {
                    Text("\(remaining) more \(remaining == 1 ? "session" : "sessions")")
                        .font(Face.sans(11))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
            }
        }
        .fixedSize(horizontal: false, vertical: true)
    }
}

private struct CircularSessions: View {
    let snapshot: WidgetSnapshot

    var body: some View {
        let needsYou = snapshot.needsYou > 0

        ZStack {
            AccessoryWidgetBackground()

            VStack(spacing: 0) {
                Image(systemName: needsYou ? "hand.raised.fill" : "circle.dotted")
                    .font(.system(size: 12, weight: .semibold))
                Text("\(needsYou ? snapshot.needsYou : snapshot.working)")
                    .font(Face.mono(18))
            }
        }
        .widgetURL(snapshot.sessions.first.map { ZeronLink.chat($0.id) } ?? ZeronLink.sessions)
        .accessibilityLabel(snapshot.summary)
    }
}

private struct RectangularSessions: View {
    let snapshot: WidgetSnapshot
    let asOf: Date?

    var body: some View {
        let top = snapshot.sessions.first

        VStack(alignment: .leading, spacing: 1) {
            HStack(spacing: 4) {
                Text(snapshot.summary)
                Spacer(minLength: 2)
                if let asOf {
                    Text(asOf.stamp)
                } else if let top {
                    SessionTime(session: top, asOf: nil)
                }
            }
            .font(Face.sans(13))
            .lineLimit(1)
            .widgetAccentable()

            if let top {
                Text(top.title)
                    .font(Face.sans(13))
                    .lineLimit(1)
                    .privacySensitive()
                Text(top.detail)
                    .font(Face.sans(11))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .privacySensitive()
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .widgetURL(top.map { ZeronLink.chat($0.id) } ?? ZeronLink.sessions)
    }
}

/// the count that matters most right now: what's waiting on the user, else
/// what's running. a stale snapshot trades the secondary count for when it
/// was true.
private struct Headline: View {
    let snapshot: WidgetSnapshot
    let asOf: Date?
    var compact = false

    var body: some View {
        Group {
            if compact {
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    primary
                    secondary
                }
            } else {
                VStack(alignment: .leading, spacing: 0) {
                    primary
                    secondary
                }
            }
        }
        .lineLimit(1)
    }

    private var primary: some View {
        Text(count.map { String($0) } ?? "All clear")
            .font(count == nil ? Face.sans(17) : Face.mono(compact ? 22 : 26))
            .foregroundStyle(count == nil ? Color.primary : snapshot.needsYou > 0 ? Tone.warning : Tone.accent)
    }

    private var secondary: some View {
        Text(caption(label, extra: snapshot.needsYou > 0 && snapshot.working > 0 ? "\(snapshot.working) working" : nil))
            .font(Face.sans(12))
            .foregroundStyle(.secondary)
    }

    private var count: Int? {
        if snapshot.needsYou > 0 { return snapshot.needsYou }
        return snapshot.working > 0 ? snapshot.working : nil
    }

    private var label: String {
        if snapshot.needsYou > 0 { return "Need you" }
        if snapshot.working > 0 { return "Working" }
        return snapshot.done > 0 ? "\(snapshot.done) finished" : "Nothing running"
    }

    private func caption(_ label: String, extra: String? = nil) -> String {
        let tail = asOf.map { "as of \($0.stamp)" } ?? extra
        return [label, tail].compactMap { $0 }.joined(separator: " · ")
    }
}

#if DEBUG
#Preview("Small", as: .systemSmall) {
    SessionsWidget()
} timeline: {
    SnapshotEntry(date: .now, snapshot: .sample)
    SnapshotEntry(date: .now.addingTimeInterval(WidgetSnapshot.staleAfter), snapshot: .sample)
    SnapshotEntry(date: .now, snapshot: .empty)
    SnapshotEntry(date: .now, snapshot: nil)
}

#Preview("Medium", as: .systemMedium) {
    SessionsWidget()
} timeline: {
    SnapshotEntry(date: .now, snapshot: .sample)
}

#Preview("Rectangular", as: .accessoryRectangular) {
    SessionsWidget()
} timeline: {
    SnapshotEntry(date: .now, snapshot: .sample)
}
#endif
