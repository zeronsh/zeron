// What the fake store holds when it starts: accounts, plugins with their
// versions in every state, reports and an audit log. Enough plugins to page
// through, each kind of permission, and plugins written to break the page
// (hostile names, keywords and READMEs), so the site is tried against them.

import { bytes } from "../src/lib/store/format.ts";
import { ICONS, screenshot } from "./art.ts";
import { makeToken, tokenId } from "./token.ts";

export const DAY = 86_400_000;
/** The fixtures' "now": fixed, so the tests see the same dates every run. */
export const NOW = Date.UTC(2026, 10, 17, 18, 0, 0);

export interface FakeUser {
  id: string;
  name: string;
  username: string | null;
  roles: string[];
  created_at: number;
  badges: { id: string; name: string; icon: string | null }[];
}

export const USERS: FakeUser[] = [
  { id: "user_ada", name: "Ada Lovelace", username: "ada", roles: [], created_at: NOW - 400 * DAY, badges: [{ id: "v3-beta", name: "V3 BETA", icon: null }] },
  { id: "user_grace", name: "Grace Hopper", username: "grace", roles: ["admin"], created_at: NOW - 900 * DAY, badges: [] },
  { id: "user_lin", name: "Lin", username: null, roles: [], created_at: NOW - 2 * DAY, badges: [] },
  { id: "user_kai", name: "Kai Moreno", username: "kai", roles: [], created_at: NOW - 30 * DAY, badges: [] },
  { id: "user_mo", name: "Mo", username: "mo-dev", roles: [], created_at: NOW - 6 * DAY, badges: [] },
  // An author with a username and nothing sent yet.
  { id: "user_noor", name: "Noor", username: "noor", roles: [], created_at: NOW - 1 * DAY, badges: [] },
];

// The store's fixed list (contract amendment 4); the names are the fake's.
export const CATEGORIES = [
  { id: "agents", name: "Agents" },
  { id: "appearance", name: "Appearance" },
  { id: "chat", name: "Chat" },
  { id: "developer-tools", name: "Developer tools" },
  { id: "git", name: "Git" },
  { id: "integrations", name: "Integrations" },
  { id: "productivity", name: "Productivity" },
  { id: "utilities", name: "Utilities" },
];

/**
 * How a version came to be, as a fixture says it (amendment 45): listed and
 * verified, listed and waiting for a reviewer, listed and declined, yanked by
 * its author (it was verified), taken down by staff (harmful: turned off on
 * devices), removed by staff (not harmful).
 */
export type Story = "verified" | "undecided" | "declined" | "yanked" | "taken_down" | "removed";

export type VersionStatus = "listed" | "yanked" | "taken_down" | "removed";

export interface FakeReview {
  decided: boolean;
  decision: "verified" | "declined" | null;
  reason?: string;
  message?: string;
  decided_at?: number;
}

export interface FakeVersion {
  version: string;
  status: VersionStatus;
  verified: boolean;
  verified_at: number | null;
  published_at: number;
  notes: string | null;
  engines: { zeron?: string } | null;
  review: FakeReview;
  via: "device" | "deploy_token";
  /** The deploy token that published it, when one did: its id, short id and name, never more. */
  deploy_token: { id: string; short: string; name: string } | null;
  /** Who stopped its new installs: its author, or the store when a newer version superseded it unverified. */
  yanked_by: "author" | "superseded" | null;
  superseded: { at: number; by: string } | null;
  /** Why staff took it down or removed it. */
  removal: { at: number; reason: string | null } | null;
  checks: { id: string; level: "ok" | "warn" | "fail"; message: string }[];
  manifest: Record<string, unknown>;
  files: Record<string, string>;
  /** Taken down and turned off on devices. */
  blocked?: boolean;
}

export interface FakePlugin {
  id: string;
  owner: string;
  status: "listed" | "unlisted" | "suspended" | "removed";
  suspend_reason?: string;
  featured: boolean;
  installs: number;
  created_at: number;
  /** Newest first. */
  versions: FakeVersion[];
}

interface Spec {
  id: string;
  name: string;
  owner?: string;
  summary: string;
  icon?: keyof typeof ICONS | null;
  /** An icon of its own, in place of the art's. */
  iconSvg?: string;
  accent?: string;
  part?: Parameters<typeof screenshot>[2];
  categories: string[];
  keywords?: string[];
  permissions?: string[];
  ui?: { change?: string[]; replace?: string[] };
  commands?: string[];
  network?: string[];
  tools?: { name: string; description: string }[];
  homepage?: string;
  repository?: string;
  license?: string;
  installs?: number;
  featured?: boolean;
  age: number;
  /** Older versions first; the last is the newest. Default: one verified 1.0.0. */
  history?: { version: string; status: Story; ago: number; notes?: string; review?: { reason?: string; message?: string; decided_at?: number }; via?: FakeVersion["via"]; engines?: string; permissions?: string[]; commands?: string[]; network?: string[]; mcp?: { command: string; args?: string[]; env?: Record<string, string> }; service?: string; settings?: Record<string, unknown>[]; files?: Record<string, string>; shots?: string[] }[];
  readme?: string;
  screenshots?: number;
  /** More files in every version's package. */
  files?: Record<string, string>;
  status?: FakePlugin["status"];
}

// (As the edge words them: the signature's names the key, 43 characters
// with nowhere to break, as wide as base64 can make it.)
const OK_CHECKS: FakeVersion["checks"] = [
  { id: "signature", level: "ok", message: "Signed by WMWMWMWMWMWMWMWMWMWMWMWMWMWMWMWMWMWMWMWMWMW, uploaded from a signed-in device." },
  { id: "manifest", level: "ok", message: "The manifest follows every rule a device checks." },
  { id: "newer", level: "ok", message: "Later than the last version." },
  { id: "readme", level: "ok", message: "README.md is there." },
];

function readme(spec: Spec, shots: string[]): string {
  if (spec.readme !== undefined) return spec.readme;
  const pictures = shots.map((s, i) => `![${spec.name}, screen ${i + 1}](${s})`).join("\n");
  return `# ${spec.name}

${spec.summary}

${pictures}

## What it does

- Shows what it knows where you're already looking, and nowhere else.
- Keeps out of the way until you need it.
- Remembers your choices on each of your devices.

## Settings

| Setting | What it does | Default |
| --- | --- | --- |
| Refresh | How often it looks again | 5 minutes |
| Quiet hours | When it keeps to itself | Off |

## For your agents

\`\`\`json
{ "tool": "${spec.id.replace(/-/g, "_")}_status", "input": {} }
\`\`\`

Made with care. Questions and ideas are welcome on [the repository](${spec.repository ?? "https://github.com/zeronsh/zeron"}).
`;
}

