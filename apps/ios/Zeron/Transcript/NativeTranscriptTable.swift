// Native row virtualization owns estimates, reuse, and gesture anchoring.
// Message content remains SwiftUI; no delayed scrollTo retries or reveal gate.
import SwiftUI
import UIKit

struct NativeTranscriptTable: UIViewRepresentable {
    let rows: [TranscriptRow]
    let scroll: ScrollState
    let runwayID: String?
    let expansionHeight: CGFloat
    let bottomSpacing: CGFloat
    let reduceMotion: Bool
    let configurationID: Int
    var render: (TranscriptRow) -> AnyView

    func makeUIView(context: Context) -> TranscriptViewport {
        TranscriptViewport(scroll: scroll)
    }

    func updateUIView(_ viewport: TranscriptViewport, context: Context) {
        viewport.table.update(self)
    }
}

/// Keep mirrored transcript motion in sync even when a keyboard animation is
/// replaced or removed without changing the viewport's destination geometry.
private final class TranscriptViewportLayer: CALayer {
    override func add(_ animation: CAAnimation, forKey key: String?) {
        super.add(animation, forKey: key)
        if (animation as? CABasicAnimation)?.keyPath == "bounds.size" {
            // A replacement keyboard spring can have the same destination
            // bounds, so UIKit does not otherwise ask the view to lay out again.
            (delegate as? UIView)?.setNeedsLayout()
        }
    }

    override func removeAnimation(forKey key: String) {
        let resizesViewport = (animation(forKey: key) as? CABasicAnimation)?.keyPath == "bounds.size"
        super.removeAnimation(forKey: key)
        if resizesViewport { (delegate as? UIView)?.setNeedsLayout() }
    }

    override func removeAllAnimations() {
        super.removeAllAnimations()
        (delegate as? UIView)?.setNeedsLayout()
    }
}

/// Expose realized content directly. UITableView's accessibility row proxies
/// otherwise instantiate thousands of UIHostingConfigurations just to inspect
/// their traits, defeating virtualization for assistive technology clients.
@MainActor
final class TranscriptViewport: UIView {
    override class var layerClass: AnyClass { TranscriptViewportLayer.self }

    let table: TranscriptTableView
    private var offsetFraction: CGFloat = 0
    private var mirroredKeys: Set<String> = []
    private var offsetFractions: [String: CGFloat] = [:]

