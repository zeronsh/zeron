import AVKit
import UIKit

/// The full-screen call (desktop `voice_stage.rs`): over the wallpaper, the
/// call's orb — a disc of glass filled to its edge with the glyph's dots —
/// the words beneath it, and the controls in a glass pill: mute, audio
/// output, the way back to the conversation, and End last. Swipe down to get
/// back to the app; the call continues until End. Held to the ear it behaves like a phone
/// call — screen off, audio in the earpiece.
final class VoiceStageViewController: UIViewController {
    private let app: AppModel
    private var voice: RemoteVoiceController { app.voice }
    private var token: AnyObject?
    private let wallpaper = WallpaperView()
    /// The orb: the glass every Zeron surface is made of, as a disc, and the
    /// glyph filling it.
    private let orb = Glass.surface(radius: VoiceStageViewController.orbSize / 2)
    /// Its dots are about 0.02 in across, as the desktop stage's and its
    /// dictation beam's are: one resolution for every voice glyph.
    private let glyph = GlyphView(shape: .stage, orb: .connecting, dot: 3)
    /// A round veil of the page's colour behind the orb, its edge a long
    /// blur (desktop `STAGE_ORB_VEIL`): the wallpaper recedes evenly all
    /// around the orb, whatever the picture holds there.
    private let halo = UIView()
    private let clock = UILabel()
    private let host = UILabel()
    /// Why the call ended, in place of the transcript once it has. While the
    /// call lives no line of status is shown: the glyph and the controls say
    /// what it is doing, and VoiceOver hears it from the orb.
    private let caption = UILabel()
    /// The state VoiceOver last heard announced, so each is said once.
    private var announced: String?
    /// The call's words, turn by turn, as they are said.
    private let transcriptView = CallTranscriptView()
    /// Back to the call's conversation (where Codex's question waits), or to
    /// the app without one.
    private lazy var back = Self.callButton(symbol: "chevron.down", tint: Palette.text, background: Palette.controlFill) { [weak self] in
        self?.leave()
    }
    private let badge = UIView()
    private let header = UIStackView()
    private let bar = Glass.surface()
    private let controls = UIStackView()
    private lazy var mute = Self.callButton(symbol: "mic.fill", tint: Palette.text, background: Palette.controlFill) { [weak self] in
        self?.voice.toggleMute()
    }
    private let route = AVRoutePickerView()
    private lazy var routeWell = Self.well(route)
    private lazy var end = Self.callButton(symbol: "phone.down.fill", tint: .white, background: .systemRed) { [weak self] in
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

    /// The layout margin, the round controls and the space between them.
    private static let margin: CGFloat = 24
    private static let control: CGFloat = 56
    private static let orbSize: CGFloat = 200
    private static let rim = Palette.dynamic(0x4A5260, 0xBCC6D2, alpha: 0.12)
    /// The veil's strength, how far it holds past the orb and its blur, as
    /// shares of the orb's width (Core Animation's radius is twice the blur's
    /// spread, desktop's sigma).
    private static let veil: (strength: Float, spread: CGFloat, blur: CGFloat) = (0.9, 0.25, 0.5)

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Palette.background
        view.accessibilityIdentifier = "voice-stage"

        view.addSubview(wallpaper)
        halo.isUserInteractionEnabled = false
        halo.layer.shadowOffset = .zero
        halo.layer.shadowOpacity = Self.veil.strength
        view.addSubview(halo)
        orb.translatesAutoresizingMaskIntoConstraints = false
        orb.layer.borderWidth = 1
        orb.clipsToBounds = true
        view.addSubview(orb)
        glyph.translatesAutoresizingMaskIntoConstraints = false
        orb.contentView.addSubview(glyph)

        host.font = Fonts.ui(.sansSemibold, 15)
        host.textColor = Palette.text
        host.textAlignment = .center
        clock.font = Fonts.ui(.monoMedium, 12)
        clock.textColor = Palette.secondary
        clock.textAlignment = .center
        header.addArrangedSubview(host)
        header.addArrangedSubview(clock)
        header.axis = .vertical
        header.alignment = .center
        header.spacing = 1
        header.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(header)

        caption.font = Fonts.ui(.sans, 17)
        caption.textColor = Palette.secondary
        caption.textAlignment = .center
        caption.numberOfLines = 0
        caption.isHidden = true
        caption.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(caption)
        transcriptView.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(transcriptView)
        // The orb tells VoiceOver what the call is doing, now that the words
        // "Listening" and "Speaking" are no longer on screen.
        orb.isAccessibilityElement = true
        orb.accessibilityLabel = "Voice call"
        orb.accessibilityTraits = .updatesFrequently

        back.accessibilityIdentifier = "voice-minimize"
        badge.backgroundColor = Palette.warning
        badge.layer.cornerRadius = 5
        badge.layer.borderWidth = 2
        badge.layer.borderColor = Palette.background.cgColor
        badge.isUserInteractionEnabled = false
        badge.translatesAutoresizingMaskIntoConstraints = false
        back.addSubview(badge)

        route.activeTintColor = Palette.accent
        route.tintColor = Palette.text
        route.prioritizesVideoDevices = false
        routeWell.accessibilityLabel = "Audio output"
        mute.accessibilityIdentifier = "voice-mute"
        end.accessibilityLabel = "End call"
        end.accessibilityIdentifier = "voice-end"
        retry.accessibilityIdentifier = "voice-retry"
        controls.axis = .horizontal
        controls.spacing = 12
        controls.alignment = .center
        controls.translatesAutoresizingMaskIntoConstraints = false
        for v in [mute, routeWell, back, end, retry, close] { controls.addArrangedSubview(v) }
        bar.contentView.addSubview(controls)
        bar.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(bar)

        let margin = Self.margin
        NSLayoutConstraint.activate([
            orb.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            // A little above where it used to sit, never closer to the header
            // than this on a small iPhone.
            orb.topAnchor.constraint(greaterThanOrEqualTo: header.bottomAnchor, constant: 20),
            orb.widthAnchor.constraint(equalToConstant: Self.orbSize),
            orb.heightAnchor.constraint(equalToConstant: Self.orbSize),
            // The dots run to the disc's edge: the orb is the field.
            glyph.topAnchor.constraint(equalTo: orb.contentView.topAnchor),
            glyph.bottomAnchor.constraint(equalTo: orb.contentView.bottomAnchor),
            glyph.leadingAnchor.constraint(equalTo: orb.contentView.leadingAnchor),
            glyph.trailingAnchor.constraint(equalTo: orb.contentView.trailingAnchor),

            badge.widthAnchor.constraint(equalToConstant: 10),
            badge.heightAnchor.constraint(equalToConstant: 10),
            badge.topAnchor.constraint(equalTo: back.topAnchor, constant: 6),
            badge.trailingAnchor.constraint(equalTo: back.trailingAnchor, constant: -6),
            header.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            header.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor, constant: 12),
            header.leadingAnchor.constraint(greaterThanOrEqualTo: view.leadingAnchor, constant: margin),

            bar.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            bar.bottomAnchor.constraint(equalTo: view.safeAreaLayoutGuide.bottomAnchor, constant: -20),
            controls.topAnchor.constraint(equalTo: bar.contentView.topAnchor, constant: 8),
            controls.bottomAnchor.constraint(equalTo: bar.contentView.bottomAnchor, constant: -8),
            controls.leadingAnchor.constraint(equalTo: bar.contentView.leadingAnchor, constant: 8),
            controls.trailingAnchor.constraint(equalTo: bar.contentView.trailingAnchor, constant: -8),

            caption.topAnchor.constraint(equalTo: orb.bottomAnchor, constant: 32),
            caption.leadingAnchor.constraint(equalTo: view.leadingAnchor, constant: margin),
            caption.trailingAnchor.constraint(equalTo: view.trailingAnchor, constant: -margin),
            // The words live in the band between the orb and the controls,
            // the whole of it: centred in it while they are few, so the
            // space above and below them always balances, as on desktop.
            transcriptView.bottomAnchor.constraint(equalTo: bar.topAnchor, constant: -20),
            transcriptView.topAnchor.constraint(equalTo: orb.bottomAnchor, constant: 24),
            // Centred under the orb, filling the width up to a reading
            // column, as the desktop stage's transcript is.
            transcriptView.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            transcriptView.widthAnchor.constraint(lessThanOrEqualToConstant: 560),
            transcriptView.leadingAnchor.constraint(greaterThanOrEqualTo: view.leadingAnchor, constant: margin),
        ])
        let fill = transcriptView.widthAnchor.constraint(equalTo: view.widthAnchor, constant: -2 * margin)
        fill.priority = .defaultHigh
        fill.isActive = true
        // A fixed step under the header, with the words' room below: the
        // stage reads top down as one column (host, orb, words, controls)
        // on every height, and the orb settles here on arrival and never
        // moves after. On a small iPhone the band below gives way first.
        let high = orb.topAnchor.constraint(equalTo: header.bottomAnchor, constant: 64)
        high.priority = .defaultHigh
        high.isActive = true

