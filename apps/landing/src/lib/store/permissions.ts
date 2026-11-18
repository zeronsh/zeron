// What a plugin asks for, in the app's words: the lines of its consent sheet
// (apps/zgx/ui/screens/plugins/words.luau), ported line for line so the web
// and the app say the same thing about the same plugin. The English is the
// app's (apps/zgx/ui/locales/en.json, `plugins.words.*`), held to it by
// permissions.test.ts. One change, listed in ON_THE_WEB: a sheet in the app
// speaks of "this computer", the one it's allowing on; a web page is read
// from anywhere, so it says "your computer".

import type icons from "../../data/icons.json";
import type { Permissions, PermissionsDiff } from "./types.ts";

export type IconName = keyof typeof icons;

export interface Item {
  name: string;
  description?: string;
}

export interface Line {
  title: string;
  detail?: string;
  risky: boolean;
  icon: IconName;
  /** What it names exactly as its manifest writes it, shown under the words. */
  literal?: string[];
  /** Named things with words of their own (an agent tool and what it tells agents). */
  items?: Item[];
}

/** The app's words, by their message key (en.json). */
export const WORDS = {
  "plugins.words.place.window": "the window",
  "plugins.words.place.sidebar": "the sidebar",
  "plugins.words.place.sidebar_row": "a sidebar row",
  "plugins.words.place.title_bar": "the title bar",
  "plugins.words.place.rail": "the sidebar's rail",
  "plugins.words.place.settings": "Settings",
  "plugins.words.place.new_chat": "a new chat",
  "plugins.words.place.new_chat_background": "a new chat's background",
  "plugins.words.place.chat": "a chat",
  "plugins.words.place.chat_header": "a chat's header",
  "plugins.words.place.message": "a message",
  "plugins.words.place.message_box": "the message box",
  "plugins.words.place.files_pane": "the files pane",
  "plugins.words.place.file_row": "a file's row",
  "plugins.words.place.file_tab": "a file's tab",
  "plugins.words.place.open_file": "the open file",
  "plugins.words.place.side_panel": "the side panel",
  "plugins.words.place.chats": "chats",
  "plugins.words.place.main_window": "the main window",
  "plugins.words.place.page_area": "the page area",
  "plugins.words.place.chat_page": "the chat page",
  "plugins.words.place.new_chat_page": "the new chat page",
  "plugins.words.place.zeron": "part of Zeron",
  "plugins.words.place.zeron_parts": "parts of Zeron",
  "plugins.words.list": "{rest} and {last}",
  "plugins.words.list_sep": ", ",
  "plugins.words.none_listed": "none listed",
  "plugins.words.programs": "programs",
  "plugins.words.more": "and {n} more",
  "plugins.words.can_anything": "{name}: can do anything you can",
  "plugins.words.full_control": "It can run {name}, which gives it full control of your computer.",
  "plugins.words.any_site": "Any site on the internet",
  "plugins.words.near_host": "{host}: your computer or your network",
  "plugins.words.details.changes": "Parts of Zeron it changes",
  "plugins.words.details.replaces": "Screens it replaces",
  "plugins.words.permission.ui_add.title": "Add things to Zeron's interface",
  "plugins.words.permission.ui_change.title": "Change how {places} shows",
  "plugins.words.permission.ui_change.detail": "It can hide, reorder, relabel or redraw what's there.",
  "plugins.words.permission.ui_replace.title": "Replace {places} with its own",
  "plugins.words.permission.ui_replace.detail": "Its code runs as part of Zeron: it can do anything you can.",
  "plugins.words.permission.profile_read.title": "See your name, username and picture",
  "plugins.words.permission.sessions_read.title": "See your sessions' titles and status",
  "plugins.words.permission.messages_read.title": "Read messages in the open chat",
  "plugins.words.permission.account_email.title": "See your email address",
  "plugins.words.permission.files_read.title": "See file names and paths in your projects",
  "plugins.words.permission.panel_tabs.title": "Open tabs in your chats' panels",
  "plugins.words.permission.pages_add.title": "Add pages to the sidebar",
  "plugins.words.permission.github.title": "See your pull requests on GitHub",
  "plugins.words.permission.github.detail": "Through the GitHub CLI you're signed in to (gh). The plugin never sees your login.",
  "plugins.words.permission.github_write.title": "Comment on pull requests as you",
  "plugins.words.permission.notifications.title": "Send you notifications",
  "plugins.words.permission.notifications.detail": "Like any other notification, set in Settings → Notifications.",
  "plugins.words.permission.usage_read.title": "See how much of your agents' plans you've used",
  "plugins.words.permission.usage_read.detail": "Each account's usage, its plan and when it resets. Not who the account is.",
  "plugins.words.permission.accounts_read.title": "See your agents' saved accounts",
  "plugins.words.permission.accounts_read.detail": "Which is active, and each one's email and name. Never a password or a sign-in.",
  "plugins.words.permission.accounts_switch.title": "Switch which account your agents use",
  "plugins.words.permission.accounts_switch.detail": "It can make another saved account the active one. It can't sign in or remove an account.",
  "plugins.words.permission.accounts_pin.title": "Run chats on other saved accounts",
  "plugins.words.permission.accounts_pin.detail":
    "It can run a chat on a saved account instead of the active one, and decide the account a new chat starts on. The active account stays as it is. It can't sign in or remove an account.",
  "plugins.words.permission.models_read.title": "See your agents' models",
  "plugins.words.permission.models_read.detail": "The models, efforts and options each offers, and what a chat uses.",
  "plugins.words.permission.models_set.title": "Change the model your chats use",
  "plugins.words.permission.models_set.detail": "A chat's model, effort and options for its next turn, and what new chats start with.",
  "plugins.words.permission.messages_send.title": "Send messages to your agents as you",
  "plugins.words.permission.messages_send.detail":
    "It can send any chat a message, start new chats and answer their questions, as if you'd typed it. Your agents then do what it says.",
  "plugins.words.permission.links_open.title": "Open links",
  "plugins.words.permission.links_open.detail": "Where you open links: Settings → Links.",
  "plugins.words.permission.navigate.title": "Open chats and pages",
  "plugins.words.permission.compose.title": "Put text in the message box",
  "plugins.words.permission.compose.detail": "It can add to what you've written or replace it. You still send it.",
  "plugins.words.permission.draft.title": "Read what you type in the message box, and set it aside",
  "plugins.words.permission.draft.detail":
    "It sees your words in any chat before you send them, and can take the draft away and put it back later. It never sends it.",
  "plugins.words.permission.network.title": "Connect to the internet",
  "plugins.words.permission.network.title_any": "Connect to any site on the internet",
  "plugins.words.permission.network.title_hosts": "Connect to these places",
  "plugins.words.permission.network.detail_named": "A named host can be on your computer or your network.",
  "plugins.words.permission.clipboard.title": "Read and change your clipboard",
  "plugins.words.permission.files.title": "Open and save files you choose",
  "plugins.words.permission.web.title": "Show web pages",
  "plugins.words.permission.windows.title": "Open windows of its own",
  "plugins.words.permission.tools.title": "Give your agents tools",
  "plugins.words.permission.devices.title": "Send data between your own devices",
  "plugins.words.permission.commands.title": "Run {list} on your computer as you",
  "plugins.words.permission.commands.detail": "It can do whatever {list} can do with your sign-ins.",
  "plugins.words.permission.commands.detail_anything": "These can do anything you can, with your sign-ins.",
  "plugins.words.permission.commands.detail_each": "They run as you, with your sign-ins.",
  "plugins.words.permission.native.title": "Run its own program on your computer for your agents",
  "plugins.words.permission.native.detail": "It can do anything you can.",
  "plugins.words.permission.service.title": "Run in the background on your device",
  "plugins.words.permission.service.detail": "So its windows and your agents can use it from any of your devices. It does only what else you allow it.",
  "plugins.words.diff.commands": "Run more programs as you",
  "plugins.words.diff.tools_added": "New tools for your agents",
  "plugins.words.diff.tools_changed": "Tools that now tell your agents something else",
  "plugins.words.diff.network": "Connect to more places",
  "plugins.words.diff.ui_replace": "Replace {places} with its own",
} as const;

