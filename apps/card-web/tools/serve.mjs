// Serves dist/ as the live site would: /@u and /@u/player are the pages,
// /cards/u is JSON, everything else a file; the player answers with the
// Content-Security-Policy that lets X frame it (and no X-Frame-Options).
// `node tools/serve.mjs [port]` (default 8787), or `startServer()` from
// another tool.
import { createServer } from "node:http";
import { readFile, stat } from "node:fs/promises";
import { dirname, extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";
import { PAGE_CSP, PLAYER_CSP } from "./build.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const dist = join(here, "..", "dist");

const TYPES = {
  ".html": "text/html; charset=utf-8", ".js": "text/javascript; charset=utf-8", ".mjs": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8", ".json": "application/json", ".webp": "image/webp", ".png": "image/png",
  ".jpg": "image/jpeg", ".woff2": "font/woff2", ".txt": "text/plain; charset=utf-8", ".svg": "image/svg+xml"
};

// The file for a request path, and the headers its kind gets.
export function resolve(pathname) {
  let p = decodeURIComponent(pathname);
  const headers = { "X-Content-Type-Options": "nosniff" };
  let m;
  if ((m = /^\/(?:@|%40)([^/]+)\/player\/?$/i.exec(p))) {
    p = `/@${m[1].toLowerCase()}/player/index.html`;
    headers["Content-Security-Policy"] = PLAYER_CSP;
    headers["Cache-Control"] = "public, max-age=60";
  } else if ((m = /^\/_gallery\/([^/]+)\/player\/?$/i.exec(p))) {
    p = `/_gallery/${m[1].toLowerCase()}/player/index.html`;
    headers["Content-Security-Policy"] = PLAYER_CSP;
  } else if ((m = /^\/(?:@|%40)([^/]+)\/?$/i.exec(p))) {
    p = `/@${m[1].toLowerCase()}/index.html`;
    headers["Content-Security-Policy"] = PAGE_CSP;
    headers["Cache-Control"] = "public, max-age=60";
  } else if ((m = /^\/cards\/([^/.]+)$/.exec(p))) {
    p = `/cards/${m[1].toLowerCase()}.json`;
    headers["Access-Control-Allow-Origin"] = "*";
    headers["Cache-Control"] = "public, max-age=60";
  } else if (p === "/") {
    p = "/index.html";
  } else if (p.startsWith("/card/fonts/")) {
    headers["Cache-Control"] = "public, max-age=31536000, immutable";
  }
  const file = normalize(join(dist, p));
  if (!file.startsWith(dist)) return null;
  return { file, headers };
}

export function startServer(port = 0) {
  const server = createServer(async (req, res) => {
    const url = new URL(req.url, "http://localhost");
    const hit = resolve(url.pathname);
    if (!hit) { res.writeHead(403); res.end("forbidden"); return; }
    try {
      const s = await stat(hit.file);
      if (!s.isFile()) throw new Error("not a file");
      const body = await readFile(hit.file);
      res.writeHead(200, { "Content-Type": TYPES[extname(hit.file)] || "application/octet-stream", "Content-Length": body.length, ...hit.headers });
      res.end(req.method === "HEAD" ? undefined : body);
    } catch {
      res.writeHead(404, { "Content-Type": "text/plain; charset=utf-8" });
      res.end("not found");
    }
  });
  return new Promise((ok) => server.listen(port, "127.0.0.1", () => ok({ server, port: server.address().port, url: `http://127.0.0.1:${server.address().port}` })));
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  const { url } = await startServer(Number(process.argv[2]) || 8787);
  console.log(`serving ${dist} at ${url}`);
}