        let swipe = UISwipeGestureRecognizer(target: self, action: #selector(swipeDown))
        swipe.direction = .down
        // A swipe that starts in the transcript scrolls it back instead.
        swipe.delegate = self
        view.addGestureRecognizer(swipe)

        token = voice.observe { [weak self] in self?.refresh() }
        refresh()
    }

    /// The orb's arrival: it grows the last bit of the way and settles,
    /// once, as the stage comes up.
    private var arrived = false

    override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        guard !arrived, animated, !UIAccessibility.isReduceMotionEnabled else { return }
        for v in [orb, halo] as [UIView] {
            v.transform = CGAffineTransform(scaleX: 0.84, y: 0.84).translatedBy(x: 0, y: 24)
            v.alpha = 0
        }
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        guard !arrived else { return }
        arrived = true
        let settle = {
            for v in [self.orb, self.halo] as [UIView] {
                v.transform = .identity
                v.alpha = 1
            }
        }
        guard animated, !UIAccessibility.isReduceMotionEnabled else { return settle() }
        UIView.animate(withDuration: 0.8, delay: 0.05, usingSpringWithDamping: 0.82, initialSpringVelocity: 0.2, options: [.allowUserInteraction], animations: settle)
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        wallpaper.frame = CGRect(x: 0, y: 0, width: view.bounds.width, height: min(view.bounds.height * 0.72, 760))
        halo.frame = orb.frame
        let spread = orb.bounds.width * Self.veil.spread
        halo.layer.shadowPath = UIBezierPath(ovalIn: halo.bounds.insetBy(dx: -spread, dy: -spread)).cgPath
        halo.layer.shadowRadius = orb.bounds.width * Self.veil.blur
        halo.layer.shadowColor = Palette.background.resolvedColor(with: traitCollection).cgColor
        // The composer's rim: a faint cool hairline that sets the glass
        // against any picture.
        orb.layer.borderColor = Self.rim.resolvedColor(with: traitCollection).cgColor
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
        glyph.orb = voice.live ? voice.orb : .idle
        glyph.setLevels(microphone: state?.microphone ?? 0, speaker: state?.speaker ?? 0)

