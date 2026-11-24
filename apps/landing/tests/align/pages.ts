// Every page of the look, in its full states and its spare ones: who's
// looking, the fake's switches for it, and what to do on it first.
import type { Page } from "playwright";

export interface AlignPage {
  /** A short name for the sheet and the failures. */
  name: string;
  path: string;
  /** Signed in as this fake account. */
  user?: string;
  /** The fake's switches, set before and reset after (POST /_fake/<what>?<params>). */
  fake?: [string, Record<string, string>][];
  /** What to do on it before it's measured (open a dialog, send a form). */
  act?: (page: Page) => Promise<void>;
}

/** New token's dialog, filled in, its plugins chosen. */
const newToken = async (page: Page) => {
  await page.getByRole("link", { name: "New token" }).click();
  const dialog = page.getByRole("dialog", { name: "New deploy token" });
  await dialog.getByLabel("Name", { exact: true }).fill("Release CI");
  await dialog.getByLabel("Chosen plugins").check();
  await dialog.getByLabel("Focus timer", { exact: true }).check();
};

/**
 * Where the rule can't hold, and why: a code view's line numbers stand right-aligned in their gutter (the
 * longest starts where the shorter end), and a diff's hunk heads sit in its table, a code view of its own.
 */
export const EXCEPTIONS: [string, string][] = [
  ["pre.numbers, td.n, .code > .n", "line numbers, right-aligned in a code view's gutter"],
  ["tr.hunk", "a diff's hunk head, in the code view's own table"],
  ["li li", "a list inside a list item steps in under it, as its level"],
  [".stage", "a screenshot set into its card's coloured stage: a picture, placed by the stage"],
  ["p code, li > code:not(:first-child)", "code inside a sentence that wraps to a line's start keeps its chip's 5 px"],
  ["pre > code", "a code block's lines start where the code puts them: its indentation is the code's"],
  [".guide :not(pre) > code", "in the guide a literal is a word in its chip wherever it falls, a table's field and a line's start too"],
  [".step-n", "a step's number hangs in the gutter, left of its title (Tailwind's steps)"],
  [".anchor", "a heading's # hangs in the gutter, left of its words"],
  [".head .get", "getting a plugin is set at the head's right: its lines end on the page's right edge (align.test.ts, the Get test, holds it)"],
  [".hero", "the store's hero is centred, its title, line and search on the page's middle, not its edge (Raycast's, Vercel's)"],
  [".make", "the store's ending is one centred block"],
  [".empty", "the empty block is centred in its area"],
  [".cb-label", "a code block's file name as typed: `.github/…` begins with a dot, its ink mid-cell"],
];

/** The columns of the look: the page's, the store's two (its sidebar, its list), a plugin's rail, Files' two, a dialog's. */
export const COLUMNS = [".wrap", ".facets", ".listing", ".rail", ".tree-pane", ".viewer", "dialog.question"];

