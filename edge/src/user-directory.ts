/**
 * UserDirectory — ONE Durable Object (`users1`) recording every WorkOS user
 * (name, email, profile picture, when they joined: WorkOS `created_at`) and
 * every organization with its members.
 *
 * Fed by WorkOS webhooks (`user.*`, `organization.*` and
 * `organization_membership.*`: created, updated, deleted) at
 * `POST /workos/webhook`, signed with the endpoint's secret
 * (`WORKOS_WEBHOOK_SECRET`). Deliveries can arrive out of order and retry,
 * so a write lands only when its `updated_at` is at least the stored one;
 * a delete leaves a tombstone (id, joined, deleted) with the profile cleared,
 * and nothing revives it.
 *
 * Membership rows name a user and an org, not directory rows, so they can
 * land before either. `GET /admin/organizations` joins them on read: a member
 * shows the user's current profile, and a member whose user or org was
 * deleted drops out however the deliveries were ordered.
 *
 * Every user gets a user number (#1, #2, …) the first time they're recorded:
 * stored, never changed or reused, and kept by a tombstone. Users recorded
 * before numbers existed were numbered once, in join order.
 *
 * `GET /admin/users[?format=csv]` and `GET /admin/organizations[?format=csv]`
 * (ADMIN_TOKEN) read the tables back.
 *
 * Badges: an operator defines a badge once (`POST /admin/badges`, id like
 * `cloud-tester`) and grants or revokes it for users
 * (`POST /admin/badges/{grant|revoke}`). A new badge is a new row, not new
 * code. Grants name a user id, not a directory row, so a user can hold a
 * badge before their first webhook lands.
 *
 * Usernames: a member card's URL (card-routes.ts). `usernames` holds every
 * name ever given, lowercase, keyed by name, so a name can't be held twice
 * however claims race. A user's name is the row without `retired_at`; a
 * retired row keeps its name out of circulation, so no one takes over a
 * deleted user's URL. The scene count a card derives from (card.ts) is stored
 * on the user at their first name, not with the name: a rename or a new claim
 * after an operator's clear keeps the card. Operators set or clear names at
 * `POST /admin/usernames/{set|clear}`.
 *
 * Writes are one row per sign-up or profile change, so a single instance is
 * not a hotspot.
 */
import { DurableObject } from "cloudflare:workers";
import { CARD_SCENES, cardFirstName, deriveCard, type CardFace } from "./card";
import { forgetCard } from "./card-cache";
import type { Env } from "./env";
import { json, jsonError } from "./http";
import { adminAuthorized } from "./cloud/billing";
import { foldUsername } from "./username";

export const USER_DIRECTORY_NAME = "users1";

/** A new row's user number. Conflict updates never set `user_number`, so a
 * number is taken only on first insert. */
const NEXT_USER_NUMBER = "(SELECT COALESCE(MAX(user_number), 0) + 1 FROM users)";

/** Availability checks and claims one user may make a minute. */
const USERNAME_ATTEMPTS_PER_MINUTE = 60;
/** Users tracked at once; past this the window starts over for everyone. */
const MAX_TRACKED_ATTEMPTS = 10_000;

/** The fields of a WorkOS user object this directory keeps (wire names). */
export interface WorkOsUser {
  readonly id: string;
  readonly email: string;
  readonly first_name: string | null;
  readonly last_name: string | null;
  readonly profile_picture_url: string | null;
  readonly created_at: string;
  readonly updated_at: string;
}

export interface DirectoryUser {
  readonly userId: string;
  /** 1 for the first user recorded; assigned once, never reused. */
  readonly userNumber: number;
  readonly email: string | null;
  readonly name: string | null;
  readonly firstName: string | null;
  readonly lastName: string | null;
  readonly profilePictureUrl: string | null;
  /** ISO 8601, WorkOS `created_at`. */
  readonly joinedAt: string;
  readonly updatedAt: string;
  readonly deletedAt: string | null;
  /** Null until they claim one, and once it's cleared or they're deleted. */
  readonly username: string | null;
  /** Badges held, oldest grant first. */
  readonly badges: readonly UserBadge[];
}

/** Everything the public can read about a member, in this order. New fields
 * here are public: no email, user id, surname, picture, org, operator note or
 * timestamp, ever (usernames.workerd.test.ts pins the exact set). */
export interface PublicCard extends CardFace {
  readonly username: string;
  /** `cardFirstName`: never a surname or an initial. */
  readonly firstName: string;
  readonly userNumber: number;
  /** `YYYY-MM-DD`, UTC. */
  readonly joinedOn: string;
  /** Oldest grant first. */
  readonly badges: readonly { readonly id: string; readonly name: string; readonly icon: string | null }[];
}

/** Why a username wasn't given. Only an operator sees `username_retired`
 * apart from `username_taken`. */
export type UsernameError =
  | "rate_limited"
  | "unknown_user"
  | "deleted_user"
  | "already_claimed"
  | "username_taken"
  | "username_retired";

export type UsernameResult = { readonly error?: undefined; readonly card: PublicCard } | { readonly error: UsernameError };

export interface Badge {
  /** Stable id, e.g. `cloud-tester`. */
  readonly id: string;
  readonly name: string;
  readonly description: string | null;
  /** Free-form icon name for clients to map; null = their default. */
  readonly icon: string | null;
  /** Epoch ms the badge was first defined. */
  readonly createdAt: number;
  /** How many users hold it. */
  readonly holders: number;
}

export interface UserBadge {
  readonly id: string;
  /** Epoch ms. */
  readonly grantedAt: number;
  readonly note: string | null;
}

