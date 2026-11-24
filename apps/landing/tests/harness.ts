// The built site, running as it will deploy (dist/server's Worker on
// workerd, its files from dist/client), with the fake edge behind its EDGE
// binding: both in one Miniflare, so no other worker on this machine is
// ever reached by name. (tests/edge puts the same Worker beside the real
// edge instead: siteWorker.)
import { readdirSync, readFileSync } from "node:fs";
import { relative } from "node:path";
import { build } from "esbuild";
import { convertV4MiniflareOptions, Miniflare } from "miniflare";
import { unstable_getMiniflareWorkerOptions } from "wrangler";

const root = new URL("../", import.meta.url);

export interface Site {
  /** Where it listens, for a browser (http://127.0.0.1:…). */
  url: URL;
  /** A request to the site, as from a browser at https://zeron.sh. */
  fetch(path: string, init?: RequestInit): Promise<Response>;
  /** A signed-in visitor's cookie (a fake account: user_ada, user_grace, …). The fake's only. */
  signIn(user: string): Promise<string>;
  /** Shape the fake's queue (its /_fake/queue): empty, this long, or as it is (null). The fake's only. */
  queueAs(as: "empty" | number | null): Promise<void>;
  /** Flip one of the fake's own switches (POST /_fake/<what>?<params>). The fake's only. */
  fake(what: string, params?: Record<string, string>): Promise<void>;
  dispose(): Promise<void>;
}

/** Every module of the built Worker, entry first. */
function modules(dir: string) {
  const files = (readdirSync(dir, { recursive: true }) as string[]).filter((f) => f.endsWith(".mjs") || f.endsWith(".js")).sort((a, b) => (a === "entry.mjs" ? -1 : b === "entry.mjs" ? 1 : a.localeCompare(b)));
  return files.map((f) => ({ type: "ESModule" as const, path: relative(dir, `${dir}/${f}`), contents: readFileSync(`${dir}/${f}`, "utf8") }));
}

/**
 * The built site as one Worker named "site", described as wrangler
 * describes it (Miniflare's v4 options: its config, its files, its
 * modules), with `serviceBindings` for its EDGE.
 */
export function siteWorker(serviceBindings: Record<string, unknown>): Record<string, unknown> {
  const { workerOptions } = unstable_getMiniflareWorkerOptions(new URL("dist/server/wrangler.json", root).pathname);
  return { ...workerOptions, name: "site", modules: modules(new URL("dist/server", root).pathname), modulesRoot: new URL("dist/server", root).pathname, serviceBindings };
}

/**
 * `edge`: the fake (the default); "missing", no EDGE binding at all; or
 * "broken", one that fails every request (the site's own pages must be
 * whole either way).
 */
export async function startSite(options: { edge?: "fake" | "missing" | "broken"; host?: string; port?: number } = {}): Promise<Site> {
  const edgeKind = options.edge ?? "fake";
  const fake =
    edgeKind === "fake"
      ? [{ name: "comet-native-edge", modules: true, script: (await build({ entryPoints: [new URL("fake-edge/index.ts", root).pathname], bundle: true, format: "esm", platform: "neutral", target: "es2024", write: false })).outputFiles[0]!.text, compatibilityDate: "2026-07-01" }]
      : [];
  const edge =
    edgeKind === "fake"
      ? { EDGE: "comet-native-edge" }
      : edgeKind === "broken"
        ? {
            EDGE: async () => {
              throw new Error("the edge is down");
            },
          }
        : {};
  // (Miniflare 5 converts the v4 options wrangler gives.)
  // (An address of its own when asked: the preview, fake-edge/serve.ts; a free one for the tests.)
  const address = options.port === undefined ? {} : { host: options.host ?? "127.0.0.1", port: options.port };
  const mf = new Miniflare(convertV4MiniflareOptions({ ...address, workers: [siteWorker(edge), ...fake] } as never));
  const url = await mf.ready;
  const fetch = async (path: string, init: RequestInit = {}) => (await mf.dispatchFetch(new URL(path, "https://zeron.sh"), { redirect: "manual", ...init } as never)) as unknown as Response;
  return {
    url: new URL(url.href),
    fetch,
    async signIn(user) {
      const response = await fetch(`/auth/web/callback?as=${user}&return_to=/`);
      const cookie = response.headers.get("set-cookie");
      if (!cookie) throw new Error(`no session for ${user}: ${response.status}`);
      return cookie.split(";")[0]!;
    },
    async fake(what, params = {}) {
      if (edgeKind !== "fake") throw new Error("only the fake edge has switches");
      const worker = await mf.getWorker("comet-native-edge");
      await worker.fetch(`https://edge.invalid/_fake/${what}?${new URLSearchParams(params)}`, { method: "POST" });
    },
    async queueAs(as) {
      if (edgeKind !== "fake") throw new Error("only the fake edge has a queue to shape");
      const worker = await mf.getWorker("comet-native-edge");
      await worker.fetch(`https://edge.invalid/_fake/queue?as=${as ?? ""}`, { method: "POST" });
    },
    dispose: () => mf.dispose(),
  };
}
