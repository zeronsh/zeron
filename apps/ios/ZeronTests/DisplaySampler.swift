#if DEBUG
import UIKit
import XCTest

/// Captures presentation geometry inside the display callback, before the test
/// applies its next stimulus. Call stop() when using the sampler directly.
@MainActor
final class DisplaySampler: NSObject {
    private var link: CADisplayLink?
    private let capture: (CADisplayLink) -> Void

    init(capture: @escaping (CADisplayLink) -> Void) {
        self.capture = capture
        super.init()
        let link = CADisplayLink(target: self, selector: #selector(tick(_:)))
        self.link = link
        link.add(to: .main, forMode: .common)
    }

    func stop() {
        link?.invalidate()
        link = nil
    }

    @objc private func tick(_ link: CADisplayLink) { capture(link) }

    /// sample returns true only once the scenario reaches its observed endpoint.
    /// Return the outcome so callers can attach diagnostics before asserting it.
    static func observe(_ description: String, timeout: TimeInterval = 5,
                        sample: @escaping (CADisplayLink) -> Bool) async -> Bool {
        let completed = XCTestExpectation(description: description)
        var finished = false
        let sampler = DisplaySampler { link in
            guard !finished else { return }
            if sample(link) {
                finished = true
                completed.fulfill()
            }
        }
        defer { sampler.stop() }
        return await XCTWaiter.fulfillment(of: [completed], timeout: timeout) == .completed
    }
}

/// For final-state assertions that do not need to observe an animation.
@MainActor
func waitForTestCondition(timeout: Duration = .seconds(5), _ condition: () -> Bool) async -> Bool {
    let clock = ContinuousClock()
    let deadline = clock.now.advanced(by: timeout)
    while !condition() {
        guard clock.now < deadline, !Task.isCancelled else { return false }
        do { try await clock.sleep(for: .milliseconds(10)) }
        catch { return false }
    }
    return true
}
#endif
