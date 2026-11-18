// A version's state and its verification, as an author and a reviewer read
// them (amendment 45: publishing is immediate, review verifies). Two things
// a version says, never one: where it stands (listed, no new installs,
// taken down, removed) and whether a reviewer verified it.
import type { Listing, PluginStatus, Review, VersionInfo } from "./types.ts";
import { statusWords, store } from "./words.ts";

/** A badge's tone: its ink on its wash (app.css .badge, by toneClass). */
export type Tone = "neutral" | "accent" | "info" | "success" | "warning" | "danger";

export interface State {
  label: string;
  tone: Tone;
  about: string;
}

/** Where a version stands. */
export function versionState(v: Pick<VersionInfo, "status"> & Pick<Partial<VersionInfo>, "yanked_by" | "superseded">): State {
  switch (v.status) {
    case "listed":
      return { label: statusWords("listed"), tone: "success", about: "Anyone can see and install it." };
    case "yanked":
      // (Superseded: a newer version was published while this one wasn't verified, amendment 45.)
      return v.yanked_by === "superseded"
        ? { label: statusWords("yanked"), tone: "accent", about: `${replaced(v)}. Those who have it keep it working; verifying it would allow installs again.` }
        : { label: statusWords("yanked"), tone: "accent", about: "Nobody can install it anymore. Those who have it keep it working." };
    case "taken_down":
      return { label: statusWords("taken_down"), tone: "danger", about: "Taken down by the store's reviewers as harmful: turned off on the devices that had it." };
    case "removed":
      return { label: statusWords("removed"), tone: "danger", about: "Removed by the store's reviewers. Nobody can install it; those who have it keep it." };
    default:
      // A state this site doesn't know yet: named as the edge says it.
      return { label: reasonWords(String(v.status)), tone: "neutral", about: "" };
  }
}

/**
 * Whether Zeron verified it (the contract's "Who does what, in words":
 * Zeron verifies, a reviewer reads). Three states: Verified; Not verified
 * yet (no decision, and all the public ever reads of one unverified); Not
 * verified (declined: its author's and staff's, by `review`, only).
 */
export function verificationState(v: Pick<VersionInfo, "verified"> & { review?: Review }): State {
  if (v.verified) return { label: store("plugins.store.verified"), tone: "success", about: "Zeron verified this version." };
  if (v.review?.decision === "declined") return { label: store("plugins.store.not_verified"), tone: "warning", about: "Zeron didn't verify it: the reviewer's note is below. It stays listed." };
  // (The app's word for the state; the site's own line for who it waits for: the contract's "Who does what, in words".)
  if (v.review && !v.review.decided) return { label: store("plugins.store.review.waiting"), tone: "info", about: "Waiting for a reviewer." };
  return { label: store("plugins.store.review.waiting"), tone: "neutral", about: "Zeron hasn't verified this version yet." };
}

/** The app's sentence for a superseded version (its replacer from the public field or the owner's). */
const replaced = (v: Pick<Partial<VersionInfo>, "superseded_by" | "superseded">) => {
  const by = v.superseded_by ?? v.superseded?.by;
  return by ? store("plugins.store.superseded", { version: by }) : "Replaced by a newer version before it was verified: no new installs";
};

/**
 * Why a version takes no new installs, for anyone: replaced before it was
 * verified, or stopped by its author. An edge that doesn't say why (before
 * dc45c3606) gets neither claim.
 */
export function stoppedWhy(v: Pick<Partial<VersionInfo>, "yanked_by" | "superseded_by" | "superseded">): string {
  if (v.yanked_by === "superseded") return `${replaced(v)}. Those who have it keep it working.`;
  if (v.yanked_by === "author") return "Its author stopped new installs of it. Those who have it keep it working.";
  return "It takes no new installs. Those who have it keep it working.";
}

/** Whether a plugin was ever listed: a version of it was (since stopped or taken down, maybe). */
export const everListed = (versions: readonly Pick<VersionInfo, "status">[]) => versions.length > 0;

/**
 * Its place in the store, in three words while it isn't suspended: Listed;
 * Not listed yet (never was); Not listed (was, and isn't now: why is its
 * versions' to say). `wasListed`: whether it ever was (everListed).
 */
export function listingState(listing: Listing | PluginStatus | "active", wasListed = false): { label: string; tone: Tone } {
  switch (listing) {
    case "listed":
    case "active":
      return { label: "Listed", tone: "success" };
    case "unlisted":
      return { label: wasListed ? "Not listed" : "Not listed yet", tone: "neutral" };
    case "suspended":
      return { label: "Suspended", tone: "danger" };
    case "removed":
      return { label: "Removed", tone: "danger" };
    default:
      return { label: reasonWords(String(listing)), tone: "neutral" };
  }
}

/** A reviewer's reason word, said plainly ("network_unexplained" → "Network unexplained"). */
export const reasonWords = (reason: string) => {
  const words = reason.replace(/[_-]+/g, " ").trim();
  return words.charAt(0).toUpperCase() + words.slice(1);
};

/** A tone as the app's stylesheet names its badge (styles/app.css). */
export const toneClass = (tone: Tone): string => ({ neutral: "", accent: "accent", info: "info", success: "ok", warning: "warn", danger: "bad" })[tone];
