import UIKit

/// Cloud's "Add an API key" sheet — the desktop dialog on the phone
/// (`crates/ui/src/settings/cloud_key.rs`). The masked key echoes the four
/// characters Cloud will list it by; an Anthropic key then says where its
/// requests go: Anthropic's API, or an LLM gateway whose base URL the core
/// checks as it is typed (the desktop's rules, in its words) and previews as
/// the URL Claude Code will call. The core checks again when it saves.
final class APIKeySheetViewController: UIViewController, UITextFieldDelegate {
    private enum Endpoint { case direct, gateway }

    private static let gatewayDetail = "Your own base URL"

    private let agent: CloudStore.Agent
    private let save: @MainActor (String, String?) async throws -> Void
    /// Only an Anthropic key names a base URL.
    private let takesBaseUrl: Bool
    private var endpoint = Endpoint.direct
    private var saving = false

    private let scroll = FadingEdgesScrollView()
    private let content = UIStackView()
    private let keyField = UITextField()
    private let suffixLabel = UILabel()
    private let pasteControl: UIPasteControl
    private let urlField = UITextField()
    private let gatewayBlock = UIStackView()
    private let routeLine = StatusLine()
    private let failureLine = StatusLine()
    private var directChoice: EndpointChoice?
    private var gatewayChoice: EndpointChoice?
    private var saveButton: UIButton!

    /// The sheet, ready to present; `save` stores the key (and the base URL
    /// for a gateway) and throws what to show if it couldn't.
    static func sheet(agent: CloudStore.Agent, save: @escaping @MainActor (String, String?) async throws -> Void) -> UIViewController {
        let nav = UINavigationController(rootViewController: APIKeySheetViewController(agent: agent, save: save))
        if let sheet = nav.sheetPresentationController {
            sheet.detents = [.large()]
            sheet.prefersGrabberVisible = true
        }
        return nav
    }

    init(agent: CloudStore.Agent, save: @escaping @MainActor (String, String?) async throws -> Void) {
        self.agent = agent
        self.save = save
        takesBaseUrl = apiKeyTakesBaseUrl(provider: agent.key)
        // Pastes without the "Allow Paste" prompt: the key arrives through
        // `paste(itemProviders:)` below.
        let paste = UIPasteControl.Configuration()
        paste.displayMode = .labelOnly
        paste.cornerStyle = .capsule
        paste.baseBackgroundColor = Palette.controlFill
        paste.baseForegroundColor = Palette.text
        pasteControl = UIPasteControl(configuration: paste)
        super.init(nibName: nil, bundle: nil)
    }

    required init?(coder: NSCoder) { fatalError() }

    /// Title, what the key is for, and the key's placeholder.
    private var copy: (title: String, body: String, placeholder: String) {
        agent.key == "openai-key"
            ? ("Add an OpenAI API key", "Cloud sessions use it for Codex instead of a ChatGPT sign-in.", "sk-proj-…")
            : ("Add an Anthropic API key", "Cloud sessions use it for Claude Code instead of a subscription sign-in.", "sk-ant-api03-…")
    }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Palette.background
        view.accessibilityIdentifier = "api-key-sheet"
        navigationItem.leftBarButtonItem = UIBarButtonItem(systemItem: .close, primaryAction: UIAction { [weak self] _ in
            self?.dismiss(animated: true)
        })
        pasteConfiguration = UIPasteConfiguration(forAccepting: NSString.self)
        pasteControl.target = self
        pasteControl.accessibilityIdentifier = "api-key-paste"

        let tile = CloudUI.tile(image: BrandMarks.image(for: agent.harness, side: 28), side: 56)
        let tileRow = UIStackView(arrangedSubviews: [tile, UIView()])
        let header = CloudUI.header(copy.title, copy.body)

