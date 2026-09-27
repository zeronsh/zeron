import XCTest
@testable import Zeron

final class PromptDraftTests: XCTestCase {
    @MainActor func testDraftsKeepConcurrentContentAndMoveWithoutChangingIt() {
        let doc = RegistryDoc(deviceId: "phone")
        let content = PromptDraftContent(prompt: "Keep this", target: PromptDraftTarget(deviceId: "host"))
        let a = PromptDraftSave(id: "a", revision: "v1", createdAt: 1, content: content)
        doc.publishPromptDraft(a)
        doc.publishPromptDraft(PromptDraftSave(id: "b", revision: "v2", createdAt: 2, content: content))
        doc.publishPromptDraft(PromptDraftSave(id: "a", revision: "v3", baseRevision: "v1", createdAt: 1, content: content))
        doc.publishPromptDraft(PromptDraftSave(id: "a", revision: "v4", baseRevision: "v1", createdAt: 1, content: content))
        XCTAssertEqual(doc.promptDraftRows.count, 3)
        XCTAssertTrue(doc.promptDraftRows.contains { $0.id == "v3" && $0.conflict })
        doc.movePromptDraft("a", before: "b", after: nil)
        XCTAssertEqual(doc.promptDraftRows.first?.id, "a")
        XCTAssertEqual(doc.promptDraftRows.first?.revision, "v4")
        doc.write(kind: "promptDrafts", id: "a", op: .upsert, set: ["closed": .bool(true)])
        doc.movePromptDraft("a", before: "b", after: nil)
        XCTAssertFalse(doc.promptDraftRows.contains { $0.id == "a" })
    }
    @MainActor func testSentRevisionDoesNotHideLateConcurrentEdit() {
        let doc = RegistryDoc(deviceId: "phone")
        let content = PromptDraftContent(prompt: "Keep this", target: PromptDraftTarget(deviceId: "host"))
        doc.publishPromptDraft(PromptDraftSave(id: "a", revision: "v1", createdAt: 1, content: content))
        doc.write(kind: "promptDrafts", id: "a", op: .upsert, set: ["sentRevision": .string("v1")])
        XCTAssertTrue(doc.promptDraftRows.isEmpty)
        doc.publishPromptDraft(PromptDraftSave(id: "a", revision: "v2", baseRevision: "v1", createdAt: 1, content: content))
        XCTAssertEqual(doc.promptDraftRows.first?.id, "v2")
        XCTAssertEqual(doc.promptDraftRows.first?.conflict, true)
    }

    func testRestoredAppshotKeepsEscapedContextAndImageReference() {
        let metadata: [String: JSONValue] = ["image": .object([
            "appName": .string("Notes & Tasks"), "windowTitle": .string("A < B"),
            "accessibility": .object(["format_version": .int(1), "content": .string("Read <this> & that"), "truncated": .bool(false)])])]
        let text = AppshotContext.withDraftMetadata("Review", metadata: metadata, imagePaths: ["image": "pending://upload/image.png"])
        XCTAssertEqual(AppshotContext.visibleText(text), "Review")
        XCTAssertTrue(text.contains("Read &lt;this&gt; &amp; that"))
        XCTAssertEqual(AppshotContext.presentations(text)["pending://upload/image.png"]?.appName, "Notes & Tasks")
    }

    @MainActor func testConsumingUneditedConflictHidesOnlyThatRecoveredHead() {
        let doc = RegistryDoc(deviceId: "phone")
        let content = PromptDraftContent(prompt: "Keep this", target: PromptDraftTarget(deviceId: "host"))
        doc.publishPromptDraft(PromptDraftSave(id: "a", revision: "v1", createdAt: 1, content: content))
        doc.publishPromptDraft(PromptDraftSave(id: "a", revision: "v2", baseRevision: "v1", createdAt: 1, content: content))
        doc.publishPromptDraft(PromptDraftSave(id: "a", revision: "v3", baseRevision: "v1", createdAt: 1, content: content))
        XCTAssertTrue(doc.promptDraftRows.contains { $0.id == "v2" })
        doc.write(kind: "promptDrafts", id: "v2", op: .upsert, set: ["sentRevision": .string("v2")])
        XCTAssertEqual(doc.promptDraftRows.map(\.id), ["a"])
    }

    @MainActor func testCopiedOrderObservesRemoteFutureClock() {
        let doc = RegistryDoc(deviceId: "phone")
        let future = "9999999999999-000001-remote"
        _ = doc.applyRows(seq: 1, rows: [RegistryRow(kind: "promptDrafts", id: "draft", seq: 1, deleted: false,
            fields: ["orderKey": .string("8")], clocks: ["orderKey": future])])
        doc.setPromptDraftOrder("draft", key: "c")
        XCTAssertEqual(doc.overlayRow(kind: "promptDrafts", id: "draft")?.fields["orderKey"]?.stringValue, "c")
    }

    @MainActor func testCanvasAppearsOnlyAfterLeavingAndStaysListed() {
        let doc = RegistryDoc(deviceId: "phone")
        let content = PromptDraftContent(prompt: "In progress", target: PromptDraftTarget(deviceId: "host"))
        var save = PromptDraftSave(deferred: true, id: "a", revision: "v1", createdAt: 1, content: content)
        doc.publishPromptDraft(save)
        XCTAssertTrue(doc.promptDraftRows.isEmpty)
        save.deferred = false
        doc.publishPromptDraft(save)
        XCTAssertEqual(doc.promptDraftRows.map(\.id), ["a"])
        save.deferred = true; save.revision = "v2"; save.baseRevision = "v1"
        doc.publishPromptDraft(save)
        XCTAssertEqual(doc.promptDraftRows.first?.revision, "v2")
    }

}
