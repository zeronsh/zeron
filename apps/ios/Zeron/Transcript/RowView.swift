import UIKit

/// Draws one layer of a row model. Plain `draw(_:)` into a layer CoreAnimation
/// renders asynchronously; no Auto Layout, no text measurement.
final class RowCanvas: UIView {
    var model: RowModel? { didSet { setNeedsDisplay() } }
    var layerIndex = 0
    var pass: RowModel.Pass = .all

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
        let state = Signposts.transcript.beginInterval("draw-row")
        defer { Signposts.transcript.endInterval("draw-row", state) }
        model.draw(layer: layerIndex, in: ctx, traits: traitCollection, hairline: 1 / max(1, traitCollection.displayScale), pass: pass)
    }

    override func traitCollectionDidChange(_ previous: UITraitCollection?) {
        super.traitCollectionDidChange(previous)
        if previous?.userInterfaceStyle != traitCollection.userInterfaceStyle { setNeedsDisplay() }
    }
}

protocol RowViewDelegate: AnyObject {
    func rowView(_ view: RowView, toggle key: UInt64)
    func rowView(_ view: RowView, toggleDetail detail: UInt64, open: Bool)
    func rowView(_ view: RowView, action: String)
    func rowView(_ view: RowView, open url: URL)
    func rowView(_ view: RowView, imageFor reference: String, into imageView: UIImageView)
}

/// A reusable transcript row: canvas + horizontal scrollers + native widgets.
final class RowView: UIView {
    private(set) var model: RowModel?
    private let canvas = RowCanvas()
    /// Freshly streamed text, fading in over the settled canvas (the veil).
    private let fresh = RowCanvas()
    private var scrollers: [UIScrollView] = []
    private var widgetViews: [UIView] = []
    private var textElement: UIAccessibilityElement?

    override var accessibilityElements: [Any]? {
        get {
            textElement?.accessibilityFrameInContainerSpace = bounds
            // Controls and links first: accessibility hit-testing takes the
            // first match, and the row's text element spans the whole row.
            return widgetViews.filter { $0 is UIControl || $0.gestureRecognizers?.isEmpty == false }
                + positionedLinkElements()
                + (textElement.map { [$0] } ?? [])
        }
        set {}
    }

    /// One element per link fragment (links are painted text, invisible to
    /// VoiceOver otherwise). Held here: accessibility doesn't retain them.
    private var linkElements: [(element: UIAccessibilityElement, hit: LinkHit)] = []

    private func rebuildLinkElements(_ d: RowDisplay) {
        linkElements = d.links.map { l in
            let e = UIAccessibilityElement(accessibilityContainer: self)
            e.accessibilityLabel = URL(string: l.url)?.host() ?? l.url
            e.accessibilityValue = l.url
            e.accessibilityTraits = .link
            e.accessibilityIdentifier = "transcript-link"
            return (e, l)
        }
    }

    /// Link elements where they're painted now (code/table scrollers move).
    private func positionedLinkElements() -> [UIAccessibilityElement] {
        linkElements.compactMap { e, l in
            var r = CGRect(x: CGFloat(l.x), y: CGFloat(l.y), width: CGFloat(l.w), height: CGFloat(l.h))
            if let s = l.scroller {
                guard Int(s) < scrollers.count else { return nil }
                let sv = scrollers[Int(s)]
                r = r.offsetBy(dx: sv.frame.minX - sv.contentOffset.x, dy: sv.frame.minY)
            }
            e.accessibilityFrameInContainerSpace = r
            return e
        }
    }
    weak var delegate: RowViewDelegate?
    /// Rings over thumbnails still uploading (`pending://` refs).
    private var uploadRings: [UploadRingView] = []
    var uploadProgress: Double? {
        didSet { if uploadProgress != oldValue { uploadRings.forEach { $0.progress = uploadProgress } } }
    }
    var key: UInt64 { model?.display.key ?? 0 }
    var version: UInt64 { model?.display.version ?? 0 }
    var kind: RowKind = .markdown

    override init(frame: CGRect) {
        super.init(frame: frame)
        addSubview(canvas)
        addSubview(fresh)
        fresh.isHidden = true
        clipsToBounds = false
    }

    required init?(coder: NSCoder) { fatalError() }

