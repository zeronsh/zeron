import UIKit

/// A subagent row in a session's side chats sheet. Its status glyph stands
/// where a side chat shows its harness mark, so the two read apart at a
/// glance:
///
///   ⠿  Audit the reduced-motion fallback ··········· Haiku
///      Reading RowView.swift…                (running: live tail)
///   ✓  Check CJK line breaking ······················ 12m
final class SubagentCell: UICollectionViewListCell {
    static func height(running: Bool) -> CGFloat { ((running ? 62 : 46) * TypeScale.factor).rounded() }

    private let glyph = StatusGlyph()
    private let title = FadingLabel()
    private let tail = FadingLabel()
    private let trailing = UILabel()
    private var running = false

    override init(frame: CGRect) {
        super.init(frame: frame)
        title.textColor = Palette.text
        tail.textColor = Palette.secondary
        trailing.textAlignment = .right
        for v in [glyph, title, tail, trailing] as [UIView] { contentView.addSubview(v) }
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

    override func preferredLayoutAttributesFitting(_ attrs: UICollectionViewLayoutAttributes) -> UICollectionViewLayoutAttributes {
        attrs.size.height = Self.height(running: running)
        return attrs
    }

    func configure(_ agent: SubagentInfo) {
        running = agent.state == .running
        let word: String
        switch agent.state {
        case .running:
            glyph.kind = .spinner
            word = "Running"
        case .done:
            glyph.kind = .check(StatusTone.done)
            word = "Done"
        case .failed:
            glyph.kind = .dot(StatusTone.failed)
            word = "Failed"
        }
        title.text = agent.title
        title.font = Fonts.ui(.sansMedium, TypeScale.size(running ? 16.5 : 15.5))
        title.textColor = running ? Palette.text : Palette.text.withAlphaComponent(0.88)
        tail.font = Fonts.ui(.sans, TypeScale.size(13.5))
        tail.text = agent.tail
        tail.isHidden = !running || agent.tail == nil
        // Running: which model it runs on. Finished: when it was spawned.
        let model = agent.model.map { $0.prefix(1).uppercased() + $0.dropFirst() }
        trailing.font = Fonts.ui(.sansMedium, TypeScale.size(13))
        if running {
            trailing.text = model
            trailing.textColor = Palette.secondary
        } else {
            trailing.text = relativeTimeLabel(atMs: agent.spawnedAtMs, nowMs: Int64(Date().timeIntervalSince1970 * 1000))
            trailing.textColor = agent.state == .failed ? StatusTone.failed : StatusTone.time
        }
        accessibilityLabel = ["Subagent", agent.title, word, model, running ? agent.tail : nil].compactMap { $0 }.joined(separator: ", ")
        accessibilityTraits = .button
        accessibilityIdentifier = "subagent-\(agent.docId)"
        setNeedsLayout()
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        let b = contentView.bounds
        let k = TypeScale.factor
        let left: CGFloat = 20
        let right: CGFloat = 20
        let titleH = (22 * k).rounded()
        let titleY = running ? (10 * k).rounded() : (b.height - titleH) / 2
        let mark = SessionCell.markSide
        glyph.frame = CGRect(x: left + (mark - 12) / 2, y: titleY + titleH / 2 - 6, width: 12, height: 12)
        let textX = left + mark + 14
        let tw = trailing.text == nil ? 0 : ceil(trailing.sizeThatFits(CGSize(width: 120, height: 40)).width)
        trailing.frame = CGRect(x: b.width - right - tw, y: titleY, width: tw, height: titleH)
        let titleRight = tw > 0 ? trailing.frame.minX - 10 : b.width - right
        title.frame = CGRect(x: textX, y: titleY, width: max(0, titleRight - textX), height: titleH)
        tail.frame = CGRect(x: textX, y: (35 * k).rounded(), width: max(0, b.width - right - textX), height: (18 * k).rounded())
    }
}
