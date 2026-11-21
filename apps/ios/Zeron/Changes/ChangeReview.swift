import UIKit

/// Review notes live in memory for this signed-in app, scoped to their chat.
/// Sending uses desktop's prompt format, including the original/changed side.
struct DiffReviewComment: Identifiable, Equatable {
    let id: UUID
    let path: String
    let oldPath: String?
    let oldSide: Bool
    let line: UInt32
    var endLine: UInt32? = nil
    let code: String
    var body: String
    var previewLines: [ChangeLine] = []

    var location: String {
        let range = endLine.map { "\(line)-\($0)" } ?? String(line)
        return "\(oldSide ? oldPath ?? path : path):\(range)"
    }
    var side: String { oldSide ? "L" : "R" }
}

final class ChangeReviewDraft {
    var comments: [DiffReviewComment] = []

    var prompt: String {
        guard !comments.isEmpty else { return "" }
        let bullets = comments.map {
            "- \($0.location) (\($0.side)): \($0.body.trimmingCharacters(in: .whitespacesAndNewlines).replacingOccurrences(of: "\n", with: "\n  "))"
        }
        return "Address the review comments below.\n\nComments on the diff (each cites the file and line it belongs to; L = line number in the original file, R = in the changed file):\n" + bullets.joined(separator: "\n")
    }
}

/// Shared by the diff and the review list. Cancel leaves the saved note alone.
final class ChangeCommentViewController: UIViewController, UITextViewDelegate, UITableViewDataSource {
    private let comment: DiffReviewComment
    private let editingExisting: Bool
    private let save: (DiffReviewComment) -> Void
    private let input = UITextView()
    private let placeholder = UILabel()
    private let preview = UITableView(frame: .zero, style: .plain)
    private var lines: [ChangeLine] = []
    private var previewWidth: CGFloat = 0
    private var previewOffset: CGFloat = 0
    private var gutterWidth = ChangeDiffMetrics.minimumGutterWidth
    private var saveItem: UIBarButtonItem!

