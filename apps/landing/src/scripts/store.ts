// The store's pages, a little quicker with this; every page works without
// it. "/" to the search; a search field empties on Escape; a copy button
// copies; and, on a public page (the same for everyone, so drawn without
// you), who you are and Review, read from your session once the page is in.
export {};

document.documentElement.classList.add("js");

// A form whose fields are all at their defaults sends no empty values, so
// the address it opens stays short ("/plugins?sort=new", not "q=&sort=new").
for (const form of document.querySelectorAll<HTMLFormElement>("form[data-tidy]")) {
  form.addEventListener("formdata", (event) => {
    for (const [name, value] of [...event.formData.entries()]) if (value === "") event.formData.delete(name);
  });
}

// ── The search ─────────────────────────────────────────────────────────
// "/" puts you in the bar's search, the store's one; on a phone, where the
// bar has only its icon, it opens the search from it, focused.
const visible = (el: HTMLElement) => el.offsetParent !== null && getComputedStyle(el).visibility !== "hidden";
const searches = () => [...document.querySelectorAll<HTMLInputElement>("input[data-search]")].filter(visible);
const typing = (target: EventTarget | null) =>
  target instanceof HTMLElement && (target.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(target.tagName));

document.addEventListener("keydown", (event) => {
  if (event.key !== "/" || event.metaKey || event.ctrlKey || event.altKey || typing(event.target)) return;
  const [first] = searches();
  event.preventDefault();
  if (first) first.focus();
  else openFind();
});
for (const field of document.querySelectorAll<HTMLInputElement>("input[data-search]")) {
  field.addEventListener("keydown", (event) => {
    if (event.key !== "Escape") return;
    event.stopPropagation();
    if (field.value) field.value = "";
    else field.blur();
  });
}
/** The phone's search: the bar's, from its icon, focused. */
function openFind() {
  const menu = document.getElementById("find-menu");
  if (!menu || typeof menu.showPopover !== "function") return;
  if (!menu.matches(":popover-open")) menu.showPopover();
  menu.querySelector<HTMLInputElement>("input[data-search]")?.focus();
}
document.getElementById("find-menu")?.addEventListener("toggle", (event) => {
  if ((event as ToggleEvent).newState === "open") document.querySelector<HTMLInputElement>("#find-menu input[data-search]")?.focus();
});

// ── Copy ───────────────────────────────────────────────────────────────
// A command's copy button: its words to the clipboard, and it says so. (A
// deploy token's copies the words of the element it names: the token is
// written on the page once, never into an attribute as well.)
document.addEventListener("click", async (event) => {
  const button = event.target instanceof Element ? event.target.closest<HTMLButtonElement>("[data-copy], [data-copy-of]") : null;
  const text = button?.dataset.copy ?? (button?.dataset.copyOf ? document.getElementById(button.dataset.copyOf)?.textContent?.trim() : undefined);
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

// ── A deploy token, shown once ─────────────────────────────────────────
// The page that made it is the only one that shows it. Its address becomes
// Deploy tokens' at once, so a reload or a return lands on the list, never
// on the post again; "I've copied it" leaves it without a step back to it;
// and leaving takes the token off the page, so no back-forward cache keeps
// it (a page come back from one goes to the list).
const secret = document.querySelector<HTMLElement>("[data-secret]");
if (secret) {
  const list = "/account/tokens";
  history.replaceState(null, "", list);
  const forget = () => secret.querySelectorAll("#the-token").forEach((el) => (el.textContent = ""));
  window.addEventListener("pagehide", forget);
  window.addEventListener("pageshow", (event) => event.persisted && location.replace(list));
  secret.querySelector("[data-secret-done]")?.addEventListener("click", (event) => {
    event.preventDefault();
    forget();
    location.replace(list);
  });
}

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
// A question named in the address (Close it… on a plugin's report, a link
// from its page), or one the page came back to (data-open: what was typed
// before signing in again): opened as its opener would, the address let go
// of it, so closing it closes it. (Without the script, :target shows it.)
{
  const named = location.hash.length > 1 ? document.getElementById(decodeURIComponent(location.hash.slice(1))) : null;
  const question = named instanceof HTMLDialogElement ? named : document.querySelector<HTMLDialogElement>("dialog.question[data-open]");
  if (question?.matches("dialog.question") && typeof question.showModal === "function") {
    if (question === named) history.replaceState(history.state, "", location.pathname + location.search);
    question.showModal();
  }
}
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
