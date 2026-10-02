import type { VaultRpc } from "../vault/src/api";
import type { CloudAccount, LifecycleParams, ProvisionParams } from "./cloud/cloud-account";
import type { CloudIndex } from "./cloud/cloud-index";

export interface Env {
  SESSION_ROOMS: DurableObjectNamespace;
  DEVICE_ROOMS: DurableObjectNamespace;
  PREVIEW_ROOMS: DurableObjectNamespace;
  /** Per-user workspace registries (`reg1/{orgId}/{userId}`) — the row-table
   * replacement for the Loro workspace doc (docs/registry-sync.md). */
  REGISTRY_ROOMS: DurableObjectNamespace;
  /** chat2 session rooms (`chat2/{chatId}`) — dumb authenticated log relays
   * replacing SessionRoom's loro-aware s2 rooms (docs/chat2-sync.md). */
  CHAT_ROOMS: DurableObjectNamespace;
  BLOBS: R2Bucket;
  /** Release artifacts (headless tarballs, dmgs, latest.txt) served at
   * /releases/* for the curl-install flow. */
  RELEASES: R2Bucket;
  WORKOS_CLIENT_ID: string;
  /** "workos" (verify AuthKit JWTs) or "dev" (bearer == userId, never prod). */
  AUTH_MODE: string;
  /** Optional overrides for the WorkOS trust anchor. */
  WORKOS_ISSUER?: string;
  WORKOS_JWKS_URL?: string;
  /** WorkOS secret API key (wrangler secret) — powers the absorbed /auth/*
   * routes (code exchange, refresh, orgs). Unset ⇒ those routes answer 501,
   * matching the old apps/server dev-mode behavior. */
  WORKOS_API_KEY?: string;
  /** APNs auth key (contents of AuthKey_XXXX.p8, wrangler secret) and its
   * key id. Unset ⇒ session notifications are decided and logged, not sent. */
  APNS_KEY_P8?: string;
  APNS_KEY_ID?: string;
  /** Apple team id and the app's bundle id (defaults: the Zeron iOS app). */
  APNS_TEAM_ID?: string;
  APNS_TOPIC?: string;

  // ── Cloud device (docs/design/cloud-device.md). Every binding is optional:
  //    deployments without them (the dev/test wrangler configs) answer
  //    `available: false` / 503 on the Cloud routes and nothing else changes.
  /** Per-user Cloud account: projects, sessions, metering (`cloud1/{orgId}/{userId}`). */
  CLOUD_ACCOUNTS?: DurableObjectNamespace<CloudAccount>;
  /** The billing walk list (`index1`). */
  CLOUD_INDEX?: DurableObjectNamespace<CloudIndex>;
  CLOUD_PROVISION?: Workflow<ProvisionParams>;
  CLOUD_WAKE?: Workflow<LifecycleParams>;
  CLOUD_SLEEP?: Workflow<LifecycleParams>;
  CLOUD_DELETE?: Workflow<LifecycleParams>;
  /** zeron-vault's `VaultApi` entrypoint (service binding; no HTTP). */
  VAULT?: VaultRpc;
  /** Provider for NEW Cloud sandboxes (cloud/providers): "boat" (default)
   * or "fake" (in-memory, local dev only). */
  SANDBOX_PROVIDER?: string;
  /** Sandbox auto-stop, seconds; unset = none (idle sleep is ours). Needed
   * when the provider account forces auto-stop (Boat trial: ≤7200). */
  CLOUD_TTL_SECONDS?: string;
  /** Boat API key (secret). Needs sandbox.create/read/update/stop/resume/
   * delete + exec + usage read. Unset ⇒ the Boat provider is unavailable. */
  BOAT_API_KEY?: string;
  /** Default `https://boat.dev/api/v1`. */
  BOAT_API_BASE?: string;
  /** Idle minutes before an unused Cloud session sleeps (default 20). */
  CLOUD_IDLE_MINUTES?: string;
  /** Origin session engines talk to; default = the creating request's
   * origin. For dev tunnels. */
  CLOUD_EDGE_URL?: string;
  /** Most sessions awake at once per user (default 5). */
  CLOUD_MAX_AWAKE?: string;
  /** Named provider snapshot new session sandboxes start from (engine
   * pre-installed, no identity). Unset ⇒ a blank machine + install. */
  CLOUD_TEMPLATE?: string;
  /** Where session machines check their repository out (default
   * `/home/user`); a local stack whose machines are processes on one
   * computer points it at a scratch folder (scripts/cloud-dev.sh). */
  CLOUD_PROJECTS_ROOT?: string;
  /** Machine size for session sandboxes: small · default · large. */
  CLOUD_MACHINE?: string;
  /** ES256 JWK (JSON) pair for runner JWTs (secrets). */
  RUNNER_JWT_PRIVATE_KEY?: string;
  RUNNER_JWT_PUBLIC_KEY?: string;
  /** Operator bearer for /admin/cloud/usage (secret). Unset ⇒ route 404s. */
  ADMIN_TOKEN?: string;
}

/** APNs settings, when push is set up for this deployment. */
export const apnsConfig = (env: Env) =>
  env.APNS_KEY_P8 && env.APNS_KEY_ID
    ? {
        keyP8: env.APNS_KEY_P8,
        keyId: env.APNS_KEY_ID,
        teamId: env.APNS_TEAM_ID ?? "5XY3M483YQ",
        topic: env.APNS_TOPIC ?? "sh.zeron.ios"
      }
    : undefined;

/** Header the Worker stamps on requests it forwards into DOs after verifying
 * the caller's JWT. DOs trust it blindly — they are only reachable through
 * the Worker (design §2: "DO never sees an unauthenticated frame"). */
export const AUTH_USER_HEADER = "x-zeron-auth-user";

/** Header the Worker stamps on requests forwarded into workspace-doc rooms
 * (`ws/{orgId}`). Membership (JWT org claim == orgId) is enforced at the
 * Worker; the SessionRoom DO sees this and skips its per-chat
 * claim-on-first-join ownership discipline for the room. */
export const ROOM_KIND_HEADER = "x-zeron-room-kind";

/** Header the Worker stamps on every DO forward made with a RUNNER bearer
 * (the Cloud device): the verified device id from the runner JWT's `dev`
 * claim. Always stripped from inbound requests first, so only the Worker can
 * assert it. RegistryRoom restricts such callers to rows their device owns
 * (registry-runner.ts). */
export const RUNNER_DEVICE_HEADER = "x-zeron-runner-device";

/** Companion of RUNNER_DEVICE_HEADER: the runner's account (logical Cloud
 * device, JWT `cld`). A session runner may read its account's project
 * spaces. Same strip-then-stamp discipline. */
export const RUNNER_ACCOUNT_HEADER = "x-zeron-runner-account";