/** The fields of a WorkOS organization object this directory keeps. */
export interface WorkOsOrganization {
  readonly id: string;
  readonly name: string;
  readonly external_id: string | null;
  /** Domain names only; WorkOS's objects also carry verification state. */
  readonly domains: readonly string[];
  readonly created_at: string;
  readonly updated_at: string;
}

/** The fields of a WorkOS organization membership this directory keeps. */
export interface WorkOsMembership {
  readonly id: string;
  readonly organization_id: string;
  readonly user_id: string;
  /** Role slug, e.g. `admin`. */
  readonly role: string | null;
  /** `active`, `inactive` or `pending`. */
  readonly status: string;
  readonly created_at: string;
  readonly updated_at: string;
}

export interface OrganizationMember {
  readonly membershipId: string;
  readonly userId: string;
  /** Null until the user's own webhook lands, as is the rest of the profile. */
  readonly userNumber: number | null;
  readonly email: string | null;
  readonly name: string | null;
  readonly profilePictureUrl: string | null;
  readonly role: string | null;
  readonly status: string;
  /** ISO 8601, when the membership was created. */
  readonly joinedAt: string;
}

export interface DirectoryOrganization {
  readonly orgId: string;
  readonly name: string | null;
  readonly externalId: string | null;
  readonly domains: readonly string[];
  /** ISO 8601, WorkOS `created_at`. */
  readonly createdAt: string;
  readonly updatedAt: string;
  readonly deletedAt: string | null;
  /** Active members; `members` also lists inactive and pending ones. */
  readonly memberCount: number;
  /** Oldest membership first. */
  readonly members: readonly OrganizationMember[];
}

const fullName = (first: string | null, last: string | null): string | null =>
  [first, last].filter(Boolean).join(" ") || null;

