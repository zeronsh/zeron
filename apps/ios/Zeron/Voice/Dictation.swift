import AVFoundation
import UIKit

/// Dictation on the phone: what is said goes into the draft as words. The
/// phone only captures: the audio streams to the device that runs the chat,
/// and that host's speech model (the desktop dictation's Parakeet) writes it
/// down. No model runs here, and no audio goes anywhere but that host. The
/// draft's rules are the desktop's (`crates/ui/src/dictation.rs`): a
/// transcript lands in the range it was started over, a changed draft is
/// never overwritten, and only an explicit Send submits, once the final
/// words are in.
enum DictationEvent: Equatable {
    /// The host takes audio.
    case listening
    /// The words so far; replaced by each next partial.
    case partial(String)
    /// The capture closed on its own (the limit, a route change): the
    /// final follows.
    case finalizing
    /// The last word on what was said; empty when nothing was heard.
    case final(String)
    /// Something else took the microphone (a phone call, the background):
    /// what was heard is kept, a pending send is not.
    case interrupted
    case denied(String)
    case unavailable(String)
    case failed(String)
}

/// Owned by the composer that started it, on the main actor. Tests and
/// screenshots inject a scripted one.
@MainActor
protocol DictationTranscriber: AnyObject {
    var onEvent: ((DictationEvent) -> Void)? { get set }
    /// Asks for permission if needed, opens the microphone and streams
    /// until finished or cancelled.
    func start()
    /// Closes the microphone; the final transcript follows.
    func finish()
    /// Drops everything: no event follows.
    func cancel()
    /// The loudest input peak since the previous call, 0–1 linear, for
    /// the track's beam.
    func takeLevel() -> Float
}

/// Where a dictation's words are written: the host running the chat.
struct DictationTarget {
    let client: CoreClient
    let hostId: String
    let hostName: String
}

/// The composer's side of a dictation: where the words go and whether to
/// send them. `range` is in UTF-16 units, as `UITextView` counts.
struct DictationDraft: Equatable {
    enum Phase: Equatable {
        case idle
        /// Permission, the microphone and the host are being obtained.
        case requesting
        case listening
        /// Capture closed; the final transcript is on its way.
        case finalizing
        case noSpeech
        case denied(String)
        case unavailable(String)
        case failed(String)

        var active: Bool {
            switch self {
            case .requesting, .listening, .finalizing: true
            default: false
            }
        }

        /// The microphone's name describes the action, not the state.
        var actionLabel: String {
            switch self {
            case .requesting, .listening: "Stop dictating"
            case .finalizing: "Transcribing"
            default: "Dictate"
            }
        }
    }

    /// The composer gives up on a transcript after this long.
    static let finalizeTimeout: TimeInterval = 30
    /// Capture closes on its own at this length, as on the desktop.
    static let maxSeconds: TimeInterval = 60

    private(set) var phase: Phase = .idle
    /// Bumped by every cancellation: late events carry a stale one.
    private(set) var generation: UInt64 = 0
    private(set) var range = NSRange(location: 0, length: 0)
    /// Words land after what is already there, not against it.
    private var lead = ""
    private var expected = ""
    private(set) var hasPartial = false
    private(set) var pendingSend = false
    private var finishStarted: Date?
    private var listeningSince: Date?
    private var frozenAt: Date?

    mutating func begin(content: String, selection: NSRange) {
        cancel()
        let ns = content as NSString
        // Clamped by hand: an empty range past the end has no intersection.
        let location = min(max(0, selection.location), ns.length)
        let selection = NSRange(location: location, length: min(max(0, selection.length), ns.length - location))
        phase = .requesting
        range = selection
        expected = content
        hasPartial = false
        lead = ""
        if selection.length == 0, selection.location > 0 {
            let before = ns.character(at: selection.location - 1)
            if let scalar = Unicode.Scalar(before), !CharacterSet.whitespacesAndNewlines.contains(scalar) {
                lead = " "
            }
        }
    }

    /// Invalidates late events and a pending send; committed words stay.
    mutating func cancel() {
        generation &+= 1
        phase = .idle
        pendingSend = false
        finishStarted = nil
        listeningSince = nil
        frozenAt = nil
        expected = ""
        lead = ""
    }

    mutating func listen(now: Date) {
        guard phase == .requesting else { return }
        phase = .listening
        listeningSince = now
    }

    /// True only on the transition that must close the microphone.
    mutating func finish(send: Bool, now: Date) -> Bool {
        guard phase.active else { return false }
        pendingSend = pendingSend || send
        guard phase != .finalizing else { return false }
        phase = .finalizing
        finishStarted = now
        frozenAt = now
        return true
    }

    /// An interrupted capture may be cut short: it is never sent on its own.
    mutating func dropPendingSend() {
        pendingSend = false
    }

    func timedOut(now: Date) -> Bool {
        finishStarted.map { now.timeIntervalSince($0) >= Self.finalizeTimeout } ?? false
    }