        configure(keyField, placeholder: copy.placeholder, size: 16)
        keyField.isSecureTextEntry = true
        keyField.returnKeyType = .done
        keyField.accessibilityLabel = "API key"
        keyField.accessibilityIdentifier = "api-key-field"
        suffixLabel.font = Fonts.ui(.mono, 15)
        suffixLabel.textColor = Palette.secondary
        suffixLabel.setContentHuggingPriority(.required, for: .horizontal)
        suffixLabel.setContentCompressionResistancePriority(.required, for: .horizontal)
        pasteControl.setContentHuggingPriority(.required, for: .horizontal)
        let keySection = Self.section("API key", FieldCard(field: keyField, trailing: [suffixLabel, pasteControl]))

        var sections: [UIView] = [tileRow, header, keySection]
        if takesBaseUrl {
            let direct = EndpointChoice(
                tile: CloudUI.tile(image: BrandMarks.image(for: agent.harness, side: 20)),
                title: "Anthropic API", detail: "api.anthropic.com"
            )
            let gateway = EndpointChoice(
                tile: CloudUI.tile(image: CloudUI.symbol("globe", 18)),
                title: "LLM gateway", detail: Self.gatewayDetail
            )
            direct.isSelected = true
            direct.accessibilityIdentifier = "api-key-endpoint-direct"
            gateway.accessibilityIdentifier = "api-key-endpoint-gateway"
            direct.addAction(UIAction { [weak self] _ in self?.choose(.direct) }, for: .touchUpInside)
            gateway.addAction(UIAction { [weak self] _ in self?.choose(.gateway) }, for: .touchUpInside)
            directChoice = direct
            gatewayChoice = gateway
            let card = CloudCard(rows: [direct, gateway])
            card.clipsToBounds = true
            card.accessibilityContainerType = .semanticGroup
            card.accessibilityLabel = "Endpoint"

            configure(urlField, placeholder: "https://gateway.example.com", size: 15)
            urlField.keyboardType = .URL
            urlField.textContentType = .URL
            urlField.clearButtonMode = .whileEditing
            urlField.returnKeyType = .done
            urlField.accessibilityLabel = "Base URL"
            urlField.accessibilityIdentifier = "api-key-base-url"
            urlField.addAction(UIAction { [weak self] _ in self?.revealGateway() }, for: .editingDidBegin)
            routeLine.accessibilityIdentifier = "api-key-route"
            gatewayBlock.addArrangedSubview(FieldCard(field: urlField))
            gatewayBlock.addArrangedSubview(routeLine)
            gatewayBlock.axis = .vertical
            gatewayBlock.spacing = 10
            gatewayBlock.isHidden = true
            gatewayBlock.alpha = 0
            let endpointSection = Self.section("Endpoint", card)
            sections += [endpointSection, gatewayBlock]
            content.setCustomSpacing(12, after: endpointSection)
        }
        let vaultNote = StatusLine()
        vaultNote.show("lock.fill", "Encrypted in your vault — only your Cloud machines can use it.", color: Palette.secondary)
        sections.append(vaultNote)

        sections.forEach(content.addArrangedSubview)
        content.axis = .vertical
        content.spacing = 24
        content.setCustomSpacing(16, after: tileRow)
        content.setCustomSpacing(28, after: header)

        // Why the last save failed, then Save: above the keyboard while it is
        // up, where the eye is after the tap.
        failureLine.isHidden = true
        failureLine.accessibilityIdentifier = "api-key-failure"
        saveButton = CloudUI.primaryButton("Save Key") { [weak self] in self?.submit() }
        saveButton.accessibilityIdentifier = "api-key-save"
        let bottom = UIStackView(arrangedSubviews: [failureLine, saveButton])
        bottom.axis = .vertical
        bottom.spacing = 12

