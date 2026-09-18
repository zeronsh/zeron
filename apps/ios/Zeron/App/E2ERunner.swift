// Headless e2e rig — launch with `-e2e` (plus a local wrangler dev edge and a
// `zeron headless` engine in dev mode) and the app exercises the full live
// stack with no taps: workspace room backfill, device-relay RPCs, space/chat
// creation, the command plane, and session-room streaming. Results append to
// Documents/e2e.log for the harness to read via simctl.

import Foundation

@MainActor
enum E2ERunner {
    static var logURL: URL {
        FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("e2e.log")
    }

    static func log(_ line: String) {
        let stamped = "[\(Int(Date().timeIntervalSince1970))] \(line)\n"
        print("E2E: \(line)")
        if let handle = try? FileHandle(forWritingTo: logURL) {
            handle.seekToEndOfFile()
            handle.write(Data(stamped.utf8))
            try? handle.close()
        } else {
            try? Data(stamped.utf8).write(to: logURL)
        }
    }

    static func run(model: AppModel) async {
        try? FileManager.default.removeItem(at: logURL)
        log("start")
        model.signInDev(edgeURL: URL(string: "http://localhost:8787")!,
                        userId: "devuser", orgId: "dev-org")

        // 1. Workspace room: wait for connection + the engine's device row.
        guard let workspace = model.workspace else {
            log("FAIL no workspace store")
            return
        }
        // Warm-start probe: rows visible BEFORE any network = disk hydration.
        log("warm-start devices=\(workspace.devices.count) chats=\(workspace.chats.count)")
        let device = await poll(timeout: 15, label: "workspace device") {
            workspace.connected ? workspace.devices.first { $0.platform != "ios" } : nil
        }
        guard let device else {
            log("FAIL workspace: connected=\(workspace.connected) devices=\(workspace.devices.map(\.id))")
            return
        }
        log("OK workspace synced; engine device \(device.id) (\(device.name))")

        // 2. Device relay: ListFolders on every engine device (stale rig
        // devices linger in the dev workspace doc — report each).
        var listing: FolderListing?
        for candidate in workspace.devices where candidate.platform != "ios" {
            do {
                let l = try await workspace.listFoldersDetailed(deviceId: candidate.id, path: nil)
                log("OK relay ListFolders[\(candidate.name)/\(candidate.id.prefix(8))]: \(l.path) → \(l.entries.count) entries")
                listing = l
            } catch {
                log("FAIL relay ListFolders[\(candidate.name)/\(candidate.id.prefix(8))]: \(error.localizedDescription)")
            }
        }

        // 2b. Live model catalog over the relay.
        let models = try? await workspace.listModels(deviceId: device.id, harness: "mock", cwd: nil)
        log(models != nil ? "OK relay ListModels: \(models!.map(\.id))" : "FAIL relay ListModels nil")

        // 3. Space + chat + first run through the command plane (mock harness).
        let spaceId = await workspace.createSpace(deviceId: device.id,
                                                  path: listing?.path ?? "/tmp", gitDetected: false)
        log("space created \(spaceId)")
        // Relay-created spaces land via doc sync — eventually consistent.
        let space = await poll(timeout: 10, label: "space row sync") {
            workspace.spaces.first { $0.id == spaceId }
        }
        guard let space else {
            log("FAIL space row never synced")
            return
        }
        let chatId = workspace.createChat(
            space: space,
            config: ChatConfig(harness: "mock", model: nil, reasoning: nil, sandbox: "workspace-write"))
        guard let chat = workspace.chats.first(where: { $0.id == chatId }),
              let store = model.sessionStore(for: chat) else {
            log("FAIL chat/session store")
            return
        }
        store.sendRun(prompt: "e2e ping", chat: chat)
        log("run queued on \(chatId)")

        let entries = await poll(timeout: 30, label: "assistant reply") {
            store.entries.contains { $0.role == .assistant && !$0.parts.isEmpty } ? store.entries : nil
        }
        if let entries {
            log("OK transcript streamed: \(entries.count) entries")
        } else {
            log("FAIL no assistant reply; entries=\(store.entries.count) connected=\(store.connected)")
        }

        // 4. Big-doc backfill (fragmented): open the chat the seeder filled.
        let bigChatId = "e2e-big-doc"
        let bigChat = Chat(id: bigChatId, deviceId: device.id, title: "big", archived: false,
                           cwd: nil, branch: nil, checkoutId: nil, config: nil,
                           lastMessagePreview: nil, lastMessageAt: nil, createdAt: nowMs(),
                           spaceId: spaceId, lastSeenAt: nil,
                           roomGen: 2)  // the seeder fills a chat2 room now
        if let bigStore = model.sessionStore(for: bigChat) {
            let big = await poll(timeout: 20, label: "big doc backfill") {
                bigStore.entries.count >= 40 ? bigStore.entries : nil
            }
            if let big {
                let bytes = big.flatMap(\.parts).reduce(0) { acc, part in
                    if case .text(_, let t) = part { return acc + t.count }
                    return acc
                }
                log("OK big-doc backfill: \(big.count) entries, ~\(bytes / 1024)KB text")
            } else {
                log("FAIL big-doc backfill: entries=\(bigStore.entries.count) connected=\(bigStore.connected)")
            }
        }

        log("done")
    }

    private static func poll<T>(timeout: TimeInterval, label: String,
                                _ probe: @MainActor () -> T?) async -> T? {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if let value = probe() { return value }
            try? await Task.sleep(nanoseconds: 300_000_000)
        }
        log("timeout waiting for \(label)")
        return nil
    }
}

extension E2ERunner {
    /// Live-relay probe: runs inside the user's real signed-in session and
    /// interrogates every engine device — workspace presence, the device
    /// room's host attachment, and a real ListFolders with the exact error.
    @MainActor
    static func runLive(model: AppModel) async {
        try? FileManager.default.removeItem(at: logURL)
        log("live start edge=\(model.edgeURLString) mode=\(model.authModeRaw) user=\(model.storedUserId.prefix(18)) org=\(model.storedOrgId.prefix(18))")
        let workspace = await poll(timeout: 25, label: "workspace connect") {
            model.workspace?.connected == true ? model.workspace : nil
        }
        guard let workspace else {
            log("FAIL workspace never connected: store=\(model.workspace != nil) "
                + "userId=\(model.storedUserId.isEmpty ? "EMPTY" : "set") "
                + "orgId=\(model.storedOrgId.isEmpty ? "EMPTY" : "set") "
                + "access=\(Keychain.load(key: "accessToken") != nil) "
                + "refresh=\(Keychain.load(key: "refreshToken") != nil) "
                + "mode=\(model.authModeRaw)")
            return
        }
        log("devices: " + workspace.devices.map {
            "\($0.name)[\($0.platform)] id=\($0.id) presence=\(workspace.deviceOnline($0.id))"
        }.joined(separator: ", "))
        guard let config = model.diagnosticsConfig else {
            log("FAIL no config")
            return
        }
        for device in workspace.devices where device.platform != "ios" {
            let status = await config.deviceStatus(deviceId: device.id)
            log("\(device.name) /status → \(status)")
            do {
                let listing = try await workspace.listFoldersDetailed(deviceId: device.id, path: nil)
                log("OK \(device.name) ListFolders → \(listing.path) (\(listing.entries.count) entries)")
            } catch {
                log("FAIL \(device.name) ListFolders → \(error.localizedDescription)")
            }
        }
        log("done")
    }
}
