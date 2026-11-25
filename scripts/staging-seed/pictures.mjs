// The templates' pictures: each template's look drawn as a window, 16:10
// (1280×800 PNG), by a headless browser: its preview (the store's cover), a
// chat, and a new chat. Invented words only.

const esc = (s) => s.replaceAll("&", "&amp;").replaceAll("<", "&lt;");

/** How each layout differs: its type, its sidebar, its cards. */
const LAYOUTS = {
  cards: { font: "system-ui, sans-serif", side: 250, gap: 14, shadow: "0 18px 50px #0007", bubbles: true },
  desk: { font: "Georgia, 'Liberation Serif', serif", side: 220, gap: 0, shadow: "none", rule: true },
  terminal: { font: "'DejaVu Sans Mono', 'Liberation Mono', monospace", side: 240, gap: 0, shadow: "none", scan: true, upper: true },
  margin: { font: "system-ui, sans-serif", side: 190, gap: 0, shadow: "none", narrow: 560 },
  reef: { font: "system-ui, sans-serif", side: 270, gap: 18, shadow: "0 10px 40px #0005", bubbles: true, right: true, wave: true },
  grid: { font: "'DejaVu Sans', sans-serif", side: 230, gap: 6, shadow: "none", grid: true, panel: true },
  pastel: { font: "system-ui, sans-serif", side: 240, gap: 16, shadow: "0 12px 36px #8a5cd62b", bubbles: true, blobs: true },
  contrast: { font: "Arial, 'Liberation Sans', sans-serif", side: 260, gap: 0, shadow: "none", strong: true }
};

const SESSIONS = ["Tidy the release notes", "Why is the build slow?", "Plan the garden app", "Rename the modules", "A parser for dates", "Weekly summary", "Fix the flaky test"];
const CHAT = [
  ["you", "Can you find why the list jumps when a row is added?"],
  ["it", "The row's key is made from its place in the list, so every row after a new one is rebuilt. I'll key it by its id."],
  ["code", "for _, item in items do\n  rows[item.id] = Row { item = item }\nend"],
  ["it", "Done. Adding a row now leaves the others where they are."]
];