        scroll.alwaysBounceVertical = true
        scroll.keyboardDismissMode = .interactive
        for v in [scroll, content, bottom] { v.translatesAutoresizingMaskIntoConstraints = false }
        scroll.addSubview(content)
        view.addSubview(scroll)
        view.addSubview(bottom)
        setContentScrollView(scroll, for: .top)
        NSLayoutConstraint.activate([
            scroll.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor),
            scroll.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            scroll.bottomAnchor.constraint(equalTo: bottom.topAnchor, constant: -8),
            content.topAnchor.constraint(equalTo: scroll.contentLayoutGuide.topAnchor, constant: 4),
            content.bottomAnchor.constraint(equalTo: scroll.contentLayoutGuide.bottomAnchor, constant: -16),
            content.leadingAnchor.constraint(equalTo: scroll.contentLayoutGuide.leadingAnchor, constant: 24),
            content.trailingAnchor.constraint(equalTo: scroll.contentLayoutGuide.trailingAnchor, constant: -24),
            content.widthAnchor.constraint(equalTo: scroll.frameLayoutGuide.widthAnchor, constant: -48),
            bottom.leadingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.leadingAnchor, constant: 24),
            bottom.trailingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.trailingAnchor, constant: -24),
            bottom.bottomAnchor.constraint(equalTo: view.keyboardLayoutGuide.topAnchor, constant: -12),
        ])
        refresh()
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        if !saving, keyField.text?.isEmpty ?? true { keyField.becomeFirstResponder() }
    }

    private func configure(_ field: UITextField, placeholder: String, size: CGFloat) {
        let font = Fonts.ui(.mono, size)
        field.font = font
        field.textColor = Palette.text
        field.tintColor = Palette.accent
        field.attributedPlaceholder = NSAttributedString(string: placeholder, attributes: [.font: font, .foregroundColor: Palette.tertiary])
        field.autocorrectionType = .no
        field.autocapitalizationType = .none
        field.spellCheckingType = .no
        field.smartQuotesType = .no
        field.smartDashesType = .no
        field.smartInsertDeleteType = .no
        field.delegate = self
        field.addAction(UIAction { [weak self] _ in self?.edited() }, for: .editingChanged)
    }

    /// A field's muted label over it, set in a little like the cards' text.
    private static func section(_ title: String, _ body: UIView) -> UIStackView {
        let label = PaddedLabel(insets: UIEdgeInsets(top: 0, left: 4, bottom: 0, right: 4))
        label.text = title
        label.font = Fonts.ui(.sansMedium, 14)
        label.textColor = Palette.secondary
        label.accessibilityTraits = .header
        let stack = UIStackView(arrangedSubviews: [label, body])
        stack.axis = .vertical
        stack.spacing = 8
        return stack
    }

    // MARK: State

    /// What Save sends — the key and, for a gateway, its base URL as the
    /// vault will keep it — once the core would take both. A key is long
    /// enough once it has the characters it will be listed by.
    private var submission: (key: String, baseUrl: String?)? {
        let key = (keyField.text ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        guard apiKeySuffix(key: key) != nil else { return nil }
        guard endpoint == .gateway else { return (key, nil) }
        guard case let .ready(url, _, _) = checkApiBaseUrl(provider: agent.key, raw: urlField.text ?? "") else { return nil }
        return (key, url)
    }

    /// Re-reads the fields: the key's echo, the base URL's verdict and the
    /// gateway it names, Save.
    private func refresh() {
        let key = keyField.text ?? ""
        let suffix = apiKeySuffix(key: key)
        suffixLabel.text = suffix
        suffixLabel.isHidden = suffix == nil
        pasteControl.isHidden = !key.isEmpty || saving
        if takesBaseUrl {
            switch checkApiBaseUrl(provider: agent.key, raw: urlField.text ?? "") {
            case let .ready(url, host, path):
                gatewayChoice?.detail = host
                routeLine.show("arrow.right", Self.route(url, path), tint: Palette.secondary)
                routeLine.accessibilityLabel = "Claude Code sends every request to \(url)\(path)"
            case let .refused(reason):
                gatewayChoice?.detail = Self.gatewayDetail
                routeLine.show("exclamationmark.triangle.fill", reason, color: Palette.danger)
            case .empty, .typing:
                gatewayChoice?.detail = Self.gatewayDetail
                routeLine.show("info.circle", "Claude Code adds /v1/messages to this URL.", color: Palette.secondary)
            }
        }
        var c = saveButton.configuration
        c?.title = saving ? "Saving…" : "Save Key"
        c?.showsActivityIndicator = saving
        c?.imagePadding = 8
        saveButton.configuration = c
        // In full ink while it saves; dimmed until there is something to save.
        saveButton.isEnabled = saving || submission != nil
        saveButton.isUserInteractionEnabled = !saving
    }

    /// "https://llm-gateway.example.com/v1/messages": the base URL as typed in
    /// the text color, the path Claude Code adds muted. The whole URL shows;
    /// a long one wraps before a slash, never inside a word.
    private static func route(_ url: String, _ path: String) -> NSAttributedString {
        let breakable = { (s: String) in s.replacingOccurrences(of: "/", with: "\u{200B}/") }
        let scheme = url.prefix(8)
        let mono = Fonts.ui(.mono, 12)
        let line = NSMutableAttributedString(
            string: String(scheme) + breakable(String(url.dropFirst(8))),
            attributes: [.font: mono, .foregroundColor: Palette.text]
        )
        line.append(NSAttributedString(string: breakable(path), attributes: [.font: mono, .foregroundColor: Palette.secondary]))
        return line
    }

    private func edited() {
        failureLine.isHidden = true
        refresh()
    }

    private func choose(_ choice: Endpoint) {
        guard choice != endpoint, !saving else { return }
        endpoint = choice
        UISelectionFeedbackGenerator().selectionChanged()
        directChoice?.isSelected = choice == .direct
        gatewayChoice?.isSelected = choice == .gateway
        failureLine.isHidden = true
        keyField.returnKeyType = choice == .gateway ? .next : .done
        if keyField.isFirstResponder { keyField.reloadInputViews() }
        let open = choice == .gateway
        UIView.animate(withDuration: 0.4, delay: 0, usingSpringWithDamping: 0.9, initialSpringVelocity: 0) {
            self.gatewayBlock.isHidden = !open
            self.gatewayBlock.alpha = open ? 1 : 0
            self.view.layoutIfNeeded()
        }
        if open {
            urlField.becomeFirstResponder()
        } else if urlField.isFirstResponder {
            keyField.becomeFirstResponder()
        }
        refresh()
    }

    /// Keeps the base URL and its route in sight above the keyboard — and
    /// clear of the scroll's fading bottom edge.
    private func revealGateway() {
        DispatchQueue.main.async { [weak self] in
            guard let self, !self.gatewayBlock.isHidden else { return }
            self.view.layoutIfNeeded()
            let block = self.gatewayBlock.convert(self.gatewayBlock.bounds, to: self.scroll)
            let margin = EdgeFade.width + 8
            self.scroll.scrollRectToVisible(block.insetBy(dx: 0, dy: -margin), animated: true)
        }
    }

    private func submit() {
        guard !saving, let submission else { return }
        saving = true
        failureLine.isHidden = true
        view.endEditing(true)
        setEditable(false)
        refresh()
        Task { @MainActor in
            do {
                try await self.save(submission.key, submission.baseUrl)
                UINotificationFeedbackGenerator().notificationOccurred(.success)
                self.dismiss(animated: true)
            } catch {
                self.saving = false
                self.setEditable(true)
                UIView.animate(withDuration: 0.25) {
                    self.failureLine.show("exclamationmark.triangle.fill", CloudStore.message(error), color: Palette.danger)
                    self.view.layoutIfNeeded()
                }
                UINotificationFeedbackGenerator().notificationOccurred(.error)
                self.refresh()
            }
        }
    }

    /// Everything but Save holds still while the key is being stored, and
    /// the sheet can't be swiped away from under it.
    private func setEditable(_ on: Bool) {
        keyField.isEnabled = on
        urlField.isEnabled = on
        directChoice?.isEnabled = on
        gatewayChoice?.isEnabled = on
        navigationItem.leftBarButtonItem?.isEnabled = on
        let presented: UIViewController = navigationController ?? self
        presented.isModalInPresentation = !on
    }

    // MARK: Keyboard and paste

    func textFieldShouldReturn(_ textField: UITextField) -> Bool {
        if textField === keyField, endpoint == .gateway {
            urlField.becomeFirstResponder()
        } else {
            submit()
        }
        return false
    }

    override func paste(itemProviders: [NSItemProvider]) {
        guard let item = itemProviders.first(where: { $0.canLoadObject(ofClass: NSString.self) }) else { return }
        _ = item.loadObject(ofClass: NSString.self) { [weak self] value, _ in
            let text = (value as? NSString).map { ($0 as String).trimmingCharacters(in: .whitespacesAndNewlines) }
            Task { @MainActor in self?.pasted(text) }
        }
    }

    private func pasted(_ text: String?) {
        guard let text, !text.isEmpty, !saving else { return }
        keyField.text = text
        edited()
        // On to where its requests go, if that is still to say.
        if endpoint == .gateway, urlField.text?.isEmpty ?? true { urlField.becomeFirstResponder() }
    }
}

