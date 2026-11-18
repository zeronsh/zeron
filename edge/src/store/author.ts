/**
 * What an author and a signed-in person do:
 *
 *   GET  /store/v1/me/plugins                         → { publisher_id, plugins: [PluginCard & { listing, versions }] }
 *   POST /store/v1/plugins/:id/versions/:version/{yank|unyank}
 *   POST /store/v1/plugins/:id/installed              → { counted }
 *   POST /store/v1/plugins/:id/reports { reason, text, version? } → { id }
 *
 * A deploy key reads only the plugins in its scope its account owns, and
 * changes nothing here: yank and unyank need a signed-in session.
 *
 * Version transitions an author makes: listed → yanked (no new installs,
 * existing ones keep working); yanked → listed, for a version its author
 * yanked (one a newer version superseded stays yanked: amendment 45), which
 * supersedes what is older on its line, as any version listed does.
 * Each is one guarded UPDATE, logged in the audit log with the author as
 * actor, and moves the listing in the same batch.
 *
 * `/me/plugins` is the owner's view: every version in every state, each
 * with its review (a declined one's reason and words are its author's to
 * read), how it was published (`via`, the signing `key`, and the deploy key
 * by its name then), so a device can raise a notice for each publish it
 * didn't make (amendment 44), and why a yank or a removal happened. It
 * names the account's `publisher_id` too, plugins or none: what a device's
 * proof for a new deploy key binds (amendment 44, proof v2).
 */
import type { Env } from "../env";
import { STORE_DATA } from "./data";
import { authorCard, authorsOf, listingState, ownerCard, publisherId, publisherIds, versionInfo } from "./cards";
import { type PluginRow, VERSION_ORDER, type VersionRow, auditIfChanged, pluginRow, relist, supersedeOlder, supersededAt, versionRow } from "./db";
import { answer, fail, notFound, objectBody, rateLimited, refusedBody } from "./http";
import { authenticatePublisher, keyInScope, readBody } from "./principal";
import { allowWrite } from "./publish";
import { purgeCatalog } from "./purge";
import { decodeSegment } from "./serve";

export const REPORT_REASONS: readonly string[] = STORE_DATA.report_reasons;
export const MAX_REPORT_TEXT = 2000;
export const REPORTS_PER_DAY = 10;

export const handleMyPlugins = async (request: Request, env: Env, db: D1Database): Promise<Response> => {
  const publisher = await authenticatePublisher(request, env);
  if (publisher instanceof Response) return publisher;
  const owned = (await db.prepare("SELECT * FROM plugins WHERE owner_id = ? ORDER BY created_at DESC, seq DESC").bind(publisher.userId).all<PluginRow>()).results;
  // A deploy key sees the plugins in its scope, and only while its account owns them.
  const plugins = publisher.via === "deploy_key" ? owned.filter((p) => keyInScope(publisher, p)) : owned;
  const versions = plugins.length
    ? (
        await db
          .prepare(`SELECT * FROM versions WHERE plugin_id IN (SELECT value FROM json_each(?)) ORDER BY plugin_id, ${VERSION_ORDER}`)
          .bind(JSON.stringify(plugins.map((p) => p.id)))
          .all<VersionRow>()
      ).results
    : [];
  const [authors, publishers] = await Promise.all([authorsOf(env, [publisher.userId]), publisherIds([publisher.userId, ...versions.map((v) => v.publisher_id)])]);
  const author = authorCard(authors[publisher.userId]!);
  return answer({
    publisher_id: await publisherId(publisher.userId),
    plugins: plugins.map((p) => {
      const own = versions.filter((v) => v.plugin_id === p.id);
      return {
        // Nothing listed: the card shows its highest version.
        ...ownerCard(p, own[0], author),
        listing: listingState(p),
        versions: own.map((v) => ({
          ...versionInfo(v, true),
          // The account that published it, fixed at publish: a plugin
          // transferred in raises no notice for its former owner's versions.
          publisher_id: publishers.get(v.publisher_id)!,
          // A deploy key's name is its account's: a version a former owner
          // published shows none to whoever owns the plugin now.
          ...(v.publisher_id === publisher.userId ? {} : { deploy_key: null }),
          superseded: v.superseded_at !== null ? { at: v.superseded_at, by: v.superseded_by } : null
        }))
      };
    })
  });
};

