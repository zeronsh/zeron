// The built site in front of the real edge worker, through its EDGE binding:
// real plugins published as a device publishes them, a real web sign-in,
// and every page and form of the store's, its admin's and an author's,
// against what the edge really answers. (tests/routes.test.ts holds the
// same site to the fake; where the two disagreed, the fake was wrong.)
import { afterAll, beforeAll, describe, expect, test } from "vitest";
import { PNG, Publisher, startRealEdge, type RealEdge } from "./real-edge.ts";
import { rawElements } from "../../src/lib/store/security.ts";

/** A page runs only the site's own script files: no inline script, no handler, no javascript: link. */
function inert(body: string) {
  for (const e of rawElements(body)) if (e.tag === "script") expect(e.attributes).toMatch(/\bsrc="\/_astro\/|type="application\/ld\+json"/);
  expect(body.replace(/="[^"]*"/g, '=""')).not.toMatch(/<[a-z][^>]*\son[a-z]+=/i);
  expect(body).not.toMatch(/(href|src)="\s*(javascript|data|vbscript):/i);
}

const SESSION = "__Host-zeron_session";

let edge: RealEdge;
let ada: Publisher;
let grace: Browser;
let adaOnWeb: Browser;

/**
 * A browser on the site: its cookies, as Set-Cookie leaves them (the
 * session the edge rotates included), and how many times the session
 * changed under it.
 */
class Browser {
  readonly cookies = new Map<string, string>();
  rotations = 0;

  get cookie(): string {
    return [...this.cookies].map(([k, v]) => `${k}=${v}`).join("; ");
  }

  keep(response: Response): Response {
    for (const line of response.headers.getSetCookie()) {
      const [pair, ...attributes] = line.split(";");
      const at = pair!.indexOf("=");
      const name = pair!.slice(0, at).trim();
      const value = pair!.slice(at + 1).trim();
      const gone = value === "" || attributes.some((a) => /^\s*max-age=0\s*$/i.test(a) || /^\s*expires=Thu, 01 Jan 1970/i.test(a));
      if (name === SESSION && !gone && this.cookies.has(SESSION) && this.cookies.get(SESSION) !== value) this.rotations++;
      if (gone) this.cookies.delete(name);
      else this.cookies.set(name, value);
    }
    return response;
  }

  async get(path: string): Promise<{ response: Response; body: string }> {
    const response = this.keep(await edge.site(path, { headers: this.cookies.size ? { cookie: this.cookie } : {} }));
    return { response, body: await response.text() };
  }

  /** A form on the site's pages, posted as a browser posts it (its Origin, the page's token). */
  async post(path: string, fields: Record<string, string>): Promise<Response> {
    return this.keep(
      await edge.site(path, {
        method: "POST",
        headers: { cookie: this.cookie, origin: edge.origin, "content-type": "application/x-www-form-urlencoded" },
        body: new URLSearchParams(fields).toString(),
      }),
    );
  }

  /** What the last answer said, as the page after it will (and forgets it, as the page does). */
  flash(): string | undefined {
    const word = this.cookies.get("__Host-zeron_flash");
    this.cookies.delete("__Host-zeron_flash");
    return word;
  }

  /** The session's token, as a page carries it in its forms. */
  async token(path: string): Promise<string> {
    const { body } = await this.get(path);
    const csrf = /name="csrf" value="([^"]+)"/.exec(body)?.[1];
    if (!csrf) throw new Error(`no form on ${path}`);
    return csrf;
  }
}

/**
 * A web sign-in through the site, as a browser makes it: the site's sign-in
 * link, AuthKit's page (the runner's, signing in `userId`), back to the
 * site's callback with the code.
 */
async function signInOnTheWeb(userId: string, returnTo = "/account"): Promise<Browser> {
  const browser = new Browser();
  const login = browser.keep(await edge.site(`/auth/web/login?return_to=${encodeURIComponent(returnTo)}`));
  expect(login.status, await login.clone().text()).toBe(302);
  const authorize = new URL(login.headers.get("location")!);
  expect(authorize.origin + authorize.pathname).toBe("https://api.workos.com/user_management/authorize");
  const page = await edge.authorize(authorize, userId);
  const callback = new URL(page.headers.get("location")!);
  expect(callback.origin).toBe(edge.origin);
  const back = browser.keep(await edge.site(`${callback.pathname}${callback.search}`, { headers: { cookie: browser.cookie } }));
  expect(back.status, await back.clone().text()).toBe(303);
  // (The edge sends the browser back to the site's own address, absolute.)
  expect(new URL(back.headers.get("location")!, edge.origin).href).toBe(new URL(returnTo, edge.origin).href);
  expect(browser.cookies.get(SESSION)).toMatch(/^[A-Za-z0-9_-]{43}$/);
  return browser;
}

beforeAll(async () => {
  edge = await startRealEdge();
  // An author, signed in on their device, with a key; and a reviewer the
  // edge has met (their first sign-in) and an operator made staff.
  ada = await Publisher.signIn(edge.url, "user_ada", "adawren");
  await Publisher.signIn(edge.url, "user_grace", "gracehop");
  await edge.grant("user_grace", "admin");
  await ada.publish("focus-timer", "1.0.0", {
    files: { "README.md": "# Focus timer\n\nTwenty-five minutes on, five off.\n\n![The timer](shots/one.png)\n\n```js\nconst on = 25;\n```\n" },
  });
  await ada.publish("theme-sync", "0.1.0", { manifest: { categories: ["appearance"] } });
  await ada.publish("pr-review", "0.3.0", { manifest: { categories: ["git"], permissions: ["ui.add", "messages.send"] } });
  grace = await signInOnTheWeb("user_grace", "/admin/plugins");
  adaOnWeb = await signInOnTheWeb("user_ada");
}, 300_000);

afterAll(async () => {
  await edge?.stop();
});

describe("signing in on the web", () => {
  test("the session the site reads is the edge's", async () => {
    const response = await edge.site("/auth/web/session", { headers: { cookie: grace.cookie } });
    expect(response.status).toBe(200);
    const session = (await response.json()) as { user: { name: string }; username: string; roles: string[]; csrf: string; fresh_sign_in: boolean };
    expect(session.username).toBe("gracehop");
    expect(session.roles).toEqual(["admin"]);
    expect(session.fresh_sign_in).toBe(true);
    const account = await grace.get("/account");
    expect(account.response.status).toBe(200);
    expect(account.body).toContain("@gracehop");
  });

  test("signed out, the account pages ask you to sign in, and the admin ones aren't there", async () => {
    const account = await edge.site("/account");
    expect(account.status).toBe(303);
    expect(account.headers.get("location")).toBe("/auth/web/login?return_to=%2Faccount");
    expect((await edge.site("/admin/plugins")).status).toBe(404);
    // (A member who isn't staff gets the same 404 as an address that isn't there.)
    expect((await adaOnWeb.get("/admin/plugins")).response.status).toBe(404);
  });
});

describe("review", () => {
  test("the queue lists what was submitted, and each submission's views open", async () => {
    const queue = await grace.get("/admin/plugins");
    expect(queue.response.status).toBe(200);
    for (const id of ["focus-timer", "theme-sync", "pr-review"]) expect(queue.body).toContain(`/admin/plugins/${id}/`);
    for (const view of ["", "?view=readme", "?view=files", "?view=changes", "?view=files&path=README.md"]) {
      const page = await grace.get(`/admin/plugins/focus-timer/1.0.0${view}`);
      expect(page.response.status, view).toBe(200);
    }
    expect((await grace.get("/admin/plugins/focus-timer/1.0.0?view=readme")).body).toContain("Twenty-five minutes on");
  });

  test("a staff session's first admin page rotates it, and the browser gets the new one", async () => {
    // (The edge replaces a staff token on its first privileged use, then
    // every 10 minutes of it; the old one works a minute more.)
    const browser = await signInOnTheWeb("user_grace", "/admin/plugins");
    const signedIn = browser.cookies.get(SESSION);
    const queue = await browser.get("/admin/plugins");
    expect(queue.response.status).toBe(200);
    expect(browser.rotations).toBe(1);
    expect(browser.cookies.get(SESSION)).not.toBe(signedIn);
    expect(queue.response.headers.get("cache-control")).toBe("private, no-store");
    const rotated = queue.response.headers.getSetCookie().find((c) => c.startsWith(`${SESSION}=`))!;
    expect(rotated).toMatch(/; Secure/i);
    expect(rotated).toMatch(/; HttpOnly/i);
    expect(rotated).toMatch(/; Path=\//i);
    // The new cookie is the one that works from here on, for pages and forms.
    expect((await browser.get("/admin/reports")).response.status).toBe(200);
    expect(browser.rotations).toBe(1);
  });

  test("approving lists it", async () => {
    const csrf = await grace.token("/admin/plugins/focus-timer/1.0.0");
    const approved = await grace.post("/admin/plugins/focus-timer/1.0.0/approve", { csrf });
    expect(approved.status).toBe(303);
    expect(approved.headers.get("location")).toBe("/admin/plugins/focus-timer/1.0.0");
    expect(grace.flash()).toBe("approved");
    await grace.post("/admin/plugins/pr-review/0.3.0/approve", { csrf: await grace.token("/admin/plugins/pr-review/0.3.0") });
    expect(grace.flash()).toBe("approved");
  });

  test("rejecting tells the author why", async () => {
    const csrf = await grace.token("/admin/plugins/theme-sync/0.1.0");
    const rejected = await grace.post("/admin/plugins/theme-sync/0.1.0/reject", { csrf, reason: "listing", message: "Add a screenshot of a theme." });
    expect(rejected.status).toBe(303);
    expect(grace.flash()).toBe("rejected");
    const mine = await adaOnWeb.get("/account/plugins/theme-sync");
    expect(mine.response.status).toBe(200);
    expect(mine.body).toContain("Add a screenshot of a theme.");
  });
});

describe("an author's words, from a real package", () => {
  // Everything an author writes, written to break the page: through the
  // edge's own checks, then onto the site's pages.
  // (The edge refuses a control or invisible formatting character in a
  // name, a right-to-left override among them: the README can still carry one.)
  const NAME = `<img src=x onerror="alert('name')"> Probe`;
  test("the edge takes it, and every page that shows it shows it as words", async () => {
    await ada.publish("probe", "1.0.0", {
      manifest: {
        name: NAME,
        description: `"><script>alert('description')</script> Every word tries to escape the page.`,
        permissions: ["ui.add", "tools"],
        tools: [{ name: "probe_tool", description: `</p><script>alert('tool')</script><img src=x onerror=alert('tool')>` }],
        homepage: "https://example.com/a?b=\"><script>alert('homepage')</script>",
      },
      files: {
        "README.md": `# Probe <script>alert('h1')</script> \u202Eevil\n\n<img src="x" onerror="alert('img')">\n\n[a link](javascript:alert('link')) and [a page](https://example.com/"onmouseover="alert(1))\n\n![a picture](shots/one.png)\n`,
        "CHANGELOG.md": "# Changes\n\n## 1.0.0\n\n- <script>alert('notes')</script> and [a link](javascript:alert('notes'))\n",
        "docs/a b #1 100%.md": "# <script>alert('file')</script>\n",
      },
    });
    const review = await grace.get("/admin/plugins/probe/1.0.0");
    expect(review.response.status).toBe(200);
    for (const view of ["", "?view=readme", "?view=files", `?view=files&path=${encodeURIComponent("docs/a b #1 100%.md")}`, "?view=changes"]) {
      const { response, body } = await grace.get(`/admin/plugins/probe/1.0.0${view}`);
      expect(response.status, view).toBe(200);
      inert(body);
    }
    await grace.post("/admin/plugins/probe/1.0.0/approve", { csrf: await grace.token("/admin/plugins/probe/1.0.0") });
    expect(grace.flash()).toBe("approved");
    for (const path of ["/plugins", "/plugins/all", "/plugins/probe", "/plugins/all?q=probe"]) {
      const { response, body } = await new Browser().get(path);
      expect(response.status, path).toBe(200);
      inert(body);
    }
    const page = await new Browser().get("/plugins/probe");
    // The link the author gave, as the edge parsed it, encoded.
    expect(page.body).toContain('href="https://example.com/a?b=%22%3E%3Cscript%3Ealert(%27homepage%27)%3C/script%3E"');
    expect(page.body).toContain('src="/plugins/probe/files/1.0.0/shots/one.png"');
    expect(page.body).toContain("probe_tool");
  });
});

describe("the catalog, from the edge's real listing", () => {
  test("lists what's approved, and only that", async () => {
    for (const path of ["/plugins", "/plugins/all"]) {
      const { response, body } = await new Browser().get(path);
      expect(response.status, path).toBe(200);
      expect(body, path).toContain('href="/plugins/focus-timer"');
      expect(body, path).toContain('href="/plugins/pr-review"');
      expect(body, path).not.toContain('href="/plugins/theme-sync"');
    }
    // The count is the edge's total (amendment 38): exact, the public list's.
    expect((await new Browser().get("/plugins/all")).body).toMatch(/All plugins<span class="title-count"> · \d+<\/span>/);
    expect((await new Browser().get("/plugins/all?q=zzzzqqq")).body).toContain('<span class="title-count"> · 0</span>');
    const git = await new Browser().get("/plugins/all?category=git");
    expect(git.body).toContain('href="/plugins/pr-review"');
    expect(git.body).not.toContain('href="/plugins/focus-timer"');
    const search = await new Browser().get("/plugins/all?q=focus");
    expect(search.body).toContain('href="/plugins/focus-timer"');
  });

  test("a plugin's page: its README, its pictures through the site, its author", async () => {
    const { response, body } = await new Browser().get("/plugins/focus-timer");
    expect(response.status).toBe(200);
    expect(body).toContain("Twenty-five minutes on");
    expect(body).toContain('src="/plugins/focus-timer/files/1.0.0/shots/one.png"');
    expect(body).toContain('href="/@adawren"');
    expect(body).toContain('href="zeron://open/plugin/focus-timer"');
    // The same page signed in, byte for byte.
    expect((await adaOnWeb.get("/plugins/focus-timer")).body).toBe(body);
  });

  test("the files proxy serves the edge's files as pictures in a sandbox, and nothing else", async () => {
    const png = await edge.site("/plugins/focus-timer/files/1.0.0/shots/one.png");
    expect(png.status).toBe(200);
    expect(png.headers.get("content-type")).toBe("image/png");
    expect(png.headers.get("content-security-policy")).toContain("sandbox");
    expect(png.headers.get("x-content-type-options")).toBe("nosniff");
    expect(Buffer.from(await png.arrayBuffer()).equals(PNG)).toBe(true);
    // Kept a minute at most, by a browser as by a shared cache: a version taken down stops showing within it.
    expect(Number(/^public, max-age=(\d+)$/.exec(png.headers.get("cache-control") ?? "")?.[1])).toBeLessThanOrEqual(60);
    const icon = await edge.site("/plugins/focus-timer/files/1.0.0/icon.svg");
    expect(icon.status).toBe(200);
    expect(icon.headers.get("content-type")).toBe("image/svg+xml");
    for (const path of ["init.luau", "README.md", "shots/none.png"]) {
      const refused = await edge.site(`/plugins/focus-timer/files/1.0.0/${path}`);
      expect(refused.status, path).toBe(404);
      expect(refused.headers.get("content-security-policy"), path).toContain("sandbox");
    }
    // A version still in review has no public files.
    expect((await edge.site("/plugins/theme-sync/files/0.1.0/icon.svg")).status).toBe(404);
  });
});

describe("a version in review's pictures", () => {
  test("its owner and the admins see them through the site, kept by no one; anyone else gets the sandboxed 404", async () => {
    await ada.publish("pending-pics", "0.1.0");
    const path = "/plugins/pending-pics/files/0.1.0/shots/one.png";
    const kai = await signInOnTheWeb("user_kai");
    for (const [who, browser] of [["the owner", adaOnWeb], ["an admin", grace]] as const) {
      const shown = await edge.site(path, { headers: { cookie: browser.cookie } });
      expect(shown.status, who).toBe(200);
      expect(shown.headers.get("content-type"), who).toBe("image/png");
      expect(shown.headers.get("cache-control"), who).toBe("private, no-store");
      expect(shown.headers.get("content-security-policy"), who).toContain("sandbox");
      expect(Buffer.from(await shown.arrayBuffer()).equals(PNG), who).toBe(true);
    }
    for (const [who, cookie] of [["anyone", undefined], ["another member", kai.cookie]] as const) {
      const refused = await edge.site(path, cookie ? { headers: { cookie } } : {});
      expect(refused.status, who).toBe(404);
      expect(refused.headers.get("content-security-policy"), who).toContain("sandbox");
      expect(refused.headers.get("cache-control"), who).toBe("no-store");
    }
  });
});

describe("an author's pages", () => {
  test("their plugins, each with its versions and keys", async () => {
    const account = await adaOnWeb.get("/account");
    expect(account.body).toContain('href="/account/plugins/focus-timer"');
    const key = await ada.deployKey("focus-timer", "ci");
    const page = await adaOnWeb.get("/account/plugins/focus-timer");
    expect(page.response.status).toBe(200);
    expect(page.body).toContain("1.0.0");
    expect(page.body).toContain(`/account/plugins/focus-timer/keys/${key}`);
    const csrf = await adaOnWeb.token("/account/plugins/focus-timer");
    expect((await adaOnWeb.post(`/account/plugins/focus-timer/keys/${key}`, { csrf })).status).toBe(303);
    expect(adaOnWeb.flash()).toBe("key_revoked");
  });

  test("stop new installs of a version, and allow them again", async () => {
    const csrf = await adaOnWeb.token("/account/plugins/focus-timer");
    expect((await adaOnWeb.post("/account/plugins/focus-timer/versions/1.0.0/yank", { csrf })).status).toBe(303);
    expect(adaOnWeb.flash()).toBe("yanked");
    expect((await adaOnWeb.post("/account/plugins/focus-timer/versions/1.0.0/unyank", { csrf })).status).toBe(303);
    expect(adaOnWeb.flash()).toBe("unyanked");
  });

  test("withdraw a version from review", async () => {
    await ada.publish("focus-timer", "1.1.0");
    const csrf = await adaOnWeb.token("/account/plugins/focus-timer");
    expect((await adaOnWeb.post("/account/plugins/focus-timer/versions/1.1.0/withdraw", { csrf })).status).toBe(303);
    expect(adaOnWeb.flash()).toBe("withdrawn");
    expect((await grace.get("/admin/plugins")).body).not.toContain("/admin/plugins/focus-timer/1.1.0");
  });
});

describe("an update", () => {
  test("the reviewer sees what changed and what it newly asks for; approved, it's the one offered", async () => {
    await ada.publish("focus-timer", "1.2.0", {
      manifest: { permissions: ["ui.add", "messages.send"] },
      files: { "init.luau": 'return "focus-timer 1.2.0, now with breaks"' },
    });
    const overview = await grace.get("/admin/plugins/focus-timer/1.2.0");
    expect(overview.body).toContain("Update from 1.0.0");
    expect(overview.body).toContain("It now also asks to:");
    expect(overview.body).toContain("Send messages to your agents as you");
    const changes = await grace.get("/admin/plugins/focus-timer/1.2.0?view=changes");
    expect(changes.response.status).toBe(200);
    expect(changes.body).toContain("init.luau");
    expect(changes.body).toContain("now with breaks");
    // The manifest isn't one of a package's files at the edge: the site shows it among them.
    expect(changes.body).toContain("zeron-plugin.json");
    expect(changes.body).toContain("messages.send");
    const files = await grace.get("/admin/plugins/focus-timer/1.2.0?view=files");
    expect(files.body).toMatch(/<p class="file-head"[^>]*><code[^>]*>zeron-plugin.json<\/code>/);
    expect(files.body).toMatch(/(&quot;|")entry(&quot;|")/);
    await grace.post("/admin/plugins/focus-timer/1.2.0/approve", { csrf: await grace.token("/admin/plugins/focus-timer/1.2.0") });
    expect(grace.flash()).toBe("approved");
    const page = await new Browser().get("/plugins/focus-timer");
    expect(page.body).toContain("1.2.0");
    expect(page.body).toContain("Send messages to your agents as you");
    expect(page.body).toContain('src="/plugins/focus-timer/files/1.2.0/shots/one.png"');
  });
});

describe("reports", () => {
  test("a member reports a plugin, and a reviewer acts on it", async () => {
    const csrf = await adaOnWeb.token("/plugins/pr-review/report");
    const sent = await adaOnWeb.post("/plugins/pr-review/report", { csrf, reason: "broken", text: "It never loads the diff." });
    expect(sent.status).toBe(303);
    expect(adaOnWeb.flash()).toBe("report_sent");
    const open = await grace.get("/admin/reports");
    expect(open.response.status).toBe(200);
    expect(open.body).toContain("It never loads the diff.");
    const report = /action="\/admin\/reports\/([^"]+)"/.exec(open.body)?.[1];
    expect(report).toBeTruthy();
    const resolved = await grace.post(`/admin/reports/${report}`, { csrf: await grace.token("/admin/reports"), resolution: "dismissed", note: "Works here." });
    expect(resolved.status).toBe(303);
    expect(grace.flash()).toBe("report_resolved");
    expect((await grace.get("/admin/reports?status=dismissed")).body).toContain("It never loads the diff.");
  });
});