    init(scroll: ScrollState) {
        table = TranscriptTableView(scroll: scroll)
        super.init(frame: .zero)
        addSubview(table)
        accessibilityIdentifier = "transcript"
        accessibilityContainerType = .list
        accessibilityCustomActions = [
            UIAccessibilityCustomAction(name: "Earlier messages") { [weak self] _ in
                self?.table.accessibilityScroll(.down) ?? false
            },
            UIAccessibilityCustomAction(name: "Later messages") { [weak self] _ in
                self?.table.accessibilityScroll(.up) ?? false
            },
        ]
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    override func layoutSubviews() {
        super.layoutSubviews()
        let oldHeight = table.logicalViewportHeight
        let oldRunway = table.tableFooterView?.frame.height ?? 0
        let following = table.followsBottom
        let height = max(bounds.height, window?.bounds.height ?? bounds.height)
        table.setViewport(size: bounds.size, renderHeight: height)
        table.layoutIfNeeded()
        let heightDelta = oldHeight - bounds.height
        if abs(heightDelta) > 0.01 {
            let runwayDelta = oldRunway - (table.tableFooterView?.frame.height ?? 0)
            offsetFraction = following ? runwayDelta / heightDelta : 1
        }
        // UIKit layers interrupted keyboard springs additively. Mirror every
        // active size animation, including its start time, rather than replacing
        // a running spring with a separate guessed curve.
        var activeKeys: Set<String> = []
        for key in layer.animationKeys() ?? [] {
            guard let animation = layer.animation(forKey: key) as? CABasicAnimation,
                  animation.keyPath == "bounds.size",
                  let from = (animation.fromValue as? NSValue)?.cgSizeValue.height,
                  let to = (animation.toValue as? NSValue)?.cgSizeValue.height else { continue }
            activeKeys.insert(key)
            let positionKey = "viewport-position-" + key
            if table.layer.animation(forKey: positionKey) == nil || animation.beginTime == 0 {
                offsetFractions[key] = offsetFraction
            }
            mirror(animation, keyPath: "position.y", delta: from - to, key: positionKey)
            mirror(animation, keyPath: "bounds.origin.y",
                   delta: (offsetFractions[key] ?? offsetFraction) * (from - to),
                   key: "viewport-offset-" + key)
        }
        for key in mirroredKeys.subtracting(activeKeys) {
            table.layer.removeAnimation(forKey: "viewport-position-" + key)
            table.layer.removeAnimation(forKey: "viewport-offset-" + key)
            offsetFractions.removeValue(forKey: key)
        }
        mirroredKeys = activeKeys
    }

    private func mirror(_ source: CABasicAnimation, keyPath: String, delta: CGFloat, key: String) {
        guard abs(delta) > 0.01 else { table.layer.removeAnimation(forKey: key); return }
        let animation = source.copy() as! CABasicAnimation
        animation.delegate = nil
        animation.keyPath = keyPath
        animation.isAdditive = true
        animation.fromValue = delta
        animation.toValue = 0
        animation.byValue = nil
        table.layer.add(animation, forKey: key)
    }

    override var accessibilityElements: [Any]? {
        get {
            table.visibleCells.filter { table.convert($0.frame, to: self).intersects(bounds) }
                .sorted { $0.frame.minY < $1.frame.minY }
        }
        set { }
    }

    override func accessibilityScroll(_ direction: UIAccessibilityScrollDirection) -> Bool {
        table.accessibilityScroll(direction)
    }
}

@MainActor
final class TranscriptTableView: UITableView, UITableViewDataSource, UITableViewDelegate {
    private let follow: ScrollState
    private var rows: [TranscriptRow] = []
    private var render: ((TranscriptRow) -> AnyView)?
    private var runwayID: String?
    private var expansionHeight: CGFloat = 0
    private var bottomSpacing: CGFloat = 24
    private var configurationID: Int?
    private var updating = false
    private var settling = false
    private var animatingFollow = false
    private var followAnimationGeneration: UInt64 = 0
    private var hasPositioned = false
    private var lastGeometry = TranscriptGeometry(contentHeight: 0, viewportHeight: 0, offset: 0, bottom: 0)
    private let runway = UIView()
    private var resizeScheduled = false
    /// Measured row heights by row id. UITableView forgets every height on
    /// `reloadData`/`endUpdates` and falls back to the estimate, so without
    /// this the content size collapses to `rows × 140` and re-expands as rows
    /// realize — the visible jump under a finger or while a reply streams.
    private var measuredHeights: [String: CGFloat] = [:]
    private var measuredWidth: CGFloat = 0
    private static let fallbackEstimate: CGFloat = 140
    /// Above this diff size a batch update costs more than a plain reload.
    private static let maxBatchChanges = 400

