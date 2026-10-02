import SafariServices
import UIKit

/// Virtualized transcript host. Rust owns geometry: each `LayoutFrame` gives
/// exact row offsets, so this view only (1) asks which rows intersect the
/// viewport, (2) positions reusable `RowView`s there, and (3) keeps the user's
/// place across frames (anchor or follow-the-tail).
final class TranscriptListView: UIScrollView, RowViewDelegate, UIScrollViewDelegate {
    let engine: TranscriptView
    let fonts = StyleFonts()
    private(set) var current: LayoutFrame?
    private var visible: [UInt64: RowView] = [:]
    private var pool: [RowView] = []
    private var cache: [ModelKey: RowModel] = [:]
    private var memoryObserver: NSObjectProtocol?

    deinit {
        if let memoryObserver { NotificationCenter.default.removeObserver(memoryObserver) }
    }
    private var cacheOrder: [ModelKey] = []
    private var knownKeys = Set<UInt64>()
    private var inflight = Set<ModelKey>()
    private let prefetchQueue = DispatchQueue(label: "sh.zeron.transcript.prefetch", qos: .userInitiated)

    /// Rows are realized this far beyond the viewport (and prefetched further).
    var overscan: CGFloat = 700

    /// Attachment upload progress: rings on pending thumbnails.
    var uploadProgress: Double? {
        didSet {
            guard uploadProgress != oldValue else { return }
            for v in visible.values { v.uploadProgress = uploadProgress }
        }
    }

    /// Following the tail: new content keeps the bottom in view.
    private(set) var following = true
    var onFollowChange: ((Bool) -> Void)?
    var onDistanceFromBottom: ((CGFloat) -> Void)?
    var imageLoader: ((String, UIImageView) -> Void)?

    // MARK: Runway (desktop transcript.rs `OwnTurnAnchor`)
    //
    // On an immediate send the prompt glides to the top of the viewport and
    // the content is held at least one viewport tall below it, so the reply
    // streams into reserved space without the view moving on every token.
    // The reply consumes the reservation; once it fills it, the runway retires
    // and the normal follow-the-tail spring takes over. A drag releases the
    // hold (the reservation stays as plain scroll room); coming back to the
    // bottom — or Jump to latest — glides back to the held position.
    private struct OwnTurn {
        /// The prompt row, once its (optimistic) echo is laid out.
        var key: UInt64?
        /// User rows that existed at send time (the new one is the prompt).
        var before: Set<UInt64>
    }
    private var ownTurn: OwnTurn?
    /// Content height the runway holds (nil: the frame's own height).
    private var runwayHeight: CGFloat?
    /// Desktop OWN_SEND_GLIDE_RETAIN: 15% of the remaining glide per 60 fps
    /// frame (~90% in ~230 ms), ease-out.
    private static let glideRetain: Double = 0.85

    /// User rows near the tail right now (a new one after this is ours).
    private func recentUserKeys() -> Set<UInt64> {
        var keys = Set<UInt64>()
        if let frame = current {
            let n = frame.rowCount()
            for i in stride(from: Int(n) - 1, through: max(0, Int(n) - 24), by: -1) {
                if let p = frame.placement(index: UInt32(i)), p.kind == .user { keys.insert(p.key) }
            }
        }
        return keys
    }

    /// Reserve the reply's space below the prompt about to be sent (every
    /// immediate send; queued sends keep the live turn's runway).
    func beginOwnTurn() {
        queuedTurn = nil
        ownTurn = OwnTurn(key: nil, before: recentUserKeys())
        setFollowing(true)
    }

    /// A message queued behind the live turn: it gets the runway once its
    /// bubble materializes in the transcript (desktop
    /// `promote_materialized_queued_turn`), not before — the live turn keeps
    /// its own until then.
    func expectQueuedTurn() {
        if queuedTurn == nil { queuedTurn = recentUserKeys() }
    }
    private var queuedTurn: Set<UInt64>?

