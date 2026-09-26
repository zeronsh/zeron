import UIKit

/// `-lab`: the real layout engine + painter over fixture markdown, with a
/// scripted stream and an on-screen hitch meter. Used for visual iteration
/// and for the scroll/stream benchmarks (`-lab -turns 300 -autostream`).
final class TranscriptLabViewController: UIViewController {
    private lazy var relay = FrameRelay { [weak self] in self?.applyFrame() }
    private lazy var engine = TranscriptView(text: TextEngine.shared, listener: relay)
    private lazy var list = TranscriptListView(engine: engine)
    private let composer = ComposerBar()
    private let meter = HitchMeter()
    private var entries: [DebugEntry] = []
    private var streamTimer: Timer?
    private let fixture = layoutFixtureMarkdown()

    override func viewDidLoad() {
        super.viewDidLoad()
        title = "Transcript Lab"
        view.backgroundColor = Palette.background
        list.frame = view.bounds
        list.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        view.addSubview(list)
        setContentScrollView(list, for: .top)
        navigationItem.largeTitleDisplayMode = .never

        composer.translatesAutoresizingMaskIntoConstraints = false
        composer.placeholder = "Message Claude"
        composer.chips = [
            ComposerChip(id: "model", title: "Opus 4.5", symbol: nil),
            ComposerChip(id: "effort", title: "High", symbol: "gauge.with.dots.needle.67percent"),
            ComposerChip(id: "branch", title: "ios-rewrite", symbol: "arrow.triangle.branch"),
        ]
        composer.attachMenu = { [weak self] in
            guard let self else { return UIMenu() }
            return AttachmentPicker.menu(host: self, limit: 8) { [weak self] in self?.composer.addImages($0) }
        }
        composer.onSend = { [weak self] text, _, _ in self?.send(text) }
        composer.onStop = { [weak self] in self?.stopStream() }
        composer.onHeightChange = { [weak self] in self?.view.setNeedsLayout() }
        view.addSubview(composer)
        NSLayoutConstraint.activate([
            composer.leadingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.leadingAnchor, constant: 12),
            composer.trailingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.trailingAnchor, constant: -12),
            composer.bottomAnchor.constraint(equalTo: view.keyboardLayoutGuide.topAnchor, constant: -8),
        ])
        meter.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(meter)
        NSLayoutConstraint.activate([
            meter.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor, constant: 4),
            meter.trailingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.trailingAnchor, constant: -12),
        ])

        let args = ProcessInfo.processInfo.arguments
        let turns = args.firstIndex(of: "-turns").flatMap { Int(args[$0 + 1]) } ?? 6
        for i in 0..<turns {
            entries.append(DebugEntry(id: "u\(i)", user: true, text: Self.prompts[i % Self.prompts.count], streaming: false))
            entries.append(DebugEntry(id: "a\(i)", user: false, text: fixture, streaming: false))
        }
        engine.setDebugEntries(entries: entries, working: false)
        if args.contains("-autostream") {
            DispatchQueue.main.asyncAfter(deadline: .now() + 1) { [weak self] in self?.send("Stream the plan again, slowly.") }
        }
        if args.contains("-top") {
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.6) { [weak self] in
                guard let self else { return }
                self.list.setContentOffset(CGPoint(x: 0, y: -self.list.adjustedContentInset.top), animated: false)
            }
        }
        if args.contains("-bench") {
            DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { [weak self] in self?.runBench() }
        }
        if args.contains("-autoscroll") {
            DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { [weak self] in self?.autoscroll() }
        }
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        list.settleEdgeEffect()
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        let covered = view.bounds.maxY - composer.frame.minY + 8
        let inset = max(0, covered - view.safeAreaInsets.bottom)
        if abs(list.contentInset.bottom - inset) > 0.5 {
            let bottom = list.following
            list.contentInset.bottom = inset
            list.verticalScrollIndicatorInsets.bottom = inset
            if bottom { list.contentOffset.y = list.maxOffsetY }
        }
    }

    private func applyFrame() {
        list.apply(engine.frame())
        meter.layoutMicros = engine.frame().buildMicros()
    }

    private func send(_ text: String) {
        stopStream()
        let n = entries.count
        entries.append(DebugEntry(id: "u\(n)", user: true, text: text, streaming: false))
        entries.append(DebugEntry(id: "a\(n)", user: false, text: "", streaming: true))
        list.scrollToBottom(animated: true)
        composer.running = true
        let words = fixture.split(separator: " ", omittingEmptySubsequences: false).map(String.init)
        var i = 0
        streamTimer = Timer.scheduledTimer(withTimeInterval: 0.045, repeats: true) { [weak self] timer in
            guard let self else { return timer.invalidate() }
            let step = Int.random(in: 2...5)
            let next = words[i..<min(words.count, i + step)].joined(separator: " ")
            i += step
            var last = self.entries[self.entries.count - 1]
            last.text += (last.text.isEmpty ? "" : " ") + next
            last.streaming = i < words.count
            self.entries[self.entries.count - 1] = last
            self.engine.setDebugEntries(entries: self.entries, working: last.streaming)
            if !last.streaming { self.stopStream() }
        }
    }

    private func stopStream() {
        streamTimer?.invalidate()
        streamTimer = nil
        composer.running = false
        if var last = entries.last, last.streaming {
            last.streaming = false
            entries[entries.count - 1] = last
            engine.setDebugEntries(entries: entries, working: false)
        }
    }

    /// Scripted fling up then back down (benchmark mode).
    private func autoscroll() {
        let top = -list.adjustedContentInset.top
        UIView.animate(withDuration: 4, delay: 0, options: [.curveEaseInOut]) {
            self.list.contentOffset.y = top
        } completion: { _ in
            UIView.animate(withDuration: 4, delay: 0.3, options: [.curveEaseInOut]) {
                self.list.contentOffset.y = self.list.maxOffsetY
            }
        }
    }

    // MARK: Benchmark

    private let benchLabel = UILabel()
    private var bench: ScrollBench?

    /// Display-link driven flings (like a real scroll: every frame runs
    /// layoutSubviews and realizes rows), idle and while streaming.
    private func runBench() {
        benchLabel.accessibilityIdentifier = "bench-result"
        benchLabel.numberOfLines = 0
        benchLabel.font = .monospacedSystemFont(ofSize: 10, weight: .regular)
        benchLabel.frame = CGRect(x: 12, y: 120, width: view.bounds.width - 24, height: 80)
        view.addSubview(benchLabel)
        let idle = ScrollBench(list: list)
        bench = idle
        idle.run(passes: 3) { [weak self] idleResult in
            guard let self else { return }
            self.send("Stream the plan again while I scroll.")
            let streaming = ScrollBench(list: self.list)
            self.bench = streaming
            streaming.run(passes: 2) { streamResult in
                let json = "{\"idle\":\(idleResult.json),\"streaming\":\(streamResult.json)}"
                NSLog("BENCH %@", json)
                self.benchLabel.text = json
                self.benchLabel.accessibilityLabel = json
            }
        }
    }

    static let prompts = [
        "How does the layout engine avoid measuring text on the main thread?",
        "Show me the plan again with the code sample and the table.",
        "Can you explain why rows never jump when content streams in above the viewport?",
    ]
}

