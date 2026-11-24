// The fake store's templates: whole interfaces, each with its preview.png,
// its README and its versions, in the states their pages show: verified,
// not verified yet, one whose newest isn't and an older one is, one at
// every limit, one no Zeron released can run.
import { DAY, NOW, type FakePlugin, type FakeVersion, type Story } from "./fixtures.ts";
import { PNG_OF, type Box, type Picture } from "./png.ts";

interface Look {
  bg: string;
  panel: string;
  raised: string;
  ink: string;
  soft: string;
  accent: string;
}
type Layout = "columns" | "reading" | "grid" | "dock" | "single" | "board";

const LOOKS: Record<string, Look> = {
  slate: { bg: "#101317", panel: "#171c22", raised: "#20272f", ink: "#dfe7ee", soft: "#5d6a77", accent: "#62b5e5" },
  paper: { bg: "#f4efe4", panel: "#fbf8f1", raised: "#ece4d3", ink: "#3b342a", soft: "#a89c86", accent: "#b4532a" },
  phosphor: { bg: "#050a06", panel: "#0a120c", raised: "#112016", ink: "#9ff0b4", soft: "#3d6b49", accent: "#4dff88" },
  plum: { bg: "#17101f", panel: "#20172b", raised: "#2c203a", ink: "#efe6f8", soft: "#7a6890", accent: "#c58cff" },
  chalk: { bg: "#ffffff", panel: "#f7f7f8", raised: "#ededf0", ink: "#1d1d22", soft: "#b0b0b8", accent: "#2f6bff" },
  ember: { bg: "#15100d", panel: "#1e1713", raised: "#2a201a", ink: "#f4e9e0", soft: "#80705f", accent: "#ff9a4d" },
};

/** A window of Zeron wearing this template, in boxes. */
export function preview(look: Look, layout: Layout): Picture {
  const { bg, panel, raised, ink, soft, accent } = look;
  const b: Box[] = [[0, 0, 1280, 800, 0, bg]];
  const lines = (x: number, y: number, widths: number[], colour = soft, step = 26, h = 10) => widths.forEach((w, i) => b.push([x, y + i * step, w, h, h / 2, colour]));
  for (const [i, c] of ["#ff5f57", "#febc2e", "#28c840"].entries()) b.push([24 + i * 20, 22, 12, 12, 6, c]);
  if (layout === "columns") {
    b.push([16, 56, 228, 728, 12, panel], [260, 56, 620, 728, 12, panel], [896, 56, 368, 728, 12, panel]);
    for (let i = 0; i < 11; i++) b.push([32, 80 + i * 44, 12, 12, 6, i === 1 ? accent : soft], [56, 81 + i * 44, 90 + ((i * 37) % 80), 10, 5, i === 1 ? ink : soft]);
    lines(292, 92, [220], ink, 26, 14);
    lines(292, 136, [520, 480, 540, 300]);
    b.push([500, 264, 348, 84, 14, raised]);
    lines(524, 288, [280, 200], ink);
    lines(292, 384, [500, 540, 420, 520, 260]);
    b.push([292, 692, 556, 68, 14, raised]);
    lines(316, 720, [240]);
    lines(920, 88, [140], ink, 26, 12);
    for (let i = 0; i < 4; i++) b.push([920, 124 + i * 156, 320, 140, 10, raised], [940, 144 + i * 156, 120, 10, 5, i === 0 ? accent : soft], [940, 172 + i * 156, 260, 8, 4, soft], [940, 194 + i * 156, 200, 8, 4, soft], [940, 228 + i * 156, 240 - i * 40, 12, 6, accent]);
  } else if (layout === "reading") {
    lines(300, 100, [420], ink, 26, 26);
    lines(300, 150, [200]);
    lines(300, 206, [680, 660, 690, 640, 400], ink, 30, 11);
    b.push([300, 376, 680, 120, 8, panel], [300, 376, 4, 120, 0, accent]);
    lines(328, 402, [560, 600, 320], soft, 28);
    lines(300, 528, [690, 650, 680, 520], ink, 30, 11);
    b.push([300, 688, 680, 64, 32, raised], [324, 714, 260, 12, 6, soft], [924, 704, 32, 32, 16, accent]);
    lines(60, 110, [120, 100, 130, 90, 110], soft, 40);
  } else if (layout === "grid") {
    for (let i = 0; i < 4; i++) {
      const [x, y] = [16 + (i % 2) * 628, 56 + Math.floor(i / 2) * 368];
      b.push([x, y, 620, 360, 6, panel], [x, y, 620, 30, 6, raised], [x + 16, y + 10, 160, 10, 2, i === 0 ? accent : ink]);
      lines(x + 16, y + 52, [420, 360, 480, 300, 520, 240, 400, 330], i === 0 ? ink : soft, 34, 9);
      b.push([x + 16, y + 328, 12, 16, 0, accent]);
    }
  } else if (layout === "dock") {
    b.push([160, 60, 960, 600, 20, panel]);
    lines(208, 108, [320], ink, 26, 18);
    lines(208, 160, [760, 720, 780, 420]);
    b.push([620, 296, 452, 96, 18, accent]);
    lines(648, 326, [360, 240], bg);
    lines(208, 432, [740, 780, 600]);
    b.push([208, 560, 864, 64, 18, raised]);
    lines(236, 586, [320]);
    b.push([420, 692, 440, 84, 28, raised]);
    for (let i = 0; i < 6; i++) b.push([444 + i * 68, 708, 52, 52, 14, i === 2 ? accent : soft]);
  } else if (layout === "single") {
    b.push([0, 48, 1280, 1, 0, raised]);
    lines(390, 110, [500], ink, 26, 22);
    for (let i = 0; i < 3; i++) {
      lines(390, 176 + i * 168, [80], accent, 26, 10);
      lines(390, 206 + i * 168, [500, 470, 490, 260], soft, 28, 10);
    }
    b.push([390, 700, 500, 56, 28, panel], [414, 722, 200, 12, 6, soft]);
  } else {
    for (let c = 0; c < 4; c++) {
      b.push([16 + c * 314, 56, 302, 728, 14, panel], [36 + c * 314, 80, 90 + c * 14, 12, 6, c === 1 ? accent : ink]);
      for (let i = 0; i < [4, 3, 5, 2][c]!; i++) b.push([32 + c * 314, 116 + i * 128, 270, 114, 10, raised], [48 + c * 314, 134 + i * 128, 180, 11, 5, ink], [48 + c * 314, 160 + i * 128, 220, 8, 4, soft], [48 + c * 314, 180 + i * 128, 140, 8, 4, soft], [48 + c * 314, 204 + i * 128, 60, 14, 7, accent]);
    }
  }
  return { w: 1280, h: 800, boxes: b };
}

