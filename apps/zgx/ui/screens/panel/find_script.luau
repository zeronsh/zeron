-- Find in page, run in a page by the browser tab's find bar
-- (screens/panel/browser): every match of the words lit with the CSS
-- Highlight API (Safari 17.2+, so WebKit's), the current one in the accent,
-- scrolled into view; nothing of the page's markup is touched.
--
--   window.__zeronFind.find(query, { wash, accent, ink })   { count, current }
--   window.__zeronFind.step(by)                              the next (1) or previous (-1)
--   window.__zeronFind.clear()                               every light out
--
-- A page is untrusted: the script answers with two numbers and nothing else
-- comes back.

return [==[
(() => {
  if (window.__zeronFind) return;
  const MOST = 2000;
  let ranges = [], current = -1, style = null;
  const supports = typeof Highlight !== 'undefined' && !!(window.CSS && CSS.highlights);
  const clear = () => {
    if (supports) { CSS.highlights.delete('zeron-find'); CSS.highlights.delete('zeron-find-current'); }
    ranges = []; current = -1;
  };
  const skip = new Set(['SCRIPT', 'STYLE', 'NOSCRIPT', 'TEMPLATE', 'TEXTAREA']);
  const collect = (query) => {
    const q = query.toLowerCase(), out = [];
    const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT, {
      acceptNode: (n) => {
        const p = n.parentElement;
        if (!p || skip.has(p.tagName)) return NodeFilter.FILTER_REJECT;
        return NodeFilter.FILTER_ACCEPT;
      },
    });
    let n;
    while ((n = walker.nextNode()) && out.length < MOST) {
      const text = n.data.toLowerCase();
      let i = 0;
      while ((i = text.indexOf(q, i)) !== -1 && out.length < MOST) {
        const r = new Range();
        r.setStart(n, i);
        r.setEnd(n, i + q.length);
        out.push(r);
        i += q.length;
      }
    }
    return out;
  };
  // The current match paints over the others whatever order the registry
  // keeps them in (a higher priority is on top), so it reads as current.
  const paint = () => {
    if (!supports) return;
    const all = new Highlight(...ranges);
    all.priority = 0;
    CSS.highlights.set('zeron-find', all);
    if (current >= 0) {
      const one = new Highlight(ranges[current]);
      one.priority = 1;
      CSS.highlights.set('zeron-find-current', one);
    } else {
      CSS.highlights.delete('zeron-find-current');
    }
  };
  const reveal = () => {
    const r = ranges[current];
    if (!r) return;
    const rect = r.getBoundingClientRect();
    if (rect.top < 0 || rect.bottom > innerHeight) {
      scrollBy({ top: rect.top - innerHeight / 2, behavior: 'smooth' });
    }
  };
  const answer = () => ({ count: ranges.length, current: ranges.length ? current + 1 : 0 });
  const api = {
    find(query, theme) {
      if (!style) {
        const t = theme || {};
        style = document.createElement('style');
        // Every match a light wash of the accent; the current one the accent
        // itself with its ink, underlined too, so it stands out on any page.
        style.textContent = `::highlight(zeron-find){background:${t.wash || 'rgba(146,119,176,.28)'};color:inherit}` +
          `::highlight(zeron-find-current){background:${t.accent || '#9277b0'};color:${t.ink || '#fff'};text-decoration:underline}`;
        document.documentElement.appendChild(style);
      }
      clear();
      if (query) {
        ranges = collect(String(query));
        current = ranges.length ? 0 : -1;
      }
      paint();
      reveal();
      return answer();
    },
    step(by) {
      if (!ranges.length) return answer();
      current = (current + (by < 0 ? -1 : 1) + ranges.length) % ranges.length;
      paint();
      reveal();
      return answer();
    },
    clear() { clear(); return answer(); },
  };
  Object.defineProperty(window, '__zeronFind', { value: Object.freeze(api), configurable: false, writable: false });
})();
]==]
