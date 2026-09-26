import SafariServices
import UIKit

/// Virtualized transcript host. Rust owns geometry: each `LayoutFrame` gives
/// exact row offsets, so this view only (1) asks which rows intersect the
/// viewport, (2) positions reusable `RowView`s there, and (3) keeps the user's
/// place across frames (anchor or follow-the-tail).
final class TranscriptListView: UIScrollView, RowViewDelegate {
    let engine: TranscriptView
    let fonts = StyleFonts()
    private(set) var current: LayoutFrame?
    private var visible: [UInt64: RowView] = [:]
    private var pool: [RowView] = []
    private var cache: [ModelKey: RowModel] = [:]
    private var cacheOrder: [ModelKey] = []
    private var knownKeys = Set<UInt64>()
    private var inflight = Set<ModelKey>()
    private let prefetchQueue = DispatchQueue(label: "sh.zeron.transcript.prefetch", qos: .userInitiated)

    /// Rows are realized this far beyond the viewport (and prefetched further).
    var overscan: CGFloat = 700

    /// Following the tail: new content keeps the bottom in view.
    private(set) var following = true
    var onFollowChange: ((Bool) -> Void)?
    var onDistanceFromBottom: ((CGFloat) -> Void)?
    var imageLoader: ((String, UIImageView) -> Void)?

    private var spring: CADisplayLink?
    private var lastSpringTick: CFTimeInterval = 0
    private var viewportWidth: CGFloat = 0
    private var textScale: CGFloat = 1

    private struct ModelKey: Hashable {
        let key: UInt64
        let version: UInt64
        let width: Float
    }

