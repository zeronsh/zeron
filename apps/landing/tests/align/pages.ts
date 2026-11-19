// Every page of the look, in its full states and its spare ones: who's
// looking, the fake's switches for it, and what to do on it first.

export interface AlignPage {
  /** A short name for the sheet and the failures. */
  name: string;
  path: string;
  /** Signed in as this fake account. */
  user?: string;
  /** The fake's switches, set before and reset after (POST /_fake/<what>?<params>). */
  fake?: [string, Record<string, string>][];
}

/**
 * Where the rule can't hold, and why: a code view's line numbers stand right-aligned in their gutter (the
 * longest starts where the shorter end), and a diff's hunk heads sit in its table, a code view of its own.
 */
export const EXCEPTIONS: [string, string][] = [
  ["pre.numbers, td.n", "line numbers, right-aligned in a code view's gutter"],
  ["tr.hunk", "a diff's hunk head, in the code view's own table"],
  ["li li", "a list inside a list item steps in under it, as its level"],
  [".stage", "a screenshot set into its card's coloured stage: a picture, placed by the stage"],
  ["p code, li > code:not(:first-child)", "code inside a sentence that wraps to a line's start keeps its chip's 5 px"],
];

/** The columns of the look: the page's, Browse's two, a plugin's rail and its Get, Files' two. */
export const COLUMNS = [".wrap", ".facets", ".browse > section", ".rail", ".get", ".tree-pane", ".viewer"];

export const ALIGN_PAGES: AlignPage[] = [
  // Discover, Browse.
  { name: "discover", path: "/plugins" },
  { name: "discover-empty", path: "/plugins", fake: [["empty", { on: "1" }]] },
  { name: "discover-down", path: "/plugins", fake: [["down", { on: "1" }]] },
  { name: "browse", path: "/plugins/all" },
  { name: "browse-category", path: "/plugins/all?category=productivity&keyword=focus" },
  { name: "browse-none", path: "/plugins/all?q=nothing-matches-this" },
  // A plugin: every part; none of its optional parts; its versions, its asks, one version.
  { name: "plugin", path: "/plugins/focus-timer" },
  { name: "plugin-worst", path: "/plugins/worst-case-listing-at-every-limit-xyz12" },
  { name: "plugin-bare", path: "/plugins/first-light" },
  { name: "plugin-no-pictures", path: "/plugins/focus-timer", fake: [["bare", { id: "focus-timer" }]] },
  { name: "versions", path: "/plugins/focus-timer/versions" },
  { name: "versions-one", path: "/plugins/first-light/versions" },
  { name: "asks", path: "/plugins/worst-case-listing-at-every-limit-xyz12/asks" },
  { name: "version", path: "/plugins/focus-timer/versions/1.2.0" },
  { name: "version-not-verified", path: "/plugins/worst-case-listing-at-every-limit-xyz12/versions/1.5.0" },
  { name: "not-here", path: "/plugins/no-such-plugin" },
  // Yours.
  { name: "yours", path: "/account", user: "user_ada" },
  { name: "yours-none", path: "/account", user: "user_noor" },
  { name: "yours-signed-out", path: "/account" },
  { name: "keys", path: "/account/keys", user: "user_ada" },
  { name: "keys-none", path: "/account/keys", user: "user_noor" },
  { name: "revoke", path: "/account/keys/Qm81LzR2sT3uV4wX5yZ6aB7cD8eF9gH0iJ1kL2mN3oP", user: "user_ada" },
  { name: "mine", path: "/account/plugins/worst-case-listing-at-every-limit-xyz12", user: "user_ada" },
  { name: "mine-stop", path: "/account/plugins/session-badges/versions/1.2.1/yank", user: "user_ada" },
  // Review.
  { name: "queue", path: "/admin/plugins", user: "user_grace" },
  { name: "queue-empty", path: "/admin/plugins", user: "user_grace", fake: [["queue", { as: "empty" }]] },
  { name: "review", path: "/admin/plugins/worst-case-listing-at-every-limit-xyz12/1.5.0", user: "user_grace" },
  { name: "review-first", path: "/admin/plugins/first-light/0.1.0", user: "user_grace" },
  { name: "review-files", path: "/admin/plugins/xss-probe/1.1.0?view=files", user: "user_grace" },
  { name: "review-changes", path: "/admin/plugins/xss-probe/1.1.0?view=changes", user: "user_grace" },
];

export const RESET: [string, Record<string, string>][] = [["empty", {}], ["down", {}], ["bare", {}], ["queue", {}]];
