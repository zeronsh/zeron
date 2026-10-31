import UIKit

/// The keys a phone keyboard lacks, on glass above it: Esc, Tab, sticky Ctrl
/// and Alt (tap for the next key, double-tap to lock), arrows that repeat
/// while held, the shell's usual punctuation, paste, and putting the
/// keyboard away.
final class TerminalKeyBar: UIInputView {
    enum Action {
        case key(TerminalKey)
        case text(String)
        case paste
        case dismiss
    }

    var onAction: ((Action) -> Void)?

    enum Latch { case off, once, locked }
    private(set) var ctrl: Latch = .off { didSet { style(ctrlButton, ctrl) } }
    private(set) var alt: Latch = .off { didSet { style(altButton, alt) } }

    var modifiers: KeyModifiers { KeyModifiers(ctrl: ctrl != .off, alt: alt != .off, shift: false) }

    /// A key went out: one-shot modifiers let go.
    func consume() {
        if ctrl == .once { ctrl = .off }
        if alt == .once { alt = .off }
    }

    private let scroller = UIScrollView()
    private let stack = UIStackView()
    private var ctrlButton: UIButton!
    private var altButton: UIButton!
    private let fade = CAGradientLayer()
    private var repeatTimer: Timer?

    init() {
        super.init(frame: CGRect(x: 0, y: 0, width: 320, height: 52), inputViewStyle: .keyboard)
        allowsSelfSizing = true
        translatesAutoresizingMaskIntoConstraints = false
        backgroundColor = .clear

        scroller.showsHorizontalScrollIndicator = false
        scroller.alwaysBounceHorizontal = true
        scroller.contentInset = UIEdgeInsets(top: 0, left: 8, bottom: 0, right: 8)
        scroller.translatesAutoresizingMaskIntoConstraints = false
        addSubview(scroller)
        stack.axis = .horizontal
        stack.spacing = 6
        stack.alignment = .center
        stack.translatesAutoresizingMaskIntoConstraints = false
        scroller.addSubview(stack)

        let dismiss = key(symbol: "keyboard.chevron.compact.down", label: "Hide keyboard") { [weak self] in self?.onAction?(.dismiss) }
        dismiss.translatesAutoresizingMaskIntoConstraints = false
        addSubview(dismiss)

        ctrlButton = key(title: "ctrl", label: "Control") { [weak self] in self?.toggle(\.ctrl) }
        altButton = key(title: "alt", label: "Option") { [weak self] in self?.toggle(\.alt) }
        let keys: [UIView] = [
            key(title: "esc", label: "Escape") { [weak self] in self?.send(.key(.escape)) },
            ctrlButton,
            key(title: "tab", label: "Tab") { [weak self] in self?.send(.key(.tab)) },
            repeating(symbol: "arrow.up", label: "Up", .up),
            repeating(symbol: "arrow.down", label: "Down", .down),
            repeating(symbol: "arrow.left", label: "Left", .left),
            repeating(symbol: "arrow.right", label: "Right", .right),
            altButton,
            divider(),
            text("~"), text("/"), text("-"), text("|"), text("*"), text("$"), text("`"),
            divider(),
            key(symbol: "doc.on.clipboard", label: "Paste") { [weak self] in self?.onAction?(.paste) },
        ]
        keys.forEach(stack.addArrangedSubview)

        NSLayoutConstraint.activate([
            heightAnchor.constraint(equalToConstant: 52),
            scroller.leadingAnchor.constraint(equalTo: leadingAnchor),
            scroller.topAnchor.constraint(equalTo: topAnchor),
            scroller.bottomAnchor.constraint(equalTo: bottomAnchor),
            scroller.trailingAnchor.constraint(equalTo: dismiss.leadingAnchor, constant: -4),
            dismiss.trailingAnchor.constraint(equalTo: safeAreaLayoutGuide.trailingAnchor, constant: -8),
            dismiss.centerYAnchor.constraint(equalTo: centerYAnchor),
            stack.leadingAnchor.constraint(equalTo: scroller.contentLayoutGuide.leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: scroller.contentLayoutGuide.trailingAnchor),
            stack.centerYAnchor.constraint(equalTo: scroller.frameLayoutGuide.centerYAnchor),
            stack.heightAnchor.constraint(equalToConstant: 38),
        ])
        // Keys slide under a soft edge instead of being cut.
        fade.startPoint = CGPoint(x: 0, y: 0.5)
        fade.endPoint = CGPoint(x: 1, y: 0.5)
        fade.colors = [UIColor.clear.cgColor, UIColor.black.cgColor, UIColor.black.cgColor, UIColor.clear.cgColor]
        fade.locations = [0, 0.03, 0.95, 1]
        scroller.layer.mask = fade
        accessibilityIdentifier = "terminal-key-bar"
    }

