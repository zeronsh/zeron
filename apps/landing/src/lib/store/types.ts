// The edge's store contract, as the site reads it
// (docs/plans/2026-11-17-plugin-store.md, "Contract (edge HTTP)", and since
// amendments 44 and 45, docs/plans/2026-11-19-store-shapes-44-45.md, as the
// edge built it at b467e6299:
// publishing is immediate, review verifies; keys belong to the account). The names
// the plan fixes are kept exactly; the fields the lanes fill in are marked
// optional where an older edge may not send them yet. Times are ms since the
// epoch.

import type { Kind } from "./kind.ts";

/** A failure's stable word. Anything else the edge says reads as `unknown`. */
export type ErrorWord =
  | "site_id"
  | "not_found"
  | "signed_out"
  | "forbidden"
  | "bad_request"
  | "bad_csrf"
  | "rate_limited"
  | "suspended"
  | "needs_username"
  | "id_taken"
  | "id_reserved"
  | "not_semver"
  | "version_exists"
  | "not_newer"
  | "bad_package"
  | "bad_signature"
  | "key_not_allowed"
  | "missing_readme"
  | "missing_listing"
  | "too_large"
  | "conflict"
  | "invalid_state"
  | "invalid_scope"
  | "not_owner"
  | "not_verified"
  | "bad_listing"
  | "step_up_required"
  | "invalid_name"
  | "invalid_expiry"
  | "too_many_tokens"
  | "unknown_token"
  | "session_required"
  | "unauthenticated"
  | "bad_cursor"
  | "bad_path"
  | "unknown";

export interface EdgeError {
  error: ErrorWord;
  message?: string;
  /** The plugin ids a refusal names (a token's scope: `invalid_scope`, `not_owner`). */
  ids?: string[];
}

/** `verified`: newest verification first, only plugins with a verified listed version (contract ee380cca3). */
export type Sort = "relevance" | "popular" | "updated" | "new" | "verified";

export interface Badge {
  id: string;
  name: string;
  icon?: string | null;
  description?: string;
}

/**
 * Who publishes it (amendments 20, 33): every listing is by an account with
 * a username. `id` is the publisher's stable, opaque id: what anything kept
 * per author is keyed by; never shown. The username is what people read.
 */
export interface Author {
  id: string;
  /** (Null only for an account since deleted.) */
  username: string | null;
  badges: Badge[];
}

export interface PluginCard {
  id: string;
  /** What it is (an edge before templates leaves it out: a plugin; lib/store/kind.ts kindOfItem). A template's `icon` is null, its `cover` always "preview.png", its `categories` []. */
  kind?: Kind;
  name: string;
  summary: string;
  /** The icon's path in the package (an SVG), or null (amendment 35). */
  icon: string | null;
  /**
   * Its first picture's path in the package, of `version`, or null when that
   * has none (asked of the edge for the store's Featured; an edge before it
   * leaves it out: no cover, the tile on its hue).
   */
  cover?: string | null;
  author: Author;
  categories: string[];
  keywords: string[];
  /** The newest listed version, a release before a pre-release, verified or not (null while it has none). */
  version: string | null;
  /** Whether `version` is verified (amendment 45). */
  verified: boolean;
  /** The newest verified listed version, or null. */
  verified_version: string | null;
  /** When `verified_version` was verified, or null (contract ee380cca3; an edge before 9a787d3c2 leaves it out). */
  verified_at?: number | null;
  updated_at: number;
  /**
   * When it was first listed: what Newest sorts by (amendment 39). Set once,
   * never moved by a later version. Always there on a public card; null only
   * on an owner's or admin's card of a plugin with no listed version.
   */
  listed_at: number | null;
  installs: number;
  featured: boolean;
}

/**
 * What a plugin asks for, as the app's consent sheet is given it
 * (apps/zgx/src/plugins/consent.rs): the permissions it names, and what
 * each names exactly, with the host's ratings of programs and hosts.
 */
export interface Permissions {
  asks: string[];
  ui?: { change?: string[]; replace?: string[] };
  commands?: string[];
  /** Of `commands`, those that can do anything you can (not contained). */
  commands_anything?: string[];
  /** Of `commands`, shells and interpreters: full control of the computer. */
  commands_full_control?: string[];
  network_hosts?: string[];
  /** Of `network_hosts`, those on this computer or your network. */
  network_local?: string[];
  /** Of `network_hosts`, names that may resolve to this computer or your network. */
  network_named?: string[];
  tools?: { name: string; description?: string }[];
}