/**
 * The store's superseding rule (amendment 45), over a plugin's versions
 * oldest first: when a version is published, every older listed version that
 * isn't verified stops taking new installs, `yanked_by: "superseded"` (a
 * release supersedes older releases and pre-releases; a pre-release, older
 * pre-releases only). Verified versions are untouched.
 */
function supersede(oldestFirst: FakeVersion[]): void {
  const pre = (v: FakeVersion) => v.version.includes("-");
  for (const [i, v] of oldestFirst.entries()) {
    if (v.status !== "listed" || v.verified) continue;
    const by = oldestFirst.slice(i + 1).find((w) => !pre(w) || pre(v));
    if (!by) continue;
    v.status = "yanked";
    v.yanked_by = "superseded";
    v.superseded = { at: by.published_at, by: by.version };
  }
}

function build(spec: Spec): FakePlugin {
  const accent = spec.accent ?? "#9277b0";
  const shots = Array.from({ length: spec.screenshots ?? 2 }, (_, i) => `shots/screen-${i + 1}.svg`);
  const history = spec.history ?? [{ version: "1.0.0", status: "verified" as Story, ago: spec.age }];
  const versions: FakeVersion[] = history.map((h, index) => {
    const files: Record<string, string> = {
      "init.luau": `-- ${spec.name}\nlocal zeron = require("zeron")\n\nzeron.place { at = "#session-header", after = true, build = function()\n\treturn ui.text { "${spec.name}" }\nend }\n${index > 0 ? `\n-- ${h.version}: ${h.notes ?? "changes"}\nzeron.on("ready", function() end)\n` : ""}`,
      "zeron.luau": "-- Zeron's helpers (the SDK).\nreturn {}\n",
      "README.md": readme(spec, shots),
      "CHANGELOG.md": history.map((x) => `## ${x.version}\n\n${x.notes ?? "First release."}\n`).reverse().join("\n"),
    };
    if (spec.icon) files["icon.svg"] = spec.iconSvg ?? ICONS[spec.icon]!;
    Object.assign(files, spec.files ?? {}, h.files ?? {});
    for (const [i, s] of shots.entries()) files[s] = screenshot(spec.name, accent, i === 0 ? (spec.part ?? "panel") : "page");
    const manifest: Record<string, unknown> = {
      id: spec.id,
      name: spec.name,
      version: h.version,
      description: spec.summary,
      entry: "init.luau",
      permissions: h.permissions ?? spec.permissions ?? ["ui.add"],
      ...(spec.icon ? { icon: "icon.svg" } : {}),
      categories: spec.categories,
      keywords: spec.keywords ?? [],
      ...(spec.ui ? { ui: spec.ui } : {}),
      ...((h.commands ?? spec.commands) ? { commands: h.commands ?? spec.commands } : {}),
      ...((h.network ?? spec.network) ? { network: { hosts: h.network ?? spec.network } } : {}),
      ...(spec.tools ? { tools: spec.tools.map((t) => ({ ...t, input: { type: "object" } })) } : {}),
      ...(h.mcp ? { mcp: h.mcp } : {}),
      ...(h.service ? { service: h.service } : {}),
      ...(h.settings ? { settings: h.settings } : {}),
      ...(spec.homepage ? { homepage: spec.homepage } : {}),
      ...(spec.repository ? { repository: spec.repository } : {}),
      ...(spec.license ? { license: spec.license } : {}),
      screenshots: [...shots, ...(h.shots ?? [])],
      engines: { zeron: h.engines ?? ">=0.3.0" },
    };
    // (The size check, as the edge words it: the package's files, the manifest apart, in the site's sizes, proposed to the edge lane.)
    const total = Object.values(files).reduce((n, text) => n + text.length, 0);
    const size = { id: "size", level: "ok" as const, message: `${Object.keys(files).length} files, ${bytes(total)} (${bytes(Math.round(total * 0.42))} packaged).` };
    const ok = [...OK_CHECKS.slice(0, 2), size, ...OK_CHECKS.slice(2)];
    const checks = h.status === "undecided" && (h.permissions ?? spec.permissions ?? []).some((p) => p === "commands" || p === "native")
      ? [...ok, { id: "risky", level: "warn" as const, message: "Asks to run programs on the computer as you." }]
      : ok;
    const published = NOW - h.ago * DAY;
    // A verified version was verified half a day after it was published (a yanked one was, before it was yanked).
    const verifiedAt = h.status === "verified" || h.status === "yanked" ? (h.review?.decided_at ?? published + DAY / 2) : null;
    const status: VersionStatus = h.status === "yanked" || h.status === "taken_down" || h.status === "removed" ? h.status : "listed";
    const staff = h.status === "taken_down" || h.status === "removed";
    const review: FakeReview =
      verifiedAt !== null
        ? { decided: true, decision: "verified", decided_at: verifiedAt }
        : h.status === "declined"
          ? { decided: true, decision: "declined", ...h.review }
          : { decided: false, decision: null };
    // (The manifest travels beside the files, never as one: the edge lists it apart.)
    return {
      version: h.version,
      status,
      verified: verifiedAt !== null,
      verified_at: verifiedAt,
      published_at: published,
      notes: h.notes ?? (index === 0 ? "First release." : null),
      engines: { zeron: h.engines ?? ">=0.3.0" },
      review,
      via: h.via ?? "device",
      deploy_token: h.via === "deploy_token" ? githubActions() : null,
      yanked_by: h.status === "yanked" ? ("author" as const) : null,
      superseded: null,
      removal: staff ? { at: h.review?.decided_at ?? published + DAY, reason: h.review?.message ?? h.review?.reason ?? null } : null,
      ...(h.status === "taken_down" ? { blocked: true } : {}),
      checks,
      manifest,
      files,
    };
  });
  supersede(versions);
  return {
    id: spec.id,
    owner: spec.owner ?? "user_ada",
    // (Listed or not is its versions' to say: a plugin is listed while one of them is.)
    status: spec.status === "suspended" ? "suspended" : "listed",
    featured: spec.featured ?? false,
    installs: spec.installs ?? 0,
    created_at: NOW - spec.age * DAY,
    versions: versions.reverse(),
  };
}

