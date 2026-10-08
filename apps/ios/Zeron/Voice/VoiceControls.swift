import UIKit

/// Voice inside the tab bar's bottom accessory. Idle: a round waveform button
/// beside "New session" (tap to call, hold for host and style). Live: a call
/// strip with the orb, the call clock and mute/hang-up — tap it for the stage.
final class VoiceAccessoryView: UIView {
    var onStart: ((UIView) -> Void)?
    var onOpen: ((UIView) -> Void)?
    var onMute: (() -> Void)?
    var onEnd: (() -> Void)?

    private(set) var live = false
    let startButton = UIButton(type: .system)
    private let strip = PressableControl()
    let orb = OrbView(preset: .avatar, orb: .connecting)
    private let title = UILabel()
    private let detail = UILabel()
    private let mute = UIButton(type: .system)
    private let end = UIButton(type: .system)
    private var inline = false

    override init(frame: CGRect) {
        super.init(frame: frame)
        var config = UIButton.Configuration.filled()
        config.image = UIImage(systemName: "waveform", withConfiguration: UIImage.SymbolConfiguration(pointSize: 15, weight: .semibold))
        config.baseForegroundColor = Palette.accent
        config.baseBackgroundColor = Palette.accentSoft
        config.cornerStyle = .capsule
        startButton.configuration = config
        startButton.accessibilityLabel = "Talk to Codex"
        startButton.accessibilityHint = "Starts a voice call. Touch and hold to choose the device and voice."
        startButton.accessibilityIdentifier = "voice-start"
        startButton.addAction(UIAction { [weak self] _ in
            guard let self else { return }
            self.onStart?(self.startButton)
        }, for: .primaryActionTriggered)

        orb.isUserInteractionEnabled = false
        title.font = Fonts.ui(.sansMedium, 15)
        title.textColor = Palette.text
        detail.font = Fonts.ui(.monoMedium, 12)
        detail.textColor = Palette.secondary
        let labels = UIStackView(arrangedSubviews: [title, detail])
        labels.axis = .vertical
        labels.spacing = 0
        labels.isUserInteractionEnabled = false
        for (button, symbol, tint, fill) in [
            (mute, "mic.fill", Palette.text, Palette.controlFill),
            (end, "phone.down.fill", UIColor.white, UIColor.systemRed),
        ] {
            var config = UIButton.Configuration.filled()
            config.image = UIImage(systemName: symbol, withConfiguration: UIImage.SymbolConfiguration(pointSize: 13, weight: .bold))
            config.baseForegroundColor = tint
            config.baseBackgroundColor = fill
            config.cornerStyle = .capsule
            button.configuration = config
            button.translatesAutoresizingMaskIntoConstraints = false
            NSLayoutConstraint.activate([
                button.widthAnchor.constraint(equalToConstant: 32),
                button.heightAnchor.constraint(equalToConstant: 32),
            ])
        }
        mute.addAction(UIAction { [weak self] _ in self?.onMute?() }, for: .primaryActionTriggered)
        end.addAction(UIAction { [weak self] _ in self?.onEnd?() }, for: .primaryActionTriggered)
        end.accessibilityLabel = "End call"
        end.accessibilityIdentifier = "voice-accessory-end"
        strip.accessibilityIdentifier = "voice-live"
        strip.isAccessibilityElement = true
        strip.accessibilityTraits = .button
        // A plain UIControl never sends .primaryActionTriggered (only buttons do).
        strip.addAction(UIAction { [weak self] _ in
            guard let self else { return }
            self.onOpen?(self.orb)
        }, for: .touchUpInside)

        for v in [orb, labels, mute, end] as [UIView] {
            v.translatesAutoresizingMaskIntoConstraints = false
            strip.addSubview(v)
        }
        for v in [startButton, strip] as [UIView] {
            v.translatesAutoresizingMaskIntoConstraints = false
            addSubview(v)
        }
        NSLayoutConstraint.activate([
            startButton.trailingAnchor.constraint(equalTo: trailingAnchor),
            startButton.centerYAnchor.constraint(equalTo: centerYAnchor),
            startButton.widthAnchor.constraint(equalToConstant: 34),
            startButton.heightAnchor.constraint(equalToConstant: 34),
            startButton.leadingAnchor.constraint(greaterThanOrEqualTo: leadingAnchor),

            strip.topAnchor.constraint(equalTo: topAnchor),
            strip.bottomAnchor.constraint(equalTo: bottomAnchor),
            strip.leadingAnchor.constraint(equalTo: leadingAnchor),
            strip.trailingAnchor.constraint(equalTo: trailingAnchor),
            orb.leadingAnchor.constraint(equalTo: strip.leadingAnchor),
            orb.centerYAnchor.constraint(equalTo: strip.centerYAnchor),
            orb.widthAnchor.constraint(equalToConstant: 34),
            orb.heightAnchor.constraint(equalToConstant: 34),
            labels.leadingAnchor.constraint(equalTo: orb.trailingAnchor, constant: 8),
            labels.centerYAnchor.constraint(equalTo: strip.centerYAnchor),
            labels.trailingAnchor.constraint(lessThanOrEqualTo: mute.leadingAnchor, constant: -8),
            mute.trailingAnchor.constraint(equalTo: end.leadingAnchor, constant: -6),
            mute.centerYAnchor.constraint(equalTo: strip.centerYAnchor),
            end.trailingAnchor.constraint(equalTo: strip.trailingAnchor),
            end.centerYAnchor.constraint(equalTo: strip.centerYAnchor),
        ])
        strip.isHidden = true
    }

