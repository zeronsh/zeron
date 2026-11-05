-- The picker run in a page while an annotation is made: the shipping app's
-- (crates/ui/src/browser/annotate.js) with what zgx gives it. The page
-- posts to zgx (`window.zgx.postMessage`) instead of being polled, so a pick
-- reaches the tab the moment it is made; a drag over the page selects a
-- region (the smallest element holding it stands for it); the overlay
-- draws in the design system's colours and type, handed over at start.
--
-- The script runs in the page's own world, so everything it posts is page
-- data: the tab checks the frame and the nonce, bounds the fields and
-- labels them as untrusted. Only real (trusted) input picks; secrets the
-- page shows in attributes or the URL are scrubbed before they leave.
--
--   window.__zeronAnnotator.start(nonce, theme)   begin (ends any pick before)
--   .stop()                                        end, posting "cancelled"
--   .label(marker, n)                              number a pick's mark as its chip
--   .keep([n, …])                                  marks whose chips remain
--   .focus(n)                                      the mark of the chip under the pointer
--
-- Posted (kind "zeron.annotation", the nonce with each): `{ event = "pick",
-- pick = { …capture, marker } }` per pick, and `{ event = "state", state =
-- "done" | "cancelled" }` as the pick ends.

return [==[
(() => {
  if (window.__zeronAnnotator) return;
  const MAX_HTML = 2000, MAX_ATTRS = 12, MAX_VALUE = 120, MAX_TEXT = 200;
  const VOID = new Set(['area','base','br','col','embed','hr','img','input','link','meta','source','track','wbr']);
  const SECRET = /pass|secret|token|api[-_]?key|auth|session|csrf|nonce|signature|credential/i;
  const SECRET_PARAM = /pass|secret|token|key|auth|session|csrf|sig|code|state|credential|jwt|ticket/i;
  const OPAQUE = /^(?:eyJ[\w-]+\.[\w-]+\.[\w-]*|[A-Za-z0-9+/_=-]{40,})$/;
  const URL_ATTRS = new Set(['href','src','action','formaction','poster','cite','data','ping']);
  const STYLES = ['display','position','width','height','margin','padding','gap','flex-direction',
    'justify-content','align-items','grid-template-columns','font-family','font-size','font-weight',
    'line-height','color','background-color','background-image','border','border-radius','box-shadow',
    'opacity','z-index','overflow'];
  const DEFAULTS = new Set(['none','normal','auto','0px','0px 0px','rgba(0, 0, 0, 0)','static','visible','1','0px none rgb(0, 0, 0)']);
  let state = 'idle', nonce = 0, target = null, frame = 0, theme = {}, session = 0;
  // The words the app hands over in the locale shown (start's theme), else English.
  const word = (k, d) => (typeof theme[k] === 'string' && theme[k]) || d;
  let host = null, ui = null, marks = new Map(), seq = 0, layer = null, markFrame = 0, resizes = null, mutations = null;
  let down = null, drag = null, skipClick = false;

  const post = (message) => {
    if (window.zgx && window.zgx.postMessage) window.zgx.postMessage({ kind: 'zeron.annotation', nonce, ...message });
  };
  const clip = (s, n) => (s.length > n ? s.slice(0, n - 1) + '…' : s);
  const squash = (s) => (s || '').replace(/\s+/g, ' ').trim();
  const esc = (s) => (window.CSS && CSS.escape ? CSS.escape(s) : s.replace(/[^\w-]/g, '\\$&'));
  const scrubParams = (params) => {
    let changed = false;
    for (const key of [...params.keys()]) {
      if (SECRET_PARAM.test(key) || OPAQUE.test(params.get(key) || '')) { params.set(key, 'redacted'); changed = true; }
    }
    return changed;
  };
  const scrubUrl = (value) => {
    let url;
    try { url = new URL(value, location.href); } catch { return value; }
    if (!/^(https?|wss?):$/.test(url.protocol)) return value;
    url.username = ''; url.password = '';
    scrubParams(url.searchParams);
    if (url.hash.includes('=')) {
      const params = new URLSearchParams(url.hash.slice(1));
      if (scrubParams(params)) url.hash = params.toString();
    }
    return url.href;
  };
  const describe = (el) => {
    let out = el.tagName.toLowerCase();
    if (el.id) out += '#' + el.id;
    for (const c of [...el.classList].slice(0, 3)) out += '.' + c;
    return out;
  };
  const unique = (sel) => { try { return document.querySelectorAll(sel).length === 1; } catch { return false; } };
  const selector = (el) => {
    if (el.id && unique('#' + esc(el.id))) return '#' + esc(el.id);
    const parts = [];
    for (let node = el; node && node.nodeType === 1 && node !== document.documentElement; node = node.parentElement) {
      if (node !== el && node.id && unique('#' + esc(node.id))) { parts.unshift('#' + esc(node.id)); break; }
      let part = node.tagName.toLowerCase();
      const testid = node.getAttribute('data-testid');
      if (testid) part += `[data-testid="${testid.replace(/"/g, '\\"')}"]`;
      else part += [...node.classList].slice(0, 2).map((c) => '.' + esc(c)).join('');
      const parent = node.parentElement;
      if (parent && !testid) {
        const same = [...parent.children].filter((c) => c.tagName === node.tagName);
        if (same.length > 1) part += `:nth-of-type(${same.indexOf(node) + 1})`;
      }
      parts.unshift(part);
      if (unique(parts.join(' > '))) break;
    }
    return parts.join(' > ');
  };
  const IMPLICIT = { a: 'link', button: 'button', img: 'img', nav: 'navigation', main: 'main', header: 'banner',
    footer: 'contentinfo', form: 'form', table: 'table', ul: 'list', ol: 'list', li: 'listitem', select: 'combobox',
    textarea: 'textbox', h1: 'heading', h2: 'heading', h3: 'heading', h4: 'heading', h5: 'heading', h6: 'heading', article: 'article', section: 'region' };
  const role = (el) => {
    const explicit = el.getAttribute('role');
    if (explicit) return explicit;
    const name = el.tagName.toLowerCase();
    if (name === 'input') return ({ checkbox: 'checkbox', radio: 'radio', button: 'button', submit: 'button', range: 'slider' })[el.type] || 'textbox';
    return IMPLICIT[name] || '';
  };
  const accessibleName = (el) => {
    const label = el.getAttribute('aria-label');
    if (label) return label;
    const by = el.getAttribute('aria-labelledby');
    if (by) return squash(by.split(/\s+/).map((id) => document.getElementById(id)?.innerText || '').join(' '));
    if (el.labels && el.labels.length) return squash(el.labels[0].innerText);
    const alt = el.getAttribute('alt') || el.getAttribute('title') || el.getAttribute('placeholder');
    if (alt) return alt;
    return el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' ? '' : clip(squash(el.innerText), 80);
  };
  const safeAttrs = (el) => {
    const out = [];
    for (const attr of [...el.attributes]) {
      if (out.length >= MAX_ATTRS) break;
      const name = attr.name.toLowerCase();
      if (name.startsWith('on') || name === 'value' || SECRET.test(name)) continue;
      if (name === 'style' && attr.value.length > MAX_VALUE) continue;
      let value = attr.value;
      if (URL_ATTRS.has(name)) value = scrubUrl(value);
      else if (OPAQUE.test(value.trim())) value = '…';
      out.push(`${name}="${clip(value, MAX_VALUE).replace(/"/g, '&quot;')}"`);
    }
    return out.length ? ' ' + out.join(' ') : '';
  };
  const sensitive = (el) => el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.isContentEditable;
  const render = (el, depth, lines, indent) => {
    const name = el.tagName.toLowerCase();
    const open = `<${name}${safeAttrs(el)}>`;
    if (VOID.has(name)) { lines.push(indent + open); return; }
    if (name === 'script' || name === 'style' || name === 'svg' && depth > 0) { lines.push(`${indent}${open}…</${name}>`); return; }
    const children = [...el.childNodes].filter((n) => n.nodeType === 1 || (n.nodeType === 3 && n.textContent.trim()));
    const elements = children.filter((n) => n.nodeType === 1);
    if (!elements.length) {
      const text = sensitive(el) ? '' : clip(squash(el.textContent), MAX_VALUE);
      lines.push(`${indent}${open}${text}</${name}>`);
      return;
    }
    if (depth >= 2) { lines.push(`${indent}${open}…</${name}>`); return; }
    lines.push(indent + open);
    let shown = 0;
    for (const child of children) {
      if (shown === 10) { lines.push(`${indent}  <!-- ${children.length - shown} more -->`); break; }
      if (child.nodeType === 3) lines.push(indent + '  ' + clip(squash(child.textContent), 80));
      else render(child, depth + 1, lines, indent + '  ');
      shown += 1;
    }
    lines.push(`${indent}</${name}>`);
  };
  const html = (el) => { const lines = []; render(el, 0, lines, ''); return clip(lines.join('\n'), MAX_HTML); };
  const styles = (el) => {
    const computed = getComputedStyle(el), out = [];
    for (const name of STYLES) {
      const value = computed.getPropertyValue(name).trim();
      if (!value || (name !== 'display' && DEFAULTS.has(value))) continue;
      out.push([name, clip(value, MAX_VALUE)]);
      if (out.length >= 20) break;
    }
    return out;
  };
  const capture = (el) => {
    const rect = el.getBoundingClientRect();
    const path = [];
    for (let node = el.parentElement; node && node !== document.documentElement; node = node.parentElement) path.unshift(describe(node));
    return {
      url: scrubUrl(location.href), title: document.title, element: describe(el), selector: selector(el),
      path: path.slice(-8), role: role(el), name: clip(squash(accessibleName(el)), 120),
      text: sensitive(el) ? '' : clip(squash(el.innerText || el.textContent), MAX_TEXT), html: html(el),
      rect: [Math.round(rect.x), Math.round(rect.y), Math.round(rect.width), Math.round(rect.height)],
      viewport: [innerWidth, innerHeight], styles: styles(el),
    };
  };

  // The design system's look, handed over by the tab: its accent, the
  // popover's surface and edge, its inks and its face.
  const vars = () => {
    const t = theme, on = t.on_accent || contrast(t.accent || '#9277b0');
    return `:host{all:initial;--accent:${t.accent};--on-accent:${on};--surface:${t.surface};--border:${t.border};` +
      `--text:${t.text};--muted:${t.muted};--faint:${t.faint};` +
      `--shadow:${t.dark ? '0 6px 20px rgba(0,0,0,.4),0 1px 2px rgba(0,0,0,.3)' : '0 6px 20px rgba(15,20,30,.12),0 1px 2px rgba(15,20,30,.1)'};` +
      `--font:${JSON.stringify(t.font || 'system-ui')},system-ui,-apple-system,"Segoe UI",sans-serif}` +
      `*{box-sizing:border-box}`;
  };
  const contrast = (hex) => {
    const n = parseInt(hex.slice(1, 7), 16);
    if (Number.isNaN(n)) return '#ffffff';
    const [r, g, b] = [n >> 16, (n >> 8) & 255, n & 255].map((c) => { c /= 255; return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4; });
    return 0.2126 * r + 0.7152 * g + 0.0722 * b > 0.4 ? '#0b0d12' : '#ffffff';
  };
  // Outlines sit just outside the element (a gap, then the line), so they
  // never draw over its own border, and round concentric with it: the
  // element's corner plus how far out they stand. The top-left corner
  // carries the label, so it stays nearly square under it.
  const BORDER = 1.5, GAP = 2, OUT = BORDER + GAP, FLAG = 20, TAB = 18, CORNER = 2;
  const ringOf = (own, r) => ({ left: r.left - OUT, top: r.top - OUT, width: r.width + OUT * 2, height: r.height + OUT * 2, radius: Math.min(own + OUT, 18) || 4 });
  const ring = (el, r) => ringOf(Math.min(parseFloat(getComputedStyle(el).borderTopLeftRadius) || 0, r.width / 2, r.height / 2), r);
  const place = (node, box) => Object.assign(node.style, {
    left: box.left + 'px', top: box.top + 'px', width: box.width + 'px', height: box.height + 'px',
    borderRadius: `${CORNER}px ${box.radius}px ${box.radius}px ${box.radius}px`,
  });
  const shadowHost = (tag, z, css) => {
    const node = document.createElement(tag);
    node.style.cssText = `position:fixed;inset:0;z-index:${z};pointer-events:none;`;
    const root = node.attachShadow({ mode: 'closed' });
    root.innerHTML = `<style>${vars()}${css}</style>`;
    document.documentElement.appendChild(node);
    return [node, root];
  };
  const FLAG_CSS = `.flag{position:fixed;display:none;align-items:center;height:${FLAG}px;padding:0 7px;border-radius:${CORNER}px 5px 0 0;
      background:var(--accent);color:var(--on-accent);font:500 11px/1 var(--font);white-space:nowrap;overflow:hidden;max-width:min(70vw,440px)}
    .flag b,.flag .n{font-weight:650}
    .flag .n,.flag .dim{font-variant-numeric:tabular-nums;flex:none}
    .flag .cls{opacity:.72;overflow:hidden;text-overflow:ellipsis}
    .flag .n{margin-right:6px}
    .flag .dim{opacity:.6;margin-left:6px}`;
  const flagAt = (flag, box, height, outline) => {
    const width = flag.offsetWidth;
    const left = Math.max(0, Math.min(box.left, innerWidth - width));
    const above = box.top - height + BORDER;
    const inside = above < 2;
    const r = (free) => (free ? '5px' : '0');
    const pastRight = left + width > box.left + box.width;
    const pastLeft = left < box.left;
    flag.style.left = left + 'px';
    flag.style.top = (inside ? Math.max(box.top, 0) : above) + 'px';
    flag.style.borderRadius = inside
      ? `0 ${r(pastRight)} 5px ${r(pastLeft)}`
      : `${pastLeft ? '5px' : CORNER + 'px'} 5px ${r(pastRight)} ${r(pastLeft)}`;
    if (!inside && pastRight) outline.style.borderTopRightRadius = CORNER + 'px';
  };

  const mount = () => {
    let root;
    [host, root] = shadowHost('zeron-annotator', 2147483647, `
      .box{position:fixed;display:none;border:${BORDER}px solid var(--accent);background:color-mix(in srgb,var(--accent) 8%,transparent);
        transition:left 80ms ease-out,top 80ms ease-out,width 80ms ease-out,height 80ms ease-out,border-radius 80ms ease-out}
      .box.region{border-style:dashed;transition:none}
      ${FLAG_CSS}
      .hint{position:fixed;left:50%;bottom:16px;display:flex;align-items:center;gap:10px;height:30px;padding:0 12px 0 11px;border-radius:9px;
        background:var(--surface);border:1px solid var(--border);box-shadow:var(--shadow);font:500 12px/1 var(--font);color:var(--text);white-space:nowrap;
        transform:translate(-50%,0);transition:opacity 140ms ease-out,transform 140ms ease-out}
      .hint.enter{opacity:0;transform:translate(-50%,4px)}
      .hint.top{bottom:auto;top:16px}
      .hint .dot{width:6px;height:6px;border-radius:3px;background:var(--accent);flex:none}
      .hint .sep{width:1px;height:12px;background:var(--border);flex:none}
      .hint .keys{color:var(--muted);font-weight:400}
      .hint .keys b{color:var(--text);font-weight:600}
      @media (prefers-reduced-motion:reduce){.box,.hint{transition:none}}
    `);
    root.innerHTML += `<div class="box"></div><div class="flag"></div>
      <div class="hint enter"><span class="dot"></span><span class="title"></span><span class="sep"></span>
      <span class="keys"></span></div>`;
    ui = { box: root.querySelector('.box'), flag: root.querySelector('.flag'), hint: root.querySelector('.hint'), title: root.querySelector('.title') };
    root.querySelector('.keys').textContent = word('hint_keys', 'Shift-click to add · Esc to finish');
    count();
    requestAnimationFrame(() => ui && ui.hint.classList.remove('enter'));
    const cursor = document.createElement('style');
    cursor.id = 'zeron-annotator-cursor';
    cursor.textContent = '*{cursor:crosshair!important}';
    document.documentElement.appendChild(cursor);
  };
  const markOf = (el) => { for (const m of marks.values()) if (m.el === el && m.n) return m.n; return 0; };
  const label = (el, r) => {
    const n = markOf(el);
    const name = el.tagName.toLowerCase();
    ui.flag.innerHTML = `${n ? '<span class="n"></span>' : ''}<b></b><span class="cls"></span><span class="dim"></span>`;
    if (n) ui.flag.querySelector('.n').textContent = String(n);
    ui.flag.querySelector('b').textContent = name;
    ui.flag.querySelector('.cls').textContent = describe(el).slice(name.length);
    ui.flag.querySelector('.dim').textContent = `${Math.round(r.width)} × ${Math.round(r.height)}`;
  };
  const paint = () => {
    frame = 0;
    if (!ui) return;
    if (drag) {
      const box = ringOf(4, drag);
      ui.box.classList.add('region');
      place(ui.box, box);
      ui.box.style.display = 'block';
      ui.flag.innerHTML = '<b></b><span class="dim"></span>';
      ui.flag.querySelector('b').textContent = word('region', 'region');
      ui.flag.querySelector('.dim').textContent = `${Math.round(drag.width)} × ${Math.round(drag.height)}`;
      ui.flag.style.display = 'flex';
      flagAt(ui.flag, box, FLAG, ui.box);
      return;
    }
    ui.box.classList.remove('region');
    if (!target) { ui.box.style.display = ui.flag.style.display = 'none'; return; }
    const r = target.getBoundingClientRect();
    const box = ring(target, r);
    place(ui.box, box);
    ui.box.style.display = 'block';
    label(target, r);
    ui.flag.style.display = 'flex';
    flagAt(ui.flag, box, FLAG, ui.box);
    ui.hint.classList.toggle('top', r.bottom > innerHeight - 60 && r.top > 60);
  };
  const schedule = () => { if (!frame) frame = requestAnimationFrame(paint); };
  const at = (x, y) => {
    const el = document.elementFromPoint(x, y);
    return el && el !== host && el !== layer && el !== document.documentElement ? el : null;
  };

  // Picked elements keep their outline, numbered like their chip, until the
  // chip leaves the draft; a region keeps its dashed box where it was drawn
  // on the page. Repaints run only when something moves, never on a timer.
  const rectOfMark = (mark) => {
    if (mark.el) return mark.el.isConnected ? mark.el.getBoundingClientRect() : null;
    const d = mark.doc;
    return { left: d.x - scrollX, top: d.y - scrollY, width: d.w, height: d.h, right: d.x - scrollX + d.w, bottom: d.y - scrollY + d.h };
  };
  const paintMarks = () => {
    markFrame = 0;
    for (const mark of marks.values()) {
      const r = rectOfMark(mark);
      const shown = r && r.width + r.height > 0 && r.bottom > 0 && r.top < innerHeight;
      mark.box.style.display = shown ? 'block' : 'none';
      if (!shown) { mark.flag.style.display = 'none'; continue; }
      const box = mark.el ? ring(mark.el, r) : ringOf(4, r);
      place(mark.box, box);
      // The number's tab: cut into the outline's top-left corner on an
      // element big enough to take it (the shipping app's), else a flag
      // just above a small one, off it (flagAt tucks it inside only where
      // the viewport's top leaves no room).
      mark.flag.style.display = 'flex';
      if (box.width < 44 || box.height < 34) {
        flagAt(mark.flag, box, TAB, mark.box);
      } else {
        // (An outline cut off by the viewport's top keeps its tab in view.)
        mark.flag.style.left = box.left + 'px';
        mark.flag.style.top = Math.min(Math.max(box.top, 0), box.top + box.height - TAB) + 'px';
        mark.flag.style.borderRadius = `${CORNER}px 0 5px 0`;
      }
    }
  };
  const scheduleMarks = () => { if (!markFrame) markFrame = requestAnimationFrame(paintMarks); };
  const watchMarks = (on) => {
    if (on && !layer) {
      let root;
      [layer, root] = shadowHost('zeron-annotations', 2147483646, `
        .mark{position:fixed;display:none;border:${BORDER}px solid var(--accent);background:color-mix(in srgb,var(--accent) 4%,transparent);
          transition:background-color 120ms ease-out}
        .mark.region{border-style:dashed}
        .mark.focus{background:color-mix(in srgb,var(--accent) 16%,transparent)}
        ${FLAG_CSS}
        .flag{height:${TAB}px;padding:0 6px;font-weight:650;font-variant-numeric:tabular-nums}
        .flag:empty{visibility:hidden}
        .mark.new{animation:settle 320ms ease-out}
        @keyframes settle{from{background:color-mix(in srgb,var(--accent) 26%,transparent)}}
        @media (prefers-reduced-motion:reduce){.mark,.mark.new{transition:none;animation:none}}
      `);
      layer.root = root;
      resizes = new ResizeObserver(scheduleMarks);
      mutations = new MutationObserver(scheduleMarks);
      mutations.observe(document.documentElement, { subtree: true, childList: true, attributes: true });
      addEventListener('scroll', scheduleMarks, { capture: true, passive: true });
      addEventListener('resize', scheduleMarks, { passive: true });
    } else if (!on && layer) {
      resizes.disconnect(); mutations.disconnect();
      removeEventListener('scroll', scheduleMarks, { capture: true });
      removeEventListener('resize', scheduleMarks);
      if (markFrame) cancelAnimationFrame(markFrame);
      layer.remove();
      layer = resizes = mutations = null; markFrame = 0;
    }
  };
  const mark = (el, region) => {
    watchMarks(true);
    const box = document.createElement('div');
    box.className = region ? 'mark region new' : 'mark new';
    const flag = document.createElement('div');
    flag.className = 'flag';
    layer.root.append(box, flag);
    if (el) resizes.observe(el);
    const doc = region ? { x: region.left + scrollX, y: region.top + scrollY, w: region.width, h: region.height } : null;
    marks.set(++seq, { el, doc, box, flag, n: 0 });
    scheduleMarks();
    return seq;
  };
  const swallow = (e) => { if (state === 'active') { e.preventDefault(); e.stopImmediatePropagation(); } };
  const rectBetween = (a, b) => {
    const left = Math.min(a.x, b.x), top = Math.min(a.y, b.y);
    const right = Math.max(a.x, b.x), bottom = Math.max(a.y, b.y);
    return { left, top, right, bottom, width: right - left, height: bottom - top };
  };
  // The smallest element holding the whole region stands for it.
  const holder = (r) => {
    let el = at((r.left + r.right) / 2, (r.top + r.bottom) / 2) || document.body;
    while (el && el !== document.body && el !== document.documentElement) {
      const b = el.getBoundingClientRect();
      if (b.left <= r.left + 1 && b.top <= r.top + 1 && b.right >= r.right - 1 && b.bottom >= r.bottom - 1) break;
      el = el.parentElement;
    }
    return el || document.body;
  };
  // The hint says how many this session has picked (the shipping app's
  // count: earlier sessions' marks aren't this one's).
  const count = () => {
    if (!ui) return;
    const n = session;
    ui.title.textContent = n ? word('selected', '{n} selected').replace('{n}', String(n)) : word('hint', 'Point at an element, or drag a region');
  };
  const deliver = (pick, more) => {
    session += 1;
    post({ event: 'pick', pick });
    if (!more) stop('done');
  };
  // Synthetic events come from the page, not the user: they never pick.
  const onDown = (e) => {
    swallow(e);
    if (!e.isTrusted || e.button !== 0) return;
    down = { x: e.clientX, y: e.clientY };
  };
  const onMove = (e) => {
    if (!e.isTrusted) return;
    if (down) {
      const here = { x: e.clientX, y: e.clientY };
      if (drag || Math.hypot(here.x - down.x, here.y - down.y) > 6) {
        drag = rectBetween(down, here);
        target = null;
        schedule();
        return;
      }
    }
    const el = at(e.clientX, e.clientY);
    if (el && el !== target) { target = el; schedule(); }
  };
  const onUp = (e) => {
    swallow(e);
    if (!e.isTrusted) return;
    if (drag) {
      const r = drag;
      drag = null; down = null; skipClick = true;
      if (r.width >= 8 && r.height >= 8) {
        const el = holder(r);
        const pick = capture(el);
        pick.element = `region ${Math.round(r.width)} × ${Math.round(r.height)} in ${describe(el)}`;
        pick.rect = [Math.round(r.left), Math.round(r.top), Math.round(r.width), Math.round(r.height)];
        pick.marker = mark(null, r);
        deliver(pick, e.shiftKey);
      } else {
        schedule();
      }
      return;
    }
    down = null;
  };
  const onClick = (e) => {
    swallow(e);
    if (!e.isTrusted) return;
    if (skipClick) { skipClick = false; return; }
    const el = at(e.clientX, e.clientY) || target;
    if (!el) return;
    deliver({ ...capture(el), marker: mark(el, null) }, e.shiftKey);
  };
  const onKey = (e) => { if (e.isTrusted && e.key === 'Escape') { swallow(e); stop('cancelled'); } };
  const LISTEN = [['mousemove', onMove], ['click', onClick], ['mousedown', onDown], ['mouseup', onUp],
    ['pointerdown', swallow], ['pointerup', swallow], ['auxclick', swallow], ['contextmenu', swallow], ['keydown', onKey],
    ['scroll', schedule], ['resize', schedule]];
  const stop = (next) => {
    if (state !== 'active') return;
    state = next;
    for (const [type, fn] of LISTEN) window.removeEventListener(type, fn, true);
    if (frame) cancelAnimationFrame(frame);
    host?.remove();
    document.getElementById('zeron-annotator-cursor')?.remove();
    host = ui = target = null; down = drag = null;
    post({ event: 'state', state: next });
  };
  const api = {
    start(id, palette) {
      stop('cancelled');
      nonce = id; state = 'active'; theme = palette || {}; session = 0;
      mount();
      count();
      for (const [type, fn] of LISTEN) window.addEventListener(type, fn, true);
    },
    stop() { stop('cancelled'); },
    label(id, n) {
      const found = marks.get(id);
      if (!found) return;
      found.n = n;
      found.flag.textContent = String(n);
      scheduleMarks();
      count();
    },
    // The chips renumbered (one left, the rest closing up): `[[was, now], …]`
    // for each kept; a numbered mark not among them goes, the rest take
    // their new numbers together (no two ever share one between).
    sync(pairs) {
      const next = new Map(pairs.map(([was, now]) => [was, now]));
      for (const [id, found] of marks) {
        if (!found.n) continue;
        if (!next.has(found.n)) { if (found.el) resizes.unobserve(found.el); found.box.remove(); found.flag.remove(); marks.delete(id); continue; }
        found.n = next.get(found.n);
        found.flag.textContent = String(found.n);
      }
      if (!marks.size) watchMarks(false);
      scheduleMarks();
      count();
    },
    keep(numbers) {
      for (const [id, found] of marks) {
        if (found.n && !numbers.includes(found.n)) { if (found.el) resizes.unobserve(found.el); found.box.remove(); found.flag.remove(); marks.delete(id); }
      }
      if (!marks.size) watchMarks(false);
      count();
    },
    // The chip under the pointer: its mark firms, and comes into view if
    // it is off it (0: none).
    focus(n) {
      for (const found of marks.values()) {
        const on = found.n === n;
        found.box.classList.toggle('focus', on);
        if (!on) continue;
        const r = rectOfMark(found);
        if (r && (r.bottom < 0 || r.top > innerHeight)) {
          if (found.el) found.el.scrollIntoView({ block: 'center', behavior: 'smooth' });
          else scrollTo({ top: found.doc.y - innerHeight / 2, behavior: 'smooth' });
        }
      }
    },
    // The element `selector` matches, picked as a click would (the mode
    // stays); false if nothing matches.
    pick(selector) {
      let el = null;
      try { el = document.querySelector(selector); } catch { return false; }
      if (!el) return false;
      deliver({ ...capture(el), marker: mark(el, null) }, true);
      return true;
    },
    // Marks for picks made before the page was left and come back to (or
    // loaded again): each by its selector, else its box; numbered as given.
    restore(list) {
      for (const item of list) {
        let el = null;
        try { el = item.selector ? document.querySelector(item.selector) : null; } catch { el = null; }
        const r = item.rect;
        const id = el ? mark(el, null)
          : r && r[2] > 0 && r[3] > 0 ? mark(null, { left: r[0] - scrollX, top: r[1] - scrollY, width: r[2], height: r[3] }) : 0;
        if (id) api.label(id, item.n);
      }
    },
  };
  Object.defineProperty(window, '__zeronAnnotator', { value: Object.freeze(api), configurable: false, writable: false });
})();
]==]
