import PhotosUI
import UIKit

/// An image staged in the composer before sending.
struct StagedImage: Identifiable, Equatable {
    let id: String
    let name: String
    let data: Data
    let thumbnail: UIImage
}

/// How a message sent during a running turn is delivered.
enum DeliveryMode: String {
    /// Waits in the shared queue for the turn to end (default).
    case queue
    /// Injected mid-turn when the harness supports it.
    case steer
    /// Stops the turn, then sends.
    case interrupt
}

/// A context chip above the input (model, effort, branch…).
struct ComposerChip: Equatable {
    let id: String
    let title: String
    let symbol: String?
    var tint: UIColor? = nil
}

/// The glass composer: [+] [growing text] [send/stop/queue], with staged
/// image thumbnails inside the capsule and a context-chip strip above it
/// while focused. One glass surface — its corner radius and height morph
/// with content, so there are no view swaps to stutter.
final class ComposerBar: UIView, UITextViewDelegate {
    enum Action: Equatable {
        case send
        case queue
        case stop
    }

    // Callbacks
    var onSend: ((String, [StagedImage], DeliveryMode) -> Void)?
    var onStop: (() -> Void)?
    var attachMenu: (() -> UIMenu)?
    var onChipTap: ((String, UIView) -> Void)?
    var onHeightChange: (() -> Void)?
    var onFocusChange: ((Bool) -> Void)?
    /// `@` file search (nil disables mentions).
    var mentionSearch: ((String) async -> [FileMatch])?

    // State
    var running = false { didSet { refreshAction() } }
    var canSteer = false { didSet { refreshAction() } }
    var preferredDelivery: DeliveryMode = .queue { didSet { refreshAction() } }
    var placeholder = "Message" { didSet { placeholderLabel.text = placeholder } }
    var chips: [ComposerChip] = [] { didSet { if chips != oldValue { rebuildChips() } } }
    /// Chips with a menu open it on tap (native, glassy, lazily loaded).
    var chipMenus: [String: () -> UIMenu?] = [:] { didSet { applyChipMenus() } }
    /// Chips stay visible without focus (new-session canvas).
    var chipsAlwaysVisible = false { didSet { updateChipsVisibility(animated: false) } }
    private(set) var images: [StagedImage] = [] { didSet { rebuildThumbs(); refreshAction() } }

    var text: String {
        get { textView.text }
        set {
            textView.text = newValue
            textChanged()
        }
    }

    let textView = UITextView()
    private let glass = Glass.surface(interactive: false, radius: 24)
    private let placeholderLabel = UILabel()
    private let attachButton = UIButton(type: .system)
    private let actionButton = UIButton(type: .custom)
    private let thumbs = UIStackView()
    private let thumbsScroll = UIScrollView()
    private let chipStrip = UIStackView()
    private let chipScroll = UIScrollView()
    private var textHeight: NSLayoutConstraint!
    private var thumbsHeight: NSLayoutConstraint!
    private var chipsHeight: NSLayoutConstraint!
    private var currentAction: Action = .send
    private var mentions = MentionIndex()
    private let suggestions = MentionSuggestions()
    private var mentionQuery: (range: NSRange, query: String)?
    private var mentionTask: Task<Void, Never>?

    private let font = Fonts.ui(.sans, UIFontMetrics(forTextStyle: .body).scaledValue(for: 16.5))
    private var maxLines: Int { traitCollection.verticalSizeClass == .compact ? 3 : 7 }

    override init(frame: CGRect) {
        super.init(frame: frame)
        build()
    }

    required init?(coder: NSCoder) { fatalError() }

