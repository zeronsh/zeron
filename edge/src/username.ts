/**
 * Username rules: 3–20 ASCII letters, digits, `_` or `-`, starting with a
 * letter or digit. Stored and served lowercase, so `Wing` and `wing` are one
 * name and a card has one URL.
 */

const USERNAME_RE = /^[A-Za-z0-9][A-Za-z0-9_-]{2,19}$/;

/** The stored form of a name, or undefined if it breaks the rules. The check
 * comes first: `toLowerCase` folds non-ASCII letters into ASCII ones (the
 * Kelvin sign into `k`). */
export const foldUsername = (raw: unknown): string | undefined =>
  typeof raw === "string" && USERNAME_RE.test(raw) ? raw.toLowerCase() : undefined;

/** Our routes and words a member could pass off as us. Compared without `-`
 * and `_`, so `ad-min` is `admin`. */
const RESERVED = new Set([
  "about", "account", "accounts", "admin", "administrator", "api", "app", "apps", "assets", "auth", "billing", "blob",
  "blog", "card", "cards", "careers", "chat", "chat2", "chats", "cli", "cloud", "comet", "community", "contact",
  "dashboard", "dev", "device", "devices", "diff", "docs", "download", "downloads", "edge", "email", "enterprise",
  "faq", "feedback", "health", "help", "home", "hello", "info", "install", "jobs", "legal", "login", "logout", "mail",
  "me", "members", "mod", "moderator", "news", "null", "official", "org", "orgs", "owner", "preview", "pricing",
  "privacy", "profile", "registry", "release", "releases", "root", "runner", "sales", "security", "session",
  "sessions", "settings", "signin", "signout", "signup", "snapshot", "staff", "staging", "stats", "status", "stripe",
  "support", "sysadmin", "system", "tail", "team", "terms", "test", "undefined", "user", "username", "usernames",
  "users", "vault", "webhook", "workos", "workspace", "www"
]);

/** `zeron` and anything that starts with it stays ours. */
export const isReservedUsername = (username: string): boolean => {
  const bare = username.replace(/[-_]/g, "");
  return bare.startsWith("zeron") || RESERVED.has(bare);
};
