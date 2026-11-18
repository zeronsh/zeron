// The app's shell, a little quicker with this (every page works without it):
// "/" to the search; the list as a drawer on a phone; and, on a public page
// (the same for everyone, so drawn without you), who you are at the list's
// foot and the staff's group, read from your session once the page is in.
import "./store.ts";

document.documentElement.classList.add("js");

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
      if (me.staff) document.querySelector<HTMLElement>("[data-staff]")?.removeAttribute("hidden");
    })
    .catch(() => undefined)
    .finally(() => account.classList.remove("is-pending"));
}
