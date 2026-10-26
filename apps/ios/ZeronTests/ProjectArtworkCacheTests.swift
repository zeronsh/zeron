import XCTest
@testable import Zeron

final class ProjectArtworkCacheTests: XCTestCase {
    func testPreviewSurvivesRestartAndSelectionChangesDoNotReuseIt() {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let preview = Data("fabricated preview".utf8)
        ProjectArtworkCache(directory: directory).store(preview, for: "project:old.svg")

        let reopened = ProjectArtworkCache(directory: directory)
        XCTAssertEqual(reopened.load("project:old.svg"), preview)
        XCTAssertNil(reopened.load("project:new.svg"))
        reopened.store(preview, for: "project:new.svg")
        reopened.retain(["project:new.svg"])
        XCTAssertNil(reopened.load("project:old.svg"))
        XCTAssertEqual(reopened.load("project:new.svg"), preview)
    }

    func testAccountStoresStaySeparateAndSignOutClearsPreviews() {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let first = ProjectArtworkCache(directory: directory.appendingPathComponent("first"))
        let second = ProjectArtworkCache(directory: directory.appendingPathComponent("second"))
        let preview = Data("fabricated preview".utf8)
        first.store(preview, for: "project:icon.svg")
        XCTAssertNil(second.load("project:icon.svg"))
        first.clear()
        XCTAssertNil(ProjectArtworkCache(directory: directory.appendingPathComponent("first")).load("project:icon.svg"))
    }
}
