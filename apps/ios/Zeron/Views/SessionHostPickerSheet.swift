import SwiftUI

/// Projectless drafts need only a host, including when there are no spaces.
struct SessionHostPickerSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    var selectedDeviceId: String? = nil
    let onSelected: (String) -> Void

    var body: some View {
        NavigationStack {
            List {
                if model.executionDevices.isEmpty {
                    Text("Connect a desktop device to start a session. No project is required.")
                        .foregroundStyle(Theme.textMuted)
                } else {
                    Section {
                        ForEach(model.executionDevices) { device in
                            Button {
                                onSelected(device.id)
                                dismiss()
                            } label: {
                                HStack {
                                    VStack(alignment: .leading, spacing: 4) {
                                        Text(device.name)
                                        Text(model.deviceOnline(device.id) ? "Online" : "Offline — sends are saved")
                                            .font(Theme.sans(12))
                                            .foregroundStyle(Theme.textMuted)
                                    }
                                    Spacer()
                                    if selectedDeviceId == device.id {
                                        Image(systemName: "checkmark")
                                    }
                                }
                            }
                            .accessibilityIdentifier("session-host-\(device.id)")
                        }
                    } footer: {
                        Text("Runs in the selected device’s home folder without a project.")
                    }
                }
            }
            .navigationTitle("Select a device")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
            }
        }
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
    }
}
