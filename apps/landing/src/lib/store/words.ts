// What the plugin store calls things, in the app's words: an author who
// stops new installs of a version in the app's Yours tab and then opens
// their account on the web meets the same action, the same states, the
// same questions. Every string here is the app's English
// (apps/zgx/ui/locales/en.json, `plugins.store.*`), held to it by
// words.test.ts, with the web's one difference (ON_THE_WEB: the web isn't
// "this computer").
import type { ReportReason, VersionStatus } from "./types.ts";

export const STORE_WORDS = {
  // The store's mark beside a version a reviewer verified (amendment 45), and the list's filter for them.
  "plugins.store.verified": "Verified",
  "plugins.store.verified_only": "Verified only",
  // The list's order, named.
  "plugins.store.order_label": "Order",
  // A version not verified, and what the plugin's page says of it (amendment 45).
  "plugins.store.not_verified": "Not verified",
  "plugins.store.not_verified_line": "{version} isn't verified yet. {verified} is.",
  "plugins.store.install_instead": "Install {version} instead",
  "plugins.store.install_version": "Install {version}",
  "plugins.store.none_verified": "No version of this plugin is verified yet.",
  "plugins.store.verified_on": "Verified on {date}",
  // A version's state.
  "plugins.store.status.yanked": "No new installs",
  "plugins.store.status.taken_down": "Taken down",
  "plugins.store.status.removed": "Removed",
  // Why a version takes no new installs, when a newer one replaced it unverified (amendment 45); and one waiting.
  "plugins.store.superseded": "Replaced by {version} before it was verified",
  "plugins.store.review.waiting": "Not verified yet",
  "plugins.store.review.declined": "Not verified",
  "plugins.store.review.declined_said": "The reviewer's note: “{words}”",
  // Your plugins' line under a newest version (the app's Yours).
  "plugins.store.summary.newest_verified": "{version} is the newest verified",
  "plugins.store.summary.none_verified": "nothing verified yet",
  // What a deploy key may publish, and when it was used (the app's keys).
  "plugins.store.key.scope.label": "Can publish",
  "plugins.store.key.scope.two": "{a} and {b}",
  "plugins.store.key.not_used_made": "Not used yet · made {date}",
  // The account's deploy keys (amendment 44).
  "plugins.store.keys_body": "A deploy key lets a CI job, such as a GitHub Action, publish new versions of your plugins without your session. Each key publishes the plugins chosen when it's made.",
  "plugins.store.key.scope.all": "All your plugins",
  "plugins.store.key.last_used_on": "Last used {date} on {plugin}",
  // What its author can do to it, and the question each asks first.
  "plugins.store.action.yank": "Stop new installs",
  "plugins.store.action.unyank": "Allow installs again",
  "plugins.store.confirm.yank.title": "Stop new installs of {name} {version}?",
  "plugins.store.confirm.yank.body": "Nobody can install it anymore. Those who have it keep it working. You can allow installs again later.",
  "plugins.store.confirm.unyank.title": "Allow installs of {name} {version}?",
  "plugins.store.confirm.unyank.body": "It can be installed again.",
  // Deploy keys.
  "plugins.store.keys": "Deploy keys",
  "plugins.store.keys_none": "No keys yet.",
  "plugins.store.keys_failed": "Couldn't see your keys",
  "plugins.store.key.new": "New key…",
  "plugins.store.key.new_title": "New deploy key",
  "plugins.store.key.name": "Name",
  "plugins.store.key.not_used": "Not used yet",
  "plugins.store.key.no_end": "Doesn't expire",
  "plugins.store.key.ends_in": "{n, plural, one {Ends in # day} other {Ends in # days}}",
  "plugins.store.key.ends_today": "Ends today",
  "plugins.store.key.ends_tomorrow": "Ends tomorrow",
  "plugins.store.key.expiry_label": "Ends",
  "plugins.store.key.expiry.never": "Never",
  "plugins.store.your_versions": "Versions you've sent",
  "plugins.store.no_versions": "Nothing sent yet.",
  "plugins.store.key.revoke": "Revoke",
  "plugins.store.key.revoke_title": "Revoke {name}?",
  "plugins.store.key.revoke_body": "Anything publishing with it stops working at once. Versions it already sent stay as they are.",
  // Reporting a plugin.
  "plugins.store.report.link": "Report this plugin",
  "plugins.store.report.title": "Report {name}",
  "plugins.store.report.reason_label": "What's wrong",
  "plugins.store.report.reason.broken": "It doesn't work",
  "plugins.store.report.reason.misleading": "It isn't what it says it is",
  "plugins.store.report.reason.malware": "It does something harmful",
  "plugins.store.report.reason.copyright": "It uses someone else's work",
  "plugins.store.report.reason.other": "Something else",
  "plugins.store.report.text_label": "More about it",
  "plugins.store.report.send": "Send report",
  // Finding plugins.
  "plugins.store.browse": "Browse",
  "plugins.store.search": "Search plugins",
  "plugins.store.featured": "Featured",
  "plugins.store.category.agents": "Agents",
  "plugins.store.category.appearance": "Appearance",
  "plugins.store.category.chat": "Chat",
  "plugins.store.category.developer_tools": "Developer tools",
  "plugins.store.category.git": "Git",
  "plugins.store.category.integrations": "Integrations",
  "plugins.store.category.productivity": "Productivity",
  "plugins.store.category.utilities": "Utilities",
  "plugins.store.sort.relevance": "Best match",
  "plugins.store.sort.popular": "Most installed",
  "plugins.store.sort.updated": "Recently updated",
  "plugins.store.sort.new": "Newest",
  // A list with nothing in it, or none at all.
  "plugins.store.nothing_matches": "Nothing matches “{query}”",
  "plugins.store.nothing_here": "No plugins here",
  "plugins.store.try_another": "Try other words, or fewer filters.",
  "plugins.store.clear": "Clear search",
  "plugins.store.empty_title": "No plugins in the store yet",
  "plugins.store.empty_body": "Plugins people publish appear here.",
  "plugins.store.failed_title": "The plugin store didn't answer",
  "plugins.store.failed_body": "Your installed plugins still work. Try again in a moment.",
  "plugins.store.try_again": "Try again",
  "plugins.store.fact.category": "Category",
  "plugins.store.fact.works_with": "Works with",
  "plugins.store.fact.license": "License",
  // A plugin's facts.
  "plugins.store.fact.homepage": "Website",
  "plugins.store.fact.repository": "Source",
  "plugins.store.versions": "Versions",
  "plugins.store.all_versions": "Show all {n} versions",
  "plugins.store.show_less": "Show less",
  "plugins.store.needs_other_zeron": "It doesn't work with this version of Zeron.",
  "plugins.store.asks": "What it asks for",
} as const;

