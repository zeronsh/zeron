/**
 * The vault's RPC surface (docs/design/cloud-device.md). The vault Worker has
 * no routes: the main edge Worker reaches it only through the `VAULT` service
 * binding (`entrypoint: "VaultApi"`) and passes the identity it already
 * verified. This file is types only, so the main Worker can `import type`
 * it without pulling vault code into its bundle.
 */

/** The user's own API keys, one vendor's each (`{key}` material, never
 * refreshed). One key serves every agent that reaches its vendor. */
export type VaultKeyProviderId =
  | "anthropic-key"
  | "openai-key"
  | "openrouter-key"
  | "gemini-key"
  | "xai-key"
  | "deepseek-key"
  | "ai-gateway-key"
  | "meta-key"
  | "cursor-key";

export type VaultProviderId =
  | "codex"
  | "claude"
  | "github"
  | "copilot"
  | "devin"
  | "grok"
  | "xai-oauth"
  | "nous"
  | "antigravity"
  | "muse"
  | VaultKeyProviderId;

export const VAULT_KEY_PROVIDERS: readonly VaultKeyProviderId[] = [
  "anthropic-key",
  "openai-key",
  "openrouter-key",
  "gemini-key",
  "xai-key",
  "deepseek-key",
  "ai-gateway-key",
  "meta-key",
  "cursor-key"
];

export const VAULT_PROVIDERS: readonly VaultProviderId[] = [
  "codex",
  "claude",
  "github",
  "anthropic-key",
  "openai-key",
  "openrouter-key",
  "gemini-key",
  "xai-key",
  "deepseek-key",
  "ai-gateway-key",
  "meta-key",
  "cursor-key",
  "copilot",
  "devin",
  "grok",
  "xai-oauth",
  "nous",
  "antigravity",
  "muse"
];

/** What a client built before the providers above existed understands: a
 * status listing anything else would fail its whole read. */
export const LEGACY_VAULT_PROVIDERS: readonly VaultProviderId[] = [
  "codex",
  "claude",
  "github",
  "anthropic-key",
  "openai-key"
];

export const isKeyProvider = (provider: VaultProviderId): provider is VaultKeyProviderId =>
  (VAULT_KEY_PROVIDERS as readonly string[]).includes(provider);

/** Identity the main Worker verified (WorkOS JWT or runner JWT). */
export interface VaultCaller {
  readonly userId: string;
  readonly orgId: string;
  /** `runner` callers may only request grants for their own device. */
  readonly kind: "user" | "runner";
  /** Present for runners (from the runner JWT's `dev` claim). */
  readonly deviceId?: string;
}

export type VaultErrorCode =
  | "disabled"
  | "bad_request"
  | "forbidden"
  | "not_found"
  | "bad_signature"
  | "stale"
  | "device_unknown"
  | "device_revoked"
  | "not_authorized"
  | "needs_reconnect"
  | "upstream"
  | "unavailable";

export type VaultResult<T> =
  | { readonly ok: true; readonly value: T }
  | {
      readonly ok: false;
      readonly error: VaultErrorCode;
      readonly message: string;
      /** HTTP status the main Worker should answer with. */
      readonly status: number;
    };

/** One provider's connection, summarized from its active account (else its
 * most recent one). */
export interface VaultConnectionView {
  readonly provider: VaultProviderId;
  readonly status: "connected" | "needsReconnect";
  readonly authorizedDevices: readonly string[];
  /** Email / GitHub login / `…abcd` key suffix. Never secret material. */
  readonly account?: string;
  readonly updatedAt: number;
  /** How many accounts the provider holds (one is used: the active one). */
  readonly accounts?: number;
  /** Whether one of them is active (grants come from it). */
  readonly hasActive?: boolean;
}

/** Non-secret identity of an uploaded login, from the uploader's own sign-in
 * (Claude's profile endpoint). Display and de-duplication only. */
export interface VaultAccountProfile {
  readonly email?: string;
  readonly displayName?: string;
  readonly organization?: string;
  readonly plan?: string;
}

