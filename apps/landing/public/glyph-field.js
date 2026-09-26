// Zeron glyph field — a dark-mode descendant of anara.com's hero ASCII background.
//
// Anara: pale-gray math equations on white, cut into cloud-like gaps by a noise field,
// a pointer trail that darkens cells, 0.88 scroll parallax.
// Zeron: the same grid, but set in engraved stone. Text is agent traces and zero-math,
// glyphs sit barely above the obsidian, a slow violet light shaft sweeps through them
// (the canyon beam), and the pointer leaves a violet afterglow instead of ink.
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
    density: 0.5,
    parallax: 0.88,
    // light shaft: angle in degrees, width as fraction of the diagonal, sweep seconds (0 = static)
    beam: { angle: -38, width: 0.14, sweep: 26, alpha: 0.22, offset: 0.5 },
    // clear hole (ellipse, fractions of viewport) so the headline stays legible
    clear: null, // e.g. { x: 0.5, y: 0.45, rx: 0.3, ry: 0.18 }
    ...opts,
  };
  const BEAM = { angle: -38, width: 0.14, sweep: 26, alpha: 0.22, offset: 0.5 };
  o.beam = opts.beam === null || opts.beam === false ? null : { ...BEAM, ...opts.beam };
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
  let heat = new Float32Array(0);
  let hot = new Set();
  const trail = [];
  const charAt = (r, c) => text[((r * 41) % text.length + c) % text.length];

  // 0..1 visibility of a cell, or -1 if it's a gap
  const field = (r, c) => {
    const n = Math.sin(c * 0.045 + 0.4) * Math.cos(r * 0.06)
      + 0.7 * Math.sin((c + r) * 0.023 + 1.1)
      + 0.5 * Math.sin((c - r) * 0.037 + 2.2)
      + 0.35 * Math.cos(c * 0.017 - r * 0.02);
    const hash = ((r * 374761393) ^ (c * 668265263)) >>> 0;
    let v = (n + 2.55) / 5.1 + ((hash % 100) / 100 - 0.5) * 0.38;
    if (o.clear) {
      const x = c / cols, y = (r * o.cellH - extra) / (H - 2 * extra || 1);
      const d = Math.hypot((x - o.clear.x) / o.clear.rx, (y - o.clear.y) / o.clear.ry);
      v -= Math.max(0, 1 - d) * 0.6;
    }
    if (v <= 1 - o.density) return -1;
    const t = Math.sin(c * 0.02) * Math.cos(r * 0.03) + 0.5 * Math.sin((c + r) * 0.015);
    return Math.min(1, Math.max(0, (t + 1.5) / 3));
  };

  const rgba = (rgb, a) => `rgba(${rgb[0]},${rgb[1]},${rgb[2]},${a.toFixed(3)})`;

  let cache = new Float32Array(0);
  const drawCell = (r, c) => {
    bx.clearRect(c * o.cellW, r * o.cellH, o.cellW, o.cellH);
    const ch = charAt(r, c);
    if (ch === ' ') return;
    const f = cache[r * cols + c];
    const h = heat[r * cols + c];
    if (f < 0 && h <= 0) return;
    const a = f < 0 ? 0 : o.baseAlpha[0] + (o.baseAlpha[1] - o.baseAlpha[0]) * f;
    if (h > 0) {
      // mix toward glow
      const m = h;
      const col = o.base.map((v, i) => Math.round(v + (o.glow[i] - v) * m));
      bx.fillStyle = rgba(col, a + (o.glowAlpha - a) * m);
    } else bx.fillStyle = rgba(o.base, a);
    bx.fillText(ch, c * o.cellW, r * o.cellH);
  };

  const setup = () => {
    const dpr = Math.min(devicePixelRatio || 1, 2);
    W = host.clientWidth;
    extra = 60;
    H = host.clientHeight + extra * 2;
    inner.style.top = `${-extra}px`;
    inner.style.height = `${H}px`;
    cols = Math.ceil(W / o.cellW) + 1;
    rows = Math.ceil(H / o.cellH) + 1;
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
      const f = cache[r * cols + c];
      lx.fillStyle = rgba(o.glow, f < 0 ? 0.05 : 0.18 + 0.5 * f);
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

  // pointer afterglow (Anara's trail, longer + softer)
  const LIFE = 900, SPACING = 12;
  let lx0 = -1e9, ly0 = -1e9, traf = 0;
  const loop = () => {
    const now = performance.now();
    while (trail.length && now - trail[0].born > LIFE) trail.shift();
    for (const i of hot) heat[i] = 0;
    const next = new Set();
    for (const p of trail) {
      const life = 1 - (now - p.born) / LIFE;
      if (life <= 0) continue;
      const rad = 10 + 70 * life, r2 = rad * rad;
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
    if (y < -50 || y > H + 50) return;
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

  let braf = 0;
  const beamLoop = (t) => { applyBeam(t); braf = requestAnimationFrame(beamLoop); };

  document.fonts.ready.then(() => { setup(); applyScroll(); });
  setup();
  if (!coarse && !reduced) addEventListener('pointermove', onMove, { passive: true });
  if (!reduced) addEventListener('scroll', onScroll, { passive: true });
  if (o.beam && o.beam.sweep && !reduced) braf = requestAnimationFrame(beamLoop);
  let rw = 0;
  const ro = new ResizeObserver(() => { if (host.clientWidth !== rw) { rw = host.clientWidth; setup(); } });
  ro.observe(host);

  return {
    destroy() {
      cancelAnimationFrame(traf); cancelAnimationFrame(sraf); cancelAnimationFrame(braf);
      removeEventListener('pointermove', onMove); removeEventListener('scroll', onScroll);
      ro.disconnect(); wrap.remove();
    },
  };
}
