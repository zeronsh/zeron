import UIKit

/// A live session backed by the Rust core. The transcript flows Rust→Rust
/// (`TranscriptView.attach`); this maps the composer-facing state.
final class CoreSessionSource: SessionSource {
    private(set) var chrome = SessionChrome()
    var onChange: (() -> Void)?
    private weak var app: AppModel?
    private let client: CoreClient
    private let handle: SessionHandle
    private let chatId: String
    private var token: AnyObject?
    private var appToken: AnyObject?
    private var hostDevice = "" {
        didSet { if hostDevice != oldValue { connectVisuals() } }
    }
    /// Between turns, on a host that switches providers, the model menu
    /// offers the other providers too.
    private var canSwitchProvider = false
    private var hostSwitchesProviders = false
    private var turnRunning = false
    /// The host's providers, read ahead: a menu whose top level waits on the
    /// host doesn't open until it answers.
    private var hostHarnesses: [HarnessInfo] = []
    private var harnessesDevice = ""
    /// What a chat with no model of its own runs: its host's default (the
    /// catalog's first model), by "device|harness". Seeded from the built-in
    /// list so the chip never shows a bare provider name, then the host's.
    private var defaultModels: [String: ModelPickerRow] = [:]
    private var defaultModelsLoading: Set<String> = []
    private weak var engine: TranscriptView?

    init(app: AppModel, client: CoreClient, handle: SessionHandle, chatId: String) {
        self.app = app
        self.client = client
        self.handle = handle
        self.chatId = chatId
        token = app.observeSession(chatId) { [weak self] in self?.refresh() }
        appToken = app.observe { [weak self] in self?.refresh() }
        refresh()
    }

    func attach(_ engine: TranscriptView) {
        _ = engine.attach(client: client, chatId: chatId)
        self.engine = engine
        connectVisuals()
        handle.setViewAttached(attached: true)
    }

    /// Agent visuals load their scripts from the chat's host device.
    private func connectVisuals() {
        guard let engine, !hostDevice.isEmpty else { return }
        engine.setVisualSource(client: client, deviceId: hostDevice, pixelScale: Float(max(2, UITraitCollection.current.displayScale)))
    }

    func detach() {
        handle.setViewAttached(attached: false)
    }

    var isUnused: Bool {
        let t = handle.transcriptStatus()
        return t.entryCount == 0 && t.pendingCount == 0 && handle.composer().queue.isEmpty
    }

    func reattach() {
        handle.setViewAttached(attached: true)
    }

