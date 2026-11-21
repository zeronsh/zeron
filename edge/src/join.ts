/**
 * `GET /` on a self-hosted edge — the server's invite (docs/self-hosting.md).
 *
 * The address of a Zeron server is all anyone needs to join it, so the
 * address is what an operator shares, and opened in a browser it says what
 * it is: "Join Acme on Zeron", one way on ("Open in Zeron", the apps' invite
 * link `zeron://server?address=…`, which opens their server dialog on this
 * address, looked up, and waits for a click), the way to get the app, and
 * the address to type by hand. The apps never switch on a link alone.
 *
 * Zeron's own hosts (`zeron.sh` and under it) keep `/` as it was: the
 * official server has a site of its own.
 */
import { discovery, tidyName } from "./discovery";
import type { Env } from "./env";
import { escapeHtml } from "./http";

/** Where Zeron is downloaded (apps/landing/src/components/home/Downloads.astro). */
export const DOWNLOADS_URL = "https://zeron.sh/#downloads";
/** How to run one (the guide this page's operator followed). */
export const GUIDE_URL = "https://github.com/zeronsh/zeron/blob/main/docs/self-hosting.md";

/** Zeron's own hosts: no invite page there. */
export const isOfficialHost = (hostname: string): boolean =>
  hostname === "zeron.sh" || hostname.endsWith(".zeron.sh");

/** The address as someone would type it into the app: the host alone over
 * https (its port too, if not the default), else the whole origin (http on
 * a local network). The same the apps fill in (screens/server `typed`). */
export const typedAddress = (url: URL): string => (url.protocol === "https:" ? url.host : url.origin);

/** The apps' invite link (crates/notify/src/links.rs `server_invite_link`). */
export const inviteLink = (address: string): string => `zeron://server?address=${encodeURIComponent(address)}`;

type State = "ready" | "dev" | "not_configured";

