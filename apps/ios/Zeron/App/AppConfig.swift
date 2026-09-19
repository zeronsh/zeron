// Session-wide connection identity and authentication for HTTP and room sockets.

import CryptoKit
import Foundation

final class AppConfig: @unchecked Sendable {
    enum Mode: String {
        case workos
        case dev
        case privateWorkspace = "private"
    }

    let edgeURL: URL
    let mode: Mode
    let userId: String
    let orgId: String
    let deviceId: String
    let deviceName: String

    private let lock = NSLock()
    private var tokens: AuthTokens?
    private var devBearer: String?
    private var privateBearer: String?
    private var invalidated = false
    /// In-flight refresh shared by every caller (single-flight). WorkOS
    /// refresh tokens are SINGLE-USE (rotated per use, desktop auth.rs
    /// refresh_gate): without this, a cold launch's N room dials raced N
    /// concurrent refreshes with the same token — one won and rotated it,
    /// the rest failed, dialed with the dead access token, got rejected,
    /// and every socket sat in backoff. That was the 5–10s "connecting"
    /// stall on every app open past token expiry (~5 min).
    private var refreshTask: Task<String?, Never>?

    init(edgeURL: URL, mode: Mode, userId: String, orgId: String,
         deviceId: String, deviceName: String,
         tokens: AuthTokens? = nil, devBearer: String? = nil, privateBearer: String? = nil) {
        self.edgeURL = edgeURL
        self.mode = mode
        self.userId = userId
        self.orgId = orgId
        self.deviceId = deviceId
        self.deviceName = deviceName
        self.tokens = tokens
        self.devBearer = devBearer
        self.privateBearer = privateBearer
    }

    var cacheNamespace: String {
        let identity = [mode.rawValue, edgeURL.absoluteString, orgId, userId, deviceId]
        let bytes = (try? JSONEncoder().encode(identity)) ?? Data()
        return SHA256.hash(data: bytes).map { String(format: "%02x", $0) }.joined()
    }

    func invalidate() {
        lock.withLock {
            invalidated = true
            tokens = nil
            devBearer = nil
            privateBearer = nil
            refreshTask?.cancel()
        }
    }

    func updateTokens(_ new: AuthTokens) {
        lock.withLock {
            tokens = new
        }
    }

    /// Current bearer, refreshing the WorkOS access token when needed.
    func currentToken() async -> String? {
        guard !lock.withLock({ invalidated }) else { return nil }
        switch mode {
        case .privateWorkspace:
            return lock.withLock { privateBearer }
        case .dev:
            return lock.withLock { devBearer }
        case .workos:
            let current = lock.withLock { tokens }
            guard let current else { return nil }
            if !Self.isExpired(jwt: current.accessToken) {
                return current.accessToken
            }
            return await refreshedToken(current: current)
        }
    }

    /// Join (or start) the one in-flight refresh. The task clears itself
    /// under the lock as its last act, so a caller either joins a live
    /// refresh or starts a fresh one — never a second concurrent POST.
    private func refreshedToken(current: AuthTokens) async -> String? {
        let task = lock.withLock {
            if let existing = refreshTask {
                return existing
            }

            let task = Task<String?, Never> { [edgeURL, orgId] in
                let client = AuthClient(baseURL: edgeURL)
                let refreshed = try? await client.refresh(refreshToken: current.refreshToken,
                                                          organizationId: orgId)
                guard !Task.isCancelled, !self.lock.withLock({ self.invalidated }) else { return nil }
                if let refreshed {
                    self.updateTokens(refreshed)
                    Keychain.save(refreshed.accessToken, key: "accessToken")
                    Keychain.save(refreshed.refreshToken, key: "refreshToken")
                } else {
                    roomLog.error("auth: token refresh failed; using expired access token (server will reject and rooms will redial)")
                }
                self.lock.withLock {
                    self.refreshTask = nil
                }
                // Failure falls back to the expired token: let the server
                // reject; the rooms' backoff redials retry through here.
                return refreshed?.accessToken ?? current.accessToken
            }
            refreshTask = task
            return task
        }
        return await task.value
    }

    private var wsBase: URL {
        var components = URLComponents(url: edgeURL, resolvingAgainstBaseURL: false)!
        components.scheme = components.scheme == "http" ? "ws" : "wss"
        return components.url!
    }

    /// The workspace registry room (docs/registry-sync.md) — the row-table
    /// replacement for the old ws Loro workspace doc.
    func registrySocketRequest() async -> URLRequest? {
        await socketRequest(path: "registry/\(orgId)/ws",
                            query: [URLQueryItem(name: "device", value: deviceId)])
    }

    /// The chat2 log-relay room (docs/chat2-sync.md B) — replaces the s2
    /// session rooms, which mobile no longer dials at all. `device` rides the
    /// URL so the DO can attribute sockets and honor excludeOwn backfills.
    func chat2SocketRequest(chatId: String) async -> URLRequest? {
        await socketRequest(path: "chat2/\(chatId)/ws",
                            query: [URLQueryItem(name: "device", value: deviceId)])
    }