interface Spec {
  id: string;
  name: string;
  summary: string;
  owner: string;
  look: keyof typeof LOOKS;
  layout: Layout;
  keywords?: string[];
  installs: number;
  featured?: boolean;
  age: number;
  screenshots?: number;
  homepage?: string;
  repository?: string;
  license?: string;
  zeron?: string;
  history?: { version: string; status: Story; ago: number; notes?: string; via?: FakeVersion["via"]; review?: FakeVersion["review"]; look?: keyof typeof LOOKS }[];
  readme?: string;
}

const long = "Every limit at once: a name of sixty-four characters with no spaces to break at, this description at three hundred characters, eight keywords of thirty-two characters and eight screenshots, so each card, row and page is seen at its worst before anyone else sees it. Every limit at once: a na";

const SPECS: Spec[] = [
  {
    id: "control-room", name: "Control room", summary: "Three columns: your chats, the one you're in, and everything running beside it.", owner: "user_ada", look: "slate", layout: "columns",
    keywords: ["dense", "dark", "columns"], installs: 4210, featured: true, age: 40, screenshots: 3, homepage: "https://example.com/control-room", repository: "https://github.com/ada/control-room", license: "MIT",
    history: [
      { version: "1.0.0", status: "verified", ago: 40, notes: "First release." },
      { version: "1.1.0", status: "verified", ago: 18, notes: "- The right column can be hidden.\n- Long chat names are cut, whole on hover." },
      { version: "1.2.0", status: "verified", ago: 4, via: "deploy_token", notes: "### New\n\n- A running agent's cost, under its name.\n\n### Fixed\n\n- The middle column kept its scroll when you changed chats." },
    ],
    readme: `# Control room\n\nEverything on screen at once: your chats on the left, the one you're in, and what's running beside it.\n\n## What it changes\n\n- The sidebar is always open, in a smaller size.\n- A third column follows the chat: its changes, its terminal, its cost.\n- The title bar goes away.\n\n## Made for\n\nWide screens. Under 1,100 px the third column folds away.\n\n| Key | What it does |\n| --- | --- |\n| \`⌘1\` | The chat list |\n| \`⌘2\` | The chat |\n| \`⌘3\` | What's running |\n\nThe rest is Zeron's own, and stays so as Zeron changes.`,
  },
  { id: "paper-desk", name: "Paper desk", summary: "One quiet page, set like a book. For long reading and long writing.", owner: "user_kai", look: "paper", layout: "reading", keywords: ["light", "reading", "calm"], installs: 2890, featured: true, age: 31, screenshots: 2, license: "MIT" },
  { id: "phosphor", name: "Phosphor", summary: "Four terminals in green on black, and nothing else.", owner: "user_mo", look: "phosphor", layout: "grid", keywords: ["terminal", "dark", "dense"], installs: 1740, featured: true, age: 22, screenshots: 1, license: "Apache-2.0" },
  { id: "dockside", name: "Dockside", summary: "Your chats along the bottom of the window, as a dock.", owner: "user_ada", look: "plum", layout: "dock", keywords: ["dock", "dark"], installs: 960, age: 26, screenshots: 2, history: [{ version: "0.9.0", status: "verified", ago: 26, notes: "First release." }, { version: "1.0.0", status: "undecided", ago: 2, notes: "The dock hides while you type." }] },
  { id: "one-thing", name: "One thing", summary: "A single chat, centred, with no sidebar until you ask for it.", owner: "user_mo", look: "chalk", layout: "single", keywords: ["light", "calm", "minimal"], installs: 1320, age: 60, screenshots: 1 },
  { id: "night-board", name: "Night board", summary: "Each project a column, each chat a card you can drag.", owner: "user_kai", look: "ember", layout: "board", keywords: ["board", "dark", "columns"], installs: 410, age: 12, screenshots: 2 },
  { id: "early-hours", name: "Early hours", summary: "Warm and dim, for the first hour of the day.", owner: "user_ada", look: "ember", layout: "reading", keywords: ["warm", "calm"], installs: 0, age: 1, history: [{ version: "0.1.0", status: "undecided", ago: 1, notes: "First release." }] },
  { id: "white-board", name: "White board", summary: "The board, for bright rooms.", owner: "user_mo", look: "chalk", layout: "board", keywords: ["board", "light"], installs: 230, age: 15 },
  { id: "wide-columns", name: "Wide columns", summary: "Control room's columns in paper's colours.", owner: "user_kai", look: "paper", layout: "columns", keywords: ["light", "columns"], installs: 180, age: 8, zeron: "0.9.0" },
  { id: "plum-terminals", name: "Plum terminals", summary: "Four terminals, gentler on the eyes.", owner: "user_mo", look: "plum", layout: "grid", keywords: ["terminal", "dark"], installs: 120, age: 9 },
  {
    id: "template-at-every-limit-there-is-xyz1234", name: "Averyveryverylongtemplatenamewithoutanyspacesatallthatgoeson64xx", summary: long.slice(0, 300), owner: "user_ada", look: "slate", layout: "board",
    keywords: Array.from({ length: 8 }, (_, i) => `keywordthatisthirtytwocharacters`.slice(0, 31) + i), installs: 7, age: 5, screenshots: 8, homepage: "https://example.com/", repository: "https://github.com/ada/limits", license: "LicenseRef-A-Very-Long-License-Identifier-That-Goes-On-And-On",
    history: [{ version: "1.0.0", status: "verified", ago: 5, notes: "First release." }, { version: "1.1.0-beta.20261124.build.99999", status: "undecided", ago: 1, notes: "A long version's name." }],
  },
];

