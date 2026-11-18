// The built site as it deploys (its Worker on workerd, its files from
// dist/client) against the fake edge: what each route answers, to whom,
// with which headers, and that nothing a plugin's author writes runs.
import { readFileSync } from "node:fs";
import { afterAll, beforeAll, describe, expect, test } from "vitest";
import { hasInlineScript, rawElements } from "../src/lib/store/security.ts";
import { fileUrl } from "../src/lib/store/urls.ts";

/** A page runs only the site's own script files: no inline script, no handler, no javascript: link. */
function inert(body: string) {
  for (const e of rawElements(body)) if (e.tag === "script") expect(e.attributes).toMatch(/\bsrc="\/_astro\/|type="application\/ld\+json"/);
  const markup = body.replace(/="[^"]*"/g, '=""');
  expect(markup).not.toMatch(/<[a-z][^>]*\son[a-z]+=/i);
  expect(body).not.toMatch(/(href|src)="\s*(javascript|data|vbscript):/i);
}
import { startSite, type Site } from "./harness.ts";

let site: Site;
beforeAll(async () => {
  site = await startSite();
}, 60_000);
afterAll(() => site?.dispose());

const html = async (path: string, cookie?: string) => {
  const response = await site.fetch(path, cookie ? { headers: { cookie } } : {});
  return { response, body: await response.text() };
};

/** A form post from the site's own pages, as a browser sends one. */
const post = (path: string, cookie: string, fields: Record<string, string>, origin = "https://zeron.sh") =>
  site.fetch(path, { method: "POST", headers: { cookie, origin, "content-type": "application/x-www-form-urlencoded" }, body: new URLSearchParams(fields).toString() });

/** The one-shot message a response set, as a cookie to send back. */
const flashOf = (response: Response) => response.headers.getSetCookie().find((c) => c.startsWith("__Host-zeron_flash="))?.split(";")[0] ?? "";

/** A version's state on its author's page, as its row's badge says it. */
const stateOf = (body: string, version: string) => new RegExp(`<span class="line version">${version.replace(/\./g, "\\.")}</span></td><td><span class="line"><span class="badge sm[^"]*">([^<]+)</span>`).exec(body)?.[1];

/** The session's CSRF token, as a page carries it in its forms. */
const tokenOf = (body: string) => /name="csrf" value="([^"]+)"/.exec(body)?.[1] ?? "";

describe("the deploy config", () => {
  test("production is the zeron.sh worker reading the production edge", () => {
    const config = JSON.parse(readFileSync(new URL("../dist/server/wrangler.json", import.meta.url), "utf8"));
    expect(config.name).toBe("comet-landing");
    expect(config.routes.map((r: { pattern: string }) => r.pattern)).toEqual(["zeron.sh", "comet.zeron.sh"]);
    expect(config.services).toEqual([{ binding: "EDGE", service: "comet-native-edge" }]);
    expect(config.assets.binding).toBe("ASSETS");
    expect(config.assets.not_found_handling).toBeUndefined();
    expect(config.cache).toEqual({ enabled: true });
  });
});

describe("the site's own pages", () => {
  test("prerendered pages are files, served without the server's headers", async () => {
    for (const path of ["/", "/privacy"]) {
      const { response, body } = await html(path);
      expect(response.status, path).toBe(200);
      expect(body).toBe(readFileSync(new URL(`../dist/client${path === "/" ? "/index" : path}.html`, import.meta.url), "utf8"));
      // (The Worker's middleware adds a CSP to what it renders; a file has none.)
      expect(response.headers.get("content-security-policy"), path).toBeNull();
    }
  });

  test("an unknown path is the site's 404 page, with 404", async () => {
    const { response, body } = await html("/no/such/page");
    expect(response.status).toBe(404);
    expect(body).toBe(readFileSync(new URL("../dist/client/404.html", import.meta.url), "utf8"));
  });

  test("robots and the sitemaps", async () => {
    expect((await html("/robots.txt")).body).toContain("Sitemap: https://zeron.sh/sitemap.xml");
    expect((await html("/sitemap.xml")).body).toContain("<loc>https://zeron.sh/plugins/sitemap.xml</loc>");
    const plugins = await html("/plugins/sitemap.xml");
    expect(plugins.response.headers.get("content-type")).toContain("application/xml");
    expect(plugins.body).toContain("<loc>https://zeron.sh/plugins/focus-timer</loc>");
    expect(plugins.body).toContain("<loc>https://zeron.sh/plugins?category=git</loc>");
    expect(plugins.body).not.toContain("voice-memos");
  });
});

describe("the store's front (/plugins)", () => {
  test("public, cached for a minute at most, no inline script, a strict CSP", async () => {
    const { response, body } = await html("/plugins");
    expect(response.status).toBe(200);
    expect(response.headers.get("cache-control")).toBe("public, max-age=30");
    expect(response.headers.get("cloudflare-cdn-cache-control")).toBe("public, max-age=30, stale-while-revalidate=30");
    const csp = response.headers.get("content-security-policy")!;
    expect(csp).toMatch(/script-src 'self';/);
    expect(csp).not.toContain("unsafe");
    expect(response.headers.get("x-frame-options")).toBe("DENY");
    expect(hasInlineScript(body)).toBe(false);
    expect(body).toContain("Focus timer");
    expect(body).toContain('id="featured-heading"');
  });

});