/**
 * Where the web's words differ from the app's, and why: [the app's, the web's].
 * (permissions.test.ts reads en.json through these.)
 */
export const ON_THE_WEB: readonly (readonly [string, string])[] = [
  ["this computer", "your computer"],
  ["this device", "your device"],
];

type Key = keyof typeof WORDS;

function say(key: Key, args?: Record<string, string | number>): string {
  return WORDS[key].replace(/\{(\w+)\}/g, (whole, name: string) => (args && name in args ? String(args[name]) : whole));
}

// Where each of Zeron's named elements is, as a person would say it.
const ELEMENTS: Record<string, Key> = {
  "#shell": "plugins.words.place.window",
  "#sidebar": "plugins.words.place.sidebar",
  ".session-row": "plugins.words.place.sidebar_row",
  ".session-title": "plugins.words.place.sidebar_row",
  ".section-header": "plugins.words.place.sidebar",
  "#sidebar-actions": "plugins.words.place.sidebar",
  "#titlebar": "plugins.words.place.title_bar",
  "#titlebar-actions": "plugins.words.place.title_bar",
  "#rail": "plugins.words.place.rail",
  "#settings-nav": "plugins.words.place.settings",
  ".settings-nav-row": "plugins.words.place.settings",
  "#settings-page": "plugins.words.place.settings",
  "#chat-home": "plugins.words.place.new_chat",
  "#new-chat-background": "plugins.words.place.new_chat_background",
  "#chat-session": "plugins.words.place.chat",
  "#session-header": "plugins.words.place.chat_header",
  "#transcript": "plugins.words.place.chat",
  ".message": "plugins.words.place.message",
  ".message-actions": "plugins.words.place.message",
  "#chat-composer": "plugins.words.place.message_box",
  "#composer-toolbar": "plugins.words.place.message_box",
  "#files-pane": "plugins.words.place.files_pane",
  "#file-tree": "plugins.words.place.files_pane",
  "#files-header": "plugins.words.place.files_pane",
  ".file-row": "plugins.words.place.file_row",
  ".file-tab": "plugins.words.place.file_tab",
  "#file-viewer": "plugins.words.place.open_file",
  "#panel": "plugins.words.place.side_panel",
  "#panel-strip": "plugins.words.place.side_panel",
  ".panel-tab": "plugins.words.place.side_panel",
  "#panel-empty": "plugins.words.place.side_panel",
};

