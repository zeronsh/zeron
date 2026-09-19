import Foundation
import XCTest
@testable import Zeron

final class PrivateWorkspaceTests: XCTestCase {
    private func profile(hub: String = "https://workspace.example.ts.net", workspace: String = "workspace-a",
                         device: String = "ios-client") -> PrivateWorkspaceProfile {
        PrivateWorkspaceProfile(hubURL: URL(string: hub)!, workspaceId: workspace,
                                userId: "workspace-user", deviceId: device,
                                name: "Development", deviceName: "Mobile")
    }

    func testPrivateSocketsUseHeaderAuthenticationAndClientRole() async throws {
        let config = profile().config(token: "private-secret")
        let registryRequest = await config.registrySocketRequest()
        let chatRequest = await config.chat2SocketRequest(chatId: "chat-a")
        let relayRequest = await config.deviceRelayRequest(deviceId: "server-a", connectionId: "connection-a")
        let registry = try XCTUnwrap(registryRequest)
        let chat = try XCTUnwrap(chatRequest)
        let relay = try XCTUnwrap(relayRequest)
        XCTAssertEqual(registry.url?.path, "/registry/workspace-a/ws")
        XCTAssertEqual(chat.url?.path, "/chat2/chat-a/ws")
        XCTAssertEqual(relay.url?.path, "/device/server-a/ws")
        for request in [registry, chat, relay] {
            XCTAssertEqual(request.url?.host, "workspace.example.ts.net")
            XCTAssertEqual(request.url?.scheme, "wss")
            XCTAssertEqual(request.value(forHTTPHeaderField: "Authorization"), "Bearer private-secret")
            XCTAssertFalse(request.url!.absoluteString.contains("private-secret"))
            let query = URLComponents(url: request.url!, resolvingAgainstBaseURL: false)?.queryItems ?? []
            XCTAssertFalse(query.contains { $0.name == "token" })
        }
        let query = URLComponents(url: relay.url!, resolvingAgainstBaseURL: false)?.queryItems
        XCTAssertEqual(query?.first(where: { $0.name == "role" })?.value, "client")
    }

    func testPrivateHTTPRequestsStayAtHubAndUseNodeToken() async throws {
        let config = profile().config(token: "node-token")
        let requests = await [config.registryRowsRequest(since: 7), config.registryPushRequest(),
                              config.chat2CheckpointRequest(chatId: "chat-a"),
                              config.chat2RowsRequest(chatId: "chat-a", after: 9),
                              config.chat2PushRequest(chatId: "chat-a", batchId: "batch-a")]
        for possible in requests {
            let request = try XCTUnwrap(possible)
            XCTAssertEqual(request.url?.host, "workspace.example.ts.net")
            XCTAssertEqual(request.value(forHTTPHeaderField: "Authorization"), "Bearer node-token")
            XCTAssertFalse(request.url!.absoluteString.contains("node-token"))
        }
        XCTAssertEqual(requests[1]?.httpMethod, "POST")
        XCTAssertEqual(requests[4]?.httpMethod, "POST")
    }

    func testPrivateAuthenticationDoesNotUseCloudTokensAndInvalidationStopsRequests() async {
        let config = AppConfig(edgeURL: profile().hubURL, mode: .privateWorkspace,
                               userId: "user", orgId: "workspace", deviceId: "client", deviceName: "Mobile",
                               tokens: AuthTokens(accessToken: "cloud-access", refreshToken: "cloud-refresh"),
                               devBearer: "dev-token", privateBearer: "node-token")
        let token = await config.currentToken()
        XCTAssertEqual(token, "node-token")
        config.invalidate()
        let request = await config.registrySocketRequest()
        XCTAssertNil(request)
    }