const SPECS: Spec[] = [
  {
    id: "focus-timer",
    name: "Focus timer",
    summary: "A quiet timer in a chat's header that keeps your agents' long runs to a rhythm.",
    icon: "timer",
    accent: "#ad98fe",
    part: "header",
    categories: ["productivity"],
    keywords: ["timer", "pomodoro", "focus"],
    permissions: ["ui.add", "sessions.read", "notifications"],
    homepage: "https://example.com/focus-timer",
    repository: "https://github.com/ada/focus-timer",
    license: "MIT",
    installs: 12840,
    featured: true,
    age: 120,
    // (Every form the site's renderer draws, for the plugin page's pictures and its tests.)
    readme: `# Focus timer

A **25 minute timer** in a chat's header. Start it when you hand an agent a long task; it counts down beside the chat's title and tells you, *quietly*, when the time is up. ~~No more guessing.~~ Find it in Settings, or run \`zeron plugin install focus-timer\`.

![Focus timer, screen 1](shots/screen-1.svg)

## What it does

- Shows what it knows where you are already looking, and nowhere else.
- Pauses when you leave the chat, and picks up when you come back.
  - A second level, for the details.
- Remembers your lengths on each of your computers.

![The timer, small](shots/small.svg)

1. Start a long run.
2. Start the timer.
3. Take the break it gives you.

## Before 2.0

- [x] Breaks that follow your agents
- [x] A chime at the end
- [ ] A second timer for breaks

## Settings

| Setting | What it does | Default |
| --- | --- | --- |
| Length | How long one run of the timer lasts | 25 minutes |
| Quiet hours | When it keeps to itself | Off |
| Chime | The sound at the end: \`soft\`, \`bell\` or \`none\` | \`soft\` |

## For your agents

Agents can ask how long is left, so a long task can pace itself:

\`\`\`json
{ "tool": "focus_timer_status", "input": {} }
\`\`\`

    -- An indented block is code too.
    local left = zeron.timer.left()

> Quiet is the point: it never takes the chat's focus.

---

A very long address, to see that nothing scrolls the page sideways: https://example.com/focus-timer/a/very/long/address/that/keeps/going/without/a/single/space/to/break/it/at/all/0123456789

\`\`\`text
a-very-long-line-of-code-with-no-spaces-at-all-that-must-scroll-inside-its-own-box-and-never-push-the-page-wider-than-the-window-0123456789
\`\`\`

Footnotes are shown as typed.[^1] Questions and ideas are welcome on [the repository](https://github.com/ada/focus-timer).

[^1]: As the publishing guide says.
`,
    history: [
      { version: "1.0.0", status: "verified", ago: 120, notes: "First release." },
      { version: "1.1.0", status: "verified", ago: 60, notes: "- Pauses when your computer sleeps.\n- A sound at the end, if you want one." },
      // (A picture smaller than the reading column: drawn at its own size, never scaled up.)
      { version: "1.2.0", status: "verified", ago: 9, via: "deploy_token", notes: "### New\n\n- Breaks that follow your agents: a long run gets a long break.\n\n### Fixed\n\n- The timer kept counting after its chat closed.", files: { "shots/small.svg": '<svg xmlns="http://www.w3.org/2000/svg" width="360" height="200" viewBox="0 0 360 200"><rect width="360" height="200" fill="#1b1726"/><circle cx="180" cy="100" r="56" fill="none" stroke="#ad98fe" stroke-width="10"/><path d="M180 64v40l24 16" fill="none" stroke="#ad98fe" stroke-width="10" stroke-linecap="round"/></svg>' } },
      { version: "1.3.0-beta.1", status: "undecided", ago: 1, notes: "Try the new compact look." },
    ],
  },
  {
    id: "pr-radar",
    name: "PR Radar",
    summary: "Every pull request waiting on you, in the sidebar, with checks and reviews at a glance.",
    icon: "radar",
    accent: "#73c580",
    part: "sidebar",
    categories: ["git"],
    keywords: ["github", "pull requests", "review"],
    permissions: ["ui.add", "ui.change", "commands", "notifications"],
    ui: { change: [".session-row >> .session-title"] },
    commands: ["gh", "git"],
    repository: "https://github.com/kai/pr-radar",
    license: "Apache-2.0",
    owner: "user_kai",
    installs: 8410,
    featured: true,
    age: 80,
    history: [
      { version: "0.9.0", status: "verified", ago: 80, notes: "First release." },
      { version: "1.0.0", status: "verified", ago: 20, notes: "Reviews requested from your teams show too." },
    ],
  },
  {
    id: "linear-issues",
    name: "Linear issues",
    summary: "Read an issue, move it along and link a chat to it, without leaving the conversation.",
    icon: "issues",
    accent: "#7584f4",
    part: "panel",
    categories: ["integrations", "productivity"],
    keywords: ["linear", "issues", "tickets"],
    permissions: ["ui.add", "panel.tabs", "network", "tools", "links.open"],
    network: ["api.linear.app"],
    tools: [
      { name: "linear_issue", description: "Reads a Linear issue by its key (ENG-123): title, state, assignee and description." },
      { name: "linear_move", description: "Moves a Linear issue to another state." },
    ],
    repository: "https://github.com/ada/linear-issues",
    license: "MIT",
    installs: 6120,
    featured: true,
    age: 64,
  },
  {
    id: "token-meter",
    name: "Token meter",
    summary: "How much of each agent's plan you've used this week, and when it resets.",
    icon: "meter",
    accent: "#e9a64b",
    part: "header",
    categories: ["agents"],
    keywords: ["usage", "limits", "plan"],
    permissions: ["ui.add", "usage.read", "accounts.read"],
    license: "MIT",
    installs: 9930,
    age: 45,
  },
  {
    id: "shell-runner",
    name: "Shell runner",
    summary: "Run a saved shell command from any chat, and give its output to your agent.",
    icon: "shell",
    accent: "#f9837b",
    part: "composer",
    categories: ["developer-tools"],
    keywords: ["shell", "scripts", "terminal"],
    permissions: ["ui.add", "commands", "compose", "tools"],
    commands: ["bash", "make"],
    tools: [{ name: "run_saved", description: "Runs one of your saved commands and returns what it printed." }],
    license: "MIT",
    owner: "user_mo",
    installs: 1540,
    age: 25,
  },
  {
    id: "postgres-peek",
    name: "Postgres peek",
    summary: "Let agents read a schema and run read-only queries against your local database.",
    icon: "database",
    accent: "#149dd2",
    part: "panel",
    categories: ["developer-tools", "agents"],
    keywords: ["postgres", "sql", "database"],
    permissions: ["ui.add", "panel.tabs", "network", "native", "tools"],
    network: ["localhost:5432", "db.internal.example.com"],
    tools: [{ name: "pg_schema", description: "Lists tables and their columns." }, { name: "pg_query", description: "Runs a read-only query and returns at most 200 rows." }],
    license: "BSD-3-Clause",
    owner: "user_kai",
    installs: 3200,
    age: 50,
  },
  {
    id: "theme-sync",
    // Pictures that are not what they say: the file route must never let one run.
    files: {
      "shots/script.svg": `<svg xmlns="http://www.w3.org/2000/svg" onload="document.documentElement.setAttribute('data-ran','onload')"><script>document.documentElement.setAttribute('data-ran','script');parent.postMessage('ran','*')</script><rect width="10" height="10"/></svg>`,
      "shots/page.png": `<!doctype html><html><body><p id="ran">an HTML page named .png</p><script>document.title='ran'</script></body></html>`,
      "shots/polyglot.gif": `GIF89a/*\x00\x00*/=1;window.__ran='polyglot';/*<html><script>document.title='ran'</script>*/`,
    },
    name: "Theme sync",
    summary: "Follows your desktop's light and dark, and your editor's theme, as they change.",
    icon: "palette",
    accent: "#e25e76",
    part: "page",
    categories: ["appearance"],
    keywords: ["theme", "dark mode", "vs code"],
    permissions: ["ui.add", "pages.add"],
    license: "MIT",
    installs: 4410,
    age: 33,
  },
  {
    id: "standup-notes",
    name: "Standup notes",
    summary: "Turns yesterday's chats into a short standup you can paste anywhere.",
    icon: "calendar",
    accent: "#18aa99",
    part: "page",
    categories: ["productivity", "chat"],
    keywords: ["standup", "summary", "notes"],
    permissions: ["sessions.read", "messages.read", "clipboard", "pages.add"],
    installs: 2210,
    age: 14,
  },
  {
    id: "agent-relay",
    name: "Agent relay",
    summary: "Sends a chat's question to your phone and its answer back to the agent.",
    icon: "bolt",
    accent: "#ad98fe",
    part: "composer",
    categories: ["agents", "chat"],
    keywords: ["phone", "relay", "answers"],
    permissions: ["messages.read", "messages.send", "devices", "service", "notifications"],
    installs: 870,
    age: 7,
  },
  {
    // Every listing limit at once (contract amendment 22): the worst a card,
    // a row or a heading has to hold.
    id: "worst-case-listing-at-every-limit-xyz12",
    name: "Averyveryverylongpluginnamewithoutanyspacesatallthatkeepsgoing64",
    summary: "Every limit at once: a name of sixty-four characters with no spaces to break at, this description at three hundred characters, eight keywords of thirty-two characters, two categories, so each card, row and control is seen at its worst before anyone else sees it there. Every limit at once: a name of",
    icon: "book",
    accent: "#e9a64b",
    part: "panel",
    categories: ["developer-tools", "integrations"],
    keywords: ['eywordthatisthirtytwocharacters0', 'eywordthatisthirtytwocharacters1', 'eywordthatisthirtytwocharacters2', 'eywordthatisthirtytwocharacters3', 'eywordthatisthirtytwocharacters4', 'eywordthatisthirtytwocharacters5', 'eywordthatisthirtytwocharacters6', 'eywordthatisthirtytwocharacters7'],
    permissions: ["ui.add", "network", "commands", "tools"],
    commands: ["python3", "git", "jq"],
    network: ["api.averyveryverylongsubdomainname.example.com", "localhost:8080", "*"],
    tools: [{ name: "a_tool_with_a_long_name_for_agents_to_call", description: "Every limit at once: a name of sixty-four characters with no spaces to break at, this description at three hundred characters, eight keywords of thirty-two characters, two categories, so each card, ro" }],
    homepage: "https://example.com/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    repository: "https://github.com/ada/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    license: "LicenseRef-A-Very-Long-License-Identifier-That-Goes-On-And-On",
    installs: 7000,
    featured: true,
    age: 12,
    // A version in every state, a reviewer's reason as long as they write them.
    history: [
      { version: "1.0.0", status: "verified", ago: 12, mcp: { command: "node", args: ["server.js"], env: { API_URL: "https://api.averyveryverylongsubdomainname.example.com" } }, service: "service.luau", settings: [{ key: "notes", kind: "folder", label: "Notes folder" }] },
      // (Verified, then stopped by its author: still its newest verified version, what 1.5.0 is weighed against.)
      { version: "1.1.0", status: "yanked", ago: 10, notes: "Moved the panel; stopped offering it after the sidebar changed under it.", mcp: { command: "node", args: ["server.js"], env: { API_URL: "https://api.averyveryverylongsubdomainname.example.com" } }, service: "service.luau", settings: [{ key: "notes", kind: "folder", label: "Notes folder" }] },
      { version: "1.2.0", status: "taken_down", ago: 8, review: { reason: "unsafe", message: "Taken down: it sent the open chat's text to a host its README doesn't name. Anyone who has it has had it turned off.", decided_at: NOW - 7 * DAY } },
      { version: "1.3.0", status: "removed", ago: 6 },
      { version: "1.4.0", status: "declined", ago: 4, review: { reason: "asks_too_much", message: "It asks to run python3, git and jq and to reach any site at all, but nothing in the README or the listing says what it runs them for or what it sends where. Name the hosts it needs and drop the wildcard, say in the README what each program is run for, and send it again: a reviewer will look at the new version the same day. Until then this one stays listed, not verified.", decided_at: NOW - 3 * DAY } },
      // (The worst to weigh: two new asks, a program more, hosts named in place
      // of the wildcard, its MCP server and service changed, a setting that grants.)
      { version: "1.5.0", status: "undecided", ago: 1, notes: "Names the hosts it reaches; says what it runs and why.", permissions: ["ui.add", "network", "commands", "tools", "messages.send", "notifications"], commands: ["python3", "git", "jq", "bash"], network: ["api.averyveryverylongsubdomainname.example.com", "localhost:8080", "uploads.averyveryverylongsubdomainname.example.com"], mcp: { command: "node", args: ["server.js", "--listen", "0.0.0.0:7777"], env: { API_URL: "https://api.averyveryverylongsubdomainname.example.com", UPLOAD_TOKEN: "set-on-first-run" } }, service: "services/sync.luau", settings: [{ key: "notes", kind: "folder", label: "Notes folder" }, { key: "share", kind: "permission", label: "Share notes with agents" }] },
    ],
  },
  {
    // A plugin whose words try to break the page.
    id: "xss-probe",
    // (With a right-to-left override in it, which must reorder nothing but itself.)
    name: `<img src=x onerror="alert('name')"> Probe \u202Eevil`,
    summary: `"><script>alert('summary')</script> A plugin whose every word tries to escape the page.`,
    icon: "flame",
    categories: ["utilities"],
    keywords: [`"><svg onload=alert(1)>`, "javascript:alert(1)", "normal"],
    // Its icon tries to run when opened on its own.
    iconSvg: `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" onload="document.documentElement.setAttribute('data-ran','onload')"><script>document.documentElement.setAttribute('data-ran','script')</script><circle cx="12" cy="12" r="8" fill="none" stroke="currentColor"/></svg>`,
    permissions: ["ui.add", "ui.replace", "network", "tools"],
    tools: [{ name: "probe_tool", description: `</p><script>alert('tool')</script><img src=x onerror=alert('tool')>` }],
    // Paths may hold %, spaces and # (and, here, markup).
    files: {
      "docs/<b>notes</b> #1 100%.md": "# <script>alert('file')</script>\n",
      "shots/a b#1 100%.svg": `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><rect width="10" height="10" fill="#9277b0"/></svg>`,
      // Files an author might hope the review can't show: a path as long as
      // the edge takes (past any shorter limit of the site's), and a folder
      // named like an encoded climb.
      [`src/${"x".repeat(600)}.lua`]: "return 'the long one'\n",
      "lib/%2e%2e/x.lua": "return 'the dotted one'\n",
    },
    history: [
      { version: "1.0.0", status: "verified", ago: 3, notes: `<script>alert('notes')</script> and [a link](javascript:alert('notes'))` },
      { version: "1.1.0", status: "undecided", ago: 1, notes: `</code></pre><script>alert('diff')</script><img src=x onerror=alert('diff')>`, network: ["*", "localhost:9090"] },
    ],
    ui: { replace: ["@app/screens/chat/composer"] },
    network: ["*", "localhost:8080"],
    owner: "user_mo",
    installs: 3,
    age: 3,
    readme: `# Probe <script>alert('h1')</script>

<script>alert('raw block')</script>

<img src="x" onerror="alert('img')">

<a href="javascript:alert('raw link')">raw link</a>

[markdown js link](javascript:alert('md'))
[data link](data:text/html;base64,PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pg==)
[vbscript](vbscript:msgbox(1))
[spaced](java&#x09;script:alert(1))
[protocol relative](//evil.example/path)
[climb](../../../../etc/passwd)
[mail](mailto:ada@example.com)
<https://autolink.example/ok>

![remote picture](https://evil.example/track.png)
![data picture](data:image/svg+xml;base64,PHN2ZyBvbmxvYWQ9YWxlcnQoMSk+)
![escape](../../secret.png)
![not a picture](init.luau)
![the package's own](shots/screen-1.svg)

<iframe src="https://evil.example"></iframe>
<style>body{display:none}</style>
<form action="https://evil.example"><input name=x></form>

\`\`\`html
<script>alert('in code, shown as text')</script>
\`\`\`

| col | <b>bold?</b> |
| --- | --- |
| [x](javascript:alert(1)) | ok |

Footnote[^x"><script>].

[^x"><script>]: A note with a [link](javascript:alert(1)).
`,
  },
  {
    // Listed, with a version waiting for a reviewer and one declined: the author's view.
    id: "session-badges",
    name: "Session badges",
    summary: "A small badge on each sidebar row: its branch, its checks, its cost.",
    icon: "badge",
    accent: "#e07434",
    part: "sidebar",
    categories: ["git", "appearance"],
    keywords: ["sidebar", "branch", "badges"],
    permissions: ["ui.add", "sessions.read"],
    license: "MIT",
    installs: 2970,
    age: 70,
    history: [
      { version: "1.0.0", status: "verified", ago: 70 },
      { version: "1.1.0", status: "yanked", ago: 40, notes: "Broke the sidebar on narrow windows." },
      { version: "1.1.1", status: "verified", ago: 39, notes: "Fixed the sidebar on narrow windows." },
      { version: "1.2.0", status: "declined", ago: 5, notes: "Shows the cost of each chat.", permissions: ["ui.add", "sessions.read", "network"], network: ["*"], review: { reason: "asks_too_much", message: "It asks to reach any site, but the README doesn't say why. Name the hosts it needs, or say what it sends where.", decided_at: NOW - 4 * DAY } },
      { version: "1.2.1", status: "undecided", ago: 2, notes: "Shows the cost of each chat, from your agents' own usage.", permissions: ["ui.add", "sessions.read", "usage.read"] },
    ],
  },
  {
    // A first version, listed at once, waiting for a reviewer.
    id: "voice-memos",
    name: "Voice memos",
    summary: "Record a thought and drop it into a chat as text.",
    icon: "mic",
    categories: ["chat"],
    keywords: ["voice", "dictation"],
    permissions: ["ui.add", "compose", "network", "commands"],
    commands: ["ffmpeg"],
    network: ["api.openai.com"],
    owner: "user_mo",
    age: 1,
    history: [{ version: "0.1.0", status: "undecided", ago: 1 }],
    status: "unlisted",
  },
  {
    // Only a declined version: listed, never verified.
    id: "old-idea",
    name: "Old idea",
    summary: "Tried once.",
    homepage: "https://user:pw@example.com/",
    categories: ["utilities"],
    owner: "user_kai",
    age: 20,
    history: [{ version: "0.1.0", status: "declined", ago: 20, review: { reason: "broken", message: "It doesn't start.", decided_at: NOW - 19 * DAY } }],
    status: "unlisted",
  },
  {
    // Suspended by an admin: gone from the store.
    id: "crypto-ticker",
    name: "Crypto ticker",
    summary: "Prices in your title bar.",
    icon: "graph",
    categories: ["utilities"],
    keywords: ["prices"],
    permissions: ["ui.add", "network"],
    network: ["*"],
    owner: "user_mo",
    installs: 140,
    age: 90,
    status: "suspended",
  },
];