    /// Resolve the prompt row and the height the runway holds for `frame`.
    private func contentHeight(for frame: LayoutFrame) -> CGFloat {
        let natural = CGFloat(frame.totalHeight())
        if let before = queuedTurn {
            let n = Int(frame.rowCount())
            for i in stride(from: n - 1, through: max(0, n - 12), by: -1) {
                if let p = frame.placement(index: UInt32(i)), p.kind == .user, !before.contains(p.key) {
                    // The queued prompt landed: promote it to own the runway.
                    queuedTurn = nil
                    ownTurn = OwnTurn(key: p.key, before: before)
                    setFollowing(true)
                    break
                }
            }
        }
        guard var turn = ownTurn else {
            runwayHeight = nil
            return natural
        }
        if turn.key == nil {
            let n = Int(frame.rowCount())
            for i in stride(from: n - 1, through: max(0, n - 12), by: -1) {
                if let p = frame.placement(index: UInt32(i)), p.kind == .user, !turn.before.contains(p.key) {
                    turn.key = p.key
                    break
                }
            }
            ownTurn = turn
        }
        guard let key = turn.key, let i = frame.indexOf(key: key), let p = frame.placement(index: i) else {
            // Echo not laid out yet: hold still (desktop returns early too).
            return max(natural, runwayHeight ?? 0)
        }
        // Hold offset puts the prompt row (its turn gap included) at the top of
        // the visible area; the reservation makes that the max offset.
        let hold = CGFloat(p.y) - adjustedContentInset.top
        let minimum = hold + bounds.height - adjustedContentInset.bottom
        if natural >= minimum - 0.5 {
            // Filled: the ordinary tail follow takes over. The anchor stays for
            // the turn, so if the viewport grows past the reply again (the
            // keyboard going away) the space is reserved again instead of the
            // prompt dropping back down.
            runwayHeight = nil
            return natural
        }
        runwayHeight = minimum
        return minimum
    }

    /// The composer's inset changes with the keyboard: resize the runway in
    /// the same pass, so "the bottom" stays the held position (otherwise the
    /// prompt dipped and glided back).
    override var contentInset: UIEdgeInsets {
        didSet { if contentInset != oldValue { refreshRunway() } }
    }

    /// Insets / viewport changed (keyboard, rotation): re-size the runway.
    private func refreshRunway() {
        guard ownTurn != nil, let frame = current else { return }
        let height = contentHeight(for: frame)
        if abs(contentSize.height - height) > 0.5 {
            contentSize = CGSize(width: bounds.width, height: height)
            if following, !isTracking { startSpring() }
        }
    }