/**
 * One stored login of a provider. A provider holds any number; grants come
 * from the ACTIVE one. Signing in to an account already stored replaces it
 * (same email/organization, same API key), anything else adds one; a new
 * sign-in becomes active.
 */
export interface VaultAccountView {
  /** Stable id within the provider. */
  readonly slot: string;
  readonly status: "connected" | "needsReconnect";
  readonly active: boolean;
  /** `oauth` (a sign-in) · `setup-token` · `api-key` · `github`. */
  readonly kind: "oauth" | "setup-token" | "api-key" | "github";
  /** Email / GitHub login / `…abcd` key suffix / plan. Never secret. */
  readonly account?: string;
  readonly email?: string;
  readonly displayName?: string;
  readonly organization?: string;
  readonly plan?: string;
  /** GitHub logins: their picture on github.com. */
  readonly avatarUrl?: string;
  readonly authorizedDevices: readonly string[];
  readonly createdAt: number;
  readonly updatedAt: number;
}

/** A subscription login's plan usage, as its provider answered it (the
 * engine parses it with its own probe code). */
export interface VaultUsageView {
  readonly body: Record<string, unknown>;
  readonly fetchedAt: number;
}

export interface VaultDeviceView {
  readonly deviceId: string;
  readonly kind: "laptop" | "cloud";
  readonly enrolledAt: number;
  readonly revokedAt?: number;
  readonly parentId?: string;
}

/** One repository from `githubRepos` (mirrors Rust `GithubRepo`). */
export interface GithubRepoView {
  readonly fullName: string;
  readonly cloneUrl: string;
  readonly defaultBranch: string;
  readonly private: boolean;
  readonly description?: string;
  /** Unix ms of the last push. */
  readonly pushedAt: number;
  /** The App installation that gives access to this repository. Access comes
   * from here, not from a login; logins only say who may use it. */
  readonly installationId?: number;
  /** The connected login a new session on this repository would use. */
  readonly account?: string;
  /** Every connected login that reaches it, the one above first. */
  readonly accounts?: readonly string[];
}

/** One installation of the Zeron GitHub App: repository access, a record of
 * its own. It sits on an account or organization (`target`), and any number of
 * connected logins may reach it (`reachedBy`). */
export interface GithubInstallationView {
  readonly id: number;
  /** The account or organization it is installed on. */
  readonly target?: string;
  /** `User` · `Organization`. */
  readonly targetType?: string;
  readonly reachedBy: readonly string[];
  /** Where the repositories it covers are chosen on GitHub. */
  readonly manageUrl?: string;
}

/** A connected login whose repositories are missing from a listing. */
export interface GithubUnavailableView {
  readonly slot: string;
  readonly account?: string;
  /** `needs_reconnect`: it must be connected again. `upstream`: try later. */
  readonly reason: "needs_reconnect" | "upstream";
}

/** Everything the connected logins reach, merged across them. */
export interface GithubListingView {
  readonly repos: readonly GithubRepoView[];
  readonly installations: readonly GithubInstallationView[];
  /** Absent when every login answered. */
  readonly unavailable?: readonly GithubUnavailableView[];
}

export interface VaultStatusView {
  readonly connections: readonly VaultConnectionView[];
  readonly devices: readonly VaultDeviceView[];
  readonly available: true;
  /** Where the user installs the GitHub App on accounts / repositories
   * (absent when the App isn't configured). */
  readonly githubInstallUrl?: string;
}