    /// How long the microphone has been open; held once capture closes.
    func elapsed(now: Date) -> TimeInterval? {
        listeningSince.map { (frozenAt ?? now).timeIntervalSince($0) }
    }

    /// Puts `transcript` where the dictation started and returns the
    /// cursor after it. Empty words never erase a selection or the last
    /// partial; a draft changed under the dictation ends it instead.
    mutating func replace(in content: inout String, with transcript: String) -> Int? {
        guard phase.active, content == expected else {
            cancel()
            return nil
        }
        guard !transcript.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return nil }
        let words = lead + transcript
        let ns = content as NSString
        guard NSMaxRange(range) <= ns.length else {
            cancel()
            return nil
        }
        content = ns.replacingCharacters(in: range, with: words)
        range.length = (words as NSString).length
        expected = content
        hasPartial = true
        return NSMaxRange(range)
    }

    /// Ends the dictation in `phase`; true when the pending send goes ahead.
    mutating func complete(_ phase: Phase) -> Bool {
        let send = pendingSend && phase == .idle
        cancel()
        self.phase = phase
        return send
    }
}

/// The phone's microphone, streamed to the chat's host as mono PCM16 at the
/// model's 16 kHz; the host's words come back over the same relay.
@MainActor
final class RemoteDictation: DictationTranscriber {
    var onEvent: ((DictationEvent) -> Void)?

    private let target: DictationTarget
    private var handle: HostDictation?
    private var bridge: DictationBridge?
    private let engine = AVAudioEngine()
    private var converter: AVAudioConverter?
    private let peak = Peak()
    private var observers: [NSObjectProtocol] = []
    private var finished = false
    private var concluded = false
    private var opened = false

    static let microphoneDenied = "Allow Microphone access for Zeron in Settings, then try again."
    /// Mono PCM16 at the host model's rate.
    nonisolated static let format = AVAudioFormat(commonFormat: .pcmFormatInt16, sampleRate: 16_000, channels: 1, interleaved: true)!

    init(target: DictationTarget) {
        self.target = target
    }

    func start() {
        Task { @MainActor [weak self] in
            guard let self else { return }
            let microphone = await AVAudioApplication.requestRecordPermission()
            guard !self.concluded else { return }
            guard microphone else { return self.conclude(.denied(Self.microphoneDenied)) }
            // Let go while permission was being asked: nothing was heard.
            if self.finished { return self.conclude(.final("")) }
            let bridge = DictationBridge { [weak self] event in self?.heard(event) }
            self.bridge = bridge
            self.handle = self.target.client.startDictation(hostDeviceId: self.target.hostId, listener: bridge)
            do {
                try self.open()
            } catch {
                self.conclude(.failed("Couldn't open the microphone."))
            }
        }
    }

