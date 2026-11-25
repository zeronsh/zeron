// The reviewer's actions: what each asks, what it sends, and what it says
// after. Every one is a post with the session's token; the destructive
// ones ask first.
import type { Ask } from "./actions.ts";
import type { FlashWord, Said } from "./flash.ts";
import { count } from "./format.ts";
import type { Kind } from "./kind.ts";
import { isVersion } from "./names.ts";
import { compared, together } from "./permissions.ts";
import type { Submission, VersionInfo } from "./types.ts";

/** Why a version isn't verified: the words the author reads beside the reviewer's own (staff's too; the public reads only that it isn't). */
export const DECLINE_REASONS = [
  { value: "asks_too_much", label: "Asks for more than it explains" },
  { value: "misleading", label: "Doesn't do what it says" },
  { value: "broken", label: "Doesn't work" },
  { value: "unsafe", label: "Unsafe or harmful" },
  { value: "listing", label: "README or listing needs work" },
  { value: "copyright", label: "Uses someone else's work or name" },
  { value: "other", label: "Something else" },
] as const;

export const reasonLabel = (value: string) => DECLINE_REASONS.find((r) => r.value === value)?.label ?? value;

/** A reviewer's action on one version (amendment 45): verify, decline to, take it down (harmful: turned off on devices), or remove it (not harmful). */
export type VersionAction = "verify" | "decline" | "take_down" | "remove" | "restore";
export type PluginAction = "suspend" | "restore" | "feature" | "unfeature" | "transfer";

export const isVersionAction = (v: unknown): v is VersionAction => v === "verify" || v === "decline" || v === "take_down" || v === "remove" || v === "restore";
export const isPluginAction = (v: unknown): v is PluginAction => v === "suspend" || v === "restore" || v === "feature" || v === "unfeature" || v === "transfer";

export const VERSION_FLASH: Record<VersionAction, FlashWord> = { verify: "verified", decline: "declined", take_down: "taken_down", remove: "removed", restore: "version_restored" };
export const PLUGIN_FLASH: Record<PluginAction, FlashWord> = { suspend: "suspended", restore: "restored", feature: "featured", unfeature: "unfeatured", transfer: "transferred" };

/**
 * What a restore did, from the edge's answer to it: the version as it is
 * now. Listed again; or superseded at once, when its plugin has a newer one
 * (listed, or stopped itself: either way it takes no new installs). An
 * answer that says neither gets neither claim.
 */
export function restoredSaid(version: string, answer: unknown): Said {
  const now = (answer as { version?: Partial<VersionInfo> } | null | undefined)?.version;
  if (now?.status === "listed") return "version_restored";
  if (now?.status === "yanked" && now.yanked_by === "superseded" && isVersion(now.superseded_by)) return ["version_replaced", version, now.superseded_by];
  return "version_back";
}

/** What a reviewer's action on one version asks first: in its dialog, and as a page. */
export function versionAsk(action: VersionAction, name: string, version: string): Ask {
  const flash = VERSION_FLASH[action];
  switch (action) {
    case "verify":
      return { title: `Verify ${name} ${version}?`, words: ["Everyone installing it sees it's verified, and devices that only take verified versions are offered it."], confirm: "Verify", tone: "primary", flash };
    case "decline":
      return { title: `Don't verify ${name} ${version}?`, words: ["It stays listed, not verified. Its author reads your reason and your words, in Zeron and on the web; the public reads only that it isn't verified."], confirm: "Don't verify", tone: "danger", flash };
    case "take_down":
      return { title: `Take down ${name} ${version}?`, words: ["For a harmful version: nobody can install it again, and the devices that have it turn it off."], confirm: "Take down", tone: "danger", flash };
    case "remove":
      return { title: `Remove ${name} ${version}?`, words: ["For a version that shouldn't be in the store but isn't harmful: nobody can install it again; those who have it keep it."], confirm: "Remove", tone: "danger", flash };
    case "restore":
      return { title: `Restore ${name} ${version}?`, words: ["It's listed again, not verified, and the devices that turned it off may turn it back on. If there's a newer version, this one takes no new installs."], confirm: "Restore", tone: "primary", flash };
  }
}

/** Freeing a reserved id: what it asks first. */
export const freeAsk = (id: string): Ask => ({
  title: `Free “${id}”?`,
  words: ["Anyone can claim it then, first come. If someone claimed it and never used it, their claim goes too."],
  confirm: "Free it",
  tone: "danger",
  flash: "reservation_released",
});