export class UserDirectory extends DurableObject<Env> {
  /** In memory: this is the one instance, so it sees every attempt, and an
   * eviction only forgives a window. */
  private readonly usernameAttempts = new Map<string, { windowStart: number; count: number }>();

  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    this.ctx.storage.sql.exec(`CREATE TABLE IF NOT EXISTS users (
      user_id TEXT PRIMARY KEY, email TEXT, first_name TEXT, last_name TEXT,
      profile_picture_url TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
      deleted_at TEXT, recorded_at INTEGER NOT NULL, user_number INTEGER)`);
    const columns = [...this.ctx.storage.sql.exec("SELECT name FROM pragma_table_info('users')")].map((row) => row.name);
    if (!columns.includes("user_number")) this.ctx.storage.sql.exec("ALTER TABLE users ADD COLUMN user_number INTEGER");
    this.ctx.storage.sql.exec("CREATE UNIQUE INDEX IF NOT EXISTS users_by_number ON users (user_number)");
    this.numberUsers();
    if (!columns.includes("card_scenes")) this.ctx.storage.sql.exec("ALTER TABLE users ADD COLUMN card_scenes INTEGER");
    this.ctx.storage.sql.exec(`CREATE TABLE IF NOT EXISTS usernames (
      username TEXT PRIMARY KEY, user_id TEXT NOT NULL, claimed_at INTEGER NOT NULL, retired_at INTEGER,
      CHECK (length(username) BETWEEN 3 AND 20 AND username GLOB '[a-z0-9]*' AND username NOT GLOB '*[^a-z0-9_-]*'))`);
    this.ctx.storage.sql.exec(
      "CREATE UNIQUE INDEX IF NOT EXISTS usernames_live_by_user ON usernames (user_id) WHERE retired_at IS NULL"
    );
    this.ctx.storage.sql.exec(`CREATE TABLE IF NOT EXISTS badges (
      badge_id TEXT PRIMARY KEY, name TEXT NOT NULL, description TEXT, icon TEXT,
      created_at INTEGER NOT NULL)`);
    this.ctx.storage.sql.exec(`CREATE TABLE IF NOT EXISTS user_badges (
      user_id TEXT NOT NULL, badge_id TEXT NOT NULL, granted_at INTEGER NOT NULL, note TEXT,
      PRIMARY KEY (user_id, badge_id))`);
    this.ctx.storage.sql.exec("CREATE INDEX IF NOT EXISTS user_badges_by_badge ON user_badges (badge_id)");
    this.ctx.storage.sql.exec(`CREATE TABLE IF NOT EXISTS organizations (
      org_id TEXT PRIMARY KEY, name TEXT, external_id TEXT, domains TEXT NOT NULL DEFAULT '[]',
      created_at TEXT NOT NULL, updated_at TEXT NOT NULL, deleted_at TEXT, recorded_at INTEGER NOT NULL)`);
    this.ctx.storage.sql.exec(`CREATE TABLE IF NOT EXISTS organization_memberships (
      membership_id TEXT PRIMARY KEY, org_id TEXT NOT NULL, user_id TEXT NOT NULL, role TEXT, status TEXT NOT NULL,
      created_at TEXT NOT NULL, updated_at TEXT NOT NULL, deleted_at TEXT, recorded_at INTEGER NOT NULL)`);
  }

  /** Number every user still without one — the rows recorded before numbers
   * existed — in join order, after the highest number held. */
  private numberUsers(): void {
    this.ctx.storage.transactionSync(() => {
      let next = (this.ctx.storage.sql.exec("SELECT COALESCE(MAX(user_number), 0) AS n FROM users").one().n as number) + 1;
      const unnumbered = [
        ...this.ctx.storage.sql.exec("SELECT user_id FROM users WHERE user_number IS NULL ORDER BY created_at, user_id")
      ];
      for (const row of unnumbered) {
        this.ctx.storage.sql.exec("UPDATE users SET user_number = ? WHERE user_id = ?", next++, row.user_id);
      }
    });
  }

  /** Define a badge, or rename/redescribe one (its id and creation time stay). */
  defineBadge(badge: { readonly id: string; readonly name: string; readonly description: string | null; readonly icon: string | null }): Badge {
    this.ctx.storage.sql.exec(
      `INSERT INTO badges (badge_id, name, description, icon, created_at) VALUES (?, ?, ?, ?, ?)
       ON CONFLICT (badge_id) DO UPDATE SET
         name = excluded.name, description = excluded.description, icon = excluded.icon`,
      badge.id,
      badge.name,
      badge.description,
      badge.icon,
      Date.now()
    );
    return this.listBadges().find((b) => b.id === badge.id)!;
  }

  listBadges(): Badge[] {
    return [
      ...this.ctx.storage.sql.exec(
        `SELECT b.*, (SELECT COUNT(*) FROM user_badges ub WHERE ub.badge_id = b.badge_id) AS holders
         FROM badges b ORDER BY b.created_at`
      )
    ].map((row) => ({
      id: row.badge_id as string,
      name: row.name as string,
      description: row.description as string | null,
      icon: row.icon as string | null,
      createdAt: row.created_at as number,
      holders: row.holders as number
    }));
  }

  /** Grant `badgeId` to each user (`"all"` = every live directory user).
   * Re-granting keeps the first grant. Undefined when the badge isn't defined;
   * otherwise how many users newly hold it. */
  grantBadge(badgeId: string, userIds: readonly string[] | "all", note: string | null): number | undefined {
    if (!this.badgeExists(badgeId)) return undefined;
    const now = Date.now();
    if (userIds === "all") {
      // Counted by RETURNING rows: `rowsWritten` also counts index writes.
      return [
        ...this.ctx.storage.sql.exec(
          `INSERT OR IGNORE INTO user_badges (user_id, badge_id, granted_at, note)
           SELECT user_id, ?, ?, ? FROM users WHERE deleted_at IS NULL RETURNING user_id`,
          badgeId,
          now,
          note
        )
      ].length;
    }
    let granted = 0;
    this.ctx.storage.transactionSync(() => {
      for (const userId of userIds) {
        granted += [
          ...this.ctx.storage.sql.exec(
            "INSERT OR IGNORE INTO user_badges (user_id, badge_id, granted_at, note) VALUES (?, ?, ?, ?) RETURNING user_id",
            userId,
            badgeId,
            now,
            note
          )
        ].length;
      }
    });
    return granted;
  }

  /** Take `badgeId` back from each user. Answers how many held it. */
  revokeBadge(badgeId: string, userIds: readonly string[]): number {
    let revoked = 0;
    this.ctx.storage.transactionSync(() => {
      for (const userId of userIds) {
        revoked += [
          ...this.ctx.storage.sql.exec(
            "DELETE FROM user_badges WHERE user_id = ? AND badge_id = ? RETURNING user_id",
            userId,
            badgeId
          )
        ].length;
      }
    });
    return revoked;
  }

  private badgeExists(badgeId: string): boolean {
    return [...this.ctx.storage.sql.exec("SELECT 1 FROM badges WHERE badge_id = ?", badgeId)].length > 0;
  }

  /** Record a user's current profile; stale or post-delete writes are no-ops.
   * A new user takes the next user number. Answers whether the row changed. */
  upsert(user: WorkOsUser): boolean {
    const cursor = this.ctx.storage.sql.exec(
      `INSERT INTO users (user_id, email, first_name, last_name, profile_picture_url, created_at, updated_at, recorded_at, user_number)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?, ${NEXT_USER_NUMBER})
       ON CONFLICT (user_id) DO UPDATE SET
         email = excluded.email, first_name = excluded.first_name, last_name = excluded.last_name,
         profile_picture_url = excluded.profile_picture_url, created_at = excluded.created_at,
         updated_at = excluded.updated_at, recorded_at = excluded.recorded_at
       WHERE users.deleted_at IS NULL AND excluded.updated_at >= users.updated_at`,
      user.id,
      user.email,
      user.first_name,
      user.last_name,
      user.profile_picture_url,
      user.created_at,
      user.updated_at,
      Date.now()
    );
    return cursor.rowsWritten > 0;
  }

  /** Tombstone a deleted user: keep id, number and joined date, drop the
   * profile and badges, and retire their username. Answers that name. */
  remove(user: WorkOsUser, deletedAt: string): string | null {
    const username = this.username(user.id);
    this.retireUsername(user.id);
    this.ctx.storage.sql.exec("DELETE FROM user_badges WHERE user_id = ?", user.id);
    this.ctx.storage.sql.exec(
      `INSERT INTO users (user_id, created_at, updated_at, deleted_at, recorded_at, user_number)
       VALUES (?, ?, ?, ?, ?, ${NEXT_USER_NUMBER})
       ON CONFLICT (user_id) DO UPDATE SET
         email = NULL, first_name = NULL, last_name = NULL, profile_picture_url = NULL,
         deleted_at = excluded.deleted_at, recorded_at = excluded.recorded_at`,
      user.id,
      user.created_at,
      user.updated_at,
      deletedAt,
      Date.now()
    );
    return username;
  }

  /** The user's name, or null. */
  username(userId: string): string | null {
    const [row] = this.ctx.storage.sql
      .exec("SELECT username FROM usernames WHERE user_id = ? AND retired_at IS NULL", userId)
      .toArray();
    return (row?.username as string | undefined) ?? null;
  }

  private retireUsername(userId: string): void {
    this.ctx.storage.sql.exec("UPDATE usernames SET retired_at = ? WHERE user_id = ? AND retired_at IS NULL", Date.now(), userId);
  }

  /** Whether `userId` used up their attempts this minute (and count this one). */
  private overAttemptLimit(userId: string): boolean {
    const now = Date.now();
    const seen = this.usernameAttempts.get(userId);
    if (seen && now - seen.windowStart < 60_000) return ++seen.count > USERNAME_ATTEMPTS_PER_MINUTE;
    if (this.usernameAttempts.size >= MAX_TRACKED_ATTEMPTS) this.usernameAttempts.clear();
    this.usernameAttempts.set(userId, { windowStart: now, count: 1 });
    return false;
  }

  /** Why this user can't hold a name, if any. */
  private userRefusal(userId: string): "unknown_user" | "deleted_user" | undefined {
    const [row] = this.ctx.storage.sql.exec("SELECT deleted_at FROM users WHERE user_id = ?", userId).toArray();
    if (!row) return "unknown_user";
    return row.deleted_at === null ? undefined : "deleted_user";
  }

  /** The stored scene count is set once, by the user's first name. */
  private freezeCardScenes(userId: string, scenes: number): void {
    this.ctx.storage.sql.exec("UPDATE users SET card_scenes = COALESCE(card_scenes, ?) WHERE user_id = ?", scenes, userId);
  }

  /** Whether a name is free; undefined past the attempt limit. A retired name
   * isn't, and reads no differently from a held one. */
  checkUsername(userId: string, username: string): boolean | undefined {
    if (this.overAttemptLimit(userId)) return undefined;
    return this.ctx.storage.sql.exec("SELECT 1 FROM usernames WHERE username = ?", username).toArray().length === 0;
  }

  /** A user takes their one name, `scenes` being today's count. The same claim
   * again succeeds, so a client can retry one whose answer it lost. */
  claimUsername(userId: string, username: string, scenes: number): UsernameResult {
    if (this.overAttemptLimit(userId)) return { error: "rate_limited" };
    return this.ctx.storage.transactionSync((): UsernameResult => {
      const refusal = this.userRefusal(userId);
      if (refusal) return { error: refusal };
      const held = this.username(userId);
      if (held) return held === username ? { card: this.card(username)! } : { error: "already_claimed" };
      const taken =
        this.ctx.storage.sql
          .exec(
            "INSERT INTO usernames (username, user_id, claimed_at) VALUES (?, ?, ?) ON CONFLICT DO NOTHING RETURNING username",
            username,
            userId,
            Date.now()
          )
          .toArray().length === 0;
      if (taken) return { error: "username_taken" };
      this.freezeCardScenes(userId, scenes);
      return { card: this.card(username)! };
    });
  }

  /** An operator gives a user a name, retiring any they held. A name retired
   * from this same user comes back to them; one held by or retired from anyone
   * else stays where it is. */
  setUsername(userId: string, username: string, scenes: number): UsernameResult {
    return this.ctx.storage.transactionSync((): UsernameResult => {
      const refusal = this.userRefusal(userId);
      if (refusal) return { error: refusal };
      const [row] = this.ctx.storage.sql.exec("SELECT user_id, retired_at FROM usernames WHERE username = ?", username).toArray();
      if (row && row.user_id !== userId) return { error: row.retired_at === null ? "username_taken" : "username_retired" };
      this.retireUsername(userId);
      this.ctx.storage.sql.exec(
        `INSERT INTO usernames (username, user_id, claimed_at) VALUES (?, ?, ?)
         ON CONFLICT (username) DO UPDATE SET retired_at = NULL`,
        username,
        userId,
        Date.now()
      );
      this.freezeCardScenes(userId, scenes);
      return { card: this.card(username)! };
    });
  }

  /** An operator takes a user's name away. It stays retired unless `release`
   * frees it for someone else. Answers the name, or null if they held none. */
  clearUsername(userId: string, release: boolean): string | null {
    const username = this.username(userId);
    if (!username) return null;
    if (release) this.ctx.storage.sql.exec("DELETE FROM usernames WHERE username = ?", username);
    else this.retireUsername(userId);
    return username;
  }

  /** The public card behind a name; undefined unless a live user holds it. */
  card(username: string): PublicCard | undefined {
    const [row] = this.ctx.storage.sql
      .exec(
        `SELECT u.user_id, u.user_number, u.first_name, u.created_at, u.card_scenes
         FROM usernames n JOIN users u ON u.user_id = n.user_id
         WHERE n.username = ? AND n.retired_at IS NULL AND u.deleted_at IS NULL`,
        username
      )
      .toArray();
    if (!row) return undefined;
    const badges = this.ctx.storage.sql
      .exec(
        `SELECT b.badge_id, b.name, b.icon FROM user_badges ub JOIN badges b ON b.badge_id = ub.badge_id
         WHERE ub.user_id = ? ORDER BY ub.granted_at, ub.badge_id`,
        row.user_id
      )
      .toArray();
    // Field by field, never spread from a row: a new column must not go public.
    const { scene, holo, sky } = deriveCard(row.user_id as string, row.card_scenes as number);
    return {
      username,
      firstName: cardFirstName(row.first_name as string | null, username),
      userNumber: row.user_number as number,
      joinedOn: new Date(row.created_at as string).toISOString().slice(0, 10),
      badges: badges.map((b) => ({ id: b.badge_id as string, name: b.name as string, icon: b.icon as string | null })),
      scene,
      holo,
      sky
    };
  }

  list(): DirectoryUser[] {
    const badges = new Map<string, UserBadge[]>();
    for (const row of this.ctx.storage.sql.exec("SELECT * FROM user_badges ORDER BY granted_at, badge_id")) {
      const userId = row.user_id as string;
      const held = badges.get(userId) ?? [];
      held.push({ id: row.badge_id as string, grantedAt: row.granted_at as number, note: row.note as string | null });
      badges.set(userId, held);
    }
    return [
      ...this.ctx.storage.sql.exec(
        `SELECT u.*, n.username FROM users u
         LEFT JOIN usernames n ON n.user_id = u.user_id AND n.retired_at IS NULL
         ORDER BY u.created_at`
      )
    ].map((row) => {
      const first = row.first_name as string | null;
      const last = row.last_name as string | null;
      return {
        userId: row.user_id as string,
        userNumber: row.user_number as number,
        email: row.email as string | null,
        name: fullName(first, last),
        firstName: first,
        lastName: last,
        profilePictureUrl: row.profile_picture_url as string | null,
        joinedAt: row.created_at as string,
        updatedAt: row.updated_at as string,
        deletedAt: row.deleted_at as string | null,
        username: row.username as string | null,
        badges: badges.get(row.user_id as string) ?? []
      };
    });
  }

  /** Record an org's current details; stale or post-delete writes are no-ops.
   * Answers whether the row changed. */
  upsertOrganization(org: WorkOsOrganization): boolean {
    const cursor = this.ctx.storage.sql.exec(
      `INSERT INTO organizations (org_id, name, external_id, domains, created_at, updated_at, recorded_at)
       VALUES (?, ?, ?, ?, ?, ?, ?)
       ON CONFLICT (org_id) DO UPDATE SET
         name = excluded.name, external_id = excluded.external_id, domains = excluded.domains,
         created_at = excluded.created_at, updated_at = excluded.updated_at, recorded_at = excluded.recorded_at
       WHERE organizations.deleted_at IS NULL AND excluded.updated_at >= organizations.updated_at`,
      org.id,
      org.name,
      org.external_id,
      JSON.stringify(org.domains),
      org.created_at,
      org.updated_at,
      Date.now()
    );
    return cursor.rowsWritten > 0;
  }

  /** Tombstone a deleted org: keep id and creation date, drop the details
   * (an org is often named after its one person). */
  removeOrganization(org: WorkOsOrganization, deletedAt: string): void {
    this.ctx.storage.sql.exec(
      `INSERT INTO organizations (org_id, created_at, updated_at, deleted_at, recorded_at)
       VALUES (?, ?, ?, ?, ?)
       ON CONFLICT (org_id) DO UPDATE SET
         name = NULL, external_id = NULL, domains = '[]',
         deleted_at = excluded.deleted_at, recorded_at = excluded.recorded_at`,
      org.id,
      org.created_at,
      org.updated_at,
      deletedAt,
      Date.now()
    );
  }

  /** Record a membership's current role and status; stale or post-delete
   * writes are no-ops. Answers whether the row changed. */
  upsertMembership(membership: WorkOsMembership): boolean {
    const cursor = this.ctx.storage.sql.exec(
      `INSERT INTO organization_memberships (membership_id, org_id, user_id, role, status, created_at, updated_at, recorded_at)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?)
       ON CONFLICT (membership_id) DO UPDATE SET
         role = excluded.role, status = excluded.status, created_at = excluded.created_at,
         updated_at = excluded.updated_at, recorded_at = excluded.recorded_at
       WHERE organization_memberships.deleted_at IS NULL AND excluded.updated_at >= organization_memberships.updated_at`,
      membership.id,
      membership.organization_id,
      membership.user_id,
      membership.role,
      membership.status,
      membership.created_at,
      membership.updated_at,
      Date.now()
    );
    return cursor.rowsWritten > 0;
  }

  /** Tombstone a removed membership, which nothing revives. */
  removeMembership(membership: WorkOsMembership, deletedAt: string): void {
    this.ctx.storage.sql.exec(
      `INSERT INTO organization_memberships (membership_id, org_id, user_id, role, status, created_at, updated_at, deleted_at, recorded_at)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
       ON CONFLICT (membership_id) DO UPDATE SET
         deleted_at = excluded.deleted_at, recorded_at = excluded.recorded_at`,
      membership.id,
      membership.organization_id,
      membership.user_id,
      membership.role,
      membership.status,
      membership.created_at,
      membership.updated_at,
      deletedAt,
      Date.now()
    );
  }

  listOrganizations(): DirectoryOrganization[] {
    const members = new Map<string, OrganizationMember[]>();
    for (const row of this.ctx.storage.sql.exec(
      `SELECT m.*, u.user_number, u.email, u.first_name, u.last_name, u.profile_picture_url
       FROM organization_memberships m LEFT JOIN users u ON u.user_id = m.user_id
       WHERE m.deleted_at IS NULL AND u.deleted_at IS NULL
       ORDER BY m.created_at, m.membership_id`
    )) {
      const orgId = row.org_id as string;
      const held = members.get(orgId) ?? [];
      held.push({
        membershipId: row.membership_id as string,
        userId: row.user_id as string,
        userNumber: row.user_number as number | null,
        email: row.email as string | null,
        name: fullName(row.first_name as string | null, row.last_name as string | null),
        profilePictureUrl: row.profile_picture_url as string | null,
        role: row.role as string | null,
        status: row.status as string,
        joinedAt: row.created_at as string
      });
      members.set(orgId, held);
    }
    return [...this.ctx.storage.sql.exec("SELECT * FROM organizations ORDER BY created_at, org_id")].map((row) => {
      const held = row.deleted_at === null ? (members.get(row.org_id as string) ?? []) : [];
      return {
        orgId: row.org_id as string,
        name: row.name as string | null,
        externalId: row.external_id as string | null,
        domains: JSON.parse(row.domains as string) as string[],
        createdAt: row.created_at as string,
        updatedAt: row.updated_at as string,
        deletedAt: row.deleted_at as string | null,
        memberCount: held.filter((m) => m.status === "active").length,
        members: held
      };
    });
  }
}

