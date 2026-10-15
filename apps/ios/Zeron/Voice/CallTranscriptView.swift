import UIKit

/// The call's words as they are said, under the orb, laid out as the app's
/// conversations are: the user's turns in their bubble on the trailing edge,
/// Codex's along the reading edge in the page's text. Every turn keeps one
/// size from its first word to its last, so nothing jumps as a turn settles
/// or the next begins; the streaming turn's newest words veil in
/// (`zeron-veil`, as on desktop).
///
/// Rows follow the call's turns by their utterance, never by their words: two
/// turns saying the same thing stay two rows, and two speakers never share
/// one. A turn whose speaker is not known yet reads on the reading edge and
/// moves to its side once the provider names it.
///
/// A short conversation sits centred in its band; one that outgrows the band
/// scrolls, fading out under the top edge, and every turn of the call stays
/// a scroll away. It follows the newest words while the reader is at the
/// bottom; scrolled up to read again, it stays put until they come back
/// down.
final class CallTranscriptView: UIView, UIScrollViewDelegate {
    private final class Row {
        /// The utterance shown; nil while its deltas stream unnamed.
        var item: String?
        var speaker: VoiceSpeaker?
        var settled = false
        var text = ""
        let view = UIView()
        let label = UILabel()
        let bubble = UIView()
        var placed: [NSLayoutConstraint] = []
        /// The veil's name for this row: an utterance named only at its final
        /// must not fade in again.
        lazy var key = "\(ObjectIdentifier(self).hashValue)"

        init(item: String?) {
            self.item = item
            label.numberOfLines = 0
            label.font = CallTranscriptView.font
            label.textColor = Palette.text
            label.translatesAutoresizingMaskIntoConstraints = false
            bubble.layer.cornerRadius = 20
            bubble.layer.cornerCurve = .continuous
            bubble.translatesAutoresizingMaskIntoConstraints = false
            bubble.addSubview(label)
            view.addSubview(bubble)
            view.accessibilityElements = [label]
            view.isHidden = true
        }

        var user: Bool { speaker == .user }

        /// The user's turn in its bubble on the trailing edge, as their
        /// messages read; Codex's (or one not yet attributed) along the
        /// reading edge, on the page.
        func place() {
            NSLayoutConstraint.deactivate(placed)
            bubble.backgroundColor = user ? Palette.userBubble : .clear
            let pad: (x: CGFloat, y: CGFloat) = user ? (16, 11) : (0, 0)
            placed = [
                label.topAnchor.constraint(equalTo: bubble.topAnchor, constant: pad.y),
                label.bottomAnchor.constraint(equalTo: bubble.bottomAnchor, constant: -pad.y),
                label.leadingAnchor.constraint(equalTo: bubble.leadingAnchor, constant: pad.x),
                label.trailingAnchor.constraint(equalTo: bubble.trailingAnchor, constant: -pad.x),
                bubble.topAnchor.constraint(equalTo: view.topAnchor),
                bubble.bottomAnchor.constraint(equalTo: view.bottomAnchor),
            ]
            if user {
                placed += [
                    bubble.trailingAnchor.constraint(equalTo: view.trailingAnchor),
                    bubble.widthAnchor.constraint(lessThanOrEqualTo: view.widthAnchor, multiplier: 0.84),
                ]
            } else {
                placed += [
                    bubble.leadingAnchor.constraint(equalTo: view.leadingAnchor),
                    bubble.trailingAnchor.constraint(lessThanOrEqualTo: view.trailingAnchor),
                ]
            }
            NSLayoutConstraint.activate(placed)
        }
    }

    private let scroll = UIScrollView()
    private let stack = UIStackView()
    private let fade = CAGradientLayer()
    private var rows: [Row] = []
    /// The streaming turn's newest words veil in; settled turns are plain.
    private let fader = CaptionFader(maxChars: 4000)
    private var link: CADisplayLink?
    private weak var veiled: Row?
    /// Whether to keep the newest words in view.
    private var following = true
    /// The band has been laid out once: later re-centring is animated.
    private var settled = false

    /// One size for every turn, streaming or settled, newest or not.
    fileprivate static let font = Fonts.ui(.sans, 19)
    /// Space between turns of one speaker, and where the speaker changes.
    private static let gap = (same: CGFloat(12), change: CGFloat(20))
    /// How far down the band the fade reaches.
    private static let fadeShare: CGFloat = 0.36