    init(scroll: ScrollState) {
        follow = scroll
        super.init(frame: .zero, style: .plain)
        dataSource = self
        delegate = self
        register(UITableViewCell.self, forCellReuseIdentifier: "message")
        rowHeight = UITableView.automaticDimension
        estimatedRowHeight = 140
        separatorStyle = .none
        sectionHeaderHeight = 0
        sectionFooterHeight = 0
        sectionHeaderTopPadding = 0
        backgroundColor = .clear
        contentInsetAdjustmentBehavior = .never
        automaticallyAdjustsScrollIndicatorInsets = false
        keyboardDismissMode = .interactive
        alwaysBounceVertical = true
        clipsToBounds = false
        topEdgeEffect.isHidden = true // SessionView owns the color fade, without blur.
        runway.backgroundColor = .clear
        runway.frame.size.height = bottomSpacing
        tableFooterView = runway
        follow.nativeScrollView = self
        follow.refreshLayout = { [weak self] in self?.scheduleResize() }
        follow.jumpToLatest = { [weak self] animated in self?.requestLatest(animated: animated) }
        let tap = UITapGestureRecognizer(target: self, action: #selector(dismissKeyboard))
        tap.cancelsTouchesInView = false
        addGestureRecognizer(tap)
    }

    var followsBottom: Bool { follow.pinned && !userOwnsScroll }
    var logicalViewportHeight: CGFloat { max(0, bounds.height - contentInset.top) }
    private var viewportHeight: CGFloat { logicalViewportHeight }

    func setViewport(size: CGSize, renderHeight: CGFloat) {
        let topInset = max(0, renderHeight - size.height)
        let nextFrame = CGRect(x: 0, y: -topInset, width: size.width, height: renderHeight)
        // UIKit reconstructs frame from bounds/center. Fractional safe-area
        // values can differ by floating-point noise; resetting an unchanged
        // offset here would cancel the native send animation.
        guard abs(frame.minY - nextFrame.minY) > 0.01
            || abs(frame.width - nextFrame.width) > 0.01
            || abs(frame.height - nextFrame.height) > 0.01
            || abs(contentInset.top - topInset) > 0.01 else { return }
        if abs(measuredWidth - size.width) > 0.5 {
            measuredHeights.removeAll(keepingCapacity: true)
            measuredWidth = size.width
        }
        let logicalOffset = contentOffset.y + contentInset.top
        let anchor = followsBottom ? nil : visibleAnchor()
        updating = true
        // Apply final layout once; the viewport mirrors its compositor motion.
        // Avoid a second spring inside UIKit's self-sizing rows.
        UIView.performWithoutAnimation {
            frame = nextFrame
            contentInset.top = topInset
            verticalScrollIndicatorInsets.top = topInset
            let target = min(max(-topInset, logicalOffset - topInset),
                             max(-topInset, contentSize.height - bounds.height))
            cancelFollowAnimation()
            setContentOffset(CGPoint(x: 0, y: target), animated: false)
            layoutIfNeeded()
            if let anchor { restore(anchor) }
        }
        updating = false
        setNeedsLayout()
    }

    override func accessibilityScroll(_ direction: UIAccessibilityScrollDirection) -> Bool {
        let step: CGFloat
        switch direction {
        case .up, .next: step = 1
        case .down, .previous: step = -1
        default: return false
        }
        let target = min(max(-contentInset.top, contentOffset.y + step * max(44, viewportHeight - 80)),
                         max(-contentInset.top, contentSize.height - bounds.height))
        guard abs(target - contentOffset.y) > 0.5 else { return false }
        cancelFollowAnimation()
        follow.interactionEpoch &+= 1
        follow.pinned = false
        setContentOffset(CGPoint(x: 0, y: target), animated: false)
        layoutIfNeeded()
        if step > 0, contentSize.height - bounds.height - contentOffset.y <= TranscriptView.stickThreshold {
            follow.arm()
            goToLatest(animated: false)
        }
        UIAccessibility.post(notification: .pageScrolled,
                             argument: step > 0 ? "Later messages" : "Earlier messages")
        return true
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    @objc private func dismissKeyboard() {
        window?.endEditing(true)
    }

    func update(_ input: NativeTranscriptTable) {
        reconcileGesture()
        render = input.render
        let previous = rows
        let previousIDs = previous.map(\.id)
        let nextIDs = input.rows.map(\.id)
        let appending = hasPositioned && !previousIDs.isEmpty
            && nextIDs.count > previousIDs.count && nextIDs.starts(with: previousIDs)
        let newSubmission = input.runwayID != nil && input.runwayID != runwayID && hasPositioned
        let presentationChanged = configurationID != input.configurationID
        configurationID = input.configurationID
        runwayID = input.runwayID
        expansionHeight = input.expansionHeight
        bottomSpacing = input.bottomSpacing
        let anchor = visibleAnchor()
        rows = input.rows
        guard window != nil, bounds.width > 0 else {
            reloadData()
            setNeedsLayout()
            return
        }
        updating = true
        UIView.performWithoutAnimation {
            var reloaded = false
            if previousIDs != nextIDs {
                if appending {
                    insertRows(at: (previousIDs.count..<nextIDs.count).map { IndexPath(row: $0, section: 0) }, with: .none)
                } else {
                    reloaded = !applyDifference(from: previousIDs, to: nextIDs)
                }
            }
            if !reloaded {
                // A chunk can finish the previous block and append the next one.
                // Insertion alone leaves that visible block missing its suffix.
                var previousVersions: [String: UInt64] = [:]
                previousVersions.reserveCapacity(previous.count)
                for row in previous { previousVersions[row.id] = row.version }
                let visible = indexPathsForVisibleRows ?? []
                let changed = visible.filter {
                    guard $0.row < rows.count, let old = previousVersions[rows[$0.row].id] else { return false }
                    return presentationChanged || old != rows[$0.row].version
                }
                if !changed.isEmpty { reconfigureRows(at: changed) }
            }
            layoutIfNeeded()
            updateRunway()
            // Appended rows land below the viewport, so the anchor already
            // holds. Rewriting the offset here would kill a live flick or the
            // send animation (UIKit ends both silently, without a delegate call).
            let momentum = isDragging || isDecelerating || animatingFollow
            if let anchor, hasPositioned, !(appending && momentum),
               previousIDs != nextIDs || (presentationChanged && !follow.pinned) {
                restore(anchor)
            }
        }
        updating = false
        guard !rows.isEmpty, bounds.height > 0 else { return }
        if !hasPositioned {
            hasPositioned = true
            goToLatest(animated: false)
        } else if newSubmission {
            follow.arm()
            requestLatest(animated: !input.reduceMotion)
        } else if follow.pinned, !userOwnsScroll, !animatingFollow {
            goToLatest(animated: false)
        }
    }

    /// Delete/insert only the rows whose ids changed; returns false when the
    /// change is too large (or the table has never laid out), in which case
    /// the caller falls back to `reloadData`. Every retained row keeps its
    /// realized cell and height, so reading history through a hydration or
    /// a tool-group split no longer shifts the visible rows.
    private func applyDifference(from previousIDs: [String], to nextIDs: [String]) -> Bool {
        guard hasPositioned else { reloadData(); return false }
        let difference = nextIDs.difference(from: previousIDs)
        guard difference.count <= Self.maxBatchChanges else { reloadData(); return false }
        var deletions: [IndexPath] = []
        var insertions: [IndexPath] = []
        for change in difference {
            switch change {
            case .remove(let offset, _, _): deletions.append(IndexPath(row: offset, section: 0))
            case .insert(let offset, _, _): insertions.append(IndexPath(row: offset, section: 0))
            }
        }
        performBatchUpdates {
            if !deletions.isEmpty { deleteRows(at: deletions, with: .none) }
            if !insertions.isEmpty { insertRows(at: insertions, with: .none) }
        }
        return true
    }

    override func layoutSubviews() {
        UIView.performWithoutAnimation { super.layoutSubviews() }
        recordVisibleHeights()
        guard window != nil, !updating, !settling, !rows.isEmpty else { return }
        if !hasPositioned {
            hasPositioned = true
            let interaction = follow.interactionEpoch
            DispatchQueue.main.async { [weak self] in
                guard let self, self.follow.interactionEpoch == interaction, self.follow.pinned else { return }
                self.goToLatest(animated: false)
            }
        }
        settling = true
        UIView.performWithoutAnimation {
            updateRunway()
            if follow.pinned, !userOwnsScroll, !animatingFollow { alignBottom(animated: false) }
        }
        settling = false
    }

    private func recordVisibleHeights() {
        guard bounds.width > 0 else { return }
        if abs(measuredWidth - bounds.width) > 0.5 {
            measuredHeights.removeAll(keepingCapacity: true)
            measuredWidth = bounds.width
        }
        for path in indexPathsForVisibleRows ?? [] where path.row < rows.count {
            let height = rectForRow(at: path).height
            if height > 0 { measuredHeights[rows[path.row].id] = height }
        }
    }

    private var userOwnsScroll: Bool {
        isTracking || isDragging || isDecelerating || follow.userScrolling
    }

    /// A programmatic offset write during deceleration stops the scroll
    /// without `scrollViewDidEndDecelerating`. Without this the gesture flag
    /// would pin the transcript in user-owned state and follow would never
    /// resume.
    private func reconcileGesture() {
        guard follow.userScrolling, !isTracking, !isDragging, !isDecelerating else { return }
        finishGesture()
    }

    private struct Anchor {
        let id: String
        let version: UInt64
        let offset: CGFloat
        let height: CGFloat
    }

    private func visibleAnchor() -> Anchor? {
        let top = contentOffset.y + contentInset.top
        guard let index = indexPathsForVisibleRows?.sorted().first(where: { rectForRow(at: $0).maxY > top }),
              index.row < rows.count else { return nil }
        let rect = rectForRow(at: index)
        return Anchor(id: rows[index.row].id, version: rows[index.row].version,
                      offset: rect.minY - top, height: rect.height)
    }

    private var tailTarget: CGFloat { max(-contentInset.top, contentSize.height - bounds.height) }

    private func restore(_ anchor: Anchor) {
        guard let index = rows.firstIndex(where: { $0.id == anchor.id }) else { return }
        let rect = rectForRow(at: IndexPath(row: index, section: 0))
        // An unchanged row stays exactly where it was, however far it
        // straddles the top edge. Only a row whose content collapsed keeps a
        // 44pt handle in view so the fold stays reachable; a hosted cell
        // settling its own measurement is not a collapse.
        let shrank = rows[index].version != anchor.version && rect.height < anchor.height - 0.5
        let visibleOffset = shrank ? max(anchor.offset, -max(0, rect.height - 44)) : anchor.offset
        let target = min(max(-contentInset.top, rect.minY - visibleOffset - contentInset.top), max(-contentInset.top, contentSize.height - bounds.height))
        guard abs(contentOffset.y - target) > 0.5 else { return }
        cancelFollowAnimation()
        setContentOffset(CGPoint(x: 0, y: target), animated: false)
    }

    private func updateRunway() {
        var height = bottomSpacing
        if let id = runwayID, let index = rows.firstIndex(where: { $0.entryId == id }) {
            let start = rectForRow(at: IndexPath(row: index, section: 0)).minY
            let end = rectForRow(at: IndexPath(row: rows.count - 1, section: 0)).maxY
            let turnHeight = end - start
            height = max(bottomSpacing, viewportHeight + expansionHeight - turnHeight)
        }
        guard height.isFinite, abs(runway.frame.height - height) > 0.5 else { return }
        runway.frame.size.height = height
        tableFooterView = runway
    }

    private func alignBottom(animated: Bool) {
        let target = tailTarget
        guard abs(contentOffset.y - target) > 0.5 else {
            cancelFollowAnimation()
            return
        }
        if animated {
            animatingFollow = true
            scheduleFollowAnimationWatchdog()
        } else {
            cancelFollowAnimation()
        }
        setContentOffset(CGPoint(x: 0, y: target), animated: animated)
    }

    private func requestLatest(animated: Bool) {
        // Explicit send/jump takes over from momentum immediately.
        setContentOffset(contentOffset, animated: false)
        follow.userScrolling = false
        follow.userDragging = false
        cancelFollowAnimation()
        goToLatest(animated: animated)
    }

    private func goToLatest(animated: Bool) {
        guard window != nil, !rows.isEmpty, !userOwnsScroll else { return }
        // Resolve the final cell before calculating its footer reservation.
        // UIKit keeps this cell realized when estimates above it change.
        if !animated {
            settling = true
            scrollToRow(at: IndexPath(row: rows.count - 1, section: 0), at: .bottom, animated: false)
            layoutIfNeeded()
            updateRunway()
            alignBottom(animated: false)
            settling = false
        } else {
            let distance = tailTarget - contentOffset.y
            if distance > 2 * bounds.height {
                settling = true
                scrollToRow(at: IndexPath(row: rows.count - 1, section: 0),
                            at: .bottom, animated: false)
                layoutIfNeeded()
                updateRunway()
                let target = max(-contentInset.top, tailTarget - bounds.height)
                setContentOffset(CGPoint(x: 0, y: target), animated: false)
                layoutIfNeeded()
                updateRunway()
                settling = false
            }
            alignBottom(animated: true)
        }
    }

    private func cancelFollowAnimation() {
        followAnimationGeneration &+= 1
        animatingFollow = false
    }

    private func scheduleFollowAnimationWatchdog() {
        followAnimationGeneration &+= 1
        let generation = followAnimationGeneration
        DispatchQueue.main.asyncAfter(deadline: .now() + .milliseconds(450)) { [weak self] in
            guard let self,
                  self.followAnimationGeneration == generation,
                  self.animatingFollow,
                  self.follow.pinned,
                  !self.userOwnsScroll,
                  self.window != nil else { return }
            self.animatingFollow = false
            self.goToLatest(animated: false)
        }
    }

    private func scheduleResize() {
        guard !resizeScheduled else { return }
        resizeScheduled = true
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            self.resizeScheduled = false
            guard self.window != nil else { return }
            self.reconcileGesture()
            self.updating = true
            UIView.performWithoutAnimation {
                self.beginUpdates()
                self.endUpdates()
                self.layoutIfNeeded()
                self.updateRunway()
            }
            self.updating = false
            if self.follow.pinned, !self.userOwnsScroll, !self.animatingFollow {
                self.goToLatest(animated: false)
            }
        }
    }

    func tableView(_ tableView: UITableView, numberOfRowsInSection section: Int) -> Int { rows.count }

    func tableView(_ tableView: UITableView, estimatedHeightForRowAt indexPath: IndexPath) -> CGFloat {
        guard indexPath.row < rows.count else { return Self.fallbackEstimate }
        return measuredHeights[rows[indexPath.row].id] ?? Self.fallbackEstimate
    }

    func tableView(_ tableView: UITableView, didEndDisplaying cell: UITableViewCell, forRowAt indexPath: IndexPath) {
        guard let id = cell.accessibilityIdentifier, cell.bounds.height > 0 else { return }
        measuredHeights[id] = cell.bounds.height
    }
    func tableView(_ tableView: UITableView, heightForHeaderInSection section: Int) -> CGFloat { .leastNormalMagnitude }
    func tableView(_ tableView: UITableView, heightForFooterInSection section: Int) -> CGFloat { .leastNormalMagnitude }

    func tableView(_ tableView: UITableView, cellForRowAt indexPath: IndexPath) -> UITableViewCell {
        let cell = dequeueReusableCell(withIdentifier: "message", for: indexPath)
        let row = rows[indexPath.row]
        cell.accessibilityIdentifier = row.id
        cell.selectionStyle = .none
        cell.backgroundColor = .clear
        cell.contentView.backgroundColor = .clear
        let content = render?(row)
        cell.contentConfiguration = UIHostingConfiguration {
            content.id(row.id).ignoresSafeArea()
        }.margins(.all, 0)
        return cell
    }

    func scrollViewWillBeginDragging(_ scrollView: UIScrollView) {
        cancelFollowAnimation()
        follow.interactionEpoch &+= 1
        follow.userScrolling = true
        follow.userDragging = true
    }

    func scrollViewDidScroll(_ scrollView: UIScrollView) {
        let geometry = TranscriptGeometry(contentHeight: contentSize.height, viewportHeight: viewportHeight,
            offset: contentOffset.y + contentInset.top, bottom: contentSize.height - contentOffset.y - bounds.height)
        follow.observe(old: lastGeometry, new: geometry)
        lastGeometry = geometry
    }

    func scrollViewDidEndDragging(_ scrollView: UIScrollView, willDecelerate decelerate: Bool) {
        if !decelerate { finishGesture() }
    }

    func scrollViewDidEndDecelerating(_ scrollView: UIScrollView) { finishGesture() }

    private func finishGesture() {
        follow.endGesture()
        if follow.pinned { goToLatest(animated: false) }
    }

    func scrollViewDidEndScrollingAnimation(_ scrollView: UIScrollView) {
        cancelFollowAnimation()
        if follow.pinned { goToLatest(animated: false) }
    }
}
