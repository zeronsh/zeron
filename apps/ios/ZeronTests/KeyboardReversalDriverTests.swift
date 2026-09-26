#if DEBUG
import XCTest

final class KeyboardReversalDriverTests: XCTestCase {
    private func start(_ driver: inout KeyboardReversalDriver) {
        XCTAssertEqual(driver.observe(position: 800, target: 800, isFirstResponder: false), .setShowing(true))
    }

    private func finishReversals(_ driver: inout KeyboardReversalDriver, from start: CGFloat = 800) {
        var position = start
        for _ in driver.reversals..<5 {
            let target: CGFloat = driver.showing ? 400 : 800
            position = (position + target) / 2
            let wasShowing = driver.showing
            XCTAssertEqual(driver.observe(position: position, target: target, isFirstResponder: wasShowing),
                           .setShowing(!wasShowing))
        }
        XCTAssertEqual(driver.phase, .finishing)
        XCTAssertNil(driver.observe(position: 800, target: 800, isFirstResponder: true))
        XCTAssertNil(driver.observe(position: 800, target: 800, isFirstResponder: false))
        // An unstable sample must reset the final endpoint observation count.
        XCTAssertNil(driver.observe(position: 810, target: 800, isFirstResponder: false))
        XCTAssertNil(driver.observe(position: 800, target: 800, isFirstResponder: false))
        XCTAssertNil(driver.observe(position: 800, target: 800, isFirstResponder: false))
        XCTAssertEqual(driver.observe(position: 800, target: 800, isFirstResponder: false), .finished)
        XCTAssertEqual(driver.phase, .complete)
        XCTAssertFalse(driver.showing)
    }

    func testNormalInFlightSequenceFinishesHidden() {
        var driver = KeyboardReversalDriver(hiddenPosition: 800)
        start(&driver)
        // A stale model endpoint or a frame before 45% is not a reversal.
        XCTAssertNil(driver.observe(position: 800, target: 800, isFirstResponder: true))
        XCTAssertNil(driver.observe(position: 700, target: 400, isFirstResponder: true))
        XCTAssertEqual(driver.reversals, 0)
        finishReversals(&driver)
        XCTAssertEqual(driver.recoveries, 0)
    }

    func testMissedWindowRecoversInBothDirectionsWithoutCountingSetup() {
        // Cover landing exactly at the endpoint, overshooting, and entering its tolerance.
        for overshoot: CGFloat in [0, 15, -0.5] {
            for showing in [true, false] {
                var driver = KeyboardReversalDriver(hiddenPosition: 800)
                start(&driver)
                if !showing {
                    XCTAssertEqual(driver.observe(position: 600, target: 400, isFirstResponder: true), .setShowing(false))
                }
                let count = driver.reversals
                let target: CGFloat = showing ? 400 : 800
                let source: CGFloat = showing ? 800 : 400
                let before: CGFloat = showing ? 700 : 650
                XCTAssertNil(driver.observe(position: before, target: target, isFirstResponder: showing))
                XCTAssertNil(driver.observe(position: target + (showing ? -overshoot : overshoot),
                                            target: target, isFirstResponder: showing))
                XCTAssertEqual(driver.phase, .missedEndpoint)
                XCTAssertEqual(driver.recoveries, 1)
                for _ in 0..<2 {
                    XCTAssertNil(driver.observe(position: target, target: target, isFirstResponder: showing))
                }
                XCTAssertEqual(driver.observe(position: target, target: target, isFirstResponder: showing), .setShowing(!showing))
                XCTAssertEqual(driver.phase, .resetting)
                // Setup may cross the eligible interval; it still must not count.
                XCTAssertNil(driver.observe(position: 600, target: source, isFirstResponder: !showing))
                for _ in 0..<2 {
                    XCTAssertNil(driver.observe(position: source, target: source, isFirstResponder: !showing))
                }
                XCTAssertEqual(driver.observe(position: source, target: source, isFirstResponder: !showing), .setShowing(showing))
                XCTAssertEqual(driver.phase, .reversing)
                XCTAssertEqual(driver.reversals, count)
                finishReversals(&driver, from: source)
                XCTAssertEqual(driver.recoveries, 1)
            }
        }
    }

    func testExhaustedRecoveryBudgetFailsExplicitly() {
        var driver = KeyboardReversalDriver(hiddenPosition: 800, maximumRecoveries: 1)
        start(&driver)
        XCTAssertNil(driver.observe(position: 390, target: 400, isFirstResponder: true))
        for _ in 0..<3 { _ = driver.observe(position: 400, target: 400, isFirstResponder: true) }
        for _ in 0..<3 { _ = driver.observe(position: 800, target: 800, isFirstResponder: false) }
        XCTAssertEqual(driver.phase, .reversing)
        XCTAssertEqual(driver.observe(position: 400, target: 400, isFirstResponder: true), .finished)
        XCTAssertEqual(driver.phase, .failed)
        XCTAssertEqual(driver.reversals, 0)
        XCTAssertEqual(driver.recoveries, 1)
        XCTAssertTrue(driver.failure?.contains("recovery budget exhausted") == true)
    }
}
#endif