/// Frame-pacing meter: counts frames that missed their deadline.
final class HitchMeter: UILabel {
    private var link: CADisplayLink?
    private var last: CFTimeInterval = 0
    private var hitches = 0
    private var frames = 0
    var layoutMicros: UInt64 = 0

    override init(frame: CGRect) {
        super.init(frame: frame)
        font = .monospacedDigitSystemFont(ofSize: 11, weight: .medium)
        textColor = Palette.secondary
        backgroundColor = Palette.elevated.withAlphaComponent(0.85)
        layer.cornerRadius = 6
        layer.masksToBounds = true
        textAlignment = .center
        isHidden = !ProcessInfo.processInfo.arguments.contains("-meter")
        let link = CADisplayLink(target: self, selector: #selector(tick(_:)))
        link.preferredFrameRateRange = CAFrameRateRange(minimum: 80, maximum: 120, preferred: 120)
        link.add(to: .main, forMode: .common)
        self.link = link
    }

    required init?(coder: NSCoder) { fatalError() }

    @objc private func tick(_ link: CADisplayLink) {
        let budget = link.targetTimestamp - link.timestamp
        if last > 0, link.timestamp - last > budget * 1.5 { hitches += 1 }
        last = link.timestamp
        frames += 1
        if frames % 30 == 0 {
            text = String(format: " %d hitches · layout %.1fms ", hitches, Double(layoutMicros) / 1000)
        }
    }
}

/// Hitch accounting in Apple's terms: time a frame was late beyond its
/// deadline, summed, per second of scrolling (ms/s; < 5 good, > 10 critical).
final class ScrollBench {
    struct Result {
        var frames = 0
        var hitches = 0
        var hitchMs = 0.0
        var seconds = 0.0
        var worstMs = 0.0
        var json: String {
            String(format: "{\"frames\":%d,\"hitches\":%d,\"hitchRatioMsPerS\":%.2f,\"worstFrameMs\":%.1f,\"seconds\":%.2f}", frames, hitches, seconds > 0 ? hitchMs / seconds : 0, worstMs, seconds)
        }
    }

    private weak var list: TranscriptListView?
    private var link: CADisplayLink?
    private var last: CFTimeInterval = 0
    private var velocity: CGFloat = 0
    private var direction: CGFloat = -1
    private var passes = 0
    private var result = Result()
    private var done: ((Result) -> Void)?

    init(list: TranscriptListView) { self.list = list }

    func run(passes: Int, done: @escaping (Result) -> Void) {
        list?.releaseFollow()
        self.passes = passes * 2
        self.done = done
        startFling()
        let link = CADisplayLink(target: self, selector: #selector(tick(_:)))
        link.preferredFrameRateRange = CAFrameRateRange(minimum: 60, maximum: 120, preferred: 120)
        link.add(to: .main, forMode: .common)
        self.link = link
    }

    private func startFling() {
        velocity = 5200 // pt/s, a hard flick
        direction = -direction
    }

    @objc private func tick(_ link: CADisplayLink) {
        guard let list else { return link.invalidate() }
        if last > 0 {
            let dt = link.timestamp - last
            let budget = link.targetTimestamp - link.timestamp
            result.frames += 1
            result.seconds += dt
            let late = dt - budget
            if late > budget * 0.5 {
                result.hitches += 1
                result.hitchMs += late * 1000
            }
            result.worstMs = max(result.worstMs, dt * 1000)
            // UIScrollView-like deceleration.
            velocity *= CGFloat(pow(0.998, dt * 1000))
            let top = -list.adjustedContentInset.top
            var y = list.contentOffset.y + direction * velocity * CGFloat(dt)
            y = min(max(y, top), list.maxOffsetY)
            list.contentOffset.y = y
            if velocity < 60 || y <= top || y >= list.maxOffsetY {
                passes -= 1
                if passes <= 0 {
                    link.invalidate()
                    done?(result)
                    return
                }
                startFling()
            }
        }
        last = link.timestamp
    }
}
