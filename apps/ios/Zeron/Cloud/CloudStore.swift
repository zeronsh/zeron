import AuthenticationServices
import UIKit

/// Cloud for this account: status and credits, GitHub, the agents' logins,
/// billing. Every screen reads it and observes it; reads are cached so a
/// screen paints at once and revalidates. Main-thread only.
@MainActor
final class CloudStore {
    /// The agents Cloud runs, with the vault providers behind them.
    struct Agent: Equatable {
        let harness: String
        let name: String
        /// Subscription sign-in provider (`claude` · `codex`).
        let login: String
        /// API key provider (`anthropic-key` · `openai-key`).
        let key: String
        /// What signing in is called ("Sign in with Claude").
        let signInTitle: String
        let keyName: String
    }

    static let agents: [Agent] = [
        Agent(harness: "claude-code", name: "Claude Code", login: "claude", key: "anthropic-key", signInTitle: "Sign in with Claude", keyName: "Anthropic API key"),
        Agent(harness: "codex", name: "Codex", login: "codex", key: "openai-key", signInTitle: "Sign in with ChatGPT", keyName: "OpenAI API key"),
    ]

    private(set) var info: CloudInfo?
    private(set) var vault: VaultInfo?
    private(set) var repos: [GithubRepoInfo]?
    private(set) var accounts: [String: [CloudAccountInfo]] = [:]
    /// Each Cloud login's plan usage, by `provider:slot` (read by the vault),
    /// and why it couldn't be read.
    private(set) var usage: [String: CloudUsageInfo] = [:]
    private(set) var usageErrors: [String: String] = [:]
    private(set) var billing: BillingInfo?
    /// Why the last billing read failed (shown with Retry until one succeeds).
    private(set) var billingError: String?
    /// The account's session machines (read when a Cloud chat needs them).
    private(set) var sessions: [CloudSessionInfo] = []
    private(set) var loading = false
    private(set) var error: String?

    private weak var app: AppModel?
    private var observers: [UUID: () -> Void] = [:]
    private var refreshTask: Task<Void, Never>?

    init(app: AppModel) {
        self.app = app
    }

    private var client: CoreClient? { app?.client }

    // MARK: Reading

    var available: Bool { info?.available ?? false }
    var credits: Double? { billing.map(\.creditsTotal) ?? info?.credits }

    /// Cloud can't start or wake a machine (the desktop's rule).
    var outOfCredits: Bool { (info?.credits ?? billing?.creditsTotal).map { $0 <= 0 } ?? false }

    /// What's wrong with Cloud as a whole, for the top of its page.
    struct Problem {
        let title: String
        let detail: String
        /// Credits fix it: the row leads to Billing.
        let billing: Bool
    }

    var problem: Problem? {
        guard let info, info.available else { return nil }
        if info.state == "error" {
            return Problem(title: "Cloud ran into a problem", detail: info.error ?? "Something went wrong with Cloud.", billing: false)
        }
        if outOfCredits {
            return Problem(title: "Out of credits", detail: "Cloud sessions can't start or wake until you add credits.", billing: true)
        }
        return nil
    }

    /// Why a Cloud chat's machine isn't running, when the edge said.
    func sessionError(chat chatId: String, device: String?) -> String? {
        let session = sessions.first { $0.chatId == chatId } ?? sessions.first { $0.deviceId == device }
        guard let session, session.state == "error" else { return nil }
        return session.error ?? "Its machine failed to start."
    }

    private var sessionsRead: Date?

    /// Re-read the session machines (at most every few seconds).
    func refreshSessions() {
        guard let client, available || info == nil else { return }
        if let last = sessionsRead, Date().timeIntervalSince(last) < 8 { return }
        sessionsRead = Date()
        Task { [weak self] in
            let list = try? await client.cloudSessions()
            let status = try? await client.cloudStatus()
            guard self?.client === client else { return }
            if let list { self?.sessions = list }
            self?.info = status ?? self?.info
            self?.changed()
        }
    }

    var githubAccount: String? {
        vault?.connections.first { $0.provider == "github" && !$0.needsReconnect }?.account
    }

    var githubConnected: Bool {
        vault?.connections.contains { $0.provider == "github" && !$0.needsReconnect } ?? false
    }

    /// The active login (or key) Cloud uses for `agent`, by its friendliest name.
    func activeAccount(_ agent: Agent) -> CloudAccountInfo? {
        accounts[agent.login]?.first(where: \.active) ?? accounts[agent.key]?.first(where: \.active)
    }

