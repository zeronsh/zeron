import UIKit

/// A session's notes: one Markdown scratchpad per main chat, synced through
/// the chat doc like the transcript, so the desktop and every other device
/// edit the same text. Writes are compare-and-set against the notes this
/// screen last saw; a change from another device merges into the editor in
/// place, keeping the caret and both sides' typing. Snapshots are versioned:
/// the watch can report this screen's own write back after a newer one, and
/// such a stale report is dropped, while any newer state merges, even one
/// that restores text this screen wrote before (`notesSupersedes`).
final class NotesViewController: UIViewController, UITextViewDelegate {
    private let handle: SessionHandle
    private let textView = UITextView(usingTextLayoutManager: true)
    private let placeholder = UILabel()
    private let errorLabel = UILabel()
    /// The notes as last read from or written to the doc: the next write's base.
    private var base: NotesSnapshot?
    private var subscription: NotesSubscription?
    private var writeTimer: Timer?
    private var restylePending = false
    private var styledLine: NSRange?
    private var backgroundObserver: NSObjectProtocol?

    private let bodySize = UIFontMetrics(forTextStyle: .body).scaledValue(for: 16.5)
    private var bodyFont: UIFont { Fonts.ui(.sans, bodySize) }
    private var baseAttributes: [NSAttributedString.Key: Any] {
        [.font: bodyFont, .foregroundColor: Palette.text]
    }

    init(handle: SessionHandle) {
        self.handle = handle
        super.init(nibName: nil, bundle: nil)
        title = "Notes"
        hidesBottomBarWhenPushed = true
    }

    required init?(coder: NSCoder) { fatalError() }

    deinit {
        subscription?.cancel()
        if let backgroundObserver { NotificationCenter.default.removeObserver(backgroundObserver) }
    }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Palette.background
        navigationItem.largeTitleDisplayMode = .never

        textView.translatesAutoresizingMaskIntoConstraints = false
        textView.backgroundColor = .clear
        textView.delegate = self
        textView.alwaysBounceVertical = true
        textView.keyboardDismissMode = .interactive
        textView.textContainerInset = UIEdgeInsets(top: 12, left: 16, bottom: 24, right: 16)
        textView.typingAttributes = baseAttributes
        textView.autocorrectionType = .default
        textView.smartDashesType = .no
        textView.smartQuotesType = .no
        // Nothing to edit until the doc says what is there.
        textView.isEditable = false
        textView.accessibilityIdentifier = "session-notes"
        view.addSubview(textView)

        placeholder.translatesAutoresizingMaskIntoConstraints = false
        placeholder.text = "Notes for this session. Markdown works here."
        placeholder.font = bodyFont
        placeholder.textColor = Palette.tertiary
        placeholder.numberOfLines = 0
        textView.addSubview(placeholder)

        errorLabel.translatesAutoresizingMaskIntoConstraints = false
        errorLabel.font = Fonts.ui(.sans, 13)
        errorLabel.textColor = Palette.danger
        errorLabel.numberOfLines = 0
        errorLabel.isHidden = true
        view.addSubview(errorLabel)

