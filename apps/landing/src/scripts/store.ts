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
