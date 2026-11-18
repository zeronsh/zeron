/**
 * `POST /store/v1/updates { zeron, plugins: [{ id, version, hash, prerelease?, verified_only? }] }`
 * → { updates: [Offer & { verified_update?: Offer }], blocked: [{ id, version, reason }] }
 *   Offer = { id, version, hash, sha256, bytes, key, publisher, publisher_id, notes, added, removed, prerelease, verified, verified_at }
 *
 * One request for every store-installed plugin on a device. For each, the
 * newest listed version above the installed one that the device may take:
 * its `engines.zeron` accepting the device's Zeron, a pre-release only where
 * the device opted in for that plugin. Every listed version is offered at
 * once (amendment 45), each saying whether it is verified; when that newest
 * one isn't and a newer-than-installed verified one is listed, it is
 * offered beside it (`verified_update`), and with `verified_only` it is the
 * offer. A yanked version is never offered. A suspended plugin offers
 * nothing.
 *
 * `blocked` names installed versions staff took down, when the installed
 * contents are the ones taken down (`hash`): the device turns them off and
 * shows the reason.
 *
 * `added` and `removed` are what the update changes in what the plugin asks,
 * against the store's record of the installed version (or, for contents the
 * store doesn't know, against nothing).
 */
import type { Env } from "../env";
import { LIMITS } from "./data";
import { authorsOf } from "./cards";
import { type ConsentItem, consentDiff } from "./consent";
import { fail, answer, objectBody, refusedBody } from "./http";
import { readBody } from "./principal";
import { type Version, comparePrecedence, parseRange, parseVersion, satisfies } from "./semver";

export const MAX_UPDATE_PLUGINS = LIMITS.max_update_plugins;
const MAX_BODY = 64 * 1024;
const ID_RE = /^[a-z0-9][a-z0-9-]{1,39}$/;
const HASH_RE = /^[0-9a-f]{64}$/;

interface Installed {
  readonly id: string;
  readonly version: string;
  readonly hash: string;
  readonly prerelease: boolean;
  /** Offer only verified versions of it. */
  readonly verifiedOnly: boolean;
}

interface Row {
  readonly plugin_id: string;
  readonly version: string;
  readonly status: string;
  readonly removal_reason: string | null;
  readonly verification: string;
  readonly verified_at: number | null;
  readonly signer_key: string;
  readonly content_hash: string;
  readonly package_sha256: string;
  readonly package_bytes: number;
  readonly engines_zeron: string | null;
  readonly permissions: string;
  readonly notes: string | null;
  readonly is_release: number;
  readonly owner_id: string;
  readonly plugin_status: string;
}

const parseRequest = (body: Record<string, unknown> | undefined): { zeron: Version; plugins: Installed[] } | string => {
  if (!body) return "The body is { zeron, plugins: [{ id, version, hash, prerelease? }] }.";
  const zeron = typeof body.zeron === "string" ? parseVersion(body.zeron) : undefined;
  if (!zeron) return "zeron is this device's Zeron version (1.2.3).";
  if (!Array.isArray(body.plugins) || body.plugins.length > MAX_UPDATE_PLUGINS) return `plugins is a list of at most ${MAX_UPDATE_PLUGINS}.`;
  const plugins: Installed[] = [];
  const seen = new Set<string>();
  for (const item of body.plugins as unknown[]) {
    const p = item as Record<string, unknown> | null;
    if (
      !p ||
      typeof p.id !== "string" ||
      !ID_RE.test(p.id) ||
      typeof p.version !== "string" ||
      p.version.length > 128 ||
      typeof p.hash !== "string" ||
      !HASH_RE.test(p.hash) ||
      (p.prerelease !== undefined && typeof p.prerelease !== "boolean") ||
      (p.verified_only !== undefined && typeof p.verified_only !== "boolean") ||
      seen.has(p.id)
    ) {
      return "Each plugin is { id, version, hash, prerelease?, verified_only? }, each id once.";
    }
    seen.add(p.id);
    plugins.push({ id: p.id, version: p.version, hash: p.hash, prerelease: p.prerelease === true, verifiedOnly: p.verified_only === true });
  }
  return { zeron, plugins };
};

