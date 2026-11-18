// A version's state, as an author and a reviewer read it.
import type { Tone } from "../../components/ui/Badge.astro";
import type { Listing, PluginStatus, VersionInfo } from "./types.ts";

export function versionState(v: Pick<VersionInfo, "status" | "review">): { label: string; tone: Tone; about: string } {
  switch (v.status) {
    case "pending":
      return { label: "In review", tone: "warning", about: "Waiting for a reviewer. Your own devices can install it now; nobody else's can yet." };
    case "approved":
      return { label: "Approved", tone: "success", about: "Anyone can install it." };
    case "rejected":
      return { label: "Not approved", tone: "danger", about: "Submit a new version with the changes the reviewer asked for." };
    case "withdrawn":
      return { label: "Withdrawn", tone: "neutral", about: "Taken out of review by its author." };
    case "yanked":
      return { label: "No longer offered", tone: "neutral", about: "Nobody new gets it. Anyone who has it keeps it." };
    case "removed":
      return { label: "Taken down", tone: "danger", about: "Removed from the store by its reviewers." };
  }
}

export function listingState(listing: Listing | PluginStatus): { label: string; tone: Tone } {
  switch (listing) {
    case "listed":
      return { label: "Listed", tone: "success" };
    case "unlisted":
      return { label: "Not listed yet", tone: "neutral" };
    case "suspended":
      return { label: "Suspended", tone: "danger" };
    case "removed":
      return { label: "Removed", tone: "danger" };
  }
}

/** A reviewer's reason word, said plainly ("network_unexplained" → "Network unexplained"). */
export const reasonWords = (reason: string) => {
  const words = reason.replace(/[_-]+/g, " ").trim();
  return words.charAt(0).toUpperCase() + words.slice(1);
};
