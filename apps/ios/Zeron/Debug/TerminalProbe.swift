#if DEBUG
import UIKit

/// `-e2e-terminal`: a live terminal probe against a real host (the dev
/// stack). Creates a project-less chat on the first online host, opens a
/// terminal on it, then measures time to first output, when the terminal
/// moves onto the direct link, and keystroke → echo latency on each path.
/// Writes `Documents/e2e-terminal.json` and leaves the terminal on screen.
@MainActor
enum TerminalProbe {
    private static var log: [String: Any] = [:]
    private static var events: [String] = []

    static func runIfAsked(app: AppModel, router: AppRouter?) {
        guard ProcessInfo.processInfo.arguments.contains("-e2e-terminal") else { return }
        Task { await run(app: app, router: router) }
    }

    private static func note(_ event: String, since start: Date) {
        let ms = Int(Date().timeIntervalSince(start) * 1000)
        events.append("\(ms)ms \(event)")
        NSLog("E2E-TERMINAL %@", "\(ms)ms \(event)")
        save()
    }

    private static func save() {
        log["events"] = events
        let docs = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
        if let data = try? JSONSerialization.data(withJSONObject: log, options: [.prettyPrinted, .sortedKeys]) {
            try? data.write(to: docs.appendingPathComponent("e2e-terminal.json"))
        }
    }

    private static func sessionScreen(_ chatId: String) -> SessionViewController? {
        func search(_ vc: UIViewController?) -> SessionViewController? {
            guard let vc else { return nil }
            if let session = vc as? SessionViewController, session.chatId == chatId { return session }
            for child in vc.children { if let found = search(child) { return found } }
            return search(vc.presentedViewController)
        }
        let windows = UIApplication.shared.connectedScenes.compactMap { ($0 as? UIWindowScene)?.keyWindow }
        return windows.lazy.compactMap { search($0.rootViewController) }.first
    }

    private static func run(app: AppModel, router: AppRouter?) async {
        let start = Date()
        note("probe started", since: start)
        var host: HostOption?
        for _ in 0..<240 {
            host = app.hostOptions.first(where: \.online)
            if host != nil { break }
            try? await Task.sleep(for: .milliseconds(250))
        }
        guard let host else { return note("no online host", since: start) }
        note("host online: \(host.id)", since: start)
        var draft = NewSessionDraft()
        draft.hostId = host.id
        draft.harness = "mock"
        guard let chatId = app.createSession(draft: draft, text: "e2e terminal", images: []) else {
            return note("couldn't create a chat", since: start)
        }
        note("chat \(chatId)", since: start)
        let opened = Date()
        let tab: TerminalTab
        do {
            tab = try await app.terminals.open(chatId: chatId, cols: 80, rows: 24)
        } catch {
            return note("open failed: \(error)", since: start)
        }
        note("terminal open (\(Int(Date().timeIntervalSince(opened) * 1000))ms)", since: start)
        router?.openSession(chatId)
        // Put the terminal itself on screen, as a person would open it.
        Task {
            try? await Task.sleep(for: .milliseconds(700))
            sessionScreen(chatId)?.openTerminal()
        }
        var firstOutput = false
        var direct = false
        var token: AnyObject?
        token = tab.observe {
            let text = tab.session.text(all: false)
            if !firstOutput, !text.isEmpty {
                firstOutput = true
                note("first output (\(Int(Date().timeIntervalSince(opened) * 1000))ms after open)", since: start)
            }
            if tab.direct != direct {
                direct = tab.direct
                note(direct ? "path: DIRECT" : "path: relay", since: start)
            }
        }
        _ = token
        // Keystroke → echo: a run while the terminal is on the relay (it
        // starts there unless the pair was already up), then a run once it
        // rides the direct link.
        var samples: [(path: String, ms: Double)] = []
        func sample(_ count: Int) async {
            for i in 0..<count {
                let before = tab.session.text(all: false).filter { $0 == "Q" }.count
                let path = tab.direct ? "direct" : "relay"
                let sent = CACurrentMediaTime()
                tab.session.typeText(text: "Q", modifiers: KeyModifiers(ctrl: false, alt: false, shift: false))
                for _ in 0..<3000 {
                    // The predictor paints ahead in the frame, not the text:
                    // this counts the host's own echo.
                    if tab.session.text(all: false).filter({ $0 == "Q" }).count > before {
                        samples.append((path, (CACurrentMediaTime() - sent) * 1000))
                        break
                    }
                    try? await Task.sleep(for: .milliseconds(1))
                }
                try? await Task.sleep(for: .milliseconds(120))
                if i % 10 == 9 {
                    tab.session.typeText(text: "u", modifiers: KeyModifiers(ctrl: true, alt: false, shift: false))
                    try? await Task.sleep(for: .milliseconds(150))
                }
            }
        }
        try? await Task.sleep(for: .milliseconds(300))
        if !tab.direct { await sample(10) }
        for _ in 0..<200 where !tab.direct { try? await Task.sleep(for: .milliseconds(100)) }
        if tab.direct { await sample(40) }
        tab.session.typeText(text: "u", modifiers: KeyModifiers(ctrl: true, alt: false, shift: false))
        for path in ["relay", "direct"] {
            let ms = samples.filter { $0.path == path }.map(\.ms).sorted()
            guard !ms.isEmpty else { continue }
            log["\(path)_echo_ms"] = [
                "n": ms.count,
                "p50": ms[ms.count / 2],
                "p90": ms[min(ms.count - 1, ms.count * 9 / 10)],
                "min": ms[0],
            ]
        }
        note("samples done", since: start)
        tab.session.typeText(text: "echo e2e-$((6*7))\n", modifiers: KeyModifiers(ctrl: false, alt: false, shift: false))
        try? await Task.sleep(for: .seconds(1))
        log["ran_command"] = tab.session.text(all: false).contains("e2e-42")
        note("done", since: start)
    }
}
#endif