export const userDirectoryStub = (env: Pick<Env, "USER_DIRECTORY">) =>
  env.USER_DIRECTORY ? env.USER_DIRECTORY.get(env.USER_DIRECTORY.idFromName(USER_DIRECTORY_NAME)) : undefined;

// ── webhook signature ───────────────────────────────────────────────────────

/** How far a webhook's signed timestamp may drift (the WorkOS SDK default). */
export const WORKOS_SIGNATURE_TOLERANCE_MS = 180_000;

const hex = (bytes: ArrayBuffer): string =>
  [...new Uint8Array(bytes)].map((b) => b.toString(16).padStart(2, "0")).join("");

const timingSafeEqual = (a: string, b: string): boolean => {
  if (a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i++) diff |= a.charCodeAt(i) ^ b.charCodeAt(i);
  return diff === 0;
};

/** Sign `payload` the way WorkOS does: hex HMAC-SHA256 of `{ms}.{body}`. */
export const signWorkOsPayload = async (secret: string, timestampMs: number, payload: string): Promise<string> => {
  const key = await crypto.subtle.importKey(
    "raw",
    new TextEncoder().encode(secret),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"]
  );
  return hex(await crypto.subtle.sign("HMAC", key, new TextEncoder().encode(`${timestampMs}.${payload}`)));
};

