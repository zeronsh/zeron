import SwiftUI

/// Native sheet material shares the presentation's device-aware corner mask.
private struct UpdatePaneGlass: ViewModifier {
    func body(content: Content) -> some View {
        // Let the presentation own its material and corner mask. A second
        // rounded background extending below the safe area breaks the lower
        // corners of the floating sheet on newer iOS presentations.
        content
            .presentationBackground(.regularMaterial)
            .presentationDragIndicator(.visible)
    }
}

/// Keep row actions distinct from content; navigation uses native toolbar chrome.
private struct UpdatePaneButtonStyle: ButtonStyle {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(Theme.sans(13, weight: .medium))
            .fixedSize(horizontal: true, vertical: false)
            .padding(.horizontal, 13)
            .frame(minWidth: 36, minHeight: 36)
            .background(Theme.elementHover, in: Capsule())
            .overlay { Capsule().strokeBorder(Theme.borderStrong, lineWidth: 0.5) }
            .scaleEffect(configuration.isPressed && !reduceMotion ? 0.96 : 1)
            .opacity(configuration.isPressed ? 0.75 : 1)
            .frame(minHeight: 44)
            .contentShape(Rectangle())
    }
}

/// The phone is a controller: all installs execute on the selected desktop.
struct HarnessUpdatesView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.scenePhase) private var scenePhase
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    let deviceId: String
    var compact = false
    var onContentHeight: ((CGFloat) -> Void)? = nil
    @State private var updates = HarnessUpdatesModel()
    @State private var showingSheet = false
    @State private var sheetHeight: CGFloat = 580

    private var supported: Bool {
        model.devices.first { $0.id == deviceId }?.supports(EngineCapability.harnessUpdatesV1) == true
    }
    private var online: Bool { model.deviceOnline(deviceId) }
    private var source: (any HarnessUpdatesSource)? {
        if let demo = model.demo { return demo }
        return model.workspace
    }
    private var watchKey: String {
        "\(deviceId)-\(online)-\(supported)-\(scenePhase == .active)"
    }

    var body: some View {
        Group {
            if compact {
                let count = updates.statuses.filter(\.visible).count
                if supported, online, count > 0 {
                    Button { showingSheet = true } label: {
                        HStack(spacing: 8) {
                            HStack(spacing: -5) {
                                ForEach(Array(updates.statuses.filter(\.visible).prefix(3))) { status in
                                    HarnessBadge(harness: status.harness, size: 16)
                                        .padding(3)
                                        .background(Theme.surface, in: Circle())
                                }
                            }
                            if updates.statuses.contains(where: \.active) {
                                ProgressView().controlSize(.mini)
                            }
                            Text(count == 1 ? "Agent update" : "\(count) agent updates")
                            Image(systemName: "chevron.up").font(.caption2)
                        }
                        .font(Theme.sans(12, weight: .medium))
                        .foregroundStyle(Theme.text)
                        .padding(.horizontal, 14).padding(.vertical, 9)
                        .glassEffect(.regular.interactive(), in: Capsule())
                    }
                    .buttonStyle(.plain)
                    .accessibilityIdentifier("agent-updates-chip")
                    .transition(.opacity)
                }
            } else {
                contents
            }
        }
        .animation(reduceMotion ? nil : .easeInOut(duration: 0.2),
                   value: updates.statuses.filter(\.visible).count)
        .sheet(isPresented: $showingSheet) {
            NavigationStack {
                contents.toolbar {
                    ToolbarItem(placement: .confirmationAction) {
                        Button("Done") { showingSheet = false }
                    }
                }
            }
            .presentationDetents([.height(sheetHeight)])
            .modifier(UpdatePaneGlass())
        }
        .task(id: watchKey) {
            guard supported, online, scenePhase == .active,
                  let source else { return }
            await updates.watch(deviceId: deviceId, source: source)
        }
    }

    private var contents: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                HStack(spacing: 12) {
                    Image(systemName: model.devices.first { $0.id == deviceId }?.platform == "macos" ? "laptopcomputer" : "server.rack")
                        .font(.system(size: 20, weight: .regular))
                        .frame(width: 28)
                        .foregroundStyle(Theme.textMuted)
                    VStack(alignment: .leading, spacing: 4) {
                        Text(model.deviceName(deviceId))
                            .font(Theme.sans(15, weight: .semibold))
                            .fixedSize(horizontal: false, vertical: true)
                        HStack(spacing: 6) {
                            Circle().fill(online ? Theme.statusCompleted : Theme.textFaint)
                                .frame(width: 5, height: 5)
                                .accessibilityHidden(true)
                            Text(online ? "Online" : "Offline")
                        }
                        .font(Theme.sans(12))
                        .foregroundStyle(Theme.textMuted)
                    }
                    Spacer(minLength: 8)
                    actionButton("Check for updates", .check, nil)
                }
                .padding(.bottom, 12)
                Divider()
                if !supported {
                    Text("Update Zeron on this device to manage agent updates.")
                        .foregroundStyle(Theme.textMuted)
                        .padding(.vertical, 12)
                } else if !online {
                    Text("Device offline. Updates can be started when it reconnects.")
                        .foregroundStyle(Theme.textMuted)
                        .padding(.vertical, 12)
                } else if !updates.connected {
                    Label("Connecting to device…", systemImage: "wifi")
                        .foregroundStyle(Theme.textMuted)
                        .padding(.vertical, 12)
                }
                if let error = updates.error {
                    Label(error, systemImage: "exclamationmark.triangle")
                        .font(Theme.sans(13)).foregroundStyle(Theme.warning)
                        .padding(.vertical, 12)
                }
                if !updates.statuses.isEmpty {
                    VStack(spacing: 0) {
                        ForEach(updates.statuses) { status in
                            updateRow(status)
                            if status.id != updates.statuses.last?.id {
                                Divider().padding(.leading, 40)
                            }
                        }
                    }
                }
                if updates.connected, updates.statuses.isEmpty {
                    Text("No enabled agents on this device.")
                }
                Text("Updates run when agents are idle, even after you close this pane.")
                    .font(Theme.sans(12))
                    .foregroundStyle(Theme.textMuted)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.top, 16)
            }
            .padding(.horizontal, 20)
            .padding(.top, 12)
            .padding(.bottom, 20)
            .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { height in
                sheetHeight = height + 64
                onContentHeight?(height + 64)
            }
        }
        .scrollBounceBehavior(.basedOnSize)
        .containerBackground(.clear, for: .navigation)
        .navigationTitle("Agent updates")
        .navigationBarTitleDisplayMode(.inline)
    }

    private func updateRow(_ status: HarnessUpdateStatus) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 12) {
                HarnessBadge(harness: status.harness, size: 24)
                    .frame(width: 28, height: 28)
                    .accessibilityHidden(true)
                ViewThatFits(in: .horizontal) {
                    HStack(spacing: 12) {
                        agentTitle(status)
                            .fixedSize(horizontal: true, vertical: false)
                        Spacer(minLength: 8)
                        rowAction(status)
                    }
                    VStack(alignment: .leading, spacing: 8) {
                        agentTitle(status)
                        rowAction(status)
                    }
                }
            }
            .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
            VStack(alignment: .leading, spacing: 8) {
                if status.phase != "available" || status.latestVersion == nil {
                    HStack(alignment: .top, spacing: 6) {
                        if status.active {
                            ProgressView().controlSize(.mini)
                        } else if status.phase == "failed" {
                            Image(systemName: "exclamationmark.circle")
                        } else if ["updated", "current"].contains(status.phase) {
                            Image(systemName: "checkmark.circle")
                        }
                        Text(status.label).fixedSize(horizontal: false, vertical: true)
                    }
                    .font(Theme.sans(12))
                    .foregroundStyle(status.phase == "failed" ? Theme.warning :
                                     ["updated", "current"].contains(status.phase) ? Theme.statusCompleted : Theme.textMuted)
                }
                if !status.actionable, ["available", "manual-action-required"].contains(status.phase),
                   let command = status.manualCommand {
                    Text(command)
                        .font(Theme.mono(11))
                        .textSelection(.enabled)
                        .padding(10)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .background(Theme.elementHover, in: RoundedRectangle(cornerRadius: 10))
                        .accessibilityLabel(command)
                }
            }
            .padding(.leading, 40)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.vertical, 12)
    }

    private func agentTitle(_ status: HarnessUpdateStatus) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(status.name).font(Theme.sans(15, weight: .semibold))
            HStack(spacing: 6) {
                if let installed = status.installedVersion {
                    Text(installed)
                        .foregroundStyle(Theme.textMuted)
                        .accessibilityLabel("Installed \(installed)")
                }
                if status.phase == "available", let latest = status.latestVersion {
                    Image(systemName: "arrow.right")
                        .font(.system(size: 9, weight: .medium))
                        .foregroundStyle(Theme.textFaint)
                        .accessibilityHidden(true)
                    Text(latest)
                        .foregroundStyle(Theme.text)
                        .accessibilityLabel("Version \(latest) available")
                }
            }
            .font(Theme.mono(11))
        }
        .fixedSize(horizontal: false, vertical: true)
    }

    @ViewBuilder
    private func rowAction(_ status: HarnessUpdateStatus) -> some View {
        if status.actionable {
            actionButton("Update", .apply, status.harness)
        } else if status.cancellable {
            actionButton("Cancel", .cancel, status.harness)
        } else if status.phase == "failed" {
            actionButton("Check again", .check, status.harness)
        }
    }

    private func actionButton(_ title: String, _ action: HarnessUpdateAction,
                              _ harness: String?) -> some View {
        Button {
            guard let source else { return }
            Task {
                await updates.action(action, harness: harness,
                                     deviceId: deviceId, source: source)
            }
        } label: {
            if harness == nil {
                Image(systemName: "arrow.clockwise")
                    .font(.system(size: 15, weight: .medium))
                    .accessibilityLabel(title)
                    .help(title)
            } else {
                Text(title)
            }
        }
        .font(Theme.sans(13, weight: .medium))
        .buttonStyle(UpdatePaneButtonStyle())
        .buttonBorderShape(.capsule)
        .accessibilityIdentifier("agent-update-\(action.method)-\(harness ?? "all")")
        .disabled(!online || !supported || !updates.connected
                  || updates.pending.contains(action.pendingKey(harness)))
    }
}

