import UIKit

/// What a delegated subagent asks the person, read from its own handle's
/// open input while it asks.
struct DelegatedAsk: Equatable {
    let requestId: String
    let questionId: String
    let isApproval: Bool
    /// An approval's command (the question's backticked part).
    let code: String?
    /// The question's words (an approval's only when it has no command).
    let words: String?
    /// Choices offered as buttons (a single-select question with a few).
    let options: [String]

    init?(_ input: InputRequest?) {
        guard let input, let q = input.questions.first else { return nil }
        requestId = input.requestId
        questionId = q.id
        isApproval = input.isApproval
        let text = q.question.isEmpty ? q.header : q.question
        if input.isApproval {
            code = Self.backticked(text)
            words = code == nil ? text : nil
            options = []
        } else {
            code = nil
            words = text
            let few = input.questions.count == 1 && !q.multiSelect && !q.options.isEmpty && q.options.count <= 5
            options = few ? q.options : []
        }
    }

    /// The first `…` span, if there is one.
    static func backticked(_ text: String) -> String? {
        guard let open = text.firstIndex(of: "`") else { return nil }
        let rest = text[text.index(after: open)...]
        guard let close = rest.firstIndex(of: "`") else { return nil }
        let code = rest[..<close].trimmingCharacters(in: .whitespacesAndNewlines)
        return code.isEmpty ? nil : code
    }
}

/// A chat the session's agent delegated to, as a companion row: how it
/// stands, its harness mark, its title, "Codex · Build box", and how long
/// it's been going (with a Stop) — and, while it asks, the ask under it with
/// its answers, so it's answered without opening anything.
final class DelegatedAgentCell: UICollectionViewListCell {
    enum Answer {
        case allow, deny, option(String), openChat
    }

    var onAnswer: ((Answer) -> Void)?
    var onStop: (() -> Void)?

    private let tile = UIView()
    private let glyph = StatusGlyph()
    private let icon = UIImageView()
    private let mark = UIImageView()
    private let title = FadingLabel()
    private let detail = FadingLabel()
    private let trailing = UILabel()
    private let stop = UIButton(type: .system)
    private let chevron = UIImageView(image: UIImage(systemName: "chevron.right", withConfiguration: UIImage.SymbolConfiguration(pointSize: 12, weight: .semibold)))
    private let card = DelegatedAskView()
    private var hasAsk = false

    override init(frame: CGRect) {
        super.init(frame: frame)
        tile.layer.cornerRadius = 8
        tile.layer.cornerCurve = .continuous
        icon.contentMode = .center
        mark.contentMode = .scaleAspectFit
        mark.tintColor = Palette.text
        chevron.tintColor = Palette.tertiary
        chevron.contentMode = .center
        trailing.textAlignment = .right
        stop.setImage(UIImage(systemName: "stop.fill", withConfiguration: UIImage.SymbolConfiguration(pointSize: 10, weight: .bold)), for: .normal)
        stop.tintColor = Palette.secondary
        stop.backgroundColor = Palette.controlFill
        stop.layer.cornerRadius = 14
        stop.accessibilityIdentifier = "delegated-stop"
        stop.addAction(UIAction { [weak self] _ in
            UIImpactFeedbackGenerator(style: .light).impactOccurred()
            self?.onStop?()
        }, for: .touchUpInside)
        card.onAnswer = { [weak self] in self?.onAnswer?($0) }
        for v in [tile, mark, title, detail, trailing, stop, chevron, card] as [UIView] { contentView.addSubview(v) }
        tile.addSubview(icon)
        tile.addSubview(glyph)
        var bg = UIBackgroundConfiguration.listCell()
        bg.backgroundColor = .clear
        backgroundConfiguration = bg
    }

    required init?(coder: NSCoder) { fatalError() }

    override func updateConfiguration(using state: UICellConfigurationState) {
        var bg = UIBackgroundConfiguration.listCell().updated(for: state)
        bg.backgroundColor = state.isHighlighted || state.isSelected ? Palette.controlFill : .clear
        bg.cornerRadius = 16
        bg.backgroundInsets = NSDirectionalEdgeInsets(top: 1, leading: 8, bottom: 1, trailing: 8)
        backgroundConfiguration = bg
    }

