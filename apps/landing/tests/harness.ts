// The built site, running as it will deploy (dist/server's Worker on
// workerd, its files from dist/client), with the fake edge behind its EDGE
// binding: both in one Miniflare, so no other worker on this machine is
// ever reached by name.
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
  dispose(): Promise<void>;
}

async function forward(request: Request, edge: string): Promise<Response> {
  const asked = new URL(request.url);
  const headers = new Headers(request.headers);
  headers.set("MF-Original-URL", request.url);
  // (How Miniflare's loopback carried it, not the request: fetch frames the body itself.)
  for (const name of ["content-length", "transfer-encoding", "connection", "host"]) headers.delete(name);
  const body = request.method === "GET" || request.method === "HEAD" ? undefined : await request.arrayBuffer();
  const answer = await fetch(new URL(`${asked.pathname}${asked.search}`, edge), { method: request.method, headers, body, redirect: "manual" });
  // EDGE_TRACE=1: every call the site made, and what the edge said to the ones it refused.
  if (process.env.EDGE_TRACE) console.error(`edge: ${request.method} ${asked.pathname}${asked.search} → ${answer.status}${answer.status >= 400 ? ` ${await answer.clone().text()}` : ""}`);
  return answer;
}

/** Every module of the built Worker, entry first. */
function modules(dir: string) {
  const files = (readdirSync(dir, { recursive: true }) as string[]).filter((f) => f.endsWith(".mjs") || f.endsWith(".js")).sort((a, b) => (a === "entry.mjs" ? -1 : b === "entry.mjs" ? 1 : a.localeCompare(b)));
  return files.map((f) => ({ type: "ESModule" as const, path: relative(dir, `${dir}/${f}`), contents: readFileSync(`${dir}/${f}`, "utf8") }));
}

/**
 * `edge`: the fake (the default); "missing", no EDGE binding at all;
 * "broken", one that fails every request (the site's own pages must be
 * whole either way); or the real edge worker at `url` (tests/edge), with
 * `origin` the web origin it serves.
 */
export async function startSite(options: { edge?: "fake" | "missing" | "broken" | { url: string }; origin?: string } = {}): Promise<Site> {
  const edgeKind = options.edge ?? "fake";
  const ORIGIN = options.origin ?? "https://zeron.sh";
  const fake =
    edgeKind === "fake"
      ? [{ name: "comet-native-edge", modules: true, script: (await build({ entryPoints: [new URL("fake-edge/index.ts", root).pathname], bundle: true, format: "esm", platform: "neutral", target: "es2024", write: false })).outputFiles[0]!.text, compatibilityDate: "2026-07-01" }]
      : [];
  const { workerOptions } = unstable_getMiniflareWorkerOptions(new URL("dist/server/wrangler.json", root).pathname);
  const mf = new Miniflare(
    // (wrangler describes a Worker in Miniflare's v4 options; Miniflare 5 converts them.)
    convertV4MiniflareOptions({
      workers: [
        {
          ...workerOptions,
          name: "site",
          modules: modules(new URL("dist/server", root).pathname),
          modulesRoot: new URL("dist/server", root).pathname,
          serviceBindings:
            typeof edgeKind === "object"
              ? // The real edge, in Miniflare elsewhere: each request handed to it
                // as a service binding hands it (the URL the site asked, its
                // headers and body), redirects and cookies back as it sent them.
                { EDGE: (request: Request) => forward(request, edgeKind.url) }
              : edgeKind === "fake"
              ? { EDGE: "comet-native-edge" }
              : edgeKind === "broken"
                ? {
                    EDGE: async () => {
                      throw new Error("the edge is down");
                    },
                  }
                : {},
        },
        ...fake,
      ],
    } as never),
  );
  const url = await mf.ready;
  const fetch = async (path: string, init: RequestInit = {}) => (await mf.dispatchFetch(new URL(path, ORIGIN), { redirect: "manual", ...init } as never)) as unknown as Response;
  return {
    url: new URL(url.href),
    fetch,
    async signIn(user) {
      const response = await fetch(`/auth/web/callback?as=${user}&return_to=/`);
      const cookie = response.headers.get("set-cookie");
      if (!cookie) throw new Error(`no session for ${user}: ${response.status}`);
      return cookie.split(";")[0]!;
    },
    dispose: () => mf.dispose(),
  };
}
