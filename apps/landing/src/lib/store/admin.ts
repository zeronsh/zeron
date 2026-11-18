// The reviewer's actions: what each asks, what it sends, and what it says
// after. Every one is a post with the session's token; the destructive
// ones ask first.
import type { Ask } from "./actions.ts";
import type { FlashWord } from "./flash.ts";
import { count } from "./format.ts";
import { compared, together } from "./permissions.ts";
import type { Submission } from "./types.ts";

/** Why a version isn't approved: the words the author reads beside the reviewer's own. */
export const REJECT_REASONS = [
  { value: "asks_too_much", label: "Asks for more than it explains" },
  { value: "misleading", label: "Doesn't do what it says" },
  { value: "broken", label: "Doesn't work" },
  { value: "unsafe", label: "Unsafe or harmful" },
  { value: "listing", label: "README or listing needs work" },
  { value: "copyright", label: "Uses someone else's work or name" },
  { value: "other", label: "Something else" },
] as const;

export const reasonLabel = (value: string) => REJECT_REASONS.find((r) => r.value === value)?.label ?? value;

export type VersionAction = "approve" | "reject" | "remove";
export type PluginAction = "suspend" | "restore" | "feature" | "unfeature" | "transfer";

export const isVersionAction = (v: unknown): v is VersionAction => v === "approve" || v === "reject" || v === "remove";
export const isPluginAction = (v: unknown): v is PluginAction => v === "suspend" || v === "restore" || v === "feature" || v === "unfeature" || v === "transfer";

export const VERSION_FLASH: Record<VersionAction, FlashWord> = { approve: "approved", reject: "rejected", remove: "removed" };
export const PLUGIN_FLASH: Record<PluginAction, FlashWord> = { suspend: "suspended", restore: "restored", feature: "featured", unfeature: "unfeatured", transfer: "transferred" };

/** What a reviewer's action on one version asks first: in its dialog, and as a page. */
export function versionAsk(action: VersionAction, name: string, version: string): Ask {
  const flash = VERSION_FLASH[action];
  switch (action) {
    case "approve":
      return { title: `Approve ${name} ${version}?`, words: ["Anyone can install it once approved."], confirm: "Approve", tone: "primary", flash };
    case "reject":
      return { title: `Don't approve ${name} ${version}?`, words: ["Its author reads your reason and your words, in Zeron and on the web, and can send a new version."], confirm: "Don't approve", tone: "danger", flash };
    case "remove":
      return { title: `Take down ${name} ${version}?`, words: ["Nobody can install it from the store again. Choose whether the devices that have it turn it off too."], confirm: "Take down", tone: "danger", flash };
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
      return { title: `Restore ${name}?`, words: ["It's back in the store with its approved versions."], confirm: "Restore", tone: "primary", flash };
    case "feature":
      return { title: `Feature ${name}?`, words: ["It shows in the store's featured row."], confirm: "Feature", tone: "primary", flash };
    case "unfeature":
      return { title: `Stop featuring ${name}?`, words: ["It leaves the featured row."], confirm: "Stop featuring", tone: "primary", flash };
    case "transfer":
      return { title: `Move ${name} to another account?`, words: ["Its new owner publishes its versions from now on. Its current owner can't, and their deploy keys for it stop working."], confirm: "Move it", tone: "danger", flash };
  }
}

/** The edge path and body for a version's action, from the form's fields; null when one is missing. */
export function versionRequest(id: string, version: string, action: VersionAction, form: { get(name: string): string }): { path: string; body: Record<string, unknown> } | null {
  const plugin = encodeURIComponent(id);
  const v = encodeURIComponent(version);
  switch (action) {
    case "approve":
      return { path: `submissions/${plugin}/${v}/approve`, body: {} };
    case "reject": {
      const reason = form.get("reason");
      const message = form.get("message");
      if (!REJECT_REASONS.some((r) => r.value === reason) || !message) return null;
      return { path: `submissions/${plugin}/${v}/reject`, body: { reason, message } };
    }
    case "remove": {
      const reason = form.get("reason");
      if (!reason) return null;
      return { path: `plugins/${plugin}/versions/${v}/remove`, body: { reason, block: form.get("block") === "on" } };
    }
  }
}

/** The edge path and body for a plugin's action; null when a field is missing. */
export function pluginRequest(id: string, action: PluginAction, form: { get(name: string): string }): { path: string; body: Record<string, unknown> } | null {
  const plugin = encodeURIComponent(id);
  switch (action) {
    case "suspend": {
      const reason = form.get("reason");
      return reason ? { path: `plugins/${plugin}/suspend`, body: { reason } } : null;
    }
    case "restore":
      return { path: `plugins/${plugin}/restore`, body: {} };
    case "feature":
      return { path: `plugins/${plugin}/feature`, body: { featured: true } };
    case "unfeature":
      return { path: `plugins/${plugin}/feature`, body: { featured: false } };
    case "transfer": {
      const to = form.get("to_username").replace(/^@/, "").toLowerCase();
      return /^[a-z0-9][a-z0-9_-]{2,19}$/.test(to) ? { path: `plugins/${plugin}/transfer`, body: { to_username: to } } : null;
    }
  }
}

/**
 * What there is to weigh before approving, said on one line ("2 new asks ·
 * 1 check to look at"), in the danger's ink when a check failed or what
 * moved reaches far, else the warning's; null when there is nothing.
 * Against the last approved version for an update; for a first version,
 * its far-reaching asks.
 */
const capital = (text: string) => text.charAt(0).toUpperCase() + text.slice(1);

export function toWeigh(s: Pick<Submission, "permissions" | "permissions_diff" | "base" | "checks">): { text: string; tone: "warn" | "bad" } | null {
  const { lines } = compared(s.permissions, s.base ? s.permissions_diff : null);
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