/** What a reviewer's action on a whole plugin asks first. */
export function pluginAsk(action: PluginAction, name: string): Ask {
  const flash = PLUGIN_FLASH[action];
  switch (action) {
    case "suspend":
      return { title: `Suspend ${name}?`, words: ["It's gone from the store at once: no page, no installs, no updates. Anyone who has it keeps it. You can restore it."], confirm: "Suspend", tone: "danger", flash };
    case "restore":
      return { title: `Restore ${name}?`, words: ["It's back in the store with its listed versions."], confirm: "Restore", tone: "primary", flash };
    case "feature":
      return { title: `Feature ${name}?`, words: ["It shows in the store's featured row."], confirm: "Feature", tone: "primary", flash };
    case "unfeature":
      return { title: `Stop featuring ${name}?`, words: ["It leaves the featured row."], confirm: "Stop featuring", tone: "primary", flash };
    case "transfer":
      return { title: `Move ${name} to another account?`, words: ["Its new owner publishes its versions from now on. Its current owner can't, and their deploy tokens for it stop working."], confirm: "Move it", tone: "danger", flash };
  }
}

/** The edge path and body for a version's action, from the form's fields; null when one is missing. */
export function versionRequest(kind: Kind, id: string, version: string, action: VersionAction, form: { get(name: string): string }): { path: string; body: Record<string, unknown> } | null {
  const plugin = encodeURIComponent(id);
  const v = encodeURIComponent(version);
  switch (action) {
    case "verify":
      return { path: `versions/${plugin}/${v}/verify`, body: {} };
    case "decline": {
      const reason = form.get("reason");
      const message = form.get("message");
      // (The edge takes any short word for a reason, so the list is the site's; the author's words, at most 2000.)
      if (!DECLINE_REASONS.some((r) => r.value === reason) || !message || message.length > 2000) return null;
      return { path: `versions/${plugin}/${v}/decline`, body: { reason, message } };
    }
    case "take_down":
    case "remove":
    case "restore": {
      const reason = form.get("reason");
      if (!reason) return null;
      return { path: `${kind}s/${plugin}/versions/${v}/${action === "take_down" ? "take-down" : action}`, body: { reason } };
    }
  }
}

/** The edge path and body for a plugin's action; null when a field is missing. */
export function pluginRequest(kind: Kind, id: string, action: PluginAction, form: { get(name: string): string }): { path: string; body: Record<string, unknown> } | null {
  const plugin = encodeURIComponent(id);
  switch (action) {
    case "suspend": {
      const reason = form.get("reason");
      return reason ? { path: `${kind}s/${plugin}/suspend`, body: { reason } } : null;
    }
    case "restore":
      return { path: `${kind}s/${plugin}/restore`, body: {} };
    case "feature":
      return { path: `${kind}s/${plugin}/feature`, body: { featured: true } };
    case "unfeature":
      return { path: `${kind}s/${plugin}/feature`, body: { featured: false } };
    case "transfer": {
      const to = form.get("to_username").replace(/^@/, "").toLowerCase();
      return /^[a-z0-9][a-z0-9_-]{2,19}$/.test(to) ? { path: `${kind}s/${plugin}/transfer`, body: { to_username: to } } : null;
    }
  }
}

/**
 * What there is to weigh before verifying, said on one line ("2 new asks ·
 * 1 check to look at"), in the danger's ink when a check failed or what
 * moved reaches far, else the warning's; null when there is nothing.
 * Against the newest verified other version when there is one; else its
 * far-reaching asks.
 */
const capital = (text: string) => text.charAt(0).toUpperCase() + text.slice(1);

export function toWeigh(s: Pick<Submission, "permissions" | "permissions_diff" | "base" | "checks">): { text: string; tone: "warn" | "bad" } | null {
  // (A template asks for nothing: its checks, and its files, are what there is.)
  const { lines } = s.permissions ? compared(s.permissions, s.base ? s.permissions_diff : null) : { lines: [] };
  const fresh = lines.filter((l) => l.change === "new");
  const changed = lines.filter((l) => l.change === "changed" && !l.bound);
  const bound = lines.filter((l) => l.bound).map((l) => l.bound!);
  const far = s.base ? [...fresh, ...changed].filter((l) => l.risky).length + bound.length : lines.filter((l) => l.risky).length;
  const failed = s.checks.filter((c) => c.level === "fail").length;
  const warned = s.checks.filter((c) => c.level === "warn").length;
  const say = (n: number, one: string, many: string) => (n ? [`${count(n)} ${n === 1 ? one : many}`] : []);
  const parts = [
    ...(s.base
      ? [...say(fresh.length, "new ask", "new asks"), ...say(changed.length, "ask changed", "asks changed"), ...(bound.length ? [`${capital(together(bound))} changed`] : [])]
      : say(far, "ask that reaches far", "asks that reach far")),
    ...say(failed, "check failed", "checks failed"),
    ...say(warned, "check to look at", "checks to look at"),
  ];
  return parts.length ? { text: parts.join(" · "), tone: failed || far ? "bad" : "warn" } : null;
}