const page = (o: { state: State; name: string; named: boolean; address: string; nonce: string }): string => {
  const name = escapeHtml(o.name);
  const address = escapeHtml(o.address);
  const title =
    o.state === "ready" ? `Join ${name} on Zeron` : o.state === "dev" ? `${name} is a development server` : `${name} isn't ready yet`;
  const lead =
    o.state === "ready"
      ? `${o.named ? `${name} runs its own Zeron server` : "This is a self-hosted Zeron server"}. Open it in Zeron to sign in.`
      : o.state === "dev"
        ? "It signs in without checking who you are, so Zeron's apps won't use it."
        : `Its sign-in isn't set up. If it's yours, the <a href="${GUIDE_URL}">self-hosting guide</a> says what's missing.`;
  const body =
    o.state === "ready"
      ? `
    <a class="primary" href="${escapeHtml(inviteLink(o.address))}">Open in Zeron</a>
    <p class="quiet">Don't have Zeron yet? <a href="${DOWNLOADS_URL}">Download it</a> for Mac, Windows, Linux, iPhone or Android.</p>
    <hr />
    <section aria-labelledby="by-hand">
      <h2 id="by-hand">Or connect by hand</h2>
      <p>In Zeron, choose <b>File → Connect to Server…</b> (on a phone, the menu at the top of the welcome screen), then <b>Self-hosted</b>, and enter:</p>
      <div class="address">
        <code id="address">${address}</code>
        <button type="button" id="copy" aria-describedby="address">Copy</button>
      </div>
      <p class="quiet" id="copied" role="status" aria-live="polite"></p>
      <p class="quiet">Your account here is separate from a Zeron account.</p>
    </section>`
      : "";
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8" />
<meta name="viewport" content="width=device-width, initial-scale=1" />
<meta name="robots" content="noindex" />
<meta name="color-scheme" content="light dark" />
<title>${title}</title>
<style nonce="${o.nonce}">
  :root { --bg: #fafafa; --ink: #0a0a0a; --quiet: #6b6b6b; --line: #e4e4e4; --well: #f0f0f0;
          --on: #0a0a0a; --on-ink: #fafafa; --ring: #2563eb; }
  @media (prefers-color-scheme: dark) {
    :root { --bg: #0a0a0a; --ink: #ededed; --quiet: #9a9a9a; --line: #262626; --well: #161616;
            --on: #ededed; --on-ink: #0a0a0a; --ring: #60a5fa; }
  }
  * { box-sizing: border-box; }
  body { margin: 0; min-height: 100vh; display: grid; place-items: center; background: var(--bg); color: var(--ink);
         font: 15px/1.6 ui-sans-serif, system-ui, -apple-system, "Segoe UI", sans-serif; -webkit-font-smoothing: antialiased; }
  main { width: 100%; max-width: 30rem; padding: 3rem 1.5rem; text-align: center; }
  .mark { width: 44px; height: 44px; margin: 0 auto 1.25rem; border-radius: 12px; display: grid; place-items: center;
          background: var(--well); border: 1px solid var(--line); color: var(--quiet); }
  h1 { font-size: 1.5rem; line-height: 1.25; font-weight: 600; letter-spacing: -0.01em; margin: 0 0 0.5rem; overflow-wrap: anywhere; }
  h2 { font-size: 0.95rem; font-weight: 600; margin: 0 0 0.35rem; }
  p { margin: 0 0 0.75rem; text-wrap: pretty; }
  h1, .lead, main > .quiet { text-wrap: balance; }
  .lead { color: var(--quiet); margin-bottom: 1.75rem; }
  .quiet { color: var(--quiet); font-size: 13px; }
  a { color: inherit; text-underline-offset: 2px; }
  a:focus-visible, button:focus-visible { outline: 2px solid var(--ring); outline-offset: 2px; }
  .primary { display: inline-block; padding: 0.7rem 1.4rem; border-radius: 999px; background: var(--on); color: var(--on-ink);
             font-weight: 500; text-decoration: none; margin-bottom: 1rem; }
  .primary:hover { opacity: 0.88; }
  hr { border: 0; border-top: 1px solid var(--line); margin: 2rem 0 1.5rem; }
  section { text-align: left; font-size: 14px; }
  .address { display: flex; align-items: center; gap: 0.5rem; margin: 0.75rem 0 0.25rem; padding: 0.35rem 0.35rem 0.35rem 0.85rem;
             background: var(--well); border: 1px solid var(--line); border-radius: 10px; }
  code { flex: 1; min-width: 0; font: 13px/1.5 ui-monospace, SFMono-Regular, Menlo, monospace; overflow-wrap: anywhere; user-select: all; }
  button { flex: none; padding: 0.35rem 0.8rem; border-radius: 7px; border: 1px solid var(--line); background: var(--bg); color: var(--ink);
           font: 500 13px ui-sans-serif, system-ui, sans-serif; cursor: pointer; }
  #copied:empty { display: none; }
</style>
</head>
<body>
<main>
  <div class="mark" aria-hidden="true">
    <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round">
      <circle cx="12" cy="12" r="9" /><path d="M3 12h18M12 3c2.5 2.6 3.8 5.6 3.8 9s-1.3 6.4-3.8 9M12 3c-2.5 2.6-3.8 5.6-3.8 9s1.3 6.4 3.8 9" />
    </svg>
  </div>
  <h1>${title}</h1>
  <p class="lead">${lead}</p>${body}
</main>${
    o.state === "ready"
      ? `
<script nonce="${o.nonce}">
  const copy = document.getElementById("copy"), said = document.getElementById("copied");
  const text = document.getElementById("address").textContent;
  copy.addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText(text);
      copy.textContent = "Copied";
      said.textContent = "Address copied.";
    } catch {
      getSelection().selectAllChildren(document.getElementById("address"));
      said.textContent = "Selected: press ⌘C or Ctrl+C to copy it.";
    }
    setTimeout(() => { copy.textContent = "Copy"; said.textContent = ""; }, 2000);
  });
</script>`
      : ""
  }
</body>
</html>`;
};

/** Answer `/` with the invite page; undefined for any other request, and on
 * Zeron's own hosts. */
export const joinRoute = (request: Request, env: Env, url: URL): Response | undefined => {
  if (url.pathname !== "/" || (request.method !== "GET" && request.method !== "HEAD")) return undefined;
  if (isOfficialHost(url.hostname)) return undefined;
  const found = discovery(env);
  const state: State = !found.ok ? "not_configured" : found.document.auth.mode === "dev" ? "dev" : "ready";
  const address = typedAddress(url);
  const named = tidyName(env.SERVER_NAME);
  const nonce = crypto.randomUUID().replaceAll("-", "");
  const html = page({ state, name: named ?? address, named: named !== undefined, address, nonce });
  return new Response(request.method === "HEAD" ? null : html, {
    status: state === "not_configured" ? 503 : 200,
    headers: {
      "content-type": "text/html; charset=utf-8",
      // Its own script and style only; nothing framed, posted or loaded.
      "content-security-policy": `default-src 'none'; script-src 'nonce-${nonce}'; style-src 'nonce-${nonce}'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'`,
      "x-content-type-options": "nosniff",
      "referrer-policy": "no-referrer",
      "x-robots-tag": "noindex",
      // (A nonce per answer: never a stored copy.)
      "cache-control": "no-store"
    }
  });
};
