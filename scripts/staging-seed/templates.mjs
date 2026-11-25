// The seed's templates: eight by two authors (slots 7 and 8, four each: an
// account claims at most five new ids in seven days, plugins and templates
// together). seed-templates.mjs publishes them through the real paths; this
// file only says what they are and writes a template's files for a version.
//
// Each is one Zeron runs: a palette of its own (theme.luau, data), and for
// some a changed copy of one of Zeron's files, an unchanged one, or a page of
// its own, so a review has files that are the same as Zeron's, changed and
// new. Zeron's own files are read from a checkout of the release named
// (`ui`: its apps/zgx/ui).
//
// The mix: light and dark, keywords that overlap, versions from a device
// (signed) and from a deploy token (unsigned), a pre-release beside a
// release, one staff never verify (green-phosphor) and one they take down
// (slate-grid).

import { readFileSync } from "node:fs";
import { join } from "node:path";

/** The Zeron they were made from. */
export const ZERON = "0.3.20";

/** What each is drawn and coloured with: `scheme` and its palette. A
 * version's `like` is the earlier one (by index) whose files it has, but its
 * words, pictures, number and notes, and those put right (a home page or
 * tokens made over Zeron's current file): `gap` its list spacing, `width` how
 * wide its chats start, `leading` its replies' lines, `plain` no tokens; `rendered`: its pictures are the app's
 * own rendering of it (seed-templates.mjs `--pictures`), not drawn here, and
 * its words (`words`) say what its files change and no more. */