// Where a screen's first part puts it.
const AREAS: Record<string, Key> = {
  shell: "plugins.words.place.window",
  sidebar: "plugins.words.place.sidebar",
  rail: "plugins.words.place.rail",
  titlebar: "plugins.words.place.title_bar",
  settings: "plugins.words.place.settings",
  chat: "plugins.words.place.chats",
  files: "plugins.words.place.files_pane",
  panel: "plugins.words.place.side_panel",
};

// A screen module's name, as a person would say it.
const SCREENS: Record<string, Key> = {
  "@app/screens/shell": "plugins.words.place.main_window",
  "@app/screens/shell/sidebar": "plugins.words.place.sidebar",
  "@app/screens/shell/title_bar": "plugins.words.place.title_bar",
  "@app/screens/shell/page": "plugins.words.place.page_area",
  "@app/screens/chat": "plugins.words.place.chat_page",
  "@app/screens/chat/home": "plugins.words.place.new_chat_page",
  "@app/screens/chat/session": "plugins.words.place.chat",
  "@app/screens/chat/composer": "plugins.words.place.message_box",
  "@app/screens/files": "plugins.words.place.files_pane",
};

const own = <T>(map: Record<string, T>, key: string | undefined): T | undefined =>
  key !== undefined && Object.hasOwn(map, key) ? map[key] : undefined;

