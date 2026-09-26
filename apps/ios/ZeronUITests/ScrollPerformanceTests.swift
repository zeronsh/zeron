import XCTest

/// Hitch benchmarks (Apple's scrolling metric: hitch time ratio in ms/s).
/// Run on a device for real numbers; the simulator gives relative signal.
///
///   xcodebuild test -scheme Zeron -only-testing:ZeronUITests/ScrollPerformanceTests
final class ScrollPerformanceTests: XCTestCase {
    override func setUp() {
        continueAfterFailure = false
    }

    private func launchLab(_ extra: [String] = []) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments = ["-lab", "-turns", "300"] + extra
        app.launch()
        XCTAssertTrue(app.scrollViews.firstMatch.waitForExistence(timeout: 10))
        return app
    }

    /// Flinging through a 300-turn (≈3,300 row) transcript.
    func testFlingLongTranscript() {
        let app = launchLab()
        let list = app.scrollViews.firstMatch
        let options = XCTMeasureOptions()
        options.invocationOptions = [.manuallyStop]
        measure(metrics: [XCTOSSignpostMetric.scrollingAndDecelerationMetric], options: options) {
            list.swipeDown(velocity: .fast)
            list.swipeDown(velocity: .fast)
            list.swipeUp(velocity: .fast)
            list.swipeUp(velocity: .fast)
            stopMeasuring()
        }
    }

    /// Scrolling while a reply streams in (layout + paint under load).
    func testScrollWhileStreaming() {
        let app = launchLab(["-autostream"])
        let list = app.scrollViews.firstMatch
        let options = XCTMeasureOptions()
        options.invocationOptions = [.manuallyStop]
        options.iterationCount = 3
        measure(metrics: [XCTOSSignpostMetric.scrollingAndDecelerationMetric], options: options) {
            list.swipeDown(velocity: .slow)
            list.swipeUp(velocity: .fast)
            stopMeasuring()
        }
    }

    /// In-app frame pacing (works on simulators too): display-link flings
    /// through ~3,300 rows, idle and while streaming. Asserts Apple's "good"
    /// hitch ratio (< 5 ms of lateness per second of scrolling).
    func testHitchRatioInApp() throws {
        let app = XCUIApplication()
        app.launchArguments = ["-lab", "-turns", "300", "-bench"]
        app.launch()
        let result = app.staticTexts["bench-result"]
        XCTAssertTrue(result.waitForExistence(timeout: 120))
        let json = result.label
        let attachment = XCTAttachment(string: json)
        attachment.name = "bench.json"
        attachment.lifetime = .keepAlways
        add(attachment)
        print("BENCH \(json)")
        let parsed = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: [String: Double]])
        for (phase, metrics) in parsed {
            XCTAssertLessThan(metrics["hitchRatioMsPerS"] ?? 99, 5, "\(phase) hitch ratio")
        }
    }

    /// Cold launch into a long transcript.
    func testLaunchIntoLongTranscript() {
        measure(metrics: [XCTApplicationLaunchMetric(waitUntilResponsive: true)]) {
            let app = XCUIApplication()
            app.launchArguments = ["-lab", "-turns", "300"]
            app.launch()
        }
    }
}
