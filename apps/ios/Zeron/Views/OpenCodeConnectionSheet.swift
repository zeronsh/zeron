import SwiftUI

/// Connection settings live on the selected execution device, not the phone.
struct OpenCodeConnectionSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var deviceId = ""
    @State private var attached = false
    @State private var secure = false
    @State private var host = "localhost"
    @State private var port = "49374"
    @State private var path = ""
    @State private var username = "opencode"
    @State private var password = ""
    @State private var hasPassword = false
    @State private var clearPassword = false
    @State private var busy = false
    @State private var loadedDeviceId: String?
    @State private var status: String?

    private var update: OpencodeConnectionUpdate {
        let prefix = path.isEmpty || path.hasPrefix("/") ? path : "/" + path
        let address: String? = attached
            ? "\(secure ? "https" : "http")://\(host.contains(":") && !host.hasPrefix("[") ? "[\(host)]" : host):\(port)\(prefix)"
            : nil
        return OpencodeConnectionUpdate(baseUrl: address, username: username,
                                        password: password.isEmpty ? nil : password,
                                        clearPassword: clearPassword)
    }

    var body: some View {
        NavigationStack {
            Form {
                Section("Execution device") {
                    Picker("Device", selection: $deviceId) {
                        ForEach(model.executionDevices) { device in
                            Text(device.name).tag(device.id)
                        }
                    }
                    .disabled(busy)
                }
                Section("OpenCode server") {
                    Toggle("Connect to an existing server", isOn: $attached)
                    if attached {
                        Picker("Protocol", selection: $secure) {
                            Text("HTTP").tag(false)
                            Text("HTTPS").tag(true)
                        }
                        TextField("Localhost or loopback IP", text: $host)
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                        TextField("Port", text: $port)
                            .keyboardType(.numberPad)
                        TextField("Path prefix (optional)", text: $path)
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                        TextField("Username", text: $username)
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                        SecureField(hasPassword ? "Password (saved)" : "Password", text: $password)
                            .disabled(clearPassword)
                    }
                    if hasPassword {
                        Toggle("Clear saved password", isOn: $clearPassword)
                            .onChange(of: clearPassword) { _, clear in
                                if clear { password = "" }
                            }
                    }
                } footer: {
                    Text("Use a server on localhost (127.0.0.1 or ::1) of the selected execution device. Remote OpenCode servers are not supported yet. Leave server connection off to let Zeron start a local server.")
                }
                Section {
                    Button("Test connection") { Task { await test() } }
                        .disabled(busy || deviceId.isEmpty || !attached || loadedDeviceId != deviceId)
                    Button("Save") { Task { await save() } }
                        .disabled(busy || deviceId.isEmpty || loadedDeviceId != deviceId)
                    if loadedDeviceId != deviceId, !deviceId.isEmpty {
                        Button("Retry loading settings") { Task { await load() } }
                            .disabled(busy)
                    }
                    if let status { Text(status).foregroundStyle(Theme.textMuted) }
                }
            }
            .navigationTitle("OpenCode connection")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .cancellationAction) { Button("Done") { dismiss() } } }
        }
        .task(id: deviceId) { await load() }
        .onAppear {
            if deviceId.isEmpty { deviceId = model.executionDevices.first?.id ?? "" }
        }
    }

    @MainActor private func load() async {
        loadedDeviceId = nil
        attached = false
        secure = false
        host = "localhost"
        port = "49374"
        path = ""
        username = "opencode"
        password = ""
        hasPassword = false
        clearPassword = false
        status = nil
        guard !deviceId.isEmpty, let workspace = model.workspace else { return }
        let target = deviceId
        busy = true
        defer { busy = false }
        do {
            let settings = try await workspace.getOpencodeConnection(deviceId: target)
            guard target == deviceId, !Task.isCancelled else { return }
            attached = settings.baseUrl != nil
            if let raw = settings.baseUrl, let url = URLComponents(string: raw) {
                secure = url.scheme == "https"
                host = url.host ?? "localhost"
                port = url.port.map(String.init) ?? (secure ? "443" : "80")
                path = url.path
            } else {
                secure = false; host = "localhost"; port = "49374"; path = ""
            }
            username = settings.username
            hasPassword = settings.hasPassword
            password = ""
            clearPassword = false
            status = nil
            loadedDeviceId = target
        } catch {
            if target == deviceId { status = error.localizedDescription }
        }
    }

    @MainActor private func test() async {
        guard loadedDeviceId == deviceId, let workspace = model.workspace else { return }
        let target = deviceId
        busy = true
        defer { busy = false }
        do {
            let result = try await workspace.testOpencodeConnection(deviceId: target, update: update)
            if target == deviceId { status = "Connected to OpenCode \(result.version)" }
        } catch {
            if target == deviceId { status = error.localizedDescription }
        }
    }

    @MainActor private func save() async {
        guard loadedDeviceId == deviceId, let workspace = model.workspace else { return }
        let target = deviceId
        busy = true
        defer { busy = false }
        do {
            let saved = try await workspace.setOpencodeConnection(deviceId: target, update: update)
            if target == deviceId {
                hasPassword = saved.hasPassword
                password = ""
                clearPassword = false
                status = "Saved on \(model.deviceName(target))"
                model.opencodeConnectionGeneration[target, default: 0] += 1
            }
        } catch {
            if target == deviceId { status = error.localizedDescription }
        }
    }
}