// Many more, for paging and sorting.
const MORE: [string, string, string, keyof typeof ICONS | null, string[]][] = [
  ["cloud-costs", "Cloud costs", "What your cloud spent today, beside the chat that spent it.", "cloud", ["integrations"]],
  ["docs-lookup", "Docs lookup", "Look a function up in the docs your project uses.", "book", ["developer-tools"]],
  ["map-pins", "Map pins", "Pin a chat to the place it's about.", "map", ["utilities"]],
  ["key-guard", "Key guard", "Warns before a chat pastes something that looks like a secret.", "key", ["developer-tools", "chat"]],
  ["green-build", "Green build", "Your CI's status in the title bar.", "leaf", ["git"]],
  ["translate-reply", "Translate reply", "Read an agent's answer in your own language.", "translate", ["chat"]],
  ["night-shift", "Night shift", "Warmer colours after dark.", "palette", ["appearance"]],
  ["release-notes", "Release notes", "Drafts release notes from merged pull requests.", "book", ["git", "productivity"]],
  ["ping-me", "Ping me", "A notification when an agent has been waiting on you too long.", "bell", ["agents"]],
  ["quick-links", "Quick links", "Your team's links, one keystroke away.", null, ["productivity"]],
  ["diff-stats", "Diff stats", "Lines changed by each chat, in its header.", "graph", ["git"]],
  ["sprint-board", "Sprint board", "Your sprint as a page in the sidebar.", "calendar", ["productivity", "integrations"]],
  ["emoji-status", "Emoji status", "An emoji for each chat's mood.", null, ["utilities", "appearance"]],
  ["log-tail", "Log tail", "Follow a log file in a chat's panel.", "shell", ["developer-tools"]],
  ["sentry-traces", "Sentry traces", "Pull a stack trace and its release into the chat.", "flame", ["integrations", "developer-tools"]],
  ["weather", "Weather", "The weather where you are, in the title bar.", "cloud", ["utilities"]],
  ["word-count", "Word count", "Counts the words in the message box.", null, ["chat"]],
  ["branch-namer", "Branch namer", "Names a chat's branch from what it's about.", "radar", ["git", "agents"]],
  ["meeting-mode", "Meeting mode", "Quiets every notification while your calendar says you're busy.", "calendar", ["productivity"]],
  ["model-switch", "Model switch", "Switches a chat's model when a turn gets hard.", "bolt", ["agents"]],
  ["focus-music", "Focus music", "Plays something calm while agents work.", "mic", ["utilities"]],
  ["api-console", "API console", "Try a request and hand the answer to your agent.", "shell", ["developer-tools", "integrations"]],
];