describe("All plugins (/plugins/all)", () => {
  /** The plugins a list's page shows, in order. */
  const listed = (body: string) => [...body.matchAll(/class="name name-link" href="\/plugins\/([^"]+)"/g)].map((m) => m[1]);

  test("public, cached for a minute at most, a strict CSP; the columns, a row a plugin", async () => {
    const { response, body } = await html("/plugins/all");
    expect(response.status).toBe(200);
    expect(response.headers.get("cache-control")).toBe("public, max-age=30");
    expect(response.headers.get("cloudflare-cdn-cache-control")).toBe("public, max-age=30, stale-while-revalidate=30");
    expect(response.headers.get("content-security-policy")).toMatch(/script-src 'self';/);
    expect(hasInlineScript(body)).toBe(false);
    inert(body);
    expect(body).toMatch(/<th scope="col">Plugin<\/th><th scope="col">By<\/th><th scope="col">Category<\/th><th scope="col" class="end">Installs<\/th>/);
    expect(listed(body)).toHaveLength(24);
    expect(body).toContain('aria-current="page"><span class="glyph g16"');
  });

  test("Next and Previous are real addresses that carry the cursor, and come back to the same list", async () => {
    const first = await html("/plugins/all");
    const next = /href="([^"]+)" rel="next"/.exec(first.body)?.[1]?.replaceAll("&amp;", "&");
    expect(next).toMatch(/^\/plugins\/all\?cursor=/);
    expect(first.body).toMatch(/<span class="btn secondary sm" aria-disabled="true">.*Previous<\/span>/);
    const second = await html(next!);
    expect(second.response.status).toBe(200);
    expect(listed(second.body)).not.toEqual(listed(first.body));
    const previous = /href="([^"]+)" rel="prev"/.exec(second.body)?.[1]?.replaceAll("&amp;", "&");
    expect(previous).toMatch(/^\/plugins\/all/);
    expect(listed((await html(previous!)).body)).toEqual(listed(first.body));
  });

  test("the last column is what the list is sorted by, its head naming it", async () => {
    const head = (body: string) => /<th scope="col" class="end">([^<]+)<\/th>/.exec(body)?.[1];
    expect(head((await html("/plugins/all")).body)).toBe("Installs");
    expect(head((await html("/plugins/all?q=timer")).body)).toBe("Installs");
    const updated = await html("/plugins/all?sort=updated");
    expect(head(updated.body)).toBe("Updated");
    expect(updated.body).toMatch(/<td class="installs end">[A-Z][a-z]{2} \d{1,2}(, \d{4})?<\/td>/);
    expect(updated.body).toMatch(/<span class="meta">@[a-z-]+ · Updated [A-Z][a-z]{2} \d/);
    // Newest: the day each was first listed (amendment 39), the newest first.
    const newest = await html("/plugins/all?sort=new");
    expect(head(newest.body)).toBe("Published");
    expect(newest.body).toMatch(/<span class="meta">@[a-z-]+ · Published [A-Z][a-z]{2} \d/);
    const days = [...newest.body.matchAll(/<td class="installs end">([^<]+)<\/td>/g)].map((m) => Date.parse(`${m[1]}${/\d{4}/.test(m[1]!) ? "" : `, ${new Date().getUTCFullYear()}`}`));
    expect(days.length).toBeGreaterThan(1);
    expect(days).toEqual([...days].sort((a, b) => b - a));
  });

  test("the pager: Previous, where you are of how many, Next", async () => {
    const first = await html("/plugins/all");
    const total = Number(/<span class="title-count"> · ([\d,]+)<\/span>/.exec(first.body)![1]!.replace(",", ""));
    expect(first.body).toContain(`<span class="where">1–24 of ${total}</span>`);
    const next = /href="([^"]+)" rel="next"/.exec(first.body)![1]!.replaceAll("&amp;", "&");
    expect(next).toMatch(/&page=2$/);
    expect((await html(next)).body).toMatch(new RegExp(`<span class="where">25–\\d+ of ${total}</span>`));
  });

  test("the sort is a row of real addresses, the chosen one marked; a search adds Best match", async () => {
    const { body } = await html("/plugins/all");
    expect(body).toMatch(/<a href="\/plugins\/all" aria-current="page">Most installed<\/a><a href="\/plugins\/all\?sort=updated">Recently updated<\/a><a href="\/plugins\/all\?sort=new">Newest<\/a>/);
    const search = await html("/plugins/all?q=timer");
    expect(search.body).toMatch(/aria-current="page">Best match<\/a>/);
  });

  test("search, category, keyword; the count when the list says it exactly", async () => {
    const timer = await html("/plugins/all?q=timer");
    expect(listed(timer.body)).toContain("focus-timer");
    expect(timer.body).toContain("Results for “timer”");
    // The title and the count, always (amendment 38's total).
    expect(timer.body).toMatch(/Results for “timer”<span class="title-count"> · \d+<\/span><\/h1>/);
    // (The field keeps the words.)
    expect(timer.body).toContain('name="q" value="timer"');
    const empty = await html("/plugins/all?q=zzzzqqq");
    expect(empty.body).toContain("Nothing matches “zzzzqqq”");
    expect(empty.body).toContain('href="/plugins/all">Clear search</a>');
    expect(empty.body).toContain('<span class="title-count"> · 0</span>');
    const git = await html("/plugins/all?category=git");
    expect(listed(git.body)).toContain("pr-radar");
    expect(listed(git.body)).not.toContain("token-meter");
    expect(git.body).toContain('<link rel="canonical" href="https://zeron.sh/plugins/all?category=git">');
    expect(git.body).toMatch(/<a class="nav-row" href="\/plugins\/all\?category=git" aria-current="page">/);
    expect(listed((await html("/plugins/all?keyword=github")).body)).toContain("pr-radar");
  });

  test("an address that says more than it means is sent to the plain one; /plugins?… is All plugins'", async () => {
    for (const [asked, plain] of [["/plugins/all?sort=popular&utm_source=x", "/plugins/all"], ["/plugins?q=timer", "/plugins/all?q=timer"], ["/plugins?category=git", "/plugins/all?category=git"]]) {
      const response = await site.fetch(asked!);
      expect(response.status, asked).toBe(301);
      expect(response.headers.get("location"), asked).toBe(plain);
    }
  });

  test("search results stay out of search engines; the list and its categories don't", async () => {
    expect((await html("/plugins/all?q=timer")).body).toContain('<meta name="robots" content="noindex, nofollow">');
    expect((await html("/plugins/all")).body).not.toContain('name="robots"');
    expect((await html("/plugins/all?category=git")).body).not.toContain('name="robots"');
  });
});

