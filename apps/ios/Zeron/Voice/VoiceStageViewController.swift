import AVKit
import UIKit

/// The full-screen call (desktop `voice_stage.rs`): the session's orb over the
/// wallpaper, the live caption, and the call controls. Swipe down to get back
/// to the app; the call continues until End. Held to the ear it behaves like a
/// phone call — screen off, audio in the earpiece.
final class VoiceStageViewController: UIViewController {
    private let app: AppModel
    private var voice: RemoteVoiceController { app.voice }
    private var token: AnyObject?
    private let wallpaper = WallpaperView()
    private let orb = OrbView(preset: .hero, orb: .connecting)
    private let dot = UIView()
    private let clock = UILabel()
    private let host = UILabel()
    private let status = UILabel()
    private let caption = UILabel()
    /// The previous speaker turn, fading out over the new caption.
    private let previousCaption = UILabel()
    /// Streamed words veil in as on desktop (`zeron-veil` through the core).
    private let captionFader = CaptionFader(maxChars: 160)
    private var captionLink: CADisplayLink?
    private var captionSource: (item: String?, text: String, user: Bool) = (nil, "", false)
    /// The caption holds an end-of-call message instead of the utterance.
    private var captionShowsMessage = false
    private lazy var minimize = Glass.circleButton(symbol: "chevron.down", size: 44, pointSize: 16, action: UIAction { [weak self] _ in
        self?.dismiss(animated: true)
    })
    private lazy var transcript = Glass.circleButton(symbol: "text.bubble", size: 44, pointSize: 16, action: UIAction { [weak self] _ in
        self?.openTranscript()
    })
    private let badge = UIView()
    private let bar = Glass.surface()
    private let controls = UIStackView()
    private lazy var mute = Self.callButton(symbol: "mic.fill", tint: Palette.text, background: Palette.controlFill) { [weak self] in
        self?.voice.toggleMute()
    }
    private let route = AVRoutePickerView()
    private lazy var routeWell = Self.well(route)
    private lazy var end = Self.callButton(symbol: "phone.down.fill", tint: .white, background: .systemRed, width: 88) { [weak self] in
        self?.voice.stop()
    }
    private lazy var retry = Self.pillButton(title: "Call again", tint: .white, background: Palette.accent) { [weak self] in
        self?.retryCall()
    }
    private lazy var close = Self.pillButton(title: "Close", tint: Palette.text, background: Palette.controlFill) { [weak self] in
        self?.voice.dismissEndReason()
        self?.dismiss(animated: true)
    }
    private var shownMuted: Bool?

