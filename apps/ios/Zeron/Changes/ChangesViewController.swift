import UIKit

/// Range selection stays active until cancelled or saved. Interior taps move
/// the nearer boundary; ties move the boundary most recently adjusted.
struct DiffLineSelection {
    let path: String
    let oldSide: Bool
    private(set) var lower: UInt32
    private(set) var upper: UInt32
    private var movingLower = false

    init(path: String, oldSide: Bool, line: UInt32) {
        self.path = path
        self.oldSide = oldSide
        lower = line
        upper = line
    }

    mutating func adjust(to line: UInt32) {
        if line < lower { lower = line; movingLower = true }
        else if line > upper { upper = line; movingLower = false }
        else if line == lower { upper = line }
        else if line == upper { lower = line }
        else {
            let nearerLower = line - lower < upper - line
            let tied = line - lower == upper - line
            if nearerLower || tied && movingLower { lower = line; movingLower = true }
            else { upper = line; movingLower = false }
        }
    }
}

/// Virtualized unified diff. File headers fold; code scrolls horizontally
/// while line numbers and the comment affordance remain in the viewport.
final class ChangesViewController: UIViewController, UITableViewDataSource, UITableViewDelegate {
    private let app: AppModel
    private let chatId: String
    private let draft: ChangeReviewDraft
    private var snapshot: CheckoutChanges?
    private var collapsed: Set<String> = []
    private var rows: [[Int]] = []
    private var poll: Task<Void, Never>?
    private var loading = false
    private var errorMessage: String?
    private let table = UITableView(frame: .zero, style: .plain)
    private let summary = UILabel()
    private let notice = UILabel()
    private let commentButton = UIButton(type: .system)
    private let selectionActions = UIStackView()
    private let cancelSelection = Glass.circleButton(symbol: "xmark", size: 50, pointSize: 17)
    private let status = UIStackView()
    private var refreshItem: UIBarButtonItem!
    private var reviewItem: UIBarButtonItem!
    private var selection: DiffLineSelection?
    private let empty = UILabel()
    private let refreshControl = UIRefreshControl()
    private var xOffset: CGFloat = 0
    private var codePlaneWidth: CGFloat = 0
    private var gutterWidth = ChangeDiffMetrics.minimumGutterWidth
    private var widthTask: Task<Void, Never>?