    init(comment: DiffReviewComment, editing: Bool, save: @escaping (DiffReviewComment) -> Void) {
        self.comment = comment
        self.editingExisting = editing
        self.save = save
        super.init(nibName: nil, bundle: nil)
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Palette.elevated
        title = editingExisting ? "Edit Comment" : "Add Comment"
        navigationController?.navigationBar.tintColor = Palette.text
        navigationController?.navigationBar.titleTextAttributes = [.font: Fonts.ui(.sansSemibold, 17), .foregroundColor: Palette.text]
        navigationItem.leftBarButtonItem = UIBarButtonItem(systemItem: .close, primaryAction: UIAction { [weak self] _ in self?.dismiss(animated: true) })
        navigationItem.leftBarButtonItem?.accessibilityIdentifier = "change-comment-close"
        navigationItem.leftBarButtonItem?.accessibilityLabel = "Close comment"
        saveItem = UIBarButtonItem(title: editingExisting ? "Save" : "Comment", primaryAction: UIAction { [weak self] _ in
            guard let self else { return }
            var updated = self.comment
            updated.body = self.input.text.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !updated.body.isEmpty else { return }
            self.save(updated)
            self.dismiss(animated: true)
        })
        saveItem.style = .prominent
        saveItem.tintColor = Palette.accent
        saveItem.accessibilityIdentifier = "change-comment-save"
        navigationItem.rightBarButtonItem = saveItem
        if let sheet = navigationController?.sheetPresentationController {
            sheet.detents = [.large()]
            sheet.prefersGrabberVisible = false
        }

        lines = comment.previewLines
        if lines.isEmpty {
            lines = comment.code.components(separatedBy: "\n").enumerated().map { offset, text in
                let number = comment.line + UInt32(offset)
                return ChangeLine(kind: comment.oldSide ? .deletion : .addition, oldLine: comment.oldSide ? number : nil, newLine: comment.oldSide ? nil : number, text: text, highlights: [])
            }
        }
        previewWidth = lines.reduce(CGFloat(0)) {
            max($0, ($1.text.replacingOccurrences(of: "\t", with: "    ") as NSString).size(withAttributes: [.font: Fonts.ui(.mono, 12)]).width + 20)
        }
        let largest = lines.reduce(UInt32(0)) { max($0, max($1.oldLine ?? 0, $1.newLine ?? 0)) }
        gutterWidth = ChangeDiffMetrics.gutterWidth(maxLine: largest)
        preview.dataSource = self
        preview.rowHeight = 30
        preview.estimatedRowHeight = 30
        preview.separatorStyle = .none
        preview.sectionHeaderHeight = 0
        preview.sectionFooterHeight = 0
        preview.sectionHeaderTopPadding = 0
        preview.backgroundColor = Palette.codeBackground
        preview.contentInsetAdjustmentBehavior = .never
        preview.allowsSelection = false
        preview.accessibilityIdentifier = "change-comment-code"
        preview.accessibilityLabel = "\(comment.location) · \(comment.oldSide ? "Original" : "Changed")"
        preview.register(ChangeCodeCell.self, forCellReuseIdentifier: "code")
        input.font = Fonts.ui(.sans, 16)
        input.textColor = Palette.text
        input.backgroundColor = .clear
        input.textContainerInset = UIEdgeInsets(top: 16, left: 12, bottom: 16, right: 12)
        input.text = comment.body
        input.delegate = self
        input.accessibilityIdentifier = "change-comment-input"
        input.accessibilityLabel = "Review comment"
        placeholder.text = "Leave a comment…"
        placeholder.font = input.font
        placeholder.textColor = Palette.tertiary
        placeholder.isAccessibilityElement = false
        placeholder.isUserInteractionEnabled = false
        placeholder.translatesAutoresizingMaskIntoConstraints = false
        input.addSubview(placeholder)
        for v in [preview, input] { v.translatesAutoresizingMaskIntoConstraints = false; view.addSubview(v) }
        let content = UILayoutGuide()
        view.addLayoutGuide(content)
        let preferredPreviewHeight = preview.heightAnchor.constraint(equalToConstant: min(CGFloat(lines.count) * 30, 180))
        preferredPreviewHeight.priority = .defaultHigh
        NSLayoutConstraint.activate([
            content.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor),
            content.bottomAnchor.constraint(equalTo: view.keyboardLayoutGuide.topAnchor),
            content.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            content.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            preview.topAnchor.constraint(equalTo: content.topAnchor),
            preview.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            preview.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            preferredPreviewHeight,
            preview.heightAnchor.constraint(lessThanOrEqualTo: content.heightAnchor, multiplier: 0.4),
            preview.heightAnchor.constraint(greaterThanOrEqualToConstant: 0),
            input.topAnchor.constraint(equalTo: preview.bottomAnchor),
            input.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            input.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            input.bottomAnchor.constraint(equalTo: content.bottomAnchor),
            placeholder.leadingAnchor.constraint(equalTo: input.frameLayoutGuide.leadingAnchor, constant: input.textContainerInset.left + input.textContainer.lineFragmentPadding),
            placeholder.trailingAnchor.constraint(lessThanOrEqualTo: input.frameLayoutGuide.trailingAnchor, constant: -16),
            placeholder.topAnchor.constraint(equalTo: input.frameLayoutGuide.topAnchor, constant: input.textContainerInset.top),
        ])
        textViewDidChange(input)
        isModalInPresentation = true
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        input.becomeFirstResponder()
    }

    func tableView(_ tableView: UITableView, numberOfRowsInSection section: Int) -> Int { lines.count }

    func tableView(_ tableView: UITableView, cellForRowAt indexPath: IndexPath) -> UITableViewCell {
        let cell = tableView.dequeueReusableCell(withIdentifier: "code", for: indexPath) as! ChangeCodeCell
        cell.configure(lines[indexPath.row], offset: previewOffset, planeWidth: previewWidth, gutterWidth: gutterWidth, hasComment: false)
        cell.lineSelected = false
        cell.accessibilityTraits = .staticText
        cell.accessibilityHint = nil
        cell.accessibilityCustomActions = nil
        cell.onScroll = { [weak self, weak cell] offset in
            guard let self else { return }
            self.previewOffset = offset
            for case let other as ChangeCodeCell in self.preview.visibleCells where other !== cell { other.setCodeOffset(offset) }
        }
        return cell
    }

    func textViewDidChange(_ textView: UITextView) {
        saveItem.isEnabled = !textView.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        placeholder.isHidden = !textView.text.isEmpty
    }
}