type AuthorAction = "yank" | "unyank";

export const handleAuthorAction = async (request: Request, env: Env, db: D1Database, id: string, versionSegment: string, action: AuthorAction): Promise<Response> => {
  const publisher = await authenticatePublisher(request, env);
  if (publisher instanceof Response) return publisher;
  if (publisher.via !== "session") return fail(403, "key_not_allowed", "Yanking and unyanking need a signed-in session.");
  if (!(await allowWrite(env, publisher.userId))) return rateLimited();
  const versionText = decodeSegment(versionSegment);
  const plugin = await pluginRow(db, id);
  const version = versionText && plugin ? await versionRow(db, id, versionText) : null;
  if (!plugin || !version || plugin.owner_id !== publisher.userId) return notFound();
  if (action === "yank" && version.status !== "listed") {
    return fail(409, "invalid_state", `Version ${version.version} is ${version.status}: only a listed version can be yanked.`);
  }
  if (action === "unyank" && (version.status !== "yanked" || version.yanked_by !== "author")) {
    return fail(
      409,
      "invalid_state",
      version.status === "yanked"
        ? `Version ${version.version} was superseded by ${version.superseded_by ?? "a newer version"} before anyone verified it: it stays yanked. Publish a new version instead.`
        : `Version ${version.version} is ${version.status}: only a version you yanked can be unyanked.`
    );
  }
  const now = Date.now();
  const update =
    action === "yank"
      ? db.prepare(
          `UPDATE versions SET status = 'yanked', yanked_by = 'author' WHERE plugin_id = ? AND version = ? AND status = 'listed'
             AND EXISTS (SELECT 1 FROM plugins WHERE id = ? AND owner_id = ?)`
        )
      : db.prepare(
          `UPDATE versions SET status = 'listed', yanked_by = NULL WHERE plugin_id = ? AND version = ? AND status = 'yanked' AND yanked_by = 'author'
             AND EXISTS (SELECT 1 FROM plugins WHERE id = ? AND owner_id = ?)`
        );
  const results = await db.batch([
    update.bind(id, version.version, id, publisher.userId),
    auditIfChanged(db, now, publisher.userId, `version.${action}`, id, version.version, undefined),
    // Listed again, it supersedes what is older, as any version listed does.
    ...(action === "unyank" ? [supersedeOlder(db, id, version.version, now)] : []),
    ...relist(db, id, null)
  ]);
  if ((results[0]?.meta.changes ?? 0) === 0) return fail(409, "invalid_state", "The version changed meanwhile: look again.");
  await purgeCatalog(db, id, [version.version, ...(action === "unyank" ? await supersededAt(db, id, version.version, now) : [])]);
  const updated = await versionRow(db, id, version.version);
  return answer({ version: versionInfo(updated!, true) });
};

/**
 * The signed-in person a write is from: the app's bearer, or the web
 * session's cookie with the site's Origin and CSRF token (else `403
 * bad_csrf`). Never `optionalUser`, which is for reads; never a deploy key.
 */
const writer = async (request: Request, env: Env): Promise<{ userId: string } | Response> => {
  const caller = await authenticatePublisher(request, env);
  if (caller instanceof Response) return caller;
  return caller.via === "session" ? caller : fail(403, "key_not_allowed", "A deploy key can't do this.");
};

export const handleInstalled = async (request: Request, env: Env, db: D1Database, id: string): Promise<Response> => {
  const caller = await writer(request, env);
  if (caller instanceof Response) return caller;
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
  const caller = await writer(request, env);
  if (caller instanceof Response) return caller;
  if (!(await allowWrite(env, caller.userId))) return rateLimited();
  const raw = await readBody(request, 8 * 1024);
  if (typeof raw === "string") return refusedBody(raw);
  const body = objectBody(raw);
  const reason = body?.reason;
  const text = body?.text ?? "";
  const version = body?.version ?? null;
  if (typeof reason !== "string" || !REPORT_REASONS.includes(reason)) {
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