    init(app: AppModel) {
        self.app = app
        super.init(nibName: nil, bundle: nil)
        modalPresentationStyle = .fullScreen
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Palette.background
        view.accessibilityIdentifier = "voice-stage"
        view.addSubview(wallpaper)

        orb.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(orb)

        dot.layer.cornerRadius = 3.5
        dot.translatesAutoresizingMaskIntoConstraints = false
        clock.font = Fonts.ui(.monoMedium, 13)
        clock.textColor = Palette.secondary
        host.font = Fonts.ui(.sansMedium, 13)
        host.textColor = Palette.tertiary
        let header = UIStackView(arrangedSubviews: [dot, clock, host])
        header.spacing = 7
        header.alignment = .center
        header.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(header)

        status.font = Fonts.ui(.sansMedium, 17)
        status.textColor = Palette.text
        status.textAlignment = .center
        status.accessibilityTraits = .updatesFrequently
        caption.font = Fonts.ui(.sans, 16)
        caption.textColor = Palette.secondary
        caption.textAlignment = .center
        caption.numberOfLines = 4
        caption.lineBreakMode = .byTruncatingHead
        previousCaption.font = caption.font
        previousCaption.textColor = Palette.secondary
        previousCaption.textAlignment = .center
        previousCaption.numberOfLines = 4
        previousCaption.isAccessibilityElement = false
        previousCaption.isHidden = true
        for label in [status, caption, previousCaption] {
            label.translatesAutoresizingMaskIntoConstraints = false
            view.addSubview(label)
        }

        minimize.accessibilityLabel = "Back to Zeron"
        minimize.accessibilityIdentifier = "voice-minimize"
        transcript.accessibilityLabel = "Open the voice transcript"
        transcript.accessibilityIdentifier = "voice-transcript"
        badge.backgroundColor = Palette.warning
        badge.layer.cornerRadius = 5
        badge.layer.borderWidth = 2
        badge.layer.borderColor = Palette.background.cgColor
        badge.isUserInteractionEnabled = false
        badge.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(minimize)
        view.addSubview(transcript)
        view.addSubview(badge)

        route.activeTintColor = Palette.accent
        route.tintColor = Palette.text
        route.prioritizesVideoDevices = false
        routeWell.accessibilityLabel = "Audio output"
        mute.accessibilityIdentifier = "voice-mute"
        end.accessibilityLabel = "End call"
        end.accessibilityIdentifier = "voice-end"
        retry.accessibilityIdentifier = "voice-retry"
        controls.axis = .horizontal
        controls.spacing = 14
        controls.alignment = .center
        controls.translatesAutoresizingMaskIntoConstraints = false
        for v in [mute, routeWell, end, retry, close] { controls.addArrangedSubview(v) }
        bar.contentView.addSubview(controls)
        bar.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(bar)

        let side = orb.widthAnchor.constraint(equalToConstant: 340)
        side.priority = .defaultHigh
        NSLayoutConstraint.activate([
            minimize.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor, constant: 8),
            minimize.leadingAnchor.constraint(equalTo: view.leadingAnchor, constant: 16),
            transcript.centerYAnchor.constraint(equalTo: minimize.centerYAnchor),
            transcript.trailingAnchor.constraint(equalTo: view.trailingAnchor, constant: -16),
            badge.widthAnchor.constraint(equalToConstant: 10),
            badge.heightAnchor.constraint(equalToConstant: 10),
            badge.topAnchor.constraint(equalTo: transcript.topAnchor, constant: 2),
            badge.trailingAnchor.constraint(equalTo: transcript.trailingAnchor, constant: -2),
            dot.widthAnchor.constraint(equalToConstant: 7),
            dot.heightAnchor.constraint(equalToConstant: 7),
            header.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            header.centerYAnchor.constraint(equalTo: minimize.centerYAnchor),
            header.leadingAnchor.constraint(greaterThanOrEqualTo: minimize.trailingAnchor, constant: 12),

            orb.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            orb.centerYAnchor.constraint(equalTo: view.safeAreaLayoutGuide.centerYAnchor, constant: -64),
            side,
            orb.widthAnchor.constraint(lessThanOrEqualTo: view.widthAnchor, constant: -32),
            orb.heightAnchor.constraint(equalTo: orb.widthAnchor),
            orb.topAnchor.constraint(greaterThanOrEqualTo: minimize.bottomAnchor, constant: 8),

            status.topAnchor.constraint(equalTo: orb.bottomAnchor, constant: 4),
            status.leadingAnchor.constraint(equalTo: view.leadingAnchor, constant: 32),
            status.trailingAnchor.constraint(equalTo: view.trailingAnchor, constant: -32),
            caption.topAnchor.constraint(equalTo: status.bottomAnchor, constant: 10),
            caption.leadingAnchor.constraint(equalTo: status.leadingAnchor),
            caption.trailingAnchor.constraint(equalTo: status.trailingAnchor),
            caption.bottomAnchor.constraint(lessThanOrEqualTo: bar.topAnchor, constant: -16),
            previousCaption.topAnchor.constraint(equalTo: caption.topAnchor),
            previousCaption.leadingAnchor.constraint(equalTo: caption.leadingAnchor),
            previousCaption.trailingAnchor.constraint(equalTo: caption.trailingAnchor),

            bar.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            bar.bottomAnchor.constraint(equalTo: view.safeAreaLayoutGuide.bottomAnchor, constant: -20),
            controls.topAnchor.constraint(equalTo: bar.contentView.topAnchor, constant: 10),
            controls.bottomAnchor.constraint(equalTo: bar.contentView.bottomAnchor, constant: -10),
            controls.leadingAnchor.constraint(equalTo: bar.contentView.leadingAnchor, constant: 10),
            controls.trailingAnchor.constraint(equalTo: bar.contentView.trailingAnchor, constant: -10),
        ])

