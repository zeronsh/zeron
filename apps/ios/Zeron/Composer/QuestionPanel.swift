import UIKit

/// Replaces the composer while the agent asks questions: one glass card per
/// question, large option rows, "Other…" free text, and a single-select
/// auto-advance (220ms) so a 3-question form is three taps.
final class QuestionPanel: UIView, UITextFieldDelegate {
    var onSubmit: (([(questionId: String, labels: [String])]) -> Void)?
    var onHeightChange: (() -> Void)?

    private let card = Glass.surface(radius: 26)
    private let header = UILabel()
    private let question = UILabel()
    private let options = UIStackView()
    private let other = UITextField()
    private let back = UIButton(type: .system)
    private let nextButton = UIButton(type: .system)
    private var items: [SessionChrome.Question] = []
    private var page = 0
    private var picks: [String: Set<String>] = [:]
    private var custom: [String: String] = [:]

    override init(frame: CGRect) {
        super.init(frame: frame)
        card.translatesAutoresizingMaskIntoConstraints = false
        addSubview(card)
        header.font = Fonts.ui(.sansMedium, 12.5)
        header.textColor = Palette.secondary
        question.font = Fonts.ui(.sansSemibold, 17)
        question.textColor = Palette.text
        question.numberOfLines = 0
        options.axis = .vertical
        options.spacing = 6
        other.placeholder = "Other…"
        other.font = Fonts.ui(.sans, 16)
        other.borderStyle = .none
        other.backgroundColor = Palette.chip.withAlphaComponent(0.6)
        other.layer.cornerRadius = 12
        other.leftView = UIView(frame: CGRect(x: 0, y: 0, width: 12, height: 1))
        other.leftViewMode = .always
        other.returnKeyType = .done
        other.delegate = self
        other.addAction(UIAction { [weak self] _ in self?.customChanged() }, for: .editingChanged)
        other.heightAnchor.constraint(equalToConstant: 44).isActive = true
        other.accessibilityIdentifier = "question-other"

        var backConfig = UIButton.Configuration.plain()
        backConfig.title = "Back"
        backConfig.baseForegroundColor = Palette.secondary
        back.configuration = backConfig
        back.addAction(UIAction { [weak self] _ in self?.go(-1) }, for: .touchUpInside)
        var nextConfig = UIButton.Configuration.prominentGlass()
        nextConfig.title = "Next"
        nextConfig.baseBackgroundColor = Palette.text
        nextConfig.baseForegroundColor = Palette.background
        nextConfig.cornerStyle = .capsule
        nextButton.configuration = nextConfig
        nextButton.accessibilityIdentifier = "question-next"
        nextButton.addAction(UIAction { [weak self] _ in self?.advance() }, for: .touchUpInside)

        let buttons = UIStackView(arrangedSubviews: [back, UIView(), nextButton])
        buttons.axis = .horizontal
        let stack = UIStackView(arrangedSubviews: [header, question, options, other, buttons])
        stack.axis = .vertical
        stack.spacing = 10
        stack.setCustomSpacing(14, after: question)
        stack.setCustomSpacing(14, after: other)
        stack.translatesAutoresizingMaskIntoConstraints = false
        card.contentView.addSubview(stack)
        NSLayoutConstraint.activate([
            card.topAnchor.constraint(equalTo: topAnchor),
            card.leadingAnchor.constraint(equalTo: leadingAnchor),
            card.trailingAnchor.constraint(equalTo: trailingAnchor),
            card.bottomAnchor.constraint(equalTo: bottomAnchor),
            stack.topAnchor.constraint(equalTo: card.contentView.topAnchor, constant: 16),
            stack.leadingAnchor.constraint(equalTo: card.contentView.leadingAnchor, constant: 16),
            stack.trailingAnchor.constraint(equalTo: card.contentView.trailingAnchor, constant: -16),
            stack.bottomAnchor.constraint(equalTo: card.contentView.bottomAnchor, constant: -12),
        ])
    }

    required init?(coder: NSCoder) { fatalError() }

    func configure(_ questions: [SessionChrome.Question]) {
        guard questions != items else { return }
        items = questions
        page = 0
        picks = [:]
        custom = [:]
        render()
    }

    private var current: SessionChrome.Question? { items.indices.contains(page) ? items[page] : nil }

