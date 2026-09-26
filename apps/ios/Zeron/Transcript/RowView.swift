import UIKit

/// Draws one layer of a row model. Plain `draw(_:)` into a layer CoreAnimation
/// renders asynchronously; no Auto Layout, no text measurement.
final class RowCanvas: UIView {
    var model: RowModel? { didSet { setNeedsDisplay() } }
    var layerIndex = 0

    override init(frame: CGRect) {
        super.init(frame: frame)
        isOpaque = false
        backgroundColor = .clear
        contentMode = .topLeft
        layer.drawsAsynchronously = true
        isUserInteractionEnabled = false
    }

    required init?(coder: NSCoder) { fatalError() }

    override func draw(_ rect: CGRect) {
        guard let model, let ctx = UIGraphicsGetCurrentContext() else { return }
        model.draw(layer: layerIndex, in: ctx, traits: traitCollection, hairline: 1 / max(1, traitCollection.displayScale))
    }

    override func traitCollectionDidChange(_ previous: UITraitCollection?) {
        super.traitCollectionDidChange(previous)
        if previous?.userInterfaceStyle != traitCollection.userInterfaceStyle { setNeedsDisplay() }
    }
}

protocol RowViewDelegate: AnyObject {
    func rowView(_ view: RowView, toggle key: UInt64)
    func rowView(_ view: RowView, open url: URL)
    func rowView(_ view: RowView, imageFor reference: String, into imageView: UIImageView)
}

/// A reusable transcript row: canvas + horizontal scrollers + native widgets.
final class RowView: UIView {
    private(set) var model: RowModel?
    private let canvas = RowCanvas()
    private var scrollers: [UIScrollView] = []
    private var widgetViews: [UIView] = []
    weak var delegate: RowViewDelegate?
    var key: UInt64 { model?.display.key ?? 0 }
    var version: UInt64 { model?.display.version ?? 0 }
    var kind: RowKind = .markdown

    override init(frame: CGRect) {
        super.init(frame: frame)
        addSubview(canvas)
        clipsToBounds = false
    }

    required init?(coder: NSCoder) { fatalError() }

    func configure(_ model: RowModel, kind: RowKind) {
        self.model = model
        self.kind = kind
        let d = model.display
        canvas.frame = CGRect(x: 0, y: 0, width: CGFloat(d.width), height: CGFloat(d.height))
        canvas.model = model
        // Scrollers: reuse views in order.
        while scrollers.count < d.scrollers.count {
            let s = UIScrollView()
            s.showsHorizontalScrollIndicator = false
            s.showsVerticalScrollIndicator = false
            s.alwaysBounceHorizontal = false
            s.alwaysBounceVertical = false
            s.contentInsetAdjustmentBehavior = .never
            s.scrollsToTop = false
            s.clipsToBounds = true
            let c = RowCanvas()
            s.addSubview(c)
            addSubview(s)
            scrollers.append(s)
        }
        for (i, s) in scrollers.enumerated() {
            guard i < d.scrollers.count else {
                s.isHidden = true
                continue
            }
            let info = d.scrollers[i]
            s.isHidden = false
            s.frame = CGRect(x: CGFloat(info.x), y: CGFloat(info.y), width: CGFloat(info.w), height: CGFloat(info.h))
            s.contentSize = CGSize(width: CGFloat(info.contentWidth), height: CGFloat(info.h))
            s.contentOffset = .zero
            s.layer.cornerRadius = 0
            if let c = s.subviews.first(where: { $0 is RowCanvas }) as? RowCanvas {
                c.frame = CGRect(x: 0, y: 0, width: CGFloat(info.contentWidth), height: CGFloat(info.h))
                c.layerIndex = i + 1
                c.model = model
            }
        }
        layoutWidgets(d)
    }

    private func layoutWidgets(_ d: RowDisplay) {
        for v in widgetViews { v.removeFromSuperview() }
        widgetViews.removeAll(keepingCapacity: true)
        for w in d.widgets {
            let rect = CGRect(x: CGFloat(w.x), y: CGFloat(w.y), width: CGFloat(w.w), height: CGFloat(w.h))
            let host: UIView = w.scroller.flatMap { Int($0) < scrollers.count ? scrollers[Int($0)] : nil } ?? self
            let view: UIView
            switch w.kind {
            case .copyCode:
                let b = CopyButton(payload: w.payload ?? "")
                view = b
            case let .disclosure(expanded):
                let b = DisclosureControl(expanded: expanded, chevron: kind == .tools)
                b.addAction(UIAction { [weak self] _ in
                    guard let self else { return }
                    self.delegate?.rowView(self, toggle: d.key)
                }, for: .touchUpInside)
                view = b
            case let .toolStatus(running, failed):
                view = ToolStatusView(running: running, failed: failed)
            case let .image(reference):
                let iv = UIImageView()
                iv.contentMode = .scaleAspectFill
                iv.clipsToBounds = true
                iv.layer.cornerRadius = min(rect.width, rect.height) > 120 ? 14 : 12
                iv.layer.cornerCurve = .continuous
                delegate?.rowView(self, imageFor: reference, into: iv)
                view = iv
            case .spinner:
                view = DotGridView(style: .working)
            case let .icon(name, color):
                let iv = UIImageView(image: UIImage(systemName: name, withConfiguration: UIImage.SymbolConfiguration(pointSize: rect.height * 0.8, weight: .medium)))
                iv.tintColor = Palette.color(color)
                iv.contentMode = .center
                view = iv
            }
            view.frame = rect
            host.addSubview(view)
            widgetViews.append(view)
        }
    }