export const TEMPLATES = [
  // ── slot 7 ──
  {
    slot: 7, id: "nightfall-cards", name: "Nightfall Cards", keywords: ["dark", "cards", "focus"],
    description: "A deep blue night for long sessions: roomy rows, soft cards and one warm lamp of an accent.",
    about: ["A midnight palette, easy on the eyes after dark.", "Sidebar rows with more air between them.", "Everything else is as you know it."],
    words: { keywords: ["dark", "palette", "spacing", "focus"], description: "A deep blue dark palette with a warm accent, and more space between the rows of a list.", about: ["A dark palette of its own, code's colours included."] },
    look: { scheme: "dark", layout: "cards", background: "#0d1326", raised: "#161f3d", text: "#e6eaf7", muted: "#a3adcc", faint: "#6f7a9e", accent: "#ffb454", danger: "#ff6b6b", warning: "#ffd166", success: "#6ee7a8", radius: 18 },
    change: "roomy", keep: ["ds/icon_button.luau"],
    versions: [
      { v: "1.0.0", by: "device", notes: "First release." },
      { v: "1.1.0", by: "token", notes: "Rows have a little more air." },
      { v: "1.1.1", by: "device", like: 1, gap: 4, rendered: true, notes: "Its pictures and words now say what it is, and what was wrong in its files is put right." }
    ]
  },
  {
    slot: 7, id: "paper-desk", name: "Paper Desk", keywords: ["light", "warm", "reading"],
    description: "Warm paper, dark ink and a red pencil, for people who read more than they type.",
    about: ["A cream page with ink that keeps its contrast.", "A quieter welcome on a new chat."],
    words: { description: "A warm, paper-coloured light palette, and a new-chat page with its own words.", about: ["A light palette of its own, code's colours included."] },
    look: { scheme: "light", layout: "desk", background: "#f4ecdd", raised: "#fbf6ec", text: "#2c2419", muted: "#6b5e4b", faint: "#978a76", accent: "#b5442e", danger: "#b3261e", warning: "#9a6700", success: "#3c7a3f", radius: 6 },
    home: ["A clean sheet.", "Write the first line, or drop a file on the desk."],
    versions: [
      { v: "1.0.0", by: "device", notes: "First release." },
      { v: "1.0.1", by: "token", like: 0, rendered: true, notes: "Its pictures and words now say what it is, and what was wrong in its files is put right." }
    ]
  },
  {
    slot: 7, id: "green-phosphor", name: "Green Phosphor", keywords: ["dark", "retro", "terminal"],
    description: "One green on black, as screens were when they hummed.",
    about: ["A single hue, in four strengths.", "A prompt for a welcome."],
    words: { description: "A dark palette of greens on near-black, and a new-chat page that says READY.", about: ["A dark palette of its own, code's colours included."] },
    look: { scheme: "dark", layout: "terminal", background: "#020a04", raised: "#06140a", text: "#8dffa6", muted: "#4fbf6b", faint: "#2f8047", accent: "#33ff66", danger: "#ff7a6b", warning: "#e8ff6b", success: "#33ff66", radius: 0 },
    home: ["READY.", "Type a command and press return."],
    versions: [
      { v: "1.0.0", by: "device", notes: "First release." },
      { v: "1.0.1", by: "token", like: 0, rendered: true, notes: "Its pictures and words now say what it is, and what was wrong in its files is put right." }
    ]
  },
  {
    slot: 7, id: "wide-margin", name: "Wide Margin", keywords: ["light", "minimal", "focus"],
    description: "White space first: a pale page, hairlines and nothing that shouts.",
    about: ["A near-white palette with one blue.", "More room in every list."],
    words: { description: "A near-white light palette with one blue, a narrower column for chats, and more space between the rows of a list.", about: ["A light palette of its own, code's colours included."] },
    look: { scheme: "light", layout: "margin", background: "#fcfcfb", raised: "#ffffff", text: "#1c1d21", muted: "#5d6068", faint: "#8b8e96", accent: "#2f5bea", danger: "#c62828", warning: "#8a6100", success: "#1f7a4d", radius: 10 },
    change: "roomy",
    versions: [
      { v: "1.0.0", by: "device", notes: "First release." },
      { v: "1.1.0-beta.1", by: "device", notes: "Trying even more room. Tell me if it's too much." },
      { v: "1.1.0", by: "token", like: 0, gap: 6, width: 560, rendered: true, notes: "Its pictures and words now say what it is, and what was wrong in its files is put right." },
      { v: "1.2.0-beta.1", by: "device", like: 1, gap: 6, width: 560, leading: 28, rendered: true, notes: "Lines in a reply are further apart, beside the release." }
    ]
  },
  // ── slot 8 ──
  {
    slot: 8, id: "coral-reef", name: "Coral Reef", keywords: ["dark", "colour", "warm"],
    description: "Deep teal water with coral, sand and sea glass.",
    about: ["A saturated dark palette that stays legible.", "Its code colours are the reef's too."],
    words: { description: "A deep teal dark palette with coral, sand and sea glass.", about: ["A dark palette of its own, code's colours included."] },
    look: { scheme: "dark", layout: "reef", background: "#04282c", raised: "#0a3a3f", text: "#e9fbf7", muted: "#9fcfc9", faint: "#6a9f9a", accent: "#ff7f6a", danger: "#ff5c7a", warning: "#ffd08a", success: "#7be0b5", radius: 24 },
    keep: ["ds/icon_button.luau"],
    versions: [
      { v: "1.0.0", by: "device", notes: "First release." },
      { v: "1.1.0", by: "token", notes: "Warmer coral, clearer comments." },
      { v: "1.1.1", by: "device", like: 1, rendered: true, notes: "Its pictures and words now say what it is, and what was wrong in its files is put right." }
    ]
  },
  {
    slot: 8, id: "slate-grid", name: "Slate Grid", keywords: ["dark", "cards", "dense"],
    description: "Cool slate and straight lines, for screens full of work.",
    about: ["A neutral grey palette.", "A grid's worth of calm."],
    words: { keywords: ["dark", "palette"], description: "A cool slate dark palette with a blue accent.", about: ["A dark palette of its own, code's colours included."] },
    look: { scheme: "dark", layout: "grid", background: "#1f242b", raised: "#2a313a", text: "#e8ebef", muted: "#adb5c0", faint: "#7c8693", accent: "#7aa2ff", danger: "#ff6b6b", warning: "#f2c14e", success: "#63d297", radius: 4 },
    versions: [
      { v: "1.0.0", by: "device", notes: "First release." },
      { v: "1.0.1", by: "token", like: 0, rendered: true, notes: "Its pictures and words now say what it is, and what was wrong in its files is put right." }
    ]
  },
  {
    slot: 8, id: "sunday-morning", name: "Sunday Morning", keywords: ["light", "colour", "calm"],
    description: "Pastels, rounded corners and late light through a window.",
    about: ["Peach, lilac and mint, kept readable.", "A slower welcome on a new chat."],
    words: { description: "A soft peach and lilac light palette, and a new-chat page with its own words.", about: ["A light palette of its own, code's colours included."] },
    look: { scheme: "light", layout: "pastel", background: "#fff3ee", raised: "#ffffff", text: "#3a2a3f", muted: "#74617a", faint: "#9c8aa1", accent: "#8a5cd6", danger: "#c2334d", warning: "#8f5f00", success: "#2e7d5b", radius: 28 },
    home: ["Nothing's urgent.", "What would be nice to get done?"],
    versions: [
      { v: "1.0.0", by: "device", notes: "First release." },
      { v: "1.0.1", by: "token", like: 0, rendered: true, notes: "Its pictures and words now say what it is, and what was wrong in its files is put right." }
    ]
  },
  {
    slot: 8, id: "contrast-plus", name: "Contrast Plus", keywords: ["light", "contrast", "reading"],
    description: "Black on white with strong edges, for bright rooms and tired eyes.",
    about: ["Every text colour at 7:1 or better on the page.", "One strong blue for what you can press."],
    words: { description: "A black-on-white light palette of strong contrast.", about: ["A light palette of its own: every text colour in it is at least 7:1 on its page."] },
    look: { scheme: "light", layout: "contrast", background: "#ffffff", raised: "#ffffff", text: "#000000", muted: "#303030", faint: "#4a4a4a", accent: "#0037b3", danger: "#9b0000", warning: "#5c3d00", success: "#00501f", radius: 2 },
    change: "roomy",
    versions: [
      { v: "1.0.0", by: "device", notes: "First release." },
      { v: "1.0.1", by: "token", like: 0, plain: true, rendered: true, notes: "Its pictures and words now say what it is, and what was wrong in its files is put right." }
    ]
  }
];

