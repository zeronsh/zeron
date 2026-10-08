import AudioToolbox
import UIKit

/// The app-wide voice orchestrator call. Audio runs on this phone; Codex, its
/// credentials and the orchestrator chat run on a registered execution host
/// (any device advertising `voice-client-media-v1`). One call at a time, no
/// automatic reconnect: a dropped call explains itself and waits for a tap.
@MainActor
final class RemoteVoiceController {
    static let capability = voiceHostCapability()
    /// Development override: offer voice before any host advertises it.
    static var forced: Bool {
        ProcessInfo.processInfo.arguments.contains("-remote-voice") || ProcessInfo.processInfo.environment["ZERON_REMOTE_VOICE"] == "1"
    }

    private weak var app: AppModel?
    private var call: VoiceCall?
    private var media: NativeVoiceMedia?
    private var generation: UInt64 = 0
    private var observers: [UUID: () -> Void] = [:]
    private var proximity: NSObjectProtocol?
    /// The call in the Dynamic Island and on the Lock Screen.
    private let liveActivity = VoiceActivityController()

    private(set) var state: VoiceCallState?
    /// Why the last call ended on its own; cleared by the next start.
    private(set) var endReason: VoiceEndReason?
    private(set) var muted = false
    private(set) var hostName = ""
    private(set) var activeSince: Date?
    /// Styles the last host offered.
    private(set) var styles: [String] = defaultVoiceStyles()

    var live: Bool {
        #if DEBUG
        return call != nil || previewing
        #else
        return call != nil
        #endif
    }
    var active: Bool { state?.phase == .active }
    var orb: VoiceOrb { state?.orb ?? (live ? .connecting : .idle) }

    init(app: AppModel) {
        self.app = app
        liveActivity.attach(to: self)
    }

    // MARK: Hosts and preferences

    static func compatible(_ device: DeviceView) -> Bool {
        device.isExecutionHost && device.capabilities.contains(capability)
    }

    /// Registered hosts that can take a call right now.
    var hosts: [DeviceView] { app?.client?.devices().filter { $0.online && Self.compatible($0) } ?? [] }

    /// Voice controls appear once any registered host can run them.
    var available: Bool {
        Self.forced || (app?.client?.devices().contains(where: Self.compatible) ?? false)
    }

    private var preferenceKey: String {
        "remoteVoice.\(app?.client?.userId() ?? "none").\(app?.client?.orgId() ?? "none")"
    }
    var selectedHost: String? {
        get { UserDefaults.standard.string(forKey: preferenceKey + ".host") }
        set { if !live { UserDefaults.standard.set(newValue, forKey: preferenceKey + ".host"); changed() } }
    }
    var selectedStyle: String? {
        get { UserDefaults.standard.string(forKey: preferenceKey + ".style") }
        set { UserDefaults.standard.set(newValue, forKey: preferenceKey + ".style"); changed() }
    }

    /// The host a plain tap calls: the chosen one, or the only one online.
    var startHost: DeviceView? {
        let hosts = self.hosts
        if let id = selectedHost, let host = hosts.first(where: { $0.id == id }) { return host }
        return hosts.count == 1 ? hosts.first : nil
    }

    func observe(_ callback: @escaping () -> Void) -> AnyObject {
        let id = UUID()
        observers[id] = callback
        return Token { [weak self] in self?.observers[id] = nil }
    }
    private func changed() { for callback in observers.values { callback() } }

    // MARK: Call

    /// Calls `host` (remembered for next time) or the default host. False
    /// when there is no host to call; the caller offers the picker.
    @discardableResult
    func start(host: DeviceView? = nil) -> Bool {
        guard !live, let client = app?.client, let device = host ?? startHost else { return false }
        if host != nil { selectedHost = device.id }
        generation &+= 1
        state = nil; endReason = nil; muted = false; activeSince = nil
        hostName = device.name
        let media = NativeVoiceMedia()
        self.media = media
        let generation = self.generation
        media.onFailure = { [weak self] in
            guard let self, self.generation == generation else { return }
            self.finish(reason: .audioUnavailable)
        }
        let listener = VoiceListenerBridge(controller: self, generation: generation)
        let call = client.startVoice(hostDeviceId: device.id, voice: selectedStyle, media: media, listener: listener)
        self.call = call
        media.call = call
        // A call keeps the screen awake, like the phone app.
        UIApplication.shared.isIdleTimerDisabled = true
        UIImpactFeedbackGenerator(style: .medium).impactOccurred()
        // The call sounds as it is placed, well before the microphone opens.
        CallSound.start.play()
        changed()
        return true
    }

