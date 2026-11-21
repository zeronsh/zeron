import XCTest
@testable import Zeron

@MainActor
final class ChangeReviewTests: XCTestCase {
    func testCommentsCiteTheCorrectSideAndRenamePath() {
        let draft = ChangeReviewDraft()
        draft.comments = [
            DiffReviewComment(id: UUID(), path: "Sources/New.swift", oldPath: "Sources/Old.swift", oldSide: true, line: 9, code: "old", body: "Keep this fallback.\nIt handles missing values."),
            DiffReviewComment(id: UUID(), path: "Sources/New.swift", oldPath: "Sources/Old.swift", oldSide: false, line: 12, code: "new", body: "  Add a test.  "),
        ]
        XCTAssertEqual(draft.prompt, """
        Address the review comments below.

        Comments on the diff (each cites the file and line it belongs to; L = line number in the original file, R = in the changed file):
        - Sources/Old.swift:9 (L): Keep this fallback.
          It handles missing values.
        - Sources/New.swift:12 (R): Add a test.
        """)
        draft.comments.removeAll()
        XCTAssertEqual(draft.prompt, "")
    }

    func testRangeCommentsCiteBothEnds() {
        let comment = DiffReviewComment(id: UUID(), path: "Sources/New.swift", oldPath: "Sources/Old.swift", oldSide: true, line: 9, endLine: 12, code: "old", body: "Keep these lines.")
        XCTAssertEqual(comment.location, "Sources/Old.swift:9-12")
        let draft = ChangeReviewDraft()
        draft.comments = [comment]
        XCTAssertTrue(draft.prompt.contains("- Sources/Old.swift:9-12 (L): Keep these lines."))
    }
    func testSelectionExpandsBothWaysAndTrimsInteriorTaps() {
        var selected = DiffLineSelection(path: "Fixture.swift", oldSide: false, line: 10)
        selected.adjust(to: 14)
        XCTAssertEqual(selected.lower...selected.upper, 10...14)
        selected.adjust(to: 8)
        XCTAssertEqual(selected.lower...selected.upper, 8...14)
        selected.adjust(to: 9)
        XCTAssertEqual(selected.lower...selected.upper, 9...14)
        selected.adjust(to: 12)
        XCTAssertEqual(selected.lower...selected.upper, 9...12)
        selected.adjust(to: 10)
        XCTAssertEqual(selected.lower...selected.upper, 10...12)
        selected.adjust(to: 11)
        XCTAssertEqual(selected.lower...selected.upper, 11...12)
        selected.adjust(to: 12)
        XCTAssertEqual(selected.lower...selected.upper, 12...12)
        selected.adjust(to: 12)
        XCTAssertEqual(selected.lower...selected.upper, 12...12)
    }

}