describe("a plugin's admin page", () => {
  test("feature, suspend, restore, transfer, remove a version", async () => {
    const page = await grace.get("/admin/plugins/pr-review");
    expect(page.response.status).toBe(200);
    const csrf = await grace.token("/admin/plugins/pr-review");
    for (const [action, fields, word] of [
      ["feature", {}, "featured"],
      ["unfeature", {}, "unfeatured"],
      ["suspend", { reason: "Looking into a report." }, "suspended"],
      ["restore", {}, "restored"],
      ["transfer", { to_username: "@gracehop" }, "transferred"],
    ] as const) {
      const response = await grace.post(`/admin/plugins/pr-review/actions/${action}`, { csrf, ...fields });
      expect(response.status, action).toBe(303);
      expect(grace.flash(), action).toBe(word);
    }
    expect((await new Browser().get("/plugins/pr-review")).body).toContain('href="/@gracehop"');
    const removed = await grace.post("/admin/plugins/pr-review/0.3.0/remove", { csrf, reason: "Removed in a test." });
    expect(removed.status).toBe(303);
    expect(grace.flash()).toBe("removed");
    expect((await new Browser().get("/plugins")).body).not.toContain('href="/plugins/pr-review"');
  });
});

describe("reserved ids", () => {
  test("reserve an id for someone, then free it", async () => {
    const csrf = await grace.token("/admin/reservations");
    const made = await grace.post("/admin/reservations", { csrf, id: "pomodoro-pro", to_username: "@adawren", note: "Promised." });
    expect(made.status).toBe(303);
    expect(grace.flash()).toBe("reserved");
    const list = await grace.get("/admin/reservations");
    expect(list.body).toContain("pomodoro-pro");
    expect(list.body).toContain("@adawren");
    // The site's own page words: the edge refuses them (400 site_id), and the page says so.
    expect((await grace.post("/admin/reservations", { csrf, id: "publishing", note: "Ours." })).status).toBe(303);
    expect(grace.flash()).toBe("site_id");
    expect((await grace.get("/admin/reservations")).body).not.toMatch(/<code[^>]*>publishing<\/code>/);
    const freed = await grace.post("/admin/reservations/pomodoro-pro", { csrf });
    expect(freed.status).toBe(303);
    expect(grace.flash()).toBe("reservation_released");
    expect((await grace.get("/admin/reservations")).body).not.toContain("pomodoro-pro");
  });
});

