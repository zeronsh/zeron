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
const ORIGIN = "https://zeron.sh";

export interface Site {
  /** Where it listens, for a browser (http://127.0.0.1:…). */
  url: URL;
  /** A request to the site, as from a browser at https://zeron.sh. */
  fetch(path: string, init?: RequestInit): Promise<Response>;
  /** A signed-in visitor's cookie (a fake account: user_ada, user_grace, …). */
  signIn(user: string): Promise<string>;
  dispose(): Promise<void>;
}

/** Every module of the built Worker, entry first. */
function modules(dir: string) {
  const files = (readdirSync(dir, { recursive: true }) as string[]).filter((f) => f.endsWith(".mjs") || f.endsWith(".js")).sort((a, b) => (a === "entry.mjs" ? -1 : b === "entry.mjs" ? 1 : a.localeCompare(b)));
  return files.map((f) => ({ type: "ESModule" as const, path: relative(dir, `${dir}/${f}`), contents: readFileSync(`${dir}/${f}`, "utf8") }));
}

export async function startSite(): Promise<Site> {
  const edge = await build({ entryPoints: [new URL("fake-edge/index.ts", root).pathname], bundle: true, format: "esm", platform: "neutral", target: "es2024", write: false });
  const { workerOptions } = unstable_getMiniflareWorkerOptions(new URL("dist/server/wrangler.json", root).pathname);
  const mf = new Miniflare(
    // (wrangler describes a Worker in Miniflare's v4 options; Miniflare 5 converts them.)
    convertV4MiniflareOptions({
      workers: [
        { ...workerOptions, name: "site", modules: modules(new URL("dist/server", root).pathname), modulesRoot: new URL("dist/server", root).pathname, serviceBindings: { EDGE: "comet-native-edge" } },
        { name: "comet-native-edge", modules: true, script: edge.outputFiles[0]!.text, compatibilityDate: "2026-07-01" },
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
