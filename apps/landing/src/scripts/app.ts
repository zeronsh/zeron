// The app's shell, a little quicker with this (every page works without it):
// "/" to the search; the list as a drawer on a phone; and, on a public page
// (the same for everyone, so drawn without you), who you are at the list's
// foot and the staff's group, read from your session once the page is in.
// (A module: its names are its own.)
export {};

document.documentElement.classList.add("js");

// A form whose fields are all at their defaults sends no empty values, so
// the address it opens stays short ("/plugins/all?sort=new", not "q=&sort=new").
for (const form of document.querySelectorAll<HTMLFormElement>("form[data-tidy]")) {
  form.addEventListener("formdata", (event) => {
    for (const [name, value] of [...event.formData.entries()]) if (value === "") event.formData.delete(name);
  });
}

const sidebar = document.querySelector<HTMLElement>(".sidebar");
const opener = document.querySelector<HTMLAnchorElement>("[data-drawer-open]");
const search = document.querySelector<HTMLInputElement>("[data-search]");

// ── The drawer (a phone) ───────────────────────────────────────────────

const scrim = document.createElement("div");
scrim.className = "drawer-scrim";
document.body.append(scrim);

const isOpen = () => sidebar?.classList.contains("is-open") ?? false;
function open(focus?: HTMLElement | null) {
  if (!sidebar) return;
  sidebar.classList.add("is-open");
  scrim.classList.add("is-open");
  opener?.setAttribute("aria-expanded", "true");
  (focus ?? sidebar.querySelector<HTMLElement>("a, input"))?.focus();
}
function close() {
  if (!isOpen()) return;
  sidebar?.classList.remove("is-open");
  scrim.classList.remove("is-open");
  opener?.setAttribute("aria-expanded", "false");
  opener?.focus();
}
opener?.setAttribute("aria-expanded", "false");
opener?.addEventListener("click", (event) => {
  event.preventDefault();
  if (isOpen()) close();
  else open();
});
scrim.addEventListener("click", close);
document.querySelector("[data-search-open]")?.addEventListener("click", (event) => {
  event.preventDefault();
  open(search);
});
// (Wider than a phone, the list is always there: no drawer to leave open.)
matchMedia("(min-width: 900px)").addEventListener("change", (event) => {
  if (event.matches) close();
});

// ── Keys ───────────────────────────────────────────────────────────────

const typing = (target: EventTarget | null) =>
  target instanceof HTMLElement && (target.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(target.tagName));

document.addEventListener("keydown", (event) => {
  if (event.key === "Escape" && isOpen()) {
    close();
    return;
  }
  if (event.key !== "/" || event.metaKey || event.ctrlKey || event.altKey || typing(event.target)) return;
  if (!search) return;
  event.preventDefault();
  if (getComputedStyle(search).visibility === "hidden" || search.offsetParent === null) open(search);
  else search.focus();
});

// ── Who you are, on a public page ──────────────────────────────────────

interface Me {
  signed_in: boolean;
  name?: string;
  staff?: boolean;
  csrf?: string;
}

const account = document.querySelector<HTMLAnchorElement>("[data-account].is-pending");
if (account) {
  const name = account.querySelector(".name");
  const face = account.querySelector(".avatar");
  fetch("/account/me.json", { headers: { accept: "application/json" }, credentials: "same-origin" })
    .then((response) => (response.ok ? (response.json() as Promise<Me>) : null))
    .then((me) => {
      if (!me?.signed_in || !me.name || !name || !face) return;
      name.textContent = me.name;
      face.textContent = me.name.replace(/^@/, "").charAt(0).toUpperCase();
      account.href = "/account";
      // (The chip is the same on every page: its Sign out too.)
      const signOut = document.querySelector<HTMLFormElement>("form[data-signout]");
      const token = signOut?.querySelector<HTMLInputElement>("input[name=csrf]");
      if (signOut && token && me.csrf) {
        token.value = me.csrf;
        signOut.hidden = false;
      }
      if (me.staff) document.querySelector<HTMLElement>("[data-staff]")?.removeAttribute("hidden");
    })
    .catch(() => undefined)
    .finally(() => account.classList.remove("is-pending"));
}