        let awaiting = state?.work == .awaitingInput
        clock.text = voice.elapsedText ?? "–:––"
        host.text = voice.hostName
        badge.isHidden = state?.chatId == nil || !awaiting
        back.accessibilityLabel = state?.chatId == nil ? "Back to Zeron" : awaiting ? "Answer Codex in the conversation" : "Back to the conversation"

        orb.accessibilityValue = voice.statusText
        if let reason = voice.endReason, !voice.live {
            caption.text = voice.message(for: reason)
            caption.isHidden = false
            transcriptView.isHidden = true
            retry.configuration?.title = reason == .microphoneDenied ? "Open Settings" : "Call again"
        } else {
            caption.isHidden = true
            // What needs the user is said aloud once, as the badge on the way
            // back shows it; the rest VoiceOver reads from the orb.
            if awaiting, announced != voice.statusText {
                UIAccessibility.post(notification: .announcement, argument: voice.statusText)
            }
            announced = voice.statusText
            transcriptView.isHidden = false
            if let state {
                transcriptView.show(state.captionTurns)
            }
        }
        let ended = !voice.live
        for v in [mute, routeWell, back, end] { v.isHidden = ended }
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

    /// The way back: to the call's conversation, or to the app without one.
    private func leave() {
        guard let chatId = voice.state?.chatId else {
            dismiss(animated: true)
            return
        }
        // The conversation is put in place under the stage, then the stage
        // slides away onto it; the call goes on.
        guard let router else { return dismiss(animated: true) }
        router.revealSession(chatId)
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

    private static func callButton(symbol: String, tint: UIColor, background: UIColor, action: @escaping () -> Void) -> UIButton {
        var config = UIButton.Configuration.filled()
        config.image = UIImage(systemName: symbol, withConfiguration: UIImage.SymbolConfiguration(pointSize: 20, weight: .semibold))
        config.baseForegroundColor = tint
        config.baseBackgroundColor = background
        config.cornerStyle = .capsule
        let button = UIButton(configuration: config, primaryAction: UIAction { _ in action() })
        button.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            button.widthAnchor.constraint(equalToConstant: control),
            button.heightAnchor.constraint(equalToConstant: control),
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
        well.layer.cornerRadius = control / 2
        well.isAccessibilityElement = false
        picker.translatesAutoresizingMaskIntoConstraints = false
        well.addSubview(picker)
        well.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            well.widthAnchor.constraint(equalToConstant: control),
            well.heightAnchor.constraint(equalToConstant: control),
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

extension VoiceStageViewController: UIGestureRecognizerDelegate {
    func gestureRecognizer(_ gestureRecognizer: UIGestureRecognizer, shouldReceive touch: UITouch) -> Bool {
        !(touch.view?.isDescendant(of: transcriptView) ?? false)
    }
}
