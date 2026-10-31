import UIKit

/// The pieces every Cloud screen is built from, in the app's own language:
/// the sign-in screen's capsule buttons, Settings' grouped rows, brand marks.
enum CloudUI {
    /// The filled capsule (text color on background): the one way forward.
    static func primaryButton(_ title: String, symbol: String? = nil, action: @escaping () -> Void) -> UIButton {
        var c = UIButton.Configuration.filled()
        c.title = title
        c.baseBackgroundColor = Palette.text
        c.baseForegroundColor = Palette.background
        c.cornerStyle = .capsule
        c.contentInsets = NSDirectionalEdgeInsets(top: 15, leading: 20, bottom: 15, trailing: 20)
        if let symbol {
            c.image = UIImage(systemName: symbol, withConfiguration: UIImage.SymbolConfiguration(pointSize: 15, weight: .semibold))
            c.imagePadding = 8
        }
        c.titleTextAttributesTransformer = UIConfigurationTextAttributesTransformer { a in
            var a = a
            a.font = Fonts.ui(.sansSemibold, 17)
            return a
        }
        return UIButton(configuration: c, primaryAction: UIAction { _ in action() })
    }

    /// A quiet glass capsule beside or under the primary one.
    static func secondaryButton(_ title: String, symbol: String? = nil, action: @escaping () -> Void) -> UIButton {
        var c = UIButton.Configuration.glass()
        c.title = title
        c.baseForegroundColor = Palette.text
        c.cornerStyle = .capsule
        c.contentInsets = NSDirectionalEdgeInsets(top: 14, leading: 20, bottom: 14, trailing: 20)
        if let symbol {
            c.image = UIImage(systemName: symbol, withConfiguration: UIImage.SymbolConfiguration(pointSize: 15, weight: .medium))
            c.imagePadding = 8
        }
        c.titleTextAttributesTransformer = UIConfigurationTextAttributesTransformer { a in
            var a = a
            a.font = Fonts.ui(.sansMedium, 16)
            return a
        }
        return UIButton(configuration: c, primaryAction: UIAction { _ in action() })
    }

    /// A text-only button ("Skip for now").
    static func plainButton(_ title: String, action: @escaping () -> Void) -> UIButton {
        var c = UIButton.Configuration.plain()
        c.title = title
        c.baseForegroundColor = Palette.secondary
        c.titleTextAttributesTransformer = UIConfigurationTextAttributesTransformer { a in
            var a = a
            a.font = Fonts.ui(.sansMedium, 15)
            return a
        }
        return UIButton(configuration: c, primaryAction: UIAction { _ in action() })
    }

