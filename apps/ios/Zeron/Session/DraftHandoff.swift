import UIKit

/// What the new-session draft hands the chat it just created, so the switch
/// reads as one continuous motion instead of a sheet dismissal + push.
struct DraftHandoff {
    /// The draft page as it looked when Send was tapped (composer hidden).
    let scene: UIView
    let sceneFrame: CGRect
    /// The draft composer, lifted out to glide into the chat's composer.
    let composer: UIView
    let composerFrame: CGRect
    /// The sent text and where it sat in the composer (window coordinates).
    let text: String
    let textFrame: CGRect

    /// Capture from the draft at the moment of sending (before the composer
    /// clears).
    @MainActor
    static func capture(from vc: UIViewController, composer: ComposerBar, text: String) -> DraftHandoff? {
        guard let window = vc.view.window else { return nil }
        // Real pixels, not replicant snapshots: replicants go blank once the
        // sheet they mirror is torn down.
        // The composer travels empty — its text is the flying bubble.
        composer.textView.alpha = 0
        let composerSnap = image(of: composer, afterScreenUpdates: true)
        composer.textView.alpha = 1
        let composerFrame = composer.convert(composer.bounds, to: window)
        let tv = composer.textView
        let inset = tv.textContainerInset
        let textFrame = tv.convert(CGRect(x: inset.left, y: inset.top, width: tv.bounds.width - inset.left - inset.right, height: max(20, tv.bounds.height - inset.top - inset.bottom)), to: window)
        // The page without its composer (the composer travels separately).
        let host = vc.navigationController?.view ?? vc.view!
        composer.alpha = 0
        let scene = image(of: host, afterScreenUpdates: true)
        let sceneFrame = host.convert(host.bounds, to: window)
        return DraftHandoff(scene: scene, sceneFrame: sceneFrame, composer: composerSnap, composerFrame: composerFrame, text: text, textFrame: textFrame)
    }
}

extension DraftHandoff {
    @MainActor
    static func image(of view: UIView, afterScreenUpdates: Bool) -> UIView {
        let format = UIGraphicsImageRendererFormat.preferred()
        let image = UIGraphicsImageRenderer(bounds: view.bounds, format: format).image { _ in
            view.drawHierarchy(in: view.bounds, afterScreenUpdates: afterScreenUpdates)
        }
        let iv = UIImageView(image: image)
        iv.frame = view.bounds
        return iv
    }
}

/// Plays a handoff over the window once the chat is in place underneath:
/// the draft dissolves, its composer glides into the chat's, the message
/// flies from the text field into its bubble, and the transcript fades in.
@MainActor
enum DraftHandoffAnimator {
    static func run(_ h: DraftHandoff, into session: SessionViewController, window: UIWindow, completion: (() -> Void)? = nil) {
        let reduce = UIAccessibility.isReduceMotionEnabled
        h.scene.frame = h.sceneFrame
        h.composer.frame = h.composerFrame
        let bubble = BubbleFlight(text: h.text)
        bubble.frame = BubbleFlight.frame(aroundText: h.textFrame)
        for v in [h.scene, h.composer, bubble] as [UIView] {
            v.isUserInteractionEnabled = false
            window.addSubview(v)
        }
        if h.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { bubble.isHidden = true }
        window.layoutIfNeeded()
        // The swap under the overlays is one heavy commit (new chat, sheet
        // teardown, keyboard handoff). Start the motion on the next frames so
        // it isn't swallowed by that commit, and read the composer's target
        // once the keyboard layout has settled.
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.05) {
            animate(h, bubble: bubble, session: session, window: window, reduce: reduce, completion: completion)
        }
    }

    private static func animate(_ h: DraftHandoff, bubble: BubbleFlight, session: SessionViewController, window: UIWindow, reduce: Bool, completion: (() -> Void)?) {
        let composerTarget = session.arrivalComposerFrame(in: window)

        // Draft page dissolves (slight settle), composer glides home.
        UIView.animate(withDuration: reduce ? 0.2 : 0.32, delay: 0, options: [.curveEaseOut]) {
            h.scene.alpha = 0
            if !reduce { h.scene.transform = CGAffineTransform(scaleX: 0.985, y: 0.985) }
        }
        // The chat's composer slides down with the keyboard while this runs,
        // so the glide tracks its live frame rather than a snapshot of it.
        // The draft card keeps its size (no image squash) and settles onto the
        // chat's resting capsule, bottom edges aligned, while they crossfade.
        ComposerGlide.run(view: h.composer, from: h.composerFrame, duration: reduce ? 0.2 : 0.55) {
            let live = session.arrivalComposerFrame(in: window) ?? composerTarget ?? h.composerFrame
            return CGRect(x: live.minX, y: live.maxY - h.composerFrame.height, width: h.composerFrame.width, height: h.composerFrame.height)
        }
        UIView.animate(withDuration: 0.18, delay: reduce ? 0 : 0.36, options: [.curveEaseInOut]) {
            session.revealArrivalComposer()
            h.composer.alpha = 0
        }
        UIView.animate(withDuration: 0.35, delay: reduce ? 0 : 0.12, options: [.curveEaseOut]) {
            session.revealArrivalTranscript()
        }

        // The bubble flies once the transcript has laid out its first row.
        waitForBubble(session, window: window, tries: 0) { target in
            let target = target ?? CGRect(x: window.bounds.width - 16 - min(bubble.bounds.width, window.bounds.width * 0.8), y: window.safeAreaInsets.top + 60, width: min(bubble.bounds.width, window.bounds.width * 0.8), height: bubble.bounds.height)
            UIView.animate(withDuration: reduce ? 0.2 : 0.5, delay: 0, usingSpringWithDamping: 0.86, initialSpringVelocity: 0, options: [.beginFromCurrentState]) {
                bubble.frame = target
                bubble.settle()
                bubble.layoutIfNeeded()
            } completion: { _ in
                session.finishArrival()
                UIView.animate(withDuration: 0.14) {
                    bubble.alpha = 0
                } completion: { _ in
                    bubble.removeFromSuperview()
                    h.scene.removeFromSuperview()
                    h.composer.removeFromSuperview()
                    completion?()
                }
            }
        }
    }

    private static func waitForBubble(_ session: SessionViewController, window: UIWindow, tries: Int, _ go: @escaping (CGRect?) -> Void) {
        session.view.layoutIfNeeded()
        if let frame = session.arrivalBubbleFrame(in: window) { return go(frame) }
        guard tries < 20 else { return go(nil) }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.016) {
            waitForBubble(session, window: window, tries: tries + 1, go)
        }
    }
}

