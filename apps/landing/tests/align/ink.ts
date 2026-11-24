// Text lines up with text, not with boxes (Wing's rule): in each column of
// a page, every run of ink that begins a line near the column puts its ink
// on it. Run in the page (page.evaluate). A column is a container's content
// edge (its border box's left, plus its border and padding). A run is
// what's drawn: a text node's first glyph on each of its lines (its ink, by
// the glyph's own side bearing, so large type counts by what shows), an
// icon's drawn geometry, a field's text start, a tile or picture's edge. A
// run begins a line when nothing else in its column is drawn left of it on
// that line; it belongs to the innermost column around it, and is held to
// that column when it lands within `window` of it.

export interface InkMiss {
  /** The column's selector, and its x. */
  column: string;
  x: number;
  /** Where the run's ink starts, and how far from the column. */
  ink: number;
  delta: number;
  /** What it is: its text, or the kind of thing drawn. */
  what: string;
  /** A path to it, short enough to read (tag.class › tag.class …). */
  selector: string;
}

export interface InkReport {
  columns: { selector: string; x: number }[];
  /** How many runs began a line near a column, and were checked. */
  checked: number;
  misses: InkMiss[];
  /** Misses in a place the rule says can't be on the column, with why. */
  excepted: (InkMiss & { why: string })[];
}

export interface InkOptions {
  /** Each selector's elements are columns. */
  columns: string[];
  /** How far from a column a line's first ink is held to it (px). */
  window: number;
  /** How far from it counts as on it (px). */
  tolerance: number;
  /** Runs in these, off the column, are the rule's exceptions: [selector, why]. */
  exceptions: [string, string][];
}