    private static var rowHeight: CGFloat { CompanionCell.height }
    private static let textX: CGFloat = 20 + 30 + 12

    override func preferredLayoutAttributesFitting(_ attrs: UICollectionViewLayoutAttributes) -> UICollectionViewLayoutAttributes {
        var h = Self.rowHeight
        if hasAsk {
            h += card.height(width: attrs.size.width - Self.textX - 20) + 12
        }
        attrs.size.height = h
        return attrs
    }

    func configure(_ row: SubagentRow, ask: DelegatedAsk?, held: Bool) {
        let running = row.state == .running
        let asking = row.asks && running
        title.font = Fonts.ui(.sansMedium, TypeScale.size(16))
        title.textColor = Palette.text
        title.text = row.title
        detail.font = Fonts.ui(.sans, TypeScale.size(13.5))
        trailing.font = Fonts.ui(.sansMedium, TypeScale.size(13))
        mark.image = BrandMarks.image(for: row.harness, side: 15)
        mark.isHidden = mark.image == nil

        // The tile: working, asks (approval or question), done, failed.
        let approval = ask?.isApproval ?? true
        let symbol: String? = switch (row.state, asking) {
        case (.running, true): approval ? "checkmark.shield" : "questionmark.bubble"
        case (.running, false): nil
        case (.failed, _): "xmark"
        case (.done, _): "checkmark"
        }
        icon.image = symbol.map { UIImage(systemName: $0, withConfiguration: UIImage.SymbolConfiguration(pointSize: 12, weight: .bold)) } ?? nil
        icon.isHidden = symbol == nil
        glyph.isHidden = symbol != nil
        glyph.kind = .spinner
        let askTone = approval ? Palette.warning : Palette.accent
        icon.tintColor = asking ? askTone : row.state == .failed ? Palette.danger : StatusTone.done
        tile.backgroundColor = asking ? askTone.withAlphaComponent(0.14)
            : running ? Palette.accentSoft
            : row.state == .failed ? Palette.color(.dangerWash) : Palette.chip

        // "Needs approval · Codex · Build box"
        let source = [row.kind, row.deviceName].compactMap { $0 }.filter { !$0.isEmpty }.joined(separator: " · ")
        let lead: (String, UIColor)? = asking ? (approval ? "Needs approval" : "Has a question", askTone)
            : row.state == .failed ? ("Failed", Palette.danger) : nil
        let text = NSMutableAttributedString()
        if let lead {
            text.append(NSAttributedString(string: lead.0, attributes: [.font: detail.font!, .foregroundColor: lead.1]))
            if !source.isEmpty { text.append(NSAttributedString(string: " · ", attributes: [.font: detail.font!, .foregroundColor: Palette.tertiary])) }
        }
        text.append(NSAttributedString(string: source.isEmpty && lead == nil ? (running ? "Working" : "Finished") : source, attributes: [.font: detail.font!, .foregroundColor: Palette.secondary]))
        detail.attributedText = text

        if running {
            trailing.text = SubagentFormat.elapsed(since: Date(timeIntervalSince1970: Double(row.startedAtMs) / 1000))
            trailing.textColor = asking ? Palette.tertiary : StatusTone.working
        } else {
            trailing.text = nil
        }
        stop.isHidden = !running
        stop.accessibilityLabel = "Stop \(row.title)"
        chevron.isHidden = running

        hasAsk = asking && ask != nil
        card.isHidden = !hasAsk
        if let ask, hasAsk { card.configure(ask, held: held) }

        accessibilityIdentifier = "companion-delegated-\(row.chatId ?? row.id)"
        let word = asking ? (approval ? "needs approval" : "has a question") : running ? "working" : row.state == .failed ? "failed" : "done"
        accessibilityLabel = [row.title, word, source].filter { !$0.isEmpty }.joined(separator: ", ")
        setNeedsLayout()
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        let b = contentView.bounds
        let rowH = Self.rowHeight
        let left: CGFloat = 20
        let side: CGFloat = 30
        tile.frame = CGRect(x: left, y: (rowH - side) / 2, width: side, height: side)
        icon.frame = tile.bounds
        glyph.frame = CGRect(x: (side - 12) / 2, y: (side - 12) / 2, width: 12, height: 12)
        let textX = Self.textX
        var right = b.width - 20
        if !stop.isHidden {
            stop.frame = CGRect(x: right - 28, y: (rowH - 28) / 2, width: 28, height: 28)
            right = stop.frame.minX - 10
        } else if !chevron.isHidden {
            chevron.frame = CGRect(x: right - 12, y: (rowH - 16) / 2, width: 12, height: 16)
            right = chevron.frame.minX - 6
        }
        let th = ceil(title.font.lineHeight)
        let dh = ceil(detail.font?.lineHeight ?? 16)
        let top = (rowH - th - dh - 3) / 2
        let tw = ceil(trailing.intrinsicContentSize.width)
        if tw > 0 {
            trailing.frame = CGRect(x: right - tw, y: top, width: tw, height: th)
            right = trailing.frame.minX - 10
        } else {
            trailing.frame = .zero
        }
        var titleX = textX
        if !mark.isHidden {
            let m: CGFloat = 15
            mark.frame = CGRect(x: textX, y: top + (th - m) / 2, width: m, height: m)
            titleX = mark.frame.maxX + 6
        }
        title.frame = CGRect(x: titleX, y: top, width: max(0, right - titleX), height: th)
        detail.frame = CGRect(x: textX, y: top + th + 3, width: max(0, b.width - 20 - textX - (stop.isHidden ? 18 : 38)), height: dh)
        if hasAsk {
            let w = b.width - textX - 20
            card.frame = CGRect(x: textX, y: rowH - 4, width: w, height: card.height(width: w))
        }
    }
}

