// The seed's catalog: 30 plugins by six authors (slots 1–6, five each: an
// account claims at most five new ids in seven days), and what each
// version is: at most ten versions an author (the store takes ten an
// account an hour, 50 a day). seed.mjs publishes them through the real CLI; this file only
// says what they are and writes a plugin's files for a version.
//
// The mix is what the store's pages need to show: every category, keywords
// that overlap (search and filters), more plugins than a page holds (24),
// a plugin with four versions, a pre-release, a chain of versions
// published before anyone verified them (superseded), one its author
// yanked, a permission an update adds, programs a plugin runs (contained,
// full control, held to its own rules), named hosts, 3D scenes, READMEs
// with screenshots, one README picture the store can't show, one plugin
// with no screenshots, and versions from a device (signed) and from a
// deploy token (unsigned).

import { deflateSync } from "node:zlib";

/** Each version: `by` "device" (the author's signed-in Zeron, signed) or
 * "token" (a CI checkout published with the author's deploy token,
 * unsigned); `perms`/`hosts`/`commands` replace the plugin's from that
 * version on. A plugin's first version is always the device's: a token
 * never publishes one. */
export const PLUGINS = [
  // ── slot 1 ──
  {
    slot: 1, id: "standup-notes", name: "Standup Notes", categories: ["productivity", "chat"],
    keywords: ["notes", "standup", "daily", "team"], perms: ["ui.add", "messages.read"],
    description: "Turns yesterday's chats into a short standup note: what you did, what's next, and what's blocking you.",
    about: ["Reads the chats you pick and drafts three lines for your standup.", "Keeps a week of notes, so Monday's summary is one click.", "Copies a note as plain text for wherever your team meets."],
    shots: 2,
    versions: [
      { v: "1.0.0", by: "device", notes: "First release." },
      { v: "1.1.0", by: "token", notes: "Notes keep a week of history." },
      { v: "1.2.0", by: "device", notes: "A note can be copied as plain text." },
      { v: "1.3.0", by: "token", notes: "Blockers get their own line." }
    ]
  },
  {
    slot: 1, id: "focus-timer", name: "Focus Timer", categories: ["productivity"],
    keywords: ["focus", "timer", "pomodoro"], perms: ["ui.add", "notifications"],
    description: "A quiet work timer in the corner: 25 minutes on, five off, and a note when it's time for a break.",
    about: ["Work and break lengths you choose.", "A notification at the end of each block.", "Pauses when you leave your desk."],
    shots: 1,
    versions: [{ v: "1.0.0", by: "device", notes: "First release." }]
  },
  {
    slot: 1, id: "quiet-hours", name: "Quiet Hours", categories: ["productivity", "utilities"],
    keywords: ["focus", "notifications", "schedule"], perms: ["ui.add", "notifications"],
    description: "Holds notifications back during the hours you set, and shows what you missed when they end.",
    about: ["Set quiet hours for each day of the week.", "A summary of what was held back.", "Urgent mentions still get through."],
    shots: 1,
    versions: [
      { v: "1.0.0", by: "device", notes: "First release." },
      { v: "1.1.0", by: "token", notes: "Weekend schedules." },
      // (A release candidate its author yanks: a version its author stopped,
      // beside the release that's listed. A pre-release never supersedes a
      // release, and nothing newer supersedes it, so it stays the author's
      // yank: a yanked version that something newer then supersedes is
      // superseded for good, whoever yanked it first.)
      { v: "1.2.0-rc.1", by: "token", notes: "Quiet hours follow your calendar (release candidate).", yank: true }
    ]
  },
  {
    slot: 1, id: "draft-polish", name: "Draft Polish", categories: ["chat"],
    keywords: ["writing", "draft", "review"], perms: ["ui.add", "draft"],
    description: "Reads the message you're writing and suggests a clearer, shorter version before you send it.",
    about: ["Suggestions appear beside your draft, never in it.", "Keeps your tone: it shortens, it doesn't rewrite.", "Undo puts your draft back as it was."],
    shots: 2,
    versions: [{ v: "1.0.0", by: "device", notes: "First release." }]
  },
  {
    slot: 1, id: "meeting-clock", name: "Meeting Clock", categories: ["productivity"],
    keywords: ["timer", "meetings", "focus"], perms: ["ui.add"],
    description: "A countdown for the meeting you're in, so it ends on time.",
    about: ["Start it when the meeting starts.", "A gentle note five minutes before the end."],
    // (No screenshots: a listing without pictures.)
    shots: 0,
    versions: [{ v: "1.0.0", by: "device", notes: "First release." }]
  },
  // ── slot 2 ──
  {
    slot: 2, id: "branch-radar", name: "Branch Radar", categories: ["git", "developer-tools"],
    keywords: ["git", "branches", "review"], perms: ["ui.add", "files.read"],
    description: "Shows every branch in the project you're in, how far each is behind main, and which ones nobody has touched in a month.",
    about: ["Branches sorted by last commit.", "Ahead and behind counts against main.", "Stale branches marked, never deleted."],
    shots: 2,
    // (Three versions in a row with none verified: each supersedes the last.)
    versions: [
      { v: "1.0.0", by: "device", notes: "First release." },
      { v: "1.0.1", by: "token", notes: "Counts refresh after a fetch." },
      { v: "1.0.2", by: "token", notes: "Fixed the count for branches with no upstream." }
    ]
  },
  {
    slot: 2, id: "commit-lens", name: "Commit Lens", categories: ["git"],
    keywords: ["git", "commits", "history", "search"], perms: ["ui.add", "files.read"],
    description: "Search the project's history by message, author or file, and open any commit's changes beside your chat.",
    about: ["Search messages, authors and paths.", "Open a commit's diff in a side panel.", "Copy a commit's hash in one click."],
    shots: 1,
    versions: [
      { v: "1.0.0", by: "device", notes: "First release." },
      { v: "1.1.0", by: "token", notes: "Search by file path." }
    ]
  },
  {
    slot: 2, id: "diff-glance", name: "Diff Glance", categories: ["git", "developer-tools"],
    keywords: ["diff", "review", "git"], perms: ["ui.add", "files.read"],
    description: "A compact summary of what changed in your working tree: files, lines added and removed, at a glance.",
    about: ["Totals per folder.", "Click a file to see its changes."],
    shots: 1,
    versions: [{ v: "1.0.0", by: "device", notes: "First release." }]
  },
  {
    slot: 2, id: "tag-keeper", name: "Tag Keeper", categories: ["git"],
    keywords: ["git", "tags", "releases"], perms: ["ui.add", "files.read", "commands"], commands: ["git"],
    description: "Lists the project's release tags with their notes, and makes the next tag for you with the version you choose.",
    about: ["Every tag with its date and message.", "Suggests the next version number.", "Runs git to make the tag, so your hooks still apply."],
    shots: 1,
    versions: [
      { v: "1.0.0", by: "device", notes: "First release." },
      { v: "1.1.0", by: "token", notes: "Shows which tags are signed." }
    ]
  },
  {
    slot: 2, id: "ci-status", name: "CI Status", categories: ["developer-tools", "integrations"],
    keywords: ["ci", "builds", "status"], perms: ["ui.add", "network"], hosts: ["ci.example.com"],
    description: "The state of your last builds in the sidebar: passing, failing or running, with a link to each one.",
    about: ["Polls your CI once a minute.", "A badge when a build fails.", "Reaches only the host you name."],
    shots: 1,
    versions: [{ v: "1.0.0", by: "device", notes: "First release." }]
  },
  // ── slot 3 ──
  {
    slot: 3, id: "palette-swap", name: "Palette Swap", categories: ["appearance"],
    keywords: ["theme", "colors", "palette"], perms: ["ui.change"],
    description: "Switch the window's colours between palettes you make, and keep one per project.",
    about: ["Make a palette from any accent colour.", "One palette per project, switched as you move.", "Light and dark versions of each."],
    shots: 2,
    // (A pre-release after a release.)
    versions: [
      { v: "1.0.0", by: "device", notes: "First release." },
      { v: "2.0.0-beta.1", by: "token", notes: "Palettes per project (beta)." }
    ]
  },
  {
    slot: 3, id: "dusk-mode", name: "Dusk Mode", categories: ["appearance"],
    keywords: ["theme", "dark", "schedule"], perms: ["ui.change"],
    description: "Moves the window to its dark colours at sunset and back at sunrise, or at the times you set.",
    about: ["Follows sunset where you are, or a schedule.", "A slow fade, not a flash."],
    shots: 1,
    versions: [
      { v: "1.0.0", by: "device", notes: "First release." },
      { v: "1.1.0", by: "token", notes: "Your own times instead of sunset." }
    ]
  },
  {
    slot: 3, id: "font-tuner", name: "Font Tuner", categories: ["appearance"],
    keywords: ["fonts", "typography", "theme"], perms: ["ui.change"],
    description: "Pick the fonts, sizes and line spacing for chats and code separately.",
    about: ["Separate settings for prose and code.", "Previews as you change them."],
    shots: 1,
    versions: [{ v: "1.0.0", by: "device", notes: "First release." }]
  },
  {
    slot: 3, id: "compact-sidebar", name: "Compact Sidebar", categories: ["appearance", "productivity"],
    keywords: ["sidebar", "layout"], perms: ["ui.change"],
    description: "A narrower sidebar with icons only, so the chat gets the room.",
    about: ["Icons only, names on hover.", "Toggle with one shortcut."],
    shots: 1,
    // (Its README shows a GIF too: a picture the store can't show.)
    gif: true,
    versions: [{ v: "1.0.0", by: "device", notes: "First release." }]
  },
  {
    slot: 3, id: "orbit-view", name: "Orbit View", categories: ["appearance", "utilities"],
    keywords: ["3d", "scene", "visual"], perms: ["ui.add", "scene3d"],
    description: "Draws your open sessions as planets in a small 3D scene: the busier one is, the larger it grows.",
    about: ["A 3D scene in a panel.", "Click a planet to open its session."],
    shots: 1,
    versions: [{ v: "1.0.0", by: "device", notes: "First release." }]
  },
  // ── slot 4 ──
  {
    slot: 4, id: "link-preview", name: "Link Preview", categories: ["chat", "integrations"],
    keywords: ["links", "preview", "web"], perms: ["ui.add"],
    description: "Shows the title and summary of a link in your chat before you open it.",
    about: ["Titles for links you paste.", "From 1.1.0, summaries fetched from one named host."],
    shots: 1,
    // (An update that asks for more: network, with a host.)
    versions: [
      { v: "1.0.0", by: "device", notes: "First release." },
      { v: "1.1.0", by: "token", notes: "Summaries, fetched from the preview service.", perms: ["ui.add", "network"], hosts: ["preview.example.com"] }
    ]
  },
  {
    slot: 4, id: "weather-strip", name: "Weather Strip", categories: ["utilities", "integrations"],
    keywords: ["weather", "forecast"], perms: ["ui.add", "network"], hosts: ["api.open-meteo.com"],
    description: "Today's weather and the next six hours in a thin strip above your chat.",
    about: ["Hourly forecast for where you are.", "Reaches one weather service, nothing else."],
    shots: 1,
    versions: [
      { v: "1.0.0", by: "device", notes: "First release." },
      { v: "1.1.0", by: "token", notes: "Rain chance per hour." }
    ]
  },
  {
    slot: 4, id: "clipboard-shelf", name: "Clipboard Shelf", categories: ["utilities"],
    keywords: ["clipboard", "snippets"], perms: ["ui.add", "clipboard"],
    description: "Keeps the last 20 things you copied, so you can paste any of them again.",
    about: ["The last 20 copies, newest first.", "Pin the ones you use every day."],
    shots: 1,
    versions: [{ v: "1.0.0", by: "device", notes: "First release." }]
  },
  {
    slot: 4, id: "shell-snippets", name: "Shell Snippets", categories: ["developer-tools", "utilities"],
    keywords: ["shell", "snippets", "terminal"], perms: ["ui.add", "commands"], commands: ["date", "python3"],
    description: "Saved shell snippets you run with one click: the date for a log line, or a short Python script.",
    about: ["Snippets saved per project.", "Runs date and python3, as you.", "Output pasted into your draft."],
    shots: 1,
    versions: [
      { v: "1.0.0", by: "device", notes: "First release." },
      { v: "1.1.0", by: "token", notes: "Snippets can take an argument." }
    ]
  },
  {
    slot: 4, id: "feed-reader", name: "Feed Reader", categories: ["integrations"],
    keywords: ["rss", "news", "reading"], perms: ["ui.add", "network"], hosts: ["feeds.example.com"],
    description: "Follow a few news feeds in a side panel and send an article to a chat to talk about it.",
    about: ["Unread counts per feed.", "Send an article to a chat."],
    shots: 2,
    versions: [{ v: "1.0.0", by: "device", notes: "First release." }]
  },
  // ── slot 5 ──
  {
    slot: 5, id: "agent-roster", name: "Agent Roster", categories: ["agents"],
    keywords: ["agents", "models", "overview"], perms: ["ui.add", "sessions.read", "models.read"],
    description: "Every agent session you have open, what model each runs, and which ones are waiting for you.",
    about: ["Sessions grouped by project.", "Waiting sessions first.", "The model each one runs."],
    shots: 2,
    versions: [
      { v: "1.0.0", by: "device", notes: "First release." },
      { v: "1.1.0", by: "token", notes: "Group by project." },
      { v: "1.2.0", by: "token", notes: "Waiting sessions sort first." }
    ]
  },
  {
    slot: 5, id: "prompt-library", name: "Prompt Library", categories: ["agents", "productivity"],
    keywords: ["prompts", "templates", "writing"], perms: ["ui.add", "compose"],
    description: "Your saved prompts, searchable, inserted into the composer with one click.",
    about: ["Folders and tags for prompts.", "Placeholders you fill in as you insert."],
    shots: 1,
    versions: [
      { v: "1.0.0", by: "device", notes: "First release." },
      { v: "1.1.0", by: "token", notes: "Placeholders in prompts." }
    ]
  },
  {
    slot: 5, id: "usage-meter", name: "Usage Meter", categories: ["agents", "utilities"],
    keywords: ["usage", "costs", "models"], perms: ["ui.add", "usage.read"],
    description: "How much you've used each model this week, as a small chart in the sidebar.",
    about: ["Usage per model per day.", "A week at a glance."],
    shots: 1,
    versions: [{ v: "1.0.0", by: "device", notes: "First release." }]
  },
  {
    slot: 5, id: "review-queue", name: "Review Queue", categories: ["agents", "developer-tools"],
    keywords: ["review", "agents", "diff"], perms: ["ui.add", "sessions.read", "messages.read"],
    description: "Collects the changes your agents finished into one list to review, oldest first.",
    about: ["One list across sessions.", "Mark each change reviewed."],
    shots: 1,
    versions: [{ v: "1.0.0", by: "device", notes: "First release." }]
  },
  {
    slot: 5, id: "model-switcher", name: "Model Switcher", categories: ["agents"],
    keywords: ["models", "switching"], perms: ["ui.add", "models.read", "models.set"],
    description: "Switch a session's model from a menu in its header, with your favourites first.",
    about: ["Favourite models first.", "Remembers the last model per project."],
    shots: 1,
    versions: [{ v: "1.0.0", by: "device", notes: "First release." }]
  },
  // ── slot 6 ──
  {
    slot: 6, id: "tab-groups", name: "Tab Groups", categories: ["productivity"],
    keywords: ["tabs", "panel", "organize"], perms: ["ui.add", "panel.tabs"],
    description: "Group the side panel's tabs by task and switch between groups.",
    about: ["Name a group, drag tabs into it.", "Switch groups from the panel's header."],
    shots: 1,
    versions: [
      { v: "1.0.0", by: "device", notes: "First release." },
      { v: "1.1.0", by: "token", notes: "Groups remember their order." }
    ]
  },
  {
    slot: 6, id: "page-pins", name: "Page Pins", categories: ["productivity", "utilities"],
    keywords: ["pages", "bookmarks"], perms: ["ui.add", "pages.add"],
    description: "Pin the pages you open every day to a row at the top, one click away.",
    about: ["Up to eight pins.", "Drag to reorder."],
    shots: 1,
    versions: [{ v: "1.0.0", by: "device", notes: "First release." }]
  },
  {
    slot: 6, id: "window-tiles", name: "Window Tiles", categories: ["utilities"],
    keywords: ["windows", "layout", "tiling"], perms: ["ui.add", "windows"],
    description: "Arranges Zeron's windows side by side, in halves or thirds, with one shortcut.",
    about: ["Halves, thirds and quarters.", "Remembers each window's last place."],
    shots: 1,
    versions: [{ v: "1.0.0", by: "device", notes: "First release." }]
  },
  {
    slot: 6, id: "file-finder", name: "File Finder", categories: ["utilities", "developer-tools"],
    keywords: ["files", "search"], perms: ["ui.add", "files.read"],
    description: "Find a file in the project by part of its name, and drop it into your chat.",
    about: ["Fuzzy search on file names.", "Drop a file into the chat as a reference."],
    shots: 1,
    versions: [
      { v: "1.0.0", by: "device", notes: "First release." },
      { v: "1.1.0", by: "token", notes: "Recent files first." }
    ]
  },
  {
    slot: 6, id: "notice-board", name: "Notice Board", categories: ["chat", "productivity"],
    keywords: ["notifications", "inbox"], perms: ["ui.add", "notifications"],
    description: "All your mentions and replies in one board, sorted by who's waiting longest.",
    about: ["Mentions and replies together.", "Oldest waiting first."],
    shots: 2,
    versions: [{ v: "1.0.0", by: "device", notes: "First release." }]
  }
];