/**
 * A version's state (amendment 45): listed (anyone can see and install it),
 * yanked (its author stopped new installs), taken_down (staff: harmful, and
 * turned off on devices), removed (staff: not harmful, not turned off). The
 * public sees listed and yanked only.
 */
export type VersionStatus = "listed" | "yanked" | "taken_down" | "removed";

/** A reviewer's decision on a version, as its author and staff see it (the public never sees "declined"). */
export interface Review {
  decided: boolean;
  decision: "verified" | "declined" | null;
  /** Why, as a stable word (declined). */
  reason?: string;
  /** What the reviewer wrote for the author. */
  message?: string;
  decided_at?: number;
}

export type CheckLevel = "ok" | "warn" | "fail";

export interface Check {
  id: string;
  level: CheckLevel;
  message: string;
}

export interface VersionInfo {
  version: string;
  status: VersionStatus;
  /** When it was published (and listed: at once). */
  published_at: number;
  /** Whether a reviewer verified it, and when. */
  verified: boolean;
  verified_at: number | null;
  prerelease?: boolean;
  /** Taken down and turned off on the devices that have it. */
  blocked?: boolean;
  /** How it was published: from a device, or by a deploy token. */
  via?: "device" | "deploy_token";
  /** This version's section of CHANGELOG.md (markdown), or null. */
  notes: string | null;
  engines: { zeron?: string } | null;
  yanked: boolean;
  /** The device's key that signed it (amendment 1); null for a version a deploy token sent, unsigned, the store's record vouching for it (a315eef5a). */
  key?: string | null;
  hash?: string;
  sha256?: string;
  bytes?: number;
  /** The owner's and an admin's view. */
  review?: Review;
  checks?: Check[];
  /** The deploy token that published it (owner and staff; null when a device did): its id, short id and name. */
  deploy_token?: { id: string; short: string; name: string } | null;
  /** Why staff took it down or removed it, and when (owner and staff). */
  removal?: { at: number; reason: string | null };
  /**
   * Who stopped its new installs (owner and staff; in public too after e3daa69b4, edge dc45c3606): its author, or the store,
   * when a newer version was published while it was unverified (amendment 45:
   * "superseded"; its author can't allow installs again, verifying it does).
   */
  yanked_by?: "author" | "superseded" | null;
  /** When it was superseded and by which version, or null (owner and staff). */
  superseded?: { at: number; by: string } | null;
  /** The version that superseded it, in public (edge dc45c3606 on). */
  superseded_by?: string | null;
}

export interface PackageInfo {
  bytes: number;
  sha256: string;
  hash: string;
  /** The public key that signed it (b64url); null for a version a deploy token sent. */
  key?: string | null;
}

export interface PluginDetail extends PluginCard {
  /** README.md, markdown. */
  readme: string;
  /** Pictures' paths in the package (amendment 35). */
  screenshots: string[];
  /** (A template asks for nothing and has none of these: it runs as Zeron itself.) */
  permissions?: Permissions;
  engines: { zeron?: string } | null;
  homepage: string | null;
  repository: string | null;
  license: string | null;
  /** The first page of its versions (5), newest first; `versions_next_cursor` for the rest. */
  versions: VersionInfo[];
  versions_next_cursor?: string;
  /** `version`'s package. */
  package: PackageInfo;
}

/** A page of a plugin's versions: listed and yanked, newest first. */
export interface VersionsPage {
  versions: VersionInfo[];
  total: number;
  next_cursor?: string;
  prev_cursor?: string;
}

/** The store's totals. */
export interface Stats {
  plugins: number;
  verified: number;
  /** Listed plugins first listed in the last seven days (ac4fc5d3f; an edge before b2094a93e leaves it out). */
  new_7d?: number;
}

export interface CatalogPage {
  items: PluginCard[];
  /**
   * How many plugins the query matches, every page of them (amendment 38;
   * the public list's count, whoever asks). Cached with its page, so two
   * pages of one list may differ for a minute: each page says its own.
   */
  total: number;
  next_cursor?: string;
  /** The cursor back to the page before this one, when there is one. */
  prev_cursor?: string;
}

export interface Category {
  id: string;
  name: string;
  /** How many listed plugins it holds. */
  count?: number;
  /** With `?top=n`: its n most installed plugins, as `?category=<id>&sort=popular` lists them (ac4fc5d3f). */
  top?: PluginCard[];
}