    func testCloudSocketCompatibilityIsPreserved() async throws {
        let config = AppConfig(edgeURL: URL(string: "https://edge.example")!, mode: .workos,
                               userId: "user", orgId: "org", deviceId: "client", deviceName: "Mobile",
                               tokens: AuthTokens(accessToken: "cloud-token", refreshToken: "refresh"))
        let possible = await config.registrySocketRequest()
        let request = try XCTUnwrap(possible)
        let query = URLComponents(url: request.url!, resolvingAgainstBaseURL: false)?.queryItems
        XCTAssertEqual(query?.first(where: { $0.name == "token" })?.value, "cloud-token")
        XCTAssertNil(request.value(forHTTPHeaderField: "Authorization"))
    }

    func testWorkspaceNodeAndEndpointIsolateCredentialsAndCaches() {
        let baseline = profile()
        for other in [profile(workspace: "workspace-b"), profile(device: "ios-other"),
                      profile(hub: "https://other.example.ts.net")] {
            XCTAssertNotEqual(baseline.tokenKey, other.tokenKey)
            XCTAssertNotEqual(baseline.config(token: "a").cacheNamespace, other.config(token: "a").cacheNamespace)
        }
        let privateConfig = baseline.config(token: "a")
        XCTAssertEqual(privateConfig.mode, .privateWorkspace)
        XCTAssertEqual(privateConfig.orgId, "workspace-a")
        XCTAssertEqual(privateConfig.userId, "workspace-user")
        XCTAssertEqual(privateConfig.deviceId, "ios-client")
        let cloudConfig = AppConfig(edgeURL: baseline.hubURL, mode: .workos,
                                    userId: baseline.userId, orgId: baseline.workspaceId,
                                    deviceId: baseline.deviceId, deviceName: baseline.deviceName)
        XCTAssertNotEqual(privateConfig.cacheNamespace, cloudConfig.cacheNamespace)
        XCTAssertEqual(privateConfig.cacheNamespace, baseline.config(token: "rotated-token").cacheNamespace)
        XCTAssertFalse(baseline.tokenKey.contains("private-secret"))
    }

    func testQueuedAttachmentsCannotBeReadAcrossProfiles() {
        let first = "private-test-" + UUID().uuidString
        let second = "cloud-test-" + UUID().uuidString
        defer {
            DocDisk.wipe(namespace: first)
            DocDisk.wipe(namespace: second)
        }
        UploadStash.save(uploadId: "same-upload", data: Data([1, 2, 3]), namespace: first)
        XCTAssertEqual(UploadStash.load(uploadId: "same-upload", namespace: first), Data([1, 2, 3]))
        XCTAssertNil(UploadStash.load(uploadId: "same-upload", namespace: second))
        XCTAssertNotEqual(DocDisk.chat2URL(for: "same-chat", namespace: first),
                          DocDisk.chat2URL(for: "same-chat", namespace: second))
    }

    func testInvitationOnlyPrefillsPairingAndRejectsAmbiguousCredentials() throws {
        let url = URL(string: "zeron://private/join?hub=https%3A%2F%2Fworkspace.example.ts.net&code=123456")!
        let invitation = try XCTUnwrap(PrivateInvitation(url: url))
        XCTAssertEqual(invitation.hubURL, "https://workspace.example.ts.net")
        XCTAssertEqual(invitation.code, "123456")
        XCTAssertNil(PrivateInvitation(url: URL(string: url.absoluteString + "&code=654321")!))
        XCTAssertNil(PrivateInvitation(url: URL(string: "zeron://callback?code=123456")!))
    }

    func testHubValidationRejectsInsecureAndCredentialBearingAddresses() throws {
        XCTAssertEqual(try PrivateWorkspaceClient.hubURL("https://workspace.example.ts.net/"),
                       URL(string: "https://workspace.example.ts.net"))
        XCTAssertEqual(try PrivateWorkspaceClient.hubURL("http://127.0.0.1:9000"),
                       URL(string: "http://127.0.0.1:9000"))
        for url in ["http://workspace.example.ts.net", "https://user:password@hub.example",
                    "https://hub.example?token=secret", "https://hub.example/path", "file:///tmp/hub"] {
            XCTAssertThrowsError(try PrivateWorkspaceClient.hubURL(url))
        }
        XCTAssertThrowsError(try PrivateWorkspaceInfo(protocolVersion: 2, workspaceId: "w", name: "n", capabilities: []).validate())
    }

