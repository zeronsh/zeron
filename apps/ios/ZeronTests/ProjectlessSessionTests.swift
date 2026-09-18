import Loro
import XCTest
@testable import Zeron

@MainActor
final class ProjectlessSessionTests: XCTestCase {
    private var appConfig: AppConfig {
        AppConfig(edgeURL: URL(string: "http://localhost:1")!, mode: .dev,
                  userId: "projectless-tests", orgId: "tests", deviceId: "ios-test",
                  deviceName: "Test phone", devBearer: "projectless-tests@tests")
    }

    private let chatConfig = ChatConfig(harness: "codex", model: "gpt-6-astra",
                                        reasoning: "high", modelOptions: ["serviceTier": .string("fast")],
                                        sandbox: "workspace-write")
    private let project = Space(id: "repo", deviceId: "host", path: "/repo", name: "Repo",
                                gitDetected: true, createdAt: 1)
    private let host = DeviceRow(id: "host", name: "Host", platform: "linux", lastSeenAt: 0)
    private let phone = DeviceRow(id: "ios-test", name: "Phone", platform: "ios")

    private func row(_ kind: String, _ id: String, _ fields: [String: JSONValue],
                     deleted: Bool = false) -> RegistryRow {
        RegistryRow(kind: kind, id: id, seq: 1, deleted: deleted, delHlc: nil,
                    fields: fields, clocks: [:])
    }

    func testProjectlessCreationPersistsWithoutSpaceAndSurvivesOfflineReload() throws {
        let doc = RegistryDoc(deviceId: appConfig.deviceId)
        let store = WorkspaceStore(config: appConfig, doc: doc)
        let id = store.createProjectlessChat(deviceId: host.id, config: chatConfig)
        let chat = try XCTUnwrap(store.chats.first)
        XCTAssertEqual(chat.id, id)
        XCTAssertNil(chat.spaceId)
        XCTAssertEqual(chat.deviceId, host.id)
        XCTAssertEqual(chat.cwd, "~")
        XCTAssertEqual(chat.roomGen, 2)
        XCTAssertEqual(chat.config, chatConfig)
        XCTAssertNil(chat.branch)
        XCTAssertNil(chat.checkoutId)
        XCTAssertTrue(store.spaces.isEmpty)
        XCTAssertEqual(store.overviewChats.map(\.id), [id])

        let fields = try XCTUnwrap(doc.pending.first?.ops.first?.set)
        XCTAssertNil(fields["spaceId"], "omit the field entirely, rather than writing an empty ID")
        XCTAssertEqual(fields["deviceId"], .string(host.id))
        XCTAssertEqual(fields["cwd"], .string("~"))
        XCTAssertEqual(fields["roomGen"], .int(2))
        XCTAssertEqual(fields["config"], JSONValue(encodable: chatConfig))

        let restoredDoc = try RegistryDoc.from(data: doc.toData(), deviceId: appConfig.deviceId)
        let restored = WorkspaceStore(config: appConfig, doc: restoredDoc)
        XCTAssertEqual(restored.overviewChats, [chat])
        XCTAssertEqual(restoredDoc.pending.count, 1, "the unsent create remains in the durable outbox")
        XCTAssertNil(restoredDoc.overlayRow(kind: "chats", id: id)?.fields["spaceId"])
    }

    func testProjectCreationKeepsItsHostFolderBranchAndWorktreeOverride() throws {
        let doc = RegistryDoc(deviceId: appConfig.deviceId)
        let store = WorkspaceStore(config: appConfig, doc: doc)
        let normalId = store.createChat(space: project, config: chatConfig)
        let normal = try XCTUnwrap(store.chats.first { $0.id == normalId })
        XCTAssertEqual(normal.spaceId, project.id)
        XCTAssertEqual(normal.deviceId, project.deviceId)
        XCTAssertEqual(normal.cwd, project.path)
        XCTAssertEqual(normal.roomGen, 2)
        XCTAssertEqual(normal.config, chatConfig)

        let worktreeId = store.createChat(space: project, config: chatConfig,
                                         branch: "feature", cwd: "/worktrees/feature")
        let worktree = try XCTUnwrap(store.chats.first { $0.id == worktreeId })
        XCTAssertEqual(worktree.spaceId, project.id)
        XCTAssertEqual(worktree.deviceId, project.deviceId)
        XCTAssertEqual(worktree.cwd, "/worktrees/feature")
        XCTAssertEqual(worktree.branch, "feature")
        XCTAssertEqual(worktree.roomGen, 2)
    }