const quote = (text) => JSON.stringify(text);

/** Its palette, as data (docs/templates.md "Colours and themes"). */
const theme = ({ name, look }, shift) => {
  const l = { ...look, ...(shift ? { accent: shift } : {}) };
  const syntax = [l.faint, l.accent, l.success, l.warning, l.warning, l.accent, l.text, l.text, l.muted, l.danger, l.warning, l.danger];
  const fields = ["background", "raised", "text", "muted", "faint", "accent", "danger", "warning", "success"].map((k) => `\t\t${k} = ${quote(l[k])},`);
  return `return {\n\tname = ${quote(name)},\n\t${l.scheme} = {\n${fields.join("\n")}\n\t\tsyntax = { ${syntax.map(quote).join(", ")} },\n\t},\n}\n`;
};

/** A new-chat page of its own: the guide's example, in its words. */
const home = ([title, line]) => `-- The new-chat page: its own words, over the composer.
local shared = require("@app/screens/chat/shared")

local function Home(): Node
	local zone = shared.drop_zone("home")
	return ui.column {
		id = "chat-home",
		hook = "chat.home",
		w_full = true,
		h_full = true,
		min_h = 0,
		items = "center",
		justify = "center",
		gap = "$space.3",
		on_file_hover = zone.on_file_hover,
		on_file_drop = zone.on_file_drop,
		drop_when = zone.drop_when,
		on_drag_over = zone.on_drag_over,
		on_drag_leave = zone.on_drag_leave,
		on_drop = zone.on_drop,
		ui.text { ${quote(title)}, text_size = "$font.size.xl", weight = "$font.weight.semibold", color = "$text.primary" },
		ui.text { ${quote(line)}, color = "$text.tertiary" },
		ui.slot("chat.home", { w_full = true, items = "center", gap = "$space.3", mb = "$space.8" }),
	}
end

return Home
`;

/** Zeron's own new-chat page and one thing more: words of its own over the
 * composer, which hangs from over the page's middle and grows down. All of
 * Zeron's is as it is, its first-launch screen included (the words go then). */
const homeOverZeron = (ui, [title, line]) => {
  const own = readFileSync(join(ui, "screens/chat/home.luau"), "utf8");
  const end = "\t}\nend\n\nreturn Home\n";
  if (!own.endsWith(end) || own.split("route.needs_project:get()").length !== 3) throw new Error("templates: Zeron's home.luau isn't as it was");
  return `${own.slice(0, -end.length)}\t\t-- Its own words, over the composer (which stands from the page's middle up).
\t\tui.column {
\t\t\tabsolute = true,
\t\t\tleft = 0,
\t\t\tright = 0,
\t\t\tbottom = "50%",
\t\t\tpb = ui.raw(140, "the composer's top over the page's middle, and room under the words"),
\t\t\titems = "center",
\t\t\tgap = "$space.2",
\t\t\tpointer_events = "none",
\t\t\thidden = function(): boolean
\t\t\t\treturn route.needs_project:get()
\t\t\tend,
\t\t\tui.text { ${quote(title)}, text_size = "$font.size.xl", weight = "$font.weight.semibold", color = "$text.primary" },
\t\t\tui.text { ${quote(line)}, color = "$text.tertiary" },
\t\t},
${end}`;
};