export type StoreKey = keyof typeof STORE_WORDS;

/** The app's words for `key`, with `{name}`-style places filled. */
export function store(key: StoreKey, args?: Record<string, string | number>): string {
  // (The app's plural, as its en.json writes it: "{n, plural, one {… # …} other {… # …}}".)
  const plural = STORE_WORDS[key].replace(/\{(\w+), plural, one \{([^{}]*)\} other \{([^{}]*)\}\}/g, (whole, name: string, one: string, other: string) => {
    const n = args?.[name];
    return typeof n === "number" ? (n === 1 ? one : other).replaceAll("#", String(n)) : whole;
  });
  return plural.replace(/\{(\w+)\}/g, (whole, name: string) => (args && name in args ? String(args[name]) : whole));
}

/**
 * Words the site says before the app does (amendments 44 and 45: states,
 * verification and the account's keys are new to both). Not compared with
 * the app's en.json until it has them; then each moves to STORE_WORDS.
 */
/** An empty list's words (the app's, since it says plugins appear at once: amendment 45). */
export const EMPTY_BODY = store("plugins.store.empty_body");

export const SITE_WORDS = {
  "status.listed": "Listed",
  // What each category holds, in a line (the coordinator's, from the prototype's index).
  "category.agents": "What your agents can use and how they report back",
  "category.appearance": "Themes, icons and how Zeron looks",
  "category.chat": "What a chat shows and remembers",
  "category.developer-tools": "Docs, databases, secrets and builds",
  "category.git": "Branches, pull requests and reviews",
  "category.integrations": "The other tools your work lives in",
  "category.productivity": "Time, notes and staying on track",
  "category.utilities": "Small things that should exist",
} as const;

/**
 * One of the store's sentences around one of its values (a link, a time),
 * so the page can draw that value as markup: the words before it and after.
 */
export function storeAround(key: StoreKey, args: Record<string, string | number>, slot: string): [string, string] {
  const said = store(key, { ...args, [slot]: "\u0000" });
  const at = said.indexOf("\u0000");
  return at < 0 ? [said, ""] : [said.slice(0, at), said.slice(at + 1)];
}

/** A version's state, named as the app names it (or the site, for the states the app doesn't name yet). */
export const statusWords = (status: VersionStatus): string => (status === "listed" ? SITE_WORDS["status.listed"] : store(`plugins.store.status.${status}`));

/** A report's reason, as the app's report sheet lists it. */
export const reportReasonWords = (reason: ReportReason): string => store(`plugins.store.report.reason.${reason}`);

/** A category's line (SITE_WORDS), or none for one the site doesn't know yet. */
export const categoryLine = (id: string): string | null => (SITE_WORDS as Record<string, string>)[`category.${id}`] ?? null;
