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

describe("the catalog", () => {
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

  test("Next and Previous are real addresses that carry the cursor", async () => {
    const first = await html("/plugins");
    const next = /<a class="page"[^>]* href="([^"]+)" rel="next"/.exec(first.body)?.[1]?.replaceAll("&amp;", "&");
    expect(next).toMatch(/^\/plugins\?cursor=/);
    const second = await html(next!);
    expect(second.response.status).toBe(200);
    expect(second.body).not.toContain('id="featured-heading"');
    const previous = /<a class="page"[^>]* href="([^"]+)" rel="prev"/.exec(second.body)?.[1]?.replaceAll("&amp;", "&");
    expect(previous).toMatch(/^\/plugins\?cursor=/);
    const back = await html(previous!);
    // (Back is the first page's list, without the featured row.)
    const listed = (body: string) => [...body.matchAll(/class="follow" href="\/plugins\/([^"]+)"/g)].map((m) => m[1]).slice(-24);
    expect(listed(back.body)).toEqual(listed(first.body));
  });

  test("search, category, keyword, sort", async () => {
    expect((await html("/plugins?q=timer")).body).toContain("Focus timer");
    const empty = await html("/plugins?q=zzzzqqq");
    expect(empty.body).toContain("No plugins match “zzzzqqq”");
    const git = await html("/plugins?category=git");
    expect(git.body).toContain("PR Radar");
    expect(git.body).not.toContain("Token meter");
    expect(git.body).toContain('<link rel="canonical" href="https://zeron.sh/plugins?category=git">');
    expect((await html("/plugins?keyword=github")).body).toContain("PR Radar");
  });

  test("an address that says more than it means is sent to the plain one", async () => {
    const response = await site.fetch("/plugins?sort=popular&utm_source=x");
    expect(response.status).toBe(301);
    expect(response.headers.get("location")).toBe("/plugins");
  });

  test("search results stay out of search engines; the store doesn't", async () => {
    expect((await html("/plugins?q=timer")).body).toContain('<meta name="robots" content="noindex, nofollow">');
    expect((await html("/plugins")).body).not.toContain('name="robots"');
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

  test("unknown, unlisted and malformed ids are the site's 404", async () => {
    const notFound = readFileSync(new URL("../dist/client/404.html", import.meta.url), "utf8");
    for (const path of ["/plugins/no-such-plugin", "/plugins/voice-memos", "/plugins/crypto-ticker", "/plugins/Bad_Id", "/plugins/a%2Fb"]) {
      const { response, body } = await html(path);
      expect(response.status, path).toBe(404);
      expect(body, path).toBe(notFound);
    }
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
    expect(response.headers.getSetCookie().join("\n")).toMatch(/__Host-zeron_session=[\w-]+; Path=\/; HttpOnly; Secure; SameSite=Lax/);
    expect(response.headers.get("cache-control")).toBe("private, no-store");
    expect(response.headers.get("cloudflare-cdn-cache-control") ?? "").not.toMatch(/public/);
  });

  test("someone else's plugin is the 404 page", async () => {
    const { response } = await html("/account/plugins/focus-timer", await site.signIn("user_kai"));
    expect(response.status).toBe(404);
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
    expect((await html("/account/plugins/session-badges", ada)).body).toMatch(/1\.2\.1<\/th>\s*<td[^>]*>[^]*?In review/);
  });

  test("a post without the session's token changes nothing", async () => {
    const ada = await site.signIn("user_ada");
    const response = await post("/account/plugins/session-badges/versions/1.2.1/withdraw", ada, { csrf: "not-the-token" });
    expect(response.status).toBe(303);
    const after = await html("/account/plugins/session-badges", `${ada}; ${flashOf(response)}`);
    expect(after.body).toContain("The page was open too long. Try again.");
    expect(after.body).toMatch(/1\.2\.1<\/th>\s*<td[^>]*>[^]*?In review/);
  });

  test("a confirm page asks first; the post does it once and says so once", async () => {
    const ada = await site.signIn("user_ada");
    const ask = await html("/account/plugins/session-badges/versions/1.2.1/withdraw", ada);
    expect(ask.response.status).toBe(200);
    expect(ask.body).toContain("Withdraw 1.2.1 from review?");
    const response = await post("/account/plugins/session-badges/versions/1.2.1/withdraw", ada, { csrf: tokenOf(ask.body) });
    expect(response.status).toBe(303);
    expect(response.headers.get("location")).toBe("/account/plugins/session-badges");
    expect(response.headers.getSetCookie().find((c) => c.startsWith("__Host-zeron_flash="))).toMatch(/^__Host-zeron_flash=withdrawn; .*Secure/);
    const cookie = `${ada}; ${flashOf(response)}`;
    const after = await html("/account/plugins/session-badges", cookie);
    expect(after.body).toContain("Withdrawn from review.");
    expect(after.body).toMatch(/1\.2\.1<\/th>\s*<td[^>]*>[^]*?Withdrawn/);
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

  test("signing out is a form post with the token; the session ends", async () => {
    const lin = await site.signIn("user_lin");
    const page = await html("/account", lin);
    expect(page.body).toContain("claim a username first");
    const response = await post("/auth/web/logout", lin, { csrf: tokenOf(page.body), return_to: "/plugins" });
    expect(response.status).toBe(303);
    expect(response.headers.get("location")).toBe("/plugins");
    expect((await site.fetch("/account", { headers: { cookie: lin } })).status).toBe(303);
  });
});