/// Tweens a view from a start frame toward a target that may move while it
/// runs (read every frame), with an ease-out close to the spring it replaces.
@MainActor
private final class ComposerGlide: NSObject {
    private var link: CADisplayLink?
    private let view: UIView
    private let from: CGRect
    private let duration: CFTimeInterval
    private let target: () -> CGRect
    private let start = CACurrentMediaTime()
    private var retain: ComposerGlide?

    static func run(view: UIView, from: CGRect, duration: CFTimeInterval, target: @escaping () -> CGRect) {
        let glide = ComposerGlide(view: view, from: from, duration: duration, target: target)
        glide.retain = glide
        let link = CADisplayLink(target: glide, selector: #selector(tick))
        link.preferredFrameRateRange = CAFrameRateRange(minimum: 60, maximum: 120, preferred: 120)
        link.add(to: .main, forMode: .common)
        glide.link = link
    }

    private init(view: UIView, from: CGRect, duration: CFTimeInterval, target: @escaping () -> CGRect) {
        self.view = view
        self.from = from
        self.duration = duration
        self.target = target
    }

    @objc private func tick() {
        let t = min(1, (CACurrentMediaTime() - start) / duration)
        // easeOutQuart
        let e = 1 - pow(1 - t, 4)
        let to = target()
        func lerp(_ a: CGFloat, _ b: CGFloat) -> CGFloat { a + (b - a) * CGFloat(e) }
        view.frame = CGRect(x: lerp(from.minX, to.minX), y: lerp(from.minY, to.minY), width: lerp(from.width, to.width), height: lerp(from.height, to.height))
        if t >= 1 || view.superview == nil {
            link?.invalidate()
            link = nil
            retain = nil
        }
    }
}

/// The flying message: starts as bare text where it was typed and settles
/// into the user bubble's look (fill, radius, padding) at its destination.
private final class BubbleFlight: UIView {
    private let label = UILabel()
    private let fill = UIView()
    static let pad = UIEdgeInsets(top: 10, left: 15, bottom: 10, right: 15)

    init(text: String) {
        super.init(frame: .zero)
        fill.backgroundColor = Palette.userBubble
        fill.layer.cornerRadius = 20
        fill.layer.cornerCurve = .continuous
        fill.alpha = 0
        addSubview(fill)
        label.text = text
        label.numberOfLines = 0
        // Never an ellipsis mid-flight (the real bubble lays the text out).
        label.lineBreakMode = .byClipping
        label.font = Fonts.ui(.sans, UIFontMetrics(forTextStyle: .body).scaledValue(for: 16.5))
        label.textColor = Palette.text
        addSubview(label)
    }

    required init?(coder: NSCoder) { fatalError() }

    static func frame(aroundText r: CGRect) -> CGRect {
        CGRect(x: r.minX - pad.left, y: r.minY - pad.top, width: r.width + pad.left + pad.right, height: r.height + pad.top + pad.bottom)
    }

    func settle() { fill.alpha = 1 }

    override func layoutSubviews() {
        super.layoutSubviews()
        fill.frame = bounds
        label.frame = bounds.inset(by: Self.pad)
    }
}