describe("the audit log", () => {
  test("every action above is in it", async () => {
    const { response, body } = await grace.get("/admin/audit");
    expect(response.status).toBe(200);
    for (const id of ["focus-timer", "theme-sync", "pr-review", "pomodoro-pro"]) expect(body, id).toContain(id);
    const one = await grace.get("/admin/audit?plugin=theme-sync");
    expect(one.body).toContain("theme-sync");
    expect(one.body).not.toContain("pomodoro-pro");
  });
});

describe("every page, with what the real edge holds", () => {
  const PUBLIC = ["/plugins", "/plugins/all", "/plugins/all?category=productivity", "/plugins/all?keyword=focus", "/plugins/all?sort=new", "/plugins/focus-timer", "/plugins/probe", "/plugins/sitemap.xml"];
  const MEMBER = ["/account", "/account/plugins/focus-timer", "/account/plugins/theme-sync", "/account/plugins/focus-timer/versions/1.0.0/yank", "/plugins/focus-timer/report"];
  const STAFF = [
    "/admin/plugins",
    "/admin/plugins/focus-timer",
    "/admin/plugins/theme-sync",
    "/admin/plugins/theme-sync/0.1.0",
    "/admin/plugins/focus-timer/1.0.0/remove",
    "/admin/plugins/focus-timer/actions/suspend",
    "/admin/plugins/focus-timer/actions/transfer",
    "/admin/reports",
    "/admin/reports?status=actioned",
    "/admin/reports?status=dismissed",
    "/admin/reservations",
    "/admin/audit",
    "/admin/audit?plugin=focus-timer",
  ];

  test("each one answers, runs nothing of anyone's, and is kept by the right people", async () => {
    for (const path of PUBLIC) {
      const { response, body } = await new Browser().get(path);
      expect(response.status, path).toBe(200);
      if (path.endsWith(".xml")) continue;
      inert(body);
      expect(response.headers.get("content-security-policy"), path).toContain("script-src 'self'");
      expect(response.headers.get("cache-control"), path).toMatch(/^public/);
    }
    for (const [who, paths] of [[adaOnWeb, MEMBER], [grace, STAFF]] as const) {
      for (const path of paths) {
        const { response, body } = await who.get(path);
        expect(response.status, path).toBe(200);
        inert(body);
        expect(response.headers.get("cache-control"), path).toBe("private, no-store");
        expect(response.headers.get("x-robots-tag"), path).toBe("noindex, nofollow");
      }
    }
  });

  test("the plugins sitemap lists what's listed", async () => {
    const { body } = await new Browser().get("/plugins/sitemap.xml");
    expect(body).toContain("<loc>https://zeron.sh/plugins/focus-timer</loc>");
    expect(body).toContain("<loc>https://zeron.sh/plugins/probe</loc>");
    expect(body).not.toContain("theme-sync</loc>");
  });
});