    required init?(coder: NSCoder) { fatalError() }

    /// Minimized tab bar: the orb, the clock and hang-up only.
    func setInline(_ inline: Bool) {
        self.inline = inline
        title.isHidden = inline
        mute.isHidden = inline
    }

    func update(_ voice: RemoteVoiceController) {
        live = voice.live
        startButton.isHidden = live
        strip.isHidden = !live
        orb.isHidden = !live
        guard live else { return }
        let state = voice.state
        orb.orb = voice.orb
        orb.setLevels(microphone: state?.microphone ?? 0, speaker: state?.speaker ?? 0)
        title.text = state?.work == .awaitingInput ? "Codex needs you" : "Codex"
        title.textColor = state?.work == .awaitingInput ? Palette.warning : Palette.text
        detail.text = voice.active ? [voice.elapsedText, voice.muted ? "Muted" : nil].compactMap { $0 }.joined(separator: " · ") : "Connecting…"
        var config = mute.configuration
        config?.image = UIImage(systemName: voice.muted ? "mic.slash.fill" : "mic.fill", withConfiguration: UIImage.SymbolConfiguration(pointSize: 13, weight: .bold))
        config?.baseBackgroundColor = voice.muted ? Palette.text : Palette.controlFill
        config?.baseForegroundColor = voice.muted ? Palette.background : Palette.text
        mute.configuration = config
        mute.accessibilityLabel = voice.muted ? "Unmute microphone" : "Mute microphone"
        strip.accessibilityLabel = "Voice call, \(voice.statusText)" + (voice.elapsedText.map { ", \($0)" } ?? "")
        strip.accessibilityHint = "Opens the call"
    }
}

/// A plain control that dims while pressed.
class PressableControl: UIControl {
    override var isHighlighted: Bool {
        didSet { UIView.animate(withDuration: 0.15) { self.alpha = self.isHighlighted ? 0.6 : 1 } }
    }
}

/// Voice in a navigation bar or toolbar: a waveform to start, the live orb
/// while a call runs. Used beside an open session and in the iPad sidebar.
@MainActor
final class VoiceBarItem {
    let item: UIBarButtonItem
    private let app: AppModel
    private let button = UIButton(type: .system)
    private let orb = OrbView(preset: .avatar, orb: .connecting)
    private weak var host: UIViewController?
    private var token: AnyObject?

    init(app: AppModel, host: UIViewController) {
        self.app = app
        self.host = host
        orb.isUserInteractionEnabled = false
        orb.translatesAutoresizingMaskIntoConstraints = false
        button.translatesAutoresizingMaskIntoConstraints = false
        button.addSubview(orb)
        NSLayoutConstraint.activate([
            button.widthAnchor.constraint(equalToConstant: 36),
            button.heightAnchor.constraint(equalToConstant: 36),
            orb.centerXAnchor.constraint(equalTo: button.centerXAnchor),
            orb.centerYAnchor.constraint(equalTo: button.centerYAnchor),
            orb.widthAnchor.constraint(equalToConstant: 32),
            orb.heightAnchor.constraint(equalToConstant: 32),
        ])
        button.setImage(UIImage(systemName: "waveform", withConfiguration: UIImage.SymbolConfiguration(pointSize: 16, weight: .semibold)), for: .normal)
        button.tintColor = Palette.text
        button.accessibilityIdentifier = "voice-bar"
        item = UIBarButtonItem(customView: button)
        button.addAction(UIAction { [weak self] _ in
            guard let self, let host = self.host else { return }
            host.openVoice(app: app, source: self.button)
        }, for: .primaryActionTriggered)
        token = app.voice.observe { [weak self] in self?.refresh() }
        refresh()
    }

    var visible: Bool { app.voice.live || app.voice.available }

    private var shownLive: Bool?

    private func refresh() {
        let voice = app.voice
        if voice.live {
            orb.orb = voice.orb
            orb.setLevels(microphone: voice.state?.microphone ?? 0, speaker: voice.state?.speaker ?? 0)
        }
        button.accessibilityLabel = voice.live ? "Voice call, \(voice.statusText)" : "Talk to Codex"
        guard shownLive != voice.live else { return }
        shownLive = voice.live
        orb.isHidden = !voice.live
        button.setImage(voice.live ? nil : UIImage(systemName: "waveform", withConfiguration: UIImage.SymbolConfiguration(pointSize: 16, weight: .semibold)), for: .normal)
        // Holding the live orb must not offer to change the call's host.
        button.menu = voice.live ? nil : host?.voiceMenu(app: app)
    }
}
