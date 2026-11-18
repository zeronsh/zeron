// The reviewer's actions: what each asks, what it sends, and what it says
// after. Every one is a post with the session's token; the destructive
// ones ask first.
import type { FlashWord } from "./flash.ts";

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