/** `WorkOS-Signature: t={ms}, v1={hex}` over the raw body, within tolerance. */
export const verifyWorkOsSignature = async (
  secret: string,
  header: string | null,
  payload: string,
  nowMs = Date.now()
): Promise<boolean> => {
  if (!header) return false;
  let timestamp: number | undefined;
  let signature: string | undefined;
  for (const part of header.split(",")) {
    const [key, value] = part.trim().split("=", 2);
    if (key === "t" && value) timestamp = Number(value);
    if (key === "v1" && value) signature = value;
  }
  if (!timestamp || !Number.isFinite(timestamp) || !signature) return false;
  if (Math.abs(nowMs - timestamp) > WORKOS_SIGNATURE_TOLERANCE_MS) return false;
  return timingSafeEqual(signature, await signWorkOsPayload(secret, timestamp, payload));
};

// ── routes ──────────────────────────────────────────────────────────────────

/** WorkOS events are a few KB; anything larger isn't one. */
const MAX_WEBHOOK_BODY = 256 * 1024;

const isObject = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

const optionalString = (value: unknown): string | null => (typeof value === "string" && value ? value : null);

/** The user in a `user.*` event's `data`, or undefined if it isn't one. */
const parseUser = (data: unknown): WorkOsUser | undefined => {
  if (!isObject(data)) return undefined;
  const { id, email, created_at, updated_at } = data;
  if (typeof id !== "string" || !id.startsWith("user_")) return undefined;
  if (typeof email !== "string" || typeof created_at !== "string" || typeof updated_at !== "string") return undefined;
  return {
    id,
    email,
    first_name: optionalString(data.first_name),
    last_name: optionalString(data.last_name),
    profile_picture_url: optionalString(data.profile_picture_url),
    created_at,
    updated_at
  };
};