    /// A rounded tile holding a glyph (brand mark or symbol).
    static func tile(image: UIImage?, side: CGFloat = 40, tint: UIColor = Palette.text) -> UIView {
        let box = UIView()
        box.backgroundColor = Palette.controlFill
        box.layer.cornerRadius = side * 0.28
        box.layer.cornerCurve = .continuous
        let iv = UIImageView(image: image)
        iv.tintColor = tint
        iv.contentMode = .scaleAspectFit
        iv.translatesAutoresizingMaskIntoConstraints = false
        box.addSubview(iv)
        box.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            box.widthAnchor.constraint(equalToConstant: side),
            box.heightAnchor.constraint(equalToConstant: side),
            iv.centerXAnchor.constraint(equalTo: box.centerXAnchor),
            iv.centerYAnchor.constraint(equalTo: box.centerYAnchor),
            iv.widthAnchor.constraint(equalToConstant: side * 0.5),
            iv.heightAnchor.constraint(equalToConstant: side * 0.5),
        ])
        return box
    }

    static var githubMark: UIImage? {
        UIImage(named: "github-mark")?.withRenderingMode(.alwaysTemplate)
    }

    static var zeronMark: UIImage? {
        UIImage(named: "zeron-logo")?.withRenderingMode(.alwaysTemplate)
    }

    static func symbol(_ name: String, _ size: CGFloat = 17, weight: UIImage.SymbolWeight = .medium) -> UIImage? {
        UIImage(systemName: name, withConfiguration: UIImage.SymbolConfiguration(pointSize: size, weight: weight))
    }

    /// Title + body, the way every onboarding and Cloud page opens.
    static func header(_ title: String, _ body: String, alignment: NSTextAlignment = .natural) -> UIStackView {
        let t = UILabel()
        t.text = title
        t.font = Fonts.ui(.sansSemibold, 28)
        t.textColor = Palette.text
        t.numberOfLines = 0
        t.textAlignment = alignment
        let b = UILabel()
        b.text = body
        b.font = Fonts.ui(.sans, 16)
        b.textColor = Palette.secondary
        b.numberOfLines = 0
        b.textAlignment = alignment
        let stack = UIStackView(arrangedSubviews: [t, b])
        stack.axis = .vertical
        stack.spacing = 10
        return stack
    }

    /// "41,250".
    static func credits(_ value: Double) -> String {
        let f = NumberFormatter()
        f.numberStyle = .decimal
        f.maximumFractionDigits = 0
        return f.string(from: NSNumber(value: value.rounded(.down))) ?? "\(Int(value))"
    }

    /// "$20" / "$12.50".
    static func price(_ cents: UInt32) -> String {
        cents % 100 == 0 ? "$\(cents / 100)" : String(format: "$%.2f", Double(cents) / 100)
    }

    static func shortDate(_ ms: Int64) -> String {
        let f = DateFormatter()
        f.setLocalizedDateFormatFromTemplate("MMM d")
        return f.string(from: Date(timeIntervalSince1970: TimeInterval(ms) / 1000))
    }
}

/// One row of an onboarding or Cloud card: a tile, a title over a muted
/// line, and an accessory (button, check, spinner) on the trailing edge.
final class CloudRow: UIView {
    let titleLabel = UILabel()
    let detailLabel = UILabel()
    private let accessoryHost = UIView()

    init(tile: UIView, title: String, detail: String?) {
        super.init(frame: .zero)
        titleLabel.text = title
        titleLabel.font = Fonts.ui(.sansMedium, 16)
        titleLabel.textColor = Palette.text
        detailLabel.text = detail
        detailLabel.font = Fonts.ui(.sans, 13)
        detailLabel.textColor = Palette.secondary
        detailLabel.numberOfLines = 2
        let text = UIStackView(arrangedSubviews: [titleLabel, detailLabel])
        text.axis = .vertical
        text.spacing = 2
        let row = UIStackView(arrangedSubviews: [tile, text, accessoryHost])
        row.axis = .horizontal
        row.alignment = .center
        row.spacing = 14
        row.translatesAutoresizingMaskIntoConstraints = false
        accessoryHost.setContentHuggingPriority(.required, for: .horizontal)
        text.setContentHuggingPriority(UILayoutPriority(1), for: .horizontal)
        addSubview(row)
        NSLayoutConstraint.activate([
            row.topAnchor.constraint(equalTo: topAnchor, constant: 12),
            row.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -12),
            row.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 16),
            row.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -16),
        ])
    }

    required init?(coder: NSCoder) { fatalError() }

    /// Swap the trailing accessory.
    func setAccessory(_ view: UIView?) {
        accessoryHost.subviews.forEach { $0.removeFromSuperview() }
        guard let view else { return }
        view.translatesAutoresizingMaskIntoConstraints = false
        // Its own size, never the row's spare width (the text takes that).
        view.setContentHuggingPriority(.required, for: .horizontal)
        view.setContentCompressionResistancePriority(.required, for: .horizontal)
        accessoryHost.addSubview(view)
        NSLayoutConstraint.activate([
            view.topAnchor.constraint(equalTo: accessoryHost.topAnchor),
            view.bottomAnchor.constraint(equalTo: accessoryHost.bottomAnchor),
            view.leadingAnchor.constraint(equalTo: accessoryHost.leadingAnchor),
            view.trailingAnchor.constraint(equalTo: accessoryHost.trailingAnchor),
        ])
    }

    /// A compact capsule for a row ("Sign in", "Connect").
    static func rowButton(_ title: String, prominent: Bool = true, action: @escaping () -> Void) -> UIButton {
        var c: UIButton.Configuration = prominent ? .filled() : .glass()
        c.title = title
        c.cornerStyle = .capsule
        c.baseBackgroundColor = prominent ? Palette.text : nil
        c.baseForegroundColor = prominent ? Palette.background : Palette.text
        c.contentInsets = NSDirectionalEdgeInsets(top: 8, leading: 14, bottom: 8, trailing: 14)
        c.titleTextAttributesTransformer = UIConfigurationTextAttributesTransformer { a in
            var a = a
            a.font = Fonts.ui(.sansSemibold, 14)
            return a
        }
        return UIButton(configuration: c, primaryAction: UIAction { _ in action() })
    }

    static func check() -> UIView {
        let iv = UIImageView(image: CloudUI.symbol("checkmark.circle.fill", 22, weight: .semibold))
        iv.tintColor = Palette.success
        iv.contentMode = .scaleAspectFit
        iv.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            iv.widthAnchor.constraint(equalToConstant: 26),
            iv.heightAnchor.constraint(equalToConstant: 26),
        ])
        return iv
    }
}

