import AVFoundation
import Observation
import Speech

enum DictationFailure: Error, Equatable {
    case permissionDenied
    case unavailable
    case failed

    var message: String {
        switch self {
        case .permissionDenied: "Allow Microphone and Speech Recognition for Zeron in Settings to dictate."
        case .unavailable: "On-device dictation is unavailable for this language or device. You can keep typing."
        case .failed: "Dictation stopped. Your draft is still here; tap the microphone to try again."
        }
    }
}

enum DictationEvent {
    case listening
    case transcript(String, final: Bool)
    case failure(DictationFailure)
}

@MainActor
protocol ComposerTranscriber: AnyObject {
    func start(receive: @escaping @MainActor (DictationEvent) -> Void)
    func finish()
    func cancel()
}

/// Frontend-only lifetime. Session tokens reject callbacks from stopped capture,
/// permissions arriving after navigation, and callbacks after sending a draft.
@MainActor @Observable
final class ComposerDictation {
    enum State: Equatable {
        case idle, requestingPermission, listening, finalizing
        case error(DictationFailure)
    }

    private(set) var state: State = .idle
    var active: Bool {
        switch state {
        case .requestingPermission, .listening, .finalizing: true
        default: false
        }
    }
    private let transcriber: any ComposerTranscriber
    private weak var editor: ComposerEditorController?
    private var generation = UUID()
    private var timeout: Task<Void, Never>?
    private var completion: (() -> Void)?

    init(transcriber: (any ComposerTranscriber)? = nil) {
        self.transcriber = transcriber ?? NativeComposerTranscriber()
    }

    func start(editor: ComposerEditorController) {
        guard !active, editor.beginDictation() else { return }
        self.editor = editor
        editor.dictationInterrupted = { [weak self] in self?.cancel() }
        generation = UUID()
        let token = generation
        state = .requestingPermission
        transcriber.start { [weak self] event in
            guard let self, self.generation == token, self.active else { return }
            switch event {
            case .listening:
                if self.state == .requestingPermission { self.state = .listening }
            case .transcript(let text, let final):
                // No-speech completion must not erase the original selection
                // (or a useful partial received before an empty final result).
                if !text.isEmpty { self.editor?.replaceDictation(with: text) }
                if final { self.complete() }
            case .failure(let error):
                self.complete(error: error)
            }
        }
    }

    func finish(then action: (() -> Void)? = nil) {
        guard active else { action?(); return }
        guard state != .finalizing else { return }
        completion = action
        if state == .requestingPermission { complete(); return }
        state = .finalizing
        let token = generation
        timeout = Task { [weak self] in
            try? await Task.sleep(for: .seconds(2))
            guard !Task.isCancelled, let self, self.generation == token else { return }
            self.complete()
        }
        transcriber.finish()
    }

    /// Navigation, editing and interruption preserve the latest text but never
    /// execute a pending send against a different draft or conversation.
    func cancel() {
        completion = nil
        complete()
    }

    private func complete(error: DictationFailure? = nil) {
        generation = UUID()
        timeout?.cancel()
        timeout = nil
        transcriber.cancel()
        editor?.endDictation()
        editor?.dictationInterrupted = {}
        editor = nil
        state = error.map(State.error) ?? .idle
        let action = completion
        completion = nil
        // A failure preserves the draft for review, rather than sending it.
        if error == nil { action?() }
    }
}

@MainActor
final class NativeComposerTranscriber: ComposerTranscriber {
    private var engine: AVAudioEngine?
    private var recognizer: SFSpeechRecognizer?
    private var request: SFSpeechAudioBufferRecognitionRequest?
    private var recognition: SFSpeechRecognitionTask?
    private var permissionTask: Task<Void, Never>?
    private var observers: [NSObjectProtocol] = []
    private var audioSessionActive = false
    private var token = UUID()

    func start(receive: @escaping @MainActor (DictationEvent) -> Void) {
        cancel()
        let token = self.token
        permissionTask = Task { [weak self] in
            let speech = await withCheckedContinuation { continuation in
                SFSpeechRecognizer.requestAuthorization { continuation.resume(returning: $0) }
            }
            guard let self, !Task.isCancelled, self.token == token else { return }
            guard speech == .authorized else { receive(.failure(.permissionDenied)); return }
            let microphone = await AVAudioApplication.requestRecordPermission()
            guard !Task.isCancelled, self.token == token else { return }
            guard microphone else { receive(.failure(.permissionDenied)); return }
            guard let recognizer = SFSpeechRecognizer(locale: .current),
                  recognizer.supportsOnDeviceRecognition, recognizer.isAvailable else {
                receive(.failure(.unavailable)); return
            }
            do {
                let session = AVAudioSession.sharedInstance()
                try session.setCategory(.record, mode: .measurement, options: .duckOthers)
                try session.setActive(true)
                self.audioSessionActive = true
                let engine = AVAudioEngine()
                let input = engine.inputNode
                let format = input.outputFormat(forBus: 0)
                guard format.sampleRate > 0, format.channelCount > 0 else {
                    self.cancel(); receive(.failure(.unavailable)); return
                }
                let request = SFSpeechAudioBufferRecognitionRequest()
                request.requiresOnDeviceRecognition = true
                request.shouldReportPartialResults = true
                self.engine = engine
                self.recognizer = recognizer
                self.request = request
                input.installTap(onBus: 0, bufferSize: 1024, format: format) { buffer, _ in
                    request.append(buffer)
                }
                self.recognition = recognizer.recognitionTask(with: request) { [weak self] result, error in
                    let text = result?.bestTranscription.formattedString
                    let final = result?.isFinal ?? false
                    let failed = error != nil
                    Task { @MainActor in
                        guard let self, self.token == token else { return }
                        if let text { receive(.transcript(text, final: final)) }
                        if failed, !final { receive(.failure(.failed)) }
                    }
                }
                for name in [AVAudioSession.interruptionNotification,
                             AVAudioSession.routeChangeNotification,
                             AVAudioSession.mediaServicesWereResetNotification] {
                    self.observers.append(NotificationCenter.default.addObserver(
                        forName: name, object: session, queue: .main
                    ) { [weak self] _ in
                        Task { @MainActor in
                            guard let self, self.token == token else { return }
                            self.cancel()
                            receive(.failure(.failed))
                        }
                    })
                }
                engine.prepare()
                try engine.start()
                receive(.listening)
            } catch {
                self.cancel()
                receive(.failure(.failed))
            }
        }
    }

    func finish() {
        releaseCapture()
        request?.endAudio()
    }

    func cancel() {
        token = UUID()
        permissionTask?.cancel()
        permissionTask = nil
        releaseCapture()
        recognition?.cancel()
        recognition = nil
        request = nil
        recognizer = nil
    }

    private func releaseCapture() {
        observers.forEach(NotificationCenter.default.removeObserver)
        observers = []
        if let engine {
            engine.stop()
            engine.inputNode.removeTap(onBus: 0)
            self.engine = nil
        }
        if audioSessionActive {
            try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
            audioSessionActive = false
        }
    }
}
