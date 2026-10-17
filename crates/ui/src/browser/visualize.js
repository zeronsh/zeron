// Independently implemented compatibility for Codex visualize HTML fragments.
// Runs inside the isolated document, never in the Zeron application context.
(() => {
  "use strict";
  document.addEventListener("DOMContentLoaded", () => {
    document.querySelectorAll(".viz-carousel").forEach(carousel => {
      const slides = [...carousel.children].filter(el => el.matches("section[data-variant]"));
      if (slides.length < 2) return;
      let index = Math.max(0, slides.findIndex(slide => !slide.hidden));
      const nav = document.createElement("nav");
      nav.setAttribute("aria-label", carousel.getAttribute("aria-label") || "Variants");
      nav.style.cssText = "display:flex;align-items:center;justify-content:center;gap:12px;margin:16px 0;font:13px system-ui";
      const previous = document.createElement("button");
      previous.textContent = carousel.dataset.previousLabel || "Previous";
      const next = document.createElement("button");
      next.textContent = carousel.dataset.nextLabel || "Next";
      const status = document.createElement("span");
      status.setAttribute("aria-live", "polite");
      function show() {
        slides.forEach((slide, i) => { slide.hidden = i !== index; });
        status.textContent = `${index + 1} / ${slides.length} · ${slides[index].dataset.variant}`;
        previous.disabled = index === 0;
        next.disabled = index === slides.length - 1;
      }
      previous.addEventListener("click", () => { index = Math.max(0, index - 1); show(); });
      next.addEventListener("click", () => { index = Math.min(slides.length - 1, index + 1); show(); });
      nav.append(previous, status, next);
      carousel.append(nav);
      show();
    });
  });
})();