    func deviceRelayRequest(deviceId: String, connectionId: String) async -> URLRequest? {
        await socketRequest(path: "device/\(deviceId)/ws", query: [
            URLQueryItem(name: "role", value: "client"),
            URLQueryItem(name: "connId", value: connectionId),
        ])
    }

    private func socketRequest(path: String, query: [URLQueryItem]) async -> URLRequest? {
        guard let token = await currentToken() else { return nil }
        var url = wsBase.appending(path: path)
        var items = query
        if mode != .privateWorkspace {
            items.append(URLQueryItem(name: "token", value: token))
        }
        url.append(queryItems: items)
        var request = URLRequest(url: url)
        if mode == .privateWorkspace {
            request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        return request
    }

    /// GET /chat2/{chatId}/checkpoint — the Range-resumable doc snapshot
    /// (auth via bearer header; the caller adds Range on resume).
    func chat2CheckpointRequest(chatId: String) async -> URLRequest? {
        guard let token = await currentToken() else { return nil }
        var request = URLRequest(url: edgeURL.appending(path: "chat2/\(chatId)/checkpoint"))
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        return request
    }

    /// GET /chat2/{chatId}/rows?after= — pull over plain HTTPS: one request
    /// collapses the socket's connect→hello→state→rowsReq→backfill, and it
    /// works on networks that strip WS upgrades (airplane wifi).
    func chat2RowsRequest(chatId: String, after: UInt64) async -> URLRequest? {
        guard let token = await currentToken() else { return nil }
        var url = edgeURL.appending(path: "chat2/\(chatId)/rows")
        url.append(queryItems: [URLQueryItem(name: "after", value: String(after)),
                                URLQueryItem(name: "device", value: deviceId)])
        var request = URLRequest(url: url)
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        return request
    }

    /// POST /chat2/{chatId}/rows?batchId= — push over plain HTTPS (batchId
    /// dedupe makes replays no-ops); body is the raw update batch.
    func chat2PushRequest(chatId: String, batchId: String) async -> URLRequest? {
        guard let token = await currentToken() else { return nil }
        var url = edgeURL.appending(path: "chat2/\(chatId)/rows")
        url.append(queryItems: [URLQueryItem(name: "batchId", value: batchId),
                                URLQueryItem(name: "device", value: deviceId)])
        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        return request
    }

    /// GET /registry/{orgId}/rows?since= — the WS hello's delta answer over
    /// plain HTTPS. `beat=1` doubles as a presence beat.
    func registryRowsRequest(since: UInt64?) async -> URLRequest? {
        guard let token = await currentToken() else { return nil }
        var url = edgeURL.appending(path: "registry/\(orgId)/rows")
        var items = [URLQueryItem(name: "device", value: deviceId),
                     URLQueryItem(name: "beat", value: "1")]
        if let since { items.append(URLQueryItem(name: "since", value: String(since))) }
        url.append(queryItems: items)
        var request = URLRequest(url: url)
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        return request
    }

    /// POST /registry/{orgId}/push — one op batch over plain HTTPS (LWW
    /// clocks make replays apply zero ops).
    func registryPushRequest() async -> URLRequest? {
        guard let token = await currentToken() else { return nil }
        var url = edgeURL.appending(path: "registry/\(orgId)/push")
        url.append(queryItems: [URLQueryItem(name: "device", value: deviceId)])
        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        return request
    }

    /// Decode the JWT payload's `exp` (60s early-refresh margin). Unparseable
    /// tokens read as non-expired — the server is the arbiter.
    private static func isExpired(jwt: String) -> Bool {
        let segments = jwt.split(separator: ".")
        guard segments.count == 3 else { return false }
        var base64 = String(segments[1]).replacingOccurrences(of: "-", with: "+")
            .replacingOccurrences(of: "_", with: "/")
        while base64.count % 4 != 0 { base64 += "=" }
        guard let data = Data(base64Encoded: base64),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let exp = obj["exp"] as? TimeInterval else { return false }
        return Date().timeIntervalSince1970 > exp - 60
    }

    /// GET /device/{deviceId}/status → whether the device's relay HOST socket
    /// is currently attached (distinct from workspace presence).
    func deviceStatus(deviceId: String) async -> String {
        guard let token = await currentToken() else { return "no-token" }
        var request = URLRequest(url: edgeURL.appending(path: "device/\(deviceId)/status"))
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        guard let (data, response) = try? await URLSession.shared.data(for: request),
              let http = response as? HTTPURLResponse else { return "unreachable" }
        return "http=\(http.statusCode) body=\(String(data: data, encoding: .utf8) ?? "")"
    }

    /// POST /device/{deviceId}/nudge {chatId} — wake a cold host to drain the
    /// command queue.
    func nudge(deviceId: String, chatId: String) async {
        guard let token = await currentToken() else { return }
        var request = URLRequest(url: edgeURL.appending(path: "device/\(deviceId)/nudge"))
        request.httpMethod = "POST"
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try? JSONSerialization.data(withJSONObject: ["chatId": chatId])
        _ = try? await URLSession.shared.data(for: request)
    }
}