describe("signing out", () => {
  test("without the session's token, it's refused in the store's word, and the session lives", async () => {
    const browser = await signInOnTheWeb("user_ada");
    const refused = await browser.post("/auth/web/logout", {});
    expect(refused.status).toBe(403);
    const said = (await refused.json()) as Record<string, unknown>;
    expect(said).toMatchObject({ error: "bad_csrf", message: expect.any(String) });
    expect(said).not.toHaveProperty("reason");
    expect((await browser.get("/account")).response.status).toBe(200);
  });


  test("ends the session at the edge, and the browser's cookie with it", async () => {
    const browser = await signInOnTheWeb("user_ada");
    const kept = browser.cookie;
    // The Sign out button's form, as the account bar has it.
    const page = await browser.get("/account");
    const form = /<form[^>]* action="\/auth\/web\/logout"[^>]*>([\s\S]*?)<\/form>/.exec(page.body)?.[1] ?? "";
    const fields = Object.fromEntries([...form.matchAll(/name="([^"]+)" value="([^"]*)"/g)].map((m) => [m[1]!, m[2]!]));
    expect(Object.keys(fields)).toEqual(["csrf"]);
    const out = await browser.post("/auth/web/logout", fields);
    // (The edge always sends a signed-out browser to the site's front page.)
    expect(out.headers.get("location")).toBe(`${edge.origin}/`);
    expect(out.status).toBe(303);
    expect(browser.cookies.has(SESSION)).toBe(false);
    // The old cookie, kept by someone, opens nothing.
    expect((await edge.site("/account", { headers: { cookie: kept } })).status).toBe(303);
  });
});