    private func render() {
        guard let q = current else { return }
        header.text = items.count > 1 ? "\(q.header.isEmpty ? "Question" : q.header) · \(page + 1) of \(items.count)" : (q.header.isEmpty ? "Question" : q.header)
        question.text = q.text
        options.arrangedSubviews.forEach { $0.removeFromSuperview() }
        let chosen = picks[q.id] ?? []
        for (i, option) in q.options.enumerated() {
            var c = UIButton.Configuration.plain()
            c.title = option
            c.image = UIImage(systemName: chosen.contains(option) ? (q.multiSelect ? "checkmark.square.fill" : "checkmark.circle.fill") : "\(i + 1).circle")
            c.imagePadding = 10
            c.baseForegroundColor = chosen.contains(option) ? Palette.accent : Palette.text
            c.contentInsets = NSDirectionalEdgeInsets(top: 11, leading: 12, bottom: 11, trailing: 12)
            c.titleTextAttributesTransformer = UIConfigurationTextAttributesTransformer { a in
                var a = a
                a.font = Fonts.ui(.sansMedium, 16)
                return a
            }
            c.titleAlignment = .leading
            c.background.backgroundColor = chosen.contains(option) ? Palette.accent.withAlphaComponent(0.12) : Palette.chip.withAlphaComponent(0.55)
            c.background.cornerRadius = 14
            let b = UIButton(configuration: c)
            b.contentHorizontalAlignment = .leading
            b.accessibilityIdentifier = "question-option-\(i)"
            b.addAction(UIAction { [weak self] _ in self?.pick(option) }, for: .touchUpInside)
            options.addArrangedSubview(b)
        }
        other.text = custom[q.id]
        back.isHidden = page == 0
        var n = nextButton.configuration
        n?.title = page == items.count - 1 ? "Submit" : "Next"
        nextButton.configuration = n
        nextButton.isEnabled = answered(q)
        onHeightChange?()
    }

    private func answered(_ q: SessionChrome.Question) -> Bool {
        !(picks[q.id] ?? []).isEmpty || !(custom[q.id] ?? "").trimmingCharacters(in: .whitespaces).isEmpty
    }

    private func pick(_ option: String) {
        guard let q = current else { return }
        UISelectionFeedbackGenerator().selectionChanged()
        var set = picks[q.id] ?? []
        if q.multiSelect {
            if set.contains(option) { set.remove(option) } else { set.insert(option) }
        } else {
            set = [option]
        }
        picks[q.id] = set
        render()
        if !q.multiSelect {
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.22) { [weak self] in self?.advance() }
        }
    }

    private func customChanged() {
        guard let q = current else { return }
        custom[q.id] = other.text
        nextButton.isEnabled = answered(q)
    }

    func textFieldShouldReturn(_ textField: UITextField) -> Bool {
        advance()
        return true
    }

    private func go(_ delta: Int) {
        page = max(0, min(items.count - 1, page + delta))
        UIView.transition(with: card, duration: 0.2, options: [.transitionCrossDissolve, .allowUserInteraction]) { self.render() }
    }

    private func advance() {
        guard let q = current, answered(q) else { return }
        if page < items.count - 1 {
            go(1)
            return
        }
        let answers = items.map { q -> (questionId: String, labels: [String]) in
            var labels = q.options.filter { (picks[q.id] ?? []).contains($0) }
            if let c = custom[q.id]?.trimmingCharacters(in: .whitespaces), !c.isEmpty { labels.append(c) }
            return (q.id, labels)
        }
        UIImpactFeedbackGenerator(style: .medium).impactOccurred()
        onSubmit?(answers)
    }
}

/// Messages waiting for the current turn to end, stacked above the composer.
final class QueuePanel: UIView {
    var onAction: ((String, QueueAction) -> Void)?
    private let card = Glass.surface(radius: 20)
    private let stack = UIStackView()
    private(set) var items: [SessionChrome.QueuedItem] = []

