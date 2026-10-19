import XCTest
@testable import Zeron

final class WidgetStorageTests: XCTestCase {
    func testAppGroupSupportsSharedSnapshotStorage() throws {
        let container = try XCTUnwrap(
            FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: WidgetStore.appGroup),
            "Widget snapshots require App Group entitlements. Keep simulator signing enabled."
        )
        let fileURL = container.appendingPathComponent("widget-storage-test-\(UUID().uuidString).json")
        defer { try? FileManager.default.removeItem(at: fileURL) }

        let snapshot = WidgetSnapshot.empty
        try JSONEncoder().encode(snapshot).write(to: fileURL, options: .atomic)
        let restored = try JSONDecoder().decode(WidgetSnapshot.self, from: Data(contentsOf: fileURL))

        XCTAssertEqual(restored, snapshot)
    }
}
