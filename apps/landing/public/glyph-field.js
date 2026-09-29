// Zeron glyph field — a dark-mode descendant of anara.com's hero ASCII background.
//
// Anara: pale-gray math equations on white, cut into cloud-like gaps by a noise field,
// a pointer trail that darkens cells, 0.88 scroll parallax.
// Zeron: the same grid, but set in engraved stone. Text is agent traces and zero-math,
// glyphs sit barely above the obsidian, a slow violet light shaft sweeps through them
// (the canyon beam), and the pointer leaves a violet afterglow instead of ink.
//
// Cells are scattered by a hash (no clouds), and fade to `clearOpacity` behind the
// text and elements they're told to keep legible, so copy sits on clean ground.
//
// mountGlyphField(host, opts) → { destroy }

const DEFAULT_LINES = [
  'zeron run --harness claude',
  'lim n→∞ 1/n = 0',
  'git worktree add ../quiet-aperture',
  'codex › plan → edit → test',
  '0 errors  0 warnings',
  'spawn(agent) for agent in [claude, codex, opencode, cursor, pi]',
  'x · 0 = 0',
  'diff --git a/engine b/engine',
  '✓ 312 passed',
  'e^(iπ) + 1 = 0',
  'await turn.complete()',
  'Σ tokens → ∅',
  'opencode › reasoning',
  'ssh zeron@threshold',
  'f(0) = 0',
  'claude › subagent › explore',
  'git merge --ff-only',
  'origin = (0, 0, 0)',
];

