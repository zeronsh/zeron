import UIKit

/// `@file` mentions: the composer shows `@name` tokens; on send they become
/// the canonical `[name](zeron-file:path)` links the host resolves.
struct MentionIndex {
    private(set) var tokens: [String: FileMatch] = [:]

    /// Token for a file, disambiguated by parent folder on basename clashes.
    mutating func token(for file: FileMatch) -> String {
        let parts = file.path.trimmingCharacters(in: CharacterSet(charactersIn: "/")).split(separator: "/")
        var token = "@" + (parts.last.map(String.init) ?? file.path)
        if let existing = tokens[token], existing.path != file.path, parts.count > 1 {
            token = "@" + parts.suffix(2).joined(separator: "/")
        }
        tokens[token] = file
        return token
    }

    /// Replace live tokens with canonical links; forget tokens no longer present.
    func encode(_ text: String) -> String {
        var out = text
        for (token, file) in tokens.sorted(by: { $0.key.count > $1.key.count }) where out.contains(token) {
            out = out.replacingOccurrences(of: token, with: fileMentionLink(path: file.path, isDir: file.isDir))
        }
        return out
    }

    mutating func reset() { tokens.removeAll() }

    /// The `@query` being typed at `cursor`, if any (no whitespace inside).
    static func activeQuery(in text: String, cursor: Int) -> (range: NSRange, query: String)? {
        let ns = text as NSString
        guard cursor <= ns.length else { return nil }
        var i = cursor - 1
        while i >= 0 {
            let c = ns.character(at: i)
            if c == 64 /* @ */ {
                let before = i > 0 ? ns.character(at: i - 1) : 32
                guard before == 32 || before == 10 else { return nil }
                let range = NSRange(location: i, length: cursor - i)
                return (range, ns.substring(with: NSRange(location: i + 1, length: cursor - i - 1)))
            }
            if c == 32 || c == 10 { return nil }
            i -= 1
        }
        return nil
    }
}

/// Glass list of matching files above the composer.
final class MentionSuggestions: UIView {
    var onPick: ((FileMatch) -> Void)?
    private let card = Glass.surface(radius: 18)
    private let stack = UIStackView()
    private(set) var files: [FileMatch] = []

    override init(frame: CGRect) {
        super.init(frame: frame)
        card.translatesAutoresizingMaskIntoConstraints = false
        addSubview(card)
        stack.axis = .vertical
        stack.translatesAutoresizingMaskIntoConstraints = false
        card.contentView.addSubview(stack)
        NSLayoutConstraint.activate([
            card.topAnchor.constraint(equalTo: topAnchor),
            card.leadingAnchor.constraint(equalTo: leadingAnchor),
            card.trailingAnchor.constraint(equalTo: trailingAnchor),
            card.bottomAnchor.constraint(equalTo: bottomAnchor),
            stack.topAnchor.constraint(equalTo: card.contentView.topAnchor, constant: 4),
            stack.leadingAnchor.constraint(equalTo: card.contentView.leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: card.contentView.trailingAnchor),
            stack.bottomAnchor.constraint(equalTo: card.contentView.bottomAnchor, constant: -4),
        ])
    }

    required init?(coder: NSCoder) { fatalError() }

    func show(_ files: [FileMatch]) {
        self.files = Array(files.prefix(5))
        stack.arrangedSubviews.forEach { $0.removeFromSuperview() }
        for (i, file) in self.files.enumerated() {
            var c = UIButton.Configuration.plain()
            let name = file.path.split(separator: "/").last.map(String.init) ?? file.path
            let dir = file.path.components(separatedBy: "/").dropLast().joined(separator: "/")
            c.image = UIImage(systemName: file.isDir ? "folder" : "doc.text", withConfiguration: UIImage.SymbolConfiguration(pointSize: 13, weight: .medium))
            c.imagePadding = 10
            c.baseForegroundColor = Palette.text
            var title = AttributedString(name)
            title.font = Fonts.ui(.sansMedium, 15)
            c.attributedTitle = title
            if !dir.isEmpty {
                var sub = AttributedString(dir)
                sub.font = Fonts.ui(.mono, 11.5)
                sub.foregroundColor = Palette.tertiary
                c.attributedSubtitle = sub
            }
            c.titleAlignment = .leading
            c.contentInsets = NSDirectionalEdgeInsets(top: 8, leading: 14, bottom: 8, trailing: 14)
            let b = UIButton(configuration: c, primaryAction: UIAction { [weak self] _ in self?.onPick?(file) })
            b.contentHorizontalAlignment = .leading
            b.accessibilityIdentifier = "mention-\(i)"
            stack.addArrangedSubview(b)
        }
    }
}