    override init(frame: CGRect) {
        super.init(frame: frame)
        card.translatesAutoresizingMaskIntoConstraints = false
        addSubview(card)
        stack.axis = .vertical
        stack.spacing = 0
        stack.translatesAutoresizingMaskIntoConstraints = false
        card.contentView.addSubview(stack)
        NSLayoutConstraint.activate([
            card.topAnchor.constraint(equalTo: topAnchor),
            card.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 10),
            card.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -10),
            card.bottomAnchor.constraint(equalTo: bottomAnchor),
            stack.topAnchor.constraint(equalTo: card.contentView.topAnchor, constant: 4),
            stack.leadingAnchor.constraint(equalTo: card.contentView.leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: card.contentView.trailingAnchor),
            stack.bottomAnchor.constraint(equalTo: card.contentView.bottomAnchor, constant: -4),
        ])
    }

    required init?(coder: NSCoder) { fatalError() }

    func configure(_ items: [SessionChrome.QueuedItem]) {
        guard items != self.items else { return }
        self.items = items
        stack.arrangedSubviews.forEach { $0.removeFromSuperview() }
        for (i, item) in items.prefix(3).enumerated() {
            stack.addArrangedSubview(row(item, index: i, count: items.count))
        }
        if items.count > 3 {
            let more = UILabel()
            more.text = "+\(items.count - 3) more queued"
            more.font = Fonts.ui(.sansMedium, 12.5)
            more.textColor = Palette.tertiary
            more.textAlignment = .center
            more.heightAnchor.constraint(equalToConstant: 24).isActive = true
            stack.addArrangedSubview(more)
        }
    }

    private func row(_ item: SessionChrome.QueuedItem, index: Int, count: Int) -> UIView {
        let v = UIView()
        v.heightAnchor.constraint(equalToConstant: 40).isActive = true
        let icon = UIImageView(image: UIImage(systemName: "clock", withConfiguration: UIImage.SymbolConfiguration(pointSize: 12, weight: .medium)))
        icon.tintColor = Palette.tertiary
        let thumb = UIImageView(image: item.thumbnail)
        thumb.contentMode = .scaleAspectFill
        thumb.clipsToBounds = true
        thumb.layer.cornerRadius = 6
        thumb.isHidden = item.thumbnail == nil
        let label = UILabel()
        label.text = item.gate.map { "\($0) · \(item.text)" } ?? item.text
        label.font = Fonts.ui(.sans, 15)
        label.textColor = Palette.text
        label.lineBreakMode = .byTruncatingTail
        var sendConfig = UIButton.Configuration.plain()
        sendConfig.image = UIImage(systemName: "arrow.up.circle.fill", withConfiguration: UIImage.SymbolConfiguration(pointSize: 20, weight: .regular))
        sendConfig.baseForegroundColor = Palette.text
        let send = UIButton(configuration: sendConfig, primaryAction: UIAction { [weak self] _ in self?.onAction?(item.id, .sendNow) })
        send.accessibilityLabel = "Send now"
        var moreConfig = UIButton.Configuration.plain()
        moreConfig.image = UIImage(systemName: "ellipsis", withConfiguration: UIImage.SymbolConfiguration(pointSize: 14, weight: .semibold))
        moreConfig.baseForegroundColor = Palette.secondary
        let more = UIButton(configuration: moreConfig)
        more.accessibilityLabel = "More queue actions"
        more.showsMenuAsPrimaryAction = true
        more.menu = UIMenu(children: [
            UIAction(title: "Edit", image: UIImage(systemName: "pencil")) { [weak self] _ in self?.onAction?(item.id, .edit) },
            UIAction(title: "Move Up", image: UIImage(systemName: "arrow.up"), attributes: index == 0 ? .disabled : []) { [weak self] _ in self?.onAction?(item.id, .moveUp) },
            UIAction(title: "Move Down", image: UIImage(systemName: "arrow.down"), attributes: index == count - 1 ? .disabled : []) { [weak self] _ in self?.onAction?(item.id, .moveDown) },
            UIAction(title: "Remove", image: UIImage(systemName: "trash"), attributes: .destructive) { [weak self] _ in self?.onAction?(item.id, .remove) },
        ])
        for s in [icon, thumb, label, send, more] {
            s.translatesAutoresizingMaskIntoConstraints = false
            v.addSubview(s)
        }
        NSLayoutConstraint.activate([
            icon.leadingAnchor.constraint(equalTo: v.leadingAnchor, constant: 14),
            icon.centerYAnchor.constraint(equalTo: v.centerYAnchor),
            thumb.leadingAnchor.constraint(equalTo: icon.trailingAnchor, constant: 8),
            thumb.centerYAnchor.constraint(equalTo: v.centerYAnchor),
            thumb.widthAnchor.constraint(equalToConstant: item.thumbnail == nil ? 0 : 26),
            thumb.heightAnchor.constraint(equalToConstant: 26),
            label.leadingAnchor.constraint(equalTo: thumb.trailingAnchor, constant: item.thumbnail == nil ? 0 : 8),
            label.centerYAnchor.constraint(equalTo: v.centerYAnchor),
            label.trailingAnchor.constraint(lessThanOrEqualTo: send.leadingAnchor, constant: -4),
            send.trailingAnchor.constraint(equalTo: more.leadingAnchor),
            send.centerYAnchor.constraint(equalTo: v.centerYAnchor),
            more.trailingAnchor.constraint(equalTo: v.trailingAnchor, constant: -4),
            more.centerYAnchor.constraint(equalTo: v.centerYAnchor),
        ])
        return v
    }
}