    override init(frame: CGRect) {
        super.init(frame: frame)
        scroll.delegate = self
        scroll.showsVerticalScrollIndicator = false
        scroll.alwaysBounceVertical = false
        scroll.contentInset = UIEdgeInsets(top: 0, left: 0, bottom: 4, right: 0)
        scroll.translatesAutoresizingMaskIntoConstraints = false
        addSubview(scroll)
        stack.axis = .vertical
        stack.spacing = Self.gap.same
        stack.translatesAutoresizingMaskIntoConstraints = false
        scroll.addSubview(stack)
        NSLayoutConstraint.activate([
            scroll.topAnchor.constraint(equalTo: topAnchor),
            scroll.bottomAnchor.constraint(equalTo: bottomAnchor),
            scroll.leadingAnchor.constraint(equalTo: leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: trailingAnchor),
            stack.topAnchor.constraint(equalTo: scroll.contentLayoutGuide.topAnchor),
            stack.bottomAnchor.constraint(equalTo: scroll.contentLayoutGuide.bottomAnchor),
            stack.leadingAnchor.constraint(equalTo: scroll.frameLayoutGuide.leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: scroll.frameLayoutGuide.trailingAnchor),
        ])
        fade.colors = [UIColor.clear.cgColor, UIColor.black.cgColor, UIColor.black.cgColor]
        layer.mask = fade
        registerForTraitChanges([UITraitUserInterfaceStyle.self]) { (self: CallTranscriptView, _) in self.paintVeil() }
    }

    required init?(coder: NSCoder) { fatalError() }

    deinit { link?.invalidate() }

    override func layoutSubviews() {
        super.layoutSubviews()
        fade.frame = bounds
        settle()
        veilTop()
    }

    /// The top edge fades only over words scrolled out under it: while the
    /// words fit, or the reader is at the top, every turn reads in full.
    private func veilTop() {
        let hidden = scroll.contentOffset.y + scroll.contentInset.top > 1
        let share = NSNumber(value: hidden ? Double(Self.fadeShare) : 0)
        guard fade.locations?.count != 3 || fade.locations?[1] != share else { return }
        fade.locations = [0, share, 1]
    }