// ── The search field ───────────────────────────────────────────────────
// It leads to All plugins with what was typed (its form's action); "/"
// puts you in it (above); Escape empties it, and a second Escape leaves it.
search?.addEventListener("keydown", (event) => {
  if (event.key !== "Escape") return;
  event.stopPropagation();
  if (search.value) search.value = "";
  else search.blur();
});

// ── A list, while the next one comes ──────────────────────────────────
// Anything that leads to another of the list's pages (Next, Previous, a
// sort, a category, a search) holds its rows' places with skeletons of
// their heights until the page is here, so nothing jumps.
const list = document.querySelector<HTMLElement>("[data-list]");
if (list) {
  const busy = () => list.setAttribute("aria-busy", "true");
  document.addEventListener("click", (event) => {
    if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    const link = event.target instanceof Element ? event.target.closest<HTMLAnchorElement>("a[href]") : null;
    if (!link || link.target) return;
    const to = new URL(link.href, location.href);
    if (to.origin === location.origin && to.pathname === "/plugins/all" && to.href !== location.href) busy();
  });
  search?.form?.addEventListener("submit", busy);
  // (Back to this page from the next: whole again.)
  addEventListener("pageshow", () => list.removeAttribute("aria-busy"));
}

// ── A page's own tabs (a plugin's Overview, Changelog, Permissions) ────
// Without this, each tab is a link to its part (#changelog) and the part
// named in the address shows (CSS :target). With it, the same, but the
// page stays where it is, and any link to a part opens its tab.
const tabNav = document.querySelector<HTMLElement>("[data-tabs]");
const tabPanels = document.querySelector<HTMLElement>(".tab-panels");
if (tabNav && tabPanels) {
  tabPanels.classList.add("js-tabs");
  const panels = [...tabPanels.querySelectorAll<HTMLElement>(":scope > .tab-panel")];
  const tabs = [...tabNav.querySelectorAll<HTMLAnchorElement>("a[href^='#']")];
  const show = (id: string | null) => {
    const panel = panels.find((p) => p.id === id) ?? panels[0]!;
    for (const p of panels) p.classList.toggle("is-shown", p === panel);
    for (const tab of tabs) {
      if (tab.hash === `#${panel.id}`) tab.setAttribute("aria-current", "page");
      else tab.removeAttribute("aria-current");
    }
  };
  show(location.hash.slice(1) || null);
  document.addEventListener("click", (event) => {
    const link = event.target instanceof Element ? event.target.closest<HTMLAnchorElement>("a[href^='#']") : null;
    if (!link || !panels.some((p) => `#${p.id}` === link.hash)) return;
    event.preventDefault();
    show(link.hash.slice(1));
    history.replaceState(null, "", link.hash);
    // (From elsewhere on the page, to the tabs.)
    if (!tabNav.contains(link)) tabNav.scrollIntoView({ block: "start" });
  });
  addEventListener("hashchange", () => show(location.hash.slice(1) || null));
}

// ── Notes that run long: ten lines, then Show more ─────────────────────
for (const notes of document.querySelectorAll<HTMLElement>("[data-clamp]")) {
  const LIMIT = 240;
  const measure = () => notes.scrollHeight > LIMIT + 24;
  if (!measure()) continue;
  notes.classList.add("is-clamped");
  const more = document.createElement("button");
  more.type = "button";
  more.className = "btn ghost sm notes-more";
  more.textContent = "Show more";
  more.setAttribute("aria-expanded", "false");
  more.addEventListener("click", () => {
    const open = notes.classList.toggle("is-clamped") === false;
    more.textContent = open ? "Show less" : "Show more";
    more.setAttribute("aria-expanded", String(open));
  });
  notes.after(more);
}

