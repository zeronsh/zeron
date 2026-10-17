import UIKit

/// A subagent's transcript, read-only: the doc a spawn card links to. A
/// running subagent streams live from its parent's host; a finished one
/// reads its frozen copy. No composer — the parent agent drives it.
final class SubagentViewController: UIViewController {
    private let app: AppModel
    let docId: String
    private let subagentTitle: String
    private let frozen: Bool
    /// The chat whose run spawned this subagent (nested spawns included).
    private let parentChatId: String
    private var handle: SessionHandle?
    private lazy var relay = FrameRelay { [weak self] in self?.applyFrame() }
    private lazy var engine = TranscriptView(text: TextEngine.shared, listener: relay)
    private lazy var list = TranscriptListView(engine: engine)
    private let titleView = SessionTitleView()
    private let loader = StatusGlyph(.spinner)
    private let empty = UILabel()
    private var revealed = false
    private var closed = false

    init(app: AppModel, docId: String, title: String, frozen: Bool) {
        self.app = app
        self.docId = docId
        self.subagentTitle = title
        self.frozen = frozen
        self.parentChatId = docId.components(separatedBy: "--sub--").first ?? docId
        super.init(nibName: nil, bundle: nil)
        hidesBottomBarWhenPushed = true
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Palette.background
        navigationItem.largeTitleDisplayMode = .never
        navigationItem.titleView = titleView
        let parent = app.row(parentChatId)?.title
        titleView.set(title: subagentTitle, subtitle: parent.map { "Subagent · \($0)" } ?? "Subagent")
        navigationItem.rightBarButtonItem = UIBarButtonItem(image: UIImage(systemName: "ellipsis"), menu: UIMenu(children: [
            UIAction(title: "Copy Transcript", image: UIImage(systemName: "doc.on.doc")) { [weak self] _ in
                guard let self else { return }
                UIPasteboard.general.string = self.engine.frame().plainText()
            },
        ]))

        list.frame = view.bounds
        list.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        list.accessibilityIdentifier = "subagent-transcript"
        list.alpha = 0
        let host = app.row(parentChatId)?.deviceId ?? ""
        let client = app.client
        list.imageLoader = { ref, iv in
            iv.accessibilityIdentifier = ref
            Task { @MainActor in
                guard let data = try? await client?.readAttachment(deviceId: host, path: ref),
                      let image = await UIImage(data: data)?.byPreparingForDisplay(),
                      iv.accessibilityIdentifier == ref
                else { return }
                iv.image = image
            }
        }
        list.imageFetcher = { ref in
            guard let data = try? await client?.readAttachment(deviceId: host, path: ref) else { return nil }
            return await UIImage(data: data)?.byPreparingForDisplay()
        }
        // A subagent can spawn its own: they push on top, as from the chat.
        list.onOpenSubagent = { [weak self] docId, title, frozen in
            guard let self else { return }
            self.navigationController?.pushViewController(SubagentViewController(app: self.app, docId: docId, title: title, frozen: frozen), animated: true)
        }
        view.addSubview(list)
        setContentScrollView(list, for: .top)
        list.topEdgeEffect.style = .soft

        loader.translatesAutoresizingMaskIntoConstraints = false
        loader.transform = CGAffineTransform(scaleX: 1.75, y: 1.75)
        loader.accessibilityIdentifier = "subagent-loading"
        view.addSubview(loader)
        empty.text = "Nothing from this subagent yet."
        empty.font = Fonts.ui(.sans, 15)
        empty.textColor = Palette.secondary
        empty.alpha = 0
        empty.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(empty)
        NSLayoutConstraint.activate([
            loader.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            loader.centerYAnchor.constraint(equalTo: view.safeAreaLayoutGuide.centerYAnchor, constant: -40),
            loader.widthAnchor.constraint(equalToConstant: 12),
            loader.heightAnchor.constraint(equalToConstant: 12),
            empty.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            empty.centerYAnchor.constraint(equalTo: view.safeAreaLayoutGuide.centerYAnchor, constant: -40),
        ])

        Task { @MainActor [weak self] in
            guard let self else { return }
            guard let handle = await self.app.openSubagent(docId: self.docId, frozen: self.frozen) else {
                return self.showEmpty("Couldn't open this subagent.")
            }
            // Popped while the frozen copy was on its way.
            guard !self.closed, let client = self.app.client else { return }
            self.handle = handle
            handle.setViewAttached(attached: true)
            _ = self.engine.attach(client: client, chatId: self.docId)
            // Nothing laid out (an empty or unreachable doc): say so eventually
            // rather than spin forever.
            DispatchQueue.main.asyncAfter(deadline: .now() + 8) { [weak self] in
                guard let self, !self.revealed else { return }
                self.showEmpty("Nothing from this subagent yet.")
            }
        }
    }

    override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        (tabBarController as? MainTabController)?.setAccessoryVisible(false, animated: false)
        if closed {
            closed = false
            handle?.setViewAttached(attached: true)
        }
    }

    override func viewDidDisappear(_ animated: Bool) {
        super.viewDidDisappear(animated)
        guard isMovingFromParent || navigationController == nil else { return }
        closed = true
        engine.close()
        if handle != nil { app.client?.closeSession(chatId: docId) }
    }

    private func applyFrame() {
        let frame = engine.frame()
        list.apply(frame)
        guard !revealed, frame.rowCount() > 0 else { return }
        revealed = true
        list.settleEdgeEffect()
        let reveal = {
            self.list.alpha = 1
            self.loader.alpha = 0
            self.empty.alpha = 0
        }
        if UIAccessibility.isReduceMotionEnabled { return reveal() }
        UIView.animate(withDuration: 0.18, delay: 0, options: [.curveEaseOut, .allowUserInteraction], animations: reveal)
    }

    private func showEmpty(_ text: String) {
        empty.text = text
        UIView.animate(withDuration: 0.18) {
            self.loader.alpha = 0
            self.empty.alpha = 1
        }
    }
}