    /// The call's turns, oldest first, every one with words. Rows are
    /// matched by utterance, so a state that carries the same words costs
    /// nothing, and a turn leaves once, with motion.
    func show(_ turns: [VoiceCaptionTurn]) {
        let shown = turns.filter { !$0.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
        var kept: [Row] = []
        var changed = false
        var streaming: Row?
        for turn in shown {
            let row = find(turn) ?? insert(Row(item: turn.item), after: kept.last)
            row.item = row.item ?? turn.item
            let text = turn.text.trimmingCharacters(in: .whitespacesAndNewlines)
            // A speaker once named stays named.
            let speaker = turn.speaker ?? row.speaker
            if row.speaker != speaker || row.text != text || row.settled != turn.settled {
                let moved = row.speaker != speaker
                row.speaker = speaker
                row.text = text
                row.settled = turn.settled
                row.view.isHidden = false
                // Constraints change only with the side; words only relabel.
                if moved {
                    row.place()
                    spaceAround(row)
                }
                row.label.accessibilityLabel = row.user ? "You: \(text)" : "Codex: \(text)"
                // Plain at the same size; the turn being said veils in below.
                row.label.attributedText = nil
                row.label.text = text
                row.label.textColor = Palette.text
                changed = true
            }
            // A turn that settles keeps veiling its last words in, so its
            // final lands without a pop.
            if !turn.settled { streaming = row }
            kept.append(row)
        }
        // Turns that have been heard leave, once.
        for row in rows where !kept.contains(where: { $0 === row }) {
            if veiled === row { veiled = nil }
            remove(row)
            changed = true
        }
        rows = kept
        if veiled !== streaming, let left = veiled {
            // The turn no longer being said rests at its full words.
            left.label.attributedText = nil
            left.label.text = left.text
            left.label.textColor = Palette.text
        }
        veiled = streaming
        // Ten states a second carry the call's levels, mostly with the same
        // words: those cost the transcript nothing (its own link veils in).
        guard changed else { return }
        paintVeil()
        settle()
        if following { scrollToLatest(animated: settled) }
    }

    /// Forgets the words: a new call starts empty.
    func reset() {
        rows.forEach { $0.view.removeFromSuperview() }
        rows.removeAll()
        veiled = nil
        following = true
        stopLink()
    }

    /// The row a turn already has: the one of its utterance. Deltas that
    /// streamed unnamed are one turn still open; its final names it.
    private func find(_ turn: VoiceCaptionTurn) -> Row? {
        if let item = turn.item, let row = rows.last(where: { $0.item == item }) { return row }
        guard turn.item == nil || turn.settled else { return nil }
        return rows.last { $0.item == nil && !$0.settled }
    }

    private func insert(_ row: Row, after previous: Row?) -> Row {
        let position = previous.flatMap { previous in rows.firstIndex { $0 === previous } }.map { $0 + 1 } ?? rows.count
        rows.insert(row, at: position)
        stack.insertArrangedSubview(row.view, at: position)
        row.place()
        spaceAround(row)
        // The new turn rises in, as the conversation's rows do.
        if window != nil, !UIAccessibility.isReduceMotionEnabled {
            row.view.alpha = 0
            row.view.transform = CGAffineTransform(translationX: 0, y: 10)
            UIView.animate(withDuration: 0.36, delay: 0, usingSpringWithDamping: 0.9, initialSpringVelocity: 0, options: [.allowUserInteraction, .beginFromCurrentState]) {
                row.view.alpha = 1
                row.view.transform = .identity
            }
        }
        return row
    }

    /// A turn that has been heard fades out and up; the rows beneath glide
    /// into its place.
    private func remove(_ row: Row) {
        guard window != nil, !UIAccessibility.isReduceMotionEnabled else { return row.view.removeFromSuperview() }
        UIView.animate(withDuration: 0.26, delay: 0, options: [.curveEaseOut, .beginFromCurrentState], animations: {
            row.view.alpha = 0
            row.view.transform = CGAffineTransform(translationX: 0, y: -8)
            row.view.isHidden = true
            self.stack.layoutIfNeeded()
        }, completion: { _ in row.view.removeFromSuperview() })
    }

    /// A change of speaker breathes more than a turn by the same speaker, as
    /// in a conversation.
    private func spaceAround(_ row: Row) {
        guard let index = rows.firstIndex(where: { $0 === row }) else { return }
        if index > 0 {
            let before = rows[index - 1]
            stack.setCustomSpacing(before.user == row.user ? Self.gap.same : Self.gap.change, after: before.view)
        }
        if index + 1 < rows.count {
            stack.setCustomSpacing(rows[index + 1].user == row.user ? Self.gap.same : Self.gap.change, after: row.view)
        }
    }

    /// The streaming turn with its newest words veiled in.
    private func paintVeil() {
        guard let row = veiled, !row.text.isEmpty else { return stopLink() }
        let frame = fader.frame(item: row.key, text: row.text, reducedMotion: UIAccessibility.isReduceMotionEnabled)
        let color = Palette.text.resolvedColor(with: traitCollection)
        let text = NSMutableAttributedString(string: frame.text, attributes: [.font: Self.font, .foregroundColor: color])
        for span in frame.spans where span.alpha < 1 {
            let range = NSRange(location: Int(span.start), length: Int(span.end) - Int(span.start))
            guard NSMaxRange(range) <= text.length else { continue }
            text.addAttribute(.foregroundColor, value: color.withAlphaComponent(color.cgColor.alpha * CGFloat(span.alpha)), range: range)
        }
        row.label.attributedText = text
        if frame.animating, link == nil {
            let link = CADisplayLink(target: TranscriptLinkProxy(self), selector: #selector(TranscriptLinkProxy.tick))
            link.add(to: .main, forMode: .common)
            self.link = link
        } else if !frame.animating {
            stopLink()
        }
    }

    fileprivate func tick() { paintVeil() }

    private func stopLink() {
        link?.invalidate()
        link = nil
    }

    /// The words sit centred in the band between the orb and the controls
    /// while they fit; once they outgrow it they fill it and scroll, the
    /// newest in view.
    private func settle() {
        let content = stack.systemLayoutSizeFitting(
            CGSize(width: bounds.width, height: UIView.layoutFittingCompressedSize.height),
            withHorizontalFittingPriority: .required,
            verticalFittingPriority: .fittingSizeLevel
        ).height
        let top = max(0, (bounds.height - content - scroll.contentInset.bottom) / 2)
        guard abs(scroll.contentInset.top - top) > 0.5 else { return }
        let change = {
            self.scroll.contentInset.top = top
            self.scroll.contentOffset = CGPoint(x: 0, y: -top)
        }
        // Words arriving re-centre the band: it glides, never jumps.
        guard window != nil, !UIAccessibility.isReduceMotionEnabled, settled else { settled = true; return change() }
        UIView.animate(withDuration: 0.36, delay: 0, usingSpringWithDamping: 0.9, initialSpringVelocity: 0, options: [.allowUserInteraction, .beginFromCurrentState], animations: change)
    }

    private func scrollToLatest(animated: Bool) {
        layoutIfNeeded()
        settle()
        let bottom = scroll.contentSize.height + scroll.contentInset.bottom - scroll.bounds.height
        let target = max(-scroll.contentInset.top, bottom)
        guard abs(scroll.contentOffset.y - target) > 0.5 else { return }
        scroll.setContentOffset(CGPoint(x: 0, y: target), animated: animated)
    }

    // MARK: UIScrollViewDelegate

    /// Reading back stops the follow; coming back down to the newest words
    /// resumes it.
    func scrollViewDidScroll(_ scrollView: UIScrollView) {
        veilTop()
        guard scrollView.isDragging || scrollView.isDecelerating else { return }
        let bottom = scrollView.contentSize.height + scrollView.contentInset.bottom - scrollView.bounds.height
        following = scrollView.contentOffset.y >= bottom - 24
    }
}

/// CADisplayLink retains its target; the proxy keeps the view releasable.
private final class TranscriptLinkProxy: NSObject {
    weak var view: CallTranscriptView?
    init(_ view: CallTranscriptView) { self.view = view }
    @objc func tick() { MainActor.assumeIsolated { view?.tick() } }
}
