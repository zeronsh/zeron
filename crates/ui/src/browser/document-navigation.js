// Fragment-only links belong to this srcdoc, whose inherited base URL points
// at the empty parent document. Set the frame's own hash instead of resolving
// the href against that base. Keep ordinary handlers and modified clicks intact.
(() => {
  "use strict";
  document.addEventListener("click", event => {
    if (event.defaultPrevented || event.button !== 0 || event.metaKey ||
        event.ctrlKey || event.shiftKey || event.altKey || !(event.target instanceof Element)) return;
    const link = event.target.closest("a[href], area[href]");
    if (!link || link.hasAttribute("download")) return;
    const target = (link.getAttribute("target") || "").toLowerCase();
    if (target && target !== "_self") return;
    const href = link.getAttribute("href").trim();
    if (!href.startsWith("#")) return;
    event.preventDefault();
    window.location.hash = href;
  });
})();