/// The ask under a delegated row: an approval's command on a code wash with
/// Deny and Allow once at its end, or a question's words with its options
/// and a way into its chat.
final class DelegatedAskView: UIView {
    var onAnswer: ((DelegatedAgentCell.Answer) -> Void)?

    private let codeWash = UIView()
    private let code = UILabel()
    private let words = UILabel()
    private var buttons: [UIButton] = []
    private var ask: DelegatedAsk?
    /// Buttons right-aligned on one line (approval) or flowing (options).
    private var trailingButtons = true

    override init(frame: CGRect) {
        super.init(frame: frame)
        codeWash.backgroundColor = Palette.codeBackground
        codeWash.layer.borderColor = Palette.codeBorder.cgColor
        codeWash.layer.borderWidth = 1
        codeWash.layer.cornerRadius = 10
        codeWash.layer.cornerCurve = .continuous
        code.numberOfLines = 0
        code.lineBreakMode = .byCharWrapping
        code.textColor = Palette.color(.codeText)
        words.numberOfLines = 0
        words.textColor = Palette.text
        addSubview(codeWash)
        codeWash.addSubview(code)
        addSubview(words)
        registerForTraitChanges([UITraitUserInterfaceStyle.self]) { (v: DelegatedAskView, _) in
            v.codeWash.layer.borderColor = Palette.codeBorder.cgColor
        }
    }

    required init?(coder: NSCoder) { fatalError() }

