/**
 * What an author and a signed-in person do:
 *
 *   GET  /store/v1/me/plugins                         → { plugins: [PluginCard & { listing, versions }] }
 *   POST /store/v1/plugins/:id/versions/:version/{withdraw|yank|unyank}
 *   POST /store/v1/plugins/:id/installed              → { counted }
 *   POST /store/v1/plugins/:id/reports { reason, text, version? } → { id }
 *
 * A deploy key reads only its own plugin's state and changes nothing here
 * (amendment 6): withdraw, yank and unyank need a signed-in session, so a
 * leaked key can add a pending version and nothing more.
 *
 * Version transitions an author makes: pending → withdrawn; approved →
 * yanked (no new installs, existing ones keep working); yanked → approved.
 * Each is one guarded UPDATE, logged in the audit log with the author as
 * actor, and moves the listing in the same batch.
 */
import type { Env } from "../env";
import { firstPageKeys } from "./catalog";
import { authorCard, authorsOf, pluginCard, versionInfo } from "./cards";
import { cacheKey, purge } from "./cache";
import { type PluginRow, VERSION_ORDER, type VersionRow, auditIfChanged, parseList, pluginRow, relist, versionRow } from "./db";
import { answer, fail, notFound, objectBody, rateLimited, refusedBody } from "./http";
import { authenticatePublisher, optionalUser, readBody } from "./principal";
import { allowWrite } from "./publish";
import { decodeSegment, versionFileKeys } from "./serve";

export const REPORT_REASONS = ["malware", "misleading", "broken", "copyright", "other"] as const;
export const MAX_REPORT_TEXT = 2000;
export const REPORTS_PER_DAY = 10;

const listingOf = (p: PluginRow): "listed" | "unlisted" | "suspended" =>
  p.status === "suspended" ? "suspended" : p.listed_version ? "listed" : "unlisted";

export const handleMyPlugins = async (request: Request, env: Env, db: D1Database): Promise<Response> => {
  const publisher = await authenticatePublisher(request, env);
  if (publisher instanceof Response) return publisher;
  const plugins =
    publisher.via === "deploy_key"
      ? (await db.prepare("SELECT * FROM plugins WHERE id = ? AND owner_id = ?").bind(publisher.pluginScope ?? "", publisher.userId).all<PluginRow>()).results
      : (await db.prepare("SELECT * FROM plugins WHERE owner_id = ? ORDER BY created_at DESC, seq DESC").bind(publisher.userId).all<PluginRow>()).results;
  const versions = plugins.length
    ? (
        await db
          .prepare(
            `SELECT * FROM versions WHERE plugin_id IN (SELECT value FROM json_each(?)) ORDER BY plugin_id, ${VERSION_ORDER}`
          )
          .bind(JSON.stringify(plugins.map((p) => p.id)))
          .all<VersionRow>()
      ).results
    : [];
  const authors = await authorsOf(env, [publisher.userId]);
  const author = authorCard(authors[publisher.userId]!);
  return answer({
    plugins: plugins.map((p) => {
      const own = versions.filter((v) => v.plugin_id === p.id);
      // Never listed: the card shows its newest submission.
      const shown: PluginRow = p.listed_version
        ? p
        : own[0]
          ? { ...p, name: own[0].name, summary: own[0].summary, icon: own[0].icon, categories: own[0].categories, keywords: own[0].keywords }
          : p;
      const card = pluginCard(shown, author);
      return {
        ...card,
        icon: p.listed_version ? card.icon : null,
        listing: listingOf(p),
        versions: own.map((v) => versionInfo(v, true))
      };
    })
  });
};

type AuthorAction = "withdraw" | "yank" | "unyank";
const TRANSITIONS: Record<AuthorAction, { readonly from: string; readonly to: string; readonly moves: boolean }> = {
  withdraw: { from: "pending", to: "withdrawn", moves: false },
  yank: { from: "approved", to: "yanked", moves: true },
  unyank: { from: "yanked", to: "approved", moves: true }
};

/** The keys a change to plugin `p`'s listing or versions moves. */
export const pluginCacheKeys = async (db: D1Database, p: PluginRow, versions: readonly string[]): Promise<string[]> => {
  const categories = new Set(parseList(p.categories));
  const after = await pluginRow(db, p.id);
  for (const c of parseList(after?.categories ?? null)) categories.add(c);
  const files = await Promise.all(versions.map((v) => versionFileKeys(db, p.id, v)));
  return [cacheKey(["plugins", p.id]), cacheKey(["plugins", p.id, "versions"]), ...firstPageKeys([...categories]), ...files.flat()];
};