/** Runs in the browser: keep it free of anything imported. */
export function inkAudit(options: InkOptions): InkReport {
  const style = (el: Element) => getComputedStyle(el);
  const shown = (el: Element) => (el as HTMLElement).checkVisibility?.({ checkOpacity: true, checkVisibilityCSS: true }) ?? true;
  const hidden = (el: Element) => !shown(el) || !!el.closest(".sr-only, script, style, noscript, template, [hidden], dialog:not([open]), [popover]:not(:popover-open), details:not([open]) > :not(summary)");
  const path = (el: Element) => {
    const parts: string[] = [];
    for (let at: Element | null = el; at && at !== document.body && parts.length < 4; at = at.parentElement) {
      const cls = [...at.classList].slice(0, 2).join(".");
      parts.unshift(at.tagName.toLowerCase() + (cls ? `.${cls}` : ""));
    }
    return parts.join(" › ");
  };

  const columns: { el: Element; selector: string; x: number }[] = [];
  for (const selector of options.columns)
    for (const el of document.querySelectorAll(selector)) {
      if (hidden(el)) continue;
      const box = el.getBoundingClientRect();
      const s = style(el);
      columns.push({ el, selector, x: box.left + parseFloat(s.borderLeftWidth) + parseFloat(s.paddingLeft) });
    }
  const columnOf = (el: Element) => {
    let best: (typeof columns)[number] | null = null;
    for (const c of columns) if (c.el.contains(el) && (!best || best.el.contains(c.el))) best = c;
    return best;
  };

  // The ink of a glyph starts this far right of where its box starts (negative: it reaches left).
  const pen = document.createElement("canvas").getContext("2d")!;
  const bearing = (el: Element, ch: string) => {
    // (Chrome gives no `font` shorthand back: the font, from its parts.)
    const s = style(el);
    pen.font = `${s.fontStyle} ${s.fontWeight} ${s.fontSize} ${s.fontFamily}`;
    return -pen.measureText(ch).actualBoundingBoxLeft;
  };
  // A digit or letter centred in a small mark (a numbered circle, a file's "M"): the mark is what's drawn there.
  const markOf = (el: Element): Element | null => {
    const box = el.getBoundingClientRect();
    const s = style(el);
    const centred = s.textAlign === "center" || ((s.display.includes("grid") || s.display.includes("flex")) && (s.placeItems.startsWith("center") || s.justifyContent === "center"));
    return box.width <= 40 && box.height <= 40 && centred ? el : null;
  };

  interface Run { el: Element; ink: number; top: number; bottom: number; what: string }
  const runs: Run[] = [];

  // Text: each line's first drawn glyph.
  const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  const range = document.createRange();
  for (let node = walker.nextNode() as Text | null; node; node = walker.nextNode() as Text | null) {
    if (!node.data.trim()) continue;
    const el = node.parentElement;
    if (!el || hidden(el)) continue;
    range.selectNodeContents(node);
    const lines = [...range.getClientRects()].filter((r) => r.width > 0.5 && r.height > 0.5);
    let from = 0;
    for (const line of lines) {
      for (let i = from; i < node.length; i++) {
        if (/\s/.test(node.data[i]!)) continue;
        range.setStart(node, i);
        range.setEnd(node, i + 1);
        const glyph = range.getClientRects()[0];
        if (!glyph || Math.abs(glyph.top - line.top) > 2) continue;
        const mark = markOf(el);
        const at = mark ? mark.getBoundingClientRect() : null;
        runs.push({ el, ink: at ? at.left : glyph.left + bearing(el, node.data[i]!), top: at ? at.top : glyph.top, bottom: at ? at.bottom : glyph.bottom, what: (mark ? "(mark) " : "") + node.data.slice(i, i + 40).trim() });
        from = i + 1;
        break;
      }
    }
  }
  // Icons: by their square. (Each glyph's ink sits its own 1 to 3 px inside it, the icon set's optical inset;
  // the squares on the column keep a column of icons in one place, which their inks would scatter.)
  for (const svg of document.querySelectorAll("svg")) {
    if (hidden(svg) || svg.closest(".tile, defs, symbol") || svg.parentElement?.closest("svg")) continue;
    // (An icon centred in a small square of its own, as the keys' "All your plugins": the square.)
    const box = ((svg.parentElement && markOf(svg.parentElement)) ?? svg).getBoundingClientRect();
    if (box.width < 2) continue;
    runs.push({ el: svg, ink: box.left, top: box.top, bottom: box.bottom, what: "(icon)" });
  }
  // Fields: where their words start; tiles, pictures and drawn marks (a checkbox, a radio's circle): their edge.
  for (const field of document.querySelectorAll("input:not([type=hidden]):not([type=checkbox]):not([type=radio]), textarea, select")) {
    if (hidden(field)) continue;
    const box = field.getBoundingClientRect();
    const s = style(field);
    const words = (field as HTMLInputElement).value || (field as HTMLInputElement).placeholder || "";
    if (!words) continue;
    runs.push({ el: field, ink: box.left + parseFloat(s.borderLeftWidth) + parseFloat(s.paddingLeft) + parseFloat(s.textIndent || "0"), top: box.top, bottom: box.bottom, what: `(field) ${words.slice(0, 30)}` });
  }
  for (const thing of document.querySelectorAll(".tile, img, input[type=checkbox], input[type=radio], .pick i")) {
    if (hidden(thing) || (thing.tagName === "IMG" && thing.closest(".tile"))) continue;
    const box = thing.getBoundingClientRect();
    if (box.width < 2) continue;
    runs.push({ el: thing, ink: box.left, top: box.top, bottom: box.bottom, what: thing.classList.contains("tile") ? "(tile)" : thing.tagName === "IMG" ? "(picture)" : "(mark)" });
  }

  // Each run against its column, when it begins its line there.
  const misses: InkMiss[] = [];
  const excepted: (InkMiss & { why: string })[] = [];
  let checked = 0;
  const byColumn = new Map<(typeof columns)[number], Run[]>();
  for (const run of runs) {
    const c = columnOf(run.el);
    if (!c) continue;
    byColumn.set(c, [...(byColumn.get(c) ?? []), run]);
  }
  // Beside a leading mark: in a row (a flex row or a grid of columns) up to the column, something drawn in
  // an earlier branch of that row, left of this run: the run is in the row's own next column, not this one.
  // (Every row up to the column: a wrapped chip in a row of chips, in the words beside a row's icon, is beside it.)
  const rowsOf = (el: Element, stop: Element) => {
    const rows: { row: Element; branch: Element }[] = [];
    for (let at: Element | null = el; at && at !== stop; at = at.parentElement) {
      const parent = at.parentElement;
      if (!parent) break;
      const s = style(parent);
      const row = (s.display.includes("flex") && !s.flexDirection.startsWith("column")) || (s.display.includes("grid") && s.gridTemplateColumns.trim().split(/\s+/).length > 1);
      if (row) rows.push({ row: parent, branch: at });
      if (parent === stop) break;
    }
    return rows;
  };
  const beside = (run: Run, list: Run[], stop: Element) =>
    rowsOf(run.el, stop).some(({ row, branch }) => list.some((other) => other !== run && other.ink < run.ink - 1 && row.contains(other.el) && !branch.contains(other.el)));
  for (const [c, list] of byColumn)
    for (const run of list) {
      if (Math.abs(run.ink - c.x) > options.window) continue;
      const begins = !list.some((other) => other !== run && other.ink < run.ink - 1 && other.ink > c.x - options.window && other.top < run.bottom - 2 && other.bottom > run.top + 2) && !beside(run, list, c.el);
      if (!begins) continue;
      checked++;
      const delta = Math.round((run.ink - c.x) * 10) / 10;
      if (Math.abs(delta) <= options.tolerance) continue;
      const miss = { column: c.selector, x: Math.round(c.x * 10) / 10, ink: Math.round(run.ink * 10) / 10, delta, what: run.what, selector: path(run.el) };
      const exception = options.exceptions.find(([selector]) => run.el.closest(selector));
      if (exception) excepted.push({ ...miss, why: exception[1] });
      else misses.push(miss);
    }
  return { columns: columns.map((c) => ({ selector: c.selector, x: Math.round(c.x * 10) / 10 })), checked, misses, excepted };
}