final class ChangeReviewViewController: UITableViewController {
    private let app: AppModel
    private let chatId: String
    private let draft: ChangeReviewDraft
    var onChange: (() -> Void)?

    init(app: AppModel, chatId: String, draft: ChangeReviewDraft) {
        self.app = app
        self.chatId = chatId
        self.draft = draft
        super.init(style: .insetGrouped)
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        title = "Review Comments"
        tableView.backgroundColor = Palette.background
        tableView.accessibilityIdentifier = "change-review-list"
        tableView.register(UITableViewCell.self, forCellReuseIdentifier: "comment")
        navigationItem.rightBarButtonItem = UIBarButtonItem(title: "Send to Agent", primaryAction: UIAction { [weak self] _ in self?.send() })
        navigationItem.rightBarButtonItem?.accessibilityIdentifier = "change-review-send"
        sync()
    }

    private func sync() {
        navigationItem.subtitle = "\(draft.comments.count) pending"
        navigationItem.rightBarButtonItem?.isEnabled = !draft.comments.isEmpty
        tableView.reloadData()
        onChange?()
    }

    private func send() {
        guard !draft.comments.isEmpty, let client = app.client else { return }
        do {
            let handle = try client.openSession(chatId: chatId)
            _ = try handle.send(request: SendRequest(text: draft.prompt, attachments: [], worktree: nil, busy: .queue))
            app.forgetFreshSideChat(chatId)
            app.noteDeliveryActivity()
            draft.comments.removeAll()
            sync()
            Toast.show("Review sent to agent", in: view.window)
            navigationController?.popViewController(animated: true)
        } catch {
            // Preserve every note if the command wasn't accepted.
            Toast.show("Couldn't send review. Your comments are still here.", in: view.window)
        }
    }

    override func tableView(_ tableView: UITableView, numberOfRowsInSection section: Int) -> Int { draft.comments.count }

    override func tableView(_ tableView: UITableView, titleForFooterInSection section: Int) -> String? {
        draft.comments.isEmpty ? nil : "Tap a comment to edit. Swipe left to remove."
    }

    override func tableView(_ tableView: UITableView, cellForRowAt indexPath: IndexPath) -> UITableViewCell {
        let cell = tableView.dequeueReusableCell(withIdentifier: "comment", for: indexPath)
        let c = draft.comments[indexPath.row]
        var config = UIListContentConfiguration.subtitleCell()
        config.text = "\(c.location) (\(c.side))"
        config.textProperties.font = Fonts.ui(.mono, 12)
        config.textProperties.color = Palette.secondary
        config.textProperties.numberOfLines = 2
        config.secondaryText = c.body
        config.secondaryTextProperties.font = Fonts.ui(.sans, 15)
        config.secondaryTextProperties.color = Palette.text
        config.secondaryTextProperties.numberOfLines = 0
        cell.contentConfiguration = config
        cell.backgroundColor = Palette.elevated
        cell.accessoryType = .disclosureIndicator
        return cell
    }

    override func tableView(_ tableView: UITableView, didSelectRowAt indexPath: IndexPath) {
        tableView.deselectRow(at: indexPath, animated: true)
        let comment = draft.comments[indexPath.row]
        let editor = ChangeCommentViewController(comment: comment, editing: true) { [weak self] updated in
            guard let self, let i = self.draft.comments.firstIndex(where: { $0.id == updated.id }) else { return }
            self.draft.comments[i] = updated
            self.sync()
        }
        present(UINavigationController(rootViewController: editor), animated: true)
    }

    override func tableView(_ tableView: UITableView, commit editingStyle: UITableViewCell.EditingStyle, forRowAt indexPath: IndexPath) {
        guard editingStyle == .delete else { return }
        draft.comments.remove(at: indexPath.row)
        sync()
    }
}
