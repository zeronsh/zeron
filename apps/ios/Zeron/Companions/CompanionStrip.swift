import UIKit

/// Glass chips above the composer for what's working alongside the session
/// — "2 working" while subagents run, side chats, live terminals — each a
/// tap from its section of the companions panel. Nothing shows when there's
/// nothing alongside.
final class CompanionStrip: UIView {
    struct State: Equatable {
        var agents = 0
        var runningAgents = 0
        var sideChats = 0
        var terminals = 0
        var liveTerminals = 0

        var isEmpty: Bool { agents == 0 && sideChats == 0 && terminals == 0 }
    }

    var onTap: ((CompanionsViewController.Section) -> Void)?
    private(set) var state = State()
    private let stack = UIStackView()
    private let agents = CompanionChip()
    private let sides = CompanionChip()
    private let terminals = CompanionChip()

    override init(frame: CGRect) {
        super.init(frame: frame)
        stack.axis = .horizontal
        stack.spacing = 6
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: topAnchor),
            stack.bottomAnchor.constraint(equalTo: bottomAnchor),
            stack.leadingAnchor.constraint(equalTo: leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: trailingAnchor),
        ])
        for (chip, section, id) in [(agents, CompanionsViewController.Section.agents, "chip-agents"), (sides, .sideChats, "chip-side-chats"), (terminals, .terminals, "chip-terminals")] {
            chip.addAction(UIAction { [weak self] _ in
                UISelectionFeedbackGenerator().selectionChanged()
                self?.onTap?(section)
            }, for: .touchUpInside)
            chip.accessibilityIdentifier = id
            chip.isHidden = true
            stack.addArrangedSubview(chip)
        }
        setContentHuggingPriority(.required, for: .horizontal)
    }

    required init?(coder: NSCoder) { fatalError() }

    func apply(_ s: State) {
        guard s != state else { return }
        state = s
        agents.isHidden = s.agents == 0
        if s.runningAgents > 0 {
            agents.configure(glyph: .spinner, symbol: nil, text: "\(s.runningAgents) working", label: "\(s.runningAgents) subagents working")
        } else {
            agents.configure(glyph: nil, symbol: "tool-bot", text: "\(s.agents)", label: "\(s.agents) subagents")
        }
        sides.isHidden = s.sideChats == 0
        sides.configure(glyph: nil, symbol: "bubble.left.and.bubble.right", text: "\(s.sideChats)", label: "\(s.sideChats) side chats")
        terminals.isHidden = s.terminals == 0
        terminals.configure(
            glyph: s.liveTerminals > 0 ? .dot(StatusTone.done) : nil,
            symbol: "apple.terminal",
            text: "\(s.terminals)",
            label: "\(s.terminals) terminals"
        )
    }
}

/// One chip: a glass capsule, an icon (or live glyph) and a short count.
final class CompanionChip: UIControl {
    private let glass = Glass.surface(interactive: true)
    private let glyph = StatusGlyph()
    private let icon = UIImageView()
    private let label = UILabel()
    private let dot = StatusGlyph()

    override init(frame: CGRect) {
        super.init(frame: frame)
        glass.isUserInteractionEnabled = false
        glass.translatesAutoresizingMaskIntoConstraints = false
        addSubview(glass)
        icon.contentMode = .center
        icon.tintColor = Palette.secondary
        label.font = Fonts.ui(.sansMedium, 13)
        label.textColor = Palette.secondary
        let row = UIStackView(arrangedSubviews: [glyph, icon, label, dot])
        row.spacing = 6
        row.alignment = .center
        row.translatesAutoresizingMaskIntoConstraints = false
        glass.contentView.addSubview(row)
        NSLayoutConstraint.activate([
            glass.topAnchor.constraint(equalTo: topAnchor),
            glass.bottomAnchor.constraint(equalTo: bottomAnchor),
            glass.leadingAnchor.constraint(equalTo: leadingAnchor),
            glass.trailingAnchor.constraint(equalTo: trailingAnchor),
            heightAnchor.constraint(equalToConstant: 30),
            glyph.widthAnchor.constraint(equalToConstant: 12),
            glyph.heightAnchor.constraint(equalToConstant: 12),
            icon.widthAnchor.constraint(equalToConstant: 15),
            dot.widthAnchor.constraint(equalToConstant: 7),
            dot.heightAnchor.constraint(equalToConstant: 7),
            row.leadingAnchor.constraint(equalTo: glass.contentView.leadingAnchor, constant: 11),
            row.trailingAnchor.constraint(equalTo: glass.contentView.trailingAnchor, constant: -11),
            row.centerYAnchor.constraint(equalTo: glass.contentView.centerYAnchor),
        ])
        isAccessibilityElement = true
        accessibilityTraits = .button
    }

    required init?(coder: NSCoder) { fatalError() }

    /// `symbol`: a `tool-*` asset or an SF symbol.
    func configure(glyph kind: StatusGlyph.Kind?, symbol: String?, text: String, label a11y: String) {
        let spinning = kind == .spinner
        glyph.isHidden = !spinning
        if spinning { glyph.kind = .spinner }
        icon.isHidden = spinning || symbol == nil
        if let symbol {
            icon.image = Self.asset(symbol)
                ?? UIImage(systemName: symbol, withConfiguration: UIImage.SymbolConfiguration(pointSize: 12, weight: .semibold))
        }
        if case let .dot(color)? = kind {
            dot.isHidden = false
            dot.kind = .dot(color)
        } else {
            dot.isHidden = true
        }
        label.text = text
        label.textColor = spinning ? Palette.text : Palette.secondary
        accessibilityLabel = a11y
    }

    override var isHighlighted: Bool {
        didSet { alpha = isHighlighted ? 0.7 : 1 }
    }

    /// A `tool-*` asset at the icon's size. The SVGs declare `1em` and load
    /// at about a point square, which a centered image view drew as a speck.
    private static var assets: [String: UIImage] = [:]
    private static func asset(_ name: String) -> UIImage? {
        if let cached = assets[name] { return cached }
        guard let image = UIImage(named: name) else { return nil }
        let side: CGFloat = 15
        let sized = UIGraphicsImageRenderer(size: CGSize(width: side, height: side)).image { _ in
            image.draw(in: CGRect(x: 0, y: 0, width: side, height: side))
        }.withRenderingMode(.alwaysTemplate)
        assets[name] = sized
        return sized
    }
}
