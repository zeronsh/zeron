/**
 * `POST /store/v1/updates { zeron, plugins: [{ id, version, hash, prerelease? }] }`
 * → { updates: [{ id, version, hash, key, publisher, sha256, bytes, prerelease, notes, added, removed, in_review? }],
 *     blocked: [{ id, version, reason }] }
 *
 * One request for every store-installed plugin on a device. For each, the
 * highest version above the installed one that the device may take: approved
 * and not yanked, its `engines.zeron` accepting the device's Zeron, a
 * pre-release only where the device opted in for that plugin. A signed-in
 * caller who owns the plugin is also offered their own pending versions
 * (`in_review: true`); nobody else is. A suspended plugin offers nothing.
 *
 * `blocked` names installed versions an admin took down with a block, when
 * the installed contents are the ones taken down (`hash`): the device turns
 * them off and shows the reason.
 *
 * `added` and `removed` are what the update changes in what the plugin asks,
 * against the store's record of the installed version (or, for contents the
 * store doesn't know, against nothing).
 */
import type { Env } from "../env";
import { authorsOf } from "./cards";
import { type ConsentItem, consentDiff } from "./consent";
import { fail, answer, objectBody, refusedBody } from "./http";
import { optionalUser, readBody } from "./principal";
import { type Version, comparePrecedence, parseRange, parseVersion, satisfies } from "./semver";

export const MAX_UPDATE_PLUGINS = 200;
const MAX_BODY = 64 * 1024;
const ID_RE = /^[a-z0-9][a-z0-9-]{1,39}$/;
const HASH_RE = /^[0-9a-f]{64}$/;

interface Installed {
  readonly id: string;
  readonly version: string;
  readonly hash: string;
  readonly prerelease: boolean;
}

interface Row {
  readonly plugin_id: string;
  readonly version: string;
  readonly status: string;
  readonly blocked: number;
  readonly block_reason: string | null;
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
      seen.has(p.id)
    ) {
      return "Each plugin is { id, version, hash, prerelease? }, each id once.";
    }
    seen.add(p.id);
    plugins.push({ id: p.id, version: p.version, hash: p.hash, prerelease: p.prerelease === true });
  }
  return { zeron, plugins };
};

export const handleUpdates = async (request: Request, env: Env, db: D1Database): Promise<Response> => {
  const raw = await readBody(request, MAX_BODY);
  if (typeof raw === "string") return refusedBody(raw);
  const parsed = parseRequest(objectBody(raw));
  if (typeof parsed === "string") return fail(400, "bad_request", parsed);
  const caller = await optionalUser(request, env);
  if (parsed.plugins.length === 0) return answer({ updates: [], blocked: [] });

  const rows = (
    await db
      .prepare(
        `SELECT v.plugin_id, v.version, v.status, v.blocked, v.block_reason, v.signer_key, v.content_hash, v.package_sha256,
                v.package_bytes, v.engines_zeron, v.permissions, v.notes, v.is_release, p.owner_id, p.status AS plugin_status
         FROM versions v JOIN plugins p ON p.id = v.plugin_id
         WHERE v.plugin_id IN (SELECT value FROM json_each(?)) AND v.status IN ('approved', 'pending', 'yanked', 'removed')`
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
    if (current && current.status === "removed" && current.blocked === 1) {
      blocked.push({ id: installed.id, version: installed.version, reason: current.block_reason ?? "" });
    }
    const have = parseVersion(installed.version);
    if (!have || versions.length === 0 || versions[0]!.plugin_status !== "active") continue;
    const mine = caller !== null && caller.userId === versions[0]!.owner_id;
    let best: { row: Row; version: Version } | undefined;
    for (const row of versions) {
      const inReview = row.status === "pending";
      if (!(row.status === "approved" || (inReview && mine))) continue;
      const version = parseVersion(row.version);
      if (!version || comparePrecedence(version, have) <= 0) continue;
      if (row.is_release === 0 && !installed.prerelease) continue;
      const range = row.engines_zeron === null ? null : parseRange(row.engines_zeron);
      if (row.engines_zeron !== null && (!range || !satisfies(parsed.zeron, range))) continue;
      if (!best || comparePrecedence(version, best.version) > 0) best = { row, version };
    }
    if (!best) continue;
    const before = current ? (JSON.parse(current.permissions) as ConsentItem) : null;
    const diff = consentDiff(before, JSON.parse(best.row.permissions) as ConsentItem);
    updates.push({
      id: installed.id,
      version: best.row.version,
      hash: best.row.content_hash,
      key: best.row.signer_key,
      publisher: owners[best.row.owner_id]?.username ?? null,
      sha256: best.row.package_sha256,
      bytes: best.row.package_bytes,
      prerelease: best.row.is_release === 0,
      notes: best.row.notes,
      added: diff.added,
      removed: diff.removed,
      ...(best.row.status === "pending" ? { in_review: true } : {})
    });
  }
  return answer({ updates, blocked });
};