/** A keyword and how many listed plugins carry it (`/store/v1/keywords`, ac4fc5d3f): the list's total for `?keyword=`. */
export interface KeywordCount {
  keyword: string;
  count: number;
}

// ── The signed-in web session (/auth/web/session) ───────────────────────

export interface WebSession {
  user: { name?: string; email?: string; avatar?: string };
  username: string | null;
  roles: string[];
  /** The token every form posts back (checked by the edge with Origin). */
  csrf: string;
  expires_at: number;
  fresh_sign_in?: boolean;
}

// ── The author's own ────────────────────────────────────────────────────

export type Listing = "listed" | "unlisted" | "suspended";

export interface MyPlugin extends PluginCard {
  listing: Listing;
  /** Every version, newest first; each with the account that published it (`publisher_id`, the owner's view). */
  versions: (VersionInfo & { publisher_id?: string | null })[];
}

/** `GET /store/v1/me/plugins` (the contract after e3daa69b4, 227392be7): the account's publisher id, and its plugins. */
export interface MyPlugins {
  publisher_id?: string;
  plugins: MyPlugin[];
}

/** `GET /store/v1/me/templates`: the same, of the account's templates. */
export interface MyTemplates {
  publisher_id?: string;
  templates: MyPlugin[];
}

/**
 * What a deploy token may publish: all the account's, of which kinds
 * (`all_kinds`; an edge before templates leaves it out: plugins), or the
 * ones named, of either kind (sorted; no kind beside an id).
 */
export type TokenScope = { all: true; all_kinds?: Kind[] } | { plugins: string[] };

/**
 * A deploy token, the account's (contract a315eef5a): named only by its short id (the 8 characters after
 * `zdt_`) beside its name. The token itself comes once, in the answer that made it (NewToken), never again.
 */
export interface DeployToken {
  id: string;
  short: string;
  name: string;
  scope: TokenScope;
  created_at: number;
  last_used_at: number | null;
  /** What it last published a version of, of either kind. */
  last_used_plugin: string | null;
  expires_at: number | null;
  /** Null while it's live. */
  revoked_at: number | null;
}

/** The answer that makes a token: the token itself, shown once and kept nowhere. */
export interface NewToken {
  token: string;
  id: string;
  short: string;
  name: string;
  scope: TokenScope;
  created_at: number;
  expires_at: number | null;
}

export const REPORT_REASONS = ["malware", "misleading", "broken", "copyright", "other"] as const;
export type ReportReason = (typeof REPORT_REASONS)[number];

// ── Admin ───────────────────────────────────────────────────────────────

/** The staff queue's view (amendment 45): versions no reviewer has decided, or those verified, or declined, or all. */
export type QueueState = "undecided" | "verified" | "declined" | "all" | "taken_down";

/**
 * One row of the queue: oldest first, versions with open reports first; in
 * the undecided view, at most two a plugin (its newest undecided release,
 * and its newest undecided pre-release when that is newer).
 */
export interface QueueItem {
  plugin: { id: string; name: string };
  version: string;
  review: Review;
  /** Open reports on the plugin. */
  reports: number;
  /** How many earlier undecided versions it superseded (each still opens from the plugin's staff page). */
  earlier_undecided: number;
  published_at?: number;
  /** This version's icon, its path in the package, as a card's (amendment 40). */
  icon: string | null;
  status: VersionStatus;
  prerelease: boolean;
  submitted_at: number;
  publisher: { id: string; username: string | null } | null;
  /** The newest verified other version, which it is compared with (none when there is none). */
  base?: string;
  /** Permissions it asks for that the base didn't. */
  new_asks: string[];
  /**
   * Anything inside an ask it already had differs from the base (a host, a
   * program, a place), a tool says or takes something
   * else, or its MCP server, service or a granting setting changed
   * (amendments 40, 41); false for a first version.
   */
  asks_changed: boolean;
  checks: { ok: number; warn: number; fail: number };
  /** The plugin's installs (edge 566c2a5ba; absent from an older edge). */
  installs?: number;
}

export interface QueuePage {
  items: QueueItem[];
  /** How many the queue holds, every page of it (amendment 38). */
  total: number;
  next_cursor?: string;
  prev_cursor?: string;
}

/** When an account was made: ms since the epoch (null for one since deleted). */
export type Created = number | null;

