import UIKit

struct NativeCodexMessage: Codable {
    var id: String
    var user: Bool
    var text: String
    var tool: NativeCodexTool?
}

struct NativeCodexConversation: Codable {
    var id = UUID().uuidString
    var threadId: String?
    var title = "New conversation"
    var messages: [NativeCodexMessage] = []
    var pinned: Bool?
    var archived: Bool?
    var section: String?
    var updatedAt: Date?
    var pendingPrompt: String?
    var settings: NativeCodexSettings?
    var workspaceName: String?
}

/// One foreground session owner shared by local screens. Codex owns agent
/// history; this file stores only the local workspace association and UI projection.
@MainActor
final class NativeCodexSession {
    static let shared = NativeCodexSession()
    let codex = EmbeddedCodex()
    let root: URL
    private(set) var conversations: [NativeCodexConversation] = []
    private(set) var conversation = NativeCodexConversation()
    private(set) var shell: MobileShellRuntime
    private(set) var models: [[String: Any]] = []
    private var defaultSettings: NativeCodexSettings = UserDefaults.standard.data(forKey: "nativeCodexSettings").flatMap { try? JSONDecoder().decode(NativeCodexSettings.self, from: $0) } ?? NativeCodexSettings(model: UserDefaults.standard.string(forKey: "nativeCodexModel") ?? "")
    var model: String { settingsForUI(draft: false).model }
    func settingsForUI(draft: Bool) -> NativeCodexSettings { (draft ? defaultSettings : conversation.settings ?? defaultSettings).normalized(catalog: models) }
    func setSettings(_ settings: NativeCodexSettings, draft: Bool) {
        guard draft || !running else { return }
        let normalized = settings.normalized(catalog: models)
        defaultSettings = normalized
        UserDefaults.standard.set(try? JSONEncoder().encode(normalized), forKey: "nativeCodexSettings")
        if !draft { conversation.settings = normalized; save() }
        changed()
    }
    private(set) var running = false
    private(set) var ready = false
    private(set) var signedIn = false
    private(set) var status = "Starting Codex…"
    private(set) var login: (id: String, url: URL, code: String)?
    private(set) var preparingLogin = false
    var onChange: (() -> Void)?
    var onWorkspaceChange: (() -> Void)?
    var onLoadingChange: (() -> Void)?
    private(set) var loadingProgress: Double?
    private var initialPrompt: String? { get { conversation.pendingPrompt } set { conversation.pendingPrompt = newValue } }

    func createConversation(prompt: String) -> String? {
        guard !running, !switching, !workspaceBusy else { return nil }
        selectConversation(nil)
        conversation.title = String(prompt.prefix(60))
        conversation.settings = defaultSettings.normalized(catalog: models)
        initialPrompt = prompt
        save()
        start()
        deliverInitialPrompt()
        return "native-codex-" + conversation.id
    }

    private func deliverInitialPrompt() {
        guard available, signedIn, let prompt = initialPrompt else { return }
        initialPrompt = nil
        _ = send(prompt)
    }

    func updateConversation(_ id: String, change: (inout NativeCodexConversation) -> Void) {
        guard let i = conversations.firstIndex(where: { $0.id == id }) else { return }
        change(&conversations[i])
        if conversation.id == id { conversation = conversations[i] }
        persistConversations()
        changed()
    }

    private(set) var workspaceBusy = false
    func beginWorkspaceChange() -> Bool {
        guard available, !running else { return false }
        workspaceBusy = true; changed(); return true
    }
    func endWorkspaceChange() { workspaceBusy = false; changed(); deliverInitialPrompt() }
    var available: Bool { ready && startup == nil && !switching && !stopping && !workspaceBusy }
    private var turnId: String?
    private var startup: Task<Void, Never>?
    private var tools: Task<Void, Never>?
    private var submission: Task<Void, Never>?
    private var stopping = false
    private var switching = false
    private var generation = 0
    private var background: NSObjectProtocol?