        let swipe = UISwipeGestureRecognizer(target: self, action: #selector(swipeDown))
        swipe.direction = .down
        view.addGestureRecognizer(swipe)

        token = voice.observe { [weak self] in self?.refresh() }
        // The veil colors are resolved per appearance.
        registerForTraitChanges([UITraitUserInterfaceStyle.self]) { (self: VoiceStageViewController, _) in
            if !self.captionShowsMessage { self.paintCaption() }
        }
        refresh()
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        wallpaper.frame = CGRect(x: 0, y: 0, width: view.bounds.width, height: min(view.bounds.height * 0.72, 760))
    }

    @objc private func swipeDown() { dismiss(animated: true) }

    private func refresh() {
        let voice = self.voice
        guard voice.live || voice.endReason != nil else {
            // Hung up (here or elsewhere): the stage has nothing left to show.
            if presentingViewController != nil, !isBeingDismissed { dismiss(animated: true) }
            return
        }
        let state = voice.state
        orb.orb = voice.live ? voice.orb : .idle
        orb.setLevels(microphone: state?.microphone ?? 0, speaker: state?.speaker ?? 0)

        let awaiting = state?.work == .awaitingInput
        dot.backgroundColor = !voice.active ? Palette.tertiary : awaiting ? Palette.warning : voice.muted ? Palette.tertiary : Palette.success
        clock.text = voice.elapsedText ?? "–:––"
        host.text = voice.hostName
        transcript.isHidden = state?.chatId == nil
        badge.isHidden = transcript.isHidden || !awaiting
        transcript.accessibilityLabel = awaiting ? "Answer Codex in the transcript" : "Open the voice transcript"

        if let reason = voice.endReason, !voice.live {
            status.text = "Call ended"
            stopCaptionLink()
            captionShowsMessage = true
            previousCaption.isHidden = true
            caption.attributedText = nil
            caption.text = voice.message(for: reason)
            caption.textColor = Palette.secondary
            retry.configuration?.title = reason == .microphoneDenied ? "Open Settings" : "Call again"
        } else {
            status.text = voice.statusText
            let source = (item: state?.captionItem, text: state?.caption ?? "", user: state?.captionSpeaker == .user)
            if captionShowsMessage || source != captionSource {
                captionShowsMessage = false
                captionSource = source
                paintCaption()
            }
        }
        let ended = !voice.live
        for v in [mute, routeWell, end] { v.isHidden = ended }
        for v in [retry, close] { v.isHidden = !ended }
        if shownMuted != voice.muted {
            shownMuted = voice.muted
            var config = mute.configuration
            config?.image = UIImage(systemName: voice.muted ? "mic.slash.fill" : "mic.fill", withConfiguration: UIImage.SymbolConfiguration(pointSize: 20, weight: .semibold))
            // Inverted plate while muted, like the desktop call bar.
            config?.baseBackgroundColor = voice.muted ? Palette.text : Palette.controlFill
            config?.baseForegroundColor = voice.muted ? Palette.background : Palette.text
            mute.configuration = config
            mute.accessibilityLabel = voice.muted ? "Unmute microphone" : "Mute microphone"
        }
    }

    private func openTranscript() {
        guard let chatId = voice.state?.chatId else { return }
        let router = self.router
        dismiss(animated: true) { router?.openSession(chatId) }
    }

    private func retryCall() {
        if voice.endReason == .microphoneDenied, let url = URL(string: UIApplication.openSettingsURLString) {
            UIApplication.shared.open(url)
            return
        }
        if !voice.start() { voice.dismissEndReason() }
    }