    func testClientDevicesCannotHostSessions() {
        XCTAssertFalse(DeviceRow(id: "client", name: "Viewer", platform: "linux", role: "client").canHostSessions)
        XCTAssertFalse(DeviceRow(id: "phone", name: "Mobile", platform: "ios", role: "server").canHostSessions)
        XCTAssertTrue(DeviceRow(id: "server", name: "Server", platform: "linux", role: "server").canHostSessions)
        XCTAssertTrue(DeviceRow(id: "legacy", name: "Cloud", platform: "linux").canHostSessions)
    }

    func testPairingUsesHubProtocolAndAlwaysEnrollsAsClient() async throws {
        let recorder = PrivatePairRecorder()
        PrivatePairProtocol.recorder = recorder
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [PrivatePairProtocol.self]
        let session = URLSession(configuration: configuration)
        defer { session.invalidateAndCancel() }
        let client = PrivateWorkspaceClient(hubURL: profile().hubURL, session: session)
        let (profile, token) = try await client.pair(code: "123456", name: "Mobile", deviceId: "ios-client")
        XCTAssertEqual(profile.workspaceId, "workspace-a")
        XCTAssertEqual(profile.userId, "workspace-user")
        XCTAssertEqual(profile.deviceId, "ios-client")
        XCTAssertEqual(token, "node-token")
        let requests = recorder.requests
        XCTAssertEqual(requests.map { $0.url!.path }, ["/private/info", "/private/pair"])
        XCTAssertTrue(requests.allSatisfy { $0.url?.host == "workspace.example.ts.net" })
        XCTAssertTrue(requests.allSatisfy { $0.value(forHTTPHeaderField: "Authorization") == nil })
        let pair = try XCTUnwrap(requests.last)
        XCTAssertEqual(pair.httpMethod, "POST")
        let body = try XCTUnwrap(recorder.pairBody)
        XCTAssertEqual(body, ["code": "123456", "name": "Mobile", "deviceId": "ios-client", "role": "client"])
    }
}

private final class PrivatePairRecorder: @unchecked Sendable {
    private let lock = NSLock()
    private var captured: [URLRequest] = []
    private var body: [String: String]?

    var requests: [URLRequest] { lock.withLock { captured } }
    var pairBody: [String: String]? { lock.withLock { body } }

    func record(_ request: URLRequest) {
        var bytes = request.httpBody
        if bytes == nil, let stream = request.httpBodyStream {
            stream.open()
            defer { stream.close() }
            var data = Data()
            var buffer = [UInt8](repeating: 0, count: 1024)
            while stream.hasBytesAvailable {
                let count = stream.read(&buffer, maxLength: buffer.count)
                guard count > 0 else { break }
                data.append(contentsOf: buffer.prefix(count))
            }
            bytes = data
        }
        lock.withLock {
            captured.append(request)
            if let bytes, request.httpMethod == "POST" {
                body = try? JSONDecoder().decode([String: String].self, from: bytes)
            }
        }
    }
}

private final class PrivatePairProtocol: URLProtocol, @unchecked Sendable {
    nonisolated(unsafe) static var recorder = PrivatePairRecorder()

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        Self.recorder.record(request)
        let body: String
        switch request.url!.path {
        case "/private/info":
            body = #"{"protocolVersion":1,"workspaceId":"workspace-a","name":"Development","capabilities":[]}"#
        case "/private/pair":
            body = #"{"workspaceId":"workspace-a","userId":"workspace-user","deviceId":"ios-client","token":"node-token"}"#
        default:
            client?.urlProtocol(self, didFailWithError: URLError(.unsupportedURL))
            return
        }
        let response = HTTPURLResponse(url: request.url!, statusCode: 200, httpVersion: nil, headerFields: nil)!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: Data(body.utf8))
        client?.urlProtocolDidFinishLoading(self)
    }

    override func stopLoading() {}
}
