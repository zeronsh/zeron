// The edge's store contract, as the site reads it
// (docs/plans/2026-11-17-plugin-store.md, "Contract (edge HTTP)"). The names
// the plan fixes are kept exactly; the fields the lanes fill in are marked
// optional where an older edge may not send them yet. Times are ms since the
// epoch.

/** A failure's stable word. Anything else the edge says reads as `unknown`. */
export type ErrorWord =
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
  | "unknown";

export interface EdgeError {
  error: ErrorWord;
  message?: string;
}

export type Sort = "relevance" | "popular" | "updated" | "new";

export interface Badge {
  id: string;
  name: string;
  description?: string;
}

export interface Author {
  username: string;
  /** The account's key signed it and the account holds that username. */
  verified: boolean;
  badges: Badge[];
}

export interface PluginCard {
  id: string;
  name: string;
  summary: string;
  /** The icon's path in the package (an SVG), or null. */
  icon: string | null;
  author: Author;
  categories: string[];
  keywords: string[];
  /** The latest approved version. */
  version: string;
  updated_at: number;
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
  /** Why, as a stable word (rejected or removed). */
  reason?: string;
  /** What the reviewer wrote for the author. */
  message?: string;
  decided_at?: number;
  /** Removed and blocked: devices turn it off at their next check. */
  blocked?: boolean;
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
  published_at: number;
  /** This version's section of CHANGELOG.md (markdown), or null. */
  notes: string | null;
  engines: { zeron?: string } | null;
  yanked: boolean;
  /** The owner's and an admin's view. */
  review?: Review;
  checks?: Check[];
}

export interface PackageInfo {
  bytes: number;
  sha256: string;
  hash: string;
}

export interface PluginDetail extends PluginCard {
  /** README.md, markdown. */
  readme: string;
  /** Pictures' paths in the package. */
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
  user: { id: string; name?: string; avatar?: string };
  username: string | null;
  roles: string[];
  /** The token every form posts back (checked by the edge with Origin). */
  csrf: string;
  expires_at?: number;
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
  id: string;
  name: string;
  version: string;
  status: VersionStatus;
  submitted_at: number;
  publisher: { username: string | null; user_id: string };
  /** The version this one would replace, or null for a first listing. */
  base: string | null;
  checks: { ok: number; warn: number; fail: number };
  /** Permissions it asks for that the last approved version didn't. */
  new_asks: string[];
}

export interface QueuePage {
  items: QueueItem[];
  next_cursor?: string;
}

export interface Publisher {
  user_id: string;
  username: string | null;
  created_at: number;
  decisions: { approved: number; rejected: number; removed: number };
  recent: { id: string; version: string; status: VersionStatus; decided_at: number | null }[];
}

export interface PackageFile {
  path: string;
  bytes: number;
  sha256: string;
  /** Not text: never shown as text, and called out. */
  binary: boolean;
}

/** What grew since the last approved version, as the app's `digest_diff`. */
export interface PermissionsDiff {
  asks_added?: string[];
  asks_removed?: string[];
  added_commands?: string[];
  network_added?: string[];
  tools_added?: { name: string; description?: string }[];
  tools_changed?: { name: string; description_after?: string }[];
  ui_replace_added?: string[];
  ui_change_added?: string[];
}

export interface Submission {
  plugin: PluginCard;
  version: VersionInfo;
  publisher: Publisher;
  /** The signed manifest, as submitted. */
  manifest: Record<string, unknown>;
  readme: string;
  permissions: Permissions;
  files: PackageFile[];
  /** The last approved version, which the diff is against, or null. */
  base: { version: string; manifest: Record<string, unknown>; files: PackageFile[] } | null;
  permissions_diff: PermissionsDiff | null;
  checks: Check[];
}

export type PluginStatus = "listed" | "unlisted" | "suspended" | "removed";

export interface AdminPlugin {
  plugin: PluginCard;
  status: PluginStatus;
  owner: { user_id: string; username: string | null; created_at: number };
  versions: VersionInfo[];
  reports: Report[];
}

export type ReportStatus = "open" | "dismissed" | "actioned";

export interface Report {
  id: string;
  plugin_id: string;
  plugin_name?: string;
  version?: string | null;
  reason: ReportReason;
  text: string;
  reporter: { username: string | null; user_id: string };
  created_at: number;
  status: ReportStatus;
  note?: string | null;
}

export interface ReportsPage {
  reports: Report[];
  next_cursor?: string;
}

export interface AuditEntry {
  id: string;
  at: number;
  actor: { username: string | null; user_id: string };
  action: string;
  plugin_id?: string | null;
  version?: string | null;
  detail?: string | null;
}

export interface AuditPage {
  entries: AuditEntry[];
  next_cursor?: string;
}
