import ActivityKit
import AppIntents
import SwiftUI
import UIKit
import WidgetKit

/// The orchestrator call outside the app. The Dynamic Island keeps the orb
/// and the call clock beside the camera; expanded, and on the Lock Screen, it
/// adds what Codex is doing, the host and the stage's mute and end controls.
/// Tapping it returns to the full-screen stage.
struct VoiceLiveActivity: Widget {
    var body: some WidgetConfiguration {
        ActivityConfiguration(for: VoiceActivityAttributes.self) { context in
            LockScreenCall(host: context.attributes.host, state: context.state)
                .activitySystemActionForegroundColor(.primary)
                .widgetURL(Stage.url)
        } dynamicIsland: { context in
            let state = context.state
            return DynamicIsland {
                DynamicIslandExpandedRegion(.leading) {
                    OrbGlyph(orb: state.orb, large: true)
                        .frame(width: 46, height: 46)
                        .padding(.leading, 6)
                        .padding(.top, 2)
                }
                DynamicIslandExpandedRegion(.trailing) {
                    CallClock(since: state.since)
                        .font(Face.mono(15))
                        .foregroundStyle(.secondary)
                        .frame(maxHeight: .infinity, alignment: .center)
                        .padding(.trailing, 6)
                }
                DynamicIslandExpandedRegion(.center) {
                    VStack(alignment: .leading, spacing: 2) {
                        Status(orb: state.orb, size: 16)
                        Text("Codex on \(context.attributes.host)")
                            .font(Face.sans(12))
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.leading, 4)
                }
                DynamicIslandExpandedRegion(.bottom) {
                    HStack(spacing: 10) {
                        MuteControl(muted: state.muted, labeled: true)
                        EndControl(labeled: true)
                    }
                    .padding(.top, 6)
                    .padding(.horizontal, 4)
                }
            } compactLeading: {
                OrbGlyph(orb: state.orb, large: false)
                    .frame(width: 22, height: 22)
                    .padding(.leading, 2)
            } compactTrailing: {
                if state.muted {
                    Image(systemName: "mic.slash.fill")
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundStyle(.secondary)
                        .padding(.trailing, 2)
                } else {
                    CallClock(since: state.since)
                        .font(Face.mono(14))
                        .frame(width: 44, alignment: .trailing)
                }
            } minimal: {
                OrbGlyph(orb: state.orb, large: false)
                    .frame(width: 20, height: 20)
            }
            .keylineTint(state.orb == .awaiting ? Tone.warning : Tone.accent)
            .widgetURL(Stage.url)
        }
    }
}

private enum Stage {
    static let url = URL(string: "zeron://voice")
}

// MARK: Lock Screen

private struct LockScreenCall: View {
    let host: String
    let state: VoiceActivityAttributes.ContentState

    var body: some View {
        HStack(spacing: 14) {
            OrbGlyph(orb: state.orb, large: true)
                .frame(width: 52, height: 52)
            VStack(alignment: .leading, spacing: 3) {
                Text("Codex · \(host)")
                    .font(Face.mono(12))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                Status(orb: state.orb, size: 17)
                CallClock(since: state.since)
                    .font(Face.mono(13))
                    .foregroundStyle(.secondary)
            }
            Spacer(minLength: 8)
            HStack(spacing: 10) {
                MuteControl(muted: state.muted, labeled: false)
                EndControl(labeled: false)
            }
        }
        .padding(.horizontal, 18)
        .padding(.vertical, 16)
    }
}

// MARK: Pieces

/// The desktop orb's drawing for this state, in the surrounding ink. Waiting
/// on an answer it takes the stage's warning tone; muted it rests dimmer.
private struct OrbGlyph: View {
    let orb: VoiceActivityOrb
    let large: Bool

    var body: some View {
        Image("orb-\(orb.rawValue)-\(large ? "avatar" : "inline")")
            .resizable()
            .renderingMode(.template)
            .scaledToFit()
            .foregroundStyle(orb == .awaiting ? AnyShapeStyle(Tone.warning) : AnyShapeStyle(.primary))
            .opacity(orb == .muted ? 0.55 : 1)
            .id(orb)
            .transition(.opacity.combined(with: .scale(scale: 0.8)))
            .accessibilityLabel(orb.status)
    }
}

private struct Status: View {
    let orb: VoiceActivityOrb
    let size: CGFloat

