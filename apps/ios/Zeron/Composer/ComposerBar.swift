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

/// A context chip in the composer toolbar (model, effort, branch…).
struct ComposerChip: Equatable {
    let id: String
    let title: String
    let symbol: String?
    var tint: UIColor? = nil
    /// Brand mark / custom glyph (takes precedence over `symbol`).
    var icon: UIImage? = nil
}

/// The composer. One glass surface with two states that morph into each
/// other (the same views re-anchor inside a spring animation):
///
/// - resting: a single-line capsule — [+]  Message…  [↑]
/// - active (focused, or holding a draft/photos): a card — photos, full-width
///   text, and a toolbar row inside the glass: [+] [model] [effort] [branch] … [Send]
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
    /// Where a dictation's words are written (the chat's host); nil when
    /// there is none to write them.
    var dictationTarget: (() -> DictationTarget?)?
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

    // Dictation (see `Voice/Dictation.swift`): the microphone is offered
    // while nothing else has it; a voice call takes it away.
    var dictationEnabled = true {
        didSet {
            guard dictationEnabled != oldValue else { return }
            if !dictationEnabled { cancelDictation() }
            micWidth.constant = dictationEnabled ? Self.control : 0
            micGap.constant = dictationEnabled ? -8 : 0
            micButton.isHidden = !dictationEnabled
            superview?.layoutIfNeeded()
        }
    }
    private let micButton = UIButton(type: .system)
    private let track = DictationTrack()
    private var micWidth: NSLayoutConstraint!
    private var micGap: NSLayoutConstraint!
    private var dictation = DictationDraft()
    private var transcriber: DictationTranscriber?
    private var dictationLink: CADisplayLink?
    /// Words are being put in by the dictation, not typed.
    private var applyingDictation = false
    private var pendingDelivery: DeliveryMode = .queue
    /// The track's unrolling, 0 (away) to 1 (across the controls), and
    /// the passage it is on.
    private var trackT: CGFloat = 0
    private var trackMorph: (from: CGFloat, to: CGFloat, start: CFTimeInterval)?
    private static let trackMorphDuration: CFTimeInterval = 0.42

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

        // The microphone: a quiet glyph beside Send. One tap starts the
        // dictation; the same button, now a stop, ends it.
        var mic = UIButton.Configuration.plain()
        mic.image = UIImage(systemName: "microphone", withConfiguration: UIImage.SymbolConfiguration(pointSize: 17, weight: .medium))
        mic.baseForegroundColor = Palette.secondary
        mic.contentInsets = .zero
        micButton.configuration = mic
        micButton.accessibilityIdentifier = "composer-dictation"
        micButton.accessibilityHint = "Tap to start, tap again to stop and transcribe."
        micButton.translatesAutoresizingMaskIntoConstraints = false
        micButton.addAction(UIAction { [weak self] _ in self?.toggleDictation() }, for: .touchUpInside)
        content.addSubview(micButton)
        track.alpha = 0
        track.isHidden = true
        content.addSubview(track)

        let c = Self.control
        textHeight = textView.heightAnchor.constraint(equalToConstant: Self.compactHeight)
        thumbsHeight = thumbsScroll.heightAnchor.constraint(equalToConstant: 0)
        toolbarHeight = toolbar.heightAnchor.constraint(equalToConstant: 0)
        micWidth = micButton.widthAnchor.constraint(equalToConstant: c)
        micGap = micButton.trailingAnchor.constraint(equalTo: actionButton.leadingAnchor, constant: -8)
        NSLayoutConstraint.activate([
            micWidth,
            micGap,
            micButton.heightAnchor.constraint(equalToConstant: c),
            micButton.centerYAnchor.constraint(equalTo: actionButton.centerYAnchor),
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
            chipScroll.trailingAnchor.constraint(equalTo: micButton.leadingAnchor, constant: -8),
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
            textView.trailingAnchor.constraint(equalTo: micButton.leadingAnchor, constant: -6),
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
            if view is UIControl || view === track { return false }
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

    /// The card is for composing: focused, holding photos, pinned open
    /// (new-session canvas) or taking a dictation, whose words show above
    /// the track as they land. An unfocused draft rests as the capsule.
    private var wantsCard: Bool {
        chipsAlwaysVisible || textView.isFirstResponder || !images.isEmpty || dictation.phase.active
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

    private func rebuildChips() {
        chipStrip.arrangedSubviews.forEach { $0.removeFromSuperview() }
        for chip in chips {
            var config = UIButton.Configuration.filled()
            config.title = chip.title
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
            config.titleTextAttributesTransformer = UIConfigurationTextAttributesTransformer { attrs in
                var a = attrs
                // PR numbers stay mono like the list's PR badges; the checkout
                // chip (branch / New worktree) reads as ordinary UI text.
                let mono = chip.id == "pr"
                a.font = Fonts.ui(mono ? .monoMedium : .sansMedium, mono ? 12.5 : 13.5)
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
        // Typing over a dictation ends it: the words typed stay.
        if !applyingDictation, dictation.phase.active { cancelDictation() }
        textChanged()
        styleMentions()
        updateMentionQuery()
    }

    func textViewDidChangeSelection(_ textView: UITextView) {
        if mentionQuery != nil { updateMentionQuery() }
    }

    private func recomputeTextHeight() {
        // At rest a draft is a preview: whole lines ending in "…", never a
        // scrolled view whose next line bleeds into the bottom inset.
        let preview = !isCard
        let lines = preview ? maxLines : 0
        let breakMode: NSLineBreakMode = preview ? .byTruncatingTail : .byWordWrapping
        if textView.textContainer.maximumNumberOfLines != lines || textView.textContainer.lineBreakMode != breakMode {
            textView.textContainer.maximumNumberOfLines = lines
            textView.textContainer.lineBreakMode = breakMode
        }
        let width = max(40, textView.bounds.width)
        let fitting = textView.sizeThatFits(CGSize(width: width, height: .greatestFiniteMagnitude)).height
        let insets = textView.textContainerInset.top + textView.textContainerInset.bottom
        let maxHeight = font.lineHeight * CGFloat(maxLines) + insets
        let minHeight: CGFloat = isCard ? 46 : Self.compactHeight
        textView.isScrollEnabled = !preview && fitting > maxHeight
        if preview { textView.contentOffset = .zero }
        textHeight.constant = preview ? max(minHeight, fitting) : min(max(minHeight, fitting), maxHeight)
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
        layoutTrack()
    }

    // MARK: Action button

    private var hasContent: Bool {
        !textView.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || !images.isEmpty
    }

    private func refreshAction(animated: Bool) {
        // Dictating, Send waits for the words and sends them once.
        let content = hasContent || dictation.phase.active
        let action: Action = running ? (content ? .queue : .stop) : .send
        let enabled = action == .stop || content
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
        // Send during a dictation waits for its final words, then sends
        // them once, the way they were asked to be delivered.
        if dictation.phase.active {
            pendingDelivery = mode
            finishDictation(send: true)
            return
        }
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

    // External keyboards: ⌘↩ sends; Esc cancels a dictation.
    override var keyCommands: [UIKeyCommand]? {
        var commands = [UIKeyCommand(title: "Send", action: #selector(commandReturn), input: "\r", modifierFlags: .command)]
        if dictation.phase.active {
            commands.append(UIKeyCommand(title: "Cancel dictation", action: #selector(escape), input: UIKeyCommand.inputEscape))
        }
        return commands
    }

    @objc private func commandReturn() {
        if hasContent || dictation.phase.active { primaryAction() }
    }

    @objc private func escape() {
        cancelDictation()
    }

    // MARK: Dictation

    /// Put away (a question panel takes the composer's place) or off screen,
    /// a dictation cannot land anywhere: it ends.
    override var isHidden: Bool {
        didSet { if isHidden { cancelDictation() } }
    }

    override func willMove(toWindow newWindow: UIWindow?) {
        super.willMove(toWindow: newWindow)
        if newWindow == nil { cancelDictation() }
    }

    private func makeTranscriber() -> DictationTranscriber? {
        #if DEBUG
        if ProcessInfo.processInfo.arguments.contains("-dictation-preview") { return ScriptedDictation() }
        #endif
        guard let target = dictationTarget?() else { return nil }
        return RemoteDictation(target: target)
    }

    #if DEBUG
    /// A scripted dictation, listening for five seconds then transcribing,
    /// for screenshots and UI work (with `-dictation-preview`).
    func previewDictation() {
        beginDictation()
        DispatchQueue.main.asyncAfter(deadline: .now() + 5) { [weak self] in self?.finishDictation(send: false) }
    }
    #endif

    /// The microphone: idle it starts, listening it stops and transcribes,
    /// opening it cancels; transcribing it waits.
    private func toggleDictation() {
        switch dictation.phase {
        case .listening: finishDictation(send: false)
        case .requesting: cancelDictation()
        case .finalizing: break
        default: beginDictation()
        }
    }

    private func beginDictation() {
        guard dictationEnabled, transcriber == nil, !dictation.phase.active else { return }
        guard textView.markedTextRange == nil else {
            explain(.unavailable("Finish composing the current character before dictating."))
            return
        }
        guard let transcriber = makeTranscriber() else {
            explain(.unavailable("This chat has no host to write the dictation on."))
            return
        }
        dictation.begin(content: textView.text, selection: textView.selectedRange)
        let generation = dictation.generation
        transcriber.onEvent = { [weak self] event in self?.dictated(event, generation: generation) }
        self.transcriber = transcriber
        track.glyph.orb = .connecting
        track.isHidden = false
        UIImpactFeedbackGenerator(style: .light).impactOccurred()
        transcriber.start()
        startDictationLink()
        updateMode(animated: true)
        morphTrack(to: 1)
        refreshAction(animated: true)
        refreshDictationControls()
    }

    /// Ends capture; the final words follow. `send` submits them once in.
    private func finishDictation(send: Bool) {
        guard dictation.phase.active else { return }
        guard dictation.finish(send: send, now: Date()) else { return }
        transcriber?.finish()
        track.glyph.orb = .transcribing
        UIImpactFeedbackGenerator(style: .light).impactOccurred()
        refreshDictationControls()
    }

    /// Drops the dictation and anything it still owed; the words already
    /// in the draft stay.
    func cancelDictation() {
        guard dictation.phase.active || transcriber != nil else { return }
        dictation.cancel()
        transcriber?.cancel()
        transcriber = nil
        settleDictation()
    }

    private func completeDictation(_ phase: DictationDraft.Phase) {
        let send = dictation.complete(phase)
        transcriber?.cancel()
        transcriber = nil
        settleDictation()
        explain(phase)
        if send { self.send(pendingDelivery) }
    }

    private func settleDictation() {
        morphTrack(to: 0)
        stopDictationLink()
        updateMode(animated: true)
        refreshAction(animated: true)
        refreshDictationControls()
    }

    private func dictated(_ event: DictationEvent, generation: UInt64) {
        guard generation == dictation.generation, dictation.phase.active else { return }
        switch event {
        case .listening:
            dictation.listen(now: Date())
            track.glyph.orb = .listening
            UIAccessibility.post(notification: .announcement, argument: "Listening")
            refreshDictationControls()
        case let .partial(words):
            applyDictation(words)
        case .finalizing:
            finishDictation(send: false)
        case .interrupted:
            dictation.dropPendingSend()
            finishDictation(send: false)
        case let .final(words):
            if words.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty, !dictation.hasPartial {
                completeDictation(.noSpeech)
            } else {
                applyDictation(words)
                completeDictation(.idle)
            }
        case let .denied(message): completeDictation(.denied(message))
        case let .unavailable(message): completeDictation(.unavailable(message))
        case let .failed(message): completeDictation(.failed(message))
        }
    }

    private func applyDictation(_ words: String) {
        var content: String = textView.text
        guard let cursor = dictation.replace(in: &content, with: words) else {
            if !dictation.phase.active { cancelDictation() }
            return
        }
        applyingDictation = true
        textView.text = content
        textView.selectedRange = NSRange(location: cursor, length: 0)
        applyingDictation = false
        textChanged()
        styleMentions()
    }

    /// What the microphone and the track say of the dictation.
    private func refreshDictationControls() {
        let phase = dictation.phase
        micButton.accessibilityLabel = phase.actionLabel
        micButton.isEnabled = dictationEnabled && phase != .finalizing
        var mic = micButton.configuration
        mic?.image = UIImage(systemName: phase.active ? "stop.fill" : "microphone", withConfiguration: UIImage.SymbolConfiguration(pointSize: phase.active ? 13 : 17, weight: phase.active ? .bold : .medium))
        mic?.baseForegroundColor = phase.active ? Palette.text : Palette.secondary
        micButton.configuration = mic
        showDictationClock()
    }

    private func showDictationClock() {
        let phase = dictation.phase
        let label: String
        switch phase {
        case .requesting: label = "Getting ready"
        case .listening: label = "Listening"
        case .finalizing: label = "Transcribing"
        default: label = ""
        }
        let elapsed = dictation.elapsed(now: Date())
        var status = label
        if let elapsed { status += String(format: ", %d seconds", Int(elapsed)) }
        track.show(elapsed: elapsed, live: phase == .listening, label: status)
    }

    private func explain(_ phase: DictationDraft.Phase) {
        let settings = { if let url = URL(string: UIApplication.openSettingsURLString) { UIApplication.shared.open(url) } }
        switch phase {
        case .noSpeech:
            Toast.show("No speech detected. Check your microphone, then try again.", in: window)
        case let .denied(message):
            Toast.show(message, action: "Settings", in: window, handler: settings)
        case let .unavailable(message), let .failed(message):
            Toast.show(message, in: window)
        default:
            break
        }
    }

    // MARK: Dictation: the track and its clock

    private func startDictationLink() {
        guard dictationLink == nil else { return }
        let link = CADisplayLink(target: DictationLinkProxy(self), selector: #selector(DictationLinkProxy.tick))
        link.preferredFrameRateRange = CAFrameRateRange(minimum: 30, maximum: 60, preferred: 60)
        link.add(to: .main, forMode: .common)
        dictationLink = link
    }

    private func stopDictationLink() {
        // The closing morph still needs frames; the link ends with it.
        guard trackMorph == nil, !dictation.phase.active else { return }
        dictationLink?.invalidate()
        dictationLink = nil
    }

    fileprivate func tickDictation() {
        let now = Date()
        if dictation.phase.active {
            // Capture closed, the beam hears silence and settles.
            let level = dictation.phase == .listening ? transcriber?.takeLevel() ?? 0 : 0
            track.glyph.setLevels(microphone: level, speaker: 0)
            showDictationClock()
            if dictation.timedOut(now: now) {
                completeDictation(.failed("Dictation took too long. Your draft is safe. Try a shorter recording."))
                return
            }
        }
        if let morph = trackMorph {
            let raw = (CACurrentMediaTime() - morph.start) / Self.trackMorphDuration
            let t = Self.trackCurve(min(1, max(0, CGFloat(raw))))
            trackT = morph.from + (morph.to - morph.from) * t
            if raw >= 1 {
                trackT = morph.to
                trackMorph = nil
            }
            setNeedsLayout()
            layoutIfNeeded()
        }
        if trackMorph == nil, !dictation.phase.active { stopDictationLink() }
    }

    /// Unrolls the track across the controls (1) or folds it away (0),
    /// over 420 ms on `cubic-bezier(0.2, 0, 0, 1)`; a retarget starts from
    /// where it is, so toggling mid-flight reverses without a jump.
    private func morphTrack(to target: CGFloat) {
        guard trackMorph?.to != target, trackT != target || trackMorph != nil else { return }
        if UIAccessibility.isReduceMotionEnabled || window == nil {
            trackMorph = nil
            trackT = target
            setNeedsLayout()
            return
        }
        trackMorph = (from: trackT, to: target, start: CACurrentMediaTime())
        startDictationLink()
    }

    /// `cubic-bezier(0.2, 0, 0, 1)` as y of x.
    private static func trackCurve(_ x: CGFloat) -> CGFloat {
        let (x1, y1, x2, y2): (CGFloat, CGFloat, CGFloat, CGFloat) = (0.2, 0, 0, 1)
        func at(_ t: CGFloat, _ a: CGFloat, _ b: CGFloat) -> CGFloat {
            let u = 1 - t
            return 3 * u * u * t * a + 3 * u * t * t * b + t * t * t
        }
        var t = x
        for _ in 0..<8 {
            let u = 1 - t
            let slope = 3 * u * u * x1 + 6 * u * t * (x2 - x1) + 3 * t * t * (1 - x2)
            guard slope > 1e-6 else { break }
            t -= (at(t, x1, x2) - x) / slope
            t = min(1, max(0, t))
        }
        return at(t, y1, y2)
    }

    /// The track runs from the attach button to the microphone, which
    /// stays as the stop; what it covers (the chips, or at rest the text)
    /// waits beneath it.
    private func layoutTrack() {
        let t = trackT
        let content = glass.contentView
        if track.isHidden, t <= 0, !dictation.phase.active { return }
        // The controls' frames settle a pass after this view's: ask for them now.
        content.layoutIfNeeded()
        let right = micButton.frame.minX - 6
        let left = attachButton.frame.maxX + 8
        let span = max(DictationTrack.height, right - left)
        let width = DictationTrack.height + (span - DictationTrack.height) * t
        let frame = CGRect(x: right - width, y: actionButton.frame.midY - DictationTrack.height / 2, width: width, height: DictationTrack.height)
        if track.frame != frame { track.frame = frame }
        if track.superview === content, content.subviews.last !== track { content.bringSubviewToFront(track) }
        track.alpha = t
        track.isHidden = t <= 0 && !dictation.phase.active
        // Away, the beam holds still and its clock stops.
        if track.isHidden { track.glyph.orb = .idle }
        let beneath = 1 - t
        chipScroll.alpha = isCard ? beneath : 0
        if !isCard { textView.alpha = beneath } else if textView.alpha != 1 { textView.alpha = 1 }
    }
}

/// CADisplayLink retains its target; the proxy keeps the composer releasable.
private final class DictationLinkProxy: NSObject {
    weak var bar: ComposerBar?
    init(_ bar: ComposerBar) { self.bar = bar }
    @objc func tick() { MainActor.assumeIsolated { bar?.tickDictation() } }
}
