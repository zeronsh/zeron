import XCTest
@testable import Zeron

/// The server beside the stored credentials, the one chosen for signing in,
/// which of them the app uses, the core store each account owns, and the
/// address an invite link carries.
final class ServerTests: XCTestCase {
    private let acme = SavedServer(url: "https://acme.example.com", workosClientId: "client_acme", name: "Acme")
    private let tokens = AuthTokens(accessToken: "a", refreshToken: "r")

    private func dict(_ data: Data?) throws -> [String: String] {
        let data = try XCTUnwrap(data)
        return try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: String])
    }

    // MARK: Credentials JSON

    func testCredentialsFromBeforeServersAreOnZeron() throws {
        let old = Data(#"{"kind":"workos","userId":"u","orgId":"o","access":"a","refresh":"r"}"#.utf8)
        let stored = try XCTUnwrap(StoredCredentials(json: old))
        XCTAssertEqual(stored.credentials, .workOs(userId: "u", orgId: "o", tokens: tokens))
        XCTAssertEqual(stored.server, .official)
        XCTAssertTrue(stored.server.isOfficial)
    }

    func testZeronCredentialsKeepTheirOldShape() throws {
        let stored = StoredCredentials(credentials: .workOs(userId: "u", orgId: "o", tokens: tokens), server: .official)
        XCTAssertEqual(try dict(stored.json), ["kind": "workos", "userId": "u", "orgId": "o", "access": "a", "refresh": "r"])
    }

    func testASelfHostedServerIsStoredWithItsCredentials() throws {
        let stored = StoredCredentials(credentials: .workOs(userId: "u", orgId: "o", tokens: tokens), server: acme)
        let json = try dict(stored.json)
        XCTAssertEqual(json["edge"], "https://acme.example.com")
        XCTAssertEqual(json["workosClientId"], "client_acme")
        XCTAssertEqual(json["serverName"], "Acme")
        XCTAssertEqual(StoredCredentials(json: try XCTUnwrap(stored.json)), stored)
    }

    func testAnUnnamedServerStoresNoName() throws {
        let unnamed = SavedServer(url: "https://zeron.example.com", workosClientId: "client_x")
        let stored = StoredCredentials(credentials: .workOs(userId: "u", orgId: "o", tokens: tokens), server: unnamed)
        let json = try dict(stored.json)
        XCTAssertNil(json["serverName"])
        XCTAssertEqual(json["edge"], "https://zeron.example.com")
        XCTAssertEqual(StoredCredentials(json: try XCTUnwrap(stored.json))?.server, unnamed)
    }

    func testAnEdgeWithoutAClientStillDecodes() throws {
        let data = Data(#"{"kind":"workos","userId":"u","orgId":"o","access":"a","refresh":"r","edge":"https://acme.example.com"}"#.utf8)
        let stored = try XCTUnwrap(StoredCredentials(json: data))
        XCTAssertEqual(stored.server, SavedServer(url: "https://acme.example.com", workosClientId: ""))
    }

    func testDevCredentialsRoundTrip() throws {
        let stored = StoredCredentials(credentials: .dev(userId: "u", orgId: "o"), server: acme)
        XCTAssertEqual(StoredCredentials(json: try XCTUnwrap(stored.json)), stored)
        let zeron = StoredCredentials(credentials: .dev(userId: "u", orgId: "o"), server: .official)
        XCTAssertEqual(try dict(zeron.json), ["kind": "dev", "userId": "u", "orgId": "o"])
    }

    func testTheDemoStoresNothing() {
        XCTAssertNil(StoredCredentials(credentials: .demo(options: AppModel.demoOptions()), server: .official).json)
    }

    func testGarbageIsNoCredentials() {
        XCTAssertNil(StoredCredentials(json: Data("nope".utf8)))
        XCTAssertNil(StoredCredentials(json: Data(#"{"kind":"other"}"#.utf8)))
        XCTAssertNil(StoredCredentials(json: Data(#"{"kind":"workos","userId":"u"}"#.utf8)))
    }

    // MARK: Which server

    func testPrecedence() {
        let override = SavedServer(url: "https://staging.example.com", workosClientId: "client_staging")
        let account = acme
        let chosen = SavedServer(url: "https://chosen.example.com", workosClientId: "client_chosen")
        XCTAssertEqual(ServerChoice.resolve(override: override, account: account, chosen: chosen), override)
        XCTAssertEqual(ServerChoice.resolve(override: override, account: nil, chosen: nil), override)
        XCTAssertEqual(ServerChoice.resolve(override: nil, account: account, chosen: chosen), account)
        XCTAssertEqual(ServerChoice.resolve(override: nil, account: account, chosen: nil), account)
        XCTAssertEqual(ServerChoice.resolve(override: nil, account: nil, chosen: chosen), chosen)
        XCTAssertEqual(ServerChoice.resolve(override: nil, account: nil, chosen: nil), .official)
    }

    func testTheOfficialServer() {
        XCTAssertEqual(SavedServer.official.url, "https://edge.zeron.sh")
        XCTAssertTrue(SavedServer.official.isOfficial)
        XCTAssertEqual(SavedServer.official.info.label, "Zeron")
        XCTAssertFalse(acme.isOfficial)
        XCTAssertEqual(acme.info.label, "Acme")
        XCTAssertEqual(SavedServer(url: "https://zeron.example.com", workosClientId: "c").info.label, "zeron.example.com")
    }

    // MARK: Core store owner

    func testZeronKeepsTheOwnerFromBeforeServers() {
        XCTAssertEqual(SavedServer.official.coreOwner(userId: "u", orgId: "o"), "u/o")
    }

    func testASelfHostedAccountOwnsItsOwnStore() {
        XCTAssertEqual(acme.coreOwner(userId: "u", orgId: "o"), "https://acme.example.com|u/o")
    }

    func testAServerIsItsAddressAndItsSignInClient() {
        XCTAssertTrue(acme.isSame(as: SavedServer(url: acme.url, workosClientId: acme.workosClientId, name: "Renamed")))
        XCTAssertFalse(acme.isSame(as: SavedServer(url: acme.url, workosClientId: "client_new", name: "Acme")))
        XCTAssertFalse(acme.isSame(as: .official))
    }

    // MARK: Chosen server

    func testTheChosenServerIsRemembered() throws {
        let name = "ServerTests.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: name))
        defer { defaults.removePersistentDomain(forName: name) }

        XCTAssertNil(ChosenServer.load(defaults))
        ChosenServer.save(acme, defaults)
        XCTAssertEqual(ChosenServer.load(defaults), acme)
        let json = try XCTUnwrap(defaults.string(forKey: ChosenServer.key))
        let fields = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: String])
        XCTAssertEqual(fields, ["url": "https://acme.example.com", "workosClientId": "client_acme", "name": "Acme"])

        ChosenServer.save(.official, defaults)
        XCTAssertNil(defaults.object(forKey: ChosenServer.key))
        ChosenServer.save(acme, defaults)
        ChosenServer.save(nil, defaults)
        XCTAssertNil(defaults.object(forKey: ChosenServer.key))

        defaults.set("{not json", forKey: ChosenServer.key)
        XCTAssertNil(ChosenServer.load(defaults))
        defaults.set(#"{"url":"","workosClientId":"c"}"#, forKey: ChosenServer.key)
        XCTAssertNil(ChosenServer.load(defaults))
    }

    // MARK: Invite links

    private func linked(_ link: String) throws -> String? {
        ServerLink.address(from: try XCTUnwrap(URL(string: link)))
    }

    func testAnInviteCarriesAHost() throws {
        XCTAssertEqual(try linked("zeron://server?address=zeron.acme.com"), "zeron.acme.com")
    }

    func testAnInviteCarriesAPercentEncodedURL() throws {
        XCTAssertEqual(try linked("zeron://server?address=https%3A%2F%2Fzeron.acme.com"), "https://zeron.acme.com")
        XCTAssertEqual(try linked("zeron://server/?address=https%3A%2F%2Fzeron.acme.com%2F"), "https://zeron.acme.com/")
    }

    func testAnInviteCarriesALocalAddress() throws {
        XCTAssertEqual(try linked("zeron://server?address=http%3A%2F%2Flocalhost%3A8899"), "http://localhost:8899")
        XCTAssertEqual(try linked("ZERON://Server?address=http://localhost:8899"), "http://localhost:8899")
    }

    func testAnInviteWithoutAnAddressIsNone() throws {
        XCTAssertNil(try linked("zeron://server"))
        XCTAssertNil(try linked("zeron://server?"))
        XCTAssertNil(try linked("zeron://server?address="))
        XCTAssertNil(try linked("zeron://server?address=%20%20"))
        XCTAssertNil(try linked("zeron://server?addr=zeron.acme.com"))
    }

    func testOtherLinksAreNoInvite() throws {
        XCTAssertNil(try linked("zeron://voice?address=zeron.acme.com"))
        XCTAssertNil(try linked("zeron://callback?address=zeron.acme.com"))
        XCTAssertNil(try linked("zeron://server/join?address=zeron.acme.com"))
        XCTAssertNil(try linked("https://server?address=zeron.acme.com"))
        XCTAssertNil(try linked("other://server?address=zeron.acme.com"))
    }

    // MARK: Lookup errors

    func testALookupErrorShowsTheCoreSentence() {
        let error = ServerLookupError.Failed(code: "insecure", message: "Use an https:// address.")
        XCTAssertEqual(ServerSheetViewController.message(for: error), "Use an https:// address.")
    }
}
