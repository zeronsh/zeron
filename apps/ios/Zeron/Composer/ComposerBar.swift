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

/// A context chip in the composer toolbar (model, branch…).
struct ComposerChip: Equatable {
    struct Detail: Equatable {
        let text: String
        var emphasized = false
    }

    let id: String
    let title: String
    let symbol: String?
    var tint: UIColor? = nil
    /// Brand mark / custom glyph (takes precedence over `symbol`).
    var icon: UIImage? = nil
    /// The model chip's muted second tone, like the desktop chip's suffix.
    var detail: [Detail] = []
}

/// The composer. One glass surface with two states that morph into each
/// other (the same views re-anchor inside a spring animation):
///
/// - resting: a single-line capsule — [+]  Message…  [↑]
/// - active (focused, or holding a draft/photos): a card — photos, full-width
///   text, and a toolbar row inside the glass: [+] [model] [branch] … [Send]
///
/// The action button is one control that becomes Send, Queue/Steer (a labelled
/// pill while an agent works) or Stop.
final class ComposerBar: UIView, UITextViewDelegate, UIGestureRecognizerDelegate {
    enum Action: Equatable {
        case send
        case queue
        case stop
    }

    // Callbacks
    /// What happened to a send, so the composer knows whether to clear.
    enum SendResult {
        /// Taken: the composer clears.
        case sent
        /// Not taken (e.g. couldn't start the session): text and images stay.
        case kept
        /// Taken, and the handler already put the composer's next contents in
        /// place (committing a queued edit restores the stashed draft).
        case replaced
    }

    var onSend: ((String, [StagedImage], DeliveryMode) -> SendResult)?
    var onStop: (() -> Void)?
    var attachMenu: (() -> UIMenu)?
    var onChipTap: ((String, UIView) -> Void)?
    var onHeightChange: (() -> Void)?
    var onFocusChange: ((Bool) -> Void)?
    /// `@` file search (nil disables mentions).
    var mentionSearch: ((String) async -> [FileMatch])?

    // State
    var running = false { didSet { if running != oldValue { refreshAction(animated: true) } } }
    var canSteer = false { didSet { refreshAction(animated: false) } }
    var preferredDelivery: DeliveryMode = .queue { didSet { refreshAction(animated: false) } }
    var placeholder = "Message" { didSet { placeholderLabel.text = placeholder } }
    var chips: [ComposerChip] = [] { didSet { if chips != oldValue { rebuildChips() } } }
    /// Chips with a menu open it on tap (native, glassy, lazily loaded).
    var chipMenus: [String: () -> UIMenu?] = [:] { didSet { applyChipMenus() } }
    /// Stay in the card state even when idle (new-session canvas).
    var chipsAlwaysVisible = false { didSet { updateMode(animated: false) } }
    /// Keep the card while something anchored to its chips is open, even after
    /// the text view hands off the keyboard.
    var holdsCard = false { didSet { if holdsCard != oldValue { updateMode(animated: true) } } }
    var images: [StagedImage] = [] { didSet { rebuildThumbs(); refreshAction(animated: true); updateMode(animated: true) } }

    var text: String {
        get { textView.text }
        set {
            textView.text = newValue
            textChanged()
            updateMode(animated: false)
        }
    }

    let textView = UITextView()
    private let glass = Glass.surface(interactive: false, radius: 25)
    private let placeholderLabel = UILabel()
    private let attachButton = UIButton(type: .system)
    private let actionButton = UIButton(type: .system)
    private let thumbs = UIStackView()
    private let thumbsScroll = FadingScrollView()
    private let toolbar = UIView()
    private let chipStrip = UIStackView()
    private let chipScroll = FadingScrollView()
    private var chipButtons: [String: UIButton] = [:]
    private var textHeight: NSLayoutConstraint!
    private var thumbsHeight: NSLayoutConstraint!
    private var toolbarHeight: NSLayoutConstraint!
    private var compactConstraints: [NSLayoutConstraint] = []
    private var cardConstraints: [NSLayoutConstraint] = []
    private(set) var isCard = false
    private var currentAction: Action = .send
    private var mentions = MentionIndex()
    private let suggestions = MentionSuggestions()
    private var mentionQuery: (range: NSRange, query: String)?
    private var mentionTask: Task<Void, Never>?