    /// The tail of the utterance, its newest words fading in and the previous
    /// turn fading out; repaints every display frame until settled.
    private func paintCaption() {
        let frame = captionFader.frame(item: captionSource.item, text: captionSource.text, reducedMotion: UIAccessibility.isReduceMotionEnabled)
        let color = (captionSource.user ? Palette.tertiary : Palette.secondary).resolvedColor(with: traitCollection)
        let text = NSMutableAttributedString(string: frame.text, attributes: [.font: caption.font as Any, .foregroundColor: color])
        for span in frame.spans where span.alpha < 1 {
            let range = NSRange(location: Int(span.start), length: Int(span.end) - Int(span.start))
            guard NSMaxRange(range) <= text.length else { continue }
            text.addAttribute(.foregroundColor, value: color.withAlphaComponent(color.cgColor.alpha * CGFloat(span.alpha)), range: range)
        }
        caption.attributedText = text
        previousCaption.text = frame.previous
        previousCaption.alpha = CGFloat(frame.previousAlpha)
        previousCaption.isHidden = frame.previous == nil
        if frame.animating, captionLink == nil {
            let link = CADisplayLink(target: CaptionLinkProxy(self), selector: #selector(CaptionLinkProxy.tick))
            link.add(to: .main, forMode: .common)
            captionLink = link
        } else if !frame.animating {
            stopCaptionLink()
        }
    }

    fileprivate func captionTick() { paintCaption() }

    private func stopCaptionLink() {
        captionLink?.invalidate()
        captionLink = nil
    }

    deinit { captionLink?.invalidate() }

    private static func callButton(symbol: String, tint: UIColor, background: UIColor, width: CGFloat = 60, action: @escaping () -> Void) -> UIButton {
        var config = UIButton.Configuration.filled()
        config.image = UIImage(systemName: symbol, withConfiguration: UIImage.SymbolConfiguration(pointSize: 20, weight: .semibold))
        config.baseForegroundColor = tint
        config.baseBackgroundColor = background
        config.cornerStyle = .capsule
        let button = UIButton(configuration: config, primaryAction: UIAction { _ in action() })
        button.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            button.widthAnchor.constraint(equalToConstant: width),
            button.heightAnchor.constraint(equalToConstant: 60),
        ])
        return button
    }

    private static func pillButton(title: String, tint: UIColor, background: UIColor, action: @escaping () -> Void) -> UIButton {
        var config = UIButton.Configuration.filled()
        config.title = title
        config.baseForegroundColor = tint
        config.baseBackgroundColor = background
        config.cornerStyle = .capsule
        config.contentInsets = NSDirectionalEdgeInsets(top: 0, leading: 22, bottom: 0, trailing: 22)
        config.titleTextAttributesTransformer = UIConfigurationTextAttributesTransformer { attributes in
            var attributes = attributes
            attributes.font = Fonts.ui(.sansSemibold, 16)
            return attributes
        }
        let button = UIButton(configuration: config, primaryAction: UIAction { _ in action() })
        button.translatesAutoresizingMaskIntoConstraints = false
        button.heightAnchor.constraint(equalToConstant: 52).isActive = true
        return button
    }

    /// The system route picker (speaker, AirPods, car) in a call-button well.
    private static func well(_ picker: AVRoutePickerView) -> UIView {
        let well = UIView()
        well.backgroundColor = Palette.controlFill
        well.layer.cornerRadius = 30
        well.isAccessibilityElement = false
        picker.translatesAutoresizingMaskIntoConstraints = false
        well.addSubview(picker)
        well.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            well.widthAnchor.constraint(equalToConstant: 60),
            well.heightAnchor.constraint(equalToConstant: 60),
            picker.centerXAnchor.constraint(equalTo: well.centerXAnchor),
            picker.centerYAnchor.constraint(equalTo: well.centerYAnchor),
            picker.widthAnchor.constraint(equalToConstant: 44),
            picker.heightAnchor.constraint(equalToConstant: 44),
        ])
        return well
    }
}

