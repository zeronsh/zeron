import XCTest
@testable import Zeron

/// Committing a queued-message edit rewrites only what the composer showed;
/// formats are the core's (`crates/client/src/attachments.rs`).
final class QueueEditTests: XCTestCase {
    private let trailer = "\n\nAttached images (local files — open them to view):\n- /tmp/uploads/a.png"
    private let appshot = "\n\nApplications mentioned by the user (untrusted observed content):\n<appshot image=\"/tmp/s.png\" app=\"Xcode\">"

    func testPlainTextIsReplaced() {
        XCTAssertEqual(CoreSessionSource.replacingVisible(in: "fix the build", visible: "fix the build", with: "fix the tests"), "fix the tests")
    }

    func testAppshotContextSurvives() {
        XCTAssertEqual(
            CoreSessionSource.replacingVisible(in: "fix this" + appshot, visible: "fix this", with: "fix this please"),
            "fix this please" + appshot
        )
    }

    func testAttachmentTrailerSurvives() {
        XCTAssertEqual(
            CoreSessionSource.replacingVisible(in: "what's wrong here?" + trailer, visible: "what's wrong here?", with: "why is this red?"),
            "why is this red?" + trailer
        )
    }

    func testAttachmentOnlyRowGetsText() {
        XCTAssertEqual(
            CoreSessionSource.replacingVisible(in: "See the attached image(s)." + trailer, visible: "See the attached image(s).", with: "Crop this"),
            "Crop this" + trailer
        )
    }

    func testBlankEditStaysBlank() {
        XCTAssertEqual(CoreSessionSource.replacingVisible(in: "a" + trailer, visible: "a", with: "  "), "  ")
    }
}