/** Zeron's `path` with each of `changes`, [its own text, the template's]: a
 * text that isn't there once stops the seed. */
const over = (ui, path, changes) => {
  let text = readFileSync(join(ui, path), "utf8");
  for (const [own, its] of changes) {
    if (text.split(own).length !== 2) throw new Error(`templates: Zeron's ${path} hasn't ${JSON.stringify(own)} once`);
    text = text.replace(own, () => its);
  }
  return text;
};

/** Zeron's own tokens, with more room in every list, and where it says so
 * between the lines of a reply. */
const roomy = (ui, gap, leading) =>
  over(ui, "ds/tokens.luau", [["list = { gap = 2 },", `list = { gap = ${gap} },`], ...(leading ? [["prose = {\n\t\t\t\tsize = 15,\n\t\t\t\tline = 24,", `prose = {\n\t\t\t\tsize = 15,\n\t\t\t\tline = ${leading},`]] : [])]);

/** Zeron's own preferences, chats starting `width` wide: the default alone (a
 * width the person set is kept). */
const narrower = (ui, width) => over(ui, "screens/settings/prefs.luau", [['ui.persist("settings_conversation_width", 660)', `ui.persist("settings_conversation_width", ${width})`]]);

/** How far apart `t`'s list rows are at version index `i` (Zeron's are 2). */
const gapOf = (t, i) => t.versions[i].gap ?? 4 + 2 * (t.versions[i].like ?? i);

/** What's said of `t` at version index `i`. */
const wordsOf = (t, i) => {
  if (!t.versions[i].rendered) return t;
  const about = [...t.words.about];
  const { width, leading, plain } = t.versions[i];
  if (width) about.push(`Chats are ${width} wide at most, where Zeron's start at 660. A width you set in Settings still wins.`);
  if (t.change === "roomy" && !plain) about.push(`The rows of a list are ${gapOf(t, i)} apart (Zeron's are 2).`);
  if (leading) about.push(`The lines of a reply are ${leading} apart (Zeron's are 24).`);
  if (t.home) about.push(`The new-chat page has words of its own: "${t.home.join(" ")}"`);
  return { description: t.words.description, keywords: t.words.keywords ?? t.keywords, about };
};

const readme = (t, i) =>
  [`# ${t.name}`, "", wordsOf(t, i).description, "", "## What it changes", "", ...wordsOf(t, i).about.map((a) => `- ${a}`), "", `![${t.name}](screenshots/chat.png)`, "", `Made from Zeron ${ZERON}. What it doesn't change is Zeron's own.`, ""].join("\n");

const changelog = (t, i) => ["# Changes", "", ...t.versions.slice(0, i + 1).reverse().flatMap((v) => [`## ${v.v}`, "", `- ${v.notes}`, ""])].join("\n");

export const manifest = (t, i) => ({
  id: t.id,
  name: t.name,
  description: wordsOf(t, i).description,
  version: t.versions[i].v,
  zeron_version: ZERON,
  keywords: wordsOf(t, i).keywords,
  screenshots: ["screenshots/chat.png", "screenshots/new-chat.png"],
  license: "MIT",
  homepage: `https://example.com/templates/${t.id}`
});

/** Every file of `t` at version index `i`, path → bytes: `ui` is Zeron's own
 * (apps/zgx/ui of the release), `pictures` its three. */
export const files = (t, i, { ui, pictures }) => {
  const as = t.versions[i].like ?? i;
  const out = new Map([
    ["zeron-template.json", Buffer.from(`${JSON.stringify(manifest(t, i), null, 2)}\n`)],
    ["README.md", Buffer.from(readme(t, i))],
    ["CHANGELOG.md", Buffer.from(changelog(t, i))],
    ["theme.luau", Buffer.from(theme(t, as > 0 && t.id === "coral-reef" ? "#ff9270" : undefined))],
    ["preview.png", pictures.preview],
    ["screenshots/chat.png", pictures.chat],
    ["screenshots/new-chat.png", pictures.home]
  ]);
  if (t.home) out.set("screens/chat/home.luau", Buffer.from(t.versions[i].rendered ? homeOverZeron(ui, t.home) : home(t.home)));
  const { width, leading, plain } = t.versions[i];
  if (t.change === "roomy" && !plain) out.set("ds/tokens.luau", Buffer.from(roomy(ui, gapOf(t, i), leading)));
  if (width) out.set("screens/settings/prefs.luau", Buffer.from(narrower(ui, width)));
  for (const path of t.keep ?? []) out.set(path, readFileSync(join(ui, path)));
  return out;
};