    var body: some View {
        Text(orb.status)
            .font(Face.sans(size))
            .foregroundStyle(orb == .awaiting ? AnyShapeStyle(Tone.warning) : AnyShapeStyle(.primary))
            .lineLimit(1)
            .minimumScaleFactor(0.8)
            .contentTransition(.opacity)
    }
}

/// Elapsed call time, ticking on its own between updates.
private struct CallClock: View {
    let since: Date?

    var body: some View {
        if let since {
            Text(timerInterval: since...Date.distantFuture, countsDown: false)
                .monospacedDigit()
                .multilineTextAlignment(.trailing)
        } else {
            Image(systemName: "ellipsis")
                .font(.system(size: 13, weight: .semibold))
        }
    }
}

/// The stage's mute toggle: a quiet disc, filled with ink while muted.
private struct MuteControl: View {
    let muted: Bool
    let labeled: Bool

    var body: some View {
        Button(intent: ToggleVoiceMuteIntent()) {
            ControlFace(symbol: muted ? "mic.slash.fill" : "mic.fill", label: labeled ? (muted ? "Unmute" : "Mute") : nil)
                // Plain colors: hierarchical styles turn vibrant on the Lock
                // Screen material, and the muted disc would lose its ink.
                .foregroundStyle(muted ? Color(uiColor: .systemBackground) : Color.primary)
                .background(muted ? Color.primary : Color.primary.opacity(0.14), in: Capsule())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(muted ? "Unmute" : "Mute")
    }
}

/// Hang up, in the stage's red.
private struct EndControl: View {
    let labeled: Bool

    var body: some View {
        Button(intent: EndVoiceCallIntent()) {
            ControlFace(symbol: "phone.down.fill", label: labeled ? "End" : nil)
                .foregroundStyle(.white)
                .background(Color.red, in: Capsule())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("End call")
    }
}

private struct ControlFace: View {
    let symbol: String
    let label: String?

    var body: some View {
        if let label {
            Label(label, systemImage: symbol)
                .font(Face.sans(14))
                .labelStyle(.titleAndIcon)
                .frame(maxWidth: .infinity)
                .frame(height: 40)
        } else {
            Image(systemName: symbol)
                .font(.system(size: 15, weight: .bold))
                .frame(width: 44, height: 44)
        }
    }
}

// MARK: Style

/// The app's faces (Geist), bundled with the extension.
private enum Face {
    static func sans(_ size: CGFloat) -> Font { .custom("Geist-Medium", size: size) }
    static func mono(_ size: CGFloat) -> Font { .custom("GeistMono-Medium", size: size) }
}

/// Zeron Dark/Light accents (`Palette`), as the app paints them.
private enum Tone {
    static let accent = Color(UIColor { $0.userInterfaceStyle == .dark ? UIColor(hex: 0x8B7CF6) : UIColor(hex: 0x5B43E8) })
    static let warning = Color(UIColor { $0.userInterfaceStyle == .dark ? UIColor(hex: 0xFACC15) : UIColor(hex: 0xA16207) })
}

private extension UIColor {
    convenience init(hex: UInt32) {
        self.init(
            red: CGFloat(hex >> 16 & 0xFF) / 255,
            green: CGFloat(hex >> 8 & 0xFF) / 255,
            blue: CGFloat(hex & 0xFF) / 255,
            alpha: 1
        )
    }
}

#if DEBUG
#Preview("Island", as: .dynamicIsland(.expanded), using: VoiceActivityAttributes(host: "Fedora")) {
    VoiceLiveActivity()
} contentStates: {
    VoiceActivityAttributes.ContentState(orb: .speaking, muted: false, since: .now.addingTimeInterval(-83))
    VoiceActivityAttributes.ContentState(orb: .awaiting, muted: true, since: .now.addingTimeInterval(-421))
}

#Preview("Compact", as: .dynamicIsland(.compact), using: VoiceActivityAttributes(host: "Fedora")) {
    VoiceLiveActivity()
} contentStates: {
    VoiceActivityAttributes.ContentState(orb: .listening, muted: false, since: .now.addingTimeInterval(-83))
    VoiceActivityAttributes.ContentState(orb: .muted, muted: true, since: .now.addingTimeInterval(-83))
}

#Preview("Lock Screen", as: .content, using: VoiceActivityAttributes(host: "Fedora")) {
    VoiceLiveActivity()
} contentStates: {
    VoiceActivityAttributes.ContentState(orb: .working, muted: false, since: .now.addingTimeInterval(-83))
}
#endif
