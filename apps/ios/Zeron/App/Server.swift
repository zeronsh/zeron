import Foundation

/// A server the app signs in to, as stored: beside the credentials (the one
/// signed in to) and in settings (the one sign-in will use). What the UI
/// shows of it (host, label) comes back from the core, so both phones say the
/// same.
struct SavedServer: Codable, Equatable {
    /// Origin, `https://host[:port]`: the edge everything talks to.
    var url: String
    /// The WorkOS client its sign-in goes through.
    var workosClientId: String
    /// What its operator calls it, when they named it.
    var name: String?

    init(url: String, workosClientId: String, name: String? = nil) {
        self.url = url
        self.workosClientId = workosClientId
        self.name = name
    }

    init(_ info: ServerInfo) {
        self.init(url: info.url, workosClientId: info.workosClientId, name: info.name)
    }

    /// The same server: its edge and its sign-in client (what its operator
    /// calls it aside). One that kept its address under another WorkOS client
    /// is another server.
    func isSame(as other: SavedServer) -> Bool {
        url == other.url && workosClientId == other.workosClientId
    }

    var info: ServerInfo { serverInfo(url: url, workosClientId: workosClientId, name: name) }
    var isOfficial: Bool { info.isOfficial }

    /// Zeron's own server.
    static let official = SavedServer(serverOfficial())

    /// `-edge <url>` / `-workos <client>` (debug builds): another stack in
    /// place of whichever server is stored or chosen. Read once; the flags
    /// stick across launches until `prod` (`Endpoints.debugOverride`).
    static let launchOverride: SavedServer? = {
        #if DEBUG
        let edge = Endpoints.debugOverride("-edge", key: "debugEdgeURL")
        let client = Endpoints.debugOverride("-workos", key: "debugWorkosClientId")
        guard edge != nil || client != nil else { return nil }
        let official = SavedServer.official
        return SavedServer(url: edge ?? official.url, workosClientId: client ?? official.workosClientId)
        #else
        return nil
        #endif
    }()

    /// The server requests actually go to: this one, unless a launch
    /// override stands in for it. Only the stored server is ever saved.
    var reached: SavedServer { Self.launchOverride ?? self }

    /// The `.owner` of the core's store. Zeron's server keeps the string from
    /// before servers, so existing caches survive; another server's account
    /// is its own, even with the same ids.
    func coreOwner(userId: String, orgId: String) -> String {
        isOfficial ? "\(userId)/\(orgId)" : "\(url)|\(userId)/\(orgId)"
    }
}

enum ServerChoice {
    /// The server the app uses: a launch override, else the one signed in
    /// to, else the one chosen for signing in, else Zeron's.
    static func resolve(override: SavedServer?, account: SavedServer?, chosen: SavedServer?) -> SavedServer {
        override ?? account ?? chosen ?? .official
    }
}

/// The server sign-in uses, chosen on the sign-in screen (settings key
/// `server`, JSON `{url, workosClientId, name?}`). Absent means Zeron's; it
/// outlives signing out, so the sign-in screen still shows the server in use.
enum ChosenServer {
    static let key = "server"
    /// Posted when the choice changes (the welcome's server capsule follows).
    static let didChange = Notification.Name("ChosenServer.didChange")

    static func load(_ defaults: UserDefaults = .standard) -> SavedServer? {
        guard let json = defaults.string(forKey: key),
              let server = try? JSONDecoder().decode(SavedServer.self, from: Data(json.utf8)),
              !server.url.isEmpty, !server.workosClientId.isEmpty
        else { return nil }
        return server
    }

    /// Zeron's server (or nil) removes the key.
    static func save(_ server: SavedServer?, _ defaults: UserDefaults = .standard) {
        if let server, !server.isOfficial, let data = try? JSONEncoder().encode(server) {
            defaults.set(String(decoding: data, as: UTF8.self), forKey: key)
        } else {
            defaults.removeObject(forKey: key)
        }
        NotificationCenter.default.post(name: didChange, object: nil)
    }
}

/// An invite to a server: `zeron://server?address=<address>`, the address as
/// someone would type it (`zeron.acme.com`, `https://zeron.acme.com`,
/// `http://localhost:8899`), percent-encoded. It only ever fills in the
/// server sheet; switching still takes a tap.
enum ServerLink {
    static let host = "server"

    /// The address a link carries; nil when it isn't one (another scheme,
    /// host or path) or has no address worth looking up.
    static func address(from url: URL) -> String? {
        guard url.scheme?.lowercased() == Endpoints.callbackScheme,
              url.host?.lowercased() == host,
              url.path.isEmpty || url.path == "/",
              let items = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems,
              let address = items.first(where: { $0.name == "address" })?.value?.trimmingCharacters(in: .whitespacesAndNewlines),
              !address.isEmpty
        else { return nil }
        return address
    }
}