    func toggleMute() {
        guard live else { return }
        muted.toggle()
        media?.muteLocally(muted)
        call?.setMuted(muted: muted)
        UIImpactFeedbackGenerator(style: .light).impactOccurred()
        changed()
    }

    /// The user hangs up. Local capture stops before any remote control.
    func stop() {
        guard live else { return }
        UIImpactFeedbackGenerator(style: .rigid).impactOccurred()
        finish(reason: nil)
    }

    func dismissEndReason() {
        guard endReason != nil else { return }
        endReason = nil
        changed()
    }

    private func finish(reason: VoiceEndReason?) {
        #if DEBUG
        previewing = false
        previewTimer?.invalidate()
        #endif
        generation &+= 1
        // A hang-up or a connected call ending sounds; a failure to connect
        // only explains itself.
        let sounds = call != nil && (activeSince != nil || reason == nil)
        media?.close()
        call?.stop()
        // After close: the call's audio session no longer ducks it.
        if sounds { CallSound.end.play() }
        call = nil; media = nil; state = nil; activeSince = nil
        endReason = reason
        setProximityMonitoring(false)
        UIApplication.shared.isIdleTimerDisabled = false
        if let reason {
            UINotificationFeedbackGenerator().notificationOccurred(.error)
            // The stage explains a failure itself; elsewhere a toast does.
            if let window = Self.keyWindow, !(Self.top(window.rootViewController) is VoiceStageViewController) {
                endReason = nil
                Toast.show(message(for: reason), action: reason == .microphoneDenied ? "Settings" : "Retry", in: window) { [weak self] in
                    if reason == .microphoneDenied, let url = URL(string: UIApplication.openSettingsURLString) {
                        UIApplication.shared.open(url)
                    } else if let self, let app = self.app, self.start() {
                        Self.top(window.rootViewController)?.presentVoiceStage(app: app, source: nil)
                    }
                }
            }
        }
        changed()
    }

    private static var keyWindow: UIWindow? {
        UIApplication.shared.connectedScenes.compactMap { ($0 as? UIWindowScene)?.keyWindow }.first
    }

    private static func top(_ controller: UIViewController?) -> UIViewController? {
        var top = controller
        while let presented = top?.presentedViewController { top = presented }
        return top
    }

    fileprivate func receive(_ state: VoiceCallState, generation: UInt64) {
        guard generation == self.generation, live else { return }
        let becameActive = state.phase == .active && self.state?.phase != .active
        self.state = state
        if !state.voices.isEmpty { styles = state.voices }
        if becameActive {
            activeSince = Date()
            setProximityMonitoring(true)
            UINotificationFeedbackGenerator().notificationOccurred(.success)
        }
        changed()
    }

    fileprivate func closed(_ reason: VoiceEndReason?, generation: UInt64) {
        guard generation == self.generation, live else { return }
        finish(reason: reason)
    }

    /// Held to the ear the screen goes dark and audio moves to the earpiece;
    /// in hand it plays on the loudspeaker. No-op on devices without a sensor.
    private func setProximityMonitoring(_ on: Bool) {
        UIDevice.current.isProximityMonitoringEnabled = on
        if on, proximity == nil {
            proximity = NotificationCenter.default.addObserver(forName: UIDevice.proximityStateDidChangeNotification, object: nil, queue: .main) { [weak self] _ in
                MainActor.assumeIsolated { self?.media?.setNearEar(UIDevice.current.proximityState) }
            }
        } else if !on, let proximity {
            NotificationCenter.default.removeObserver(proximity)
            self.proximity = nil
        }
    }

    // MARK: Presentation

    /// `m:ss`, or `h:mm:ss` past the hour (desktop `format_elapsed`).
    var elapsedText: String? {
        guard let activeSince else { return nil }
        let seconds = max(0, Int(Date().timeIntervalSince(activeSince)))
        let (h, m, s) = (seconds / 3600, seconds / 60 % 60, seconds % 60)
        return h > 0 ? String(format: "%d:%02d:%02d", h, m, s) : String(format: "%d:%02d", m, s)
    }