    func testSyncedProjectlessChatsAreVisibleButDanglingProjectsAreNot() {
        let doc = RegistryDoc(deviceId: appConfig.deviceId)
        func chat(_ spaceId: String?, archived: Bool = false) -> [String: JSONValue] {
            var fields: [String: JSONValue] = ["deviceId": .string(host.id), "cwd": .string("~"),
                                               "archived": .bool(archived), "roomGen": .int(2)]
            if let spaceId { fields["spaceId"] = .string(spaceId) }
            return fields
        }
        // Authoritative desktop rows, not creates through the mobile API.
        doc.applyState(seq: 1, full: true, gcFloor: 0, rows: [
            row("spaces", project.id, ["deviceId": .string(host.id), "path": .string(project.path)]),
            row("spaces", "deleted", [:], deleted: true),
            row("chats", "projectless", chat(nil)),
            row("chats", "project", chat(project.id)),
            row("chats", "dangling", chat("missing")),
            row("chats", "deleted-project", chat("deleted")),
            row("chats", "archived-projectless", chat(nil, archived: true)),
            row("chats", "archived-dangling", chat("missing", archived: true)),
        ])
        let store = WorkspaceStore(config: appConfig, doc: doc)
        XCTAssertEqual(Set(store.overviewChats.map(\.id)), ["projectless", "project"])
        XCTAssertEqual(store.chats(in: project.id).map(\.id), ["project"])
        XCTAssertEqual(Set(store.archivedChats().map(\.id)), ["archived-projectless", "archived-dangling"])
        XCTAssertTrue(store.archivedChats(in: project.id).isEmpty)

        store.setArchived(chatId: "archived-projectless", archived: false)
        XCTAssertTrue(store.overviewChats.contains { $0.id == "archived-projectless" })
        XCTAssertFalse(store.chats(in: project.id).contains { $0.id == "archived-projectless" })
    }

    func testTypedRoutesKeepExistingProjectNavigationAndResolveTheSelectedHost() {
        let projectDestination = NewSessionDestination.project(spaceId: project.id)
        let projectlessDestination = NewSessionDestination.projectless(deviceId: host.id)
        XCTAssertEqual(Route.newSession(spaceId: project.id), .newSession(projectDestination))
        XCTAssertNotEqual(Route.newSession(projectDestination), .newSession(projectlessDestination))
        XCTAssertEqual(projectDestination.space(in: [project]), project)
        XCTAssertEqual(projectDestination.deviceId(spaces: [project], devices: [host, phone]), host.id)
        XCTAssertNil(projectlessDestination.space(in: [project]))
        XCTAssertEqual(projectlessDestination.deviceId(spaces: [], devices: [host, phone]), host.id)
        XCTAssertNil(projectDestination.deviceId(spaces: [], devices: [host]),
                     "a deleted project cannot silently become projectless")

        let other = DeviceRow(id: "other", name: "Other", platform: "macos")
        let selected = NewSessionDestination.projectless(deviceId: other.id)
        XCTAssertEqual(selected.deviceId(spaces: [project], devices: [host, other, phone]), other.id)
        XCTAssertNil(selected.deviceId(spaces: [], devices: [host, phone]))
        XCTAssertNil(NewSessionDestination.projectless(deviceId: phone.id)
            .deviceId(spaces: [], devices: [host, phone]))
    }

    func testAppModelCanCreateWithNoProjectsAndRejectsNonHosts() throws {
        let model = AppModel()
        model.demo = DemoDataset(devices: [phone, host], spaces: [], chats: [], sessions: [:])
        XCTAssertEqual(model.executionDevices.map(\.id), [host.id])
        XCTAssertFalse(model.deviceOnline(host.id), "offline hosts remain valid destinations")
        XCTAssertNil(model.createProjectlessChat(deviceId: phone.id, config: chatConfig))
        XCTAssertNil(model.createProjectlessChat(deviceId: "missing", config: chatConfig))
        XCTAssertNil(model.createProjectlessChat(deviceId: "", config: chatConfig))
        let id = try XCTUnwrap(model.createProjectlessChat(deviceId: host.id, config: chatConfig))
        let chat = try XCTUnwrap(model.chat(id: id))
        XCTAssertNil(chat.spaceId)
        XCTAssertEqual(chat.deviceId, host.id)
        XCTAssertEqual(chat.cwd, "~")
        XCTAssertEqual(chat.config, chatConfig)
        XCTAssertEqual(chat.roomGen, 2)
        XCTAssertEqual(model.overviewChats, [chat])
        XCTAssertTrue(model.spaces.isEmpty)
        XCTAssertNotNil(model.sessionStore(for: chat))
    }

