import UIKit

/// Composer-facing state of a session (the transcript itself flows Rust→Rust
/// into the layout engine and never crosses FFI as rows).
struct SessionChrome: Equatable {
    enum Banner: Equatable {
        case none
        case working(since: Date?, word: String)
        case offline
        case reconnecting(in: Int)
        case notDelivered
        case uploading(progress: Double)
        case failed(String)
    }

    struct Question: Equatable {
        let id: String
        let header: String
        let text: String
        let options: [String]
        let multiSelect: Bool
    }

    struct QueuedItem: Equatable {
        let id: String
        let text: String
        let thumbnail: UIImage?
        let gate: String?
    }

    var title = ""
    var subtitle = ""
    var running = false
    var canSteer = false
    var placeholder = "Message"
    var chips: [ComposerChip] = []
    var banner: Banner = .none
    var questions: (requestId: String, items: [Question])?
    var queue: [QueuedItem] = []
    var error: String?

    static func == (a: SessionChrome, b: SessionChrome) -> Bool {
        a.title == b.title && a.subtitle == b.subtitle && a.running == b.running && a.canSteer == b.canSteer
            && a.placeholder == b.placeholder && a.chips == b.chips && a.banner == b.banner
            && a.questions?.requestId == b.questions?.requestId && a.questions?.items == b.questions?.items
            && a.queue == b.queue && a.error == b.error
    }
}

/// A session's data + commands, independent of where it comes from.
protocol SessionSource: AnyObject {
    var chrome: SessionChrome { get }
    var onChange: (() -> Void)? { get set }
    /// Bind the Rust layout engine to this session's transcript.
    func attach(_ engine: TranscriptView)
    func detach()
    func send(text: String, images: [StagedImage], mode: DeliveryMode)
    func stop()
    func answer(requestId: String, answers: [(questionId: String, labels: [String])])
    func queueAction(_ id: String, _ action: QueueAction)
    func retryDelivery()
    func chipTapped(_ id: String, from view: UIView, in vc: UIViewController)
    func loadImage(_ reference: String, into view: UIImageView)
}

enum QueueAction {
    case sendNow
    case edit
    case moveUp
    case moveDown
    case remove
}

/// Scripted source over fixture markdown (lab + UI tests until the core's
/// demo session lands).
final class FixtureSessionSource: SessionSource {
    private(set) var chrome = SessionChrome()
    var onChange: (() -> Void)?
    private weak var engine: TranscriptView?
    private var entries: [DebugEntry] = []
    private var timer: Timer?
    private let fixture = layoutFixtureMarkdown()

    init(title: String, subtitle: String) {
        chrome.title = title
        chrome.subtitle = subtitle
        chrome.placeholder = "Message Claude"
        chrome.chips = [
            ComposerChip(id: "model", title: "Opus 4.5", symbol: nil),
            ComposerChip(id: "effort", title: "High", symbol: "gauge.with.dots.needle.67percent"),
            ComposerChip(id: "branch", title: "ios-rewrite", symbol: "arrow.triangle.branch"),
        ]
        for i in 0..<3 {
            entries.append(DebugEntry(id: "u\(i)", user: true, text: TranscriptLabViewController.prompts[i], streaming: false))
            entries.append(DebugEntry(id: "a\(i)", user: false, text: fixture, streaming: false))
        }
    }

    func attach(_ engine: TranscriptView) {
        self.engine = engine
        engine.setDebugEntries(entries: entries, working: chrome.running)
    }

    func detach() {
        timer?.invalidate()
    }

    private func update(_ change: (inout SessionChrome) -> Void) {
        let old = chrome
        change(&chrome)
        if chrome != old { onChange?() }
    }

    func send(text: String, images: [StagedImage], mode: DeliveryMode) {
        if chrome.running, mode == .queue {
            update { $0.queue.append(.init(id: UUID().uuidString, text: text, thumbnail: images.first?.thumbnail, gate: nil)) }
            return
        }
        timer?.invalidate()
        let n = entries.count
        entries.append(DebugEntry(id: "u\(n)", user: true, text: text, streaming: false))
        entries.append(DebugEntry(id: "a\(n)", user: false, text: "", streaming: true))
        update {
            $0.running = true
            $0.banner = .working(since: Date(), word: "Thinking")
        }
        let words = fixture.split(separator: " ", omittingEmptySubsequences: false).map(String.init)
        var i = 0
        engine?.setDebugEntries(entries: entries, working: true)
        timer = Timer.scheduledTimer(withTimeInterval: 0.05, repeats: true) { [weak self] t in
            guard let self else { return t.invalidate() }
            let step = Int.random(in: 2...4)
            let next = words[i..<min(words.count, i + step)].joined(separator: " ")
            i += step
            var last = self.entries[self.entries.count - 1]
            last.text += (last.text.isEmpty ? "" : " ") + next
            last.streaming = i < words.count
            self.entries[self.entries.count - 1] = last
            self.engine?.setDebugEntries(entries: self.entries, working: last.streaming)
            if !last.streaming { self.finish() }
        }
    }

    private func finish() {
        timer?.invalidate()
        update {
            $0.running = false
            $0.banner = .none
        }
        if let next = chrome.queue.first {
            update { $0.queue.removeFirst() }
            send(text: next.text, images: [], mode: .queue)
        }
    }

    func stop() {
        guard var last = entries.last, last.streaming else { return }
        last.streaming = false
        entries[entries.count - 1] = last
        engine?.setDebugEntries(entries: entries, working: false)
        timer?.invalidate()
        update {
            $0.running = false
            $0.banner = .none
        }
    }

    func answer(requestId: String, answers: [(questionId: String, labels: [String])]) {
        update { $0.questions = nil }
    }

    func queueAction(_ id: String, _ action: QueueAction) {
        update { chrome in
            guard let i = chrome.queue.firstIndex(where: { $0.id == id }) else { return }
            switch action {
            case .remove: chrome.queue.remove(at: i)
            case .moveUp where i > 0: chrome.queue.swapAt(i, i - 1)
            case .moveDown where i + 1 < chrome.queue.count: chrome.queue.swapAt(i, i + 1)
            default: break
            }
        }
    }

    func retryDelivery() {}
    func chipTapped(_ id: String, from view: UIView, in vc: UIViewController) {}
    func loadImage(_ reference: String, into view: UIImageView) {}
}