    private func build() {
        // Chip strip above the capsule.
        chipScroll.showsHorizontalScrollIndicator = false
        chipScroll.clipsToBounds = false
        chipScroll.translatesAutoresizingMaskIntoConstraints = false
        chipStrip.axis = .horizontal
        chipStrip.spacing = 8
        chipStrip.distribution = .fill
        chipStrip.alignment = .center
        chipStrip.translatesAutoresizingMaskIntoConstraints = false
        chipScroll.addSubview(chipStrip)
        addSubview(chipScroll)

        glass.translatesAutoresizingMaskIntoConstraints = false
        addSubview(glass)
        let content = glass.contentView

        thumbsScroll.showsHorizontalScrollIndicator = false
        thumbsScroll.translatesAutoresizingMaskIntoConstraints = false
        thumbs.axis = .horizontal
        thumbs.spacing = 8
        thumbs.translatesAutoresizingMaskIntoConstraints = false
        thumbsScroll.addSubview(thumbs)
        content.addSubview(thumbsScroll)

        var attach = UIButton.Configuration.plain()
        attach.image = UIImage(systemName: "plus", withConfiguration: UIImage.SymbolConfiguration(pointSize: 17, weight: .medium))
        attach.baseForegroundColor = Palette.text
        attachButton.configuration = attach
        attachButton.accessibilityLabel = "Attach"
        attachButton.accessibilityIdentifier = "composer-attach"
        attachButton.translatesAutoresizingMaskIntoConstraints = false
        // Built on open so "Paste Image" reflects the pasteboard right now.
        attachButton.menu = UIMenu(children: [UIDeferredMenuElement.uncached { [weak self] done in
            done(self?.attachMenu?().children ?? [])
        }])
        attachButton.showsMenuAsPrimaryAction = true
        content.addSubview(attachButton)

        textView.font = font
        textView.textColor = Palette.text
        textView.backgroundColor = .clear
        textView.delegate = self
        textView.isScrollEnabled = false
        textView.textContainerInset = UIEdgeInsets(top: 11, left: 0, bottom: 11, right: 0)
        textView.textContainer.lineFragmentPadding = 0
        textView.accessibilityIdentifier = "composer-input"
        textView.translatesAutoresizingMaskIntoConstraints = false
        textView.keyboardDismissMode = .interactive
        content.addSubview(textView)

        placeholderLabel.font = font
        placeholderLabel.textColor = Palette.tertiary
        placeholderLabel.text = placeholder
        placeholderLabel.isUserInteractionEnabled = false
        placeholderLabel.translatesAutoresizingMaskIntoConstraints = false
        content.addSubview(placeholderLabel)

        actionButton.translatesAutoresizingMaskIntoConstraints = false
        actionButton.layer.cornerRadius = 17
        actionButton.layer.cornerCurve = .continuous
        actionButton.accessibilityIdentifier = "composer-send"
        actionButton.addAction(UIAction { [weak self] _ in self?.primaryAction() }, for: .touchUpInside)
        content.addSubview(actionButton)

        textHeight = textView.heightAnchor.constraint(equalToConstant: 44)
        thumbsHeight = thumbsScroll.heightAnchor.constraint(equalToConstant: 0)
        chipsHeight = chipScroll.heightAnchor.constraint(equalToConstant: 0)
        NSLayoutConstraint.activate([
            chipScroll.topAnchor.constraint(equalTo: topAnchor),
            chipScroll.leadingAnchor.constraint(equalTo: leadingAnchor),
            chipScroll.trailingAnchor.constraint(equalTo: trailingAnchor),
            chipsHeight,
            chipStrip.topAnchor.constraint(equalTo: chipScroll.contentLayoutGuide.topAnchor),
            chipStrip.bottomAnchor.constraint(equalTo: chipScroll.contentLayoutGuide.bottomAnchor),
            chipStrip.leadingAnchor.constraint(equalTo: chipScroll.contentLayoutGuide.leadingAnchor, constant: 2),
            chipStrip.trailingAnchor.constraint(equalTo: chipScroll.contentLayoutGuide.trailingAnchor, constant: -2),
            chipStrip.heightAnchor.constraint(equalTo: chipScroll.frameLayoutGuide.heightAnchor),

            glass.topAnchor.constraint(equalTo: chipScroll.bottomAnchor),
            glass.leadingAnchor.constraint(equalTo: leadingAnchor),
            glass.trailingAnchor.constraint(equalTo: trailingAnchor),
            glass.bottomAnchor.constraint(equalTo: bottomAnchor),

            thumbsScroll.topAnchor.constraint(equalTo: content.topAnchor, constant: 0),
            thumbsScroll.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 12),
            thumbsScroll.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -12),
            thumbsHeight,
            thumbs.topAnchor.constraint(equalTo: thumbsScroll.contentLayoutGuide.topAnchor),
            thumbs.bottomAnchor.constraint(equalTo: thumbsScroll.contentLayoutGuide.bottomAnchor),
            thumbs.leadingAnchor.constraint(equalTo: thumbsScroll.contentLayoutGuide.leadingAnchor),
            thumbs.trailingAnchor.constraint(equalTo: thumbsScroll.contentLayoutGuide.trailingAnchor),
            thumbs.heightAnchor.constraint(equalTo: thumbsScroll.frameLayoutGuide.heightAnchor),

            attachButton.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 4),
            attachButton.bottomAnchor.constraint(equalTo: content.bottomAnchor, constant: -2),
            attachButton.widthAnchor.constraint(equalToConstant: 44),
            attachButton.heightAnchor.constraint(equalToConstant: 44),