// Thirty more of Ada's, only while a test asks for them (POST /_fake/many?on=1): a key for fifty
// plugins, the most a key may name, as a real author's would look, every one of them hers.
const MANY: [string, string, string, keyof typeof ICONS | null, string[]][] = [
  ["branch-sweeper", "Branch sweeper", "Deletes the branches whose chats are done.", "leaf", ["git"]],
  ["changelog-check", "Changelog check", "Says when a change has no changelog line.", "book", ["git"]],
  ["chat-export", "Chat export", "Saves a chat as Markdown.", "database", ["chat"]],
  ["ci-watch", "CI watch", "Follows a pull request's checks in its chat.", "radar", ["git", "integrations"]],
  ["code-owners", "Code owners", "Who owns the files a chat is changing.", "key", ["git"]],
  ["commit-coach", "Commit coach", "A commit message from the chat's changes.", "issues", ["git"]],
  ["context-gauge", "Context gauge", "How full each chat's context is.", "meter", ["agents"]],
  ["daily-digest", "Daily digest", "What your agents did yesterday, each morning.", "calendar", ["productivity"]],
  ["dark-sky", "Dark sky", "A darker dark for late nights.", "palette", ["appearance"]],
  ["deploy-button", "Deploy button", "Deploy from a chat's header.", "bolt", ["integrations"]],
  ["dependency-alerts", "Dependency alerts", "Warns when a chat adds a package with a known problem.", "flame", ["developer-tools"]],
  ["error-first", "Error first", "A build's first error, at the top of its chat.", "graph", ["developer-tools"]],
  ["fix-later", "Fix later", "Saves a TODO from a chat to your list.", null, ["productivity"]],
  ["git-blame", "Git blame", "Who last changed a line, and in which chat.", "shell", ["git"]],
  ["hand-off", "Hand off", "Passes a chat to a teammate with a note.", "bell", ["chat"]],
  ["issue-from-chat", "Issue from chat", "Opens an issue from what a chat found.", "issues", ["integrations"]],
  ["lint-on-save", "Lint on save", "Runs your linter when an agent saves.", "leaf", ["developer-tools"]],
  ["local-time", "Local time", "Your teammates' time of day beside their names.", "map", ["utilities"]],
  ["mute-hours", "Mute hours", "No notifications outside your hours.", "bell", ["productivity"]],
  ["pair-mode", "Pair mode", "Two agents, one chat, taking turns.", "translate", ["agents"]],
  ["port-finder", "Port finder", "Which process holds a port, and its chat.", "radar", ["developer-tools"]],
  ["prompt-library", "Prompt library", "Your saved prompts, one keystroke away.", "book", ["chat"]],
  ["readme-lint", "README lint", "Checks a README against the store's listing rules.", null, ["developer-tools"]],
  ["schema-peek", "Schema peek", "A table's columns, beside the query that uses them.", "database", ["developer-tools"]],
  ["screen-share", "Screen share", "Shares one chat, not your screen.", "mic", ["chat"]],
  ["snippet-shelf", "Snippet shelf", "Keeps the code an agent wrote that you want again.", null, ["productivity"]],
  ["test-runner", "Test runner", "Runs the tests a chat touched.", "timer", ["developer-tools"]],
  ["token-budget", "Token budget", "Stops a chat at the budget you set.", "meter", ["agents"]],
  ["voice-notes", "Voice notes", "Speak a message; it's typed as you talk.", "mic", ["chat"]],
  ["work-log", "Work log", "A day's chats, as a list of what got done.", "calendar", ["productivity"]],
];
export const MANY_IDS = MANY.map(([id]) => id);

