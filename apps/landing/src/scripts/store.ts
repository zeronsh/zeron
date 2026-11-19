// The store's pages, a little quicker with this; every page works without
// it. "/" to the search; a search field empties on Escape; a copy button
// copies; and, on a public page (the same for everyone, so drawn without
// you), who you are and Review, read from your session once the page is in.
export {};

document.documentElement.classList.add("js");

// A form whose fields are all at their defaults sends no empty values, so
// the address it opens stays short ("/plugins/all?sort=new", not "q=&sort=new").
for (const form of document.querySelectorAll<HTMLFormElement>("form[data-tidy]")) {
  form.addEventListener("formdata", (event) => {
    for (const [name, value] of [...event.formData.entries()]) if (value === "") event.formData.delete(name);
  });
}

// ── The search ─────────────────────────────────────────────────────────
// "/" puts you in the page's own search when it has one (Discover's, a
// list's), else the bar's; on a phone, where the bar has only its icon, it
// leads to the list's search (/plugins/all#q, focused here on arrival).
const visible = (el: HTMLElement) => el.offsetParent !== null && getComputedStyle(el).visibility !== "hidden";
const searches = () => [...document.querySelectorAll<HTMLInputElement>("input[data-search]")].filter(visible);
const typing = (target: EventTarget | null) =>
  target instanceof HTMLElement && (target.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(target.tagName));

document.addEventListener("keydown", (event) => {
  if (event.key !== "/" || event.metaKey || event.ctrlKey || event.altKey || typing(event.target)) return;
  const [first] = searches().sort((a, b) => Number(b.hasAttribute("data-page-search")) - Number(a.hasAttribute("data-page-search")));
  event.preventDefault();
  if (first) first.focus();
  else location.href = "/plugins/all#q";
});
for (const field of document.querySelectorAll<HTMLInputElement>("input[data-search]")) {
  field.addEventListener("keydown", (event) => {
    if (event.key !== "Escape") return;
    event.stopPropagation();
    if (field.value) field.value = "";
    else field.blur();
  });
}
if (location.hash === "#q") document.getElementById("q")?.focus();

// ── Copy ───────────────────────────────────────────────────────────────
// A command's copy button: its words to the clipboard, and it says so.
document.addEventListener("click", async (event) => {
  const button = event.target instanceof Element ? event.target.closest<HTMLButtonElement>("[data-copy]") : null;
  const text = button?.dataset.copy;
  if (!button || !text) return;
  try {
    await navigator.clipboard.writeText(text);
    const was = button.getAttribute("aria-label") ?? "Copy";
    button.setAttribute("aria-label", "Copied");
    button.classList.add("is-done");
    window.setTimeout(() => {
      button.setAttribute("aria-label", was);
      button.classList.remove("is-done");
    }, 1600);
  } catch {
    // (No clipboard: the words are there to select.)
  }
});