    private let font = Fonts.ui(.sans, UIFontMetrics(forTextStyle: .body).scaledValue(for: 16.5))
    private var maxLines: Int {
        // Resting capsule previews at most two lines of a draft.
        if !isCard { return 2 }
        return traitCollection.verticalSizeClass == .compact ? 3 : 8
    }
    private static let control: CGFloat = 34
    private static let compactHeight: CGFloat = 50

    override init(frame: CGRect) {
        super.init(frame: frame)
        build()
    }

    required init?(coder: NSCoder) { fatalError() }

    private func build() {
        glass.translatesAutoresizingMaskIntoConstraints = false
        addSubview(glass)
        let content = glass.contentView
        // Tapping anywhere on the surface focuses the input.
        let tap = UITapGestureRecognizer(target: self, action: #selector(focusFromTap))
        tap.cancelsTouchesInView = false
        tap.delegate = self
        glass.addGestureRecognizer(tap)

        thumbsScroll.showsHorizontalScrollIndicator = false
        thumbsScroll.translatesAutoresizingMaskIntoConstraints = false
        thumbs.axis = .horizontal
        thumbs.spacing = 8
        thumbs.translatesAutoresizingMaskIntoConstraints = false
        thumbsScroll.addSubview(thumbs)
        content.addSubview(thumbsScroll)

        var attach = UIButton.Configuration.filled()
        attach.image = UIImage(systemName: "plus", withConfiguration: UIImage.SymbolConfiguration(pointSize: 15, weight: .semibold))
        attach.baseForegroundColor = Palette.text
        attach.baseBackgroundColor = Palette.controlFill
        attach.cornerStyle = .capsule
        attach.contentInsets = .zero
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
        textView.tintColor = Palette.accent
        textView.backgroundColor = .clear
        textView.delegate = self
        textView.isScrollEnabled = false
        textView.textContainerInset = UIEdgeInsets(top: 14, left: 0, bottom: 14, right: 0)
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

        toolbar.translatesAutoresizingMaskIntoConstraints = false
        toolbar.clipsToBounds = true
        // Beneath the controls: the row spans the card, and above them it
        // would swallow taps on "+" (the action button is added later).
        content.insertSubview(toolbar, at: 0)
        chipScroll.showsHorizontalScrollIndicator = false
        chipScroll.translatesAutoresizingMaskIntoConstraints = false
        chipStrip.axis = .horizontal
        chipStrip.spacing = 6
        chipStrip.alignment = .center
        chipStrip.translatesAutoresizingMaskIntoConstraints = false
        chipScroll.addSubview(chipStrip)
        toolbar.addSubview(chipScroll)

        actionButton.translatesAutoresizingMaskIntoConstraints = false
        actionButton.accessibilityIdentifier = "composer-send"
        actionButton.addAction(UIAction { [weak self] _ in self?.primaryAction() }, for: .touchUpInside)
        actionButton.setContentHuggingPriority(.required, for: .horizontal)
        content.addSubview(actionButton)

        let c = Self.control
        textHeight = textView.heightAnchor.constraint(equalToConstant: Self.compactHeight)
        thumbsHeight = thumbsScroll.heightAnchor.constraint(equalToConstant: 0)
        toolbarHeight = toolbar.heightAnchor.constraint(equalToConstant: 0)
        NSLayoutConstraint.activate([
            glass.topAnchor.constraint(equalTo: topAnchor),
            glass.leadingAnchor.constraint(equalTo: leadingAnchor),
            glass.trailingAnchor.constraint(equalTo: trailingAnchor),
            glass.bottomAnchor.constraint(equalTo: bottomAnchor),

            thumbsScroll.topAnchor.constraint(equalTo: content.topAnchor),
            thumbsScroll.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 12),
            thumbsScroll.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -12),
            thumbsHeight,
            thumbs.topAnchor.constraint(equalTo: thumbsScroll.contentLayoutGuide.topAnchor),
            thumbs.bottomAnchor.constraint(equalTo: thumbsScroll.contentLayoutGuide.bottomAnchor),
            thumbs.leadingAnchor.constraint(equalTo: thumbsScroll.contentLayoutGuide.leadingAnchor),
            thumbs.trailingAnchor.constraint(equalTo: thumbsScroll.contentLayoutGuide.trailingAnchor),
            thumbs.heightAnchor.constraint(equalTo: thumbsScroll.frameLayoutGuide.heightAnchor),

            textView.topAnchor.constraint(equalTo: thumbsScroll.bottomAnchor),
            textHeight,
            toolbar.topAnchor.constraint(equalTo: textView.bottomAnchor),
            toolbar.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            toolbar.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            toolbar.bottomAnchor.constraint(equalTo: content.bottomAnchor),
            toolbarHeight,

            placeholderLabel.leadingAnchor.constraint(equalTo: textView.leadingAnchor),
            placeholderLabel.trailingAnchor.constraint(lessThanOrEqualTo: textView.trailingAnchor),
            placeholderLabel.topAnchor.constraint(equalTo: textView.topAnchor, constant: 14),

            attachButton.widthAnchor.constraint(equalToConstant: c),
            attachButton.heightAnchor.constraint(equalToConstant: c),
            actionButton.heightAnchor.constraint(equalToConstant: c),
            actionButton.widthAnchor.constraint(greaterThanOrEqualToConstant: c),

            chipScroll.leadingAnchor.constraint(equalTo: attachButton.trailingAnchor, constant: 8),
            chipScroll.trailingAnchor.constraint(equalTo: actionButton.leadingAnchor, constant: -8),
            chipScroll.centerYAnchor.constraint(equalTo: toolbar.centerYAnchor),
            chipScroll.heightAnchor.constraint(equalToConstant: c),
            chipStrip.topAnchor.constraint(equalTo: chipScroll.contentLayoutGuide.topAnchor),
            chipStrip.bottomAnchor.constraint(equalTo: chipScroll.contentLayoutGuide.bottomAnchor),
            chipStrip.leadingAnchor.constraint(equalTo: chipScroll.contentLayoutGuide.leadingAnchor),
            chipStrip.trailingAnchor.constraint(equalTo: chipScroll.contentLayoutGuide.trailingAnchor),
            chipStrip.heightAnchor.constraint(equalTo: chipScroll.frameLayoutGuide.heightAnchor),
        ])
        // Resting capsule: controls at the capsule's ends, text between them.
        compactConstraints = [
            attachButton.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 8),
            attachButton.bottomAnchor.constraint(equalTo: content.bottomAnchor, constant: -8),
            actionButton.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -8),
            actionButton.bottomAnchor.constraint(equalTo: content.bottomAnchor, constant: -8),
            textView.leadingAnchor.constraint(equalTo: attachButton.trailingAnchor, constant: 10),
            textView.trailingAnchor.constraint(equalTo: actionButton.leadingAnchor, constant: -8),
        ]
        // Card: full-width text, controls in the toolbar row.
        cardConstraints = [
            attachButton.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 10),
            attachButton.centerYAnchor.constraint(equalTo: toolbar.centerYAnchor),
            actionButton.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -10),
            actionButton.centerYAnchor.constraint(equalTo: toolbar.centerYAnchor),
            textView.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 16),
            textView.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -16),
        ]
        NSLayoutConstraint.activate(compactConstraints)
        chipScroll.alpha = 0
        suggestions.isHidden = true
        suggestions.translatesAutoresizingMaskIntoConstraints = false
        suggestions.onPick = { [weak self] file in self?.insertMention(file) }
        refreshAction(animated: false)
    }

    /// Taps on controls (attach, chips, send) are theirs: focusing would
    /// morph the capsule under the finger and cancel the control's menu.
    func gestureRecognizer(_ g: UIGestureRecognizer, shouldReceive touch: UITouch) -> Bool {
        var v = touch.view
        while let view = v, view !== glass {
            if view is UIControl { return false }
            v = view.superview
        }
        return true
    }

    @objc private func focusFromTap() {
        if !textView.isFirstResponder { textView.becomeFirstResponder() }
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
            suggestions.bottomAnchor.constraint(equalTo: topAnchor, constant: -8),
        ])
    }

    // MARK: Resting ↔ card

    /// The card is for composing: focused, holding photos, or pinned open
    /// (new-session canvas). An unfocused draft rests as the capsule.
    private var wantsCard: Bool {
        chipsAlwaysVisible || holdsCard || textView.isFirstResponder || !images.isEmpty
    }

    private func updateMode(animated: Bool) {
        let card = wantsCard
        guard card != isCard else { return }
        isCard = card
        let change = {
            if card {
                NSLayoutConstraint.deactivate(self.compactConstraints)
                NSLayoutConstraint.activate(self.cardConstraints)
            } else {
                NSLayoutConstraint.deactivate(self.cardConstraints)
                NSLayoutConstraint.activate(self.compactConstraints)
            }
            self.toolbarHeight.constant = card ? 50 : 0
            self.chipScroll.alpha = card ? 1 : 0
            self.textView.textContainerInset = card ? UIEdgeInsets(top: 14, left: 0, bottom: 6, right: 0) : UIEdgeInsets(top: 14, left: 0, bottom: 14, right: 0)
            self.glass.cornerConfiguration = .uniformCorners(radius: .fixed(card ? 26 : 25))
            self.recomputeTextHeight()
            self.onHeightChange?()
            self.superview?.layoutIfNeeded()
        }
        guard animated, window != nil, !UIAccessibility.isReduceMotionEnabled else { return change() }
        UIView.animate(withDuration: 0.42, delay: 0, usingSpringWithDamping: 0.86, initialSpringVelocity: 0, options: [.allowUserInteraction, .beginFromCurrentState], animations: change)
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
        updateMode(animated: true)
        onFocusChange?(true)
    }

    func textViewDidEndEditing(_ textView: UITextView) {
        setSuggestionsVisible(false)
        updateMode(animated: true)
        textView.setContentOffset(.zero, animated: false)
        onFocusChange?(false)
    }

    /// A popover anchors to the chip, so it must show in full.
    func reveal(_ chip: UIView) {
        guard chip.isDescendant(of: chipScroll) else { return }
        chipScroll.scrollRectToVisible(chip.convert(chip.bounds, to: chipScroll), animated: false)
    }

    private func rebuildChips() {
        let ids = Set(chips.map(\.id))
        let removed = chipButtons.keys.filter { !ids.contains($0) }
        for id in removed {
            guard let button = chipButtons.removeValue(forKey: id) else { continue }
            chipStrip.removeArrangedSubview(button)
            button.removeFromSuperview()
        }
        let buttons = chips.map { chip in
            let button = chipButtons[chip.id] ?? makeChipButton(id: chip.id)
            chipButtons[chip.id] = button
            button.configuration = Self.chipConfiguration(chip)
            button.accessibilityLabel = chip.detail.isEmpty ? nil : ([chip.title] + chip.detail.map(\.text)).joined(separator: ", ")
            return button
        }
        let ordered = chipStrip.arrangedSubviews.count == buttons.count
            && zip(chipStrip.arrangedSubviews, buttons).allSatisfy { pair in pair.0 === pair.1 }
        if !ordered {
            chipStrip.arrangedSubviews.forEach { chipStrip.removeArrangedSubview($0) }
            buttons.forEach { chipStrip.addArrangedSubview($0) }
        }
        applyChipMenus()
    }

    private func makeChipButton(id: String) -> UIButton {
        let button = UIButton(configuration: .filled())
        button.accessibilityIdentifier = "composer-chip-\(id)"
        button.setContentHuggingPriority(.required, for: .horizontal)
        button.setContentCompressionResistancePriority(.required, for: .horizontal)
        button.addAction(UIAction { [weak self, weak button] _ in
            guard let self, let button else { return }
            self.onChipTap?(id, button)
        }, for: .touchUpInside)
        return button
    }

    private static func chipConfiguration(_ chip: ComposerChip) -> UIButton.Configuration {
        var config = UIButton.Configuration.filled()
        if let icon = chip.icon {
            config.image = icon
        } else if let symbol = chip.symbol {
            config.image = UIImage(systemName: symbol, withConfiguration: UIImage.SymbolConfiguration(pointSize: 11.5, weight: .semibold))
        }
        config.imagePadding = chip.id == "pr" ? 5 : 6
        // Tinted chips (the PR badge) use the desktop's tone wash: fill
        // @ 0.08, ink @ 0.85.
        config.baseForegroundColor = chip.tint.map { $0.withAlphaComponent(0.85) } ?? Palette.text
        config.baseBackgroundColor = chip.tint.map { $0.withAlphaComponent(0.1) } ?? Palette.controlFill
        config.cornerStyle = .capsule
        config.contentInsets = NSDirectionalEdgeInsets(top: 7, leading: 11, bottom: 7, trailing: 11)
        let mono = chip.id == "branch" || chip.id == "pr"
        let titleFont = Fonts.ui(mono ? .monoMedium : .sansMedium, mono ? 12.5 : 13.5)
        var attributed = AttributedString(chip.title)
        attributed.font = titleFont
        if !chip.detail.isEmpty {
            var spacing = AttributedString("  ")
            spacing.font = titleFont
            attributed.append(spacing)
            for (index, detail) in chip.detail.enumerated() {
                if index > 0 {
                    var separator = AttributedString(" · ")
                    separator.font = titleFont
                    separator.foregroundColor = Palette.secondary
                    attributed.append(separator)
                }
                var part = AttributedString(detail.text)
                part.font = titleFont
                part.foregroundColor = detail.emphasized ? Palette.text.withAlphaComponent(0.85) : Palette.secondary
                attributed.append(part)
            }
        }
        config.attributedTitle = attributed
        return config
    }

    private func applyChipMenus() {
        for (id, button) in chipButtons {
            button.menu = chipMenus[id]?()
            button.showsMenuAsPrimaryAction = button.menu != nil
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
            iv.layer.cornerRadius = 14
            iv.layer.cornerCurve = .continuous
            iv.layer.borderWidth = 1 / max(1, traitCollection.displayScale)
            iv.layer.borderColor = Palette.hairline.resolvedColor(with: window?.traitCollection ?? traitCollection).cgColor
            iv.isUserInteractionEnabled = true
            iv.translatesAutoresizingMaskIntoConstraints = false
            iv.widthAnchor.constraint(equalToConstant: 56).isActive = true
            iv.heightAnchor.constraint(equalToConstant: 56).isActive = true
            var x = UIButton.Configuration.filled()
            x.image = UIImage(systemName: "xmark", withConfiguration: UIImage.SymbolConfiguration(pointSize: 8, weight: .bold))
            x.baseBackgroundColor = UIColor.black.withAlphaComponent(0.55)
            x.baseForegroundColor = .white
            x.cornerStyle = .capsule
            x.contentInsets = .zero
            let remove = UIButton(configuration: x)
            remove.accessibilityLabel = "Remove image"
            remove.translatesAutoresizingMaskIntoConstraints = false
            remove.addAction(UIAction { [weak self] _ in
                UIView.animate(withDuration: 0.25) { self?.images.removeAll { $0.id == img.id } }
            }, for: .touchUpInside)
            iv.addSubview(remove)
            NSLayoutConstraint.activate([
                remove.topAnchor.constraint(equalTo: iv.topAnchor, constant: 4),
                remove.trailingAnchor.constraint(equalTo: iv.trailingAnchor, constant: -4),
                remove.widthAnchor.constraint(equalToConstant: 18),
                remove.heightAnchor.constraint(equalToConstant: 18),
            ])
            thumbs.addArrangedSubview(iv)
        }
        thumbsHeight.constant = images.isEmpty ? 0 : 68
        thumbs.layoutMargins = UIEdgeInsets(top: 12, left: 0, bottom: 0, right: 0)
        thumbs.isLayoutMarginsRelativeArrangement = true
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

    private func recomputeTextHeight() {
        let width = max(40, textView.bounds.width)
        let fitting = textView.sizeThatFits(CGSize(width: width, height: .greatestFiniteMagnitude)).height
        let insets = textView.textContainerInset.top + textView.textContainerInset.bottom
        let maxHeight = font.lineHeight * CGFloat(maxLines) + insets
        let minHeight: CGFloat = isCard ? 46 : Self.compactHeight
        textView.isScrollEnabled = fitting > maxHeight
        textHeight.constant = min(max(minHeight, fitting), maxHeight)
    }

    private func textChanged() {
        placeholderLabel.isHidden = !textView.text.isEmpty
        let before = textHeight.constant
        updateMode(animated: true)
        recomputeTextHeight()
        if abs(textHeight.constant - before) > 0.5 {
            UIView.animate(withDuration: 0.18, delay: 0, options: [.beginFromCurrentState, .allowUserInteraction]) {
                self.onHeightChange?()
                self.superview?.layoutIfNeeded()
            }
        }
        refreshAction(animated: true)
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        if textView.bounds.width > 0, !textView.text.isEmpty, abs(textView.contentSize.height - textHeight.constant) > 1 {
            recomputeTextHeight()
        }
    }

    // MARK: Action button

    private var hasContent: Bool {
        !textView.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || !images.isEmpty
    }

    private func refreshAction(animated: Bool) {
        let action: Action = running ? (hasContent ? .queue : .stop) : .send
        let enabled = action == .stop || hasContent
        let steering = action == .queue && preferredDelivery == .steer && canSteer
        var config = UIButton.Configuration.filled()
        config.cornerStyle = .capsule
        let symbol: String
        switch action {
        case .send:
            symbol = "arrow.up"
            config.baseBackgroundColor = enabled ? Palette.accent : Palette.controlFill
            config.baseForegroundColor = enabled ? .white : Palette.tertiary
            config.contentInsets = .zero
        case .queue:
            // Same look as Send (the long-press menu still offers steer /
            // stop-and-send); only the accessibility label says "Queue".
            symbol = "arrow.up"
            config.baseBackgroundColor = Palette.accent
            config.baseForegroundColor = .white
            config.contentInsets = .zero
        case .stop:
            symbol = "stop.fill"
            config.baseBackgroundColor = Palette.text
            config.baseForegroundColor = Palette.background
            config.contentInsets = .zero
        }
        config.image = UIImage(systemName: symbol, withConfiguration: UIImage.SymbolConfiguration(pointSize: action == .stop ? 11 : 15, weight: .bold))
        let changed = currentAction != action
        currentAction = action
        let apply = {
            self.actionButton.configuration = config
            self.actionButton.isEnabled = enabled
            self.superview?.layoutIfNeeded()
        }
        if animated, changed, window != nil {
            UIView.animate(withDuration: 0.32, delay: 0, usingSpringWithDamping: 0.8, initialSpringVelocity: 0, options: [.allowUserInteraction, .beginFromCurrentState], animations: apply)
            actionButton.imageView?.addSymbolEffect(.bounce.byLayer, options: .nonRepeating)
        } else {
            apply()
        }
        switch action {
        case .send: actionButton.accessibilityLabel = "Send message"
        case .queue: actionButton.accessibilityLabel = steering ? "Steer" : "Queue message"
        case .stop: actionButton.accessibilityLabel = "Stop response"
        }
        // Long-press offers the other delivery modes mid-turn.
        if action == .queue {
            var items: [UIMenuElement] = [
                UIAction(title: "Queue for next turn", image: UIImage(systemName: "text.line.last.and.arrowtriangle.forward")) { [weak self] _ in self?.send(.queue) },
            ]
            if canSteer {
                items.append(UIAction(title: "Steer now", subtitle: "Deliver into the running turn", image: UIImage(systemName: "arrow.turn.down.right")) { [weak self] _ in self?.send(.steer) })
            }
            items.append(UIAction(title: "Stop and send", image: UIImage(systemName: "stop.circle"), attributes: .destructive) { [weak self] _ in self?.send(.interrupt) })
            actionButton.menu = UIMenu(title: "While the agent works", children: items)
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
        let result = onSend?(body, staged, mode) ?? .sent
        guard result != .kept else { return }
        setSuggestionsVisible(false)
        // `.replaced` put a draft back: its @file mentions must still resolve.
        guard result == .sent else { return }
        mentions.reset()
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