    private func refresh() {
        let c = handle.composer()
        let row = app?.row(chatId)
        hostDevice = c.host.deviceId
        hostSwitchesProviders = c.host.capabilities.providerSwitch
        turnRunning = c.live.turnRunning
        // Not mid-turn, and not while a message is still on its way (its
        // turn is about to start on the current provider).
        let sendPending = c.sendState == .sending || c.sendState == .queued
        canSwitchProvider = hostSwitchesProviders && !turnRunning && !sendPending
        if canSwitchProvider, harnessesDevice != hostDevice {
            let device = hostDevice
            harnessesDevice = device
            Task { @MainActor [weak self] in
                guard let self else { return }
                let list = await self.client.listHarnesses(deviceId: device)
                guard self.hostDevice == device else { return }
                // Nothing back (offline, slow): ask again on the next refresh.
                guard !list.isEmpty else {
                    self.harnessesDevice = ""
                    return
                }
                self.hostHarnesses = list
                self.onChange?()
            }
        }
        var next = SessionChrome()
        next.title = c.title
        let project = row?.project?.name ?? "No project"
        // A Cloud chat's machine is "Cloud", not its own device name.
        let hostName = row?.isCloud == true ? row?.deviceName : c.host.name
        next.subtitle = hostName.map { "\(project) @ \($0)" } ?? project
        next.running = c.live.turnRunning
        next.canSteer = c.host.capabilities.midTurnSteering ?? false
        next.placeholder = "Message \(row?.harnessLabel ?? "the agent")"
        var chips: [ComposerChip] = []
        if let model = row?.modelLabel ?? row.flatMap({ defaultModel(for: $0)?.label }) ?? row?.harnessLabel {
            chips.append(ComposerChip(id: "model", title: model, symbol: nil, icon: BrandMarks.image(for: row?.harness ?? "claude-code", side: 13)))
        }
        if let r = row?.reasoning, !r.isEmpty {
            chips.append(ComposerChip(id: "effort", title: reasoningLabel(level: r), symbol: "gauge.with.dots.needle.67percent"))
        }
        if let pr = row?.pullRequest {
            let state: SessionRowVM.PR = switch pr.state { case .open: .open; case .merged: .merged; case .closed: .closed }
            chips.append(ComposerChip(id: "pr", title: "\(pr.number)", symbol: nil, tint: PRBadgeView.tone(state), icon: PRIcon.image(side: 12)))
        } else if let b = row?.branch, !b.isEmpty {
            chips.append(ComposerChip(id: "branch", title: b, symbol: nil, icon: BranchIcon.sized()))
        }
        if let usage = c.contextUsage, let tokens = usage.tokens, let window = usage.window, window > 0 {
            let fraction = Double(tokens) / Double(window)
            if fraction >= 0.5 {
                chips.append(ComposerChip(
                    id: "context",
                    title: "\(Int((fraction * 100).rounded()))% context",
                    symbol: fraction >= 0.85 ? "exclamationmark.circle" : "circle.lefthalf.filled",
                    tint: fraction >= 0.85 ? Palette.warning : nil
                ))
            }
        }
        next.chips = chips
        if let sendFailure {
            next.banner = .failed(sendFailure)
        } else if c.sendState == .failed {
            next.banner = .notDelivered
        } else if c.sendState == .queued {
            // A Cloud chat's message waits for its machine to start or wake —
            // the transcript's setup step says so, as on the desktop.
            if row?.isCloud != true {
                next.banner = .failed("\(c.host.name ?? "Host") is offline — will send when it's back")
            }
        } else if c.sendState == .sending {
            next.banner = .sending
        } else if app?.connectivity?.state == .offline {
            next.banner = .offline
        } else if !c.room.connected, let retry = c.room.retryAtMs {
            next.banner = .reconnecting(in: max(1, Int((retry - Int64(Date().timeIntervalSince1970 * 1000)) / 1000)))
        }
        next.uploadProgress = c.transferProgress
        // Working state is shown at the transcript tail (layout engine), not here.
        if let input = c.openInput {
            next.questions = (input.requestId, input.questions.map {
                SessionChrome.Question(id: $0.id, header: $0.header, text: $0.question, options: $0.options, multiSelect: $0.multiSelect, prefill: $0.prefill, multiline: $0.multiline)
            })
        }
        next.queue = c.queue.map { q in
            let gate: String? = switch q.gate {
            case let .editing(_, _, mine)?: mine ? "Editing" : "Being edited"
            case .reviewRequired?: "Needs review"
            case nil: q.actionPending ? "Updating" : nil
            }
            return SessionChrome.QueuedItem(id: q.id, text: q.visibleText, thumbnail: nil, gate: gate)
        }
        next.error = c.queueError
        if next != chrome {
            chrome = next
            onChange?()
        }
    }

    /// False when the core refused the message (it stays in the composer).
    @discardableResult
    func send(text: String, images: [StagedImage], mode: DeliveryMode) -> Bool {
        do {
            if mode == .interrupt, chrome.running { try handle.interrupt() }
            _ = try handle.send(request: SendRequest(text: text, attachments: images.map(\.outgoing), worktree: nil, busy: mode == .steer ? .steer : .queue))
            app?.forgetFreshSideChat(chatId)
            app?.noteDeliveryActivity()
            sendFailure = nil
            return true
        } catch {
            // Kept across refreshes (which rebuild the chrome) until the next send.
            sendFailure = "Couldn't send: \(error)"
            refresh()
            return false
        }
    }

    private var sendFailure: String?

    func stop() {
        try? handle.interrupt()
    }

    func answer(requestId: String, answers: [(questionId: String, labels: [String])]) {
        try? handle.respondInput(requestId: requestId, answers: answers.map { UserInputAnswer(questionId: $0.questionId, labels: $0.labels) })
    }

    func queueAction(_ id: String, _ action: QueueAction) {
        switch action {
        // Steers text into the live turn (never interrupts it); only
        // attachment rows stop the turn to send.
        case .sendNow: Task { _ = try? await handle.deliverQueuedNow(id: id) }
        case .remove: Task { _ = try? await handle.removeQueued(id: id) }
        case .moveUp: _ = try? handle.moveQueuedBy(id: id, delta: -1)
        case .moveDown: _ = try? handle.moveQueuedBy(id: id, delta: 1)
        case .edit: break // Driven by the view: beginEdit / finishEdit.
        }
    }

    private var lease: QueueEditLease?
    private var renewal: Task<Void, Never>?
    /// The row as it was when the edit began: the composer only edits its
    /// visible text, the rest (Appshot context, attachment trailer) rides along.
    private var editBase: (raw: String, visible: String)?