        view.keyboardLayoutGuide.usesBottomSafeArea = false
        NSLayoutConstraint.activate([
            textView.topAnchor.constraint(equalTo: view.topAnchor),
            textView.leadingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.leadingAnchor),
            textView.trailingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.trailingAnchor),
            textView.bottomAnchor.constraint(equalTo: errorLabel.topAnchor, constant: -4),
            errorLabel.leadingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.leadingAnchor, constant: 20),
            errorLabel.trailingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.trailingAnchor, constant: -20),
            errorLabel.bottomAnchor.constraint(lessThanOrEqualTo: view.keyboardLayoutGuide.topAnchor, constant: -8),
            errorLabel.bottomAnchor.constraint(lessThanOrEqualTo: view.safeAreaLayoutGuide.bottomAnchor, constant: -8),
            placeholder.topAnchor.constraint(equalTo: textView.topAnchor, constant: 12),
            placeholder.leadingAnchor.constraint(equalTo: textView.frameLayoutGuide.leadingAnchor, constant: 21),
            placeholder.trailingAnchor.constraint(equalTo: textView.frameLayoutGuide.trailingAnchor, constant: -21),
        ])

        // The app can be killed in the background: the notes go to disk first.
        backgroundObserver = NotificationCenter.default.addObserver(forName: UIApplication.didEnterBackgroundNotification, object: nil, queue: .main) { [weak self] _ in
            self?.write()
        }
        subscription = handle.watchNotes(listener: NotesRelay { [weak self] notes in
            DispatchQueue.main.async { self?.receive(notes) }
        })
    }

    override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)
        write()
    }

    // MARK: Sync

    private func receive(_ remote: NotesSnapshot) {
        guard let base else {
            textView.text = remote.text
            self.base = remote
            textView.isEditable = true
            updatePlaceholder()
            restyle()
            return
        }
        // Stale: a report of a state this screen has already moved past.
        guard notesSupersedes(snapshot: remote, base: base) else { return }
        if remote.hash == base.hash {
            self.base = remote
            return
        }
        merge(remote)
    }

    /// Fold another device's notes into the editor, keeping local edits not
    /// yet written.
    private func merge(_ remote: NotesSnapshot) {
        let local = textView.text ?? ""
        let merged = notesRebase(base: base?.text ?? "", local: local, remote: remote.text)
        if let replacement = notesTextReplacement(old: local, new: merged) {
            apply(replacement)
        }
        base = remote
        if merged != remote.text { scheduleWrite(after: 0) }
    }

    /// Replace one range of the text in place, carrying the selection across.
    private func apply(_ r: TextReplacement) {
        let range = NSRange(location: Int(r.location), length: Int(r.length))
        let inserted = (r.text as NSString).length
        let map = { (offset: Int) -> Int in
            if offset <= range.location { return offset }
            if offset >= NSMaxRange(range) { return offset - range.length + inserted }
            return range.location + inserted
        }
        let selection = textView.selectedRange
        let storage = textView.textStorage
        storage.beginEditing()
        storage.replaceCharacters(in: range, with: NSAttributedString(string: r.text, attributes: baseAttributes))
        storage.endEditing()
        let start = map(selection.location)
        let end = map(NSMaxRange(selection))
        textView.selectedRange = NSRange(location: start, length: max(0, end - start))
        // UIKit's undo steps hold ranges from before this change; replaying
        // one would land in the wrong place or revert the other device's text.
        textView.undoManager?.removeAllActions()
        updatePlaceholder()
        restyle()
    }

    private func scheduleWrite(after delay: TimeInterval) {
        writeTimer?.invalidate()
        writeTimer = Timer.scheduledTimer(withTimeInterval: delay, repeats: false) { [weak self] _ in
            self?.write()
        }
    }

    /// Writes land on this device's replica synchronously and reach the room
    /// through the durable outbox, online or not.
    private func write() {
        writeTimer?.invalidate()
        writeTimer = nil
        guard let base else { return }
        let text = textView.text ?? ""
        if text == base.text { return }
        do {
            switch try handle.setNotes(baseHash: base.hash, text: text) {
            case let .applied(hash, epoch, seq):
                self.base = NotesSnapshot(text: text, hash: hash, epoch: epoch, seq: seq)
                showError(nil)
            case let .conflict(current):
                // Writes land on the local replica synchronously, so a refused
                // one never landed: whatever is there merges.
                showError(nil)
                merge(current)
            case let .tooLarge(limit):
                showError("Notes are limited to \(limit / 1024) KB. The latest changes aren't saved.")
            }
        } catch {
            NSLog("notes write failed: \(error)")
            showError("Couldn't save the notes.")
            scheduleWrite(after: 2)
        }
    }

    private func showError(_ message: String?) {
        errorLabel.text = message
        errorLabel.isHidden = message == nil
    }

    // MARK: UITextViewDelegate

    func textViewDidChange(_ textView: UITextView) {
        updatePlaceholder()
        scheduleWrite(after: 0.75)
        restyle()
    }

    func textViewDidChangeSelection(_ textView: UITextView) {
        // Markers show on the caret's line only: restyle when it moves lines.
        let text = (textView.text ?? "") as NSString
        let line = text.lineRange(for: NSRange(location: min(textView.selectedRange.location, text.length), length: 0))
        if line != styledLine { restyle() }
    }

    func textViewDidEndEditing(_ textView: UITextView) {
        write()
    }

    private func updatePlaceholder() {
        placeholder.isHidden = !(textView.text ?? "").isEmpty
    }

    // MARK: Live preview

    /// Coalesced to one pass per run-loop turn.
    private func restyle() {
        guard !restylePending else { return }
        restylePending = true
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            self.restylePending = false
            self.applyStyles()
        }
    }

    /// Rust reads the Markdown (`zeron_markdown::live`, the desktop editor's
    /// rules); this only paints it. Attributes never change the text, so the
    /// notes stay plain Markdown.
    private func applyStyles() {
        let text = textView.text ?? ""
        let selection = textView.selectedRange
        styledLine = (text as NSString).lineRange(for: NSRange(location: min(selection.location, (text as NSString).length), length: 0))
        let styles = markdownLiveStyles(text: text, selectionLocation: UInt64(selection.location), selectionLength: UInt64(selection.length))
        let storage = textView.textStorage
        let full = NSRange(location: 0, length: storage.length)
        storage.beginEditing()
        storage.setAttributes(baseAttributes, range: full)
        for style in styles {
            let range = NSRange(location: Int(style.location), length: Int(style.length))
            guard NSMaxRange(range) <= storage.length else { continue }
            switch style.kind {
            case let .heading(level):
                let size = bodySize * (level == 1 ? 1.45 : level == 2 ? 1.25 : level == 3 ? 1.1 : 1)
                storage.addAttribute(.font, value: Fonts.ui(.sansSemibold, size), range: range)
            case .strong:
                storage.enumerateAttribute(.font, in: range) { value, sub, _ in
                    let size = (value as? UIFont)?.pointSize ?? self.bodySize
                    storage.addAttribute(.font, value: Fonts.ui(.sansSemibold, size), range: sub)
                }
            case .emphasis:
                storage.enumerateAttribute(.font, in: range) { value, sub, _ in
                    let size = (value as? UIFont)?.pointSize ?? self.bodySize
                    storage.addAttribute(.font, value: Fonts.ui(.sansItalic, size), range: sub)
                }
            case .code:
                storage.addAttributes([
                    .font: Fonts.ui(.mono, self.bodySize * 0.9),
                    .foregroundColor: Palette.color(.inlineCodeText),
                    .backgroundColor: Palette.color(.inlineCodeBackground),
                ], range: range)
            case .strikethrough:
                storage.addAttributes([
                    .strikethroughStyle: NSUnderlineStyle.single.rawValue,
                    .foregroundColor: Palette.secondary,
                ], range: range)
            case .marker:
                storage.addAttribute(.foregroundColor, value: Palette.tertiary, range: range)
            case .hidden:
                // Off the caret's line, Markdown markers take no room.
                storage.addAttributes([
                    .font: UIFont.systemFont(ofSize: 0.01),
                    .foregroundColor: UIColor.clear,
                ], range: range)
            }
        }
        storage.endEditing()
        textView.typingAttributes = baseAttributes
    }
}

/// Bridges notes snapshots from the core (any thread) to a closure.
private final class NotesRelay: NotesListener, @unchecked Sendable {
    private let handler: (NotesSnapshot) -> Void
    init(_ handler: @escaping (NotesSnapshot) -> Void) { self.handler = handler }
    func onNotes(notes: NotesSnapshot) { handler(notes) }
}