    /// A disclosure the user just toggled: its next height change tweens with
    /// the desktop fold (140 ms ease-out).
    private var pendingFold: UInt64?
    /// Row frames animate during this apply (fold / tool-row arrival).
    private var frameAnimation: UIViewPropertyAnimator?

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
        delegate = self
        // Display models are a pure cache (rebuilt from the frame on demand).
        memoryObserver = NotificationCenter.default.addObserver(forName: UIApplication.didReceiveMemoryWarningNotification, object: nil, queue: .main) { [weak self] _ in
            self?.cache.removeAll()
            self?.cacheOrder.removeAll()
            self?.pool.forEach { $0.removeFromSuperview() }
            self?.pool.removeAll()
        }
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
        refreshRunway()
        reportDistance()
    }

    var maxOffsetY: CGFloat {
        max(-adjustedContentInset.top, contentSize.height + adjustedContentInset.bottom - bounds.height)
    }

    var distanceFromBottom: CGFloat { maxOffsetY - contentOffset.y }

    private var lastReportedY: CGFloat = 0

    /// Follow re-engages only when the user *returns* to the tail: momentum
    /// carrying the list down into the last 70pt. Never while the finger is
    /// down — a drag that starts at the bottom would otherwise re-latch
    /// immediately and spring back down on release.
    private func reportDistance() {
        let distance = distanceFromBottom
        onDistanceFromBottom?(distance)
        let movingDown = contentOffset.y > lastReportedY + 0.1
        lastReportedY = contentOffset.y
        if !following, !isTracking, isDecelerating, movingDown || distance <= 0, distance < 70 {
            setFollowing(true)
        }
    }

    private func setFollowing(_ value: Bool) {
        guard following != value else { return }
        following = value
        onFollowChange?(value)
        if !value { stopSpring() }
    }

    /// A drag hands control to the user. Releasing within 70pt of the tail
    /// while not flinging away from it hands control back.
    @objc private func panned(_ pan: UIPanGestureRecognizer) {
        switch pan.state {
        case .began:
            setFollowing(false)
        case .ended, .cancelled:
            // Finger moving up (negative y) scrolls toward the tail.
            if distanceFromBottom < 70, pan.velocity(in: self).y <= 50 { setFollowing(true) }
        default:
            break
        }
    }

    /// Status-bar tap: the user is going to the top, not the tail.
    func scrollViewShouldScrollToTop(_ scrollView: UIScrollView) -> Bool {
        setFollowing(false)
        return true
    }

    /// Stop following the tail (programmatic scrolling, benchmarks).
    func releaseFollow() { setFollowing(false) }

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
        let state = Signposts.transcript.beginInterval("apply-frame")
        let t0 = CACurrentMediaTime()
        defer {
            Signposts.transcript.endInterval("apply-frame", state)
            TranscriptPerf.maxApplyMs = max(TranscriptPerf.maxApplyMs, (CACurrentMediaTime() - t0) * 1000)
        }
        if frame.styleCount() != fonts.count { fonts.update(frame.styles()) }
        let first = current == nil || current?.rowCount() == 0
        var anchor: (key: UInt64, delta: CGFloat)?
        if !following, let old = current {
            let y = contentOffset.y + adjustedContentInset.top
            if let i = old.indexAt(y: Float(max(0, y))), let p = old.placement(index: i) {
                anchor = (p.key, contentOffset.y - CGFloat(p.y))
            }
        }
        let old = current
        current = frame
        if old == nil || old?.rowCount() == 0 {
            // Groups present when the transcript attaches never animate.
            ToolRailView.quietUntil = CACurrentMediaTime() + 0.8
        }
        let animation = rowAnimation(old: old, new: frame)
        let height = contentHeight(for: frame)
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
        layoutRows(animation: animation)
    }

    /// Fold toggles tween 140 ms ease-out; a visible tool group that grew
    /// (a new call arrived) reveals over 360 ms expo — desktop timings.
    private func rowAnimation(old: LayoutFrame?, new: LayoutFrame) -> UIViewPropertyAnimator? {
        guard let old, !UIAccessibility.isReduceMotionEnabled else { return nil }
        func height(_ f: LayoutFrame, _ key: UInt64) -> Float? {
            f.indexOf(key: key).flatMap { f.placement(index: $0)?.height }
        }
        if let key = pendingFold, let a = height(old, key), let b = height(new, key), a != b {
            pendingFold = nil
            return UIViewPropertyAnimator(duration: Motion.fold, curve: .easeOut)
        }
        let grew = visible.contains { key, view in
            view.kind == .tools && (height(new, key) ?? 0) > (height(old, key) ?? .greatestFiniteMagnitude)
        }
        return grew ? UIViewPropertyAnimator(duration: Motion.rowReveal, timingParameters: Motion.expo) : nil
    }

    private func layoutRows(animation: UIViewPropertyAnimator? = nil) {
        guard let frame = current else { return }
        var moves: [(RowView, CGRect)] = []
        let state = Signposts.transcript.beginInterval("layout-rows")
        let t0 = CACurrentMediaTime()
        defer {
            Signposts.transcript.endInterval("layout-rows", state)
            TranscriptPerf.maxLayoutMs = max(TranscriptPerf.maxLayoutMs, (CACurrentMediaTime() - t0) * 1000)
        }
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
                view = pool.popLast() ?? { TranscriptPerf.viewsCreated += 1; return RowView() }()
                view.delegate = self
                visible[p.key] = view
                if view.superview !== self { addSubview(view) }
                view.isHidden = false
            }
            let stale = view.model == nil || view.version != p.version || view.model?.display.width != width || view.key != p.key
            if stale, view.key == p.key, view.model?.display.width == width,
               cache[ModelKey(key: p.key, version: p.version, width: width)] == nil {
                // Same row, new content (a streaming tail): keep painting the
                // current model and build the new one off the main thread.
                upgradeAsync(p, frame: frame)
            } else if stale {
                view.configure(model(for: p, frame: frame), kind: p.kind)
                view.uploadProgress = uploadProgress
                if !knownKeys.contains(p.key) {
                    knownKeys.insert(p.key)
                    view.alpha = 0
                    UIView.animate(withDuration: 0.28, delay: 0, options: [.curveEaseOut, .allowUserInteraction]) { view.alpha = 1 }
                }
            }
            let rect = CGRect(x: 0, y: CGFloat(p.y), width: bounds.width, height: CGFloat(p.height))
            if view.frame != rect {
                // Existing rows tween with the animation; new ones just land.
                if animation != nil, !view.frame.isEmpty, !stale || view.key == p.key {
                    moves.append((view, rect))
                } else {
                    view.frame = rect
                }
            }
        }
        for (key, view) in visible where !seen.contains(key) {
            visible[key] = nil
            view.isHidden = true
            view.layer.removeAllAnimations()
            view.alpha = 1
            pool.append(view)
        }
        if let animation, !moves.isEmpty {
            for (view, _) in moves where view.kind == .tools { view.clipsToBounds = true }
            animation.addAnimations { for (view, rect) in moves { view.frame = rect } }
            animation.addCompletion { _ in for (view, _) in moves { view.clipsToBounds = false } }
            animation.startAnimation()
        } else {
            for (view, rect) in moves { view.frame = rect }
        }
        prefetch(frame: frame, around: y0...y1)
    }

    private func model(for p: RowPlacement, frame: LayoutFrame) -> RowModel {
        let k = ModelKey(key: p.key, version: p.version, width: frame.width())
        if let m = cache[k] { return m }
        let state = Signposts.transcript.beginInterval("build-model-sync")
        let t0 = CACurrentMediaTime()
        TranscriptPerf.syncBuilds += 1
        defer {
            Signposts.transcript.endInterval("build-model-sync", state)
            TranscriptPerf.maxSyncBuildMs = max(TranscriptPerf.maxSyncBuildMs, (CACurrentMediaTime() - t0) * 1000)
        }
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

    private func upgradeAsync(_ p: RowPlacement, frame: LayoutFrame) {
        let k = ModelKey(key: p.key, version: p.version, width: frame.width())
        guard !inflight.contains(k) else { return }
        inflight.insert(k)
        let fonts = self.fonts
        prefetchQueue.async { [weak self] in
            guard let d = frame.display(index: p.index) else { return }
            let model = RowModel(display: d, fonts: fonts)
            TranscriptPerf.asyncBuilds += 1
            DispatchQueue.main.async {
                guard let self else { return }
                self.inflight.remove(k)
                self.store(model, for: k)
                // Apply only if this is still the newest version on screen.
                if let view = self.visible[p.key], let current = self.current,
                   let i = current.indexOf(key: p.key), current.placement(index: i)?.version == p.version {
                    view.configure(model, kind: p.kind)
                    view.uploadProgress = self.uploadProgress
                }
            }
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
        if runwayHeight != nil {
            // Runway glide: the desktop's frame-rate-independent ease-out.
            let frames = min(8, dt / (1.0 / 60))
            contentOffset.y += delta * CGFloat(1 - pow(Self.glideRetain, frames))
        } else {
            // Critically damped approach: feels like the tail "settles" into place.
            contentOffset.y += delta * CGFloat(1 - exp(-dt * 16))
        }
    }

    /// The first user message bubble currently laid out (handoff target).
    func firstUserBubble(in target: UIView) -> CGRect? {
        let rows = visible.values.filter { $0.kind == .user && !$0.isHidden }.sorted { $0.frame.minY < $1.frame.minY }
        for row in rows {
            guard let d = row.model?.display,
                  let box = d.boxes.first(where: { $0.color == .userBubble && $0.scroller == nil })
            else { continue }
            let r = CGRect(x: CGFloat(box.x), y: CGFloat(box.y), width: CGFloat(box.w), height: CGFloat(box.h))
            return row.convert(r, to: target)
        }
        return nil
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
        pendingFold = key
        engine.toggle(key: key)
    }

    func rowView(_ view: RowView, toggleDetail detail: UInt64, open: Bool) {
        pendingFold = view.key
        engine.toggleDetail(row: view.key, detail: detail, open: open)
    }

    func rowView(_ view: RowView, action: String) {
        UISelectionFeedbackGenerator().selectionChanged()
        pendingFold = view.key
        // Folding, pausing, stopping… are decided in Rust; only navigation comes back.
        if case let .openChat(chatId) = engine.act(payload: action) {
            (window?.rootViewController as? AppRouter)?.openSession(chatId)
        }
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
    private let mono: Bool

    init(text: String, title: String = "Select Text", mono: Bool = false) {
        self.text = text
        self.mono = mono
        super.init(nibName: nil, bundle: nil)
        self.title = title
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Palette.background
        let tv = UITextView()
        tv.text = text
        tv.isEditable = false
        tv.font = mono ? Fonts.ui(.mono, 14) : Fonts.ui(.sans, UIFontMetrics(forTextStyle: .body).scaledValue(for: 16.5))
        tv.textColor = Palette.text
        tv.backgroundColor = .clear
        tv.textContainerInset = UIEdgeInsets(top: 16, left: 14, bottom: 32, right: 14)
        tv.frame = view.bounds
        tv.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        view.addSubview(tv)
        navigationItem.rightBarButtonItem = UIBarButtonItem(systemItem: .done, primaryAction: UIAction { [weak self] _ in
            self?.dismiss(animated: true)
        })
        if !mono { DispatchQueue.main.async { tv.selectAll(nil) } }
    }
}
