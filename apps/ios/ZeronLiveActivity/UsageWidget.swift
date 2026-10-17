import SwiftUI
import WidgetKit

/// agent plan limits (claude's session and week windows, codex's primary and
/// secondary, …) as the hosts last probed them.
struct UsageWidget: Widget {
    var body: some WidgetConfiguration {
        AppIntentConfiguration(kind: WidgetStore.usageKind, intent: UsageConfigurationIntent.self, provider: UsageProvider()) { entry in
            UsageWidgetView(entry: entry)
                .zeronWidgetBackground()
                .widgetURL(ZeronLink.sessions)
        }
        .configurationDisplayName("Usage Limits")
        .description("How much of each agent plan's limits you've used, and when they reset. Edit the widget to pick which agents it shows.")
        .supportedFamilies([.systemSmall, .systemMedium, .systemLarge, .accessoryCircular, .accessoryRectangular])
    }
}

/// unlike the other widgets this one schedules entries ahead: "resets in"
/// has to count down, and a window that has reset must stop reading as used,
/// even while the app (which would republish) isn't running.
struct UsageProvider: AppIntentTimelineProvider {
    private static let step: TimeInterval = 15 * 60
    private static let steps = 16

    func placeholder(in context: Context) -> UsageEntry {
        UsageEntry(date: .now, snapshot: .sample, sources: [])
    }

    func snapshot(for configuration: UsageConfigurationIntent, in context: Context) async -> UsageEntry {
        let snapshot = WidgetStore.load()
        return UsageEntry(date: .now, snapshot: context.isPreview && snapshot == nil ? .sample : snapshot, sources: configuration.sources)
    }

    func timeline(for configuration: UsageConfigurationIntent, in context: Context) async -> Timeline<UsageEntry> {
        let snapshot = WidgetStore.load()
        let start = Date.now
        let entries = (0..<Self.steps).map {
            UsageEntry(date: start.addingTimeInterval(Double($0) * Self.step), snapshot: snapshot, sources: configuration.sources)
        }
        return Timeline(entries: entries, policy: .atEnd)
    }
}

struct UsageEntry: TimelineEntry {
    let date: Date
    /// nil when signed out.
    let snapshot: WidgetSnapshot?
    /// the picked `UsageAccount.source`s; empty shows the automatic pick.
    let sources: [String]
}

private extension UsageConfigurationIntent {
    var sources: [String] { (agents ?? []).map(\.id) }
}

private struct UsageWidgetView: View {
    @Environment(\.widgetFamily) private var family
    let entry: UsageEntry

    var body: some View {
        if let snapshot = entry.snapshot {
            let accounts = snapshot.usageAccounts(showing: entry.sources)

            if accounts.isEmpty {
                NoUsage(compact: family.isAccessory)
            } else {
                switch family {
                case .systemSmall:
                    UsageCard(account: accounts[0], now: entry.date)
                case .systemMedium:
                    UsageGrid(accounts: Array(accounts.prefix(2)), columns: 2, now: entry.date)
                case .systemLarge:
                    UsageGrid(accounts: Array(accounts.prefix(4)), columns: 2, now: entry.date)
                case .accessoryCircular:
                    CircularUsage(account: accounts[0], now: entry.date)
                default:
                    RectangularUsage(account: accounts[0], now: entry.date)
                }
            }
        } else {
            SignedOut(compact: family.isAccessory)
        }
    }
}

private struct UsageGrid: View {
    let accounts: [WidgetSnapshot.UsageAccount]
    let columns: Int
    let now: Date

    var body: some View {
        let rows = stride(from: 0, to: accounts.count, by: columns).map { Array(accounts[$0..<min($0 + columns, accounts.count)]) }

        VStack(alignment: .leading, spacing: 14) {
            ForEach(rows.indices, id: \.self) { index in
                HStack(alignment: .top, spacing: 16) {
                    ForEach(rows[index]) { account in
                        UsageCard(account: account, now: now)
                    }

                    if rows[index].count < columns {
                        Spacer(minLength: 0)
                            .frame(maxWidth: .infinity)
                    }
                }

                if index < rows.count - 1 {
                    Divider()
                }
            }

            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }
}

private struct UsageCard: View {
    let account: WidgetSnapshot.UsageAccount
    let now: Date

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            VStack(alignment: .leading, spacing: 0) {
                Text(account.agent)
                    .font(Face.sans(14))
                    .lineLimit(1)
                Text([account.plan, account.host].compactMap { $0 }.joined(separator: " · "))
                    .font(Face.sans(11))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }

            if account.windows.isEmpty {
                Text(account.error ?? "Usage unavailable")
                    .font(Face.sans(11))
                    .foregroundStyle(.secondary)
                    .lineLimit(3)
            } else {
                ForEach(account.windows.prefix(2), id: \.label) { window in
                    WindowMeter(window: window, now: now)
                }
            }

            Spacer(minLength: 0)

            if let stale = account.staleLabel(now: now) {
                Text(stale)
                    .font(Face.sans(10))
                    .foregroundStyle(.tertiary)
                    .lineLimit(1)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }
}

private struct WindowMeter: View {
    let window: WidgetSnapshot.UsageWindow
    let now: Date

