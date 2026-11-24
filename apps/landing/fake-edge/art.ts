// The fixtures' pictures, drawn here so the fake carries no binary files:
// icons as the app wants them (one colour, currentColor, a square
// viewBox), and screenshots as a window of the app in its dark theme with
// the plugin's part in it.

const svg = (body: string, size = 24) =>
  `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${size} ${size}" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">${body}</svg>`;

export const ICONS: Record<string, string> = {
  timer: svg(`<circle cx="12" cy="13" r="8"/><path d="M12 9v4l2.5 2.5M9.5 2.5h5"/>`),
  radar: svg(`<circle cx="12" cy="12" r="9"/><circle cx="12" cy="12" r="5"/><path d="M12 12l6-6"/><circle cx="12" cy="12" r="1" fill="currentColor"/>`),
  issues: svg(`<circle cx="12" cy="12" r="9"/><path d="M8.5 12.5l2.5 2.5 4.5-5"/>`),
  meter: svg(`<path d="M4 17a8 8 0 1 1 16 0"/><path d="M12 17l4-5"/><path d="M4 21h16"/>`),
  badge: svg(`<rect x="3.5" y="5" width="17" height="14" rx="3.5"/><path d="M7.5 10h5M7.5 14h9"/>`),
  database: svg(`<ellipse cx="12" cy="6" rx="7" ry="3"/><path d="M5 6v12c0 1.7 3.1 3 7 3s7-1.3 7-3V6M5 12c0 1.7 3.1 3 7 3s7-1.3 7-3"/>`),
  shell: svg(`<rect x="3" y="4" width="18" height="16" rx="3.5"/><path d="M7 9l3 3-3 3M12.5 15H17"/>`),
  palette: svg(`<path d="M12 3a9 9 0 1 0 0 18c1.2 0 1.8-.9 1.8-1.8 0-.6-.3-1-.6-1.4-.3-.4-.5-.8-.5-1.3 0-1 .8-1.8 1.8-1.8H17a4 4 0 0 0 4-4C21 6.6 17 3 12 3z"/><circle cx="7.5" cy="11" r="1" fill="currentColor"/><circle cx="10" cy="7" r="1" fill="currentColor"/><circle cx="15" cy="7.5" r="1" fill="currentColor"/>`),
  bell: svg(`<path d="M6 16V11a6 6 0 1 1 12 0v5l1.5 2h-15z"/><path d="M10 20.5a2 2 0 0 0 4 0"/>`),
  graph: svg(`<path d="M4 19V5M4 19h16"/><path d="M7 15l4-4 3 3 5-6"/>`),
  calendar: svg(`<rect x="3.5" y="5" width="17" height="15" rx="3"/><path d="M3.5 10h17M8 3v4M16 3v4"/>`),
  translate: svg(`<path d="M4 6h9M8.5 4v2c0 4-2 7-5 8.5M6 10c1.5 2 3.5 3.5 6 4.5"/><path d="M13 20l4-9 4 9M14.3 17h5.4"/>`),
  bolt: svg(`<path d="M13 3L5 13.5h6L10 21l8-10.5h-6z"/>`),
  leaf: svg(`<path d="M5 19c0-8 5-14 15-15-1 10-7 15-15 15z"/><path d="M5 19l7-7"/>`),
  mic: svg(`<rect x="9" y="3" width="6" height="11" rx="3"/><path d="M5.5 11a6.5 6.5 0 0 0 13 0M12 17.5V21"/>`),
  map: svg(`<path d="M9 4L3.5 6v14L9 18l6 2 5.5-2V4L15 6z"/><path d="M9 4v14M15 6v14"/>`),
  key: svg(`<circle cx="8" cy="15" r="4.5"/><path d="M11.2 11.8L20 3M16.5 6.5l2.5 2.5M14 9l2 2"/>`),
  cloud: svg(`<path d="M7 18a4.5 4.5 0 0 1-.5-9A6 6 0 0 1 18 8.5a4.5 4.5 0 0 1-.5 9.5z"/>`),
  book: svg(`<path d="M4 5.5A2.5 2.5 0 0 1 6.5 3H20v15H6.5A2.5 2.5 0 0 0 4 20.5z"/><path d="M4 20.5A2.5 2.5 0 0 0 6.5 23H20"/>`),
  flame: svg(`<path d="M12 21c-4 0-6.5-2.7-6.5-6.2C5.5 10 10 8 10 3c3.5 2 7.5 6 7.5 11.3 0 3.8-2.5 6.7-5.5 6.7z"/><path d="M12 21c-1.6 0-2.8-1.2-2.8-2.9 0-2 2.8-3.6 2.8-5.6 1.4 1.4 2.8 3 2.8 5.3 0 1.9-1.2 3.2-2.8 3.2z"/>`),
};

