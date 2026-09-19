import CryptoKit
import Foundation

struct PrivateWorkspaceInfo: Codable, Equatable {
    let protocolVersion: Int
    let workspaceId: String
    let name: String
    let capabilities: [String]

    func validate() throws {
        guard protocolVersion == 1 else { throw PrivateWorkspaceError.unsupportedProtocol }
        guard !workspaceId.isEmpty, !name.isEmpty else { throw PrivateWorkspaceError.invalidResponse }
    }
}

struct PrivatePairResponse: Codable {
    let workspaceId: String
    let userId: String
    let deviceId: String
    let token: String
}

struct PrivateWorkspaceProfile: Codable, Equatable {
    let hubURL: URL
    let workspaceId: String
    let userId: String
    let deviceId: String
    let name: String
    let deviceName: String

    var tokenKey: String {
        let identity = [hubURL.absoluteString, workspaceId, deviceId]
        let bytes = (try? JSONEncoder().encode(identity)) ?? Data()
        return "private." + SHA256.hash(data: bytes).map { String(format: "%02x", $0) }.joined()
    }

    func config(token: String) -> AppConfig {
        AppConfig(edgeURL: hubURL, mode: .privateWorkspace, userId: userId, orgId: workspaceId,
                  deviceId: deviceId, deviceName: deviceName, privateBearer: token)
    }
}

struct PrivateInvitation: Identifiable, Equatable {
    let id = UUID()
    let hubURL: String
    let code: String

    init(hubURL: String = "", code: String = "") {
        self.hubURL = hubURL
        self.code = code
    }

    init?(url: URL) {
        guard url.scheme == "zeron", url.host == "private", url.path == "/join",
              let items = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems,
              items.filter({ $0.name == "hub" }).count == 1,
              items.filter({ $0.name == "code" }).count == 1,
              let hub = items.first(where: { $0.name == "hub" })?.value,
              let code = items.first(where: { $0.name == "code" })?.value,
              (try? PrivateWorkspaceClient.hubURL(hub)) != nil,
              !code.isEmpty else { return nil }
        self.init(hubURL: hub, code: code)
    }
}

enum PrivateWorkspaceError: LocalizedError {
    case invalidURL
    case unsupportedProtocol
    case invalidResponse
    case invalidInput
    case http(Int)
    case keychain

    var errorDescription: String? {
        switch self {
        case .invalidURL: "Enter the hub's HTTPS address from Tailscale."
        case .unsupportedProtocol: "This hub uses an unsupported private workspace protocol."
        case .invalidResponse: "The hub returned an invalid workspace identity."
        case .invalidInput: "Enter a pairing code and a name for this device."
        case .http(let status): "Pairing failed (HTTP \(status)). Check the code and hub access."
        case .keychain: "The device credential could not be saved securely. Try pairing again."
        }
    }
}

struct PrivateWorkspaceClient {
    let hubURL: URL
    var session: URLSession = .shared

    static func hubURL(_ input: String) throws -> URL {
        guard var components = URLComponents(string: input.trimmingCharacters(in: .whitespacesAndNewlines)),
              let host = components.host, !host.isEmpty,
              components.user == nil, components.password == nil,
              components.query == nil, components.fragment == nil,
              components.path.isEmpty || components.path == "/",
              components.scheme == "https" ||
                (components.scheme == "http" && ["localhost", "127.0.0.1", "[::1]"].contains(host))
        else { throw PrivateWorkspaceError.invalidURL }
        components.path = ""
        guard let url = components.url else { throw PrivateWorkspaceError.invalidURL }
        return url
    }

    func info() async throws -> PrivateWorkspaceInfo {
        let info: PrivateWorkspaceInfo = try await send(URLRequest(url: hubURL.appending(path: "private/info")))
        try info.validate()
        return info
    }

    func pair(code: String, name: String, deviceId: String) async throws
        -> (PrivateWorkspaceProfile, String) {
        let code = code.trimmingCharacters(in: .whitespacesAndNewlines)
        let name = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !code.isEmpty, !name.isEmpty, name.count <= 100, !deviceId.isEmpty
        else { throw PrivateWorkspaceError.invalidInput }
        let info = try await info()
        var request = URLRequest(url: hubURL.appending(path: "private/pair"))
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONEncoder().encode([
            "code": code, "name": name, "deviceId": deviceId, "role": "client",
        ])
        let paired: PrivatePairResponse = try await send(request)
        guard paired.workspaceId == info.workspaceId, paired.deviceId == deviceId,
              !paired.userId.isEmpty, !paired.token.isEmpty
        else { throw PrivateWorkspaceError.invalidResponse }
        return (PrivateWorkspaceProfile(hubURL: hubURL, workspaceId: paired.workspaceId,
                    userId: paired.userId, deviceId: paired.deviceId, name: info.name, deviceName: name),
                paired.token)
    }

    private func send<T: Decodable>(_ request: URLRequest) async throws -> T {
        var request = request
        request.timeoutInterval = 20
        request.cachePolicy = .reloadIgnoringLocalCacheData
        let (data, response) = try await session.data(for: request)
        guard let http = response as? HTTPURLResponse else { throw PrivateWorkspaceError.invalidResponse }
        guard (200..<300).contains(http.statusCode) else { throw PrivateWorkspaceError.http(http.statusCode) }
        return try JSONDecoder().decode(T.self, from: data)
    }
}
