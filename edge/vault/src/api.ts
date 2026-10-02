/**
 * The vault's RPC surface (docs/design/cloud-device.md). The vault Worker has
 * no routes: the main edge Worker reaches it only through the `VAULT` service
 * binding (`entrypoint: "VaultApi"`) and passes the identity it already
 * verified. This file is types only, so the main Worker can `import type`
 * it without pulling vault code into its bundle.
 */

export type VaultProviderId = "codex" | "claude" | "github" | "anthropic-key" | "openai-key";

export const VAULT_PROVIDERS: readonly VaultProviderId[] = [
  "codex",
  "claude",
  "github",
  "anthropic-key",
  "openai-key"
];

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

export interface VaultConnectionView {
  readonly provider: VaultProviderId;
  readonly status: "connected" | "needsReconnect";
  readonly authorizedDevices: readonly string[];
  /** Email / GitHub login / `…abcd` key suffix. Never secret material. */
  readonly account?: string;
  readonly updatedAt: number;
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
 * - `anthropic-key` / `openai-key`: `{ key }`.
 * - `github`: not uploadable; use the device flow. The resulting user token
 *   stays in the vault: `github` grants are App installation tokens.
 */
export type VaultMaterial =
  | { readonly authJson: Record<string, unknown> }
  | { readonly claudeAiOauth: Record<string, unknown> }
  | { readonly oauthToken: string; readonly expiresAt?: number }
  | { readonly key: string };

export interface PutCredentialRequest {
  readonly material: VaultMaterial;
  readonly authorizedDevices: readonly string[];
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
   * Selects which of the user's App installations to mint for (the one on
   * `owner`); the token then reaches every repository of that installation.
   */
  readonly repo?: string;
}

export interface Grant {
  readonly provider: VaultProviderId;
  /** Provider access token (or the API key for `*-key` providers). For
   * `github`: an App installation token (one hour, acts as the App). */
  readonly accessToken: string;
  /** Unix ms. The consumer may cache the grant until then (offline fallback). */
  readonly expiresAt: number;
  /** Codex: ChatGPT account id. */
  readonly accountId?: string;
  /** Claude: OAuth scopes and plan of the stored login (the CLI's credential
   * file carries both). */
  readonly scopes?: readonly string[];
  readonly subscriptionType?: string;
  /** Codex: id_token (the CLI's auth.json requires it). */
  readonly idToken?: string;
  /** GitHub login / account email, for display. */
  readonly account?: string;
  readonly generation: number;
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
  githubDeviceStart(
    caller: VaultCaller,
    authorizedDevices: readonly string[]
  ): Promise<VaultResult<GithubDeviceStart>>;
  githubDevicePoll(caller: VaultCaller, flowId: string): Promise<VaultResult<GithubDevicePoll>>;
  /**
   * Repositories the user's GitHub connection can see (GitHub App user token:
   * every installation's repositories), newest push first, filtered by a
   * case-insensitive substring of `fullName`. The vault calls GitHub itself;
   * the token never leaves it. Users only (runners use their own grant).
   * `not_found` when GitHub isn't connected.
   */
  githubRepos(caller: VaultCaller, query?: string): Promise<VaultResult<readonly GithubRepoView[]>>;
  /**
   * Branch names of `repo` (`owner/name`) through the user's GitHub
   * connection, for picking the branch a new Cloud session starts from.
   * Users only. `not_found` when GitHub isn't connected or can't see it.
   */
  githubBranches(caller: VaultCaller, repo: string): Promise<VaultResult<readonly string[]>>;
}