/** The organization in an `organization.*` event's `data`, or undefined. */
const parseOrganization = (data: unknown): WorkOsOrganization | undefined => {
  if (!isObject(data)) return undefined;
  const { id, name, domains, created_at, updated_at } = data;
  if (typeof id !== "string" || !id.startsWith("org_")) return undefined;
  if (typeof name !== "string" || typeof created_at !== "string" || typeof updated_at !== "string") return undefined;
  return {
    id,
    name,
    external_id: optionalString(data.external_id),
    domains: Array.isArray(domains)
      ? domains.flatMap((d: unknown) => (isObject(d) && typeof d.domain === "string" ? [d.domain] : []))
      : [],
    created_at,
    updated_at
  };
};

/** The membership in an `organization_membership.*` event's `data`, or undefined. */
const parseMembership = (data: unknown): WorkOsMembership | undefined => {
  if (!isObject(data)) return undefined;
  const { id, organization_id, user_id, role, status, created_at, updated_at } = data;
  if (typeof id !== "string" || !id.startsWith("om_")) return undefined;
  if (typeof organization_id !== "string" || !organization_id.startsWith("org_")) return undefined;
  if (typeof user_id !== "string" || !user_id.startsWith("user_")) return undefined;
  if (typeof status !== "string" || typeof created_at !== "string" || typeof updated_at !== "string") return undefined;
  return {
    id,
    organization_id,
    user_id,
    role: isObject(role) ? optionalString(role.slug) : null,
    status,
    created_at,
    updated_at
  };
};