// ── Questions before doing ─────────────────────────────────────────────
// An opener is a link to its question as a page; with this, it opens the
// question's dialog instead (components/kit/Ask.astro).
document.addEventListener("click", (event) => {
  if (!(event.target instanceof Element) || event.defaultPrevented) return;
  const opener = event.target.closest<HTMLElement>("[data-dialog]");
  if (!opener || (event instanceof MouseEvent && (event.metaKey || event.ctrlKey || event.shiftKey || event.button !== 0))) return;
  const dialog = document.getElementById(opener.dataset.dialog ?? "");
  if (!(dialog instanceof HTMLDialogElement) || typeof dialog.showModal !== "function") return;
  event.preventDefault();
  dialog.showModal();
});
// A question named in the address (a link from another page, as Close it…
// on a plugin's report), or one the page came back to (data-open: what was
// typed before signing in again): opened as its opener would, the address
// let go of it, so closing it closes it (CSS :target shows it without the
// script).
{
  const named = location.hash.length > 1 ? document.getElementById(decodeURIComponent(location.hash.slice(1))) : null;
  const question = named instanceof HTMLDialogElement ? named : document.querySelector<HTMLDialogElement>("dialog[data-open]");
  if (question?.matches("dialog.dialog, dialog.sheet") && typeof question.showModal === "function") {
    if (question === named) history.replaceState(history.state, "", location.pathname + location.search);
    question.showModal();
  }
}
// (A click on the dim closes it, where `closedby` isn't known yet.)
for (const dialog of document.querySelectorAll<HTMLDialogElement>("dialog.dialog, dialog.sheet")) {
  if (typeof (dialog as { closedBy?: unknown }).closedBy === "string") continue;
  dialog.addEventListener("click", (event) => {
    if (event.target === dialog) dialog.close();
  });
}

// ── Don't verify: the author's view, as it's written ──────────────────
// (components/kit/Decline.astro: their Versions row, the words in quotes,
// the reason's label.)
for (const part of document.querySelectorAll<HTMLElement>("[data-decline]")) {
  const reason = part.querySelector<HTMLSelectElement>("select[name=reason]");
  const message = part.querySelector<HTMLTextAreaElement>("textarea[name=message]");
  const said = part.querySelector<HTMLElement>("[data-said]");
  const label = part.querySelector<HTMLElement>("[data-reason-label]");
  if (!reason || !message || !said || !label) continue;
  const show = () => {
    const words = message.value.trim();
    said.textContent = words ? `\u201c${words}\u201d` : (said.dataset.empty ?? "");
    said.classList.toggle("is-empty", !words);
    const chosen = reason.selectedOptions[0];
    label.textContent = chosen?.value ? (chosen.textContent ?? "") : (label.dataset.empty ?? "");
  };
  reason.addEventListener("change", show);
  message.addEventListener("input", show);
  show();
}

// ── Copy ───────────────────────────────────────────────────────────────
// A copy box's Copy: its words to the clipboard, and it says so.
document.addEventListener("click", async (event) => {
  const button = event.target instanceof Element ? event.target.closest<HTMLButtonElement>("[data-copy]") : null;
  const text = button?.closest(".copybox")?.querySelector("code")?.textContent;
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
// (A dialog's own "Done" or "Cancel" link closes it; without the script it leads away.)
document.addEventListener("click", (event) => {
  const link = event.target instanceof Element ? event.target.closest<HTMLElement>("dialog [data-close]") : null;
  const dialog = link?.closest("dialog");
  if (!link || !dialog?.open) return;
  event.preventDefault();
  dialog.close();
});

// ── A plugin's versions: the latest three, then all ───────────────────
for (const button of document.querySelectorAll<HTMLButtonElement>("[data-show-all]")) {
  const list = button.parentElement?.querySelector<HTMLElement>("[data-versions]");
  const all = button.textContent ?? "";
  button.addEventListener("click", () => {
    const open = list?.classList.toggle("is-all") ?? false;
    button.textContent = open ? (button.dataset.less ?? "Show less") : all;
    button.setAttribute("aria-expanded", String(open));
  });
}