    init(app: AppModel, chatId: String, initial: CheckoutChanges? = nil) {
        self.app = app
        self.chatId = chatId
        self.draft = app.changeReview(for: chatId)
        self.snapshot = initial
        super.init(nibName: nil, bundle: nil)
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        title = "Files Changed"
        navigationItem.largeTitleDisplayMode = .never
        view.backgroundColor = Palette.background
        refreshItem = UIBarButtonItem(image: UIImage(systemName: "arrow.clockwise"), primaryAction: UIAction { [weak self] _ in
            Task { await self?.refresh() }
        })
        refreshItem.accessibilityLabel = "Refresh changes"
        reviewItem = UIBarButtonItem(title: "Review", primaryAction: UIAction { [weak self] _ in self?.showReview() })
        reviewItem.accessibilityIdentifier = "changes-review"
        let heading = UILabel()
        heading.text = "Files Changed"
        heading.font = Fonts.ui(.sansSemibold, 17)
        heading.textColor = Palette.text
        let titleStack = UIStackView(arrangedSubviews: [heading, summary])
        titleStack.axis = .vertical
        titleStack.alignment = .center
        titleStack.spacing = -2
        navigationItem.titleView = titleStack
        table.backgroundColor = Palette.codeBackground
        table.separatorStyle = .none
        table.dataSource = self
        table.delegate = self
        table.rowHeight = 30
        table.estimatedRowHeight = 30
        table.sectionHeaderHeight = 44
        table.estimatedSectionHeaderHeight = 44
        table.sectionHeaderTopPadding = 0
        table.accessibilityIdentifier = "changes-diff"
        table.register(ChangeCodeCell.self, forCellReuseIdentifier: "line")
        table.register(ChangeFileHeader.self, forHeaderFooterViewReuseIdentifier: "file")
        table.refreshControl = refreshControl
        refreshControl.addAction(UIAction { [weak self] _ in Task { await self?.refresh() } }, for: .valueChanged)
        empty.textColor = Palette.secondary
        empty.font = Fonts.ui(.sans, 15)
        empty.numberOfLines = 0
        empty.textAlignment = .center
        empty.accessibilityIdentifier = "changes-empty"
        summary.font = Fonts.ui(.sansMedium, 14)
        summary.accessibilityIdentifier = "changes-summary"
        notice.font = Fonts.ui(.sans, 12)
        notice.textColor = Palette.warning
        notice.numberOfLines = 0
        notice.accessibilityIdentifier = "changes-notice"
        status.axis = .vertical
        status.addArrangedSubview(notice)
        status.isLayoutMarginsRelativeArrangement = true
        var config = UIButton.Configuration.prominentGlass()
        config.baseBackgroundColor = Palette.accent
        config.baseForegroundColor = .white
        config.image = UIImage(systemName: "text.bubble", withConfiguration: UIImage.SymbolConfiguration(pointSize: 17, weight: .medium))
        config.imagePadding = 8
        config.cornerStyle = .capsule
        config.contentInsets = NSDirectionalEdgeInsets(top: 15, leading: 18, bottom: 15, trailing: 18)
        commentButton.configuration = config
        commentButton.titleLabel?.adjustsFontSizeToFitWidth = true
        commentButton.titleLabel?.minimumScaleFactor = 0.8
        selectionActions.axis = .horizontal
        selectionActions.alignment = .center
        selectionActions.spacing = 8
        selectionActions.addArrangedSubview(commentButton)
        selectionActions.addArrangedSubview(cancelSelection)
        selectionActions.isHidden = true
        cancelSelection.accessibilityIdentifier = "changes-clear-selection"
        cancelSelection.accessibilityLabel = "Cancel line selection"
        cancelSelection.addAction(UIAction { [weak self] _ in
            self?.selection = nil
            self?.updateReview()
        }, for: .touchUpInside)
        commentButton.accessibilityIdentifier = "changes-comment"
        commentButton.addAction(UIAction { [weak self] _ in self?.commentOnSelection() }, for: .touchUpInside)
        for v in [status, table, selectionActions] as [UIView] { v.translatesAutoresizingMaskIntoConstraints = false; view.addSubview(v) }
        table.addSubview(empty)
        empty.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            status.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor),
            status.leadingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.leadingAnchor),
            status.trailingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.trailingAnchor),
            table.topAnchor.constraint(equalTo: status.bottomAnchor),
            table.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            table.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            table.bottomAnchor.constraint(equalTo: view.bottomAnchor),
            selectionActions.leadingAnchor.constraint(greaterThanOrEqualTo: view.safeAreaLayoutGuide.leadingAnchor, constant: 16),
            selectionActions.trailingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.trailingAnchor, constant: -16),
            selectionActions.bottomAnchor.constraint(equalTo: view.safeAreaLayoutGuide.bottomAnchor, constant: -16),
            empty.centerXAnchor.constraint(equalTo: table.frameLayoutGuide.centerXAnchor),
            empty.centerYAnchor.constraint(equalTo: table.frameLayoutGuide.centerYAnchor),
            empty.widthAnchor.constraint(lessThanOrEqualTo: table.frameLayoutGuide.widthAnchor, constant: -40),
        ])
        sync()
    }

    override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        updateReview()
        poll?.cancel()
        poll = Task { [weak self] in
            while !Task.isCancelled {
                await self?.refresh()
                try? await Task.sleep(for: .seconds(5))
            }
        }
    }

    override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)
        poll?.cancel()
        poll = nil
    }

    private func refresh() async {
        guard !loading, let client = app.client else { refreshControl.endRefreshing(); return }
        loading = true
        if snapshot == nil { empty.text = "Loading changes…" }
        defer { loading = false; refreshControl.endRefreshing() }
        do {
            let next = try await client.checkoutChanges(chatId: chatId, knownChecksum: snapshot?.checksum)
            guard !Task.isCancelled else { return }
            errorMessage = nil
            if let next {
                selection = nil
                snapshot = next
                sync()
            } else { syncStatus() }
        } catch {
            guard !Task.isCancelled else { return }
            errorMessage = Self.errorText(error)
            syncStatus()
        }
    }

    static func errorText(_ error: Error) -> String {
        if case CoreError.HostUnavailable = error { return "This session's computer isn't reachable. Pull to retry." }
        if case CoreError.Unsupported = error { return "Update Zeron on this session's computer to view changes." }
        if case CoreError.NotFound = error { return "This session's folder isn't available yet. Pull to retry." }
        return "Couldn't load changes. Check that the session's computer is online and its folder is a Git repository, then retry."
    }

    private func sync() {
        let files = snapshot?.files ?? []
        rows = files.map { collapsed.contains($0.path) ? [] : Array($0.lines.indices) }
        let maxLine = files.reduce(UInt32(0)) { largest, file in
            file.lines.reduce(largest) { max($0, max($1.oldLine ?? 0, $1.newLine ?? 0)) }
        }
        gutterWidth = ChangeDiffMetrics.gutterWidth(maxLine: maxLine)
        summary.attributedText = changeTotals(additions: snapshot?.additions ?? 0, deletions: snapshot?.deletions ?? 0)
        syncStatus()
        table.reloadData()
        widthTask?.cancel()
        widthTask = Task { [weak self] in
            let width = await Task.detached(priority: .userInitiated) {
                let attributes: [NSAttributedString.Key: Any] = [.font: Fonts.ui(.mono, 12)]
                return files.reduce(CGFloat(0)) { width, file in
                    file.lines.reduce(width) { width, line in
                        max(width, (line.text.replacingOccurrences(of: "\t", with: "    ") as NSString).size(withAttributes: attributes).width + 20)
                    }
                }
            }.value
            guard let self, !Task.isCancelled else { return }
            self.codePlaneWidth = width
            // Only visible cells are materialized, even for large patches.
            self.table.reloadData()
        }
        updateReview()
    }

    private func syncStatus() {
        let partial = snapshot?.truncated == true ? "Partial snapshot — some changes exceed the preview limit." : nil
        notice.text = [errorMessage, partial].compactMap { $0 }.joined(separator: "\n")
        notice.isHidden = notice.text?.isEmpty != false
        status.directionalLayoutMargins = notice.isHidden ? .zero : NSDirectionalEdgeInsets(top: 8, leading: 16, bottom: 8, trailing: 16)
        empty.isHidden = snapshot?.files.isEmpty == false
        empty.text = errorMessage ?? (snapshot == nil ? "Loading changes…" : "No uncommitted changes")
    }

    private func updateReview() {
        let n = draft.comments.count
        reviewItem.title = "Review \(n)"
        navigationItem.rightBarButtonItems = n > 0 ? [refreshItem, reviewItem] : [refreshItem]
        selectionActions.isHidden = selection == nil
        if let selection {
            let count = Int(selection.upper - selection.lower) + 1
            commentButton.configuration?.title = count == 1 ? "Comment on \(selection.lower)" : "Comment on \(selection.lower) to \(selection.upper)"
            commentButton.accessibilityLabel = count == 1 ? "Comment on selected line" : "Comment on \(count) selected lines"
        }
        table.contentInset.bottom = selection == nil ? 0 : 80
        // Refresh visible note markers without disturbing the reader's scroll.
        for case let cell as ChangeCodeCell in table.visibleCells {
            if let path = table.indexPath(for: cell), let (file, line) = code(at: path) {
                cell.hasComment = comment(for: file, line: line) != nil
                cell.lineSelected = isSelected(file: file, line: line)
            }
        }
    }

    private func showReview() {
        let vc = ChangeReviewViewController(app: app, chatId: chatId, draft: draft)
        vc.onChange = { [weak self] in self?.updateReview() }
        navigationController?.pushViewController(vc, animated: true)
    }

    func numberOfSections(in tableView: UITableView) -> Int { snapshot?.files.count ?? 0 }
    func tableView(_ tableView: UITableView, numberOfRowsInSection section: Int) -> Int { rows[section].count }

    func tableView(_ tableView: UITableView, viewForHeaderInSection section: Int) -> UIView? {
        guard let file = snapshot?.files[section], let header = tableView.dequeueReusableHeaderFooterView(withIdentifier: "file") as? ChangeFileHeader else { return nil }
        header.configure(file, collapsed: collapsed.contains(file.path)) { [weak self] in
            guard let self else { return }
            if !self.collapsed.insert(file.path).inserted { self.collapsed.remove(file.path) }
            if self.collapsed.contains(file.path), self.selection?.path == file.path {
                self.selection = nil
                self.updateReview()
            }
            self.rows[section] = self.collapsed.contains(file.path) ? [] : Array(file.lines.indices)
            self.table.reloadSections(IndexSet(integer: section), with: .none)
        }
        return header
    }

    private func code(at path: IndexPath) -> (ChangedFile, ChangeLine)? {
        guard let files = snapshot?.files, files.indices.contains(path.section), rows[path.section].indices.contains(path.row) else { return nil }
        let file = files[path.section]
        return (file, file.lines[rows[path.section][path.row]])
    }

    func tableView(_ tableView: UITableView, cellForRowAt indexPath: IndexPath) -> UITableViewCell {
        let cell = tableView.dequeueReusableCell(withIdentifier: "line", for: indexPath) as! ChangeCodeCell
        guard let (file, line) = code(at: indexPath) else { return cell }
        cell.configure(line, offset: xOffset, planeWidth: codePlaneWidth, gutterWidth: gutterWidth, hasComment: comment(for: file, line: line) != nil)
        cell.lineSelected = isSelected(file: file, line: line)
        cell.onTap = { [weak self] in self?.tap(file: file, line: line) }
        cell.onHold = { [weak self] in self?.startSelection(file: file, line: line) }
        cell.onScroll = { [weak self, weak cell] offset in
            guard let self else { return }
            self.xOffset = offset
            for case let other as ChangeCodeCell in self.table.visibleCells where other !== cell { other.setCodeOffset(offset) }
        }
        return cell
    }

    func tableView(_ tableView: UITableView, didSelectRowAt indexPath: IndexPath) {
        tableView.deselectRow(at: indexPath, animated: false)
        guard let (file,line) = code(at: indexPath) else { return }
        tap(file: file, line: line)
    }

    private func isSelected(file: ChangedFile, line: ChangeLine) -> Bool {
        guard let selection else { return false }
        return isSelected(file: file, line: line, in: selection)
    }

    private func isSelected(file: ChangedFile, line: ChangeLine, in selection: DiffLineSelection) -> Bool {
        guard selection.path == file.path,
              selection.oldSide == (line.kind == .deletion),
              let number = selection.oldSide ? line.oldLine : line.newLine else { return false }
        return (selection.lower...selection.upper).contains(number)
    }

    private func startSelection(file: ChangedFile, line: ChangeLine) {
        let old = line.kind == .deletion
        guard let number = old ? line.oldLine : line.newLine else { return }
        selection = DiffLineSelection(path: file.path, oldSide: old, line: number)
        UISelectionFeedbackGenerator().selectionChanged()
        updateReview()
    }

    private func tap(file: ChangedFile, line: ChangeLine) {
        let old = line.kind == .deletion
        guard let number = old ? line.oldLine : line.newLine else { return }
        guard var current = selection else {
            presentComment(for: DiffLineSelection(path: file.path, oldSide: old, line: number))
            return
        }
        if current.path == file.path, current.oldSide == old {
            current.adjust(to: number)
            selection = current
        } else {
            selection = DiffLineSelection(path: file.path, oldSide: old, line: number)
        }
        updateReview()
    }

    private func comment(for file: ChangedFile, line: ChangeLine) -> DiffReviewComment? {
        let old = line.kind == .deletion
        guard let number = old ? line.oldLine : line.newLine else { return nil }
        return draft.comments.first { note in
            guard note.path == file.path, note.oldSide == old,
                  (note.line...(note.endLine ?? note.line)).contains(number) else { return false }
            let source = note.code.components(separatedBy: "\n")
            let offset = Int(number - note.line)
            return source.indices.contains(offset) && source[offset] == line.text
        }
    }

    private func commentOnSelection() {
        guard let selection else { return }
        presentComment(for: selection)
    }

    private func presentComment(for selected: DiffLineSelection) {
        guard let file = snapshot?.files.first(where: { $0.path == selected.path }) else { return }
        let selectedLines = file.lines.filter { isSelected(file: file, line: $0, in: selected) }
        let selectedCode = selectedLines.map(\.text).joined(separator: "\n")
        let existing = draft.comments.first {
            $0.path == selected.path && $0.oldSide == selected.oldSide && $0.line == selected.lower && ($0.endLine ?? $0.line) == selected.upper && $0.code == selectedCode
        }
        let c = existing ?? DiffReviewComment(id: UUID(), path: file.path, oldPath: file.oldPath, oldSide: selected.oldSide, line: selected.lower, endLine: selected.upper > selected.lower ? selected.upper : nil, code: selectedCode, body: "", previewLines: selectedLines)
        let editor = ChangeCommentViewController(comment: c, editing: existing != nil) { [weak self] updated in
            guard let self else { return }
            if let i = self.draft.comments.firstIndex(where: { $0.id == updated.id }) { self.draft.comments[i] = updated }
            else { self.draft.comments.append(updated) }
            self.selection = nil
            self.updateReview()
        }
        present(UINavigationController(rootViewController: editor), animated: true)
    }

}

func changeTotals(files: Int? = nil, additions: UInt32, deletions: UInt32) -> NSAttributedString {
    let text = NSMutableAttributedString(string: "")
    if let files { text.append(NSAttributedString(string: "\(files) file\(files == 1 ? "" : "s")   ", attributes: [.foregroundColor: Palette.secondary])) }
    text.append(NSAttributedString(string: "+\(additions)", attributes: [.foregroundColor: Palette.success]))
    text.append(NSAttributedString(string: " −\(deletions)", attributes: [.foregroundColor: Palette.danger]))
    return text
}