export interface Publisher {
  id?: string;
  username: string | null;
  created_at: Created;
  /** Reviewers' decisions on this publisher's versions (never shown: Wing's ruling). */
  decisions: { verified: number; declined: number };
  /** When the account first listed a version, or null (edge 566c2a5ba; absent from an older edge). */
  first_published_at?: number | null;
  /** Its listed plugins, and how many of them have a verified version (edge 566c2a5ba). */
  plugins?: { total: number; verified: number };
  recent: { plugin_id: string; version: string; status: VersionStatus; verified: boolean; published_at?: number; reason?: string | null }[];
}

export interface PackageFile {
  path: string;
  bytes: number;
  sha256: string;
  /** Not text: never shown as text, and called out. */
  binary: boolean;
}

/** What changed in what it asks since the newest verified other version (the edge's consentDiff). */
export interface PermissionsDiff {
  added?: string[];
  removed?: string[];
  added_commands?: string[];
  removed_commands?: string[];
  network_added?: string[];
  network_removed?: string[];
  tools_added?: { name: string; description?: string }[];
  tools_removed?: string[];
  tools_changed?: { name: string; description_before?: string; description_after?: string; input_changed?: boolean }[];
  ui_replace_added?: string[];
  ui_change_added?: string[];
  /** The MCP server, before and after (each null when there was none), or null when unchanged (amendment 41; the engine's `digest_diff`). */
  mcp_changed: { before: McpRecord | null; after: McpRecord | null } | null;
  /** The service's path, before and after, or null when unchanged (amendment 41). */
  service_changed: { before: string | null; after: string | null } | null;
  /** A setting that grants (a permission or a folder) is new or different (amendment 41; the engine records no more). */
  settings_changed: boolean;
}

/** An MCP server as the engine's capability record holds it: env's names sorted, its values as sent. */
export interface McpRecord {
  command: string;
  args: string[];
  env: Record<string, string>;
}

export interface Submission {
  /** The plugin (for one never listed: this submission's words, `version` and `icon` null). */
  plugin: PluginCard;
  /** The version, with what it lists: its name, icon, README and more. */
  version: VersionInfo & { name?: string; summary?: string; icon?: string | null; readme?: string; screenshots?: string[] };
  publisher: Publisher;
  /** The signed manifest, as submitted. */
  manifest: Record<string, unknown>;
  permissions: Permissions;
  files: PackageFile[];
  /** The newest verified other version, which the diff is against, when there is one. */
  base?: { version: string; manifest: Record<string, unknown> } | null;
  /** When this version was superseded and by which, or null. */
  superseded?: { at: number; by: string } | null;
  /** The base's files. */
  base_files: PackageFile[];
  permissions_diff: PermissionsDiff | null;
  checks: Check[];
}

export type PluginStatus = "listed" | "unlisted" | "suspended" | "removed";

export interface AdminPlugin {
  plugin: PluginCard & { listed_version?: string | null; created_at?: number; suspend_reason?: string | null };
  status: Listing;
  featured: boolean;
  owner: { id?: string; user_id: string; username: string | null; created_at: Created };
  versions: VersionInfo[];
  /** As /admin/reports gives them. */
  reports: Report[];
}

export type ReportStatus = "open" | "dismissed" | "actioned";

export interface Report {
  id: number | string;
  plugin: { id: string; name: string | null };
  version?: string | null;
  reason: ReportReason;
  text: string;
  reporter: { id?: string; username: string | null } | null;
  created_at: number;
  status: ReportStatus;
  resolved_by?: { username: string | null } | null;
  resolved_at?: number | null;
  note?: string | null;
}

export interface ReportsPage {
  reports: Report[];
  /** How many reports in this state, every page of them (amendment 38). */
  total: number;
  next_cursor?: string;
  prev_cursor?: string;
}

/** An id held back from first-come claims: for one account, or for nobody. */
export interface Reservation {
  id: string;
  to: { username: string } | null;
  note: string | null;
  created_at: number;
}

export interface AuditEntry {
  seq: number;
  at: number;
  actor: { id?: string | null; username: string | null; user_id: string };
  /** `version.verify`, `version.decline`, `plugin.suspend`, `report.dismissed`, `reservation.set`, … */
  action: string;
  plugin_id?: string | null;
  version?: string | null;
  /** What else it recorded (the edge's JSON). */
  detail?: unknown;
}

export interface AuditPage {
  entries: AuditEntry[];
  /** How many entries (for one plugin, when asked), every page of them (amendment 38). */
  total: number;
  next_cursor?: string;
  prev_cursor?: string;
}
