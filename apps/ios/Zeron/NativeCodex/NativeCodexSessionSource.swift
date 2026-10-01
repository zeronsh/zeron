import UIKit

/// Adapts the embedded agent to Zeron's shared transcript and composer.
@MainActor
final class NativeCodexSessionSource: SessionSource {
    private let session = NativeCodexSession.shared
    let conversationId: String
    var onChange: (() -> Void)?
    var onRefresh: (() -> Void)?
    var onAccount: (() -> Void)?
    private weak var engine: TranscriptView?
    private var failure: String?
    private(set) var chrome = SessionChrome()

    init(conversationId: String? = nil) {
        self.conversationId = conversationId ?? session.conversation.id
        refresh()
    }

    func attach(_ engine: TranscriptView) {
        self.engine = engine
        reattach()
        session.start()
    }
    func reattach() {
        session.onChange = { [weak self] in self?.refresh() }
        refresh()
    }
    func detach() { engine = nil }

    private func refresh() {
        guard let conversation = session.conversation.id == conversationId ? session.conversation : session.conversations.first(where: { $0.id == conversationId }) else { return }
        var next = SessionChrome()
        next.title = conversation.title
        next.subtitle = (conversation.workspaceName.map { $0 + " · " } ?? "") + "Native Codex @ This iPhone"
        next.running = session.running && session.conversation.id == conversationId
        next.loadingProgress = session.loadingProgress
        next.loadingLabel = "Loading Native Codex…"
        next.placeholder = session.signedIn ? "Message Native Codex" : "Sign in with ChatGPT to start"
        next.chips = [ComposerChip(id: "provider", title: "Native Codex", symbol: "iphone")] + session.settingsChips(draft: false)
        if !session.signedIn {
            next.chips.append(ComposerChip(id: "account", title: session.preparingLogin ? "Signing in…" : "Sign in with ChatGPT", symbol: "person.crop.circle"))
        }
        let ordinaryStatus = ["Starting Codex…", "Sign in to ChatGPT to start", "Preparing ChatGPT sign-in…", "Signed out of Codex", "Sign-in cancelled", "Interrupted", "Local workspace · OpenAI model", "On this iPhone · files saved", "Signed in to Codex"]
        if let failure { next.banner = .failed(failure) }
        else if !session.running && session.login == nil && !ordinaryStatus.contains(session.status) { next.banner = .failed(session.status) }
        chrome = next
        var messages = conversation.messages
        if let prompt = conversation.pendingPrompt { messages.append(.init(id: "pending-first-message", user: true, text: prompt)) }
        engine?.setLocalEntries(entries: messages.map {
            $0.transcriptEntry(streaming: next.running && $0.id == messages.last?.id && !$0.user, working: next.running)
        }, working: next.running)
        onChange?()
        onRefresh?()
    }

    func send(text: String, images: [StagedImage], mode: DeliveryMode) -> Bool {
        guard session.conversation.id == conversationId else {
            failure = "Another Native Codex conversation is active. Reopen this chat after it finishes."
            refresh(); return false
        }
        guard !session.workspaceBusy else { failure = "Wait for the workspace import or export to finish"; refresh(); return false }
        guard images.isEmpty else { failure = "Import text files from the attachment menu"; refresh(); return false }
        guard session.signedIn else { onAccount?(); return false }
        guard !session.running else { failure = "Stop the current turn before sending another message"; refresh(); return false }
        failure = nil
        let accepted = session.send(text)
        if !accepted { failure = session.status; refresh() }
        return accepted
    }
    func stop() { guard session.conversation.id == conversationId else { return }; failure = nil; session.stop() }
    func chipMenu(_ id: String) -> UIMenu? {
        if id == "account" {
            return UIMenu(children: [UIAction(title: "Sign in with ChatGPT") { [weak self] _ in self?.onAccount?() }])
        }
        return session.settingsMenu(id, draft: false)
    }
    func searchFiles(_ query: String) async -> [FileMatch] {
        guard session.conversation.id == conversationId, !session.running, let result = try? await session.shell.execute("find /workspace -type f") else { return [] }
        return result.stdout.split(separator: "\n").map(String.init).filter { query.isEmpty || $0.localizedCaseInsensitiveContains(query) }.prefix(30).map { FileMatch(path: $0, isDir: false) }
    }
    func answer(requestId: String, answers: [(questionId: String, labels: [String])]) {}
    func queueAction(_ id: String, _ action: QueueAction) {}
    func retryDelivery() { session.start() }
    func beginEdit(_ id: String) async -> String? { nil }
    func finishEdit(text: String?) async {}
    func loadImage(_ reference: String, into view: UIImageView) {}
}
