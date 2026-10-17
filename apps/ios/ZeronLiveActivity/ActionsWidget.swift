import AppIntents
import SwiftUI
import WidgetKit

/// one tap into a new session or a voice call.
struct ActionsWidget: Widget {
    var body: some WidgetConfiguration {
        StaticConfiguration(kind: WidgetStore.actionsKind, provider: SnapshotProvider()) { entry in
            ActionsWidgetView(entry: entry)
                .zeronWidgetBackground()
        }
        .configurationDisplayName("Quick Actions")
        .description("Start a new session or talk to Codex.")
        .supportedFamilies([.systemSmall, .accessoryCircular])
    }
}

private struct ActionsWidgetView: View {
    @Environment(\.widgetFamily) private var family
    let entry: SnapshotEntry

    var body: some View {
        if family == .accessoryCircular {
            ZStack {
                AccessoryWidgetBackground()
                Image(systemName: "plus")
                    .font(.system(size: 20, weight: .semibold))
            }
            .widgetURL(ZeronLink.newSession)
            .accessibilityLabel("New session")
        } else if let snapshot = entry.snapshot {
            VStack(spacing: 8) {
                ActionTile(title: "New session", symbol: "plus", tint: Tone.accent, intent: NewSessionIntent())

                if snapshot.voiceAvailable {
                    ActionTile(title: "Talk to Codex", symbol: "waveform", tint: .primary, intent: TalkToCodexIntent())
                }
            }
        } else {
            SignedOut(compact: false)
        }
    }
}

/// small widgets take a single `widgetURL`, so each tile is an intent button
/// (the intents open the app and route there).
private struct ActionTile<Intent: AppIntent>: View {
    let title: String
    let symbol: String
    let tint: Color
    let intent: Intent

    var body: some View {
        Button(intent: intent) {
            HStack(spacing: 8) {
                Image(systemName: symbol)
                    .font(.system(size: 14, weight: .bold))
                Text(title)
                    .font(Face.sans(13))
                    .lineLimit(1)
                    .minimumScaleFactor(0.85)
                Spacer(minLength: 0)
            }
            .foregroundStyle(tint)
            .padding(.horizontal, 12)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(tint.opacity(0.12), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
        }
        .buttonStyle(.plain)
    }
}

#if DEBUG
#Preview("Small", as: .systemSmall) {
    ActionsWidget()
} timeline: {
    SnapshotEntry(date: .now, snapshot: .sample)
    SnapshotEntry(date: .now, snapshot: .empty)
}
#endif