/// `FadingScrollView`'s vertical sibling: the sheet's edges fade while
/// there's more of it that way — none at rest at the top, none once scrolled
/// to the end — so nothing is cut under the bar or above Save.
private final class FadingEdgesScrollView: UIScrollView {
    private let fade = CAGradientLayer()

    override func layoutSubviews() {
        super.layoutSubviews()
        let inset = adjustedContentInset
        let maxY = contentSize.height + inset.top + inset.bottom - bounds.height
        guard maxY > 0.5 else {
            if layer.mask != nil { layer.mask = nil }
            return
        }
        let y = contentOffset.y + inset.top
        // Ramped in over the first points of travel so it never pops.
        let height = max(bounds.height, 1)
        let top = EdgeFade.width * min(1, max(0, y / EdgeFade.width))
        let bottom = EdgeFade.width * min(1, max(0, (maxY - y) / EdgeFade.width))
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        fade.startPoint = CGPoint(x: 0.5, y: 0)
        fade.endPoint = CGPoint(x: 0.5, y: 1)
        fade.colors = [UIColor.clear.cgColor, UIColor.black.cgColor, UIColor.black.cgColor, UIColor.clear.cgColor]
        fade.locations = [0, NSNumber(value: Double(min(top / height, 0.5))), NSNumber(value: Double(max(1 - bottom / height, 0.5))), 1]
        fade.frame = bounds
        if layer.mask !== fade { layer.mask = fade }
        CATransaction.commit()
    }
}