struct DeviceUpdatesList: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var path: [String]
    @State private var listHeight: CGFloat = 280
    @State private var detailHeight: CGFloat = 640

    init(initialDeviceId: String? = nil) {
        _path = State(initialValue: initialDeviceId.map { [$0] } ?? [])
    }

    var body: some View {
        NavigationStack(path: $path) {
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    VStack(spacing: 0) {
                        ForEach(model.executionDevices) { device in
                            NavigationLink(value: device.id) {
                                HStack(spacing: 12) {
                                    Image(systemName: device.platform == "macos" ? "laptopcomputer" : "server.rack")
                                        .font(.system(size: 20, weight: .regular))
                                        .frame(width: 28, height: 28)
                                        .foregroundStyle(Theme.textMuted)
                                    VStack(alignment: .leading, spacing: 4) {
                                        Text(device.name).font(Theme.sans(15, weight: .semibold))
                                            .fixedSize(horizontal: false, vertical: true)
                                        HStack(spacing: 6) {
                                            Circle().fill(model.deviceOnline(device.id) ? Theme.statusCompleted : Theme.textFaint)
                                                .frame(width: 5, height: 5)
                                            Text(model.deviceOnline(device.id) ? "Online" : "Offline")
                                                .font(Theme.sans(12)).foregroundStyle(Theme.textMuted)
                                        }
                                    }
                                    Spacer(minLength: 8)
                                    Image(systemName: "chevron.right")
                                        .font(.system(size: 12, weight: .semibold))
                                        .foregroundStyle(Theme.textFaint)
                                }
                                .frame(minHeight: 44)
                                .padding(.vertical, 12)
                                .contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                            .accessibilityIdentifier("device-updates-\(device.id)")
                            if device.id != model.executionDevices.last?.id {
                                Divider().padding(.leading, 40)
                            }
                        }
                    }
                    if model.executionDevices.isEmpty {
                        ContentUnavailableView("No devices", systemImage: "desktopcomputer",
                                               description: Text("Connect a desktop to manage its agents."))
                    }
                }
                .padding(.horizontal, 20)
                .padding(.top, 8)
                .padding(.bottom, 16)
                .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { height in
                    listHeight = height + 64
                }
            }
            .scrollBounceBehavior(.basedOnSize)
            .containerBackground(.clear, for: .navigation)
            .navigationDestination(for: String.self) { deviceId in
                HarnessUpdatesView(deviceId: deviceId, onContentHeight: { detailHeight = $0 })
                    .navigationBarBackButtonHidden(true)
                    .toolbar {
                        ToolbarItem(placement: .topBarLeading) {
                            Button {
                                path.removeLast()
                            } label: {
                                Image(systemName: "chevron.left")
                                    .font(.system(size: 15, weight: .semibold))
                            }
                            .accessibilityLabel("Devices")
                        }
                        ToolbarItem(placement: .confirmationAction) {
                            Button("Done") { dismiss() }
                        }
                    }
            }
            .navigationTitle("Devices")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
        .presentationDetents([.height(path.isEmpty ? listHeight : detailHeight)])
        .modifier(UpdatePaneGlass())
    }
}
