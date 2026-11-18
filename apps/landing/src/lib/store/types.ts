// The edge's store contract, as the site reads it
// (docs/plans/2026-11-17-plugin-store.md, "Contract (edge HTTP)"). The names
// the plan fixes are kept exactly; the fields the lanes fill in are marked
// optional where an older edge may not send them yet. Times are ms since the
// epoch.

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
  | "unauthenticated"
  | "bad_cursor"
  | "bad_path"
  | "unknown";

export interface EdgeError {
  error: ErrorWord;
  message?: string;
}

export type Sort = "relevance" | "popular" | "updated" | "new";

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
  name: string;
  summary: string;
  /** The icon's path in the package (an SVG), or null (amendment 35). */
  icon: string | null;
  author: Author;
  categories: string[];
  keywords: string[];
  /** The latest approved version (null while it has none). */
  version: string | null;
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

export type VersionStatus = "pending" | "approved" | "rejected" | "withdrawn" | "yanked" | "removed";

export interface Review {
  state: VersionStatus;
  /** Why, as a stable word (rejected or removed). */
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
  /** When it was submitted. */
  submitted_at: number;
  /** When it was approved (or later decided); null before. */
  published_at: number | null;
  prerelease?: boolean;
  /** Taken down and turned off on the devices that have it. */
  blocked?: boolean;
  /** How it was published: from a device, or by a deploy key. */
  via?: string;
  /** This version's section of CHANGELOG.md (markdown), or null. */
  notes: string | null;
  engines: { zeron?: string } | null;
  yanked: boolean;
  /** What was signed (amendment 1). */
  key?: string;
  hash?: string;
  sha256?: string;
  bytes?: number;
  /** The owner's and an admin's view. */
  review?: Review;
  checks?: Check[];
}

export interface PackageInfo {
  bytes: number;
  sha256: string;
  hash: string;
  /** The public key that signed it (b64url). */
  key?: string;
}

export interface PluginDetail extends PluginCard {
  /** README.md, markdown. */
  readme: string;
  /** Pictures' paths in the package (amendment 35). */
  screenshots: string[];
  permissions: Permissions;
  engines: { zeron?: string } | null;
  homepage: string | null;
  repository: string | null;
  license: string | null;
  /** The latest few. */
  versions: VersionInfo[];
  package: PackageInfo;
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
  /** Every version, newest first. */
  versions: VersionInfo[];
}

export interface DeployKey {
  /** The public key (base64url). */
  key: string;
  name: string;
  created_at: number;
  last_used_at?: number | null;
  expires_at?: number | null;
  revoked_at?: number | null;
}

export const REPORT_REASONS = ["malware", "misleading", "broken", "copyright", "other"] as const;
export type ReportReason = (typeof REPORT_REASONS)[number];

// ── Admin ───────────────────────────────────────────────────────────────

export interface QueueItem {
  plugin: { id: string; name: string };
  version: string;
  /** This version's icon, its path in the package, as a card's (amendment 40). */
  icon: string | null;
  status: VersionStatus;
  prerelease: boolean;
  submitted_at: number;
  publisher: { id: string; username: string | null } | null;
  /** The version it would replace (none for a first listing). */
  base?: string;
  /** Permissions it asks for that the last approved version didn't. */
  new_asks: string[];
  /**
   * Anything inside an ask it already had differs from the last approved
   * version (a host, a program, a place), a tool says or takes something
   * else, or its MCP server, service or a granting setting changed
   * (amendments 40, 41); false for a first version.
   */
  asks_changed: boolean;
  checks: { ok: number; warn: number; fail: number };
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
  decisions: { approved: number; rejected: number };
  recent: { plugin_id: string; version: string; status: VersionStatus; submitted_at?: number; reason?: string | null }[];
}

export interface PackageFile {
  path: string;
  bytes: number;
  sha256: string;
  /** Not text: never shown as text, and called out. */
  binary: boolean;
}

/** What changed in what it asks since the last approved version (the edge's consentDiff). */
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
  /** The last approved version, which the diff is against, when there is one. */
  base?: { version: string; manifest: Record<string, unknown> } | null;
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
  /** `version.approve`, `plugin.suspend`, `report.dismissed`, `reservation.set`, … */
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