    init(engine: TranscriptView) {
        self.engine = engine
        super.init(frame: .zero)
        alwaysBounceVertical = true
        keyboardDismissMode = .interactive
        contentInsetAdjustmentBehavior = .always
        showsVerticalScrollIndicator = true
        backgroundColor = .clear
        let tap = UITapGestureRecognizer(target: self, action: #selector(tapped(_:)))
        tap.cancelsTouchesInView = false
        addGestureRecognizer(tap)
        addInteraction(UIContextMenuInteraction(delegate: self))
        panGestureRecognizer.addTarget(self, action: #selector(panned(_:)))
    }

    required init?(coder: NSCoder) { fatalError() }

    // MARK: Viewport

    override func layoutSubviews() {
        super.layoutSubviews()
        let scale = UIFontMetrics(forTextStyle: .body).scaledValue(for: 17) / 17
        if bounds.width != viewportWidth || scale != textScale {
            viewportWidth = bounds.width
            textScale = scale
            engine.setViewport(width: Float(bounds.width), textScale: Float(scale))
        }
        layoutRows()
        reportDistance()
    }

    var maxOffsetY: CGFloat {
        max(-adjustedContentInset.top, contentSize.height + adjustedContentInset.bottom - bounds.height)
    }

    var distanceFromBottom: CGFloat { maxOffsetY - contentOffset.y }

    private func reportDistance() {
        onDistanceFromBottom?(distanceFromBottom)
        if (isDragging || isDecelerating), !following, distanceFromBottom < 70 {
            setFollowing(true)
        }
    }

    private func setFollowing(_ value: Bool) {
        guard following != value else { return }
        following = value
        onFollowChange?(value)
        if !value { stopSpring() }
    }

    /// A drag hands control to the user; `reportDistance` re-engages follow
    /// once they bring the tail back within 70pt.
    @objc private func panned(_ pan: UIPanGestureRecognizer) {
        if pan.state == .began { setFollowing(false) }
    }

    /// Jump (or glide) to the newest content and resume following.
    func scrollToBottom(animated: Bool) {
        setFollowing(true)
        if animated {
            startSpring()
        } else {
            stopSpring()
            contentOffset.y = maxOffsetY
        }
    }

    /// The system scroll-edge effect only engages after an on-screen scroll;
    /// the first frame positions us mid-transition, so settle once visible.
    func settleEdgeEffect() {
        guard current != nil else { return }
        let target = following ? maxOffsetY : contentOffset.y
        contentOffset.y = target - 1
        contentOffset.y = target
    }

    // MARK: Frames

    func apply(_ frame: LayoutFrame) {
        if frame.styleCount() != fonts.count { fonts.update(frame.styles()) }
        let first = current == nil || current?.rowCount() == 0
        var anchor: (key: UInt64, delta: CGFloat)?
        if !following, let old = current {
            let y = contentOffset.y + adjustedContentInset.top
            if let i = old.indexAt(y: Float(max(0, y))), let p = old.placement(index: i) {
                anchor = (p.key, contentOffset.y - CGFloat(p.y))
            }
        }
        current = frame
        let height = CGFloat(frame.totalHeight())
        if contentSize.height != height || contentSize.width != bounds.width {
            contentSize = CGSize(width: bounds.width, height: height)
        }
        if first {
            for i in 0..<frame.rowCount() { if let p = frame.placement(index: i) { knownKeys.insert(p.key) } }
            contentOffset.y = maxOffsetY
        } else if following {
            if !isTracking { startSpring() }
        } else if let anchor, let i = frame.indexOf(key: anchor.key), let p = frame.placement(index: i) {
            let target = CGFloat(p.y) + anchor.delta
            if abs(target - contentOffset.y) > 0.5 {
                // Shift bounds (not contentOffset) so momentum survives.
                bounds.origin.y = target
            }
        }
        layoutRows()
    }

    private func layoutRows() {
        guard let frame = current else { return }
        let y0 = contentOffset.y - overscan
        let y1 = contentOffset.y + bounds.height + overscan
        let placements = frame.rowsIn(y0: Float(y0), y1: Float(y1))
        var seen = Set<UInt64>(minimumCapacity: placements.count)
        let width = frame.width()
        for p in placements {
            seen.insert(p.key)
            let view: RowView
            if let v = visible[p.key] {
                view = v
            } else {
                view = pool.popLast() ?? RowView()
                view.delegate = self
                visible[p.key] = view
                if view.superview !== self { addSubview(view) }
                view.isHidden = false
            }
            if view.model == nil || view.version != p.version || view.model?.display.width != width || view.key != p.key {
                view.configure(model(for: p, frame: frame), kind: p.kind)
                if !knownKeys.contains(p.key) {
                    knownKeys.insert(p.key)
                    view.alpha = 0
                    UIView.animate(withDuration: 0.28, delay: 0, options: [.curveEaseOut, .allowUserInteraction]) { view.alpha = 1 }
                }
            }
            let rect = CGRect(x: 0, y: CGFloat(p.y), width: bounds.width, height: CGFloat(p.height))
            if view.frame != rect { view.frame = rect }
        }
        for (key, view) in visible where !seen.contains(key) {
            visible[key] = nil
            view.isHidden = true
            view.layer.removeAllAnimations()
            view.alpha = 1
            pool.append(view)
        }
        prefetch(frame: frame, around: y0...y1)
    }

    private func model(for p: RowPlacement, frame: LayoutFrame) -> RowModel {
        let k = ModelKey(key: p.key, version: p.version, width: frame.width())
        if let m = cache[k] { return m }
        let display = frame.display(index: p.index)!
        let m = RowModel(display: display, fonts: fonts)
        store(m, for: k)
        return m
    }

    private func store(_ m: RowModel, for k: ModelKey) {
        if cache[k] == nil { cacheOrder.append(k) }
        cache[k] = m
        if cacheOrder.count > 500 {
            for old in cacheOrder.prefix(100) { cache[old] = nil }
            cacheOrder.removeFirst(100)
        }
    }

    /// Build display models for rows just beyond the realized band off-main,
    /// in the direction of travel first.
    private func prefetch(frame: LayoutFrame, around band: ClosedRange<CGFloat>) {
        let ahead = bounds.height * 1.5
        let down = panGestureRecognizer.velocity(in: self).y <= 0
        let ranges = down
            ? [(band.upperBound, band.upperBound + ahead), (band.lowerBound - ahead / 2, band.lowerBound)]
            : [(band.lowerBound - ahead, band.lowerBound), (band.upperBound, band.upperBound + ahead / 2)]
        var todo: [RowPlacement] = []
        for (a, b) in ranges where b > 0 {
            for p in frame.rowsIn(y0: Float(a), y1: Float(b)) {
                let k = ModelKey(key: p.key, version: p.version, width: frame.width())
                if cache[k] == nil, !inflight.contains(k) {
                    inflight.insert(k)
                    todo.append(p)
                }
            }
        }
        guard !todo.isEmpty else { return }
        let fonts = self.fonts
        prefetchQueue.async { [weak self] in
            let built = todo.compactMap { p -> (ModelKey, RowModel)? in
                guard let d = frame.display(index: p.index) else { return nil }
                return (ModelKey(key: p.key, version: p.version, width: frame.width()), RowModel(display: d, fonts: fonts))
            }
            DispatchQueue.main.async {
                guard let self else { return }
                for (k, m) in built {
                    self.inflight.remove(k)
                    self.store(m, for: k)
                }
            }
        }
    }

    // MARK: Follow spring

    private func startSpring() {
        guard spring == nil else { return }
        let link = CADisplayLink(target: self, selector: #selector(springTick(_:)))
        link.preferredFrameRateRange = CAFrameRateRange(minimum: 60, maximum: 120, preferred: 120)
        link.add(to: .main, forMode: .common)
        lastSpringTick = CACurrentMediaTime()
        spring = link
    }

    private func stopSpring() {
        spring?.invalidate()
        spring = nil
    }

    @objc private func springTick(_ link: CADisplayLink) {
        let now = link.targetTimestamp
        let dt = min(1 / 30, max(0, now - lastSpringTick))
        lastSpringTick = now
        guard following, !isTracking else { return stopSpring() }
        let target = maxOffsetY
        let delta = target - contentOffset.y
        if abs(delta) < 0.5 {
            contentOffset.y = target
            return stopSpring()
        }
        // Critically damped approach: feels like the tail "settles" into place.
        contentOffset.y += delta * CGFloat(1 - exp(-dt * 16))
    }

    // MARK: Interaction

    @objc private func tapped(_ tap: UITapGestureRecognizer) {
        let point = tap.location(in: self)
        guard let row = visible.values.first(where: { !$0.isHidden && $0.frame.contains(point) }),
              let url = row.link(at: tap.location(in: row))
        else { return }
        rowView(row, open: url)
    }

    func rowView(_ view: RowView, toggle key: UInt64) {
        UISelectionFeedbackGenerator().selectionChanged()
        engine.toggle(key: key)
    }

    func rowView(_ view: RowView, open url: URL) {
        guard let vc = findViewController() else { return }
        if url.scheme == "http" || url.scheme == "https" {
            let safari = SFSafariViewController(url: url)
            safari.preferredControlTintColor = Palette.accent
            vc.present(safari, animated: true)
        } else {
            UIApplication.shared.open(url)
        }
    }

    func rowView(_ view: RowView, imageFor reference: String, into imageView: UIImageView) {
        imageLoader?(reference, imageView)
    }

    fileprivate func row(at point: CGPoint) -> RowView? {
        visible.values.first { !$0.isHidden && $0.frame.contains(point) }
    }
}

extension TranscriptListView: UIContextMenuInteractionDelegate {
    func contextMenuInteraction(_ interaction: UIContextMenuInteraction, configurationForMenuAtLocation location: CGPoint) -> UIContextMenuConfiguration? {
        guard let row = row(at: location), let model = row.model, let frame = current,
              let index = frame.indexOf(key: model.display.key)
        else { return nil }
        let block = model.display.copyText
        return UIContextMenuConfiguration(identifier: NSNumber(value: model.display.key), previewProvider: nil) { _ in
            var actions: [UIMenuElement] = []
            if !block.isEmpty {
                actions.append(UIAction(title: "Copy", image: UIImage(systemName: "doc.on.doc")) { _ in
                    UIPasteboard.general.string = block
                })
            }
            if let message = frame.messageText(index: index), !message.isEmpty, message != block {
                actions.append(UIAction(title: "Copy Message", image: UIImage(systemName: "doc.on.clipboard")) { _ in
                    UIPasteboard.general.string = message
                })
            }
            let selectable = frame.messageText(index: index) ?? block
            if !selectable.isEmpty {
                actions.append(UIAction(title: "Select Text", image: UIImage(systemName: "character.cursor.ibeam")) { [weak self] _ in
                    self?.presentSelection(selectable)
                })
            }
            return actions.isEmpty ? nil : UIMenu(children: actions)
        }
    }

    func contextMenuInteraction(_ interaction: UIContextMenuInteraction, previewForHighlightingMenuWithConfiguration configuration: UIContextMenuConfiguration) -> UITargetedPreview? {
        guard let key = (configuration.identifier as? NSNumber)?.uint64Value, let row = visible[key] else { return nil }
        let params = UIPreviewParameters()
        params.backgroundColor = Palette.background
        params.visiblePath = UIBezierPath(roundedRect: row.bounds.insetBy(dx: 8, dy: 0), cornerRadius: 16)
        return UITargetedPreview(view: row, parameters: params)
    }

    private func presentSelection(_ text: String) {
        guard let vc = findViewController() else { return }
        let sheet = SelectTextViewController(text: text)
        vc.present(UINavigationController(rootViewController: sheet), animated: true)
    }
}

extension UIView {
    func findViewController() -> UIViewController? {
        var r: UIResponder? = self
        while let next = r?.next {
            if let vc = next as? UIViewController { return vc }
            r = next
        }
        return nil
    }
}

/// Full-text selection for a message (the transcript itself is paint-only).
final class SelectTextViewController: UIViewController {
    private let text: String

    init(text: String) {
        self.text = text
        super.init(nibName: nil, bundle: nil)
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        title = "Select Text"
        view.backgroundColor = Palette.background
        let tv = UITextView()
        tv.text = text
        tv.isEditable = false
        tv.font = Fonts.ui(.sans, UIFontMetrics(forTextStyle: .body).scaledValue(for: 16.5))
        tv.textColor = Palette.text
        tv.backgroundColor = .clear
        tv.textContainerInset = UIEdgeInsets(top: 16, left: 14, bottom: 32, right: 14)
        tv.frame = view.bounds
        tv.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        view.addSubview(tv)
        navigationItem.rightBarButtonItem = UIBarButtonItem(systemItem: .done, primaryAction: UIAction { [weak self] _ in
            self?.dismiss(animated: true)
        })
        DispatchQueue.main.async { tv.selectAll(nil) }
    }
}