export function mountGlyphField(host, opts = {}) {
  const o = {
    lines: DEFAULT_LINES,
    cellW: 7,
    cellH: 13,
    font: '11px "Geist Mono", ui-monospace, SFMono-Regular, Menlo, monospace',
    // base glyph color as [r,g,b] and alpha range
    base: [196, 188, 226],
    baseAlpha: [0.028, 0.07],
    glow: [167, 139, 250], // #a78bfa
    glowAlpha: 0.75,
    // which fraction of cells are kept (lower = more gaps)
    density: 0.62,
    parallax: 0.88,
    // light shaft: angle in degrees, width as fraction of the diagonal, sweep seconds (0 = static)
    beam: { angle: -38, width: 0.14, sweep: 26, alpha: 0.22, offset: 0.5 },
    // clear hole (ellipse, fractions of the field) thinning the cells under it
    clear: null, // e.g. { x: 0.5, y: 0.45, rx: 0.3, ry: 0.18 }
    // elements whose box, and text elements whose rendered lines, keep glyphs
    // at `clearOpacity`, feathered out over `clearFeather` px
    clearElements: null,
    clearTextElements: null,
    clearPadding: 4,
    clearFeather: 24,
    clearOpacity: 0.2,
    // end the field partway down this element instead of at the host's bottom
    endElement: null,
    endFraction: 1,
    pointerTarget: window,
    ...opts,
  };
  const BEAM = { angle: -38, width: 0.14, sweep: 26, alpha: 0.22, offset: 0.5 };
  o.beam = opts.beam === null || opts.beam === false ? null : { ...BEAM, ...opts.beam };
  const clearEls = o.clearElements || [];
  const clearTextEls = o.clearTextElements || [];
  const text = o.lines.join('     ') + '     ';
  const coarse = matchMedia('(hover: none), (max-width: 767px)').matches;
  const reduced = matchMedia('(prefers-reduced-motion: reduce)').matches;

  const wrap = document.createElement('div');
  wrap.className = 'glyph-field';
  wrap.setAttribute('aria-hidden', 'true');
  Object.assign(wrap.style, {
    position: 'absolute', inset: '0', overflow: 'hidden', pointerEvents: 'none', zIndex: 0,
    maskImage: 'linear-gradient(to bottom, transparent 0%, #000 8%, #000 80%, transparent 100%)',
    WebkitMaskImage: 'linear-gradient(to bottom, transparent 0%, #000 8%, #000 80%, transparent 100%)',
  });
  if (o.endElement) wrap.style.bottom = 'auto';
  const inner = document.createElement('div');
  Object.assign(inner.style, { position: 'absolute', left: 0, top: 0, width: '100%', willChange: 'transform' });
  const base = document.createElement('canvas');
  const lit = document.createElement('canvas');
  for (const c of [base, lit]) Object.assign(c.style, { position: 'absolute', left: 0, top: 0 });
  inner.append(base, lit);
  wrap.append(inner);
  host.prepend(wrap);
  const bx = base.getContext('2d');
  const lx = lit.getContext('2d');

  let cols = 0, rows = 0, W = 0, H = 0, extra = 0;
  let clears = [];
  let heat = new Float32Array(0);
  let hot = new Set();
  const trail = [];
  const charAt = (r, c) => text[((r * 41) % text.length + c) % text.length];

  // 0.3..0.7 intensity of a kept cell, or -1 if it's a gap
  const field = (r, c) => {
    let h = Math.imul(r + 1, 374761393) ^ Math.imul(c + 1, 668265263);
    h = Math.imul(h ^ (h >>> 13), 1274126177);
    h = (h ^ (h >>> 16)) >>> 0;
    let v = h / 4294967296;
    if (o.clear) {
      const x = c / cols, y = (r * o.cellH - extra) / (H - 2 * extra || 1);
      const d = Math.hypot((x - o.clear.x) / o.clear.rx, (y - o.clear.y) / o.clear.ry);
      v -= Math.max(0, 1 - d) * 0.6;
    }
    if (v <= 1 - o.density) return -1;
    return 0.3 + 0.4 * ((h >>> 8) % 256) / 255;
  };

  // 1 away from cleared text/elements, easing down to clearOpacity inside them
  const clearFactor = (r, c) => {
    if (!clears.length) return 1;
    const x = (c + 0.5) * o.cellW, y = (r + 0.5) * o.cellH - extra;
    let a = 1;
    for (const b of clears) {
      const dx = Math.max(b.left - x, 0, x - b.right), dy = Math.max(b.top - y, 0, y - b.bottom);
      const s = Math.min(1, Math.hypot(dx, dy) / o.clearFeather);
      a = Math.min(a, o.clearOpacity + (1 - o.clearOpacity) * s * s * (3 - 2 * s));
    }
    return a;
  };

  const rgba = (rgb, a) => `rgba(${rgb[0]},${rgb[1]},${rgb[2]},${a.toFixed(3)})`;

  let cache = new Float32Array(0);
  const drawCell = (r, c) => {
    bx.clearRect(c * o.cellW, r * o.cellH, o.cellW, o.cellH);
    const ch = charAt(r, c);
    if (ch === ' ') return;
    const f = cache[r * cols + c];
    const h = heat[r * cols + c];
    const k = clearFactor(r, c);
    if ((f < 0 && h <= 0) || k < 0.001) return;
    const a = f < 0 ? 0 : o.baseAlpha[0] + (o.baseAlpha[1] - o.baseAlpha[0]) * f;
    if (h > 0) {
      // mix toward glow
      const col = o.base.map((v, i) => Math.round(v + (o.glow[i] - v) * h));
      bx.fillStyle = rgba(col, (a + (o.glowAlpha - a) * h) * k);
    } else bx.fillStyle = rgba(o.base, a * k);
    bx.fillText(ch, c * o.cellW, r * o.cellH);
  };

  const setup = () => {
    const dpr = Math.min(devicePixelRatio || 1, 2);
    if (o.endElement) {
      const hr = host.getBoundingClientRect(), er = o.endElement.getBoundingClientRect();
      wrap.style.height = `${Math.max(0, er.top - hr.top + er.height * o.endFraction)}px`;
    }
    W = host.clientWidth;
    extra = 60;
    H = wrap.clientHeight + extra * 2;
    // cleared boxes, in field coordinates
    const hr = host.getBoundingClientRect();
    const local = (b) => ({
      left: b.left - hr.left - o.clearPadding, right: b.right - hr.left + o.clearPadding,
      top: b.top - hr.top - o.clearPadding, bottom: b.bottom - hr.top + o.clearPadding,
    });
    clears = clearEls.map((el) => local(el.getBoundingClientRect()));
    for (const el of clearTextEls) {
      const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
      for (let node; (node = walker.nextNode());) {
        if (!node.textContent.trim()) continue;
        const range = document.createRange();
        range.selectNodeContents(node);
        for (const b of range.getClientRects()) if (b.width && b.height) clears.push(local(b));
      }
    }
    inner.style.top = `${-extra}px`;
    inner.style.height = `${H}px`;
    cols = Math.ceil(W / o.cellW) + 1;
    rows = Math.ceil(H / o.cellH) + 1;
    trail.length = 0;
    heat = new Float32Array(cols * rows);
    cache = new Float32Array(cols * rows);
    for (let r = 0; r < rows; r++) for (let c = 0; c < cols; c++) cache[r * cols + c] = field(r, c);
    hot = new Set();
    for (const [cv, cx] of [[base, bx], [lit, lx]]) {
      cv.width = Math.floor(W * dpr); cv.height = Math.floor(H * dpr);
      cv.style.width = `${W}px`; cv.style.height = `${H}px`;
      cx.setTransform(dpr, 0, 0, dpr, 0, 0);
      cx.font = o.font; cx.textBaseline = 'top';
    }
    bx.clearRect(0, 0, W, H);
    for (let r = 0; r < rows; r++) for (let c = 0; c < cols; c++) drawCell(r, c);
    // lit layer: every glyph (gaps included, faintly), revealed only inside the beam mask
    lx.clearRect(0, 0, W, H);
    for (let r = 0; r < rows; r++) for (let c = 0; c < cols; c++) {
      const ch = charAt(r, c);
      if (ch === ' ') continue;
      const f = cache[r * cols + c], k = clearFactor(r, c);
      if (k < 0.001) continue;
      lx.fillStyle = rgba(o.glow, (f < 0 ? 0.05 : 0.18 + 0.5 * f) * k);
      lx.fillText(ch, c * o.cellW, r * o.cellH);
    }
    applyBeam(performance.now());
  };

  // beam = a soft band mask on the lit canvas
  const applyBeam = (now) => {
    if (!o.beam) { lit.style.display = 'none'; return; }
    const b = o.beam;
    const p = b.sweep && !reduced ? ((now / 1000) / b.sweep) % 1 : 0;
    const center = b.sweep && !reduced ? -0.2 + 1.4 * (0.5 - 0.5 * Math.cos(p * Math.PI * 2)) : b.offset;
    const w = b.width * 100, c = center * 100;
    const g = `linear-gradient(${b.angle + 90}deg, transparent ${c - w}%, rgba(0,0,0,${b.alpha * 0.4}) ${c - w * 0.45}%, rgba(0,0,0,${b.alpha}) ${c}%, rgba(0,0,0,${b.alpha * 0.4}) ${c + w * 0.45}%, transparent ${c + w}%)`;
    lit.style.maskImage = g; lit.style.webkitMaskImage = g;
  };

  // scroll parallax
  let sraf = 0;
  const applyScroll = () => {
    sraf = 0;
    const y = window.scrollY * (1 - o.parallax);
    inner.style.transform = `translate3d(0, ${y.toFixed(2)}px, 0)`;
  };
  const onScroll = () => { sraf ||= requestAnimationFrame(applyScroll); };

  // pointer afterglow (Anara's trail, softer)
  const LIFE = 650, SPACING = 12;
  let lx0 = -1e9, ly0 = -1e9, traf = 0;
  const loop = () => {
    const now = performance.now();
    while (trail.length && now - trail[0].born > LIFE) trail.shift();
    for (const i of hot) heat[i] = 0;
    const next = new Set();
    for (const p of trail) {
      const life = 1 - (now - p.born) / LIFE;
      if (life <= 0) continue;
      const rad = 10 + 48 * life, r2 = rad * rad;
      const c0 = Math.max(0, Math.floor((p.x - rad) / o.cellW)), c1 = Math.min(cols - 1, Math.ceil((p.x + rad) / o.cellW));
      const r0 = Math.max(0, Math.floor((p.y - rad) / o.cellH)), r1 = Math.min(rows - 1, Math.ceil((p.y + rad) / o.cellH));
      for (let r = r0; r <= r1; r++) for (let c = c0; c <= c1; c++) {
        const dx = c * o.cellW + o.cellW / 2 - p.x, dy = r * o.cellH + o.cellH / 2 - p.y, d2 = dx * dx + dy * dy;
        if (d2 >= r2) continue;
        const d = 1 - Math.sqrt(d2) / rad;
        const v = d * d * (3 - 2 * d) * life * life;
        const i = r * cols + c;
        if (v > heat[i]) heat[i] = v;
        next.add(i);
      }
    }
    for (const i of hot) if (!next.has(i)) { const r = (i / cols) | 0; drawCell(r, i - r * cols); }
    for (const i of next) { const r = (i / cols) | 0; drawCell(r, i - r * cols); }
    hot = next;
    traf = trail.length ? requestAnimationFrame(loop) : 0;
  };
  const onMove = (e) => {
    const rect = base.getBoundingClientRect();
    const x = e.clientX - rect.left, y = e.clientY - rect.top, now = performance.now();
    if (x < 0 || x > W || y < 0 || y > H) { lx0 = ly0 = -1e9; return; }
    if (lx0 < -9000) { lx0 = x; ly0 = y; }
    let dx = x - lx0, dy = y - ly0, d = Math.hypot(dx, dy);
    while (d >= SPACING) {
      const k = SPACING / d; lx0 += dx * k; ly0 += dy * k;
      trail.push({ x: lx0, y: ly0, born: now });
      dx = x - lx0; dy = y - ly0; d = Math.hypot(dx, dy);
    }
    trail.push({ x, y, born: now });
    while (trail.length > 70) trail.shift();
    lx0 = x; ly0 = y;
    traf ||= requestAnimationFrame(loop);
  };
  const onLeave = () => { lx0 = ly0 = -1e9; };

  let braf = 0;
  const beamLoop = (t) => { applyBeam(t); braf = requestAnimationFrame(beamLoop); };

  document.fonts.ready.then(() => { setup(); applyScroll(); });
  setup();
  if (!coarse && !reduced) {
    o.pointerTarget.addEventListener('pointermove', onMove, { passive: true });
    o.pointerTarget.addEventListener('pointerleave', onLeave);
  }
  if (!reduced && o.parallax !== 1) addEventListener('scroll', onScroll, { passive: true });
  if (o.beam && o.beam.sweep && !reduced) braf = requestAnimationFrame(beamLoop);
  // re-lay out when the host, the end element or anything cleared resizes
  let sizes = '';
  const ro = new ResizeObserver(() => {
    const hr = host.getBoundingClientRect();
    const endTop = o.endElement ? o.endElement.getBoundingClientRect().top - hr.top : 0;
    const next = [host.clientWidth, host.clientHeight,
      ...(o.endElement ? [endTop, o.endElement.clientHeight] : []),
      ...[...clearEls, ...clearTextEls].flatMap((el) => [el.clientWidth, el.clientHeight])].join('/');
    if (next !== sizes) { sizes = next; setup(); }
  });
  ro.observe(host);
  if (o.endElement) ro.observe(o.endElement);
  for (const el of [...clearEls, ...clearTextEls]) ro.observe(el);

  return {
    destroy() {
      cancelAnimationFrame(traf); cancelAnimationFrame(sraf); cancelAnimationFrame(braf);
      o.pointerTarget.removeEventListener('pointerleave', onLeave);
      o.pointerTarget.removeEventListener('pointermove', onMove);
      removeEventListener('scroll', onScroll);
      ro.disconnect(); wrap.remove();
    },
  };
}