/**
 * Upload material per provider:
 * - `codex`: `{ authJson }` — the parsed `auth.json` written by `codex login`
 *   (`{ tokens: { id_token, access_token, refresh_token, account_id }, last_refresh }`).
 * - `claude`: either `{ claudeAiOauth }` — the Claude Code credential blob from a
 *   sign-in through Claude's own OAuth flow (`{ accessToken, refreshToken, expiresAt,
 *   scopes, subscriptionType? }`, refreshable) — or `{ oauthToken, expiresAt? }`, a
 *   long-lived token from `claude setup-token` (not refreshable; default expiry one year).
 *   Running Claude Code on the user's own Cloud device with their subscription is
 *   permitted for Zeron under its arrangement with Anthropic; grants go only to the
 *   unmodified Claude Code CLI on the user's own devices.
 * - `*-key`: `{ key }` — the user's own API key. `cursor-key` may add
 *   `expiresAt` (Unix ms): a key Cursor's browser sign-in minted expires.
 * - `copilot`: `{ copilotToken, login }` — the GitHub token of Copilot's editor
 *   app (device flow) and the GitHub login it belongs to. Never expires.
 * - `devin`: `{ devinCredentials }` — Devin's `credentials.toml` as a table
 *   (`windsurf_api_key` and its server urls). Never refreshed.
 * - `grok`: `{ grokAuth, scope }` — one entry of the `auth.json` that
 *   `grok login` writes (`key`, `refresh_token`, `expires_at`, `oidc_issuer`,
 *   `oidc_client_id`, identity fields) and its map key `{issuer}::{client_id}`.
 *   Refreshed by the vault at the issuer (an xAI host only).
 * - `xai-oauth`: `{ xaiOauth }` — Hermes' `providers["xai-oauth"]` record
 *   (`tokens: {access_token, refresh_token, id_token?, expires_in?,
 *   token_type?}`, `discovery.token_endpoint?`) and optional `clientId`
 *   (default Hermes' public xAI client). Refreshed at the token endpoint, an
 *   `https` `x.ai` host only.
 * - `nous`: `{ nousAuth }` — Hermes' `providers.nous` record (`access_token`,
 *   `refresh_token`, `expires_at` ISO, `client_id`, `portal_base_url`,
 *   `inference_base_url`, `scope`, `token_type`, …). Refreshed at
 *   `{portal}/api/oauth/token` (an `https` `nousresearch.com` host only),
 *   the refresh token in the `x-nous-refresh-token` header.
 * - `antigravity`: `{ antigravityToken }` — Antigravity's `acp_token.json`
 *   (`client_id`, `client_secret`, `refresh_token`, `token_uri`, `scopes`,
 *   `project_id`). Refreshed at Google's token endpoint; Google refresh
 *   tokens don't rotate.
 * - `muse`: `{ museAuth }` — Muse's whole `auth.json` document; its
 *   `providers.meta` login (`access_token`, `expires_at` Unix seconds) is
 *   used until it expires (Muse refreshes nothing; it mints model keys from
 *   the access token).
 * - `github`: not uploadable; use the device flow. The resulting user token
 *   stays in the vault: `github` grants are App installation tokens.
 */
export type VaultMaterial =
  | { readonly authJson: Record<string, unknown> }
  | { readonly claudeAiOauth: Record<string, unknown> }
  | { readonly oauthToken: string; readonly expiresAt?: number }
  | { readonly key: string; readonly expiresAt?: number }
  | { readonly copilotToken: string; readonly login: string }
  | { readonly devinCredentials: Record<string, unknown> }
  | { readonly grokAuth: Record<string, unknown>; readonly scope: string }
  | { readonly xaiOauth: Record<string, unknown>; readonly clientId?: string }
  | { readonly nousAuth: Record<string, unknown> }
  | { readonly antigravityToken: Record<string, unknown> }
  | { readonly museAuth: Record<string, unknown> };

export interface PutCredentialRequest {
  readonly material: VaultMaterial;
  readonly authorizedDevices: readonly string[];
  /** The login's identity, when the uploader knows it (Claude: its profile). */
  readonly profile?: VaultAccountProfile;
}

export interface EnrollDeviceRequest {
  readonly deviceId: string;
  readonly kind: "laptop" | "cloud";
  /** Raw 32-byte Ed25519 public key, base64url (no padding). */
  readonly publicKey: string;
  /**
   * Cloud session devices: the account's logical Cloud device id. A grant is
   * authorized when the credential's `authorizedDevices` lists the device
   * itself OR its parent, so connecting a provider "for Cloud" once covers
   * every session sandbox. Revoking the parent revokes all its children.
   */
  readonly parentId?: string;
}