    func beginEdit(_ id: String) async -> String? {
        let start = await handle.beginQueuedEdit(id: id, instanceId: UUID().uuidString)
        guard case let .acquired(lease) = start else { return nil }
        self.lease = lease
        renewal?.cancel()
        renewal = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(20))
                guard let self, let lease = self.lease, !Task.isCancelled else { return }
                if await !self.handle.renewQueuedEdit(lease: lease) { return }
            }
        }
        guard let item = handle.composer().queue.first(where: { $0.id == id }) else { return nil }
        editBase = (item.text, item.visibleText)
        return item.visibleText
    }

    func finishEdit(text: String?) async {
        renewal?.cancel()
        let base = editBase
        editBase = nil
        guard let lease else { return }
        self.lease = nil
        var body = text
        if let edited = text, let base {
            // Unchanged: release the row as it was (a commit would drop what
            // the composer never showed).
            body = edited == base.visible ? nil : Self.replacingVisible(in: base.raw, visible: base.visible, with: edited)
        }
        _ = await handle.finishQueuedEdit(lease: lease, action: body == nil ? .cancel : .commit, text: body)
    }

    /// The row's raw text with its visible part replaced, keeping the hidden
    /// context after it. Blank edits stay blank (the host removes the row).
    static func replacingVisible(in raw: String, visible: String, with edited: String) -> String {
        if edited.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || raw == visible { return edited }
        let body = raw.drop(while: \.isWhitespace)
        if body.hasPrefix(visible) { return edited + body.dropFirst(visible.count) }
        // Attachment-only rows show a placeholder: all of the raw text is context.
        return body.isEmpty ? edited : edited + "\n\n" + body
    }

    func retryDelivery() {
        try? handle.retryDelivery()
    }

    func chipMenu(_ id: String) -> UIMenu? {
        guard let row = app?.row(chatId) else { return nil }
        let harness = row.harness ?? "claude-code"
        switch id {
        case "model":
            // The picker sheet takes the tap (see `modelPicker`).
            return nil
        case "effort":
            return UIMenu(title: "Reasoning effort", children: [UIDeferredMenuElement { [weak self] done in
                guard let self else { return done([]) }
                Task { @MainActor in
                    let models = (try? await self.client.listModels(deviceId: self.hostDevice, harness: harness)) ?? fallbackModels(harness: harness)
                    let levels = models.first { $0.id == row.model }?.reasoningLevels ?? models.first?.reasoningLevels ?? []
                    done(levels.map { l in
                        UIAction(title: reasoningLabel(level: l), state: l == row.reasoning ? .on : .off) { [weak self] _ in
                            self?.setConfig { $0.reasoning = l }
                        }
                    })
                }
            }])
        case "pr":
            guard let url = row.pullRequest.flatMap({ URL(string: $0.url) }) else { return nil }
            return UIMenu(title: row.pullRequest?.title ?? "", children: [
                UIAction(title: "Open Pull Request", image: UIImage(systemName: "safari")) { _ in UIApplication.shared.open(url) },
                UIAction(title: "Copy Link", image: UIImage(systemName: "link")) { _ in UIPasteboard.general.url = url },
            ])
        default:
            return nil
        }
    }

    func modelPicker() -> UIViewController? {
        guard let row = app?.row(chatId) else { return nil }
        let harness = row.harness ?? "claude-code"
        let rows: (String) async -> [ModelPickerRow] = { [weak self] id in
            guard let self else { return [] }
            let models = (try? await self.client.listModels(deviceId: self.hostDevice, harness: id)) ?? fallbackModels(harness: id)
            return models.map { ModelPickerRow(id: $0.id, label: $0.label, detail: $0.description) }
        }
        var tabs = [ModelPickerTab(harness: harness, label: row.harnessLabel ?? HarnessNames.label(harness)) { await rows(harness) }]
        if canSwitchProvider {
            tabs += providers(cloud: row.isCloud).filter { $0.id != harness }.map { h in
                ModelPickerTab(harness: h.id, label: h.label, footer: "Your next message continues this chat with \(h.label).") { await rows(h.id) }
            }
        }
        #if DEBUG
        // Screenshots: `-all-providers` offers every agent there is.
        if ProcessInfo.processInfo.arguments.contains("-all-providers") {
            tabs = [tabs[0]] + ["claude-code", "codex", "opencode", "cursor", "devin", "grok", "hermes", "pi", "antigravity", "muse"]
                .filter { $0 != harness }
                .map { id in ModelPickerTab(harness: id, label: HarnessNames.label(id)) { await rows(id) } }
        }
        #endif
        let note: String? = canSwitchProvider ? nil
            : turnRunning ? "You can switch provider when this turn ends."
            : hostSwitchesProviders ? nil
            : "This computer needs a newer Zeron to switch provider."
        let host = row.isCloud ? (name: "Cloud", symbol: "cloud") : (name: app?.deviceName(hostDevice) ?? "This computer", symbol: "desktopcomputer")
        // No model of its own: it runs the default, so that's the one ticked.
        let current = row.model ?? defaultModel(for: row)?.id
        return ModelPickerViewController.sheet(tabs: tabs, harness: harness, model: current, host: host, note: note) { [weak self] pickedHarness, model in
            guard let self else { return }
            // Re-checked at the pick: a turn may have started since the
            // sheet opened with other providers' tabs.
            if pickedHarness != (self.app?.row(self.chatId)?.harness ?? harness), !self.canSwitchProvider { return }
            self.setConfig {
                if pickedHarness != $0.harness {
                    $0.harness = pickedHarness
                    // The old provider's effort and options don't carry over.
                    $0.reasoning = nil
                    $0.modelOptions = [:]
                }
                $0.model = model.id
            }
        }
    }

    /// The chat's provider, where it runs: Cloud's account for a Cloud chat,
    /// else the computer hosting it.
    func signInTarget() -> AgentAccountsViewController.Target? {
        guard let row = app?.row(chatId) else { return nil }
        let harness = row.harness ?? "claude-code"
        if row.isCloud {
            return CloudStore.agents.first { $0.harness == harness }.map { .cloud($0) }
        }
        guard !hostDevice.isEmpty, let app else { return nil }
        return .device(id: hostDevice, name: app.deviceName(hostDevice), harness: harness, label: row.harnessLabel ?? HarnessNames.label(harness))
    }

    /// The model a chat without one runs: the host's default for its
    /// provider. The built-in list answers at once; the host's (or Cloud's)
    /// own list replaces it when it arrives.
    private func defaultModel(for row: SessionRow) -> ModelPickerRow? {
        let harness = row.harness ?? "claude-code"
        let key = "\(hostDevice)|\(harness)"
        if defaultModels[key] == nil, let first = fallbackModels(harness: harness).first {
            defaultModels[key] = ModelPickerRow(id: first.id, label: first.label, detail: first.description)
        }
        if !hostDevice.isEmpty, defaultModelsLoading.insert(key).inserted {
            let device = hostDevice
            Task { @MainActor [weak self] in
                guard let self, let first = await self.client.listModels(deviceId: device, harness: harness).first else { return }
                let row = ModelPickerRow(id: first.id, label: first.label, detail: first.description)
                guard self.defaultModels[key] != row else { return }
                self.defaultModels[key] = row
                self.refresh()
            }
        }
        return defaultModels[key]
    }

    /// The providers a chat on this host can move to: Cloud runs Claude
    /// Code and Codex; a computer, the agents it offers.
    private func providers(cloud: Bool) -> [HarnessInfo] {
        // A computer offers what it reports; until it has, only the current
        // provider (a built-in guess could offer one it doesn't have).
        cloud
            ? fallbackHarnesses().filter { AppModel.cloudHarnesses.contains($0.id) }
            : hostHarnesses.filter(\.offered)
    }

    private func setConfig(_ change: (inout ChatConfig) -> Void) {
        var config = client.sessionConfig(chatId: chatId) ?? ChatConfig(harness: app?.row(chatId)?.harness ?? "claude-code", model: nil, reasoning: nil, modelOptions: [:], sandbox: .workspaceWrite)
        change(&config)
        try? client.setSessionConfig(chatId: chatId, config: config)
    }

    func searchFiles(_ query: String) async -> [FileMatch] {
        (try? await client.searchFiles(deviceId: hostDevice, chatId: chatId, spaceId: nil, query: query)) ?? []
    }

    private static let images = NSCache<NSString, UIImage>()

    func image(_ reference: String) async -> UIImage? {
        if let hit = Self.images.object(forKey: reference as NSString) { return hit }
        guard let data = try? await client.readAttachment(deviceId: hostDevice, path: reference),
              let image = await UIImage(data: data)?.byPreparingForDisplay()
        else { return nil }
        Self.images.setObject(image, forKey: reference as NSString)
        return image
    }

    func loadImage(_ reference: String, into view: UIImageView) {
        // Claim the view first: a slower load for a row it used to show
        // must not land on top of this one.
        view.accessibilityIdentifier = reference
        if let hit = Self.images.object(forKey: reference as NSString) {
            view.image = hit
            return
        }
        let device = hostDevice
        let client = self.client
        view.accessibilityIdentifier = reference
        Task.detached(priority: .userInitiated) {
            guard let data = try? await client.readAttachment(deviceId: device, path: reference),
                  let image = UIImage(data: data)?.preparingForDisplay()
            else { return }
            Self.images.setObject(image, forKey: reference as NSString)
            await MainActor.run {
                guard view.accessibilityIdentifier == reference else { return }
                UIView.transition(with: view, duration: 0.2, options: .transitionCrossDissolve) { view.image = image }
            }
        }
    }
}
