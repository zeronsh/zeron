import XCTest

/// Against a real stack: `wrangler dev` edge (AUTH_MODE=dev) + a headless
/// engine running the mock harness, both as alice@org1. Skipped unless
/// `TEST_RUNNER_ZERON_LIVE_EDGE=http://localhost:<port>` is set:
///
///   (cd edge && npx wrangler dev --port 27650 --var AUTH_MODE:dev) &
///   ZERON_DATA_DIR=/tmp/e ZERON_IPC_PORT=27811 ZERON_EDGE_URL=http://localhost:27650 \
///     ZERON_EDGE_TOKEN=alice@org1 ZERON_ORG_ID=org1 ZERON_HARNESS=mock zeron headless &
///   TEST_RUNNER_ZERON_LIVE_EDGE=http://localhost:27650 xcodebuild test \
///     -only-testing:ZeronUITests/LiveStackTests …
final class LiveStackTests: XCTestCase {
    func testSendRoundTripsThroughRealEngine() throws {
        let edge = try XCTUnwrap(ProcessInfo.processInfo.environment["ZERON_LIVE_EDGE"], "set TEST_RUNNER_ZERON_LIVE_EDGE")
        let app = XCUIApplication()
        app.launchArguments = ["-signedout", "-dev", "alice", "org1", "-edge", edge]
        app.launch()

        let accessory = app.buttons["new-session"]
        XCTAssertTrue(accessory.waitForExistence(timeout: 15))
        accessory.tap()
        let input = app.textViews["composer-input"]
        XCTAssertTrue(input.waitForExistence(timeout: 5))
        input.typeText("hello from the phone")
        app.buttons["composer-send"].tap()

        // The engine adopts the command (echo keeps its id) and the mock
        // harness replies through the chat2 room.
        let echo = app.staticTexts.matching(NSPredicate(format: "label BEGINSWITH 'You: hello from the phone'")).firstMatch
        XCTAssertTrue(echo.waitForExistence(timeout: 10))
        let reply = app.staticTexts.matching(identifier: "row-markdown").firstMatch
        XCTAssertTrue(reply.waitForExistence(timeout: 60), "mock harness reply streamed back")
        let shot = XCTAttachment(screenshot: app.screenshot())
        shot.name = "live-roundtrip"
        shot.lifetime = .keepAlways
        add(shot)
        XCTAssertTrue(app.buttons["Send message"].waitForExistence(timeout: 60), "turn settles")
    }
}