/**
 * `sig` = base64url Ed25519 signature by the device key over the UTF-8 bytes of
 * `zeron-vault-grant\n{userId}\n{deviceId}\n{provider}\n{ts}`. `ts` is Unix ms,
 * accepted within ±120 s and strictly greater than the device's last accepted
 * grant `ts` (replay fence).
 */
export interface GrantRequest {
  readonly provider: VaultProviderId;
  readonly deviceId: string;
  readonly ts: number;
  readonly sig: string;
  /**
   * `github` only, and required there: the `owner/name` the device works on.
   * Selects the login and App installation to use (the session's own, recorded
   * at its first grant); the token reaches that one repository.
   */
  readonly repo?: string;
  /**
   * `github`, runners only, set by the main Worker and never from a client's
   * body: the parent session's device, whose login and installation a delegate
   * takes at its first grant.
   */
  readonly inheritFrom?: string;
}

export interface Grant {
  readonly provider: VaultProviderId;
  /** Provider access token (or the API key for `*-key` providers). For
   * `github`: an App installation token (one hour, acts as the App). */
  readonly accessToken: string;
  /** Unix ms. The consumer may cache the grant until then (offline fallback). */
  readonly expiresAt: number;
  /** The account it comes from (the active one): what a consumer keys what
   * it learns with it by, such as the models a machine discovers with it. */
  readonly slot?: string;
  /** Codex: ChatGPT account id. */
  readonly accountId?: string;
  /** Claude: OAuth scopes and plan of the stored login (the CLI's credential
   * file carries both). */
  readonly scopes?: readonly string[];
  readonly subscriptionType?: string;
  /** Codex: id_token (the CLI's auth.json requires it). */
  readonly idToken?: string;
  /** GitHub login / account email. For `github`, the session's login: the
   * author of its commits. */
  readonly account?: string;
  /** `github`: the App installation the token came from. */
  readonly installationId?: number;
  /**
   * What the consumer needs beside the token to write the agent's own
   * credential file, never a refresh token: `devin`, the rest of
   * `credentials.toml` (its server urls); `grok`, the `auth.json` entry's
   * identity and issuer fields plus `scope` (its map key); `xai-oauth`,
   * `token_endpoint`, `token_type`, `id_token`; `nous`, the record's
   * non-token fields (`portal_base_url`, `inference_base_url`, `client_id`,
   * `scope`, `token_type`); `antigravity`, `client_id`, `project_id`,
   * `scopes` (space-separated); `muse`, `document` — the `auth.json` as
   * uploaded, JSON-encoded, every `refresh_token` emptied.
   */
  readonly settings?: Readonly<Record<string, string>>;
  readonly generation: number;
}

/**
 * Several providers' grants for one device in one round trip. `sig` signs
 * the grant message with `providers.join(",")` as its provider. The list is
 * sorted and distinct, and never holds `github` (its grants name a
 * repository). One signature spends one `ts`.
 */
export interface GrantManyRequest {
  readonly providers: readonly VaultProviderId[];
  readonly deviceId: string;
  readonly ts: number;
  readonly sig: string;
}

export interface GrantManyReply {
  readonly grants: readonly Grant[];
  /** The providers granted nothing, and why (`not_found`, `needs_reconnect`, …). */
  readonly missing: readonly { readonly provider: VaultProviderId; readonly error: VaultErrorCode; readonly message: string }[];
}

export interface GithubDeviceStart {
  readonly flowId: string;
  readonly userCode: string;
  readonly verificationUri: string;
  readonly intervalSecs: number;
  readonly expiresAt: number;
}