export const ALIGN_PAGES: AlignPage[] = [
  // The store: unnarrowed (Featured over the list), in another order, narrowed, nothing found, empty, down.
  { name: "store", path: "/plugins" },
  { name: "store-verified", path: "/plugins?sort=verified" },
  { name: "store-category", path: "/plugins?category=productivity&keyword=focus" },
  { name: "store-none", path: "/plugins?q=nothing-matches-this" },
  { name: "store-none-narrowed", path: "/plugins?category=git&keyword=focus&verified=1&q=nothing" },
  { name: "store-empty", path: "/plugins", fake: [["empty", { on: "1" }]] },
  { name: "store-down", path: "/plugins", fake: [["down", { on: "1" }]] },
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
  { name: "tokens", path: "/account/tokens", user: "user_ada" },
  { name: "tokens-none", path: "/account/tokens", user: "user_noor" },
  { name: "tokens-none-author", path: "/account/tokens", user: "user_kai" },
  { name: "tokens-signed-out", path: "/account/tokens" },
  { name: "new-token-no-plugins", path: "/account/tokens/new", user: "user_noor" },
  { name: "revoke", path: "/account/tokens/tok_0OZtGptZyNgbeUuc", user: "user_ada" },
  { name: "new-token", path: "/account/tokens/new?again=1&name=Nightly&ends=365&scope=chosen&plugins=focus-timer%2Cweather", user: "user_ada" },
  { name: "new-token-dialog", path: "/account/tokens", user: "user_ada", act: newToken },
  { name: "token-shown", path: "/account/tokens", user: "user_ada", fake: [["tokens", {}]], act: async (page) => {
    await newToken(page);
    await Promise.all([page.waitForLoadState("networkidle"), page.getByRole("dialog").getByRole("button", { name: "Create token" }).click()]);
    await page.getByRole("heading", { name: "Copy your token now" }).waitFor();
  } },
  { name: "mine", path: "/account/plugins/worst-case-listing-at-every-limit-xyz12", user: "user_ada" },
  { name: "mine-stop", path: "/account/plugins/session-badges/versions/1.2.1/yank", user: "user_ada" },
  // Review.
  { name: "queue", path: "/admin/plugins", user: "user_grace" },
  { name: "queue-empty", path: "/admin/plugins", user: "user_grace", fake: [["queue", { as: "empty" }]] },
  ...(["verified", "declined", "taken_down"] as const).map((state) => ({ name: `queue-empty-${state}`, path: `/admin/plugins?state=${state}`, user: "user_grace", fake: [["queue", { as: "empty" }]] as [string, Record<string, string>][] })),
  ...(["reports", "reservations", "audit"] as const).map((list) => ({ name: `staff-${list}-empty`, path: `/admin/${list}`, user: "user_grace", fake: [["queue", { as: "empty" }]] as [string, Record<string, string>][] })),
  { name: "review", path: "/admin/plugins/worst-case-listing-at-every-limit-xyz12/1.5.0", user: "user_grace" },
  { name: "review-first", path: "/admin/plugins/first-light/0.1.0", user: "user_grace" },
  { name: "review-files", path: "/admin/plugins/xss-probe/1.1.0?view=files", user: "user_grace" },
  { name: "review-changes", path: "/admin/plugins/xss-probe/1.1.0?view=changes", user: "user_grace" },
  // The pages that were on the old app window: the guide, a report, the staff's lists and questions.
  { name: "guide", path: "/plugins/publishing" },
  { name: "report", path: "/plugins/focus-timer/report", user: "user_kai" },
  { name: "staff-reports", path: "/admin/reports", user: "user_grace" },
  { name: "staff-reports-closed", path: "/admin/reports?status=actioned", user: "user_grace" },
  { name: "staff-reservations", path: "/admin/reservations", user: "user_grace" },
  { name: "staff-reservation", path: "/admin/reservations/zeron-tools", user: "user_grace" },
  { name: "staff-log", path: "/admin/audit", user: "user_grace" },
  { name: "staff-log-plugin", path: "/admin/audit?plugin=focus-timer", user: "user_grace" },
  { name: "staff-plugin", path: "/admin/plugins/focus-timer", user: "user_grace" },
  { name: "staff-plugin-suspended", path: "/admin/plugins/crypto-ticker", user: "user_grace" },
  { name: "staff-suspend", path: "/admin/plugins/focus-timer/actions/suspend", user: "user_grace" },
  { name: "staff-transfer", path: "/admin/plugins/focus-timer/actions/transfer", user: "user_grace" },
  { name: "staff-decline", path: "/admin/plugins/session-badges/1.2.1/decline", user: "user_grace" },
  { name: "staff-verify", path: "/admin/plugins/session-badges/1.2.1/verify", user: "user_grace" },
];

export const RESET: [string, Record<string, string>][] = [["empty", {}], ["down", {}], ["bare", {}], ["queue", {}], ["tokens", {}]];
