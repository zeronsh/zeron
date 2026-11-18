// The store's pages work without this; with it, a few things are quicker.

// A choice that sends its form as soon as it's made (its button stays for
// anyone without this script: see `.js-hidden` in the store's styles).
for (const select of document.querySelectorAll<HTMLSelectElement>("select[data-autosubmit]")) {
  select.addEventListener("change", () => select.form?.requestSubmit());
}

// A form whose fields are all at their defaults sends no empty values, so
// the address it opens stays short ("/plugins?sort=new", not "q=&sort=new").
for (const form of document.querySelectorAll<HTMLFormElement>("form[data-tidy]")) {
  form.addEventListener("formdata", (event) => {
    for (const [name, value] of [...event.formData.entries()]) if (value === "") event.formData.delete(name);
  });
}
