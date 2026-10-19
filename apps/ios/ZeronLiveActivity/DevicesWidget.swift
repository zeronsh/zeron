import SwiftUI
import WidgetKit

/// which hosts can take work right now.
struct DevicesWidget: Widget {
    var body: some WidgetConfiguration {
        StaticConfiguration(kind: WidgetStore.devicesKind, provider: SnapshotProvider()) { entry in
            DevicesWidgetView(entry: entry)
                .zeronWidgetBackground()
                .widgetURL(ZeronLink.sessions)
        }
        .configurationDisplayName("Devices")
        .description("Your hosts, whether they're online, and their sessions.")
        .supportedFamilies([.systemSmall, .systemMedium])
    }
}

private struct DevicesWidgetView: View {
    @Environment(\.widgetFamily) private var family
    let entry: SnapshotEntry

    var body: some View {
        if let snapshot = entry.snapshot {
            let limit = family == .systemSmall ? 3 : 4
            let online = snapshot.devices.count(where: \.online)
            let label = family == .systemSmall ? "Online" : "of \(snapshot.devices.count) online"

            VStack(alignment: .leading, spacing: 8) {
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    Text("\(online)")
                        .font(Face.mono(22))
                        .foregroundStyle(online > 0 && entry.asOf == nil ? Tone.success : .secondary)
                    Text(entry.asOf.map { "\(label) at \($0.stamp)" } ?? label)
                        .font(Face.sans(13))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }

                if snapshot.devices.isEmpty {
                    Spacer()
                    Text("No hosts yet")
                        .font(Face.sans(13))
                        .foregroundStyle(.secondary)
                    Spacer()
                } else {
                    ForEach(snapshot.devices.prefix(limit)) { device in
                        DeviceLine(device: device, compact: family == .systemSmall, stale: entry.asOf != nil)
                    }
                    Spacer(minLength: 0)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        } else {
            SignedOut(compact: false)
        }
    }
}

/// a stale snapshot keeps the online state but drops the green: the host may
/// have gone offline since.
private struct DeviceLine: View {
    let device: WidgetSnapshot.Device
    let compact: Bool
    let stale: Bool

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: device.symbol)
                .font(.system(size: 12, weight: .semibold))
                .foregroundStyle(device.online ? .primary : .secondary)
                .frame(width: 18)
                .overlay(alignment: .bottomTrailing) {
                    Circle()
                        .fill(device.online && !stale ? Tone.success : Color.secondary)
                        .frame(width: 6, height: 6)
                        .offset(x: 2, y: 2)
                }

            Text(device.name)
                .font(Face.sans(13))
                .foregroundStyle(device.online ? .primary : .secondary)
                .lineLimit(1)
                .privacySensitive()

            Spacer(minLength: 4)

            if !compact {
                DeviceStatus(device: device)
                    .font(Face.mono(11))
                    .foregroundStyle(.secondary)
            }
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(device.name), \(device.online ? "online" : "offline")")
    }
}

private struct DeviceStatus: View {
    let device: WidgetSnapshot.Device

    var body: some View {
        if device.online {
            Text(device.sessions == 1 ? "1 session" : "\(device.sessions) sessions")
        } else if let lastSeen = device.lastSeen {
            Text(lastSeen, format: .relative(presentation: .numeric, unitsStyle: .abbreviated))
        } else {
            Text("Offline")
        }
    }
}

private extension WidgetSnapshot.Device {
    var symbol: String {
        switch platform.lowercased() {
        case let p where p.contains("mac") || p.contains("darwin"): "laptopcomputer"
        case let p where p.contains("linux"): "server.rack"
        case let p where p.contains("win"): "pc"
        default: "desktopcomputer"
        }
    }
}

#if DEBUG
#Preview("Small", as: .systemSmall) {
    DevicesWidget()
} timeline: {
    SnapshotEntry(date: .now, snapshot: .sample)
}

#Preview("Medium", as: .systemMedium) {
    DevicesWidget()
} timeline: {
    SnapshotEntry(date: .now, snapshot: .sample)
    SnapshotEntry(date: .now.addingTimeInterval(WidgetSnapshot.staleAfter), snapshot: .sample)
}
#endif
