// What the plugin store calls things, in the app's words: an author who
// stops new installs of a version in the app's Yours tab and then opens
// their account on the web meets the same action, the same states, the
// same questions. Every string here is the app's English
// (apps/zgx/ui/locales/en.json, `plugins.store.*`), held to it by
// words.test.ts, with the web's one difference (ON_THE_WEB: the web isn't
// "this computer").
import type { ReportReason, VersionStatus } from "./types.ts";

export const STORE_WORDS = {
  // A version's state.
  "plugins.store.status.pending": "In review",
  "plugins.store.status.approved": "Approved",
  "plugins.store.status.rejected": "Not approved",
  "plugins.store.status.withdrawn": "Withdrawn",
  "plugins.store.status.yanked": "No new installs",
  "plugins.store.status.removed": "Taken down",
  // What its author can do to it, and the question each asks first.
  "plugins.store.action.withdraw": "Withdraw",
  "plugins.store.action.yank": "Stop new installs",
  "plugins.store.action.unyank": "Allow installs again",
  "plugins.store.confirm.withdraw.title": "Withdraw {name} {version}?",
  "plugins.store.confirm.withdraw.body": "It leaves the review queue. A version once sent keeps its number: send the next one under a new version.",
  "plugins.store.confirm.yank.title": "Stop new installs of {name} {version}?",
  "plugins.store.confirm.yank.body": "Nobody can install it anymore. Those who have it keep it working. You can allow installs again later.",
  "plugins.store.confirm.unyank.title": "Allow installs of {name} {version}?",
  "plugins.store.confirm.unyank.body": "It can be installed again.",
  // Deploy keys.
  "plugins.store.keys": "Deploy keys",
  "plugins.store.keys_body": "A CI job, a GitHub Action say, publishes new versions with a key instead of your session. A key publishes this plugin alone, and every version still waits for review.",
  "plugins.store.keys_none": "No keys yet.",
  "plugins.store.keys_failed": "Couldn't see its keys",
  "plugins.store.keys_publish_first": "Send its first version from your computer; a deploy key can send the ones after it.",
  "plugins.store.key.new": "New key…",
  "plugins.store.key.new_title": "New deploy key",
  "plugins.store.key.name": "Name",
  "plugins.store.key.never_used": "never used",
  "plugins.store.key.expiry_label": "Ends",
  "plugins.store.key.expiry.never": "Never",
  "plugins.store.your_versions": "Versions you've sent",
  "plugins.store.no_versions": "Nothing sent yet",
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
  "plugins.store.all_categories": "All categories",
  "plugins.store.category.agents": "Agents",
  "plugins.store.category.appearance": "Appearance",
  "plugins.store.category.chat": "Chat",
  "plugins.store.category.developer_tools": "Developer tools",
  "plugins.store.category.git": "Git",
  "plugins.store.category.integrations": "Integrations",
  "plugins.store.category.productivity": "Productivity",
  "plugins.store.category.utilities": "Utilities",
  "plugins.store.sort_label": "Order",
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
  "plugins.store.empty_body": "Plugins people share appear here once they're reviewed.",
  "plugins.store.failed_title": "The plugin store didn't answer",
  "plugins.store.failed_body": "Your installed plugins still work. Try again in a moment.",
  "plugins.store.try_again": "Try again",
  "plugins.store.fact.by": "By",
  "plugins.store.fact.installs": "Installs",
  "plugins.store.fact.category": "Category",
  "plugins.store.fact.works_with": "Works with",
  "plugins.store.fact.license": "License",
  "plugins.store.fact.version": "Version",
  // A plugin's facts.
  "plugins.store.fact.homepage": "Website",
  "plugins.store.fact.repository": "Source",
  "plugins.store.versions": "Versions",
  "plugins.store.asks": "What it asks for",
} as const;

export type StoreKey = keyof typeof STORE_WORDS;

/** The app's words for `key`, with `{name}`-style places filled. */
export function store(key: StoreKey, args?: Record<string, string | number>): string {
  return STORE_WORDS[key].replace(/\{(\w+)\}/g, (whole, name: string) => (args && name in args ? String(args[name]) : whole));
}

/** A version's state, named as the app names it. */
export const statusWords = (status: VersionStatus): string => store(`plugins.store.status.${status}`);

/** A report's reason, as the app's report sheet lists it. */
export const reportReasonWords = (reason: ReportReason): string => store(`plugins.store.report.reason.${reason}`);
