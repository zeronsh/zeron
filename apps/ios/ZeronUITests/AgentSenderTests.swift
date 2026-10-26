import XCTest

/// A message another chat sent: its sender rides over the bubble and opens
/// that chat.
final class AgentSenderTests: XCTestCase {
    override func setUp() {
        continueAfterFailure = false
    }

    private func snapshot(_ app: XCUIApplication, _ name: String) {
        let shot = XCTAttachment(screenshot: app.screenshot())
        shot.name = name
        shot.lifetime = .keepAlways
        add(shot)
    }

    func testSenderCaptionOpensTheSendingChat() {
        let app = XCUIApplication()
        app.launchArguments = ["-demo", "-route", "chat:chat-side"]
        app.launch()

        let body = app.descendants(matching: .any)
            .matching(NSPredicate(format: "label BEGINSWITH 'You: Keeping the power curve'")).firstMatch
        XCTAssertTrue(body.waitForExistence(timeout: 15))
        // The routing header left the bubble.
        XCTAssertFalse(body.label.contains("send_message"))
        snapshot(app, "sender-caption")

        let sender = app.descendants(matching: .any)["Open sending chat"].firstMatch
        XCTAssertTrue(sender.exists)
        sender.tap()
        let reply = app.descendants(matching: .any)
            .matching(NSPredicate(format: "label CONTAINS 'critically damped spring'")).firstMatch
        XCTAssertTrue(reply.waitForExistence(timeout: 10), "the sending chat opened")
        snapshot(app, "sender-opened")
    }
}