    func configure(_ model: RowModel, kind: RowKind) {
        // Same row grew (streaming append): veil the new text in.
        let previous = self.model?.display
        let grew = previous.map { $0.key == model.display.key && model.display.text.hasPrefix($0.text) && model.display.text.count > $0.text.count } ?? false
        let veilFrom = grew ? UInt32((previous?.text ?? "").utf16.count) : 0
        self.model = model
        self.kind = kind
        let d = model.display
        let bounds = CGRect(x: 0, y: 0, width: CGFloat(d.width), height: CGFloat(d.height))
        canvas.frame = bounds
        if grew, !UIAccessibility.isReduceMotionEnabled {
            canvas.pass = .settled(veilFrom: veilFrom)
            fresh.frame = bounds
            fresh.pass = .fresh(veilFrom: veilFrom)
            fresh.model = model
            fresh.isHidden = false
            fresh.layer.removeAllAnimations()
            fresh.alpha = 0
            UIView.animate(withDuration: 0.22, delay: 0, options: [.curveEaseOut, .allowUserInteraction]) {
                self.fresh.alpha = 1
            } completion: { [weak self] done in
                guard done, let self, self.model === model else { return }
                self.canvas.pass = .all
                self.canvas.setNeedsDisplay()
                self.fresh.isHidden = true
            }
        } else {
            canvas.pass = .all
            fresh.isHidden = true
        }
        canvas.model = model
        // Painted text is invisible to accessibility: one text element per
        // row, followed by the row's native widgets (copy, disclosure…).
        let element = UIAccessibilityElement(accessibilityContainer: self)
        element.accessibilityLabel = kind == .user ? "You: \(d.copyText)" : (d.copyText.isEmpty ? d.text : d.copyText)
        element.accessibilityIdentifier = switch kind {
        case .user: "row-user"
        case .markdown: "row-markdown"
        case .tools: "row-tools"
        case .chip: "row-chip"
        case .image: "row-image"
        case .working: "row-working"
        case .card: "row-card"
        }
        element.accessibilityTraits = .staticText
        element.accessibilityFrameInContainerSpace = bounds
        textElement = element
        rebuildLinkElements(d)
        // Scrollers: reuse views in order.
        while scrollers.count < d.scrollers.count {
            let s = FadingScrollView()
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
        uploadRings.removeAll()
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
                let b = DisclosureControl(expanded: expanded, chevron: false)
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
                iv.isUserInteractionEnabled = true
                iv.accessibilityTraits = .image
                iv.addGestureRecognizer(UITapGestureRecognizer(target: self, action: #selector(openImage(_:))))
                delegate?.rowView(self, imageFor: reference, into: iv)
                if reference.hasPrefix("pending://") {
                    // Size the thumbnail first: autoresizing from a zero frame
                    // would double the ring when the widget frame lands.
                    iv.frame = rect
                    let ring = UploadRingView(frame: CGRect(origin: .zero, size: rect.size))
                    ring.autoresizingMask = [.flexibleWidth, .flexibleHeight]
                    ring.progress = uploadProgress
                    iv.addSubview(ring)
                    uploadRings.append(ring)
                }
                view = iv
            case .spinner:
                view = StatusGlyph(.spinner)
            case let .working(sinceMs, streaming):
                view = WorkingIndicatorView(since: sinceMs.map { Date(timeIntervalSince1970: Double($0) / 1000) }, streaming: streaming)
            case let .detail(title):
                let b = UIControl()
                b.accessibilityLabel = "\(title) details"
                b.accessibilityTraits = .button
                let payload = w.payload ?? ""
                b.addAction(UIAction { [weak self] _ in
                    guard let vc = self?.findViewController() else { return }
                    let sheet = UINavigationController(rootViewController: SelectTextViewController(text: payload, title: title, mono: true))
                    if let s = sheet.sheetPresentationController { s.detents = [.medium(), .large()]; s.prefersGrabberVisible = true }
                    vc.present(sheet, animated: true)
                }, for: .touchUpInside)
                view = b
            case let .icon(name, color):
                // Desktop icon assets (tool-*, fileicon-*), else an SF symbol.
                let iv: UIImageView
                if let asset = RowView.asset(name) {
                    iv = UIImageView(image: asset)
                    iv.contentMode = .scaleAspectFit
                } else {
                    iv = UIImageView(image: UIImage(systemName: name, withConfiguration: UIImage.SymbolConfiguration(pointSize: rect.height * 0.8, weight: .medium)))
                    iv.contentMode = .center
                }
                iv.tintColor = Palette.color(color)
                view = iv
            case let .chevron(expanded):
                view = ToolChevronView(key: d.key, expanded: expanded)
            case let .toolRail(trunkX, bend, branchEnd, rowMid, tops, heights):
                let live = d.widgets.contains { if case .shimmer = $0.kind { return true } else { return false } }
                view = ToolRailView(key: d.key, trunkX: trunkX, bend: bend, branchEnd: branchEnd, rowMid: rowMid, tops: tops, heights: heights, rowWidth: CGFloat(d.width) - rect.minX, live: live)
            case let .toolToggle(detail, open):
                let b = ToolToggleControl()
                b.accessibilityTraits = .button
                b.accessibilityLabel = open ? "Hide details" : "Show details"
                b.addAction(UIAction { [weak self] _ in
                    guard let self else { return }
                    UISelectionFeedbackGenerator().selectionChanged()
                    self.delegate?.rowView(self, toggleDetail: detail, open: open)
                }, for: .touchUpInside)
                view = b
            case let .action(label):
                // A card control (goal / todo / workflow): Rust decides what the payload means.
                let b = UIControl()
                b.accessibilityLabel = label
                b.accessibilityTraits = .button
                let payload = w.payload ?? ""
                b.addAction(UIAction { [weak self] _ in
                    guard let self else { return }
                    self.delegate?.rowView(self, action: payload)
                }, for: .touchUpInside)
                view = b
            case .shimmer:
                view = ShimmerView(model: model!, rect: rect)
            }
            view.frame = rect
            host.addSubview(view)
            widgetViews.append(view)
        }
    }

    private static var assets: [String: UIImage?] = [:]

    static func asset(_ name: String) -> UIImage? {
        if let hit = assets[name] { return hit }
        let image = (name.hasPrefix("tool-") || name.hasPrefix("fileicon-")) ? UIImage(named: name) : nil
        assets[name] = image
        return image
    }

    @objc private func openImage(_ tap: UITapGestureRecognizer) {
        guard let iv = tap.view as? UIImageView, let image = iv.image, let vc = findViewController() else { return }
        vc.present(ImageViewer(image: image, source: iv), animated: true)
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

/// Tool state glyph: running mini spinner, failed cross, or a quiet check.
final class ToolStatusView: UIView {
    init(running: Bool, failed: Bool) {
        super.init(frame: .zero)
        isUserInteractionEnabled = false
        if running {
            let grid = StatusGlyph(.spinner)
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

/// Tail-of-turn indicator, as the desktop transcript trailer: the 3×3
/// gradient spinner + "Working…" in muted text, elapsed time fainter.
final class WorkingIndicatorView: UIView {
    private let grid = StatusGlyph(.trailer)
    private let label = UILabel()
    private let since: Date?
    private let word: String
    private var timer: Timer?

    init(since: Date?, streaming: Bool) {
        self.since = since
        self.word = streaming ? "Writing" : "Working"
        super.init(frame: .zero)
        isUserInteractionEnabled = false
        label.font = Fonts.ui(.sansMedium, UIFontMetrics(forTextStyle: .body).scaledValue(for: 13.5))
        label.textColor = Palette.secondary
        addSubview(grid)
        addSubview(label)
        tick()
    }

    required init?(coder: NSCoder) { fatalError() }

    private func tick() {
        let secs = since.map { max(0, Int(Date().timeIntervalSince($0))) } ?? 0
        let text = NSMutableAttributedString(string: "\(word)…", attributes: [.font: label.font as Any, .foregroundColor: Palette.secondary])
        if secs > 0 {
            text.append(NSAttributedString(string: "  \(StatusPill.elapsed(secs))", attributes: [.font: label.font as Any, .foregroundColor: Palette.tertiary]))
        }
        label.attributedText = text
        label.sizeToFit()
        setNeedsLayout()
    }

    override func didMoveToWindow() {
        super.didMoveToWindow()
        timer?.invalidate()
        guard window != nil else { return }
        tick()
        timer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in self?.tick() }
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        grid.frame = CGRect(x: 0, y: (bounds.height - 14) / 2, width: 14, height: 14)
        label.frame = CGRect(x: 22, y: (bounds.height - label.bounds.height) / 2, width: label.bounds.width, height: label.bounds.height)
    }
}

/// Upload progress over a pending thumbnail: a light scrim, a ring that fills
/// with the transfer, and the percentage. Fades away when the upload ends.
final class UploadRingView: UIView {
    private let track = CAShapeLayer()
    private let arc = CAShapeLayer()
    private let label = UILabel()

    var progress: Double? {
        didSet {
            guard let p = progress else {
                UIView.animate(withDuration: 0.25) { self.alpha = 0 }
                return
            }
            if alpha < 1 { UIView.animate(withDuration: 0.2) { self.alpha = 1 } }
            arc.strokeEnd = CGFloat(max(0.02, min(1, p)))
            label.text = "\(Int((min(1, max(0, p)) * 100).rounded()))%"
            accessibilityValue = label.text
        }
    }

    override init(frame: CGRect) {
        super.init(frame: frame)
        isUserInteractionEnabled = false
        backgroundColor = UIColor.black.withAlphaComponent(0.38)
        alpha = 0
        for l in [track, arc] {
            l.fillColor = nil
            l.lineWidth = 3
            l.lineCap = .round
            layer.addSublayer(l)
        }
        track.strokeColor = UIColor.white.withAlphaComponent(0.3).cgColor
        arc.strokeColor = UIColor.white.cgColor
        arc.strokeEnd = 0
        label.font = Fonts.ui(.sansSemibold, 11)
        label.textColor = .white
        label.textAlignment = .center
        addSubview(label)
        isAccessibilityElement = true
        accessibilityLabel = "Uploading"
        accessibilityIdentifier = "upload-progress"
    }

    required init?(coder: NSCoder) { fatalError() }

    override func layoutSubviews() {
        super.layoutSubviews()
        let d = min(34, min(bounds.width, bounds.height) * 0.55)
        let r = CGRect(x: bounds.midX - d / 2, y: bounds.midY - d / 2, width: d, height: d)
        let path = UIBezierPath(arcCenter: CGPoint(x: r.midX, y: r.midY), radius: d / 2, startAngle: -.pi / 2, endAngle: 1.5 * .pi, clockwise: true).cgPath
        track.path = path
        arc.path = path
        label.frame = r
        label.font = Fonts.ui(.sansSemibold, d < 30 ? 9 : 10.5)
    }
}
