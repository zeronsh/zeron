import SwiftUI
import WidgetKit

struct SessionLine: View {
    let session: WidgetSnapshot.Session
    let asOf: Date?

    var body: some View {
        HStack(spacing: 8) {
            StatusIcon(status: session.status)
                .frame(width: 16)

            VStack(alignment: .leading, spacing: 1) {
                Text(session.title)
                    .font(Face.sans(13))
                    .lineLimit(1)
                    .privacySensitive()
                Text(session.detail)
                    .font(Face.sans(11))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .privacySensitive()
            }
            .frame(maxWidth: .infinity, alignment: .leading)

            SessionTime(session: session, asOf: asOf)
                .font(Face.mono(11))
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .frame(width: 52, alignment: .trailing)
        }
    }
}

struct StatusIcon: View {
    let status: WidgetSnapshot.Status

    var body: some View {
        Image(systemName: status.symbol)
            .font(.system(size: 12, weight: .semibold))
            .foregroundStyle(status.tone)
            .accessibilityLabel(status.label)
    }
}

/// running sessions tick their elapsed time on their own; the rest show when
/// they last changed, which stays true without a refresh. a stale snapshot
/// (`asOf`) can't tell whether a run is still going, so it shows when the run
/// started instead of ticking on.
struct SessionTime: View {
    let session: WidgetSnapshot.Session
    let asOf: Date?

    var body: some View {
        if session.status == .working, asOf == nil {
            Text(timerInterval: session.since...Date.distantFuture, countsDown: false)
                .monospacedDigit()
                .multilineTextAlignment(.trailing)
                .frame(maxWidth: 52, alignment: .trailing)
        } else {
            Text(session.since, style: .time)
        }
    }
}

struct NewSessionButton: View {
    var body: some View {
        Link(destination: ZeronLink.newSession) {
            Image(systemName: "plus")
                .font(.system(size: 13, weight: .bold))
                .foregroundStyle(Tone.accent)
                .frame(width: 28, height: 28)
                .background(Tone.accent.opacity(0.14), in: Circle())
        }
        .accessibilityLabel("New session")
    }
}

extension WidgetSnapshot.Session {
    var detail: String {
        [place, agent].compactMap { $0 }.joined(separator: " · ")
    }
}

extension WidgetSnapshot.Status {
    var symbol: String {
        switch self {
        case .awaiting: "hand.raised.fill"
        case .failed: "exclamationmark.triangle.fill"
        case .working: "circle.dotted"
        case .done: "checkmark.circle.fill"
        }
    }

    var tone: Color {
        switch self {
        case .awaiting: Tone.warning
        case .failed: Tone.danger
        case .working: Tone.accent
        case .done: Tone.success
        }
    }

    var label: String {
        switch self {
        case .awaiting: "Needs your input"
        case .failed: "Failed"
        case .working: "Working"
        case .done: "Finished"
        }
    }
}