// ── A plugin's pictures ────────────────────────────────────────────────
// The strip scrolls by itself (keyboard, touch, trackpad); the dots follow
// it, and take you to a picture.
for (const gallery of document.querySelectorAll<HTMLElement>("[data-gallery]")) {
  const track = gallery.querySelector<HTMLElement>("[data-track]");
  const slides = [...gallery.querySelectorAll<HTMLElement>("[data-slide]")];
  const dots = [...gallery.querySelectorAll<HTMLButtonElement>("[data-dot]")];
  if (!track || dots.length === 0) continue;
  const show = (i: number) => dots.forEach((dot, j) => (j === i ? dot.setAttribute("aria-current", "true") : dot.removeAttribute("aria-current")));
  dots.forEach((dot, i) => dot.addEventListener("click", () => track.scrollTo({ left: slides[i]!.offsetLeft - slides[0]!.offsetLeft, behavior: matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth" })));
  const seen = new IntersectionObserver((entries) => {
    for (const entry of entries) if (entry.isIntersecting && entry.intersectionRatio > 0.6) show(slides.indexOf(entry.target as HTMLElement));
  }, { root: track, threshold: [0.6] });
  for (const slide of slides) seen.observe(slide);
}

// ── Install in Zeron ───────────────────────────────────────────────────
// If the page is still in front a moment after the app's link was followed,
// nothing took it: say how to get Zeron.
for (const link of document.querySelectorAll<HTMLAnchorElement>("a[data-install]")) {
  link.addEventListener("click", () => {
    let left = false;
    const away = () => (left = true);
    window.addEventListener("blur", away, { once: true });
    document.addEventListener("visibilitychange", away, { once: true });
    window.setTimeout(() => {
      window.removeEventListener("blur", away);
      if (left || document.hidden) return;
      const help = link.closest("[data-get]")?.querySelector<HTMLElement>("[popover]");
      if (help && !help.matches(":popover-open")) help.showPopover();
    }, 1500);
  });
}

// ── The phone's order and filters ──────────────────────────────────────
// Each a disclosure that works without this; with it, one open at a time,
// and a click elsewhere or Escape closes it.
const drops = [...document.querySelectorAll<HTMLDetailsElement>("details.drop")];
for (const drop of drops) drop.addEventListener("toggle", () => drop.open && drops.forEach((other) => other !== drop && (other.open = false)));
document.addEventListener("click", (event) => {
  for (const drop of drops) if (drop.open && !(event.target instanceof Node && drop.contains(event.target))) drop.open = false;
});
document.addEventListener("keydown", (event) => {
  if (event.key !== "Escape") return;
  const open = drops.find((d) => d.open);
  if (!open) return;
  open.open = false;
  open.querySelector("summary")?.focus();
});

// ── Who you are, on a public page ──────────────────────────────────────

interface Me {
  signed_in: boolean;
  name?: string;
  staff?: boolean;
  csrf?: string;
}

const me = document.querySelector<HTMLButtonElement>("button[data-me][hidden]");
if (me) {
  fetch("/account/me.json", { headers: { accept: "application/json" }, credentials: "same-origin" })
    .then((response) => (response.ok ? (response.json() as Promise<Me>) : null))
    .then((who) => {
      if (!who?.signed_in || !who.name) return;
      const handle = who.name;
      for (const el of document.querySelectorAll<HTMLElement>("[data-handle]")) el.textContent = handle;
      for (const el of document.querySelectorAll<HTMLElement>("[data-face]")) el.textContent = handle.replace(/^@/, "").charAt(0).toUpperCase();
      for (const el of document.querySelectorAll<HTMLElement>("[data-signin]")) el.hidden = true;
      for (const el of document.querySelectorAll<HTMLElement>("[data-signed-in], button[data-me]")) el.hidden = false;
      for (const form of document.querySelectorAll<HTMLFormElement>("form[data-signout]")) {
        const token = form.querySelector<HTMLInputElement>("input[name=csrf]");
        if (token && who.csrf) token.value = who.csrf;
        form.hidden = false;
      }
      if (who.staff) for (const el of document.querySelectorAll<HTMLElement>("[data-staff]")) el.hidden = false;
    })
    .catch(() => undefined);
}

// ── Questions before doing (a version under review) ────────────────────
// An opener is a link to its question as a page of its own; with this, it
// opens the question's dialog in place instead.
document.addEventListener("click", (event) => {
  if (!(event.target instanceof Element) || event.defaultPrevented) return;
  const opener = event.target.closest<HTMLElement>("[data-dialog]");
  if (!opener || (event instanceof MouseEvent && (event.metaKey || event.ctrlKey || event.shiftKey || event.button !== 0))) return;
  const dialog = document.getElementById(opener.dataset.dialog ?? "");
  if (!(dialog instanceof HTMLDialogElement) || typeof dialog.showModal !== "function") return;
  event.preventDefault();
  dialog.showModal();
});
// (A click on the dim closes it, where `closedby` isn't known yet.)
for (const dialog of document.querySelectorAll<HTMLDialogElement>("dialog.question")) {
  if (typeof (dialog as { closedBy?: unknown }).closedBy === "string") continue;
  dialog.addEventListener("click", (event) => {
    if (event.target === dialog) dialog.close();
  });
}
// Don't verify: the author's view of it, as it's written (components/kit/Decline.astro).
for (const part of document.querySelectorAll<HTMLElement>("[data-decline]")) {
  const reason = part.querySelector<HTMLSelectElement>("select[name=reason]");
  const message = part.querySelector<HTMLTextAreaElement>("textarea[name=message]");
  const said = part.querySelector<HTMLElement>("[data-said]");
  const label = part.querySelector<HTMLElement>("[data-reason-label]");
  if (!reason || !message || !said || !label) continue;
  const show = () => {
    const words = message.value.trim();
    said.textContent = words ? `“${words}”` : (said.dataset.empty ?? "");
    said.classList.toggle("is-empty", !words);
    const chosen = reason.selectedOptions[0];
    label.textContent = chosen?.value ? (chosen.textContent ?? "") : (label.dataset.empty ?? "");
  };
  reason.addEventListener("change", show);
  message.addEventListener("input", show);
  show();
}