/// A field on the key sheet: the app's elevated card, ringed in the accent
/// while it has the keyboard; `trailing` sits inside it after the text.
private final class FieldCard: UIView {
    private var hasKeyboard = false

    init(field: UITextField, trailing: [UIView] = []) {
        super.init(frame: .zero)
        backgroundColor = Palette.elevated
        layer.cornerRadius = 16
        layer.cornerCurve = .continuous
        layer.borderWidth = 1.5
        let row = UIStackView(arrangedSubviews: [field] + trailing)
        row.alignment = .center
        row.spacing = 10
        row.translatesAutoresizingMaskIntoConstraints = false
        field.setContentHuggingPriority(UILayoutPriority(1), for: .horizontal)
        field.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        addSubview(row)
        NSLayoutConstraint.activate([
            heightAnchor.constraint(equalToConstant: 56),
            row.topAnchor.constraint(equalTo: topAnchor),
            row.bottomAnchor.constraint(equalTo: bottomAnchor),
            row.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 16),
            row.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -12),
            // The whole card takes the tap, not just the line of text.
            field.heightAnchor.constraint(equalTo: heightAnchor),
        ])
        field.addAction(UIAction { [weak self] _ in self?.setFocused(true) }, for: .editingDidBegin)
        field.addAction(UIAction { [weak self] _ in self?.setFocused(false) }, for: .editingDidEnd)
        registerForTraitChanges([UITraitUserInterfaceStyle.self]) { (self: FieldCard, _) in self.paintRing() }
        paintRing()
    }

    required init?(coder: NSCoder) { fatalError() }

    private func setFocused(_ on: Bool) {
        hasKeyboard = on
        paintRing()
    }

    private func paintRing() {
        let ring = hasKeyboard ? Palette.accent.resolvedColor(with: traitCollection).withAlphaComponent(0.6) : .clear
        layer.borderColor = ring.cgColor
    }
}