export interface GithubDevicePoll {
  readonly state: "pending" | "connected" | "failed";
  readonly account?: string;
  /** Connected, but as a login that was already connected: nothing was added.
   * GitHub approves as whoever the browser is signed in as. */
  readonly alreadyConnected?: boolean;
  readonly error?: string;
}

/** The `VaultApi` WorkerEntrypoint's methods, as seen through the binding. */
export interface VaultRpc {
  status(caller: VaultCaller): Promise<VaultResult<VaultStatusView>>;
  putCredential(
    caller: VaultCaller,
    provider: VaultProviderId,
    request: PutCredentialRequest
  ): Promise<VaultResult<VaultStatusView>>;
  authorize(
    caller: VaultCaller,
    provider: VaultProviderId,
    authorizedDevices: readonly string[]
  ): Promise<VaultResult<VaultStatusView>>;
  disconnect(caller: VaultCaller, provider: VaultProviderId): Promise<VaultResult<VaultStatusView>>;
  enrollDevice(caller: VaultCaller, request: EnrollDeviceRequest): Promise<VaultResult<VaultStatusView>>;
  revokeDevice(caller: VaultCaller, deviceId: string): Promise<VaultResult<VaultStatusView>>;
  /** Per-user kill switch. */
  setDisabled(caller: VaultCaller, disabled: boolean): Promise<VaultResult<VaultStatusView>>;
  grant(caller: VaultCaller, request: GrantRequest): Promise<VaultResult<Grant>>;
  /** Several grants at once (`GrantManyRequest`). A refusal of the device
   * (signature, revoked, disabled) fails the whole call; a provider with
   * nothing to grant is listed in `missing`. */
  grantMany(caller: VaultCaller, request: GrantManyRequest): Promise<VaultResult<GrantManyReply>>;
  githubDeviceStart(
    caller: VaultCaller,
    authorizedDevices: readonly string[]
  ): Promise<VaultResult<GithubDeviceStart>>;
  githubDevicePoll(caller: VaultCaller, flowId: string): Promise<VaultResult<GithubDevicePoll>>;
  /**
   * Repositories the user's GitHub logins can see (GitHub App user tokens:
   * every installation's repositories), merged, newest push first, filtered by
   * a case-insensitive substring of `fullName`. The vault calls GitHub itself;
   * the tokens never leave it. Users only (runners use their own grant).
   * `not_found` when GitHub isn't connected. One login failing leaves the
   * others' repositories; it fails only when no login answered.
   */
  githubRepos(caller: VaultCaller, query?: string): Promise<VaultResult<readonly GithubRepoView[]>>;
  /** `githubRepos` with the installations and any login that didn't answer. */
  githubListing(caller: VaultCaller, query?: string): Promise<VaultResult<GithubListingView>>;
  /**
   * Branch names of `repo` (`owner/name`) through the first login that can see
   * it, for picking the branch a new Cloud session starts from. Users only.
   * `not_found` when GitHub isn't connected or no login can see it.
   */
  githubBranches(caller: VaultCaller, repo: string): Promise<VaultResult<readonly string[]>>;
  /** Every stored login of `provider` (users only). */
  accounts(caller: VaultCaller, provider: VaultProviderId): Promise<VaultResult<readonly VaultAccountView[]>>;
  /** Make `slot` the login grants come from; `null` = none (the engine then
   * falls through to the provider's API key). Users only. */
  activateAccount(
    caller: VaultCaller,
    provider: VaultProviderId,
    slot: string | null
  ): Promise<VaultResult<readonly VaultAccountView[]>>;
  /** Delete one stored login. Forgetting the active one leaves none active.
   * Users only. */
  forgetAccount(
    caller: VaultCaller,
    provider: VaultProviderId,
    slot: string
  ): Promise<VaultResult<readonly VaultAccountView[]>>;
  /** Plan usage of one Claude, Codex or Antigravity login, read by the vault (users only;
   * `not_found` for providers without a usage view). */
  accountUsage(caller: VaultCaller, provider: VaultProviderId, slot: string): Promise<VaultResult<VaultUsageView>>;
}
