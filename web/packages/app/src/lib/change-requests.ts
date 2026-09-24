import type { ChangeRequestState, ChangeRequestSummary } from "@zeron/proto";

/**
 * Pure helpers for change-request display: tone mapping and the badge model.
 *
 * There is no create-page URL builder: the desktop has no create flow at all
 * (no wire `CreateChangeRequest`, no button), so guessing a provider's compare
 * URL was web-only invention and is gone.
 */

export type BadgeTone = "open" | "merged" | "closed";

export interface BadgeModel {
  readonly number: string;
  readonly stateLabel: string;
  readonly title: string;
  readonly tone: BadgeTone;
}

const STATE_LABEL: Readonly<Record<ChangeRequestState, string>> = {
  open: "Open",
  merged: "Merged",
  closed: "Closed",
};

export function badgeModel(summary: ChangeRequestSummary): BadgeModel {
  return {
    number: `#${summary.number}`,
    stateLabel: STATE_LABEL[summary.state],
    title: summary.title.replace(/[\r\n]+/g, " "),
    tone: toneFor(summary.state),
  };
}

export function toneFor(state: ChangeRequestState): BadgeTone {
  switch (state) {
    case "open":
      return "open";
    case "merged":
      return "merged";
    case "closed":
      return "closed";
  }
}

const PROVIDER_KEYS: Readonly<Record<string, string>> = {
  github: "github",
  gitlab: "gitlab",
  bitbucket: "bitbucket",
  azuredevops: "azuredevops",
  codeberg: "codeberg",
};

/** The provider keys `normalizeProvider` recognizes. */
export const PROVIDERS: readonly string[] = Object.keys(PROVIDER_KEYS);

function providerKey(provider: string): string {
  return PROVIDER_KEYS[provider.toLowerCase()] ?? provider.toLowerCase();
}

/**
 * Normalize a provider string the engine may have produced (e.g.
 * `"GitHub"`, `"github"`, or an unknown host like `"gitlab.example.com"`).
 * Returns the lower-cased key the URL builder switches on, or `null`
 * when the string is empty or otherwise unusable. Use this when threading
 * the provider from a `ChangeRequestSummary.provider` (engine-detected
 * from the checkout's remote URL) into the create-URL helper.
 */
export function normalizeProvider(provider: string | null | undefined): string | null {
  if (provider === null || provider === undefined) {
    return null;
  }
  const trimmed = provider.trim();
  if (trimmed.length === 0) {
    return null;
  }
  return providerKey(trimmed);
}

/**
 * The chat-row badge copy — when no PR has been observed yet for this
 * chat, return `null`; otherwise return the badge model the chat header
 * renders. Kept here so the chat header and the changes surface share
 * the same derivation.
 */
export function changeRequestForBadge(summary: ChangeRequestSummary | null): BadgeModel | null {
  return summary === null ? null : badgeModel(summary);
}