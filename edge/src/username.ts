/**
 * Username rules: 3–20 ASCII letters, digits, `_` or `-`, starting with a
 * letter or digit. Stored and served lowercase, so `Wing` and `wing` are one
 * name and a card has one URL.
 *
 * Reserved names (`isReservedUsername`) keep routes, staff words and brands a
 * member could pass themselves off as out of circulation for new claims. A
 * plugin's "Made by @name" reads straight from the name, so a name that looks
 * like Zeron, its staff or a vendor's must not be claimable. Operators set
 * names past this check (`POST /admin/usernames/set`).
 */

const USERNAME_RE = /^[A-Za-z0-9][A-Za-z0-9_-]{2,19}$/;

/** The stored form of a name, or undefined if it breaks the rules. The check
 * comes first: `toLowerCase` folds non-ASCII letters into ASCII ones (the
 * Kelvin sign into `k`). */
export const foldUsername = (raw: unknown): string | undefined =>
  typeof raw === "string" && USERNAME_RE.test(raw) ? raw.toLowerCase() : undefined;

/** Our routes and words a member could pass off as us. Compared on the
 * skeleton (`skeleton`), so `ad-min` and `adm1n` are `admin`. */
const RESERVED = new Set([
  "about", "account", "accounts", "admin", "administrator", "api", "app", "apps", "assets", "auth", "billing", "blob",
  "blog", "card", "cards", "careers", "chat", "chat2", "chats", "cli", "cloud", "comet", "community", "contact",
  "dashboard", "dev", "device", "devices", "diff", "docs", "download", "downloads", "edge", "email", "enterprise",
  "faq", "feedback", "health", "help", "home", "hello", "info", "install", "jobs", "legal", "login", "logout", "mail",
  "me", "members", "mod", "moderator", "news", "null", "official", "org", "orgs", "owner", "preview", "pricing",
  "privacy", "profile", "registry", "release", "releases", "root", "runner", "sales", "security", "session",
  "sessions", "settings", "signin", "signout", "signup", "snapshot", "staff", "staging", "stats", "status", "stripe",
  "support", "sysadmin", "system", "tail", "team", "terms", "test", "undefined", "user", "username", "usernames",
  "users", "vault", "webhook", "workos", "workspace", "www",
  // Staff and the roles people trust.
  "wing", "winglee", "wingleeio", "abuse", "postmaster", "hostmaster", "webmaster", "noreply", "trust", "safety",
  "verified", "plugins", "plugin", "templates", "template", "marketplace", "store", "bot", "bots",
  // Vendors whose names an author could borrow.
  "aws", "meta", "amazon", "netflix", "nvidia", "slack", "discord", "linear", "notion", "vercel", "npm", "docker",
  "homebrew", "brew", "mozilla", "firefox", "chrome", "safari", "windows", "linux", "ubuntu", "android", "ios", "macos",
  "xcode", "vscode", "jetbrains", "cursor", "codex", "gemini", "copilot", "chatgpt", "gpt", "grok", "xai", "mistral",
  "ollama", "huggingface", "twitter", "x", "posthog", "sentry", "railway", "supabase", "paypal", "visa", "mastercard"
]);

/** Taken wherever they appear in a name: `github-team`, `the-openai`, `zeronhq`. */
const RESERVED_ANYWHERE = ["zeron", "anthropic", "openai", "github", "gitlab", "microsoft", "claude", "cloudflare"];

/** Taken at the start of a name: `google-dev`, `apple1`, `admin-bob`,
 * `support2`, `security-team`. Short or common words stay prefix-only so
 * `pineapple` and `bigapple` are still anyone's. */
const RESERVED_PREFIX = ["google", "apple", "anthropic", "workos", "stripe", "admin", "official", "moderator", "verified", "sysadmin", "wingl"];

/** Taken as a whole part between `-` or `_`: `support-bob`, `acme_security`
 * (but `supporter` and `staffing` are anyone's). */
const RESERVED_PART = new Set(["admin", "support", "security", "staff", "official", "verified", "moderator", "team", "help", "zeron"]);

/** Taken at the end: `bob-admin`, `teamsupport`, `acme-official`. */
const RESERVED_SUFFIX = ["admin", "support", "official", "staff", "security", "verified"];

/** Digits and doubled letters read as letters (`zer0n`, `g00gle`, `rn` as
 * `m`), with `-` and `_` gone: what a name looks like at a glance. */
export const skeleton = (name: string): string =>
  name
    .toLowerCase()
    .replace(/[-_.\s]/g, "")
    .replace(/0/g, "o")
    .replace(/[1|!]/g, "l")
    .replace(/3/g, "e")
    .replace(/4/g, "a")
    .replace(/5/g, "s")
    .replace(/7/g, "t")
    .replace(/8/g, "b")
    .replace(/9/g, "g")
    .replace(/vv/g, "w")
    .replace(/rn/g, "m");

/** Every way to read a name: `l` may be `i`, so `1` (folded to `l`) tests as
 * both, and `m` may be a folded `rn`. Brand words are checked against each. */
const readings = (name: string): string[] => {
  const s = skeleton(name);
  return [...new Set([s, s.replace(/l/g, "i"), s.replace(/m/g, "rn"), s.replace(/l/g, "i").replace(/m/g, "rn")])];
};

/** A brand or staff word a name (or an org's name) passes itself off as, if
 * any. Shared with the org-name check (`impersonates` in auth-routes.ts). */
export const impersonatedWord = (name: string): string | undefined => {
  for (const r of readings(name)) {
    for (const word of RESERVED_ANYWHERE) if (r.includes(word)) return word;
  }
  return undefined;
};

/** `zeron` and anything that looks like it, staff and brand words stay ours. */
export const isReservedUsername = (username: string): boolean =>
  readings(username).some(
    (r) =>
      RESERVED.has(r) ||
      RESERVED_ANYWHERE.some((w) => r.includes(w)) ||
      RESERVED_PREFIX.some((w) => r.startsWith(w)) ||
      RESERVED_SUFFIX.some((w) => r.length > w.length && r.endsWith(w))
  ) ||
  RESERVED.has(username.replace(/[-_]/g, "")) ||
  (/[-_]/.test(username) && username.split(/[-_]+/).some((part) => RESERVED_PART.has(skeleton(part))));
