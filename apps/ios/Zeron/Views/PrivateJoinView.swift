import SwiftUI

struct PrivateJoinView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var hubURL: String
    @State private var code: String
    @State private var name = "Mobile"
    @State private var busy = false
    @State private var error: String?

    init(invitation: PrivateInvitation) {
        _hubURL = State(initialValue: invitation.hubURL)
        _code = State(initialValue: invitation.code)
    }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("Hub address", text: $hubURL)
                        .keyboardType(.URL)
                        .textContentType(.URL)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .accessibilityIdentifier("private-hub-url")
                    TextField("Pairing code", text: $code)
                        .textContentType(.oneTimeCode)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .accessibilityIdentifier("private-pairing-code")
                    TextField("Device name", text: $name)
                        .accessibilityIdentifier("private-device-name")
                } footer: {
                    Text("Connect Tailscale, then enter the hub address and pairing code shown by your workspace's host. This device controls agents running on your servers.")
                }

                if model.workspace != nil {
                    Section {
                        Text("Pairing switches this app to the private workspace. Your current workspace stays separate.")
                    }
                }

                if let error {
                    Section { Text(error).foregroundStyle(Theme.danger) }
                }

                Section {
                    Button(action: pair) {
                        HStack {
                            Text(busy ? "Pairing…" : "Pair with workspace")
                            Spacer()
                            if busy { ProgressView() }
                        }
                    }
                    .disabled(busy || hubURL.isEmpty || code.isEmpty || name.isEmpty)
                    .accessibilityIdentifier("private-pair")
                }
            }
            .disabled(busy)
            .navigationTitle("Private via Tailscale")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }.disabled(busy)
                }
            }
        }
        .interactiveDismissDisabled(busy)
    }

    private func pair() {
        busy = true
        error = nil
        Task {
            do {
                try await model.joinPrivateWorkspace(hubURL: hubURL, code: code, name: name)
                dismiss()
            } catch {
                self.error = error.localizedDescription
            }
            busy = false
        }
    }
}