    /// One line for what the call is doing now.
    var statusText: String {
        guard let state else { return live ? "Calling \(hostName)…" : "Codex voice" }
        if state.phase == .connecting { return "Connecting to \(hostName)…" }
        if state.phase == .ending { return "Ending…" }
        switch state.orb {
        case .speaking: return "Speaking"
        case .awaitingInput: return "Codex needs your answer"
        case .working: return "Working on it…"
        case .muted: return "Muted"
        default: return "Listening"
        }
    }

    func message(for reason: VoiceEndReason) -> String {
        Self.message(for: reason, host: hostName.isEmpty ? "the selected device" : hostName)
    }

    static func message(for reason: VoiceEndReason, host: String) -> String {
        switch reason {
        case .microphoneDenied: return "Zeron needs microphone access to talk with Codex."
        case .audioUnavailable: return "The audio connection closed. Start a new call to reconnect."
        case .signInRequired: return "Sign in to Codex with ChatGPT on \(host)."
        case .usageUnavailable: return "Codex usage is currently unavailable on \(host)."
        case .busy: return "\(host) is already on a voice call."
        case .hostUnavailable: return "\(host) isn't reachable right now."
        case .hostIncompatible: return "Update Zeron on \(host) and enable remote voice there."
        case .connectionLost: return "The call dropped. Start a new one to reconnect."
        }
    }

    #if DEBUG
    private var previewing = false
    private var previewTimer: Timer?

    /// `-voice-preview`: a scripted call for screenshots and UI work — the
    /// orb walks through listening, speaking and working with synthetic
    /// levels. No audio, host or relay is involved.
    func preview() {
        previewing = true
        hostName = "Fedora"
        activeSince = Date().addingTimeInterval(-83)
        let script: [(VoiceOrb, VoiceCallWork, String, VoiceSpeaker?)] = [
            (.listening, .idle, "Open a chat on the Mac and ask Claude to fix the failing sync test", .user),
            (.speaking, .idle, "On it. I'm starting a Claude session in the comet project to look at the sync test.", nil),
            (.working, .working, "On it. I'm starting a Claude session in the comet project to look at the sync test.", .assistant),
            (.awaitingInput, .awaitingInput, "Claude wants to know whether to update the fixture or the parser.", .assistant),
        ]
        var step = 0
        previewTimer = Timer.scheduledTimer(withTimeInterval: 0.1, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self else { return }
                let turn = (step / 40) % script.count
                let (orb, work, caption, speaker) = script[turn]
                let wave = Float(abs(sin(Double(step) / 2.3)))
                // Words stream in, about three per second, like a live partial.
                let words = caption.split(separator: " ")
                let streamed = words.prefix(1 + (step % 40) / 3).joined(separator: " ")
                self.state = VoiceCallState(phase: .active, orb: orb, chatId: "voice-orchestrator-preview", work: work, muted: self.muted, speaking: orb == .speaking, caption: streamed, captionSpeaker: speaker, captionItem: "preview-\(turn)", microphone: orb == .listening ? wave : 0, speaker: orb == .speaking ? wave : 0, voices: [])
                step += 1
                self.changed()
            }
        }
    }
    #endif

    private final class Token {
        let cancel: () -> Void
        init(cancel: @escaping () -> Void) { self.cancel = cancel }
        deinit { cancel() }
    }
}

/// The call's two sounds, as system sounds: they play beside the call's own
/// audio session and follow the ringer switch, like the phone's.
@MainActor
private enum CallSound: String {
    case start = "voice-start"
    case end = "voice-end"

    private static var loaded: [CallSound: SystemSoundID] = [:]

    func play() {
        if Self.loaded[self] == nil, let url = Bundle.main.url(forResource: rawValue, withExtension: "wav") {
            var id: SystemSoundID = 0
            if AudioServicesCreateSystemSoundID(url as CFURL, &id) == kAudioServicesNoError { Self.loaded[self] = id }
        }
        if let id = Self.loaded[self] { AudioServicesPlaySystemSound(id) }
    }
}

private final class VoiceListenerBridge: VoiceSessionListener, @unchecked Sendable {
    private weak var controller: RemoteVoiceController?
    private let generation: UInt64
    init(controller: RemoteVoiceController, generation: UInt64) { self.controller = controller; self.generation = generation }
    func onVoiceState(state: VoiceCallState) {
        let controller = controller; let generation = generation
        Task { @MainActor [weak controller] in controller?.receive(state, generation: generation) }
    }
    func onVoiceClosed(reason: VoiceEndReason?) {
        let controller = controller; let generation = generation
        Task { @MainActor [weak controller] in controller?.closed(reason, generation: generation) }
    }
}