/** The plugin as it stands at version index `i`: its permissions, hosts and
 * commands as of that version. */
export const at = (plugin, i) => {
  let perms = plugin.perms;
  let hosts = plugin.hosts ?? [];
  let commands = plugin.commands ?? [];
  for (const v of plugin.versions.slice(0, i + 1)) {
    if (v.perms) perms = v.perms;
    if (v.hosts) hosts = v.hosts;
    if (v.commands) commands = v.commands;
  }
  return { perms, hosts, commands, version: plugin.versions[i] };
};

// ── Files ───────────────────────────────────────────────────────────────────

/** Each plugin's own colour, from its id. */
const hue = (id) => [...id].reduce((h, c) => (h * 31 + c.charCodeAt(0)) % 360, 7);
const rgb = (h, s, l) => {
  const a = s * Math.min(l, 1 - l);
  const f = (n) => {
    const k = (n + h / 30) % 12;
    return Math.round(255 * (l - a * Math.max(-1, Math.min(k - 3, 9 - k, 1))));
  };
  return [f(0), f(8), f(4)];
};
const hex = ([r, g, b]) => `#${[r, g, b].map((x) => x.toString(16).padStart(2, "0")).join("")}`;

export const iconSvg = (plugin) => {
  const h = hue(plugin.id);
  const fill = hex(rgb(h, 0.62, 0.48));
  const light = hex(rgb((h + 40) % 360, 0.7, 0.72));
  const letter = plugin.name[0];
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><rect width="64" height="64" rx="14" fill="${fill}"/><circle cx="46" cy="18" r="9" fill="${light}"/><text x="32" y="44" font-family="sans-serif" font-size="30" font-weight="700" text-anchor="middle" fill="#ffffff">${letter}</text></svg>\n`;
};

const CRC = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c >>> 0;
  }
  return t;
})();
const crc32 = (buf) => {
  let c = 0xffffffff;
  for (const b of buf) c = CRC[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
};
const chunk = (type, data) => {
  const out = Buffer.alloc(12 + data.length);
  out.writeUInt32BE(data.length, 0);
  out.write(type, 4, "ascii");
  data.copy(out, 8);
  out.writeUInt32BE(crc32(out.subarray(4, 8 + data.length)), 8 + data.length);
  return out;
};

/** A screenshot: a window drawn in the plugin's colour (a title bar, a
 * sidebar, a panel of rows), `n` making each picture of a plugin differ.
 * A real PNG, as the store reads one. */
export const screenshotPng = (plugin, n) => {
  const w = 960;
  const h = 600;
  const accent = rgb(hue(plugin.id), 0.6, 0.5);
  const soft = rgb(hue(plugin.id), 0.45, 0.9);
  const raw = Buffer.alloc((w * 3 + 1) * h);
  let seed = hue(plugin.id) * 7 + n * 13;
  const rows = Array.from({ length: 9 }, () => {
    seed = (seed * 1103515245 + 12345) % 2147483648;
    return 180 + (seed % 420);
  });
  for (let y = 0; y < h; y++) {
    const at = y * (w * 3 + 1);
    raw[at] = 0;
    for (let x = 0; x < w; x++) {
      let c = [246, 246, 248];
      if (y < 44) c = accent;
      else if (x < 210) c = [232, 233, 238];
      else {
        const row = Math.floor((y - 80) / 52);
        const inRow = (y - 80) % 52;
        if (row >= 0 && row < rows.length && inRow > 12 && inRow < 34 && x > 250 && x < 250 + rows[row]) c = row === n % rows.length ? accent : soft;
      }
      if (x < 210 && y > 70 && (y - 70) % 40 < 16 && x > 24 && x < 180) c = [208, 210, 218];
      raw[at + 1 + x * 3] = c[0];
      raw[at + 2 + x * 3] = c[1];
      raw[at + 3 + x * 3] = c[2];
    }
  }
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(w, 0);
  ihdr.writeUInt32BE(h, 4);
  ihdr[8] = 8; // depth
  ihdr[9] = 2; // RGB
  return Buffer.concat([Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]), chunk("IHDR", ihdr), chunk("IDAT", deflateSync(raw)), chunk("IEND", Buffer.alloc(0))]);
};

/** A 1×1 GIF: a picture a README may show that the store can't. */
export const GIF = Buffer.from("47494638396101000100800000000000ffffff21f90401000000002c00000000010001000002024401003b", "hex");

const shotPaths = (plugin) => Array.from({ length: plugin.shots }, (_, n) => `shots/${n + 1}.png`);

export const readme = (plugin) => {
  const lines = [`# ${plugin.name}`, "", plugin.description, "", "## What it does", "", ...plugin.about.map((a) => `- ${a}`), ""];
  for (const path of shotPaths(plugin)) lines.push(`![${plugin.name}](${path})`, "");
  if (plugin.gif) lines.push("It folds away like this:", "", "![Folding the sidebar](shots/fold.gif)", "");
  lines.push("## Permissions", "", `It asks for ${plugin.perms.map((p) => `\`${p}\``).join(", ")}${plugin.hosts?.length ? `, and reaches ${plugin.hosts.join(", ")} only` : ""}${plugin.commands?.length ? `; it runs ${plugin.commands.join(" and ")}` : ""}.`, "");
  return `${lines.join("\n")}`;
};

export const changelog = (plugin, i) => {
  const parts = ["# Changes", ""];
  for (const v of plugin.versions.slice(0, i + 1).reverse()) parts.push(`## ${v.v}`, "", `- ${v.notes}`, "");
  return parts.join("\n");
};

/** The manifest fields the seed sets (over what `zeron plugin new` made). */
export const manifestFields = (plugin, i) => {
  const now = at(plugin, i);
  return {
    id: plugin.id,
    name: plugin.name,
    version: now.version.v,
    description: plugin.description,
    icon: "icon.svg",
    categories: plugin.categories,
    keywords: plugin.keywords,
    permissions: now.perms,
    ...(now.hosts.length ? { network: { hosts: now.hosts } } : {}),
    ...(now.commands.length ? { commands: now.commands } : {}),
    screenshots: shotPaths(plugin),
    license: "MIT",
    homepage: `https://example.com/plugins/${plugin.id}`
  };
};

/** Every file the seed writes for `plugin` at version index `i` (beside
 * what `zeron plugin new` made: its entry file), path → bytes. */
export const files = (plugin, i) => {
  const out = new Map([
    ["icon.svg", Buffer.from(iconSvg(plugin))],
    ["README.md", Buffer.from(readme(plugin))],
    ["CHANGELOG.md", Buffer.from(changelog(plugin, i))]
  ]);
  shotPaths(plugin).forEach((path, n) => out.set(path, screenshotPng(plugin, n)));
  if (plugin.gif) out.set("shots/fold.gif", GIF);
  return out;
};