    required init?(coder: NSCoder) { fatalError() }

    override func layoutSubviews() {
        super.layoutSubviews()
        fade.frame = scroller.bounds
    }

    private func send(_ action: Action) {
        UIDevice.current.playInputClick()
        onAction?(action)
    }

    /// Tap: for the next key. Tap again while armed: locked. Again: off.
    private func toggle(_ latch: ReferenceWritableKeyPath<TerminalKeyBar, Latch>) {
        UISelectionFeedbackGenerator().selectionChanged()
        self[keyPath: latch] = switch self[keyPath: latch] {
        case .off: .once
        case .once: .locked
        case .locked: .off
        }
    }

    private func style(_ button: UIButton, _ latch: Latch) {
        var c = button.configuration ?? .glass()
        switch latch {
        case .off:
            c = UIButton.Configuration.glass()
            c.baseForegroundColor = Palette.text
        case .once:
            c = UIButton.Configuration.prominentGlass()
            c.baseBackgroundColor = Palette.accent.withAlphaComponent(0.75)
            c.baseForegroundColor = .white
        case .locked:
            c = UIButton.Configuration.prominentGlass()
            c.baseBackgroundColor = Palette.accent
            c.baseForegroundColor = .white
        }
        c.title = button.configuration?.title
        c.titleTextAttributesTransformer = Self.titleFont
        c.contentInsets = NSDirectionalEdgeInsets(top: 0, leading: 12, bottom: 0, trailing: 12)
        button.configuration = c
        button.accessibilityValue = latch == .off ? nil : latch == .locked ? "Locked" : "On"
    }

    private static let titleFont = UIConfigurationTextAttributesTransformer { a in
        var a = a
        a.font = Fonts.ui(.monoMedium, 14)
        return a
    }

    private func key(title: String? = nil, symbol: String? = nil, label: String, action: @escaping () -> Void) -> UIButton {
        var c = UIButton.Configuration.glass()
        c.baseForegroundColor = Palette.text
        c.cornerStyle = .capsule
        c.contentInsets = NSDirectionalEdgeInsets(top: 0, leading: 12, bottom: 0, trailing: 12)
        if let title {
            c.title = title
            c.titleTextAttributesTransformer = Self.titleFont
        }
        if let symbol {
            c.image = UIImage(systemName: symbol, withConfiguration: UIImage.SymbolConfiguration(pointSize: 14, weight: .semibold))
        }
        let b = UIButton(configuration: c, primaryAction: UIAction { _ in action() })
        b.accessibilityLabel = label
        b.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            b.heightAnchor.constraint(equalToConstant: 38),
            b.widthAnchor.constraint(greaterThanOrEqualToConstant: 44),
        ])
        return b
    }

    private func text(_ t: String) -> UIButton {
        key(title: t, label: t) { [weak self] in self?.send(.text(t)) }
    }

    /// Arrows: one step on tap, then repeat while held.
    private func repeating(symbol: String, label: String, _ k: TerminalKey) -> UIButton {
        let b = key(symbol: symbol, label: label) {}
        b.addAction(UIAction { [weak self] _ in
            guard let self else { return }
            self.send(.key(k))
            self.repeatTimer?.invalidate()
            self.repeatTimer = Timer.scheduledTimer(withTimeInterval: 0.38, repeats: false) { [weak self] _ in
                self?.repeatTimer = Timer.scheduledTimer(withTimeInterval: 0.06, repeats: true) { [weak self] _ in
                    self?.onAction?(.key(k))
                }
            }
        }, for: .touchDown)
        for event: UIControl.Event in [.touchUpInside, .touchUpOutside, .touchCancel, .touchDragExit] {
            b.addAction(UIAction { [weak self] _ in self?.repeatTimer?.invalidate() }, for: event)
        }
        return b
    }

    private func divider() -> UIView {
        let v = UIView()
        v.backgroundColor = Palette.hairline
        v.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            v.widthAnchor.constraint(equalToConstant: 1),
            v.heightAnchor.constraint(equalToConstant: 22),
        ])
        return v
    }
}

extension TerminalKeyBar: UIInputViewAudioFeedback {
    var enableInputClicksWhenVisible: Bool { true }
}
