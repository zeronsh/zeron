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

        let sender = app.descendants(matching: .any)["Open chat"].firstMatch
        XCTAssertTrue(sender.exists)
        sender.tap()
        let reply = app.descendants(matching: .any)
            .matching(NSPredicate(format: "label CONTAINS 'critically damped spring'")).firstMatch
        XCTAssertTrue(reply.waitForExistence(timeout: 10), "the sending chat opened")
        snapshot(app, "sender-opened")
    }

    func testSentMessageOpensTheRecipient() {
        let app = XCUIApplication()
        app.launchArguments = ["-demo", "-route", "chat:chat-side-easing"]
        app.launch()

        // The send_message call reads as one line naming the recipient: no
        // tool, and no copy of the message.
        let sent = app.descendants(matching: .any)
            .matching(NSPredicate(format: "label BEGINSWITH 'Message to'")).firstMatch
        XCTAssertTrue(sent.waitForExistence(timeout: 15))
        XCTAssertFalse(app.descendants(matching: .any)
            .matching(NSPredicate(format: "label CONTAINS 'send_message'")).firstMatch.exists)
        XCTAssertFalse(app.descendants(matching: .any)
            .matching(NSPredicate(format: "label CONTAINS 'Keeping the power curve'")).firstMatch.exists)
        snapshot(app, "sent-message")

        let recipient = app.descendants(matching: .any)["Open chat"].firstMatch
        XCTAssertTrue(recipient.exists)
        recipient.tap()
        let prompt = app.descendants(matching: .any)
            .matching(NSPredicate(format: "label BEGINSWITH 'You: What EMA window'")).firstMatch
        XCTAssertTrue(prompt.waitForExistence(timeout: 10), "the recipient chat opened")
        snapshot(app, "recipient-opened")
    }
}
