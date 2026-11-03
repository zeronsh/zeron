import UIKit

/// A chat's terminal: a shell on the chat's host, in its folder. The title
/// menu switches between the chat's terminals (and opens another); the key
/// bar rides the keyboard; connection, exit and reconnect states show on a
/// glass pill at the bottom.
final class TerminalViewController: UIViewController {
    private let app: AppModel
    let chatId: String
    private(set) var current: TerminalTab?
    private var terminal: TerminalView?
    private let keys = TerminalKeyBar()
    private let status = TerminalStatusPill()
    private let keyboardButton = Glass.circleButton(symbol: "keyboard", size: 48, pointSize: 18)
    private let loader = StatusGlyph(.spinner)
    private var tabToken: AnyObject?
    private var storeToken: AnyObject?
    /// A screenshot script to type once the shell is up (DEBUG launch arg).
    var script: [String] = []

    static let background = Palette.dynamic(0xFAFAFB, 0x090909)

    init(app: AppModel, chatId: String, tab: TerminalTab?) {
        self.app = app
        self.chatId = chatId
        self.current = tab
        super.init(nibName: nil, bundle: nil)
        hidesBottomBarWhenPushed = true
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Self.background
        navigationItem.largeTitleDisplayMode = .never
        navigationItem.title = "Terminal"
        let add = UIBarButtonItem(image: UIImage(systemName: "plus"), primaryAction: UIAction { [weak self] _ in self?.openAnother() })
        add.accessibilityLabel = "New terminal"
        add.accessibilityIdentifier = "terminal-new"
        let more = UIBarButtonItem(image: UIImage(systemName: "ellipsis"), menu: menu())
        more.accessibilityIdentifier = "terminal-menu"
        navigationItem.rightBarButtonItems = [more, add]
        navigationItem.titleMenuProvider = { [weak self] _ in self?.tabsMenu() }

        keys.onAction = { [weak self] action in self?.perform(action) }

        loader.translatesAutoresizingMaskIntoConstraints = false
        loader.transform = CGAffineTransform(scaleX: 1.75, y: 1.75)
        view.addSubview(loader)

        status.translatesAutoresizingMaskIntoConstraints = false
        status.onAction = { [weak self] in self?.statusAction() }
        view.addSubview(status)

        keyboardButton.addAction(UIAction { [weak self] _ in _ = self?.terminal?.becomeFirstResponder() }, for: .touchUpInside)
        keyboardButton.accessibilityLabel = "Show keyboard"
        keyboardButton.accessibilityIdentifier = "terminal-keyboard"
        view.addSubview(keyboardButton)

        view.keyboardLayoutGuide.usesBottomSafeArea = false
        NSLayoutConstraint.activate([
            loader.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            loader.centerYAnchor.constraint(equalTo: view.safeAreaLayoutGuide.centerYAnchor, constant: -40),
            loader.widthAnchor.constraint(equalToConstant: 12),
            loader.heightAnchor.constraint(equalToConstant: 12),
            status.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            status.bottomAnchor.constraint(lessThanOrEqualTo: view.keyboardLayoutGuide.topAnchor, constant: -12),
            status.bottomAnchor.constraint(lessThanOrEqualTo: view.safeAreaLayoutGuide.bottomAnchor, constant: -12),
            keyboardButton.trailingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.trailingAnchor, constant: -16),
            keyboardButton.bottomAnchor.constraint(equalTo: view.safeAreaLayoutGuide.bottomAnchor, constant: -12),
        ])
        status.isHidden = true
        storeToken = app.terminals.observe { [weak self] chat in
            guard let self, chat == self.chatId else { return }
            self.syncTitle()
        }
        if let current { show(current) }
    }

    override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        // Pair with the host while the screen slides in.
        app.client?.warmDirect(chatId: chatId)
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        if current == nil, !opening { openAnother() }
        // The keyboard comes up with the terminal on a phone; beside other
        // columns it waits for a tap.
        if traitCollection.horizontalSizeClass == .compact || splitViewController == nil {
            _ = terminal?.becomeFirstResponder()
        }
    }

    // MARK: Tabs

    private var opening = false

    private func openAnother() {
        guard !opening else { return }
        opening = true
        loader.alpha = 1
        let cols = terminal?.grid.cols ?? TerminalViewController.guessGrid(view.bounds.size).cols
        let rows = terminal?.grid.rows ?? TerminalViewController.guessGrid(view.bounds.size).rows
        Task { @MainActor in
            defer { opening = false }
            do {
                let tab = try await app.terminals.open(chatId: chatId, cols: cols, rows: rows)
                UIImpactFeedbackGenerator(style: .light).impactOccurred()
                show(tab)
                _ = terminal?.becomeFirstResponder()
                runScript()
            } catch {
                loader.alpha = 0
                status.isHidden = false
                status.show(.failed(Self.describe(error)))
            }
        }
    }

    private func runScript() {
        guard !script.isEmpty, let tab = current else { return }
        let lines = script
        script = []
        Task { @MainActor in
            for line in lines {
                try? await Task.sleep(for: .milliseconds(900))
                for ch in line {
                    tab.session.typeText(text: String(ch), modifiers: KeyModifiers(ctrl: false, alt: false, shift: false))
                    try? await Task.sleep(for: .milliseconds(45))
                }
                tab.session.press(key: .enter, modifiers: KeyModifiers(ctrl: false, alt: false, shift: false))
                try? await Task.sleep(for: .milliseconds(2400))
            }
        }
    }

    static func describe(_ error: Error) -> String {
        if case let CoreError.Unsupported(message) = error, !message.isEmpty {
            return "This computer needs a newer Zeron for terminals"
        }
        if case CoreError.HostUnavailable = error { return "The computer isn't reachable right now" }
        return "Couldn't open a terminal"
    }

    static func guessGrid(_ size: CGSize) -> (cols: UInt16, rows: UInt16) {
        let m = TerminalMetrics(size: TerminalView.defaultFontSize)
        let w = max(320, size.width) - TerminalView.inset.left - TerminalView.inset.right
        let h = max(400, size.height * 0.55)
        return (UInt16(max(20, floor(w / m.cellWidth))), UInt16(max(8, floor(h / m.lineHeight))))
    }

    func show(_ tab: TerminalTab) {
        let wasFocused = terminal?.isFirstResponder ?? false
        terminal?.removeFromSuperview()
        current = tab
        let t = TerminalView(tab: tab)
        t.translatesAutoresizingMaskIntoConstraints = false
        t.modifiers = { [weak self] in self?.keys.modifiers ?? KeyModifiers(ctrl: false, alt: false, shift: false) }
        t.consumeModifiers = { [weak self] in self?.keys.consume() }
        t.onAddToMessage = { [weak self] text in self?.addToMessage(text) }
        t.onFirstResponderChange = { [weak self] focused in self?.setKeyboardButtonVisible(!focused) }
        view.insertSubview(t, at: 0)
        NSLayoutConstraint.activate([
            t.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor),
            t.leadingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.leadingAnchor),
            t.trailingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.trailingAnchor),
            t.bottomAnchor.constraint(equalTo: view.keyboardLayoutGuide.topAnchor),
        ])
        t.accessory = keys
        terminal = t
        tabToken = tab.observe { [weak self] in self?.syncState() }
        syncTitle()
        syncState()
        if wasFocused { _ = t.becomeFirstResponder() }
        setKeyboardButtonVisible(!t.isFirstResponder)
    }

    private func tabsMenu() -> UIMenu {
        let tabs = app.terminals.tabs(for: chatId)
        let items: [UIMenuElement] = tabs.map { t in
            UIAction(title: t.name, subtitle: t.folder + (t.isLive ? "" : " · ended"), image: UIImage(systemName: "apple.terminal"), state: t === current ? .on : .off) { [weak self] _ in
                self?.show(t)
            }
        }
        let new = UIAction(title: "New Terminal", image: UIImage(systemName: "plus")) { [weak self] _ in self?.openAnother() }
        return UIMenu(children: [UIMenu(options: .displayInline, children: items), new])
    }

    private func syncTitle() {
        guard let tab = current else { return }
        navigationItem.title = tab.name
        let host = app.deviceName(tab.ref.deviceId)
        navigationItem.subtitle = "\(tab.folder) · \(host)"
    }

    private var shownPhase: TerminalPhase?

    private func syncState() {
        guard let tab = current else { return }
        let phase = tab.phase
        if phase == shownPhase { return }
        shownPhase = phase
        syncTitle()
        let animate = view.window != nil
        UIView.animate(withDuration: animate ? 0.25 : 0) {
            switch phase {
            case .connecting:
                self.loader.alpha = (self.terminal?.frameModel?.lines.contains { !$0.runs.isEmpty } ?? false) ? 0 : 1
                self.status.isHidden = true
            case .live:
                self.loader.alpha = 0
                self.status.isHidden = true
            case .reconnecting:
                self.loader.alpha = 0
                self.status.isHidden = false
                self.status.show(.reconnecting)
            case let .exited(code, signal):
                self.loader.alpha = 0
                self.status.isHidden = false
                self.status.show(.exited(signal.map { "Ended by \($0)" } ?? (code == 0 ? "Process exited" : "Exited with code \(code)")))
                if self.terminal?.isFirstResponder == true { _ = self.terminal?.resignFirstResponder() }
            case .gone:
                self.loader.alpha = 0
                self.status.isHidden = false
                self.status.show(.exited("This terminal ended on its host"))
            }
        }
    }

    private func statusAction() {
        guard let tab = current else { return openAnother() }
        switch tab.phase {
        case .exited, .gone:
            app.terminals.forget(tab)
            openAnother()
        default:
            break
        }
    }

    private func setKeyboardButtonVisible(_ visible: Bool) {
        let ended = current.map { !$0.isLive } ?? false
        let show = visible && !ended
        UIView.animate(withDuration: 0.3, delay: 0, usingSpringWithDamping: 0.8, initialSpringVelocity: 0, options: [.beginFromCurrentState, .allowUserInteraction]) {
            self.keyboardButton.alpha = show ? 1 : 0
            self.keyboardButton.transform = show ? .identity : CGAffineTransform(scaleX: 0.6, y: 0.6)
        }
    }

    // MARK: Keys

    private func perform(_ action: TerminalKeyBar.Action) {
        guard let terminal else { return }
        switch action {
        case let .key(k): terminal.press(k)
        case let .text(t): terminal.insertText(t)
        case .paste: terminal.pasteFromClipboard()
        case .dismiss: _ = terminal.resignFirstResponder()
        }
    }

    override var keyCommands: [UIKeyCommand]? {
        [
            UIKeyCommand(title: "New Terminal", action: #selector(newTerminalCommand), input: "t", modifierFlags: .command),
            UIKeyCommand(title: "Clear", action: #selector(clearCommand), input: "k", modifierFlags: .command),
            UIKeyCommand(title: "Bigger", action: #selector(biggerCommand), input: "+", modifierFlags: .command),
            UIKeyCommand(title: "Smaller", action: #selector(smallerCommand), input: "-", modifierFlags: .command),
        ]
    }

    @objc private func newTerminalCommand() { openAnother() }
    @objc private func clearCommand() { current?.session.typeText(text: "l", modifiers: KeyModifiers(ctrl: true, alt: false, shift: false)) }
    @objc private func biggerCommand() { terminal.map { $0.setFontSize(min(22, $0.fontSize + 1)) } }
    @objc private func smallerCommand() { terminal.map { $0.setFontSize(max(8, $0.fontSize - 1)) } }

    // MARK: Menu

    private func menu() -> UIMenu {
        UIMenu(children: [UIDeferredMenuElement.uncached { [weak self] done in
            guard let self, let tab = self.current else { return done([]) }
            let size = UIMenu(title: "Text Size", options: .displayInline, preferredElementSize: .small, children: [
                UIAction(title: "Smaller", image: UIImage(systemName: "textformat.size.smaller")) { [weak self] _ in self?.smallerCommand() },
                UIAction(title: "Reset", image: UIImage(systemName: "textformat.size")) { [weak self] _ in self?.terminal?.setFontSize(TerminalView.defaultFontSize) },
                UIAction(title: "Bigger", image: UIImage(systemName: "textformat.size.larger")) { [weak self] _ in self?.biggerCommand() },
            ])
            done([
                size,
                UIAction(title: "Add Output to Message", image: UIImage(systemName: "text.bubble")) { [weak self] _ in
                    guard let self else { return }
                    self.addToMessage(tab.session.text(all: false))
                },
                UIAction(title: "Copy All Output", image: UIImage(systemName: "doc.on.doc")) { _ in
                    UIPasteboard.general.string = tab.session.text(all: true)
                    Toast.show("Copied", in: self.view.window)
                },
                UIAction(title: "Clear Screen", image: UIImage(systemName: "clear")) { [weak self] _ in self?.clearCommand() },
                UIMenu(options: .displayInline, children: [
                    UIAction(title: "Close Terminal", image: UIImage(systemName: "xmark.circle"), attributes: .destructive) { [weak self] _ in self?.closeCurrent() },
                ]),
            ])
        }])
    }

    private func closeCurrent() {
        guard let tab = current else { return }
        app.terminals.close(tab)
        if let next = app.terminals.tabs(for: chatId).first {
            show(next)
        } else {
            leave()
        }
    }

    private func leave() {
        if let nav = navigationController, nav.viewControllers.count > 1 {
            nav.popViewController(animated: true)
        } else {
            (splitViewController as? SplitRootController)?.closeCompanion(self)
        }
    }

    /// Output into the chat's draft, as a fenced block the agent can read.
    private func addToMessage(_ text: String) {
        let trimmed = text.trimmingCharacters(in: .newlines)
        guard !trimmed.isEmpty else { return }
        let block = "```\n\(trimmed)\n```\n"
        if let session = SessionViewController.live(chatId) {
            session.appendToComposer(block)
        } else {
            let draft = Drafts.load(chatId)
            Drafts.save(chatId, draft.isEmpty ? block : draft + "\n" + block)
        }
        UINotificationFeedbackGenerator().notificationOccurred(.success)
        Toast.show("Added to message", in: view.window)
    }
}

/// The terminal's connection / exit state, on glass.
final class TerminalStatusPill: UIControl {
    enum State {
        case reconnecting
        case exited(String)
        case failed(String)
    }

    var onAction: (() -> Void)?
    private let glass = Glass.surface(interactive: true)
    private let glyph = StatusGlyph(.spinner)
    private let label = UILabel()
    private let action = UILabel()

    override init(frame: CGRect) {
        super.init(frame: frame)
        glass.isUserInteractionEnabled = false
        glass.translatesAutoresizingMaskIntoConstraints = false
        addSubview(glass)
        label.font = Fonts.ui(.sansMedium, 14)
        label.textColor = Palette.text
        action.font = Fonts.ui(.sansSemibold, 14)
        action.textColor = Palette.accent
        let row = UIStackView(arrangedSubviews: [glyph, label, action])
        row.spacing = 9
        row.alignment = .center
        row.translatesAutoresizingMaskIntoConstraints = false
        glass.contentView.addSubview(row)
        NSLayoutConstraint.activate([
            glass.topAnchor.constraint(equalTo: topAnchor),
            glass.bottomAnchor.constraint(equalTo: bottomAnchor),
            glass.leadingAnchor.constraint(equalTo: leadingAnchor),
            glass.trailingAnchor.constraint(equalTo: trailingAnchor),
            heightAnchor.constraint(equalToConstant: 40),
            glyph.widthAnchor.constraint(equalToConstant: 12),
            glyph.heightAnchor.constraint(equalToConstant: 12),
            row.leadingAnchor.constraint(equalTo: glass.contentView.leadingAnchor, constant: 16),
            row.trailingAnchor.constraint(equalTo: glass.contentView.trailingAnchor, constant: -16),
            row.centerYAnchor.constraint(equalTo: glass.contentView.centerYAnchor),
        ])
        addAction(UIAction { [weak self] _ in self?.onAction?() }, for: .touchUpInside)
        accessibilityIdentifier = "terminal-status"
    }

    required init?(coder: NSCoder) { fatalError() }

    func show(_ state: State) {
        switch state {
        case .reconnecting:
            glyph.kind = .spinner
            label.text = "Reconnecting…"
            action.isHidden = true
        case let .exited(text):
            glyph.kind = .dot(StatusTone.idle)
            label.text = text
            action.text = "New Terminal"
            action.isHidden = false
        case let .failed(text):
            glyph.kind = .dot(Palette.danger)
            label.text = text
            action.text = "Try Again"
            action.isHidden = false
        }
        accessibilityLabel = [label.text, action.isHidden ? nil : action.text].compactMap { $0 }.joined(separator: ", ")
    }
}