    var body: some View {
        let reset = window.hasReset(now: now)

        VStack(alignment: .leading, spacing: 3) {
            HStack(alignment: .firstTextBaseline) {
                Text(window.label)
                    .font(Face.sans(11))
                    .foregroundStyle(.secondary)
                Spacer(minLength: 4)
                Text(reset ? "Reset" : window.percent)
                    .font(Face.mono(12))
                    .foregroundStyle(reset ? AnyShapeStyle(.secondary) : AnyShapeStyle(window.tone))
            }

            UsageBar(fraction: reset ? 0 : window.used, tone: window.tone)

            if !reset, let resetsAt = window.resetsAt {
                Text("Resets \(relative(resetsAt, from: now))")
                    .font(Face.sans(10))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
        }
    }
}

private struct UsageBar: View {
    let fraction: Double
    let tone: Color

    var body: some View {
        GeometryReader { proxy in
            ZStack(alignment: .leading) {
                Capsule()
                    .fill(Color.primary.opacity(0.1))
                Capsule()
                    .fill(tone)
                    .frame(width: max(fraction > 0 ? 4 : 0, proxy.size.width * min(fraction, 1)))
            }
        }
        .frame(height: 5)
    }
}

private struct CircularUsage: View {
    let account: WidgetSnapshot.UsageAccount
    let now: Date

    var body: some View {
        let window = account.windows.filter { !$0.hasReset(now: now) }.max { $0.used < $1.used }

        Gauge(value: window?.used ?? 0) {
            Text(account.agent.prefix(1))
        } currentValueLabel: {
            Text(window.map { "\(Int(($0.used * 100).rounded()))" } ?? "–")
                .font(Face.mono(14))
        }
        .gaugeStyle(.accessoryCircularCapacity)
        .widgetAccentable()
        .accessibilityLabel("\(account.agent) \(window?.label ?? "usage") \(window?.percent ?? "unavailable")")
    }
}

private struct RectangularUsage: View {
    let account: WidgetSnapshot.UsageAccount
    let now: Date

    var body: some View {
        VStack(alignment: .leading, spacing: 1) {
            Text([account.agent, account.plan].compactMap { $0 }.joined(separator: " · "))
                .font(Face.sans(13))
                .lineLimit(1)
                .widgetAccentable()

            if account.windows.isEmpty {
                Text(account.error ?? "Usage unavailable")
                    .font(Face.sans(12))
                    .lineLimit(2)
            } else {
                ForEach(account.windows.prefix(2), id: \.label) { window in
                    let reset = window.hasReset(now: now)
                    HStack(spacing: 4) {
                        Text(window.label)
                        Spacer(minLength: 2)
                        Text(reset ? "Reset" : window.percent)
                            .font(Face.mono(12))
                        if !reset, let resetsAt = window.resetsAt {
                            Text("· \(relative(resetsAt, from: now))")
                                .foregroundStyle(.secondary)
                        }
                    }
                    .font(Face.sans(12))
                    .lineLimit(1)
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

private struct NoUsage: View {
    let compact: Bool

    var body: some View {
        if compact {
            Text("No usage yet")
                .font(Face.sans(12))
        } else {
            VStack(spacing: 4) {
                Text("No usage yet")
                    .font(Face.sans(14))
                Text("Open Zeron with a host online to load your agent plan limits")
                    .font(Face.sans(11))
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
            }
        }
    }
}

/// relative to the entry's date, not the clock: entries are rendered ahead
/// of when they're shown.
private func relative(_ date: Date, from now: Date) -> String {
    let formatter = RelativeDateTimeFormatter()
    formatter.unitsStyle = .abbreviated
    return formatter.localizedString(for: date, relativeTo: now)
}

private extension WidgetSnapshot.UsageWindow {
    var percent: String { "\(Int((used * 100).rounded()))%" }

    var tone: Color {
        switch used {
        case ..<0.7: Tone.accent
        case ..<0.9: Tone.warning
        default: Tone.danger
        }
    }

    func hasReset(now: Date) -> Bool {
        resetsAt.map { $0 <= now } ?? false
    }
}

private extension WidgetSnapshot.UsageAccount {
    /// probes older than an hour say so: the host may be offline.
    func staleLabel(now: Date) -> String? {
        guard let fetchedAt, now.timeIntervalSince(fetchedAt) > 3600 else { return nil }
        return "Updated \(relative(fetchedAt, from: now))"
    }
}

#if DEBUG
#Preview("Small", as: .systemSmall) {
    UsageWidget()
} timeline: {
    UsageEntry(date: .now, snapshot: .sample, sources: [])
}

#Preview("Medium", as: .systemMedium) {
    UsageWidget()
} timeline: {
    UsageEntry(date: .now, snapshot: .sample, sources: [])
    UsageEntry(date: .now, snapshot: .empty, sources: [])
}

#Preview("Medium, picked", as: .systemMedium) {
    UsageWidget()
} timeline: {
    UsageEntry(date: .now, snapshot: .sample, sources: ["opencode/opencode-go", "codex"])
}

#Preview("Large", as: .systemLarge) {
    UsageWidget()
} timeline: {
    UsageEntry(date: .now, snapshot: .sample, sources: [])
}

#Preview("Circular", as: .accessoryCircular) {
    UsageWidget()
} timeline: {
    UsageEntry(date: .now, snapshot: .sample, sources: [])
}

#Preview("Rectangular", as: .accessoryRectangular) {
    UsageWidget()
} timeline: {
    UsageEntry(date: .now, snapshot: .sample, sources: [])
}
#endif