    private func open() throws {
        let session = AVAudioSession.sharedInstance()
        try session.setCategory(.playAndRecord, mode: .default, options: [.duckOthers, .allowBluetoothHFP, .defaultToSpeaker])
        try session.setActive(true)
        let input = engine.inputNode
        let native = input.outputFormat(forBus: 0)
        guard native.sampleRate > 0, native.channelCount > 0, let converter = AVAudioConverter(from: native, to: Self.format) else {
            throw Failure.unavailable
        }
        self.converter = converter
        let peak = self.peak
        let handle = self.handle
        input.installTap(onBus: 0, bufferSize: 1024, format: native) { buffer, _ in
            peak.hear(buffer)
            guard let pcm = Self.convert(buffer, with: converter) else { return }
            handle?.pushAudio(pcm16le: pcm)
        }
        engine.prepare()
        try engine.start()
        opened = true
        for name in [AVAudioSession.interruptionNotification, UIApplication.didEnterBackgroundNotification] {
            observers.append(NotificationCenter.default.addObserver(forName: name, object: nil, queue: .main) { [weak self] notification in
                if notification.name == AVAudioSession.interruptionNotification,
                   (notification.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt) != AVAudioSession.InterruptionType.began.rawValue {
                    return
                }
                MainActor.assumeIsolated { self?.interrupted() }
            })
        }
        // A route change (a headset unplugged) reconfigures the engine: the
        // capture ends there, as interrupted, rather than carrying on with
        // an unknown input.
        observers.append(NotificationCenter.default.addObserver(forName: .AVAudioEngineConfigurationChange, object: engine, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.interrupted() }
        })
    }

    /// Resamples a native buffer to the model's format; the bytes are
    /// handed to the core at once and not kept.
    private nonisolated static func convert(_ buffer: AVAudioPCMBuffer, with converter: AVAudioConverter) -> Data? {
        let ratio = format.sampleRate / buffer.format.sampleRate
        let capacity = AVAudioFrameCount(Double(buffer.frameLength) * ratio) + 32
        guard let out = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: capacity) else { return nil }
        var consumed = false
        var error: NSError?
        converter.convert(to: out, error: &error) { _, status in
            if consumed {
                status.pointee = .noDataNow
                return nil
            }
            consumed = true
            status.pointee = .haveData
            return buffer
        }
        guard error == nil, out.frameLength > 0, let bytes = out.int16ChannelData else { return nil }
        return Data(bytes: bytes[0], count: Int(out.frameLength) * MemoryLayout<Int16>.size)
    }

    private func heard(_ event: DictationEvent) {
        guard !concluded else { return }
        switch event {
        case .listening, .partial, .finalizing:
            onEvent?(event)
        case .final, .failed, .unavailable:
            conclude(event)
        case .denied, .interrupted:
            conclude(event)
        }
    }

    private func interrupted() {
        guard opened, !finished, !concluded else { return }
        onEvent?(.interrupted)
        finish()
    }

    func finish() {
        guard !finished, !concluded else { return }
        finished = true
        guard opened else { return }
        closeCapture()
        handle?.finish()
    }

    func cancel() {
        guard !concluded else { return }
        concluded = true
        closeCapture()
        handle?.cancel()
        handle = nil
        bridge = nil
    }

    func takeLevel() -> Float { peak.take() }

    private func conclude(_ event: DictationEvent) {
        guard !concluded else { return }
        let deliver = onEvent
        cancel()
        deliver?(event)
    }

    private func closeCapture() {
        observers.forEach(NotificationCenter.default.removeObserver)
        observers.removeAll()
        guard opened else { return }
        opened = false
        engine.inputNode.removeTap(onBus: 0)
        engine.stop()
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
    }

    enum Failure: Error { case unavailable }

    /// The loudest sample since last read; written on the audio thread.
    private final class Peak: @unchecked Sendable {
        private let lock = NSLock()
        private var value: Float = 0

        func hear(_ buffer: AVAudioPCMBuffer) {
            guard let channels = buffer.floatChannelData, buffer.frameLength > 0 else { return }
            var loudest: Float = 0
            for channel in 0..<Int(buffer.format.channelCount) {
                let samples = channels[channel]
                for i in 0..<Int(buffer.frameLength) { loudest = max(loudest, abs(samples[i])) }
            }
            lock.lock()
            value = max(value, loudest)
            lock.unlock()
        }

        func take() -> Float {
            lock.lock()
            defer { value = 0; lock.unlock() }
            return value
        }
    }

    /// What the host says, hopped to the main actor and put in the
    /// transcriber's terms.
    private final class DictationBridge: HostDictationListener, @unchecked Sendable {
        private let deliver: @MainActor (DictationEvent) -> Void
        init(_ deliver: @escaping @MainActor (DictationEvent) -> Void) { self.deliver = deliver }

        func onDictationEvent(event: HostDictationEvent) {
            let mapped: DictationEvent
            switch event {
            case .listening: mapped = .listening
            case let .partial(text): mapped = .partial(text)
            case .finalizing: mapped = .finalizing
            case let .final(text): mapped = .final(text)
            case let .failed(message): mapped = .failed(message)
            case let .unavailable(reason): mapped = .unavailable(RemoteDictation.explain(reason))
            }
            Task { @MainActor in self.deliver(mapped) }
        }
    }

    nonisolated static func explain(_ reason: DictationUnavailable) -> String {
        switch reason {
        case .modelMissing: "Download the speech model in Settings → Voice on the host, then try again."
        case .busy: "The host is already taking a dictation."
        case .hostUnreachable: "The host isn't reachable right now."
        case .hostIncompatible: "Update Zeron on the host to dictate from here."
        }
    }
}

#if DEBUG
/// A dictation made up, for looking at its surfaces without a microphone
/// or a host (`-dictation-preview`): words stream in as a live transcript
/// does, over a voice that rises and falls, until stopped.
@MainActor
final class ScriptedDictation: DictationTranscriber {
    var onEvent: ((DictationEvent) -> Void)?
    private static let words = "Run the parser tests and open a pull request once they pass".split(separator: " ").map(String.init)
    private var started: Date?
    private var said = 0
    private var timer: Timer?
    private var finished = false

    func start() {
        started = Date()
        onEvent?(.listening)
        timer = Timer.scheduledTimer(withTimeInterval: 0.1, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.tick() }
        }
    }

    private func tick() {
        guard let started, !finished else { return }
        let due = min(Self.words.count, Int(Date().timeIntervalSince(started) * 2.5))
        guard due > said else { return }
        said = due
        onEvent?(.partial(Self.words.prefix(due).joined(separator: " ")))
    }

    func finish() {
        guard !finished else { return }
        finished = true
        timer?.invalidate()
        let words = Self.words.prefix(max(said, 1)).joined(separator: " ")
        DispatchQueue.main.asyncAfter(deadline: .now() + 3) { [weak self] in
            guard let self, self.finished else { return }
            self.onEvent?(.final(words))
        }
    }

    func cancel() {
        finished = true
        timer?.invalidate()
        onEvent = nil
    }

    func takeLevel() -> Float {
        guard let started, !finished else { return 0 }
        let t = Float(Date().timeIntervalSince(started))
        return 0.03 + 0.3 * max(0, sin(t * 4.7 * .pi * 2))
    }
}
#endif