    func hasAccount(_ agent: Agent) -> Bool {
        !(accounts[agent.login] ?? []).isEmpty || !(accounts[agent.key] ?? []).isEmpty
            || vault?.connections.contains { ($0.provider == agent.login || $0.provider == agent.key) && $0.accounts > 0 } == true
    }

    /// Cloud reaches `repo` (`owner/name`): its sessions can run there.
    func reaches(_ repo: String?) -> Bool {
        guard available, let repo, let repos else { return false }
        return repos.contains { $0.fullName.caseInsensitiveCompare(repo) == .orderedSame }
    }

    // MARK: Observation

    @discardableResult
    func observe(_ handler: @escaping () -> Void) -> AnyObject {
        let id = UUID()
        observers[id] = handler
        return Token { [weak self] in self?.observers[id] = nil }
    }

    private final class Token {
        let cancel: () -> Void
        init(cancel: @escaping () -> Void) { self.cancel = cancel }
        deinit { cancel() }
    }

    private func changed() {
        observers.values.forEach { $0() }
    }

    // MARK: Loading

    /// Re-read everything (status first: the rest needs Cloud set up).
    func refresh() {
        guard let client, refreshTask == nil else { return }
        loading = true
        // The task is recorded before anyone hears of the change: an observer
        // that re-renders and asks again must find this read in flight.
        refreshTask = Task { [weak self] in
            defer {
                // Signed out (or into another account) meanwhile: reset()
                // already cleared this; never clobber the new account's.
                if self?.client === client {
                    self?.refreshTask = nil
                    self?.loading = false
                    self?.changed()
                }
            }
            do {
                var info = try await client.cloudStatus()
                // Cloud is part of every signed-in account: set it up.
                if info.available, info.state == "off" {
                    info = (try? await client.cloudEnable()) ?? info
                }
                guard self?.client === client else { return }
                self?.info = info
                self?.error = nil
                self?.changed()
                guard info.available else { return }
                async let vault = try? client.vaultStatus()
                async let repos = try? client.githubRepos(query: nil)
                async let billing = try? client.billingStatus()
                var lists: [String: [CloudAccountInfo]] = [:]
                for agent in Self.agents {
                    for provider in [agent.login, agent.key] {
                        lists[provider] = (try? await client.vaultAccounts(provider: provider)) ?? []
                    }
                }
                let (v, r, b) = (await vault, await repos, await billing)
                guard self?.client === client else { return }
                self?.vault = v
                self?.repos = r ?? []
                self?.billing = b
                self?.accounts = lists
                self?.refreshUsage()
            } catch {
                guard let self, self.client === client else { return }
                self.error = Self.message(error)
                // Until a read answers, Cloud reads as not offered: ask again
                // (30 s, doubling to 5 min) rather than hide it for good.
                let delay = self.retryDelay
                self.retryDelay = min(delay * 2, 300)
                Task { [weak self] in
                    try? await Task.sleep(nanoseconds: UInt64(delay * 1_000_000_000))
                    guard let self, self.client === client, self.info == nil || self.error != nil else { return }
                    self.refresh()
                }
                return
            }
            self?.retryDelay = 30
        }
        changed()
    }

    /// How long until a failed status read is tried again.
    private var retryDelay: Double = 30

    func refreshAccounts() {
        guard let client else { return }
        Task { [weak self] in
            var lists: [String: [CloudAccountInfo]] = [:]
            for agent in Self.agents {
                for provider in [agent.login, agent.key] {
                    lists[provider] = (try? await client.vaultAccounts(provider: provider)) ?? []
                }
            }
            let vault = try? await client.vaultStatus()
            guard self?.client === client else { return }
            self?.accounts = lists
            self?.vault = vault ?? self?.vault
            self?.changed()
            self?.refreshUsage()
        }
    }

    /// Re-read every Cloud login's plan usage (keys have none).
    func refreshUsage() {
        guard let client else { return }
        let logins = Self.agents.flatMap { agent in
            (accounts[agent.login] ?? []).map { (agent.login, $0.slot) }
        }
        Task { [weak self] in
            await withTaskGroup(of: (String, Result<CloudUsageInfo, Error>).self) { group in
                for (provider, slot) in logins {
                    group.addTask {
                        do {
                            return ("\(provider):\(slot)", .success(try await client.vaultUsage(provider: provider, slot: slot)))
                        } catch {
                            return ("\(provider):\(slot)", .failure(error))
                        }
                    }
                }
                for await (key, result) in group {
                    guard self?.client === client else { continue }
                    switch result {
                    case let .success(usage):
                        self?.usage[key] = usage
                        self?.usageErrors[key] = nil
                    case let .failure(error):
                        self?.usageErrors[key] = Self.message(error)
                    }
                }
            }
            self?.changed()
        }
    }