for (const [i, [id, name, summary, icon, categories]] of MORE.entries()) {
  SPECS.push({
    id,
    name,
    summary,
    icon: icon && ICONS[icon] ? icon : null,
    categories,
    keywords: [id.split("-")[0]!],
    permissions: i % 3 === 0 ? ["ui.add", "network"] : ["ui.add", "sessions.read"],
    network: i % 3 === 0 ? [`api.${id}.example.com`] : undefined,
    owner: ["user_ada", "user_kai", "user_mo"][i % 3],
    installs: Math.round(5000 / (i + 2)),
    age: 10 + i * 3,
    screenshots: i % 4 === 0 ? 0 : 1,
    // (Weather needs a Zeron not out yet: Install is off on its page.)
    history: [{ version: `1.${i % 5}.0`, status: "verified", ago: 10 + i * 3, ...(id === "weather" ? { engines: ">=0.9.0" } : {}) }],
  });
}

// Ada's plugins in the states an author's list must show apart from the
// steady one: a first version waiting for a reviewer, one declined, one
// taken down, one whose newest version staff removed.
SPECS.push(
  { id: "first-light", name: "First light", summary: "Opens your morning chats as you left them.", icon: null, categories: ["productivity"], keywords: ["morning"], permissions: ["ui.add"], owner: "user_ada", installs: 0, age: 2, screenshots: 0, history: [{ version: "0.1.0", status: "undecided", ago: 2 }] },
  { id: "rough-draft", name: "Rough draft", summary: "Drafts a reply in the composer from the chat so far.", icon: null, categories: ["chat"], keywords: ["draft"], permissions: ["ui.add", "messages.read"], owner: "user_ada", installs: 0, age: 9, screenshots: 0, history: [{ version: "0.1.0", status: "declined", ago: 9, review: { reason: "listing", message: "The README is one line. Say what it drafts from, where the draft appears, and add a picture of it.", decided_at: NOW - 8 * DAY } }] },
  { id: "clipboard-sync", name: "Clipboard sync", summary: "Keeps the clipboard the same on all your devices.", icon: null, categories: ["utilities"], keywords: ["clipboard"], permissions: ["ui.add", "clipboard", "network"], network: ["sync.clipboard-sync.example.com"], owner: "user_ada", installs: 310, age: 40, screenshots: 0, history: [{ version: "1.0.0", status: "taken_down", ago: 40, review: { reason: "unsafe", message: "It sent every copied password to its sync host in plain text.", decided_at: NOW - 12 * DAY } }] },
  { id: "old-sketch", name: "Old sketch", summary: "A first try at a sidebar of pinned chats.", icon: null, categories: ["productivity"], keywords: ["pins"], permissions: ["ui.add"], owner: "user_ada", installs: 0, age: 30, screenshots: 0, history: [{ version: "0.1.0", status: "yanked", ago: 30 }, { version: "0.2.0", status: "removed", ago: 20 }] },
  // Words built to cost the parser the square of their length, each at the store's cap (a 512 KB
  // README, 64 KB notes): the page must still answer at once, showing them as text.
  { id: "slow-words", name: "Slow words", summary: "Its README and notes are built to be slow to draw.", icon: null, categories: ["utilities"], keywords: ["slow"], permissions: ["ui.add"], owner: "user_mo", installs: 3, age: 7, screenshots: 0, readme: "- a\n+ b\n".repeat(65_536), history: [{ version: "1.0.0", status: "verified", ago: 7, notes: ">".repeat(65_536) }, { version: "1.1.0", status: "verified", ago: 5, notes: "[".repeat(13_107) + "a" + "](b)".repeat(13_107) }, { version: "1.2.0", status: "undecided", ago: 1, notes: `|${"a|".repeat(64)}\n|${"-|".repeat(64)}\n${"a\n".repeat(32_000)}`, files: { "src/nest.js": "`${\n".repeat(300) + "`${".repeat(130) }, shots: ["../../../../account/me.json"] }] },
);

