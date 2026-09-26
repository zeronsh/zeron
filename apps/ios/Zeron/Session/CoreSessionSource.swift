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
    private var hostDevice = ""

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
        handle.setViewAttached(attached: true)
    }

    func detach() {
        handle.setViewAttached(attached: false)
    }

    private func refresh() {
        let c = handle.composer()
        let row = app?.row(chatId)
        hostDevice = c.host.deviceId
        var next = SessionChrome()
        next.title = c.title
        let project = row?.project?.name ?? "No project"
        next.subtitle = c.host.name.map { "\(project) @ \($0)" } ?? project
        next.running = c.live.turnRunning
        next.canSteer = c.host.capabilities.midTurnSteering ?? false
        next.placeholder = "Message \(row?.harnessLabel ?? "the agent")"
        var chips: [ComposerChip] = []
        if let model = row?.modelLabel ?? row?.harnessLabel {
            chips.append(ComposerChip(id: "model", title: model, symbol: nil))
        }
        if let r = row?.reasoning, !r.isEmpty {
            chips.append(ComposerChip(id: "effort", title: reasoningLabel(level: r), symbol: "gauge.with.dots.needle.67percent"))
        }
        if let pr = row?.pullRequest {
            chips.append(ComposerChip(id: "pr", title: "#\(pr.number)", symbol: pr.state == .merged ? "arrow.triangle.merge" : "arrow.triangle.pull", tint: pr.state == .open ? Palette.success : pr.state == .merged ? UIColor(hex: 0x8250DF) : Palette.danger))
        } else if let b = row?.branch, !b.isEmpty {
            chips.append(ComposerChip(id: "branch", title: b, symbol: "arrow.triangle.branch"))
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
        if c.sendState == .failed {
            next.banner = .notDelivered
        } else if c.sendState == .queued {
            next.banner = .failed("\(c.host.name ?? "Host") is offline — will send when it's back")
        } else if let p = c.transferProgress {
            next.banner = .uploading(progress: p)
        } else if app?.connectivity?.state == .offline {
            next.banner = .offline
        } else if !c.room.connected, let retry = c.room.retryAtMs {
            next.banner = .reconnecting(in: max(1, Int((retry - Int64(Date().timeIntervalSince1970 * 1000)) / 1000)))
        }
        // Working state is shown at the transcript tail (layout engine), not here.
        if let input = c.openInput {
            next.questions = (input.requestId, input.questions.map {
                SessionChrome.Question(id: $0.id, header: $0.header, text: $0.question, options: $0.options, multiSelect: $0.multiSelect)
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

    func send(text: String, images: [StagedImage], mode: DeliveryMode) {
        do {
            if mode == .interrupt, chrome.running { try handle.interrupt() }
            _ = try handle.send(request: SendRequest(text: text, attachments: images.map(\.outgoing), worktree: nil, busy: mode == .steer ? .steer : .queue))
        } catch {
            chrome.banner = .failed("Couldn't send: \(error)")
            onChange?()
        }
    }

    func stop() {
        try? handle.interrupt()
    }

    func answer(requestId: String, answers: [(questionId: String, labels: [String])]) {
        try? handle.respondInput(requestId: requestId, answers: answers.map { UserInputAnswer(questionId: $0.questionId, labels: $0.labels) })
    }

    func queueAction(_ id: String, _ action: QueueAction) {
        switch action {
        case .sendNow: Task { _ = try? await handle.sendQueuedNow(id: id) }
        case .remove: Task { _ = try? await handle.removeQueued(id: id) }
        case .moveUp: _ = try? handle.moveQueuedBy(id: id, delta: -1)
        case .moveDown: _ = try? handle.moveQueuedBy(id: id, delta: 1)
        case .edit: break // Driven by the view: beginEdit / finishEdit.
        }
    }

    private var lease: QueueEditLease?
    private var renewal: Task<Void, Never>?

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
        return handle.composer().queue.first { $0.id == id }?.visibleText
    }

    func finishEdit(text: String?) async {
        renewal?.cancel()
        guard let lease else { return }
        self.lease = nil
        _ = await handle.finishQueuedEdit(lease: lease, action: text == nil ? .cancel : .commit, text: text)
    }

    func retryDelivery() {
        try? handle.retryDelivery()
    }

    func chipMenu(_ id: String) -> UIMenu? {
        guard let row = app?.row(chatId) else { return nil }
        let harness = row.harness ?? "claude-code"
        switch id {
        case "model":
            return UIMenu(title: "Model", children: [UIDeferredMenuElement { [weak self] done in
                guard let self else { return done([]) }
                Task {
                    let models = (try? await self.client.listModels(deviceId: self.hostDevice, harness: harness)) ?? fallbackModels(harness: harness)
                    done(models.map { m in
                        UIAction(title: m.label, subtitle: m.description, state: m.id == row.model ? .on : .off) { [weak self] _ in
                            self?.setConfig { $0.model = m.id }
                        }
                    })
                }
            }])
        case "effort":
            return UIMenu(title: "Reasoning effort", children: [UIDeferredMenuElement { [weak self] done in
                guard let self else { return done([]) }
                Task {
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

    private func setConfig(_ change: (inout ChatConfig) -> Void) {
        var config = client.sessionConfig(chatId: chatId) ?? ChatConfig(harness: app?.row(chatId)?.harness ?? "claude-code", model: nil, reasoning: nil, modelOptions: [:], sandbox: .workspaceWrite)
        change(&config)
        try? client.setSessionConfig(chatId: chatId, config: config)
    }

    private static let images = NSCache<NSString, UIImage>()

    func loadImage(_ reference: String, into view: UIImageView) {
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