export const handleUpdates = async (request: Request, env: Env, db: D1Database): Promise<Response> => {
  const raw = await readBody(request, MAX_BODY);
  if (typeof raw === "string") return refusedBody(raw);
  const parsed = parseRequest(objectBody(raw));
  if (typeof parsed === "string") return fail(400, "bad_request", parsed);
  if (parsed.plugins.length === 0) return answer({ updates: [], blocked: [] });

  const rows = (
    await db
      .prepare(
        `SELECT v.plugin_id, v.version, v.status, v.removal_reason, v.verification, v.verified_at, v.signer_key, v.content_hash,
                v.package_sha256, v.package_bytes, v.engines_zeron, v.permissions, v.notes, v.is_release, p.owner_id, p.status AS plugin_status
         FROM versions v JOIN plugins p ON p.id = v.plugin_id
         WHERE v.plugin_id IN (SELECT value FROM json_each(?))`
      )
      .bind(JSON.stringify(parsed.plugins.map((p) => p.id)))
      .all<Row>()
  ).results;
  const byPlugin = new Map<string, Row[]>();
  for (const row of rows) {
    const list = byPlugin.get(row.plugin_id);
    if (list) list.push(row);
    else byPlugin.set(row.plugin_id, [row]);
  }
  // Who owns each plugin now (amendment 25): a device asks again when it changes.
  const owners = await authorsOf(env, [...new Set(rows.map((r) => r.owner_id))]);

  const updates: unknown[] = [];
  const blocked: unknown[] = [];
  for (const installed of parsed.plugins) {
    const versions = byPlugin.get(installed.id) ?? [];
    const current = versions.find((v) => v.version === installed.version && v.content_hash === installed.hash);
    if (current && current.status === "taken_down") {
      blocked.push({ id: installed.id, version: installed.version, reason: current.removal_reason ?? "" });
    }
    const have = parseVersion(installed.version);
    if (!have || versions.length === 0 || versions[0]!.plugin_status !== "active") continue;
    // The newest it may take, and the newest verified one.
    let newest: { row: Row; version: Version } | undefined;
    let vetted: { row: Row; version: Version } | undefined;
    for (const row of versions) {
      if (row.status !== "listed") continue;
      const version = parseVersion(row.version);
      if (!version || comparePrecedence(version, have) <= 0) continue;
      if (row.is_release === 0 && !installed.prerelease) continue;
      const range = row.engines_zeron === null ? null : parseRange(row.engines_zeron);
      if (row.engines_zeron !== null && (!range || !satisfies(parsed.zeron, range))) continue;
      if (!newest || comparePrecedence(version, newest.version) > 0) newest = { row, version };
      if (row.verification === "verified" && (!vetted || comparePrecedence(version, vetted.version) > 0)) vetted = { row, version };
    }
    const before = current ? (JSON.parse(current.permissions) as ConsentItem) : null;
    const offer = (row: Row) => {
      const diff = consentDiff(before, JSON.parse(row.permissions) as ConsentItem);
      return {
        id: installed.id,
        version: row.version,
        hash: row.content_hash,
        sha256: row.package_sha256,
        bytes: row.package_bytes,
        key: row.signer_key,
        publisher: owners[row.owner_id]?.username ?? null,
        publisher_id: owners[row.owner_id]!.id,
        notes: row.notes,
        added: diff.added,
        removed: diff.removed,
        prerelease: row.is_release === 0,
        verified: row.verification === "verified",
        verified_at: row.verification === "verified" ? row.verified_at : null
      };
    };
    const chosen = installed.verifiedOnly ? vetted : newest;
    if (!chosen) continue;
    updates.push({
      ...offer(chosen.row),
      ...(chosen.row.verification !== "verified" && vetted ? { verified_update: offer(vetted.row) } : {})
    });
  }
  return answer({ updates, blocked });
};
