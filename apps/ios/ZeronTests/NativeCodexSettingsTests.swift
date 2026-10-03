import XCTest
@testable import Zeron

final class NativeCodexSettingsTests: XCTestCase {
    private let catalog: [[String: Any]] = [["id": "display-id", "model": "test-model", "isDefault": true, "defaultReasoningEffort": "medium", "supportedReasoningEfforts": [["reasoningEffort": "medium"], ["reasoningEffort": "high"]], "serviceTiers": [["id": "default"], ["id": "priority"]]]]

    func testExplicitOptionsAndResetUseWireSemantics() throws {
        let explicit = NativeCodexSettings(model: "test-model", effort: "high", serviceTier: "priority")
        let params = explicit.turnParameters(catalog: catalog)
        XCTAssertEqual(params["model"] as? String, "test-model")
        XCTAssertEqual(params["effort"] as? String, "high")
        XCTAssertEqual(params["serviceTier"] as? String, "priority")
        let reset = NativeCodexSettings(model: "test-model").turnParameters(catalog: catalog)
        XCTAssertEqual(reset["effort"] as? String, "medium")
        XCTAssertTrue(reset["serviceTier"] is NSNull, "Automatic must clear a previous thread tier")
        XCTAssertEqual(try JSONDecoder().decode(NativeCodexSettings.self, from: JSONEncoder().encode(explicit)), explicit)
    }

    func testModelChangeDropsUnsupportedOptions() {
        let value = NativeCodexSettings(model: "unavailable", effort: "xhigh", serviceTier: "flex").normalized(catalog: catalog)
        XCTAssertEqual(value.model, "test-model")
        XCTAssertNil(value.effort)
        XCTAssertNil(value.serviceTier)
    }
}