            textView.topAnchor.constraint(equalTo: thumbsScroll.bottomAnchor),
            textView.leadingAnchor.constraint(equalTo: attachButton.trailingAnchor, constant: 0),
            textView.trailingAnchor.constraint(equalTo: actionButton.leadingAnchor, constant: -8),
            textView.bottomAnchor.constraint(equalTo: content.bottomAnchor),
            textHeight,

            placeholderLabel.leadingAnchor.constraint(equalTo: textView.leadingAnchor),
            placeholderLabel.trailingAnchor.constraint(lessThanOrEqualTo: textView.trailingAnchor),
            placeholderLabel.topAnchor.constraint(equalTo: textView.topAnchor, constant: 11),

            actionButton.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -6),
            actionButton.bottomAnchor.constraint(equalTo: content.bottomAnchor, constant: -5),
            actionButton.widthAnchor.constraint(equalToConstant: 34),
            actionButton.heightAnchor.constraint(equalToConstant: 34),
        ])
        suggestions.isHidden = true
        suggestions.translatesAutoresizingMaskIntoConstraints = false
        suggestions.onPick = { [weak self] file in self?.insertMention(file) }
        refreshAction()
    }

    /// Suggestions float above the bar, outside its bounds — so they live in
    /// the screen's root view (ancestors would otherwise swallow their taps).
    override func didMoveToWindow() {
        super.didMoveToWindow()
        guard let root = findViewController()?.view, suggestions.superview !== root else { return }
        root.addSubview(suggestions)
        NSLayoutConstraint.activate([
            suggestions.leadingAnchor.constraint(equalTo: leadingAnchor),
            suggestions.trailingAnchor.constraint(equalTo: trailingAnchor),
            suggestions.bottomAnchor.constraint(equalTo: glass.topAnchor, constant: -8),
        ])
    }

    // MARK: Mentions

    private func updateMentionQuery() {
        guard mentionSearch != nil else { return }
        mentionQuery = MentionIndex.activeQuery(in: textView.text, cursor: textView.selectedRange.location)
        mentionTask?.cancel()
        guard let q = mentionQuery else {
            setSuggestionsVisible(false)
            return
        }
        mentionTask = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(120))
            guard !Task.isCancelled, let self, let search = self.mentionSearch else { return }
            let files = await search(q.query)
            guard !Task.isCancelled, self.mentionQuery?.query == q.query else { return }
            self.suggestions.show(files)
            self.setSuggestionsVisible(!files.isEmpty)
        }
    }

    private func setSuggestionsVisible(_ visible: Bool) {
        guard suggestions.isHidden == visible else { return }
        suggestions.isHidden = false
        suggestions.alpha = visible ? 0 : 1
        suggestions.transform = visible ? CGAffineTransform(translationX: 0, y: 8) : .identity
        UIView.animate(withDuration: 0.22, delay: 0, options: [.curveEaseOut, .beginFromCurrentState]) {
            self.suggestions.alpha = visible ? 1 : 0
            self.suggestions.transform = visible ? .identity : CGAffineTransform(translationX: 0, y: 8)
        } completion: { _ in
            if !visible { self.suggestions.isHidden = true }
        }
    }

    private func insertMention(_ file: FileMatch) {
        guard let q = mentionQuery else { return }
        UISelectionFeedbackGenerator().selectionChanged()
        let token = mentions.token(for: file) + " "
        let ns = textView.text as NSString
        textView.text = ns.replacingCharacters(in: q.range, with: token)
        textView.selectedRange = NSRange(location: q.range.location + (token as NSString).length, length: 0)
        mentionQuery = nil
        setSuggestionsVisible(false)
        textChanged()
    }

    /// Accent the live `@tokens`; everything else is plain body text.
    private func styleMentions() {
        guard !mentions.tokens.isEmpty, textView.markedTextRange == nil else { return }
        let selected = textView.selectedRange
        let base: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: Palette.text]
        let styled = NSMutableAttributedString(string: textView.text, attributes: base)
        let ns = textView.text as NSString
        for token in mentions.tokens.keys {
            var search = NSRange(location: 0, length: ns.length)
            while true {
                let r = ns.range(of: token, options: [], range: search)
                if r.location == NSNotFound { break }
                styled.addAttributes([.foregroundColor: Palette.accent, .font: Fonts.ui(.sansMedium, font.pointSize)], range: r)
                search = NSRange(location: r.upperBound, length: ns.length - r.upperBound)
            }
        }
        textView.attributedText = styled
        textView.selectedRange = selected
        textView.typingAttributes = base
    }

    // MARK: Focus & chips

    @discardableResult
    override func becomeFirstResponder() -> Bool { textView.becomeFirstResponder() }

    @discardableResult
    override func resignFirstResponder() -> Bool { textView.resignFirstResponder() }

    func textViewDidBeginEditing(_ textView: UITextView) {
        updateChipsVisibility(animated: true)
        onFocusChange?(true)
    }

    func textViewDidEndEditing(_ textView: UITextView) {
        updateChipsVisibility(animated: true)
        onFocusChange?(false)
    }

    private func updateChipsVisibility(animated: Bool) {
        let show = !chips.isEmpty && (chipsAlwaysVisible || textView.isFirstResponder)
        let target: CGFloat = show ? 42 : 0
        guard chipsHeight.constant != target || chipScroll.alpha != (show ? 1 : 0) else { return }
        let change = {
            self.chipsHeight.constant = target
            self.chipScroll.alpha = show ? 1 : 0
            self.onHeightChange?()
            self.superview?.layoutIfNeeded()
        }
        if animated {
            UIView.animate(withDuration: 0.32, delay: 0, usingSpringWithDamping: 0.9, initialSpringVelocity: 0, options: [.allowUserInteraction, .beginFromCurrentState], animations: change)
        } else {
            change()
        }
    }

    private func rebuildChips() {
        chipStrip.arrangedSubviews.forEach { $0.removeFromSuperview() }
        for chip in chips {
            var config = UIButton.Configuration.glass()
            config.title = chip.title
            config.image = chip.symbol.flatMap { UIImage(systemName: $0, withConfiguration: UIImage.SymbolConfiguration(pointSize: 12, weight: .medium)) }
            config.imagePadding = 5
            config.baseForegroundColor = chip.tint ?? Palette.text
            config.cornerStyle = .capsule
            config.contentInsets = NSDirectionalEdgeInsets(top: 6, leading: 12, bottom: 6, trailing: 12)
            config.titleTextAttributesTransformer = UIConfigurationTextAttributesTransformer { attrs in
                var a = attrs
                a.font = Fonts.ui(.sansMedium, 13.5)
                return a
            }
            let b = UIButton(configuration: config)
            b.accessibilityIdentifier = "composer-chip-\(chip.id)"
            b.setContentHuggingPriority(.required, for: .horizontal)
            b.setContentCompressionResistancePriority(.required, for: .horizontal)
            b.addAction(UIAction { [weak self, weak b] _ in
                guard let self, let b else { return }
                self.onChipTap?(chip.id, b)
            }, for: .touchUpInside)
            chipStrip.addArrangedSubview(b)
        }
        applyChipMenus()
        updateChipsVisibility(animated: false)
    }

    private func applyChipMenus() {
        for case let b as UIButton in chipStrip.arrangedSubviews {
            guard let id = b.accessibilityIdentifier?.replacingOccurrences(of: "composer-chip-", with: ""),
                  let provider = chipMenus[id]
            else { continue }
            b.menu = provider()
            b.showsMenuAsPrimaryAction = b.menu != nil
        }
    }

    // MARK: Attachments

    func addImages(_ new: [StagedImage]) {
        images.append(contentsOf: new.prefix(max(0, 8 - images.count)))
    }

    func clearImages() { images.removeAll() }

    private func rebuildThumbs() {
        thumbs.arrangedSubviews.forEach { $0.removeFromSuperview() }
        for img in images {
            let iv = UIImageView(image: img.thumbnail)
            iv.contentMode = .scaleAspectFill
            iv.clipsToBounds = true
            iv.layer.cornerRadius = 12
            iv.layer.cornerCurve = .continuous
            iv.isUserInteractionEnabled = true
            iv.translatesAutoresizingMaskIntoConstraints = false
            iv.widthAnchor.constraint(equalToConstant: 60).isActive = true
            let remove = UIButton(type: .system)
            remove.setImage(UIImage(systemName: "xmark.circle.fill", withConfiguration: UIImage.SymbolConfiguration(pointSize: 16, weight: .semibold)), for: .normal)
            remove.tintColor = .white
            remove.layer.shadowOpacity = 0.3
            remove.layer.shadowRadius = 2
            remove.layer.shadowOffset = .zero
            remove.accessibilityLabel = "Remove image"
            remove.translatesAutoresizingMaskIntoConstraints = false
            remove.addAction(UIAction { [weak self] _ in
                UIView.animate(withDuration: 0.2) { self?.images.removeAll { $0.id == img.id } }
            }, for: .touchUpInside)
            iv.addSubview(remove)
            NSLayoutConstraint.activate([
                remove.topAnchor.constraint(equalTo: iv.topAnchor, constant: 2),
                remove.trailingAnchor.constraint(equalTo: iv.trailingAnchor, constant: -2),
            ])
            thumbs.addArrangedSubview(iv)
        }
        thumbsHeight.constant = images.isEmpty ? 0 : 72
        thumbs.layoutMargins = UIEdgeInsets(top: 12, left: 0, bottom: 0, right: 0)
        thumbs.isLayoutMarginsRelativeArrangement = true
        glass.cornerConfiguration = .uniformCorners(radius: .fixed(images.isEmpty && textHeight.constant <= 44 ? 22 : 24))
        onHeightChange?()
    }

    // MARK: Text

    func textViewDidChange(_ textView: UITextView) {
        textChanged()
        styleMentions()
        updateMentionQuery()
    }

    func textViewDidChangeSelection(_ textView: UITextView) {
        if mentionQuery != nil { updateMentionQuery() }
    }

    func textView(_ textView: UITextView, shouldChangeTextIn range: NSRange, replacementText text: String) -> Bool {
        true
    }

    private func textChanged() {
        placeholderLabel.isHidden = !textView.text.isEmpty
        let width = max(40, textView.bounds.width)
        let fitting = textView.sizeThatFits(CGSize(width: width, height: .greatestFiniteMagnitude)).height
        let maxHeight = font.lineHeight * CGFloat(maxLines) + 22
        let height = min(max(44, fitting), maxHeight)
        textView.isScrollEnabled = fitting > maxHeight
        if abs(textHeight.constant - height) > 0.5 {
            textHeight.constant = height
            UIView.animate(withDuration: 0.18, delay: 0, options: [.beginFromCurrentState, .allowUserInteraction]) {
                self.onHeightChange?()
                self.superview?.layoutIfNeeded()
            }
        }
        refreshAction()
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        if textView.bounds.width > 0, textHeight.constant == 44, !textView.text.isEmpty { textChanged() }
    }

    // MARK: Action button

    private var hasContent: Bool {
        !textView.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || !images.isEmpty
    }

    private func refreshAction() {
        let action: Action = running ? (hasContent ? .queue : .stop) : .send
        let enabled = action == .stop || hasContent
        let symbol: String
        switch action {
        case .send: symbol = "arrow.up"
        case .queue: symbol = preferredDelivery == .steer && canSteer ? "arrow.turn.down.right" : "arrow.up"
        case .stop: symbol = "stop.fill"
        }
        let config = UIImage.SymbolConfiguration(pointSize: action == .stop ? 12 : 15, weight: .bold)
        let image = UIImage(systemName: symbol, withConfiguration: config)
        if currentAction != action {
            actionButton.setImage(image, for: .normal)
            actionButton.imageView?.addSymbolEffect(.bounce.byLayer, options: .nonRepeating)
        } else {
            actionButton.setImage(image, for: .normal)
        }
        currentAction = action
        actionButton.tintColor = enabled ? Palette.background : Palette.tertiary
        actionButton.backgroundColor = enabled ? Palette.text : Palette.chip
        actionButton.isEnabled = enabled
        switch action {
        case .send: actionButton.accessibilityLabel = "Send message"
        case .queue: actionButton.accessibilityLabel = preferredDelivery == .steer ? "Steer" : "Queue message"
        case .stop: actionButton.accessibilityLabel = "Stop response"
        }
        // Long-press offers the other delivery modes mid-turn.
        if action == .queue {
            var items: [UIMenuElement] = [
                UIAction(title: "Queue for next turn", image: UIImage(systemName: "text.line.last.and.arrowtriangle.forward")) { [weak self] _ in self?.send(.queue) },
            ]
            if canSteer {
                items.append(UIAction(title: "Steer now", image: UIImage(systemName: "arrow.turn.down.right")) { [weak self] _ in self?.send(.steer) })
            }
            items.append(UIAction(title: "Stop and send", image: UIImage(systemName: "stop.circle"), attributes: .destructive) { [weak self] _ in self?.send(.interrupt) })
            actionButton.menu = UIMenu(title: "Deliver while working", children: items)
            actionButton.showsMenuAsPrimaryAction = false
        } else {
            actionButton.menu = nil
        }
    }

    private func primaryAction() {
        switch currentAction {
        case .stop:
            UIImpactFeedbackGenerator(style: .rigid).impactOccurred()
            onStop?()
        case .send:
            send(.queue)
        case .queue:
            send(canSteer ? preferredDelivery : (preferredDelivery == .steer ? .queue : preferredDelivery))
        }
    }

    private func send(_ mode: DeliveryMode) {
        guard hasContent else { return }
        UIImpactFeedbackGenerator(style: .medium).impactOccurred()
        let body = mentions.encode(textView.text.trimmingCharacters(in: .whitespacesAndNewlines))
        let staged = images
        onSend?(body, staged, mode)
        mentions.reset()
        setSuggestionsVisible(false)
        textView.text = ""
        images = []
        textChanged()
    }

    // External keyboards: ⌘↩ sends.
    override var keyCommands: [UIKeyCommand]? {
        [UIKeyCommand(title: "Send", action: #selector(commandReturn), input: "\r", modifierFlags: .command)]
    }

    @objc private func commandReturn() {
        if hasContent { primaryAction() }
    }
}