    func testDemoAppModelKeepsProjectAndProjectlessListsSeparate() throws {
        let model = AppModel()
        model.demo = DemoDataset(devices: [host], spaces: [project], chats: [], sessions: [:])
        let projectId = try XCTUnwrap(model.createChat(space: project, config: chatConfig,
                                                      branch: "main", cwd: "/checkout"))
        let projectlessId = try XCTUnwrap(model.createProjectlessChat(deviceId: host.id, config: chatConfig))
        XCTAssertEqual(Set(model.overviewChats.map(\.id)), [projectId, projectlessId])
        XCTAssertEqual(model.chats(in: project.id).map(\.id), [projectId])
        let chat = try XCTUnwrap(model.chat(id: projectId))
        XCTAssertEqual(chat.cwd, "/checkout")
        XCTAssertEqual(chat.branch, "main")
        XCTAssertEqual(chat.spaceId, project.id)
        XCTAssertEqual(chat.config, chatConfig)
        XCTAssertEqual(chat.roomGen, 2)
        model.demo?.spaces = []
        XCTAssertEqual(model.overviewChats.map(\.id), [projectlessId])
        model.archive(chatId: projectlessId)
        XCTAssertEqual(model.archivedChats().map(\.id), [projectlessId])
        XCTAssertTrue(model.overviewChats.isEmpty)
    }

    func testProjectlessFirstRunUsesHomeAndConfigInTheCommandLedger() throws {
        let workspace = WorkspaceStore(config: appConfig)
        let id = workspace.createProjectlessChat(deviceId: host.id, config: chatConfig)
        let chat = try XCTUnwrap(workspace.chats.first)
        let session = SessionStore(chatId: id, config: appConfig)
        session.hostDeviceId = chat.deviceId
        session.sendRun(prompt: "Inspect my home folder", chat: chat)
        XCTAssertEqual(session.hostDeviceId, host.id, "nudge and uploads target the chosen host")
        let commands = try XCTUnwrap(session.doc.getDeepValue().mapValue?["commands"]?.listValue)
        XCTAssertEqual(commands.count, 1)
        let command = try XCTUnwrap(commands.first?.mapValue)
        XCTAssertEqual(command["status"]?.stringValue, "pending")
        let request = try XCTUnwrap(command["payload"]?.mapValue?["request"]?.mapValue)
        XCTAssertEqual(request["cwd"]?.stringValue, "~")
        XCTAssertEqual(request["prompt"]?.stringValue, "Inspect my home folder")
        XCTAssertEqual(request["harness"]?.stringValue, chatConfig.harness)
        XCTAssertEqual(request["model"]?.stringValue, chatConfig.model)
        XCTAssertEqual(request["reasoning"]?.stringValue, chatConfig.reasoning)
        XCTAssertEqual(request["modelOptions"]?.mapValue?["serviceTier"]?.stringValue, "fast")
        XCTAssertNil(request["worktree"])
    }

    func testLiveAppModelCreatesOnSelectedHostAndUsesItsCapabilitiesWithoutProjects() throws {
        let doc = RegistryDoc(deviceId: appConfig.deviceId)
        doc.applyState(seq: 1, full: true, gcFloor: 0, rows: [
            row("devices", host.id, ["platform": .string("linux"), "version": .string("0.2.12"),
                                     "capabilities": .array([.string(EngineCapability.messageQueueV1)])]),
            row("devices", "old-host", ["platform": .string("macos"), "version": .string("0.2.11")]),
            row("devices", phone.id, ["platform": .string("ios")]),
        ])
        let model = AppModel()
        model.workspace = WorkspaceStore(config: appConfig, doc: doc)
        XCTAssertEqual(Set(model.executionDevices.map(\.id)), [host.id, "old-host"])
        let id = try XCTUnwrap(model.createProjectlessChat(deviceId: host.id, config: chatConfig))
        let chat = try XCTUnwrap(model.chat(id: id))
        XCTAssertEqual(chat.deviceId, host.id)
        XCTAssertNil(chat.spaceId)
        XCTAssertEqual(model.overviewChats, [chat])
        XCTAssertTrue(model.hostSupportsQueuedAttachments(chat))
        XCTAssertTrue(model.hostSupportsMessageQueue(chat))
        XCTAssertFalse(model.hostSupportsQueuedAttachmentsOn(deviceId: "old-host"))
        XCTAssertFalse(model.deviceOnline(host.id))
        XCTAssertTrue(model.spaces.isEmpty)
        XCTAssertNil(model.createProjectlessChat(deviceId: phone.id, config: chatConfig))
    }
}
