// A version's state, as an author and a reviewer read it (named as the app names it: words.ts).
// Six states, six tones, none shared: in review (info), approved (success),
// not approved (warning: the author's to fix), withdrawn (neutral), no new
// installs (accent), taken down (danger).
import type { Listing, PluginStatus, VersionInfo } from "./types.ts";
import { statusWords } from "./words.ts";

/** A badge's tone: its ink on its wash (app.css .badge, by toneClass). */
export type Tone = "neutral" | "accent" | "info" | "success" | "warning" | "danger";

export function versionState(v: Pick<VersionInfo, "status" | "review">): { label: string; tone: Tone; about: string } {
  switch (v.status) {
    case "pending":
      return { label: statusWords("pending"), tone: "info", about: "Waiting for a reviewer. Your own devices can install it now; nobody else's can yet." };
    case "approved":
      return { label: statusWords("approved"), tone: "success", about: "Anyone can install it." };
    case "rejected":
      return { label: statusWords("rejected"), tone: "warning", about: "Send a new version with the changes the reviewer asked for." };
    case "withdrawn":
      return { label: statusWords("withdrawn"), tone: "neutral", about: "Taken out of review by its author." };
    case "yanked":
      return { label: statusWords("yanked"), tone: "accent", about: "Nobody can install it anymore. Those who have it keep it working." };
    case "removed":
      return { label: statusWords("removed"), tone: "danger", about: "Taken down by the store's reviewers: turned off on the devices that had it." };
    default:
      // A state this site doesn't know yet: named as the edge says it.
      return { label: reasonWords(String(v.status)), tone: "neutral", about: "" };
  }
}

/** Whether a plugin was ever listed: a version of it was approved once (since stopped or taken down, maybe). */
export const everListed = (versions: readonly Pick<VersionInfo, "status">[]) => versions.some((v) => v.status === "approved" || v.status === "yanked" || v.status === "removed");

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