    func refreshGitHub() async {
        guard let client else { return }
        let vault = try? await client.vaultStatus()
        let repos = try? await client.githubRepos(query: nil)
        guard self.client === client else { return }
        self.vault = vault ?? self.vault
        self.repos = repos ?? self.repos
        changed()
    }

    func refreshBilling() async {
        guard let client else { return }
        let read: Result<BillingInfo, Error>
        do {
            read = .success(try await client.billingStatus())
        } catch {
            read = .failure(error)
        }
        let status = try? await client.cloudStatus()
        guard self.client === client else { return }
        switch read {
        case let .success(value):
            billing = value
            billingError = nil
        case let .failure(error):
            billingError = Self.message(error)
        }
        info = status ?? info
        changed()
    }

    /// Forget everything (sign-out).
    func reset() {
        refreshTask?.cancel()
        refreshTask = nil
        info = nil
        vault = nil
        repos = nil
        accounts = [:]
        usage = [:]
        usageErrors = [:]
        billing = nil
        billingError = nil
        retryDelay = 30
        sessions = []
        sessionsRead = nil
        error = nil
        changed()
    }

    // MARK: Accounts

    func activate(_ account: CloudAccountInfo, provider: String) async throws {
        guard let client else { return }
        accounts[provider] = try await client.vaultActivate(provider: provider, slot: account.slot)
        changed()
    }

    func remove(_ account: CloudAccountInfo, provider: String) async throws {
        guard let client else { return }
        accounts[provider] = try await client.vaultForget(provider: provider, slot: account.slot)
        changed()
        refreshAccounts()
    }

    func saveKey(_ key: String, for agent: Agent) async throws {
        guard let client else { return }
        try await client.vaultPutApiKey(provider: agent.key, key: key)
        refreshAccounts()
    }

    // MARK: Projects and sessions

    @discardableResult
    func createProject(repo: String) async throws -> String {
        guard let client else { throw CoreError.Closed }
        let id = try await client.createCloudProject(repo: repo)
        app?.workspaceDidChange()
        return id
    }

    static func message(_ error: Error) -> String {
        guard let core = error as? CoreError else { return error.localizedDescription }
        switch core {
        case let .NotFound(m), let .InvalidArgument(m), let .HostUnavailable(m), let .Unsupported(m),
             let .HostError(m), let .Network(m), let .Auth(m), let .Storage(m), let .NotImplemented(m), let .Internal(m):
            return m
        case .Closed:
            return "You're signed out."
        }
    }
}

/// Signs in to Claude or ChatGPT for Cloud: the provider's own page in the
/// system auth sheet, which closes by itself once the provider answers; the
/// login then lands in Cloud's vault (no token stays on the phone).
@MainActor
final class ProviderSignIn: NSObject, ASWebAuthenticationPresentationContextProviding {
    private var session: ASWebAuthenticationSession?
    private weak var anchor: UIView?

    static let doneURL = "zeron://signed-in"

    func run(_ agent: CloudStore.Agent, client: CoreClient, from view: UIView) async throws {
        anchor = view
        let start = try await client.providerLoginStart(provider: agent.login, doneUrl: Self.doneURL)
        guard let url = URL(string: start.url) else { throw CoreError.Internal(message: "Bad sign-in link") }
        let cancelled: Bool = await withCheckedContinuation { cont in
            let s = ASWebAuthenticationSession(url: url, callback: .customScheme("zeron")) { _, error in
                let wasCancelled = (error as? ASWebAuthenticationSessionError)?.code == .canceledLogin
                cont.resume(returning: wasCancelled)
            }
            s.presentationContextProvider = self
            // The browser's own cookies: already signed in to claude.ai or
            // chatgpt.com there means one tap.
            s.prefersEphemeralWebBrowserSession = false
            session = s
            if !s.start() { cont.resume(returning: true) }
        }
        session = nil
        if cancelled {
            client.providerLoginCancel(loginId: start.loginId)
            throw CancellationError()
        }
        try await client.providerLoginFinish(loginId: start.loginId)
    }

    func presentationAnchor(for session: ASWebAuthenticationSession) -> ASPresentationAnchor {
        anchor?.window ?? ASPresentationAnchor()
    }
}