/** Ada's thirty more (MANY), built as every other plugin is. */
export function manyMore(): FakePlugin[] {
  return MANY.map(([id, name, summary, icon, categories], i) =>
    build({ id, name, summary, icon, categories, keywords: [id.split("-")[0]!], permissions: ["ui.add"], owner: "user_ada", installs: 40 + i * 7, age: 20 + i, screenshots: 0, history: [{ version: "1.0.0", status: "verified", ago: 20 + i }] }),
  );
}

export function plugins(): FakePlugin[] {
  return SPECS.map(build).map((p) => (p.id === "crypto-ticker" ? { ...p, suspend_reason: "Sends what you type to a third party." } : p));
}

export interface FakeReport {
  id: number;
  plugin_id: string;
  version: string | null;
  reason: "malware" | "misleading" | "broken" | "copyright" | "other";
  text: string;
  reporter: string;
  created_at: number;
  status: "open" | "dismissed" | "actioned";
  note: string | null;
}

export function reports(): FakeReport[] {
  return [
    { id: 1, plugin_id: "shell-runner", version: "1.0.0", reason: "misleading", text: "It says it runs saved commands, but it asks to run bash, which can run anything.", reporter: "user_ada", created_at: NOW - 2 * DAY, status: "open", note: null },
    { id: 2, plugin_id: "xss-probe", version: "1.0.0", reason: "malware", text: `Its name is <script>alert("report")</script>, which can't be good.`, reporter: "user_kai", created_at: NOW - 1 * DAY, status: "open", note: null },
    { id: 3, plugin_id: "focus-timer", version: "1.1.0", reason: "broken", text: "Doesn't stop when the chat closes.", reporter: "user_mo", created_at: NOW - 30 * DAY, status: "actioned", note: "Fixed in 1.2.0." },
  ];
}

