// Zeron element picker. Injected only while the user annotates; the host
// polls `take(nonce)` for picks. Overlay lives in a closed shadow root with
// pointer-events disabled, so page styles and hit testing stay untouched.
(() => {
  if (window.__zeronAnnotator) return;
  const MAX_HTML = 2000, MAX_ATTRS = 12, MAX_VALUE = 120, MAX_TEXT = 200;
  const VOID = new Set(['area','base','br','col','embed','hr','img','input','link','meta','source','track','wbr']);
  const SECRET = /pass|secret|token|api[-_]?key|auth|session|csrf/i;
  const STYLES = ['display','position','width','height','margin','padding','gap','flex-direction',
    'justify-content','align-items','grid-template-columns','font-family','font-size','font-weight',
    'line-height','color','background-color','background-image','border','border-radius','box-shadow',
    'opacity','z-index','overflow'];
  const DEFAULTS = new Set(['none','normal','auto','0px','0px 0px','rgba(0, 0, 0, 0)','static','visible','1','0px none rgb(0, 0, 0)']);
  let state = 'idle', nonce = 0, picks = [], target = null, host = null, box = null, tag = null, frame = 0;
  let color = '#7c6cf2', marks = new Map(), seq = 0, layer = null, markFrame = 0, resizes = null, mutations = null;

  const clip = (s, n) => (s.length > n ? s.slice(0, n - 1) + '…' : s);
  const squash = (s) => (s || '').replace(/\s+/g, ' ').trim();
  const esc = (s) => (window.CSS && CSS.escape ? CSS.escape(s) : s.replace(/[^\w-]/g, '\\$&'));
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
      out.push(`${name}="${clip(attr.value, MAX_VALUE).replace(/"/g, '&quot;')}"`);
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
      url: location.href, title: document.title, element: describe(el), selector: selector(el),
      path: path.slice(-8), role: role(el), name: clip(squash(accessibleName(el)), 120),
      text: sensitive(el) ? '' : clip(squash(el.innerText || el.textContent), MAX_TEXT), html: html(el),
      rect: [Math.round(rect.x), Math.round(rect.y), Math.round(rect.width), Math.round(rect.height)],
      viewport: [innerWidth, innerHeight], styles: styles(el),
    };
  };

  const mount = (accent) => {
    host = document.createElement('zeron-annotator');
    host.style.cssText = 'position:fixed;inset:0;z-index:2147483647;pointer-events:none;';
    const root = host.attachShadow({ mode: 'closed' });
    root.innerHTML = `<style>
      .box{position:fixed;border:1.5px solid ${accent};background:${accent}1f;border-radius:3px;pointer-events:none;display:none}
      .tag{position:fixed;padding:2px 6px;border-radius:5px;background:${accent};color:#fff;font:500 11px/16px ui-monospace,SFMono-Regular,Consolas,monospace;white-space:nowrap;pointer-events:none;display:none;max-width:60vw;overflow:hidden;text-overflow:ellipsis}
    </style><div class="box"></div><div class="tag"></div>`;
    box = root.querySelector('.box');
    tag = root.querySelector('.tag');
    document.documentElement.appendChild(host);
    const cursor = document.createElement('style');
    cursor.id = 'zeron-annotator-cursor';
    cursor.textContent = '*{cursor:crosshair!important}';
    document.documentElement.appendChild(cursor);
  };
  const paint = () => {
    frame = 0;
    if (!target || !box) return;
    const r = target.getBoundingClientRect();
    Object.assign(box.style, { display: 'block', left: r.left + 'px', top: r.top + 'px', width: r.width + 'px', height: r.height + 'px' });
    tag.textContent = `${describe(target)} · ${Math.round(r.width)} × ${Math.round(r.height)}`;
    tag.style.display = 'block';
    const above = r.top - 22;
    tag.style.left = Math.max(4, Math.min(r.left, innerWidth - tag.offsetWidth - 4)) + 'px';
    tag.style.top = (above >= 4 ? above : Math.min(r.bottom + 4, innerHeight - 22)) + 'px';
  };
  const schedule = () => { if (!frame) frame = requestAnimationFrame(paint); };
  const at = (x, y) => {
    const el = document.elementFromPoint(x, y);
    return el && el !== host && el !== document.documentElement ? el : null;
  };
  // Picked elements stay outlined with their number until their chip leaves
  // the draft. Repaints run only when something moves (scroll, resize,
  // layout or DOM changes), never on a timer.
  const paintMarks = () => {
    markFrame = 0;
    for (const mark of marks.values()) {
      const r = mark.el.isConnected ? mark.el.getBoundingClientRect() : null;
      const shown = r && r.width + r.height > 0;
      mark.box.style.display = shown ? 'block' : 'none';
      if (!shown) continue;
      Object.assign(mark.box.style, { left: r.left + 'px', top: r.top + 'px', width: r.width + 'px', height: r.height + 'px' });
    }
  };
  const scheduleMarks = () => { if (!markFrame) markFrame = requestAnimationFrame(paintMarks); };
  const watchMarks = (on) => {
    if (on && !layer) {
      layer = document.createElement('zeron-annotations');
      layer.style.cssText = 'position:fixed;inset:0;z-index:2147483646;pointer-events:none;';
      const root = layer.attachShadow({ mode: 'closed' });
      root.innerHTML = `<style>
        .mark{position:fixed;border:1.5px solid var(--c);border-radius:3px;background:color-mix(in srgb,var(--c) 8%,transparent);display:none}
        .num{position:absolute;top:-9px;left:-9px;min-width:18px;height:18px;padding:0 5px;box-sizing:border-box;border-radius:9px;background:var(--c);color:#fff;font:600 11px/18px ui-sans-serif,system-ui,sans-serif;text-align:center;box-shadow:0 1px 3px rgba(0,0,0,.35)}
        .num:empty{display:none}
      </style>`;
      layer.root = root;
      document.documentElement.appendChild(layer);
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
  const mark = (el) => {
    watchMarks(true);
    const box = document.createElement('div');
    box.className = 'mark';
    box.style.setProperty('--c', color);
    box.innerHTML = '<div class="num"></div>';
    layer.root.appendChild(box);
    resizes.observe(el);
    marks.set(++seq, { el, box, n: 0 });
    scheduleMarks();
    return seq;
  };
  const swallow = (e) => { if (state === 'active') { e.preventDefault(); e.stopImmediatePropagation(); } };
  const onMove = (e) => { const el = at(e.clientX, e.clientY); if (el && el !== target) { target = el; schedule(); } };
  const onClick = (e) => {
    swallow(e);
    const el = at(e.clientX, e.clientY) || target;
    if (!el) return;
    picks.push({ ...capture(el), marker: mark(el) });
    if (!e.shiftKey) stop('done');
  };
  const onKey = (e) => { if (e.key === 'Escape') { swallow(e); stop('cancelled'); } };
  const LISTEN = [['mousemove', onMove], ['click', onClick], ['mousedown', swallow], ['mouseup', swallow],
    ['pointerdown', swallow], ['pointerup', swallow], ['auxclick', swallow], ['contextmenu', swallow], ['keydown', onKey],
    ['scroll', schedule], ['resize', schedule]];
  const stop = (next) => {
    if (state !== 'active') return;
    state = next;
    for (const [type, fn] of LISTEN) window.removeEventListener(type, fn, true);
    if (frame) cancelAnimationFrame(frame);
    host?.remove();
    document.getElementById('zeron-annotator-cursor')?.remove();
    host = box = tag = target = null;
  };
  window.__zeronAnnotator = {
    start(id, accent) {
      stop('cancelled');
      nonce = id; picks = []; state = 'active'; color = accent;
      mount(accent);
      for (const [type, fn] of LISTEN) window.addEventListener(type, fn, true);
    },
    stop() { stop('cancelled'); },
    take(id) {
      if (id !== nonce) return null;
      const out = JSON.stringify({ state, picks });
      picks = [];
      return out;
    },
    label(id, n) {
      const found = marks.get(id);
      if (found) { found.n = n; found.box.firstChild.textContent = String(n); }
    },
    keep(numbers) {
      for (const [id, found] of marks) {
        if (!numbers.includes(found.n)) { resizes.unobserve(found.el); found.box.remove(); marks.delete(id); }
      }
      if (!marks.size) watchMarks(false);
    },
  };
})();