/** Where a selector or a screen is, as a person would say it. */
export function place(target: string): string {
  const screen = own(SCREENS, target);
  if (screen) return say(screen);
  // A selector's last part (`#a >> .b` is where .b is), then the element it
  // starts with (`.message.message-user` is a message).
  const last = /(?:>>\s*)([^>]*?)\s*$/.exec(target)?.[1] ?? target.trim();
  const element = own(ELEMENTS, last);
  if (element) return say(element);
  const head = /^([#.][\w-]+)/.exec(last)?.[1];
  const headElement = own(ELEMENTS, head);
  if (headElement) return say(headElement);
  const area = own(AREAS, /^@app\/screens\/(\w+)/.exec(target)?.[1]);
  if (area) return say(area);
  return say("plugins.words.place.zeron");
}

/** "the sidebar and the title bar": each place once, in order. */
export function places(targets: readonly string[] | undefined): string {
  const out: string[] = [];
  for (const target of targets ?? []) {
    const said = place(target);
    if (!out.includes(said)) out.push(said);
  }
  if (out.length === 0) return say("plugins.words.place.zeron_parts");
  if (out.length === 1) return out[0]!;
  const last = out.pop()!;
  return say("plugins.words.list", { rest: out.join(say("plugins.words.list_sep")), last });
}

/** `list`'s strings, as given, at most `most` (the rest counted). */
function literal(list: readonly unknown[] | undefined, most = 12): string[] | undefined {
  const out: string[] = [];
  const all = list ?? [];
  for (const [index, value] of all.entries()) {
    if (typeof value !== "string") continue;
    if (index >= most) {
      out.push(say("plugins.words.more", { n: all.length - most }));
      break;
    }
    out.push(value);
  }
  return out.length > 0 ? out : undefined;
}

/**
 * Programs by name, as the host rates them: one in `full` (a shell or an
 * interpreter) says it's full control of the computer, one in `anything`
 * (all of them when there's no rating) that it can do anything you can, the
 * rest (git and gh, what's contained) by name alone.
 */
function programs(names: readonly string[], anything: readonly string[] | undefined, full: readonly string[] | undefined) {
  const said = { list: [] as string[], anything: false, full: false, plain: false };
  for (const name of names) {
    if (full?.includes(name)) {
      said.full = true;
      said.list.push(say("plugins.words.full_control", { name }));
    } else if (anything === undefined || anything.includes(name)) {
      said.anything = true;
      said.list.push(say("plugins.words.can_anything", { name }));
    } else {
      said.plain = true;
      said.list.push(name);
    }
  }
  return said;
}

/** Hosts as written ("*": any site), those in `near` said so. */
function hosts(list: readonly string[], near: readonly string[] | undefined): { list: string[]; near: boolean } {
  const out: string[] = [];
  let anyNear = false;
  for (const host of list) {
    if (host === "*") out.push(say("plugins.words.any_site"));
    else if (near?.includes(host)) {
      anyNear = true;
      out.push(say("plugins.words.near_host", { host }));
    } else out.push(host);
  }
  return { list: out, near: anyNear };
}

function tools(list: readonly { name: string; description?: string }[] | undefined): Item[] | undefined {
  const out: Item[] = [];
  for (const tool of list ?? []) {
    if (typeof tool?.name !== "string") continue;
    out.push(tool.description ? { name: tool.name, description: tool.description } : { name: tool.name });
  }
  return out.length > 0 ? out : undefined;
}

function line(risky: boolean, icon: IconName, title: string, detail?: string, extra: { literal?: string[]; items?: Item[] } = {}): Line {
  const out: Line = { title, risky, icon };
  if (detail !== undefined) out.detail = detail;
  if (extra.literal) out.literal = extra.literal;
  if (extra.items) out.items = extra.items;
  return out;
}

/** One permission's line. */
export function permission(p: Permissions, name: string): Line {
  const ui = p.ui ?? {};
  switch (name) {
    case "ui.add":
      return line(false, "widget", say("plugins.words.permission.ui_add.title"));
    case "ui.change":
      return line(false, "pen", say("plugins.words.permission.ui_change.title", { places: places(ui.change) }), say("plugins.words.permission.ui_change.detail"));
    case "ui.replace":
      return line(true, "danger-triangle", say("plugins.words.permission.ui_replace.title", { places: places(ui.replace) }), say("plugins.words.permission.ui_replace.detail"));
    case "profile.read":
      return line(false, "user-rounded", say("plugins.words.permission.profile_read.title"));
    case "sessions.read":
      return line(false, "list", say("plugins.words.permission.sessions_read.title"));
    case "messages.read":
      return line(false, "chat-round-line", say("plugins.words.permission.messages_read.title"));
    case "account.email":
      return line(false, "eye", say("plugins.words.permission.account_email.title"));
    case "files.read":
      return line(false, "folder", say("plugins.words.permission.files_read.title"));
    case "panel.tabs":
      return line(false, "split-columns", say("plugins.words.permission.panel_tabs.title"));
    case "pages.add":
      return line(false, "list", say("plugins.words.permission.pages_add.title"));
    case "github":
      return line(false, "pull-request", say("plugins.words.permission.github.title"), say("plugins.words.permission.github.detail"));
    case "github.write":
      return line(false, "chat-round-line", say("plugins.words.permission.github_write.title"));
    case "notifications":
      return line(false, "bell", say("plugins.words.permission.notifications.title"), say("plugins.words.permission.notifications.detail"));
    case "usage.read":
      return line(false, "eye", say("plugins.words.permission.usage_read.title"), say("plugins.words.permission.usage_read.detail"));
    case "accounts.read":
      return line(false, "user-rounded", say("plugins.words.permission.accounts_read.title"), say("plugins.words.permission.accounts_read.detail"));
    case "accounts.switch":
      return line(true, "user-rounded", say("plugins.words.permission.accounts_switch.title"), say("plugins.words.permission.accounts_switch.detail"));
    case "accounts.pin":
      return line(true, "user-rounded", say("plugins.words.permission.accounts_pin.title"), say("plugins.words.permission.accounts_pin.detail"));
    case "models.read":
      return line(false, "list", say("plugins.words.permission.models_read.title"), say("plugins.words.permission.models_read.detail"));
    case "models.set":
      return line(true, "pen", say("plugins.words.permission.models_set.title"), say("plugins.words.permission.models_set.detail"));
    case "messages.send":
      return line(true, "chat-round-line", say("plugins.words.permission.messages_send.title"), say("plugins.words.permission.messages_send.detail"));
    case "links.open":
      return line(false, "arrow-up-right", say("plugins.words.permission.links_open.title"), say("plugins.words.permission.links_open.detail"));
    case "navigate":
      return line(false, "map-arrow-right", say("plugins.words.permission.navigate.title"));
    case "compose":
      return line(false, "chat-round-line", say("plugins.words.permission.compose.title"), say("plugins.words.permission.compose.detail"));
    case "draft":
      return line(false, "pen", say("plugins.words.permission.draft.title"), say("plugins.words.permission.draft.detail"));
    case "network": {
      // The places it names (any site, with "*"), those on your computer or
      // your network said so, and risky then.
      const named = p.network_hosts ?? [];
      if (named.length > 0) {
        const listed = hosts(named, p.network_local);
        const title = named.includes("*") ? say("plugins.words.permission.network.title_any") : say("plugins.words.permission.network.title_hosts");
        const detail = (p.network_named?.length ?? 0) > 0 ? say("plugins.words.permission.network.detail_named") : undefined;
        return line(listed.near, "global", title, detail, { literal: listed.list });
      }
      return line(false, "global", say("plugins.words.permission.network.title"));
    }
    case "clipboard":
      return line(false, "copy", say("plugins.words.permission.clipboard.title"));
    case "files":
      return line(false, "document", say("plugins.words.permission.files.title"));
    case "web":
      return line(false, "global", say("plugins.words.permission.web.title"));
    case "windows":
      return line(false, "monitor", say("plugins.words.permission.windows.title"));
    case "tools": {
      // Each tool by name, with what it tells your agents.
      const items = tools(p.tools);
      return line(false, "magic-stick", say("plugins.words.permission.tools.title"), items ? undefined : say("plugins.words.none_listed"), items ? { items } : {});
    }
    case "devices":
      return line(false, "smartphone", say("plugins.words.permission.devices.title"));
    case "service":
      return line(false, "action-play", say("plugins.words.permission.service.title"), say("plugins.words.permission.service.detail"));
    case "commands": {
      // The programs its manifest names, as you: what they can do, it can.
      const names = (p.commands ?? []).filter((c): c is string => typeof c === "string");
      const said = programs(names, p.commands_anything, p.commands_full_control);
      const list = names.length > 0 ? names.join(", ") : say("plugins.words.programs");
      const detail =
        said.full || (said.anything && said.plain)
          ? say("plugins.words.permission.commands.detail_each")
          : said.anything
            ? say("plugins.words.permission.commands.detail_anything")
            : say("plugins.words.permission.commands.detail", { list });
      return line(true, "terminal", say("plugins.words.permission.commands.title", { list }), detail, said.full || said.anything ? { literal: said.list } : {});
    }
    case "native":
      return line(true, "danger-triangle", say("plugins.words.permission.native.title"), say("plugins.words.permission.native.detail"));
  }
  // A permission this site doesn't know yet: named as written, and called
  // out, as the app does.
  return line(true, "danger-triangle", name);
}

/** Each permission's line, the risky ones last. */
export function lines(p: Permissions, names: readonly string[] = p.asks): Line[] {
  const all = names.map((name) => permission(p, name));
  return [...all.filter((l) => !l.risky), ...all.filter((l) => l.risky)];
}

/** What of Zeron's interface it reaches, exactly as it names it. */
export function details(p: Permissions): { title: string; list: string[] }[] {
  const out: { title: string; list: string[] }[] = [];
  const change = literal(p.ui?.change, 40);
  if (change) out.push({ title: say("plugins.words.details.changes"), list: change });
  const replace = literal(p.ui?.replace, 40);
  if (replace) out.push({ title: say("plugins.words.details.replaces"), list: replace });
  return out;
}

/** One ask's line, beside the last approved version's: new, changed inside, or as it was. */
export interface Compared extends Line {
  /** The permission, as the manifest names it. */
  name: string;
  /** Null: as before (or nothing to compare with, a first version). */
  change: "new" | "changed" | null;
  /** Inside an ask it had, what it now names that it didn't (programs, hosts, tools, places)… */
  added?: string[];
  /** …what changed in place (a tool that says something else to agents)… */
  altered?: string[];
  /** …and what it no longer names. */
  removed?: string[];
}

/**
 * What a version asks for, as a reviewer weighs it against the last
 * approved one (the edge's consentDiff): each ask marked new, changed
 * inside (what it now names, changed or dropped), or as before; those that
 * moved first, the risky ones last within each; and the asks it dropped.
 * Without a diff (a first version) nothing is marked: all of it is new.
 */
export function compared(p: Permissions, diff: PermissionsDiff | null): { lines: Compared[]; dropped: Line[] } {
  const added = new Set(diff?.added ?? []);
  const inside = (name: string): Pick<Compared, "added" | "altered" | "removed"> => {
    if (!diff) return {};
    switch (name) {
      case "commands":
        return { added: diff.added_commands, removed: diff.removed_commands };
      case "network":
        return { added: diff.network_added, removed: diff.network_removed };
      case "tools":
        return { added: diff.tools_added?.map((t) => t.name), altered: diff.tools_changed?.map((t) => t.name), removed: diff.tools_removed };
      case "ui.replace":
        return { added: diff.ui_replace_added };
      case "ui.change":
        return { added: diff.ui_change_added };
    }
    return {};
  };
  const all = p.asks.map((name): Compared => {
    const out: Compared = { ...permission(p, name), name, change: null };
    if (!diff) return out;
    if (added.has(name)) return { ...out, change: "new" };
    const moved = inside(name);
    for (const key of ["added", "altered", "removed"] as const) if (moved[key]?.length) out[key] = moved[key];
    if (out.added || out.altered || out.removed) out.change = "changed";
    return out;
  });
  const order = (list: Compared[]) => [...list.filter((l) => !l.risky), ...list.filter((l) => l.risky)];
  return {
    lines: [...order(all.filter((l) => l.change)), ...order(all.filter((l) => !l.change))],
    dropped: (diff?.removed ?? []).map((name) => permission({ asks: [] }, name)),
  };
}