export interface FakeAudit {
  seq: number;
  at: number;
  actor: string;
  /** The edge's words: version.verify, plugin.suspend, report.dismissed, reservation.set, … */
  action: string;
  plugin_id: string | null;
  version: string | null;
  detail: Record<string, unknown> | null;
}

export function audit(): FakeAudit[] {
  return [
    { seq: 3, at: NOW - 4 * DAY, actor: "user_grace", action: "version.decline", plugin_id: "session-badges", version: "1.2.0", detail: { reason: "asks_too_much" } },
    { seq: 2, at: NOW - 9 * DAY, actor: "user_grace", action: "version.verify", plugin_id: "focus-timer", version: "1.2.0", detail: null },
    { seq: 1, at: NOW - 20 * DAY, actor: "user_grace", action: "plugin.suspend", plugin_id: "crypto-ticker", version: null, detail: { reason: "Sends what you type to a third party." } },
  ];
}

/**
 * A deploy token, the account's (contract a315eef5a), with its scope fixed
 * when made. As the store, the fake keeps a made token's hash and short id,
 * never its secret; a fixture's id and short id are a real token's, made
 * the store's way from fixed bytes, the same every run.
 */
export interface FakeToken {
  owner: string;
  id: string;
  short: string;
  /** The SHA-256 of the secret, in hex (one the fake made; a fixture's secret was never kept). */
  hash: string | null;
  scope: { all: true } | { plugins: string[] };
  last_used_plugin: string | null;
  name: string;
  created_at: number;
  last_used_at: number | null;
  expires_at: number | null;
  revoked_at: number | null;
}

function fixed(n: number, salt: number) {
  return Uint8Array.from({ length: 32 }, (_, j) => (n * 151 + j * j * 29 + j * 11 + salt) & 0xff);
}
/** Fixture token `n`'s id and short id. */
export function fixtureToken(n: number) {
  return { id: tokenId(fixed(n, 101)), short: makeToken(fixed(n, 7)).short };
}
/** The token that published Focus timer's 1.2.0: "GitHub Actions", the sixth below. */
export function githubActions() {
  return { ...fixtureToken(5), name: "GitHub Actions" };
}

export function tokens(): FakeToken[] {
  // (Tokens for many of Ada's plugins: the page names three, then counts, and lists the rest on request.)
  const adas = plugins().filter((p) => p.owner === "user_ada").map((p) => p.id).sort();
  type Row = Omit<FakeToken, "id" | "short" | "hash">;
  const rows: Row[] = [
    // (Each token's first three, as its row draws them, three different plugins: no two tiles alike side by side.)
    { owner: "user_ada", scope: { plugins: adas.slice(6) }, last_used_plugin: "focus-timer", name: "GitHub Actions, the monorepo", created_at: NOW - 30 * DAY, last_used_at: NOW - 2 * 3_600_000, expires_at: NOW + 300 * DAY, revoked_at: null },
    { owner: "user_ada", scope: { plugins: ["cloud-costs", "standup-notes", "token-meter", "weather"] }, last_used_plugin: null, name: "Docs site build", created_at: NOW - 6 * DAY, last_used_at: null, expires_at: null, revoked_at: null },
    { owner: "user_ada", scope: { plugins: adas.slice(4, 6) }, last_used_plugin: "focus-timer", name: "An old runner, ended", created_at: NOW - 400 * DAY, last_used_at: NOW - 40 * DAY, expires_at: NOW - 5 * DAY, revoked_at: null },
    // (Three, named in full; and every one of Ada's by name: twenty, or fifty, the most a token may name, with /_fake/many.)
    { owner: "user_ada", scope: { plugins: ["agent-relay", "quick-links", "theme-sync"] }, last_used_plugin: null, name: "Three of them", created_at: NOW - 3 * DAY, last_used_at: null, expires_at: NOW + 200 * DAY, revoked_at: null },
    { owner: "user_ada", scope: { plugins: adas }, last_used_plugin: null, name: "Every one", created_at: NOW - 1 * DAY, last_used_at: null, expires_at: null, revoked_at: null },
    { owner: "user_ada", scope: { plugins: ["focus-timer"] }, last_used_plugin: "focus-timer", name: "GitHub Actions", created_at: NOW - 50 * DAY, last_used_at: NOW - 9 * DAY, expires_at: NOW + 315 * DAY, revoked_at: null },
    { owner: "user_ada", scope: { all: true }, last_used_plugin: "session-badges", name: "Old laptop script", created_at: NOW - 110 * DAY, last_used_at: NOW - 100 * DAY, expires_at: null, revoked_at: NOW - 60 * DAY },
    // The worst case's ten: live and revoked, used and never, ending soon and never.
    ...[
      ["GitHub Actions: release on tag, the main repository's workflow", 40, 1, 300, null],
      ["Nightly build", 90, 0.5, 12, null],
      ["Staging deploy from the CI runner in the second datacenter", 30, null, null, null],
      ["Laptop", 200, 30, 6, null],
      ["Backup runner", 15, null, 365, null],
      ["Release candidate pipeline", 60, 2, null, null],
      ["Old CI", 300, 250, null, 200],
      ["A token that was leaked in a log", 120, 119, 200, 118],
      ["Test", 20, 19, 30, 10],
      ["Contributor's fork", 8, 3, 3, null],
    ].map(([name, made, used, ends, revoked], i) => ({
      // (The worst case is Ada's, as every plugin with no owner of its own is.)
      owner: "user_ada",
      scope: i % 3 === 0 ? ({ all: true } as const) : { plugins: ["worst-case-listing-at-every-limit-xyz12", ...(i % 3 === 2 ? ["focus-timer"] : [])].sort() },
      last_used_plugin: used === null ? null : "worst-case-listing-at-every-limit-xyz12",
      name: name as string,
      created_at: NOW - (made as number) * DAY,
      last_used_at: used === null ? null : NOW - (used as number) * DAY,
      expires_at: ends === null ? null : NOW + (ends as number) * DAY,
      revoked_at: revoked === null ? null : NOW - (revoked as number) * DAY,
    })),
  ];
  return rows.map((row, n) => ({ ...row, ...fixtureToken(n), hash: null }));
}