/** A window of the app with a sidebar, a chat and `part` drawn in its accent. */
export function screenshot(title: string, accent: string, part: "panel" | "header" | "sidebar" | "composer" | "page"): string {
  const rows = Array.from({ length: 9 }, (_, i) => {
    const y = 132 + i * 44;
    const w = [118, 152, 96, 136, 110, 160, 124, 90, 142][i]!;
    const on = i === 2;
    return `<rect x="28" y="${y - 14}" width="236" height="34" rx="8" fill="${on ? "#ffffff17" : "none"}"/><circle cx="48" cy="${y + 3}" r="5" fill="${on ? accent : "#ffffff30"}"/><rect x="62" y="${y - 2}" width="${w}" height="10" rx="5" fill="#ffffff${on ? "c0" : "52"}"/>`;
  }).join("");
  const lines = (x: number, y: number, widths: number[], ink = "#ffffff38") =>
    widths.map((w, i) => `<rect x="${x}" y="${y + i * 26}" width="${w}" height="11" rx="5.5" fill="${ink}"/>`).join("");
  const parts: Record<typeof part, string> = {
    panel: `<rect x="884" y="64" width="368" height="708" rx="14" fill="#141417" stroke="#ffffff14"/><rect x="908" y="92" width="120" height="14" rx="7" fill="${accent}"/>${lines(908, 132, [300, 260, 312, 220, 286])}<rect x="908" y="292" width="320" height="180" rx="10" fill="${accent}22" stroke="${accent}66"/><path d="M928 440l52-60 40 30 54-80 60 70 54-40" stroke="${accent}" stroke-width="4" fill="none" stroke-linecap="round" stroke-linejoin="round"/>${lines(908, 500, [280, 300, 190])}`,
    header: `<rect x="316" y="76" width="540" height="44" rx="10" fill="#141417" stroke="#ffffff14"/><rect x="332" y="92" width="150" height="12" rx="6" fill="#ffffffa0"/><rect x="704" y="86" width="136" height="24" rx="12" fill="${accent}33" stroke="${accent}"/><circle cx="722" cy="98" r="5" fill="${accent}"/><rect x="734" y="93" width="90" height="10" rx="5" fill="${accent}"/>`,
    sidebar: `<rect x="196" y="${132 + 2 * 44 - 10}" width="56" height="26" rx="13" fill="${accent}33" stroke="${accent}"/><rect x="196" y="${132 + 5 * 44 - 10}" width="56" height="26" rx="13" fill="${accent}33" stroke="${accent}"/>`,
    composer: `<rect x="316" y="660" width="540" height="104" rx="18" fill="#18181b" stroke="#c7d2de17"/>${lines(340, 684, [380, 240], "#ffffff60")}<rect x="340" y="728" width="132" height="22" rx="11" fill="${accent}33" stroke="${accent}"/><rect x="352" y="734" width="108" height="10" rx="5" fill="${accent}"/>`,
    page: `<rect x="316" y="64" width="936" height="708" rx="14" fill="#141417" stroke="#ffffff14"/><rect x="348" y="96" width="220" height="18" rx="9" fill="#ffffffd0"/>${lines(348, 136, [560, 480])}${[0, 1, 2].map((i) => `<rect x="${348 + i * 300}" y="208" width="276" height="160" rx="12" fill="${i === 0 ? `${accent}26` : "#ffffff08"}" stroke="${i === 0 ? accent : "#ffffff14"}"/>${lines(372 + i * 300, 236, [140, 200, 120], i === 0 ? accent : "#ffffff38")}`).join("")}${lines(348, 400, [820, 760, 800, 540])}`,
  };
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1280 800" width="1280" height="800"><defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#1b1726"/><stop offset="1" stop-color="#0b0b0d"/></linearGradient></defs><rect width="1280" height="800" fill="url(#g)"/><rect x="12" y="12" width="1256" height="776" rx="16" fill="#0e0e10" stroke="#ffffff1a"/><circle cx="36" cy="36" r="6" fill="#ff5f57"/><circle cx="56" cy="36" r="6" fill="#febc2e"/><circle cx="76" cy="36" r="6" fill="#28c840"/><text x="640" y="41" fill="#ffffff70" font-family="Geist, system-ui, sans-serif" font-size="13" text-anchor="middle">${title}</text><rect x="20" y="64" width="260" height="708" rx="14" fill="#ffffff08"/>${rows}${lines(316, 160, [460, 520, 380], "#ffffff40")}<rect x="520" y="264" width="336" height="64" rx="16" fill="#ffffff14"/>${lines(540, 280, [280, 200], "#ffffff70")}${lines(316, 372, [500, 420, 520, 300], "#ffffff38")}${parts[part]}</svg>`;
}