/// One endpoint on the key sheet: a Cloud row (its glyph, what it is over
/// where it sends the key) with a radio mark on the trailing edge.
private final class EndpointChoice: UIControl {
    private let row: CloudRow
    private let mark = UIImageView()

    init(tile: UIView, title: String, detail: String) {
        row = CloudRow(tile: tile, title: title, detail: detail)
        super.init(frame: .zero)
        row.isUserInteractionEnabled = false
        row.detailLabel.numberOfLines = 1
        row.detailLabel.lineBreakMode = .byTruncatingMiddle
        row.translatesAutoresizingMaskIntoConstraints = false
        addSubview(row)
        mark.contentMode = .center
        NSLayoutConstraint.activate([
            row.topAnchor.constraint(equalTo: topAnchor),
            row.bottomAnchor.constraint(equalTo: bottomAnchor),
            row.leadingAnchor.constraint(equalTo: leadingAnchor),
            row.trailingAnchor.constraint(equalTo: trailingAnchor),
            mark.widthAnchor.constraint(equalToConstant: 24),
            mark.heightAnchor.constraint(equalToConstant: 24),
        ])
        row.setAccessory(mark)
        isAccessibilityElement = true
        accessibilityLabel = title
        accessibilityValue = detail
        paint()
    }

    required init?(coder: NSCoder) { fatalError() }

    var detail: String? {
        get { row.detailLabel.text }
        set {
            row.detailLabel.text = newValue
            accessibilityValue = newValue
        }
    }

    override var isSelected: Bool { didSet { paint() } }
    override var isHighlighted: Bool { didSet { backgroundColor = isHighlighted ? Palette.controlFill : .clear } }
    override var isEnabled: Bool { didSet { alpha = isEnabled ? 1 : 0.5 } }

    private func paint() {
        mark.image = CloudUI.symbol(isSelected ? "checkmark.circle.fill" : "circle", 21, weight: isSelected ? .semibold : .regular)
        mark.tintColor = isSelected ? Palette.accent : Palette.tertiary
        row.titleLabel.textColor = isSelected ? Palette.text : Palette.secondary
        accessibilityTraits = isSelected ? [.button, .selected] : .button
    }
}

/// A line under a field: a glyph, then a few words — a note, a refusal, or
/// the URL the key's requests will go to.
private final class StatusLine: UIStackView {
    private let icon = UIImageView()
    private let label = UILabel()

    init() {
        super.init(frame: .zero)
        alignment = .top
        spacing = 7
        isLayoutMarginsRelativeArrangement = true
        directionalLayoutMargins = NSDirectionalEdgeInsets(top: 0, leading: 4, bottom: 0, trailing: 4)
        isAccessibilityElement = true
        icon.contentMode = .center
        label.numberOfLines = 0
        addArrangedSubview(icon)
        addArrangedSubview(label)
        NSLayoutConstraint.activate([
            icon.widthAnchor.constraint(equalToConstant: 14),
            icon.heightAnchor.constraint(equalToConstant: 17),
        ])
    }

    required init(coder: NSCoder) { fatalError() }

    func show(_ symbol: String, _ text: String, color: UIColor) {
        show(symbol, NSAttributedString(string: text, attributes: [.font: Fonts.ui(.sans, 13), .foregroundColor: color]), tint: color)
        accessibilityLabel = text
    }

    func show(_ symbol: String, _ text: NSAttributedString, tint: UIColor) {
        icon.image = CloudUI.symbol(symbol, 11, weight: .semibold)
        icon.tintColor = tint
        label.attributedText = text
        accessibilityLabel = text.string
        isHidden = false
    }
}