const page = (t, kind) => {
  const l = t.look;
  const y = LAYOUTS[l.layout];
  const line = y.strong ? `2px solid ${l.text}` : `1px solid ${l.text}22`;
  const words = (s) => esc(y.upper ? s.toUpperCase() : s);
  const side = `<aside>${SESSIONS.map((s, i) => `<div class="row${i === 1 ? " on" : ""}">${words(s)}</div>`).join("")}</aside>`;
  const chat = CHAT.map(([who, s]) => (who === "code" ? `<pre>${esc(s)}</pre>` : `<div class="msg ${who}">${words(s)}</div>`)).join("");
  const [title, sub] = t.home ?? ["What are we building?", "Ask anything. Drop files to attach them."];
  const home = `<div class="home"><h1>${words(title)}</h1><p>${words(sub)}</p></div>`;
  const cover = `<div class="cover"><h1>${words(t.name)}</h1><p>${words(t.keywords.join(" · "))}</p></div>`;
  const body = kind === "home" ? home : kind === "preview" ? cover + chat : chat;
  const panel = y.panel ? `<section class="panel">${["main.luau", "row.luau", "store.luau", "theme.luau"].map((f) => `<div class="row">${f}</div>`).join("")}</section>` : "";
  return `<!doctype html><meta charset="utf-8"><style>
*{box-sizing:border-box}html,body{margin:0;width:1280px;height:800px;overflow:hidden}
body{background:${l.background};color:${l.text};font:17px/1.55 ${y.font};${y.grid ? `background-image:linear-gradient(${l.text}0d 1px,transparent 1px),linear-gradient(90deg,${l.text}0d 1px,transparent 1px);background-size:32px 32px;` : ""}${y.blobs ? `background-image:radial-gradient(circle at 12% 8%,#ffd3c4 0,transparent 34%),radial-gradient(circle at 92% 20%,#e1d2ff 0,transparent 36%),radial-gradient(circle at 70% 100%,#cdf3e1 0,transparent 38%);` : ""}${y.wave ? `background-image:radial-gradient(120% 60% at 50% 110%,#0f6a6a 0,transparent 70%),radial-gradient(60% 40% at 85% 0,#ff7f6a33 0,transparent 70%);` : ""}}
.bar{height:44px;display:flex;align-items:center;gap:9px;padding:0 18px;color:${l.muted};border-bottom:${y.gap ? "0" : line};font-size:14px}
.dot{width:12px;height:12px;border-radius:${l.radius ? 6 : 0}px;background:${l.faint}}.dot:first-child{background:${l.accent}}
.win{display:flex;flex-direction:${y.right ? "row-reverse" : "row"};gap:${y.gap}px;padding:${y.gap}px;height:756px}
aside{width:${y.side}px;padding:${y.gap ? 4 : 18}px ${y.gap ? 4 : 14}px;${y.gap ? "" : `border-right:${line};`}}
.row{padding:9px 12px;margin-bottom:4px;border-radius:${Math.min(l.radius, 12)}px;color:${l.muted};white-space:nowrap;overflow:hidden;text-overflow:ellipsis;font-size:15px}
.row.on{background:${y.strong ? l.accent : `${l.accent}2e`};color:${y.strong ? "#fff" : l.text}}
main{flex:1;background:${l.raised};border-radius:${y.gap ? l.radius : 0}px;box-shadow:${y.shadow};padding:34px 44px;display:flex;flex-direction:column;gap:16px;position:relative;overflow:hidden;${y.grid ? `border:${line};` : ""}}
main>*{max-width:${y.narrow ?? 760}px;${y.narrow ? "margin-left:auto;margin-right:auto;width:100%;" : ""}}
.msg{padding:${y.bubbles ? "13px 18px" : "0"};border-radius:${l.radius}px}.msg.you{${y.bubbles ? `background:${l.accent};color:${l.background};align-self:flex-end;` : `color:${l.accent};`}${y.rule ? `border-left:3px solid ${l.accent};padding-left:16px;` : ""}}.msg.it{${y.bubbles ? `background:${l.text}14;` : ""}}
pre{margin-top:0;margin-bottom:0;padding:16px 20px;border-radius:${Math.min(l.radius, 14)}px;background:${l.background};border:${line};color:${l.success};font:15px/1.5 'DejaVu Sans Mono','Liberation Mono',monospace}
.composer{position:absolute;left:44px;right:44px;bottom:28px;max-width:none;border:${y.strong ? `3px solid ${l.accent}` : line};border-radius:${l.radius}px;padding:17px 22px;color:${l.faint};background:${l.background};display:flex;justify-content:space-between;width:auto}
.send{color:${l.accent};font-weight:700}
.cover h1{margin:0;font-size:46px;line-height:1.1;letter-spacing:-.01em;color:${l.text}}.cover p{margin:8px 0 6px;color:${l.accent};font-size:18px}
.home{margin:auto;text-align:center;padding-bottom:90px}.home h1{font-size:40px;margin:0 0 8px}.home p{color:${l.muted};margin:0}
.panel{width:200px;padding:14px;border:${line};border-radius:${l.radius}px;background:${l.raised}}
.scan{position:fixed;inset:0;background:repeating-linear-gradient(0deg,#0000 0 2px,#0006 2px 4px);pointer-events:none}
</style><div class="bar"><span class="dot"></span><span class="dot"></span><span class="dot"></span><span style="margin-left:10px">${words(kind === "home" ? "New chat" : SESSIONS[1])}</span></div>
<div class="win">${side}<main>${body}<div class="composer"><span>${words("Ask anything")}</span><span class="send">${y.upper ? "RUN" : "Send"}</span></div></main>${panel}</div>${y.scan ? '<div class="scan"></div>' : ""}`;
};

/** `t`'s three pictures, with a Playwright `browser`. */
export const pictures = async (browser, t) => {
  const context = await browser.newContext({ viewport: { width: 1280, height: 800 }, deviceScaleFactor: 1 });
  try {
    const tab = await context.newPage();
    const out = {};
    for (const kind of ["preview", "chat", "home"]) {
      await tab.setContent(page(t, kind));
      out[kind] = await tab.screenshot({ type: "png" });
    }
    return out;
  } finally {
    await context.close();
  }
};