/** Zeron's own files a template replaces, as its package carries them. */
const appFiles = (spec: Spec, version: string): Record<string, string> => ({
  "init.luau": `-- ${spec.name} ${version}\nlocal shell = require("shell")\n\nreturn shell.compose {\n\tlayout = "${spec.layout}",\n}\n`,
  "shell/layout.luau": `-- The ${spec.layout} layout.\nlocal ui = require("ui")\n\nreturn function(app)\n\treturn ui.row { app.sidebar, app.chat }\nend\n`,
  "shell/sidebar.luau": "-- Zeron's own sidebar.\nreturn require(\"ui/sidebar\")\n",
  "theme.css": `:root { --bg: ${LOOKS[spec.look]!.bg}; --accent: ${LOOKS[spec.look]!.accent}; }\n`,
});

function build(spec: Spec): FakePlugin {
  const history = spec.history ?? [{ version: "1.0.0", status: "verified" as Story, ago: spec.age, notes: "First release." }];
  const shots = Array.from({ length: spec.screenshots ?? 0 }, (_, i) => `screenshots/${i + 1}.png`);
  const layouts: Layout[] = ["columns", "reading", "board", "dock", "single", "grid"];
  const zeron = spec.zeron ?? "0.3.4";
  const versions: FakeVersion[] = history.map((h) => {
    const files: Record<string, string> = {
      ...appFiles(spec, h.version),
      "README.md": spec.readme ?? `# ${spec.name}\n\n${spec.summary}\n\n## What it changes\n\n- How the window is laid out.\n- Its colours, light and dark.\n\nAnything it leaves out is Zeron's own.`,
      "CHANGELOG.md": history.map((x) => `## ${x.version}\n\n${x.notes ?? ""}\n`).reverse().join("\n"),
      "preview.png": PNG_OF + JSON.stringify(preview(LOOKS[spec.look]!, spec.layout)),
    };
    for (const [i, s] of shots.entries()) files[s] = PNG_OF + JSON.stringify(preview(LOOKS[spec.look]!, layouts[(layouts.indexOf(spec.layout) + i + 1) % layouts.length]!));
    const published = NOW - h.ago * DAY;
    const verifiedAt = h.status === "verified" || h.status === "yanked" ? published + DAY / 2 : null;
    return {
      version: h.version,
      status: h.status === "yanked" || h.status === "taken_down" || h.status === "removed" ? h.status : "listed",
      verified: verifiedAt !== null,
      verified_at: verifiedAt,
      published_at: published,
      notes: h.notes ?? null,
      engines: { zeron: `>=${zeron}` },
      review: h.review ?? (verifiedAt !== null ? { decided: true, decision: "verified", decided_at: verifiedAt } : { decided: false, decision: null }),
      via: h.via ?? "device",
      deploy_token: null,
      yanked_by: h.status === "yanked" ? "author" : null,
      superseded: null,
      removal: null,
      checks: [
        { id: "signature", level: "ok", message: "Signed by WMWMWMWMWMWMWMWMWMWMWMWMWMWMWMWMWMWMWMWMWMW, uploaded from a signed-in device." },
        { id: "manifest", level: "ok", message: "The manifest follows every rule a device checks." },
        { id: "preview", level: "ok", message: "preview.png is there, a still PNG." },
      ],
      manifest: {
        id: spec.id, name: spec.name, version: h.version, description: spec.summary, zeron_version: zeron, keywords: spec.keywords ?? [], screenshots: shots,
        ...(spec.homepage ? { homepage: spec.homepage } : {}), ...(spec.repository ? { repository: spec.repository } : {}), ...(spec.license ? { license: spec.license } : {}),
      },
      files,
    };
  });
  return { kind: "template", id: spec.id, owner: spec.owner, status: "listed", featured: spec.featured ?? false, installs: spec.installs, created_at: NOW - spec.age * DAY, versions: versions.reverse() };
}

/** Twenty more, only while a test asks for them (POST /_fake/templates?on=1): a list longer than a page. */
export const moreTemplates = (): FakePlugin[] => {
  const looks = Object.keys(LOOKS), layouts: Layout[] = ["columns", "reading", "grid", "dock", "single", "board"];
  return Array.from({ length: 20 }, (_, i) => build({ id: `house-${i + 1}`, name: `House ${i + 1}`, summary: "One of twenty more, for a list longer than a page.", owner: ["user_kai", "user_mo"][i % 2]!, look: looks[i % looks.length]!, layout: layouts[(i * 5 + 1) % layouts.length]!, keywords: ["house"], installs: 90 - i, age: 20 + i }));
};

export const templates = (): FakePlugin[] => SPECS.map(build);