/// A card of rows separated by hairlines (onboarding pages).
final class CloudCard: UIView {
    private let stack = UIStackView()

    init(rows: [UIView]) {
        super.init(frame: .zero)
        backgroundColor = Palette.elevated
        layer.cornerRadius = 22
        layer.cornerCurve = .continuous
        stack.axis = .vertical
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: topAnchor, constant: 4),
            stack.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -4),
            stack.leadingAnchor.constraint(equalTo: leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: trailingAnchor),
        ])
        setRows(rows)
    }

    required init?(coder: NSCoder) { fatalError() }

    func setRows(_ rows: [UIView]) {
        stack.arrangedSubviews.forEach { $0.removeFromSuperview() }
        for (i, row) in rows.enumerated() {
            if i > 0 {
                let line = UIView()
                line.backgroundColor = Palette.hairline
                line.translatesAutoresizingMaskIntoConstraints = false
                let holder = UIView()
                holder.addSubview(line)
                NSLayoutConstraint.activate([
                    line.heightAnchor.constraint(equalToConstant: 1 / UIScreen.main.scale),
                    line.leadingAnchor.constraint(equalTo: holder.leadingAnchor, constant: 70),
                    line.trailingAnchor.constraint(equalTo: holder.trailingAnchor),
                    line.topAnchor.constraint(equalTo: holder.topAnchor),
                    line.bottomAnchor.constraint(equalTo: holder.bottomAnchor),
                ])
                stack.addArrangedSubview(holder)
            }
            stack.addArrangedSubview(row)
        }
    }
}

extension UIFont {
    /// Figures that don't shift as they change.
    func withMonospacedDigits() -> UIFont {
        let descriptor = fontDescriptor.addingAttributes([
            .featureSettings: [[UIFontDescriptor.FeatureKey.type: kNumberSpacingType, UIFontDescriptor.FeatureKey.selector: kMonospacedNumbersSelector]],
        ])
        return UIFont(descriptor: descriptor, size: pointSize)
    }
}

extension UIImage {
    /// The image, fainter (a dimmed row's mark).
    func withAlpha(_ alpha: CGFloat) -> UIImage? {
        UIGraphicsImageRenderer(size: size, format: imageRendererFormat).image { _ in
            draw(at: .zero, blendMode: .normal, alpha: alpha)
        }
    }
}