    func configure(_ ask: DelegatedAsk, held: Bool) {
        code.font = Fonts.ui(.mono, TypeScale.size(13))
        words.font = Fonts.ui(.sans, TypeScale.size(14.5))
        if ask != self.ask {
            self.ask = ask
            code.text = ask.code
            words.text = ask.words
            buttons.forEach { $0.removeFromSuperview() }
            if ask.isApproval {
                trailingButtons = true
                buttons = [
                    Self.button("Deny", style: .ghost, id: "delegated-deny") { [weak self] in self?.onAnswer?(.deny) },
                    Self.button("Allow once", style: .primary, id: "delegated-allow") { [weak self] in self?.onAnswer?(.allow) },
                ]
            } else {
                trailingButtons = false
                buttons = ask.options.map { option in
                    Self.button(option, style: .choice, id: "delegated-option") { [weak self] in self?.onAnswer?(.option(option)) }
                } + [Self.button("Answer in its chat", style: .ghost, id: "delegated-open") { [weak self] in self?.onAnswer?(.openChat) }]
            }
            buttons.forEach(addSubview)
        }
        // Pressed: nothing more until the ask clears (or a few seconds pass).
        for b in buttons where b.accessibilityIdentifier != "delegated-open" {
            b.isEnabled = !held
        }
        codeWash.isHidden = ask.code == nil
        words.isHidden = ask.words == nil
        words.textColor = Palette.text
        setNeedsLayout()
    }

    private enum Style { case primary, ghost, choice }

    private static func button(_ title: String, style: Style, id: String, action: @escaping () -> Void) -> UIButton {
        var c: UIButton.Configuration = switch style {
        case .primary: .filled()
        case .ghost: .plain()
        case .choice: .filled()
        }
        c.cornerStyle = .capsule
        c.contentInsets = NSDirectionalEdgeInsets(top: 6, leading: 12, bottom: 6, trailing: 12)
        let font = Fonts.ui(.sansMedium, TypeScale.size(13.5))
        c.attributedTitle = AttributedString(title, attributes: AttributeContainer([.font: font]))
        c.titleLineBreakMode = .byTruncatingTail
        switch style {
        case .primary:
            c.baseBackgroundColor = Palette.text
            c.baseForegroundColor = Palette.background
        case .ghost:
            c.baseForegroundColor = Palette.secondary
        case .choice:
            c.baseBackgroundColor = Palette.controlFill
            c.baseForegroundColor = Palette.text
        }
        let b = UIButton(configuration: c)
        b.accessibilityIdentifier = id
        b.addAction(UIAction { _ in
            UISelectionFeedbackGenerator().selectionChanged()
            action()
        }, for: .touchUpInside)
        return b
    }

    private static let gap: CGFloat = 8
    private static var buttonH: CGFloat { (30 * TypeScale.factor).rounded() }

    /// Lays out (or just measures) at `width`; returns the height.
    @discardableResult
    private func arrange(width: CGFloat, apply: Bool) -> CGFloat {
        guard let ask else { return 0 }
        var y: CGFloat = 0
        if ask.code != nil {
            let inset: CGFloat = 10
            let h = ceil(code.sizeThatFits(CGSize(width: width - inset * 2, height: .greatestFiniteMagnitude)).height)
            if apply {
                codeWash.frame = CGRect(x: 0, y: y, width: width, height: h + 16)
                code.frame = CGRect(x: inset, y: 8, width: width - inset * 2, height: h)
            }
            y += h + 16 + Self.gap
        }
        if ask.words != nil {
            let h = ceil(words.sizeThatFits(CGSize(width: width, height: .greatestFiniteMagnitude)).height)
            if apply { words.frame = CGRect(x: 0, y: y, width: width, height: h) }
            y += h + Self.gap
        }
        let bh = Self.buttonH
        let sizes = buttons.map { min(width, ceil($0.sizeThatFits(CGSize(width: width, height: bh)).width)) }
        if trailingButtons {
            var x = width
            for (b, w) in zip(buttons, sizes).reversed() {
                x -= w
                if apply { b.frame = CGRect(x: x, y: y, width: w, height: bh) }
                x -= 6
            }
            y += bh
        } else {
            var x: CGFloat = 0
            for (b, w) in zip(buttons, sizes) {
                if x > 0, x + w > width {
                    x = 0
                    y += bh + 6
                }
                if apply { b.frame = CGRect(x: x, y: y, width: w, height: bh) }
                x += w + 6
            }
            y += bh
        }
        return y
    }

    func height(width: CGFloat) -> CGFloat { arrange(width: width, apply: false) }

    override func layoutSubviews() {
        super.layoutSubviews()
        arrange(width: bounds.width, apply: true)
    }
}