    /// Link at a point in row coordinates.
    func link(at point: CGPoint) -> URL? {
        guard let d = model?.display else { return nil }
        for l in d.links {
            var p = point
            if let s = l.scroller, Int(s) < scrollers.count {
                let sv = scrollers[Int(s)]
                p = CGPoint(x: point.x - sv.frame.minX + sv.contentOffset.x, y: point.y - sv.frame.minY)
            }
            let r = CGRect(x: CGFloat(l.x), y: CGFloat(l.y), width: CGFloat(l.w), height: CGFloat(l.h)).insetBy(dx: -4, dy: -2)
            if r.contains(p) { return URL(string: l.url) }
        }
        return nil
    }
}

/// Code-block copy button with a checkmark confirmation.
final class CopyButton: UIButton {
    private let payload: String

    init(payload: String) {
        self.payload = payload
        super.init(frame: .zero)
        let config = UIImage.SymbolConfiguration(pointSize: 13, weight: .medium)
        setImage(UIImage(systemName: "square.on.square", withConfiguration: config), for: .normal)
        tintColor = Palette.tertiary
        accessibilityLabel = "Copy code"
        addAction(UIAction { [weak self] _ in self?.copy() }, for: .touchUpInside)
    }

    required init?(coder: NSCoder) { fatalError() }

    private func copy() {
        UIPasteboard.general.string = payload
        UIImpactFeedbackGenerator(style: .light).impactOccurred()
        let config = UIImage.SymbolConfiguration(pointSize: 13, weight: .semibold)
        setImage(UIImage(systemName: "checkmark", withConfiguration: config), for: .normal)
        tintColor = Palette.accent
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.4) { [weak self] in
            self?.setImage(UIImage(systemName: "square.on.square", withConfiguration: UIImage.SymbolConfiguration(pointSize: 13, weight: .medium)), for: .normal)
            self?.tintColor = Palette.tertiary
        }
    }
}

/// Invisible tap target; tool groups also show a rotating chevron.
final class DisclosureControl: UIControl {
    init(expanded: Bool, chevron: Bool) {
        super.init(frame: .zero)
        accessibilityTraits = .button
        accessibilityLabel = expanded ? "Collapse" : "Expand"
        guard chevron else { return }
        let iv = UIImageView(image: UIImage(systemName: "chevron.right", withConfiguration: UIImage.SymbolConfiguration(pointSize: 10, weight: .semibold)))
        iv.tintColor = Palette.tertiary
        iv.transform = expanded ? CGAffineTransform(rotationAngle: .pi / 2) : .identity
        iv.tag = 1
        addSubview(iv)
    }

    required init?(coder: NSCoder) { fatalError() }

    override func layoutSubviews() {
        super.layoutSubviews()
        if let iv = viewWithTag(1) {
            iv.sizeToFit()
            iv.center = CGPoint(x: bounds.width - 12, y: bounds.midY)
        }
    }

    override var isHighlighted: Bool {
        didSet { alpha = isHighlighted ? 0.5 : 1 }
    }
}

/// Tool state glyph: running dot-grid, failed cross, or a quiet done dot.
final class ToolStatusView: UIView {
    init(running: Bool, failed: Bool) {
        super.init(frame: .zero)
        isUserInteractionEnabled = false
        if running {
            let grid = DotGridView(style: .working)
            grid.autoresizingMask = [.flexibleWidth, .flexibleHeight]
            addSubview(grid)
        } else {
            let iv = UIImageView(image: UIImage(systemName: failed ? "xmark" : "checkmark", withConfiguration: UIImage.SymbolConfiguration(pointSize: 10, weight: .bold)))
            iv.tintColor = failed ? Palette.danger : Palette.tertiary
            iv.contentMode = .center
            iv.autoresizingMask = [.flexibleWidth, .flexibleHeight]
            addSubview(iv)
        }
    }

    required init?(coder: NSCoder) { fatalError() }

    override func layoutSubviews() {
        super.layoutSubviews()
        subviews.forEach { $0.frame = bounds }
    }
}
