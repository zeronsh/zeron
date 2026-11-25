/**
 * `POST /store/v1/updates { zeron, plugins: [{ id, version, hash, prerelease?, verified_only? }], templates?: [the same] }`
 * → { updates: [Offer & { verified_update?: Offer }], template_updates: [the same], blocked: [{ id, version, reason, item_kind }] }
 *   Offer = { id, kind, version, hash, sha256, bytes, key, publisher, publisher_id, notes, added, removed, prerelease, verified, verified_at }
 *
 * One request for every store-installed item on a device, plugins and
 * templates. For each, the newest listed version above the installed one
 * that the device may take: its `engines.zeron` (a template's, `>=` the
 * Zeron it was made from) accepting the device's Zeron, a pre-release only
 * where the device opted in for that item. Every listed version is offered
 * at once (amendment 45), each saying whether it is verified; when that
 * newest one isn't and a newer-than-installed verified one is listed, it is
 * offered beside it (`verified_update`), and with `verified_only` it is the
 * offer. A yanked version is never offered. A suspended item offers nothing.
 *
 * An id asked about as the other kind than it is is answered as an unknown
 * one: never offered, never blocked.
 *
 * `blocked` names installed versions staff took down, when the installed
 * contents are the ones taken down (`hash`): the device turns them off and
 * shows the reason.
 *
 * `added` and `removed` are what the update changes in what the plugin asks,
 * against the store's record of the installed version (or, for contents the
 * store doesn't know, against nothing). A template asks for nothing: `[]`.
 */
import type { Env } from "../env";
import { authorsOf } from "./cards";
import { type ConsentItem, consentDiff } from "./consent";
import { fail, answer, objectBody, refusedBody } from "./http";
import type { Kind } from "./kind";
import { readBody } from "./principal";
import { type Version, comparePrecedence, parseRange, parseVersion, satisfies } from "./semver";
import { parseRequest } from "./update-ask";

const MAX_BODY = 64 * 1024;

interface Row {
  readonly plugin_id: string;
  readonly kind: Kind;
  readonly version: string;
  readonly status: string;
  readonly removal_reason: string | null;
  readonly verification: string;
  readonly verified_at: number | null;
  readonly signer_key: string | null;
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

export const handleUpdates = async (request: Request, env: Env, db: D1Database): Promise<Response> => {
  const raw = await readBody(request, MAX_BODY);
  if (typeof raw === "string") return refusedBody(raw);
  const parsed = parseRequest(objectBody(raw));
  if (typeof parsed === "string") return fail(400, "bad_request", parsed);
  if (parsed.items.length === 0) return answer({ updates: [], template_updates: [], blocked: [] });

  const rows = (
    await db
      .prepare(
        `SELECT v.plugin_id, p.kind, v.version, v.status, v.removal_reason, v.verification, v.verified_at, v.signer_key, v.content_hash,
                v.package_sha256, v.package_bytes, v.engines_zeron, v.permissions, v.notes, v.is_release, p.owner_id, p.status AS plugin_status
         FROM versions v JOIN plugins p ON p.id = v.plugin_id
         WHERE v.plugin_id IN (SELECT value FROM json_each(?))`
      )
      .bind(JSON.stringify([...new Set(parsed.items.map((p) => p.id))]))
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

  const offers: Record<Kind, unknown[]> = { plugin: [], template: [] };
  const blocked: unknown[] = [];
  for (const installed of parsed.items) {
    // (An id of the other kind than it was asked about as is unknown.)
    const found = byPlugin.get(installed.id) ?? [];
    const versions = found[0]?.kind === installed.kind ? found : [];
    const current = versions.find((v) => v.version === installed.version && v.content_hash === installed.hash);
    if (current && current.status === "taken_down") {
      blocked.push({ id: installed.id, version: installed.version, reason: current.removal_reason ?? "", item_kind: installed.kind });
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
      const diff = installed.kind === "plugin" ? consentDiff(before, JSON.parse(row.permissions) as ConsentItem) : { added: [], removed: [] };
      return {
        id: installed.id,
        kind: installed.kind,
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
    offers[installed.kind].push({
      ...offer(chosen.row),
      ...(chosen.row.verification !== "verified" && vetted ? { verified_update: offer(vetted.row) } : {})
    });
  }
  return answer({ updates: offers.plugin, template_updates: offers.template, blocked });
};