export const handleAuthorAction = async (request: Request, env: Env, db: D1Database, id: string, versionSegment: string, action: AuthorAction): Promise<Response> => {
  const publisher = await authenticatePublisher(request, env);
  if (publisher instanceof Response) return publisher;
  if (publisher.via !== "session") return fail(403, "key_not_allowed", "Withdrawing, yanking and unyanking need a signed-in session.");
  if (!(await allowWrite(env, publisher.userId))) return rateLimited();
  const versionText = decodeSegment(versionSegment);
  const plugin = await pluginRow(db, id);
  const version = versionText && plugin ? await versionRow(db, id, versionText) : null;
  if (!plugin || !version || plugin.owner_id !== publisher.userId) return notFound();
  const t = TRANSITIONS[action];
  if (version.status !== t.from) {
    return fail(409, "invalid_state", `Version ${version.version} is ${version.status}: only a ${t.from} version can be ${action === "withdraw" ? "withdrawn" : action === "yank" ? "yanked" : "unyanked"}.`);
  }
  const now = Date.now();
  const results = await db.batch([
    db
      .prepare(
        `UPDATE versions SET status = ? WHERE plugin_id = ? AND version = ? AND status = ?
           AND EXISTS (SELECT 1 FROM plugins WHERE id = ? AND owner_id = ?)`
      )
      .bind(t.to, id, version.version, t.from, id, publisher.userId),
    auditIfChanged(db, now, publisher.userId, `version.${action}`, id, version.version, undefined),
    ...relist(db, id, null)
  ]);
  if ((results[0]?.meta.changes ?? 0) === 0) return fail(409, "invalid_state", "The version changed meanwhile: look again.");
  if (t.moves) await purge(await pluginCacheKeys(db, plugin, [version.version]));
  const updated = await versionRow(db, id, version.version);
  return answer({ version: versionInfo(updated!, true) });
};

export const handleInstalled = async (request: Request, env: Env, db: D1Database, id: string): Promise<Response> => {
  const caller = await optionalUser(request, env);
  if (!caller) return fail(401, "unauthenticated", "Sign in to do this.");
  if (!(await allowWrite(env, caller.userId))) return rateLimited();
  const results = await db.batch([
    db
      .prepare(
        `INSERT INTO installs (plugin_id, user_id, at)
         SELECT ?, ?, ? WHERE EXISTS (SELECT 1 FROM plugins p WHERE p.id = ? AND p.status = 'active' AND p.listed_version IS NOT NULL)
         ON CONFLICT DO NOTHING`
      )
      .bind(id, caller.userId, Date.now(), id),
    db.prepare("UPDATE plugins SET installs = installs + 1 WHERE id = ? AND changes() = 1").bind(id)
  ]);
  const counted = (results[0]?.meta.changes ?? 0) === 1;
  if (!counted && !(await db.prepare("SELECT 1 FROM plugins p WHERE p.id = ? AND p.status = 'active' AND p.listed_version IS NOT NULL").bind(id).first())) {
    return notFound();
  }
  return answer({ counted });
};

export const handleReport = async (request: Request, env: Env, db: D1Database, id: string): Promise<Response> => {
  const caller = await optionalUser(request, env);
  if (!caller) return fail(401, "unauthenticated", "Sign in to report a plugin.");
  if (!(await allowWrite(env, caller.userId))) return rateLimited();
  const raw = await readBody(request, 8 * 1024);
  if (typeof raw === "string") return refusedBody(raw);
  const body = objectBody(raw);
  const reason = body?.reason;
  const text = body?.text ?? "";
  const version = body?.version ?? null;
  if (typeof reason !== "string" || !(REPORT_REASONS as readonly string[]).includes(reason)) {
    return fail(400, "bad_request", `reason is one of ${REPORT_REASONS.join(", ")}.`);
  }
  if (typeof text !== "string" || text.length > MAX_REPORT_TEXT) return fail(400, "bad_request", `text is at most ${MAX_REPORT_TEXT} characters.`);
  if (version !== null && (typeof version !== "string" || version.length > 128)) return fail(400, "bad_request", "version is a version of this plugin.");
  const plugin = await pluginRow(db, id);
  if (!plugin || plugin.status !== "active" || !plugin.listed_version) return notFound();
  const open = await db
    .prepare("SELECT id FROM reports WHERE plugin_id = ? AND reporter_id = ? AND status = 'open'")
    .bind(id, caller.userId)
    .first<{ id: number }>();
  if (open) return answer({ id: open.id, status: "open" });
  const now = Date.now();
  const inserted = await db
    .prepare(
      `INSERT INTO reports (plugin_id, version, reporter_id, reason, text, created_at)
       SELECT ?, ?, ?, ?, ?, ? WHERE (SELECT COUNT(*) FROM reports WHERE reporter_id = ? AND created_at > ?) < ?
       ON CONFLICT DO NOTHING
       RETURNING id`
    )
    // (ON CONFLICT: the one open report per person, reports_one_open.)
    .bind(id, version, caller.userId, reason, text, now, caller.userId, now - 86_400_000, REPORTS_PER_DAY)
    .first<{ id: number }>();
  if (!inserted) {
    // Another report of theirs landed meanwhile, or the day's limit.
    const raced = await db.prepare("SELECT id FROM reports WHERE plugin_id = ? AND reporter_id = ? AND status = 'open'").bind(id, caller.userId).first<{ id: number }>();
    if (raced) return answer({ id: raced.id, status: "open" });
    return fail(429, "rate_limited", "Too many reports today. Try again tomorrow.");
  }
  return answer({ id: inserted.id, status: "open" }, 201);
};