const csvField = (value: string | null): string =>
  value === null ? "" : /[",\n]/.test(value) ? `"${value.replace(/"/g, '""')}"` : value;

const csvResponse = (filename: string, header: string, rows: readonly (readonly (string | null)[])[]): Response =>
  new Response(`${[header, ...rows.map((row) => row.map(csvField).join(","))].join("\n")}\n`, {
    headers: {
      "content-type": "text/csv; charset=utf-8",
      "content-disposition": `attachment; filename="${filename}"`
    }
  });

const BADGE_ID_RE = /^[a-z0-9][a-z0-9-]{0,63}$/;
const USER_ID_RE = /^user_[A-Za-z0-9]{1,64}$/;
/** One grant/revoke call; `"all"` covers the whole directory in one statement. */
const MAX_BADGE_USERS = 5000;

const readJson = async (request: Request): Promise<Record<string, unknown> | undefined> => {
  try {
    const body: unknown = await request.json();
    return isObject(body) ? body : undefined;
  } catch {
    return undefined;
  }
};

/** A string of at most `max` chars, null when absent; false when invalid. */
const optionalText = (value: unknown, max: number): string | null | false =>
  value === undefined || value === null ? null : typeof value === "string" && value.length <= max ? value || null : false;

/**
 * `/admin/badges`:
 *   GET                                   — every badge with its holder count
 *   POST {id, name, description?, icon?}  — define (or rename) a badge
 *   POST /grant  {badgeId, userIds | "all", note?} — answers {granted}
 *   POST /revoke {badgeId, userIds}       — answers {revoked}
 */
const handleBadgeRoute = async (
  request: Request,
  directory: DurableObjectStub<UserDirectory>,
  action: string
): Promise<Response> => {
  if (action === "") {
    if (request.method === "GET") return json({ badges: await directory.listBadges() });
    if (request.method !== "POST") return jsonError(405, "method_not_allowed", "GET or POST only.");
    const body = await readJson(request);
    const description = optionalText(body?.description, 500);
    const icon = optionalText(body?.icon, 64);
    if (
      !body ||
      typeof body.id !== "string" ||
      !BADGE_ID_RE.test(body.id) ||
      typeof body.name !== "string" ||
      !body.name.trim() ||
      body.name.length > 80 ||
      description === false ||
      icon === false
    ) {
      return jsonError(400, "bad_request", "Expected {id (a-z, 0-9, -), name, description?, icon?}.");
    }
    return json(await directory.defineBadge({ id: body.id, name: body.name.trim(), description, icon }));
  }
  if (action !== "grant" && action !== "revoke") return jsonError(404, "not_found", "Not found.");
  if (request.method !== "POST") return jsonError(405, "method_not_allowed", "POST only.");
  const body = await readJson(request);
  const note = optionalText(body?.note, 500);
  const userIds = body?.userIds;
  const validIds =
    Array.isArray(userIds) &&
    userIds.length > 0 &&
    userIds.length <= MAX_BADGE_USERS &&
    userIds.every((id) => typeof id === "string" && USER_ID_RE.test(id));
  if (
    !body ||
    typeof body.badgeId !== "string" ||
    !BADGE_ID_RE.test(body.badgeId) ||
    note === false ||
    !(validIds || (action === "grant" && userIds === "all"))
  ) {
    return jsonError(
      400,
      "bad_request",
      `Expected {badgeId, userIds (1–${MAX_BADGE_USERS} user ids${action === "grant" ? ' or "all"' : ""})${action === "grant" ? ", note?" : ""}}.`
    );
  }
  const ids = userIds === "all" ? "all" : [...new Set(userIds as string[])];
  if (action === "revoke") return json({ revoked: await directory.revokeBadge(body.badgeId, ids as string[]) });
  const granted = await directory.grantBadge(body.badgeId, ids, note);
  if (granted === undefined) return jsonError(404, "unknown_badge", "Define the badge first (POST /admin/badges).");
  return json({ granted });
};

/**
 * `/admin/usernames`:
 *   POST /set   {userId, username}  — give or change a name; reserved names are
 *                                     allowed here, retired ones are not
 *   POST /clear {userId, release?}  — answers {cleared}, the name taken away.
 *                                     It stays retired unless `release` frees it.
 */
const handleUsernameAdminRoute = async (
  request: Request,
  directory: DurableObjectStub<UserDirectory>,
  action: string
): Promise<Response> => {
  if (action !== "set" && action !== "clear") return jsonError(404, "not_found", "Not found.");
  if (request.method !== "POST") return jsonError(405, "method_not_allowed", "POST only.");
  const body = await readJson(request);
  const userId = body?.userId;
  if (typeof userId !== "string" || !USER_ID_RE.test(userId)) return jsonError(400, "bad_request", "Expected {userId, …}.");
  if (action === "clear") {
    const cleared = await directory.clearUsername(userId, body?.release === true);
    if (cleared) await forgetCard(cleared);
    return json({ cleared });
  }
  const username = foldUsername(body?.username);
  if (!username) return jsonError(400, "bad_request", "Expected {userId, username (3–20 letters, digits, _ or -)}.");
  const previous = await directory.username(userId);
  const result = await directory.setUsername(userId, username, CARD_SCENES);
  if (result.error === "unknown_user") return jsonError(404, result.error, "That user isn't in the directory.");
  if (result.error) return jsonError(409, result.error, "That username can't go to this user.");
  await Promise.all([forgetCard(username), previous && forgetCard(previous)]);
  return json(result.card);
};

/**
 * `POST /workos/webhook` (WorkOS-signed, no bearer), `GET /admin/users`,
 * `GET /admin/organizations`, `/admin/badges[/grant|/revoke]` and
 * `/admin/usernames/{set|clear}` (ADMIN_TOKEN) — routed BEFORE the bearer gate. Events other than the created, updated and
 * deleted of users, organizations and memberships are acknowledged so WorkOS
 * stops retrying.
 */
export const handleUserDirectoryRoute = async (
  request: Request,
  env: Env,
  url: URL
): Promise<Response | undefined> => {
  const badgeAction =
    url.pathname === "/admin/badges" ? "" : url.pathname.startsWith("/admin/badges/") ? url.pathname.slice(14) : undefined;
  const usernameAction = url.pathname.startsWith("/admin/usernames/") ? url.pathname.slice(17) : undefined;
  if (
    url.pathname === "/admin/users" ||
    url.pathname === "/admin/organizations" ||
    badgeAction !== undefined ||
    usernameAction !== undefined
  ) {
    if (!env.ADMIN_TOKEN) return json({ error: "not_found" }, 404);
    if (!(await adminAuthorized(request, env.ADMIN_TOKEN))) {
      return jsonError(401, "unauthenticated", "Admin token required.");
    }
    const directory = userDirectoryStub(env);
    if (!directory) return jsonError(503, "unavailable", "The user directory isn't set up on this server.");
    if (badgeAction !== undefined) return handleBadgeRoute(request, directory, badgeAction);
    if (usernameAction !== undefined) return handleUsernameAdminRoute(request, directory, usernameAction);
    if (request.method !== "GET") return jsonError(405, "method_not_allowed", "GET only.");
    const csv = url.searchParams.get("format") === "csv";
    if (url.pathname === "/admin/organizations") {
      const organizations = await directory.listOrganizations();
      if (!csv) return json({ count: organizations.length, organizations });
      return csvResponse(
        "zeron-organizations.csv",
        "org_id,name,external_id,domains,member_count,members,created_at,updated_at,deleted_at",
        organizations.map((o) => [
          o.orgId,
          o.name,
          o.externalId,
          o.domains.join(";") || null,
          String(o.memberCount),
          o.members
            .filter((m) => m.status === "active")
            .map((m) => m.email ?? m.userId)
            .join(";") || null,
          o.createdAt,
          o.updatedAt,
          o.deletedAt
        ])
      );
    }
    const users = await directory.list();
    if (!csv) return json({ count: users.length, users });
    return csvResponse(
      "zeron-users.csv",
      "user_number,user_id,email,name,first_name,last_name,profile_picture_url,joined_at,updated_at,deleted_at,badges,username",
      users.map((u) => [
        String(u.userNumber),
        u.userId,
        u.email,
        u.name,
        u.firstName,
        u.lastName,
        u.profilePictureUrl,
        u.joinedAt,
        u.updatedAt,
        u.deletedAt,
        u.badges.map((b) => b.id).join(";") || null,
        u.username
      ])
    );
  }

  if (url.pathname !== "/workos/webhook") return undefined;
  if (request.method !== "POST") return jsonError(405, "method_not_allowed", "POST only.");
  const directory = userDirectoryStub(env);
  if (!env.WORKOS_WEBHOOK_SECRET || !directory) {
    return jsonError(503, "unavailable", "WorkOS webhooks aren't set up on this server.");
  }
  const payload = await request.text();
  if (payload.length > MAX_WEBHOOK_BODY) return jsonError(413, "too_large", "Body too large.");
  if (!(await verifyWorkOsSignature(env.WORKOS_WEBHOOK_SECRET, request.headers.get("workos-signature"), payload))) {
    return jsonError(400, "bad_signature", "Signature check failed.");
  }
  let event: unknown;
  try {
    event = JSON.parse(payload);
  } catch {
    return jsonError(400, "bad_request", "Body must be JSON.");
  }
  if (!isObject(event) || typeof event.event !== "string") {
    return jsonError(400, "bad_request", "Body must be a WorkOS event.");
  }
  const eventId = event.id;
  const deletedAt = typeof event.created_at === "string" ? event.created_at : new Date().toISOString();
  /** A delete answers nothing and always lands; an upsert answers whether it did. */
  const apply = async (write: () => Promise<boolean | void>): Promise<Response> => {
    try {
      return json({ received: true, applied: (await write()) ?? true });
    } catch (e) {
      console.error("users: applying a webhook failed", eventId, String(e));
      // WorkOS retries a non-2xx: nothing is lost.
      return jsonError(503, "unavailable", "Try again.");
    }
  };
  switch (event.event) {
    case "user.created":
    case "user.updated":
    case "user.deleted": {
      const user = parseUser(event.data);
      if (!user) return jsonError(400, "bad_request", "Event data must be a WorkOS user.");
      if (event.event !== "user.deleted") return apply(() => directory.upsert(user));
      return apply(async () => {
        const username = await directory.remove(user, deletedAt);
        if (username) await forgetCard(username);
      });
    }
    case "organization.created":
    case "organization.updated":
    case "organization.deleted": {
      const org = parseOrganization(event.data);
      if (!org) return jsonError(400, "bad_request", "Event data must be a WorkOS organization.");
      return apply(() =>
        event.event === "organization.deleted" ? directory.removeOrganization(org, deletedAt) : directory.upsertOrganization(org)
      );
    }
    case "organization_membership.created":
    case "organization_membership.updated":
    case "organization_membership.deleted": {
      const membership = parseMembership(event.data);
      if (!membership) return jsonError(400, "bad_request", "Event data must be a WorkOS organization membership.");
      return apply(() =>
        event.event === "organization_membership.deleted"
          ? directory.removeMembership(membership, deletedAt)
          : directory.upsertMembership(membership)
      );
    }
    default:
      return json({ received: true });
  }
};