    private init() {
        var directory = "NativeCodex"
        #if DEBUG
        let args = ProcessInfo.processInfo.arguments
        if args.contains("-native-codex-fixture") {
            directory = "NativeCodexFixture"
            if let i = args.firstIndex(of: "-native-codex-fixture-session"), i + 1 < args.count, let id = UUID(uuidString: args[i + 1]) { directory += "-" + id.uuidString }
        }
        #endif
        root = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0].appendingPathComponent(directory)
        if let data = try? Data(contentsOf: root.appendingPathComponent("conversations.json")),
           let saved = try? JSONDecoder().decode([NativeCodexConversation].self, from: data) { conversations = saved }
        conversation = conversations.first ?? NativeCodexConversation()
        if conversation.settings == nil { conversation.settings = defaultSettings }
        shell = MobileShellRuntime(checkpointURL: root.appendingPathComponent("workspaces/\(conversation.id)/workspace.json"))
        finishPendingTools()
        codex.onEvent = { [weak self] in self?.receive($0) }
        background = NotificationCenter.default.addObserver(forName: UIApplication.didEnterBackgroundNotification, object: nil, queue: .main) { [weak self] _ in
            Task { @MainActor in
                guard let self else { return }
                self.save()
                if self.running { self.stop(); self.status = "Interrupted when the app entered the background"; self.changed() }
            }
        }
    }

    func start() {
        guard startup == nil, !ready else { return }
        loadingProgress = 0.1
        changed()
        startup = Task {
            defer { startup = nil; loadingProgress = nil; changed(); deliverInitialPrompt() }
            do {
                var fixture: String?
                #if DEBUG
                let args = ProcessInfo.processInfo.arguments
                if let index = args.firstIndex(of: "-native-codex-fixture"), index + 1 < args.count { fixture = args[index + 1] }
                if fixture != nil && args.contains("-native-codex-test-load-delay") { try await Task.sleep(for: .seconds(4)) }
                #endif
                try await codex.start(home: root.appendingPathComponent("engine"), fixtureBaseURL: fixture)
                ready = true
                loadingProgress = 0.65; changed()
                if fixture != nil { signedIn = true; defaultSettings.model = "gpt-5.1-codex" }
                else { try await refreshAccount() }
                loadingProgress = 0.8; changed()
                let catalog = try await codex.request("model/list", ["limit": 100])
                models = catalog["data"] as? [[String: Any]] ?? []
                defaultSettings = defaultSettings.normalized(catalog: models)
                if fixture != nil {
                    defaultSettings.model = "gpt-5.1-codex"
                    models = [["id": "gpt-5.1-codex", "model": "gpt-5.1-codex", "displayName": "gpt-5.1-codex", "isDefault": true, "defaultReasoningEffort": "medium", "supportedReasoningEfforts": [["reasoningEffort": "medium", "description": "Fixture medium"], ["reasoningEffort": "high", "description": "Fixture high"]], "serviceTiers": [["id": "default", "name": "Standard", "description": "Fixture standard"], ["id": "flex", "name": "Flex", "description": "Fixture flex"]]]]
                }
                status = signedIn ? "Local workspace · OpenAI model" : "Sign in to ChatGPT to start"
                loadingProgress = 0.95; changed()
                if let id = conversation.threadId { _ = try await codex.request("thread/resume", ["threadId": id]); await recoverImages() }
                changed()
            } catch { fail(error) }
        }
    }

    func refreshAccount() async throws {
        let response = try await codex.request("account/read", ["refreshToken": false])
        signedIn = response["account"] is [String: Any]
        if signedIn && startup == nil {
            let catalog = try await codex.request("model/list", ["limit": 100])
            models = catalog["data"] as? [[String: Any]] ?? []
        }
        changed()
        deliverInitialPrompt()
    }

    func signIn() {
        guard available, !signedIn, !preparingLogin, login == nil else { return }
        preparingLogin = true
        Task {
            defer { preparingLogin = false; changed() }
            do {
                status = "Preparing ChatGPT sign-in…"; changed()
                let result = try await codex.request("account/login/start", ["type": "chatgptDeviceCode"])
                guard let id = result["loginId"] as? String, let code = result["userCode"] as? String,
                      let urlString = result["verificationUrl"] as? String, let url = URL(string: urlString), url.scheme == "https" else {
                    throw EmbeddedCodex.Failure.message("Codex did not return a device sign-in code")
                }
                login = (id, url, code); status = "Enter \(code) in ChatGPT, then return here"; changed()
            } catch { fail(error) }
        }
    }

    func useAPIKey(_ key: String) async throws {
        _ = try await codex.request("account/login/start", ["type": "apiKey", "apiKey": key])
        try await refreshAccount()
        status = "Local workspace · OpenAI model"; changed()
    }

    func cancelLogin() {
        guard let login else { return }
        self.login = nil
        Task { _ = try? await codex.request("account/login/cancel", ["loginId": login.id]) }
        status = "Sign-in cancelled"; changed()
    }

    func signOut() {
        guard !running, !switching, !workspaceBusy else { return }
        Task {
            do { _ = try await codex.request("account/logout"); signedIn = false; login = nil; status = "Signed out of Codex"; changed() }
            catch { fail(error) }
        }
    }

    func selectModel(_ id: String) {
        var settings = settingsForUI(draft: false)
        settings.model = id
        setSettings(settings, draft: false)
    }

    func selectConversation(_ selected: NativeCodexConversation?) {
        guard !running, !switching, !workspaceBusy else { return }
        save()
        generation += 1
        conversation = selected ?? NativeCodexConversation()
        if conversation.settings == nil { conversation.settings = defaultSettings.normalized(catalog: models) }
        shell = MobileShellRuntime(checkpointURL: root.appendingPathComponent("workspaces/\(conversation.id)/workspace.json"))
        finishPendingTools()
        turnId = nil
        changed()
        if ready, let id = conversation.threadId {
            switching = true
            Task {
                defer { switching = false; changed(); deliverInitialPrompt() }
                do { _ = try await codex.request("thread/resume", ["threadId": id]); status = signedIn ? "On this iPhone · files saved" : "Sign in to ChatGPT to start"; await recoverImages() }
                catch { fail(error) }
            }
        } else { deliverInitialPrompt() }
    }

    @discardableResult
    func send(_ text: String) -> Bool {
        guard available, signedIn, !running, !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return false }
        let stamp = generation
        running = true; status = "Thinking…"
        conversation.messages.append(.init(id: UUID().uuidString, user: true, text: text))
        save(); changed()
        submission = Task {
            do {
                if conversation.threadId == nil {
                    var params: [String: Any] = [:]
                    if !model.isEmpty { params["model"] = model }
                    let response = try await codex.request("thread/start", params)
                    guard let id = (response["thread"] as? [String: Any])?["id"] as? String else { throw EmbeddedCodex.Failure.message("Missing Codex thread") }
                    conversation.threadId = id
                    conversation.title = String(text.prefix(60))
                }
                guard stamp == generation else { return }
                save(); changed()
                var params: [String: Any] = ["threadId": conversation.threadId!, "input": [["type": "text", "text": text]]]
                if !model.isEmpty { params["model"] = model }
                params.merge(settingsForUI(draft: false).turnParameters(catalog: models)) { _, new in new }
                let result = try await codex.request("turn/start", params)
                turnId = (result["turn"] as? [String: Any])?["id"] as? String
            } catch { running = false; fail(error) }
        }
        return true
    }

    func stop() {
        guard running, !stopping else { return }
        stopping = true
        generation += 1
        let submission = submission
        Task {
            await shell.cancel()
            await submission?.value
            if let thread = conversation.threadId, let turn = turnId {
                do { _ = try await codex.request("turn/interrupt", ["threadId": thread, "turnId": turn]) }
                catch { fail(error) }
            }
            finishPendingTools()
            stopping = false; running = false; status = "Interrupted. Saved file changes are kept."; save(); changed()
        }
    }

    private func receive(_ event: [String: Any]) {
        let method = event["method"] as? String ?? ""
        let params = event["params"] as? [String: Any] ?? [:]
        if method == "account/login/completed" {
            login = nil
            Task {
                do {
                    try await refreshAccount()
                    status = signedIn ? "Signed in to Codex" : (params["error"] as? String ?? "Sign-in failed")
                    changed()
                } catch { fail(error) }
            }
            return
        }
        if method == "mobile/error" { ready = false; running = false; status = params["message"] as? String ?? "Codex stopped"; changed(); return }
        if let thread = params["threadId"] as? String, thread != conversation.threadId { return }
        if let id = conversation.threadId { shell.generatedImagesDirectory = root.appendingPathComponent("engine/generated_images/" + id) }
        switch method {
        case "item/started", "item/completed":
            if let item = params["item"] as? [String: Any], item["type"] as? String == "imageGeneration" {
                receiveImage(item, completed: method == "item/completed")
            }
        case "turn/started": turnId = (params["turn"] as? [String: Any])?["id"] as? String
        case "item/agentMessage/delta":
            let id = params["itemId"] as? String ?? "assistant"
            append(id: id, text: params["delta"] as? String ?? "")
        case "item/tool/call":
            guard let id = event["id"] else { return }
            let stamp = generation
            let previous = tools
            let shell = shell
            let entry = params["callId"] as? String ?? UUID().uuidString
            let tool = params["tool"] as? String ?? "tool"
            let args = params["arguments"] as? [String: Any] ?? [:]
            conversation.messages.append(.init(id: entry, user: false, text: "", tool: .init(name: tool, argument: args["command"] as? String ?? args["path"] as? String ?? "")))
            save(); changed()
            tools = Task {
                await previous?.value
                let result: [String: Any]
                if stamp != generation || stopping { result = ["success": false, "contentItems": [["type": "inputText", "text": "Interrupted"]]] }
                else { result = await MobileCodexTools.dispatch(params, shell: shell) }
                if stamp == generation {
                    let content = (result["contentItems"] as? [[String: Any]])?.compactMap { $0["text"] as? String }.joined(separator: "\n") ?? ""
                    if let index = conversation.messages.firstIndex(where: { $0.id == entry }) {
                        conversation.messages[index].tool?.output = String(content.prefix(12000))
                        conversation.messages[index].tool?.resolved = true
                        conversation.messages[index].tool?.isError = result["success"] as? Bool != true
                    }
                    changed()
                    save()
                }
                try? codex.respond(id: id, result: result)
            }
        case "turn/completed":
            let pending = tools, stamp = generation
            Task {
                await pending?.value
                guard stamp == generation else { return }
                running = stopping; turnId = nil
                let turn = params["turn"] as? [String: Any] ?? [:]
                if let error = turn["error"] as? [String: Any] { status = error["message"] as? String ?? "Turn failed" }
                else { status = turn["status"] as? String == "interrupted" ? "Interrupted" : "On this iPhone · files saved" }
                finishPendingTools()
                save(); changed()
            }
        case "error": status = (params["error"] as? [String: Any])?["message"] as? String ?? "Codex error"; changed()
        default:
            if let id = event["id"] { try? codex.reject(id: id, message: "This request is not available in the mobile workspace") }
        }
    }

    private func receiveImage(_ item: [String: Any], completed: Bool) {
        guard let id = item["id"] as? String else { return }
        if !conversation.messages.contains(where: { $0.id == id }) {
            conversation.messages.append(.init(id: id, user: false, text: "", tool: .init(name: "imagegen", argument: item["revisedPrompt"] as? String ?? "Generate image")))
        }
        save(); changed()
        guard completed else { return }
        let previous = tools, stamp = generation, shell = shell
        tools = Task {
            await previous?.value
            guard stamp == generation else { return }
            let output: String
            var failed = false
            do {
                guard item["status"] as? String == "completed" else { throw NativeWorkspaceFiles.failure("Image generation did not complete") }
                let path = try await importGeneratedImage(source: item["savedPath"] as? String ?? id + ".png", shell: shell)
                output = "Saved \(path). Open Workspace files to preview or Save to Files."
            } catch { output = "Image could not be saved to the workspace: " + error.localizedDescription; failed = true }
            guard stamp == generation, let index = conversation.messages.firstIndex(where: { $0.id == id }) else { return }
            conversation.messages[index].tool?.output = output
            conversation.messages[index].tool?.isError = failed
            conversation.messages[index].tool?.resolved = true
            save(); changed()
        }
    }

    private func importGeneratedImage(source: String, shell: MobileShellRuntime) async throws -> String {
        guard let directory = shell.generatedImagesDirectory else { throw NativeWorkspaceFiles.failure("No conversation image directory") }
        let name = try NativeWorkspaceArtifacts.name(source)
        let path = "/workspace/generated/" + name
        // Preserve an already imported file, including any subsequent user edits.
        if try await shell.entries().contains(where: { $0.path == path }) { return path }
        let data = try await Task.detached { try NativeWorkspaceArtifacts.read(source, directory: directory) }.value
        try await shell.importEntries([.init(path: path, type: "file", mode: 420, content: data.base64EncodedString())])
        return path
    }

    /// Recover images from older app versions that dropped image-result events.
    private func recoverImages() async {
        guard let id = conversation.threadId else { return }
        let directory = root.appendingPathComponent("engine/generated_images/" + id)
        shell.generatedImagesDirectory = directory
        guard let files = try? FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil) else { return }
        for file in files.sorted(by: { $0.lastPathComponent < $1.lastPathComponent }) where file.pathExtension == "png" {
            let itemID = file.deletingPathExtension().lastPathComponent
            do {
                let path = try await importGeneratedImage(source: file.lastPathComponent, shell: shell)
                if !conversation.messages.contains(where: { $0.id == itemID }) {
                    conversation.messages.append(.init(id: itemID, user: false, text: "", tool: .init(name: "imagegen", argument: "Recovered generated image", output: "Saved \(path). Open Workspace files to preview or Save to Files.", resolved: true)))
                }
            } catch { status = "Could not recover generated image: " + error.localizedDescription; break }
        }
        save(); changed()
    }

    private func finishPendingTools() {
        for index in conversation.messages.indices {
            if conversation.messages[index].tool?.resolved == false {
                conversation.messages[index].tool?.resolved = true
                conversation.messages[index].tool?.isError = true
                conversation.messages[index].tool?.output = "Interrupted before a result was received. Saved file changes are kept."
            }
        }
    }

    private func append(id: String, text: String) {
        if let index = conversation.messages.firstIndex(where: { $0.id == id }) { conversation.messages[index].text += text }
        else { conversation.messages.append(.init(id: id, user: false, text: text)) }
        changed()
    }

    func nameWorkspace(_ name: String) { conversation.workspaceName = name; workspaceChanged() }

    func workspaceChanged() { if conversation.title == "New conversation" { conversation.title = "Local workspace" }; save(); changed() }

    private func save() {
        guard conversation.threadId != nil || !conversation.messages.isEmpty || conversation.pendingPrompt != nil || conversation.title != "New conversation" else { return }
        conversation.updatedAt = Date()
        conversations.removeAll { $0.id == conversation.id }
        conversations.insert(conversation, at: 0)
        persistConversations()
        onWorkspaceChange?()
    }

    private func persistConversations() {
        do {
            try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
            try JSONEncoder().encode(conversations).write(to: root.appendingPathComponent("conversations.json"), options: .atomic)
        } catch { status = "Could not save local history: \(error.localizedDescription)" }
    }
    private func fail(_ error: Error) { status = error.localizedDescription; changed() }
    private func changed() { onChange?(); onWorkspaceChange?(); onLoadingChange?() }
}