describe("a plugin's page", () => {
  test("its README, what it asks for, Install in Zeron", async () => {
    const { response, body } = await html("/plugins/pr-radar");
    expect(response.status).toBe(200);
    expect(response.headers.get("cloudflare-cdn-cache-control")).toBe("public, max-age=30, stale-while-revalidate=30");
    expect(body).toContain('href="zeron://open/plugin/pr-radar"');
    expect(body).toContain("Run gh, git on your computer as you");
    expect(body).toContain("Install it only if you trust @kai.");
    expect(body).toContain('<h3 id="user-content-what-it-does">What it does</h3>');
    expect(body).toContain('"@type":"SoftwareApplication"');
    expect(hasInlineScript(body)).toBe(false);
  });

  test("the page's rules: one primary button, pictures by their number, the README under the page, its parts as tabs", async () => {
    const { body } = await html("/plugins/focus-timer");
    // One action slot, one style: Install the page's one primary button; what else is said, in words.
    expect(body.match(/class="btn primary/g)).toHaveLength(1);
    expect(body).toContain("<summary>Nothing happened?</summary>");
    // Two pictures share the column.
    expect(body).toContain('class="strip two"');
    expect((await html("/plugins/pr-radar")).body).toMatch(/class="strip (one|two|many)"/);
    // The README doesn't say the name again, and nothing in it outweighs the title.
    expect(body.match(/<h[1-6][^>]*>Focus timer<\/h[1-6]>/g)).toEqual(['<h1 class="t-title">Focus timer</h1>']);
    expect(body).not.toMatch(/<div class="prose"[^>]*>[^]*?<h1/);
    // Its parts are tabs that are addresses; Report is always in the header.
    expect(body).toMatch(/<a href="#overview" aria-current="page">Overview<\/a><a href="#changelog">Changelog <span class="count">\d+<\/span><\/a><a href="#permissions">Permissions <span class="count">\d+<\/span><\/a>/);
    expect(body).toMatch(/<a slot="tools"|class="tools"><a class="btn ghost sm" href="\/plugins\/focus-timer\/report">/);
    // Older versions are rows that open in place; the latest's notes are there to clamp.
    expect(body).toContain("data-clamp");
    expect(body).toMatch(/<details><summary><span class="num">1\.1\.0<\/span>/);
    // A risky ask, in the rail and the tab, in the danger's ink.
    expect((await html("/plugins/pr-radar")).body).toMatch(/<li class="asks-line risky">/);
  });

  test("a hostile plugin's words are only words", async () => {
    const { body } = await html("/plugins/xss-probe");
    // (Tags only: an attribute's value may hold "<script>" as inert text.)
    const markup = body.replace(/="[^"]*"/g, '=""');
    const scripts = [...markup.matchAll(/<script\b[^>]*>/g)].map((m) => m[0]);
    expect(scripts).toEqual(['<script type="">', '<script type="" src="">', '<script type="" src="">', '<script type="" src="">']);
    expect(body).toContain('<script type="application/ld+json">');
    expect(markup).not.toMatch(/<img[^>]+onerror/i);
    expect(markup).not.toMatch(/<svg[^>]+onload/i);
    expect(body).not.toMatch(/href="(javascript|data|vbscript):/i);
    expect(body).not.toMatch(/<(iframe|form action="https)/i);
    expect(body).toContain("&lt;img src=x onerror=&quot;alert(&#39;name&#39;)&quot;&gt; Probe");
    // The structured data can't close its own script.
    const ld = /<script type="application\/ld\+json">([^]*?)<\/script>/.exec(body)![1]!;
    expect(ld).not.toContain("</");
    expect(JSON.parse(ld).name).toContain("<img");
  });

  test("a name with a right-to-left override can't turn the page's words around", async () => {
    const { body } = await html("/plugins/xss-probe");
    const title = /<title>([^<]*)<\/title>/.exec(body)![1]!;
    expect(title).not.toMatch(/[\u202a-\u202e\u2066-\u2069]/);
    expect(title).toContain("a plugin for Zeron");
    expect(/<meta name="description" content="([^"]*)"/.exec(body)![1]).not.toMatch(/[\u202a-\u202e\u2066-\u2069]/);
    // (In the page, it stays as written, in an element of its own.)
    expect(body).toMatch(/<h1[^>]*>[^<]*\u202eevil<\/h1>/);
  });

  test("an author's links are followed only as parsed https addresses", async () => {
    const { body } = await html("/plugins/focus-timer");
    expect(body).toContain('href="https://example.com/focus-timer" rel="nofollow noopener ugc"');
    expect(body).toContain('href="https://github.com/ada/focus-timer" rel="nofollow noopener ugc"');
  });

  test("unknown, unlisted and malformed ids are one 404, in the shell, saying nothing of why", async () => {
    const pages = await Promise.all(["/plugins/no-such-plugin", "/plugins/voice-memos", "/plugins/crypto-ticker", "/plugins/Bad_Id", "/plugins/a%2Fb"].map(async (path) => ({ path, ...(await html(path)) })));
    for (const { path, response, body } of pages) {
      expect(response.status, path).toBe(404);
      expect(body, path).toContain("No plugin is listed here");
      expect(response.headers.get("x-robots-tag") ?? body, path).toMatch(/noindex/);
    }
    // (Each the same but for its own address: its canonical, and where sign-in comes back to.)
    const same = (body: string) => body.replace(/(rel="canonical" href|property="og:url" content)="[^"]*"/g, "").replace(/return_to=[^"]*/g, "");
    expect(new Set(pages.map((p) => same(p.body))).size).toBe(1);
  });

  test("a query on a plugin's page goes to its one address", async () => {
    const response = await site.fetch("/plugins/focus-timer?x=1");
    expect(response.status).toBe(301);
    expect(response.headers.get("location")).toBe("/plugins/focus-timer");
  });
});

describe("package files", () => {
  test("a listed version's picture: typed by its name, sandboxed, a minute in shared caches", async () => {
    const response = await site.fetch("/plugins/focus-timer/files/1.2.0/icon.svg");
    expect(response.status).toBe(200);
    expect(response.headers.get("content-type")).toBe("image/svg+xml");
    expect(response.headers.get("content-security-policy")).toContain("sandbox");
    expect(response.headers.get("x-content-type-options")).toBe("nosniff");
    expect(response.headers.get("cloudflare-cdn-cache-control")).toBe("public, max-age=60");
    expect(response.headers.get("cache-control")).toBe("public, max-age=60");
  });

  test("pictures that aren't what they say are still only pictures: typed by name, sandboxed, never sniffed", async () => {
    for (const [file, type] of [["script.svg", "image/svg+xml"]]) {
      const response = await site.fetch(`/plugins/theme-sync/files/1.0.0/shots/${file}`);
      expect(response.status, file).toBe(200);
      expect(response.headers.get("content-type"), file).toBe(type);
      expect(response.headers.get("x-content-type-options"), file).toBe("nosniff");
      expect(response.headers.get("content-disposition"), file).toBe("inline");
      const csp = response.headers.get("content-security-policy") ?? "";
      expect(csp, file).toContain("sandbox");
      expect(csp, file).toContain("default-src 'none'");
      expect(csp, file).not.toContain("script");
      expect(response.headers.get("cross-origin-resource-policy"), file).toBe("same-origin");
    }
    // A file the edge gives only as a download (an HTML page named .png) is never shown inline from here,
    // and every answer of this route, a refusal too, carries the sandbox.
    const attached = await site.fetch("/plugins/theme-sync/files/1.0.0/shots/page.png");
    expect(attached.status).toBe(404);
    expect(await attached.text()).toBe("");
    expect(attached.headers.get("content-security-policy")).toContain("sandbox");
    expect(attached.headers.get("x-content-type-options")).toBe("nosniff");
    // Anything else in a package is never served from here, whatever it's called.
    for (const file of ["polyglot.gif", "page.html", "x.js", "x.htm", "x.xml", "x.svgz", "x.json", "x.webp", "x"]) {
      expect((await site.fetch(`/plugins/theme-sync/files/1.0.0/shots/${file}`)).status, file).toBe(404);
    }
  });

  test("a path with spaces, # and % is a file's, encoded part by part", async () => {
    const response = await site.fetch(fileUrl("xss-probe", "1.0.0", "shots/a b#1 100%.svg"));
    expect(response.status).toBe(200);
    expect(response.headers.get("content-type")).toBe("image/svg+xml");
  });

  test("one address per file, page and sitemap: a query or a doubled slash goes to the plain one", async () => {
    for (const [asked, plain] of [["/plugins/focus-timer/files/1.2.0/icon.svg?v=2", "/plugins/focus-timer/files/1.2.0/icon.svg"], ["/plugins/sitemap.xml?n=1", "/plugins/sitemap.xml"], ["/plugins//focus-timer", "/plugins/focus-timer"], ["https://zeron.sh//plugins/focus-timer", "/plugins/focus-timer"]]) {
      const response = await site.fetch(asked);
      expect(response.status, asked).toBe(301);
      expect(response.headers.get("location"), asked).toBe(plain);
    }
  });

  test("a name that is an encoded dot is a name, not a climb: the edge's 404, sandboxed", async () => {
    const response = await site.fetch("/plugins/focus-timer/files/1.2.0/%252e%252e/icon.svg");
    expect(response.status).toBe(404);
    expect(response.headers.get("content-security-policy")).toContain("sandbox");
    expect(response.headers.get("x-content-type-options")).toBe("nosniff");
  });

  test("only pictures pass, and no path climbs out", async () => {
    for (const path of ["/plugins/focus-timer/files/1.2.0/init.luau", "/plugins/focus-timer/files/1.2.0/README.md", "/plugins/focus-timer/files/1.2.0/..%2f..%2fsecret.png", "/plugins/focus-timer/files/v1/icon.svg"]) {
      expect((await site.fetch(path)).status, path).toBe(404);
    }
  });

  test("a version in review: nobody's but its author's and the admins', and kept by no one", async () => {
    const path = "/plugins/focus-timer/files/1.3.0-beta.1/icon.svg";
    expect((await site.fetch(path)).status).toBe(404);
    expect((await site.fetch(path, { headers: { cookie: await site.signIn("user_kai") } })).status).toBe(404);
    for (const user of ["user_ada", "user_grace"]) {
      const response = await site.fetch(path, { headers: { cookie: await site.signIn(user) } });
      expect(response.status, user).toBe(200);
      expect(response.headers.get("cache-control"), user).toBe("private, no-store");
      expect(response.headers.get("cloudflare-cdn-cache-control") ?? "", user).not.toMatch(/public/);
    }
  });
});

describe("public pages are the same for everyone (so a shared cache can keep them)", () => {
  test("signed in or not, admin or not, the bytes are the same", async () => {
    const people = [undefined, await site.signIn("user_ada"), await site.signIn("user_grace")];
    for (const path of ["/plugins", "/plugins?category=git", "/plugins/focus-timer", "/plugins/session-badges", "/plugins/sitemap.xml"]) {
      const bodies = await Promise.all(people.map(async (cookie) => (await html(path, cookie)).body));
      expect(new Set(bodies).size, path).toBe(1);
      const csp = await Promise.all(people.map(async (cookie) => (await site.fetch(path, cookie ? { headers: { cookie } } : {})).headers.get("set-cookie")));
      expect(csp.every((c) => c === null), `${path} sets no cookie`).toBe(true);
    }
  });
});

describe("signed-in pages", () => {
  test("signed out, they send you to sign in and back (a path here, never anywhere else)", async () => {
    for (const path of ["/account", "/account/plugins/focus-timer", "/plugins/focus-timer/report"]) {
      const response = await site.fetch(path);
      expect(response.status, path).toBe(303);
      expect(response.headers.get("location"), path).toBe(`/auth/web/login?return_to=${encodeURIComponent(path)}`);
    }
  });

  test("yours: never cached, never indexed", async () => {
    const cookie = await site.signIn("user_ada");
    const { response, body } = await html("/account", cookie);
    expect(response.status).toBe(200);
    expect(response.headers.get("cache-control")).toBe("private, no-store");
    expect(response.headers.get("x-robots-tag")).toBe("noindex, nofollow");
    expect(response.headers.get("cloudflare-cdn-cache-control") ?? "").not.toMatch(/public/);
    expect(body).toContain("Session badges");
    expect(hasInlineScript(body)).toBe(false);
  });

  test("a session the edge rotates goes back to the browser, and that page is kept by no one", async () => {
    // (The edge rotates a staff session as it's used.)
    const { response } = await html("/admin/plugins", await site.signIn("user_grace"));
    expect(response.headers.getSetCookie().join("\n")).toMatch(/__Host-zeron_session=[\w-]+; Path=\/; Secure; HttpOnly; SameSite=Lax/);
    expect(response.headers.get("cache-control")).toBe("private, no-store");
    expect(response.headers.get("cloudflare-cdn-cache-control") ?? "").not.toMatch(/public/);
  });

  test("its place in the store, in three words: Listed, plain; Not listed yet, never was; Not listed, was and isn't now", async () => {
    const { body } = await html("/account", await site.signIn("user_ada"));
    const cell = (id: string) => body.slice(body.indexOf(`<span class="id">${id}</span>`)).match(/<td class="listing">(.*?)<\/td>/)![1]!;
    expect(cell("focus-timer")).toBe('<span class="steady">Listed</span>');
    expect(cell("rough-draft")).toContain(">Not listed yet<");
    expect(cell("first-light")).toContain(">Not listed yet<");
    expect(cell("clipboard-sync")).toContain(">Not listed<");
    expect(cell("old-sketch")).toContain(">Not listed<");
  });

  test("an id with nothing listed or in review says it goes back after 90 days", async () => {
    const kai = await site.signIn("user_kai");
    expect((await html("/account/plugins/old-idea", kai)).body).toContain("Its id stays yours for 90 days after your last submission");
    expect((await html("/account/plugins/pr-radar", kai)).body).not.toContain("90 days");
  });

  test("someone else's plugin is the 404 page", async () => {
    const { response } = await html("/account/plugins/focus-timer", await site.signIn("user_kai"));
    expect(response.status).toBe(404);
  });
});

describe("who is signed in, for a public page's list (account/me.json)", () => {
  test("always 200, never kept; signed out, a member, staff", async () => {
    const answers: [string | undefined, Record<string, unknown>][] = [
      [undefined, { signed_in: false }],
      [await site.signIn("user_ada"), { signed_in: true, name: "@ada", staff: false, csrf: expect.any(String) }],
      [await site.signIn("user_grace"), { signed_in: true, name: "@grace", staff: true, csrf: expect.any(String) }],
    ];
    for (const [cookie, expected] of answers) {
      const response = await site.fetch("/account/me.json", cookie ? { headers: { cookie } } : {});
      expect(response.status).toBe(200);
      expect(response.headers.get("cache-control")).toBe("private, no-store");
      expect(await response.json()).toEqual(expected);
    }
  });
});

describe("the admin pages can't be told from pages that don't exist", () => {
  const PATHS = ["/admin/reservations", "/admin/reservations/linear", "/admin/plugins", "/admin/plugins/session-badges", "/admin/plugins/session-badges/1.2.1", "/admin/plugins/session-badges/1.2.1/approve", "/admin/plugins/session-badges/actions/suspend", "/admin/reports", "/admin/audit"];

  test("signed out, or signed in without the role: the site's 404, byte for byte", async () => {
    const notFound = readFileSync(new URL("../dist/client/404.html", import.meta.url), "utf8");
    const unknown = await site.fetch("/admin/nothing-here");
    const ada = await site.signIn("user_ada");
    for (const path of PATHS) {
      for (const cookie of [undefined, ada]) {
        const { response, body } = await html(path, cookie);
        expect(response.status, `${path} ${cookie ? "ada" : "anyone"}`).toBe(404);
        expect(body, path).toBe(notFound);
        // Every header, not only the body: nothing says "this one exists".
        const headersOf = (r: Response) => JSON.stringify([...r.headers].filter(([k]) => k !== "date" && k !== "cf-cache-status"));
        expect(headersOf(response), path).toBe(headersOf(unknown));
        expect(response.headers.get("location"), path).toBeNull();
      }
    }
  });

  test("an admin sees them, never cached, never indexed", async () => {
    const grace = await site.signIn("user_grace");
    for (const path of PATHS) {
      const { response } = await html(path, grace);
      expect(response.status, path).toBe(200);
      expect(response.headers.get("cache-control"), path).toBe("private, no-store");
      expect(response.headers.get("x-robots-tag"), path).toBe("noindex, nofollow");
    }
  });

  /** A response's every header but the clock's. */
  const headersOf = (r: Response) => JSON.stringify([...r.headers].filter(([k]) => k !== "date" && k !== "cf-cache-status"));
  /** Pages that are there, beside addresses that aren't (an action no page has among them). */
  const PAIRS: [string, string][] = [
    ["/admin/reservations", "/admin/nothing-here"],
    ["/admin/plugins/session-badges/1.2.1/approve", "/admin/plugins/session-badges/1.2.1/bogus"],
    ["/admin/plugins/session-badges/actions/suspend", "/admin/plugins/session-badges/actions/bogus"],
    ["/admin/reports/1", "/admin/reports/1/more"],
  ];

  test("a form from another site: the same 404 whether a page is there or not", async () => {
    const notFound = readFileSync(new URL("../dist/client/404.html", import.meta.url), "utf8");
    for (const pair of PAIRS) {
      const answers = await Promise.all(pair.map((path) => post(path, "", { csrf: "x" }, "https://evil.example")));
      for (const [i, response] of answers.entries()) {
        expect(response.status, pair[i]).toBe(404);
        expect(await response.text(), pair[i]).toBe(notFound);
      }
      expect(headersOf(answers[0]!), pair[0]).toBe(headersOf(answers[1]!));
    }
  });

  test("a form posted with a session that ended: the same trip to sign in, whether a page is there or not", async () => {
    for (const pair of PAIRS) {
      const answers = await Promise.all(pair.map((path) => post(path, "__Host-zeron_session=ended", { csrf: "x", reason: "Looks wrong" })));
      for (const [i, response] of answers.entries()) {
        expect(response.status, pair[i]).toBe(303);
        // (What was typed comes back; the stale token doesn't.)
        expect(response.headers.get("location"), pair[i]).toBe(`/auth/web/login?${new URLSearchParams({ return_to: `${pair[i]}?reason=Looks+wrong` })}`);
      }
      expect(headersOf(answers[0]!).replace(/return_to=[^"]*/, ""), pair[0]).toBe(headersOf(answers[1]!).replace(/return_to=[^"]*/, ""));
    }
  });

  test("a doubled slash: the 404 whether a page is there or not; an admin is sent on", async () => {
    const unknown = await site.fetch("/admin/nothing-here");
    for (const path of ["/admin//plugins", "/admin//nothing-here", "//admin/audit", "/admin"]) {
      const response = await site.fetch(path);
      expect(response.status, path).toBe(404);
      expect(headersOf(response), path).toBe(headersOf(unknown));
    }
    const grace = await site.signIn("user_grace");
    const sent = await site.fetch("/admin//plugins", { headers: { cookie: grace } });
    expect(sent.status).toBe(301);
    expect(sent.headers.get("location")).toBe("/admin/plugins");
  });

  test("an admin: a form from another site is refused; an address no page has is the 404", async () => {
    const grace = await site.signIn("user_grace");
    expect((await post("/admin/reservations", grace, { csrf: "x", id: "a-b" }, "https://evil.example")).status).toBe(403);
    expect((await site.fetch("/admin/nothing-here", { headers: { cookie: grace } })).status).toBe(404);
  });

  test("a report closed after signing in again comes back to its row, the note as typed", async () => {
    const grace = await site.signIn("user_grace");
    const back = await site.fetch("/admin/reports/1?note=Checked+it", { headers: { cookie: grace } });
    expect(back.status).toBe(303);
    expect(back.headers.get("location")).toBe("/admin/reports?report=1&note=Checked+it");
  });

  test("a post from a non-admin does nothing and says nothing", async () => {
    const ada = await site.signIn("user_ada");
    const page = await html("/account", ada);
    const response = await post("/admin/plugins/voice-memos/0.1.0/approve", ada, { csrf: tokenOf(page.body) });
    expect(response.status).toBe(404);
    expect((await html("/plugins/voice-memos")).response.status).toBe(404);
  });
});

describe("forms", () => {
  test("a post from another site is refused before anything runs", async () => {
    const ada = await site.signIn("user_ada");
    const page = await html("/account/plugins/session-badges", ada);
    const response = await post("/account/plugins/session-badges/versions/1.2.1/withdraw", ada, { csrf: tokenOf(page.body) }, "https://evil.example");
    expect(response.status).toBe(403);
    expect(stateOf((await html("/account/plugins/session-badges", ada)).body, "1.2.1")).toBe("In review");
  });

  test("a post without the session's token changes nothing", async () => {
    const ada = await site.signIn("user_ada");
    const response = await post("/account/plugins/session-badges/versions/1.2.1/withdraw", ada, { csrf: "not-the-token" });
    expect(response.status).toBe(303);
    const after = await html("/account/plugins/session-badges", `${ada}; ${flashOf(response)}`);
    expect(after.body).toContain("The page was open too long. Try again.");
    expect(stateOf(after.body, "1.2.1")).toBe("In review");
  });

  test("a confirm page names the plugin in the app's words; someone else's plugin is the 404", async () => {
    const ada = await site.signIn("user_ada");
    const ask = await html("/account/plugins/focus-timer/versions/1.2.0/yank", ada);
    expect(ask.response.status).toBe(200);
    expect(ask.body).toContain("Stop new installs of Focus timer 1.2.0?");
    expect(ask.body).toContain("Those who have it keep it working.");
    const kai = await site.signIn("user_kai");
    expect((await html("/account/plugins/focus-timer/versions/1.2.0/yank", kai)).response.status).toBe(404);
  });

  test("an author with nothing sent is told how to send the first", async () => {
    const { body } = await html("/account", await site.signIn("user_noor"));
    expect(body).toContain("You haven't published a plugin yet");
    expect(body).toContain("<code>zeron plugin publish</code>");
    expect(body).toMatch(/Your plugins<span class="title-count"> · 0<\/span>/);
  });

  test("one plugin: a version's one action in its column; six states, six tones; keys short, the gone ones apart", async () => {
    const { body } = await html("/account/plugins/worst-case-listing-at-every-limit-xyz12", await site.signIn("user_ada"));
    const states = Object.fromEntries(["1.5.0", "1.4.0", "1.3.0", "1.2.0", "1.1.0", "1.0.0"].map((v) => [v, stateOf(body, v)]));
    expect(states).toEqual({ "1.5.0": "In review", "1.4.0": "Not approved", "1.3.0": "Withdrawn", "1.2.0": "Taken down", "1.1.0": "No new installs", "1.0.0": "Approved" });
    // Each state's tone its own.
    const tones = [...body.matchAll(/<span class="line"><span class="badge sm ?([a-z]*)">/g)].map((m) => m[1]);
    expect(new Set(tones).size).toBe(6);
    // Exactly one action where one can be taken, none where none can.
    const actions = [...body.matchAll(/data-dialog="(withdraw|yank|unyank)-([\d_]+)"/g)].map((m) => `${m[1]} ${m[2]}`);
    expect(actions).toEqual(["withdraw 1_5_0", "unyank 1_1_0", "yank 1_0_0"]);
    // A reviewer's words in full, never cut.
    expect(body).toContain("Until then it can&#39;t be listed, and nobody else can install it.”");
    // Keys: seven working, three gone in their group with their count, each shown the one short way.
    expect(body.match(/data-dialog="revoke-/g)).toHaveLength(7);
    expect(body).toMatch(/<details class="gone"><summary>Revoked keys · 3/);
    for (const m of body.matchAll(/<span class="line key" title="([^"]+)">([^<]+)<\/span>/g)) expect(m[2]).toBe(`${m[1]!.slice(0, 6)}…${m[1]!.slice(-4)}`);
    // Ending within 14 days: the warning's ink and its days in words (three of them), never colour alone.
    const soon = [...body.matchAll(/<span class="line soon" title="Ends in (\d+) days?">.*?<span class="sr-only">, Ends in \1 days?<\/span>/g)];
    expect(soon).toHaveLength(3);
    // Its parts are tabs that are addresses; New key… opens a sheet that is one too.
    expect(body).toMatch(/<a href="#versions" aria-current="page">Versions <span class="count">6<\/span><\/a><a href="#keys">Deploy keys <span class="count">7<\/span><\/a>/);
    expect(body).toContain('<dialog class="sheet" id="new-key"');
    expect(body).toContain("<code>zeron plugin key create worst-case-listing-at-every-limit-xyz12</code>");
  });

  test("a confirm page asks first; the post does it once and says so once", async () => {
    const ada = await site.signIn("user_ada");
    const ask = await html("/account/plugins/session-badges/versions/1.2.1/withdraw", ada);
    expect(ask.response.status).toBe(200);
    expect(ask.body).toContain("Withdraw Session badges 1.2.1?");
    const response = await post("/account/plugins/session-badges/versions/1.2.1/withdraw", ada, { csrf: tokenOf(ask.body) });
    expect(response.status).toBe(303);
    expect(response.headers.get("location")).toBe("/account/plugins/session-badges");
    expect(response.headers.getSetCookie().find((c) => c.startsWith("__Host-zeron_flash="))).toMatch(/^__Host-zeron_flash=withdrawn; .*Secure/);
    const cookie = `${ada}; ${flashOf(response)}`;
    const after = await html("/account/plugins/session-badges", cookie);
    expect(after.body).toContain("Withdrawn from review.");
    expect(stateOf(after.body, "1.2.1")).toBe("Withdrawn");
    // (Said once: the page that shows it clears it.)
    expect(after.response.headers.getSetCookie().join("\n")).toMatch(/__Host-zeron_flash=[^;]*;.*(Max-Age=0|Expires=Thu, 01 Jan 1970)/);
  });

  test("an admin approves a first submission and it's listed", async () => {
    const grace = await site.signIn("user_grace");
    expect((await html("/plugins/voice-memos")).response.status).toBe(404);
    const page = await html("/admin/plugins/voice-memos/0.1.0", grace);
    expect(page.body).toContain("First listing");
    const response = await post("/admin/plugins/voice-memos/0.1.0/approve", grace, { csrf: tokenOf(page.body) });
    expect(response.status).toBe(303);
    expect(response.headers.get("set-cookie")).toMatch(/zeron_flash=approved/);
    expect((await html("/plugins/voice-memos")).response.status).toBe(200);
    expect((await html("/admin/audit", grace)).body).toMatch(/Approved\s*<a href="\/admin\/plugins\/voice-memos\/0\.1\.0"/);
  });

  test("an admin whose session ended keeps what they typed through signing in again", async () => {
    const expired = "__Host-zeron_session=ended-long-ago";
    const fields = { csrf: "stale", reason: "listing", message: "Say what <it> does, & why." };
    const response = await post("/admin/plugins/focus-timer/1.3.0-beta.1/reject", expired, fields);
    expect(response.status).toBe(303);
    const location = new URL(response.headers.get("location")!, "https://zeron.sh");
    expect(location.pathname).toBe("/auth/web/login");
    const back = location.searchParams.get("return_to")!;
    expect(back).toBe("/admin/plugins/focus-timer/1.3.0-beta.1/reject?reason=listing&message=Say+what+%3Cit%3E+does%2C+%26+why.");
    // Signed in again, the question comes back with the answer as typed (and nothing was done meanwhile).
    const page = await html(back, await site.signIn("user_grace"));
    expect(page.body).toContain(">Say what &lt;it&gt; does, &amp; why.</textarea>");
    expect(page.body).toMatch(/<option value="listing" selected/);
    expect((await html("/admin/plugins/focus-timer/1.3.0-beta.1", await site.signIn("user_grace"))).body).toContain("In review");
  });

  test("a non-admin's post, signed in or not, is the 404 page and nothing more", async () => {
    const notFound = readFileSync(new URL("../dist/client/404.html", import.meta.url), "utf8");
    for (const cookie of [await site.signIn("user_ada"), "theme=dark"]) {
      for (const path of ["/admin/plugins/focus-timer/1.3.0-beta.1/reject", "/admin/reports/rep_1", "/admin/reservations"]) {
        const response = await post(path, cookie, { csrf: "x", reason: "listing", message: "m", resolution: "dismissed", id: "abc" });
        expect(response.status, path).toBe(404);
        expect(response.headers.get("location"), path).toBeNull();
        expect(await response.text(), path).toBe(notFound);
      }
    }
  });

  test("what an author wrote shows as words on every admin view: tools, notes, file names, contents, diffs", async () => {
    const grace = await site.signIn("user_grace");
    const here = "/admin/plugins/xss-probe/1.1.0";
    const overview = await html(here, grace);
    inert(overview.body);
    expect(overview.body).toContain("&lt;/p&gt;&lt;script&gt;alert(&#39;tool&#39;)&lt;/script&gt;&lt;img src=x onerror=alert(&#39;tool&#39;)&gt;");
    expect(overview.body).toContain("Notes for 1.1.0");
    const files = await html(`${here}?view=files&path=${encodeURIComponent("docs/<b>notes</b> #1 100%.md")}`, grace);
    inert(files.body);
    expect(files.body).toContain("&lt;b&gt;notes&lt;/b&gt; #1 100%.md");
    // (The file's text, highlighted: its words are there, as text.)
    expect(files.body).toContain("alert('file')");
    const changes = await html(`${here}?view=changes`, grace);
    inert(changes.body);
    expect(changes.body).toMatch(/&#x3C;\/code>&#x3C;\/pre>&#x3C;script>alert\('diff'\)|&lt;\/code&gt;&lt;\/pre&gt;&lt;script&gt;alert\(&#39;diff&#39;\)/);
    const readme = await html(`${here}?view=readme`, grace);
    inert(readme.body);
    // The public page's notes too.
    inert((await html("/plugins/xss-probe")).body);
  });

  test("no file can hide from the reviewer: each is shown as itself, or said to be missing, never another in its place", async () => {
    const grace = await site.signIn("user_grace");
    const here = "/admin/plugins/xss-probe/1.1.0";
    const view = (path: string) => html(`${here}?view=files&path=${encodeURIComponent(path)}`, grace);
    const long = await view(`src/${"x".repeat(600)}.lua`);
    expect(long.response.status).toBe(200);
    expect(long.body).toContain("the long one");
    const dotted = await view("lib/%2e%2e/x.lua");
    expect(dotted.response.status).toBe(200);
    expect(dotted.body).toContain("the dotted one");
    const missing = await view("no/such/file.lua");
    expect(missing.body).toContain("This version has no file at that path.");
    expect(missing.body).not.toMatch(/"entry"|&quot;entry&quot;/);
    // An SVG is shown as a picture and as its source: what it draws, and what runs.
    const svg = await view("shots/a b#1 100%.svg");
    expect(svg.body).toContain(`src="${fileUrl("xss-probe", "1.1.0", "shots/a b#1 100%.svg")}"`);
    expect(svg.body).toMatch(/rect/);
    expect(svg.body).toContain("9277b0");
    // Every file of the version, changed or not, can be compared.
    expect((await html(`${here}?view=changes`, grace)).response.status).toBe(200);
  });

  test("a decision's page says what is decided, and says so when the visit came from elsewhere", async () => {
    const grace = await site.signIn("user_grace");
    const plain = await html("/admin/plugins/session-badges/1.2.1/approve", grace);
    expect(plain.body).toContain("Session badges");
    expect(plain.body).toContain("Update from");
    expect(plain.body).toContain("Read it first");
    expect(plain.body).not.toContain("You followed a link from somewhere else");
    const lured = await site.fetch("/admin/plugins/session-badges/1.2.1/approve", { headers: { cookie: grace, "sec-fetch-site": "cross-site" } });
    expect(await lured.text()).toContain("You followed a link from somewhere else");
  });

  test("reserving an id for an account, and freeing it", async () => {
    const grace = await site.signIn("user_grace");
    const page = await html("/admin/reservations", grace);
    expect(page.body).toContain("zeron-tools");
    const csrf = tokenOf(page.body);
    const reserved = await post("/admin/reservations", grace, { csrf, id: "Kai-Tools", to_username: "@kai", note: "Asked by email." });
    expect(flashOf(reserved)).toBe("__Host-zeron_flash=reserved");
    expect((await html("/admin/reservations", grace)).body).toMatch(/<code[^>]*>kai-tools<\/code>[^]*?@kai[^]*?Asked by email\./);
    // An id in use moves by a transfer, and the page says so.
    expect(flashOf(await post("/admin/reservations", grace, { csrf, id: "focus-timer" }))).toBe("__Host-zeron_flash=id_in_use");
    expect(flashOf(await post("/admin/reservations", grace, { csrf, id: "x" }))).toBe("__Host-zeron_flash=bad_input");
    // The site's own page words: nobody can be granted one, and the page says why, beside the form too.
    for (const fields of [{ id: "all" }, { id: "publishing", to_username: "@kai" }] as Record<string, string>[]) {
      expect(flashOf(await post("/admin/reservations", grace, { csrf, ...fields }))).toBe("__Host-zeron_flash=site_id");
    }
    const after = await html("/admin/reservations", grace);
    expect(after.body).not.toMatch(/<code[^>]*>(all|publishing)<\/code>/);
    expect(after.body).toContain("The site's own page words can't be reserved: no plugin can have them.");
    expect(flashOf(await post("/admin/reservations/kai-tools", grace, { csrf }))).toBe("__Host-zeron_flash=reservation_released");
    expect((await html("/admin/reservations", grace)).body).not.toContain("kai-tools");
    expect((await html("/admin/audit", grace)).body).toMatch(/Freed\s*<a href="\/admin\/reservations"/);
  });

  test("rejecting needs a reason and words for the author", async () => {
    const grace = await site.signIn("user_grace");
    const page = await html("/admin/plugins/focus-timer/1.3.0-beta.1", grace);
    const csrf = tokenOf(page.body);
    const missing = await post("/admin/plugins/focus-timer/1.3.0-beta.1/reject", grace, { csrf, reason: "listing" });
    expect(missing.headers.get("set-cookie")).toMatch(/zeron_flash=bad_input/);
    const done = await post("/admin/plugins/focus-timer/1.3.0-beta.1/reject", grace, { csrf, reason: "listing", message: "Say what <b>it</b> does." });
    expect(done.headers.get("set-cookie")).toMatch(/zeron_flash=rejected/);
    const author = await html("/account/plugins/focus-timer", await site.signIn("user_ada"));
    expect(author.body).toContain("Say what &lt;b&gt;it&lt;/b&gt; does.");
  });

  test("a report needs a known reason, reaches the reviewers, and thanks you", async () => {
    const kai = await site.signIn("user_kai");
    const page = await html("/plugins/theme-sync/report", kai);
    expect(page.response.headers.get("cache-control")).toBe("private, no-store");
    const csrf = tokenOf(page.body);
    expect((await post("/plugins/theme-sync/report", kai, { csrf, reason: "bogus", text: "x" })).headers.get("set-cookie")).toMatch(/bad_input/);
    const sent = await post("/plugins/theme-sync/report", kai, { csrf, reason: "broken", text: "It <script>breaks</script> the sidebar." });
    expect((await html("/plugins/theme-sync/report", `${kai}; ${flashOf(sent)}`)).body).toContain("Thanks for telling us");
    const reports = await html("/admin/reports", await site.signIn("user_grace"));
    expect(reports.body).toContain("It &lt;script&gt;breaks&lt;/script&gt; the sidebar.");
  });

  test("signing out without the token is refused in the store's word, and the session lives", async () => {
    const lin = await site.signIn("user_lin");
    const refused = await post("/auth/web/logout", lin, {});
    expect(refused.status).toBe(403);
    const said = (await refused.json()) as Record<string, unknown>;
    expect(said).toMatchObject({ error: "bad_csrf", message: expect.any(String) });
    expect(said).not.toHaveProperty("reason");
    expect((await site.fetch("/account", { headers: { cookie: lin } })).status).toBe(200);
  });

  test("signing out is a form post with the token; the session ends", async () => {
    const lin = await site.signIn("user_lin");
    const page = await html("/account", lin);
    expect(page.body).toContain("Claim a username first.");
    const response = await post("/auth/web/logout", lin, { csrf: tokenOf(page.body) });
    expect(response.status).toBe(303);
    // (As the edge does: always the front page, absolute.)
    expect(response.headers.get("location")).toBe("https://zeron.sh/");
    expect((await site.fetch("/account", { headers: { cookie: lin } })).status).toBe(303);
  });
});

describe("Discover", () => {
  test("Featured draws covers only when every featured plugin has a picture; one without, and the row is compact, one height", async () => {
    const withCovers = (await html("/plugins")).body;
    expect(withCovers).toContain('<ul class="featured-row" role="list">');
    expect(withCovers.match(/<img class="feature-picture"/g)).toHaveLength(3);
    try {
      await site.fake("bare", { id: "pr-radar" });
      // (A minute's cache: another address of the same page, a fresh one.)
      const { body } = await html("/plugins", await site.signIn("user_kai"));
      expect(body).toContain('<ul class="featured-row compact" role="list">');
      expect(body).not.toContain("feature-picture");
    } finally {
      await site.fake("bare");
    }
  });

  test("its lists say See all the one way", async () => {
    const { body } = await html("/plugins");
    expect(body.match(/>See all<span class="glyph/g)).toHaveLength(2);
  });
});

describe("the staff's side: the queue, a submission, the decision", () => {
  /** The rows of a table, each its cells' text. */
  const rowsOf = (body: string, table: string) => {
    const at = body.indexOf(`class="table ${table}`);
    const tbody = body.slice(body.indexOf("<tbody>", at), body.indexOf("</tbody>", at));
    return tbody.split("<tr").slice(1);
  };

  test("the queue: a table, the longest waiting first, its count in the title; a badge only where a row needs a look", async () => {
    const grace = await site.signIn("user_grace");
    const { body } = await html("/admin/plugins", grace);
    const rows = rowsOf(body, "comfortable queue");
    const total = Number(body.match(/Queue<span class="title-count"> · (\d+)<\/span>/)![1]);
    expect(rows).toHaveLength(total);
    const sent = rows.map((r) => Date.parse(r.match(/<time datetime="([^"]+)"/)![1]!));
    expect(sent).toEqual([...sent].sort((a, b) => a - b));
    // An update that asks for more (here a far-reaching ask among them): the danger's badge.
    const worst = rows.find((r) => r.includes("worst-case-listing-at-every-limit-xyz12"))!;
    expect(worst).toContain("Update");
    expect(worst).toMatch(/<span class="badge sm bad" title="New since 1\.0\.0: [^"]*messages\.send[^"]*">Asks for more<\/span>/);
    // Nothing new, but a host swapped inside an ask it had: "Asks changed".
    const xss = rows.find((r) => r.includes(">xss-probe<"))!;
    expect(xss).toMatch(/<span class="badge sm warn" title="What it asks for changed inside since 1\.0\.0">Asks changed<\/span>/);
    // A new plugin asks for everything it asks for: said by its kind, not a badge.
    const first = rows.find((r) => r.includes(">first-light<"))!;
    expect(first).toContain("New plugin");
    expect(first).not.toContain("badge");
  });

  test("an empty queue says so; forty fit one page, with no pager", async () => {
    const grace = await site.signIn("user_grace");
    try {
      await site.queueAs("empty");
      const empty = await html("/admin/plugins", grace);
      expect(empty.body).toContain("Nothing is waiting");
      expect(empty.body).toMatch(/Queue<span class="title-count"> · 0<\/span>/);
      await site.queueAs(40);
      const forty = await html("/admin/plugins", grace);
      expect(rowsOf(forty.body, "comfortable queue")).toHaveLength(40);
      expect(forty.body).not.toContain('class="pager"');
    } finally {
      await site.queueAs(null);
    }
  });

  test("a submission: what it asks for against the last approved version, what moved first, the risky in the danger's ink", async () => {
    const { body } = await html("/admin/plugins/worst-case-listing-at-every-limit-xyz12/1.5.0", await site.signIn("user_grace"));
    expect(body).toContain("Against 1.0.0, the last approved version: 2 new asks, 2 asks changed inside, its MCP server, its service and a setting that grants changed.");
    const rows = body.slice(body.indexOf('class="weigh"'), body.indexOf('id="checks"')).split('class="weigh-row').slice(1);
    const titles = rows.map((r) => r.match(/<p class="title">([^<]+)/)![1]!.trim());
    expect(titles.slice(0, 4)).toEqual(["Send you notifications", "Connect to these places", "Run python3, git, jq, bash on your computer as you", "Send messages to your agents as you"]);
    expect(rows[0]).toContain('<span class="badge sm info">New</span>');
    expect(rows[2]).toMatch(/^ risky"/);
    expect(rows[2]).toContain('<span class="badge sm info">Changed</span>');
    expect(rows[2]).toMatch(/<span class="plus" aria-hidden="true">\+<\/span><span>bash/);
    expect(rows[1]).toMatch(/<span class="minus" aria-hidden="true">−<\/span><span class="t-3">\*/);
    // Then what it binds past its asks (amendment 41), risky, its MCP server and service whole, before and after.
    expect(titles.slice(4, 7)).toEqual(["Its MCP server changed", "Its service changed", "A setting that grants access changed"]);
    expect(rows.slice(4, 7).every((r) => r.startsWith(" risky"))).toBe(true);
    expect(rows[4]).toMatch(/<dt>Before<\/dt><dd><span>node server\.js<\/span><span>API_URL=https:\/\/api\.[^<]+<\/span><\/dd>/);
    expect(rows[4]).toContain("<span>node server.js --listen 0.0.0.0:7777</span>");
    expect(rows[5]).toContain("<dd><span>services/sync.luau</span></dd>");
    // Then what is as before, quieter.
    expect(rows.slice(7).every((r) => r.startsWith(" same"))).toBe(true);
  });

  test("the decision: Approve, the one primary, asks once with what there is to weigh; Don't approve asks for a reason and words, showing the author's view; an approved version's is Take down", async () => {
    const grace = await site.signIn("user_grace");
    const pending = (await html("/admin/plugins/worst-case-listing-at-every-limit-xyz12/1.5.0", grace)).body;
    const rail = pending.slice(pending.indexOf('<aside class="decide'), pending.indexOf("</aside>"));
    expect(rail.match(/btn primary/g)).toHaveLength(1);
    expect(rail).toContain('<a class="btn primary" href="/admin/plugins/worst-case-listing-at-every-limit-xyz12/1.5.0/approve" data-dialog="approve">Approve…</a>');
    const ask = pending.slice(pending.indexOf('<dialog class="dialog sm" id="approve"'), pending.indexOf("</dialog>", pending.indexOf('id="approve"')));
    expect(ask).toContain("Anyone can install it once approved.");
    expect(ask).toContain('<p class="weigh-line bad">2 new asks · 2 asks changed · Its MCP server, its service and a setting that grants changed · 1 check to look at</p>');
    // (Approve has the focus: a clean update is a click, then Enter.)
    expect(ask).toContain('<button class="btn primary" type="submit" autofocus>Approve</button>');
    expect(rail).toContain("<dt>This author</dt><dd>");
    expect(rail).toContain('data-dialog="reject">Don\'t approve…</a>');
    expect(rail).not.toContain("Take down");
    expect(pending).toContain('<select id="reject-reason" name="reason" required>');
    expect(pending).toContain('<option value="" disabled selected>Choose a reason</option>');
    expect(pending).toContain("What @ada reads");
    // (The raw manifest, folded.)
    expect(pending).toMatch(/<details class="fold">\s*<summary>/);
    const approved = (await html("/admin/plugins/focus-timer/1.2.0", grace)).body;
    const approvedRail = approved.slice(approved.indexOf('<aside class="decide'), approved.indexOf("</aside>"));
    expect(approvedRail).toContain('data-dialog="take-down">Take down…</a>');
    expect(approvedRail).not.toContain("/approve");
    // Without the script, the same question as a page, the author's view on it too.
    const page = await html("/admin/plugins/worst-case-listing-at-every-limit-xyz12/1.5.0/reject", grace);
    expect(page.body).toContain("Don&#39;t approve Averyveryverylongpluginnamewithoutanyspacesatallthatkeepsgoing64 1.5.0?");
    expect(page.body).toContain("What @ada reads");
  });
});