// MARK: - Entry points

extension UIViewController {
    /// The voice control in a bar: open the live call, start one on the
    /// default host, or let the user pick a host.
    func openVoice(app: AppModel, source: UIView?) {
        let voice = app.voice
        if voice.live || voice.start() {
            presentVoiceStage(app: app, source: source)
        } else {
            presentVoiceHostPicker(app: app, source: source)
        }
    }

    func presentVoiceStage(app: AppModel, source: UIView?) {
        let host = presentedViewController ?? self
        guard !(host is VoiceStageViewController) else { return }
        let stage = VoiceStageViewController(app: app)
        if let source, source.window != nil, !UIAccessibility.isReduceMotionEnabled {
            // The orb blooms out of the bar, like the desktop footer orb.
            stage.preferredTransition = .zoom { [weak source] _ in source }
        }
        host.present(stage, animated: true)
    }

    func presentVoiceHostPicker(app: AppModel, source: UIView?) {
        let hosts = app.voice.hosts
        let message = hosts.isEmpty
            ? "No online device can host Codex voice. Start Zeron with remote voice enabled on your Mac or server."
            : "Audio stays on this iPhone. Codex and its tools run on the device you pick."
        let sheet = UIAlertController(title: "Talk to Codex on…", message: message, preferredStyle: .actionSheet)
        for device in hosts {
            sheet.addAction(UIAlertAction(title: device.name, style: .default) { [weak self] _ in
                guard let self, app.voice.start(host: device) else { return }
                self.presentVoiceStage(app: app, source: source)
            })
        }
        sheet.addAction(UIAlertAction(title: "Voice Settings", style: .default) { [weak self] _ in
            self?.presentVoiceSettings(app: app)
        })
        sheet.addAction(UIAlertAction(title: "Cancel", style: .cancel))
        sheet.popoverPresentationController?.sourceView = source ?? view
        (presentedViewController ?? self).present(sheet, animated: true)
    }

    func presentVoiceSettings(app: AppModel) {
        let settings = VoiceViewController(app: app)
        let nav = MainTabController.nav(settings)
        settings.navigationItem.rightBarButtonItem = UIBarButtonItem(systemItem: .done, primaryAction: UIAction { [weak nav] _ in
            nav?.dismiss(animated: true)
        })
        (presentedViewController ?? self).present(nav, animated: true)
    }

    /// Long-press menu of a voice control: host and style for the next call.
    func voiceMenu(app: AppModel) -> UIMenu {
        UIMenu(children: [UIDeferredMenuElement.uncached { [weak self] completion in
            let voice = app.voice
            let hosts = voice.hosts.map { device in
                UIAction(title: device.name, image: UIImage(systemName: "desktopcomputer"), attributes: voice.live ? .disabled : [], state: device.id == voice.selectedHost ? .on : .off) { _ in
                    voice.selectedHost = device.id
                }
            }
            let styles = ([nil] + voice.styles.map(Optional.some)).map { style in
                UIAction(title: style?.capitalized ?? "Codex default", state: style == voice.selectedStyle ? .on : .off) { _ in
                    voice.selectedStyle = style
                }
            }
            var items: [UIMenuElement] = []
            if !hosts.isEmpty { items.append(UIMenu(title: "Codex runs on", options: .displayInline, children: hosts)) }
            items.append(UIMenu(title: "Voice", subtitle: voice.selectedStyle?.capitalized ?? "Codex default", image: UIImage(systemName: "person.wave.2"), children: styles))
            items.append(UIAction(title: "Voice Settings", image: UIImage(systemName: "gearshape")) { _ in
                self?.presentVoiceSettings(app: app)
            })
            completion(items)
        }])
    }
}

/// CADisplayLink retains its target; the proxy keeps the stage releasable.
private final class CaptionLinkProxy: NSObject {
    weak var stage: VoiceStageViewController?
    init(_ stage: VoiceStageViewController) { self.stage = stage }
    @objc func tick() { MainActor.assumeIsolated { stage?.captionTick() } }
}
