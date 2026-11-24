// What the store holds: plugins and templates (docs/plans/
// 2026-11-19-store-shapes-44-45.md, "Templates in the store"). The same
// pages draw both, each given its kind: by its address (astro.config.mjs
// puts the one set of page files under both prefixes) or by the item's own
// `kind`. What differs is here, as words and addresses.
export const KINDS = ["plugin", "template"] as const;
export type Kind = (typeof KINDS)[number];

/** The kind a page's address is about: /templates…, and yours and the staff's of it; anything else is plugins. */
export const kindOf = (url: URL): Kind => (/^\/(?:account\/|admin\/)?templates(?:\/|$)/.test(url.pathname) ? "template" : "plugin");

/** An item's kind. (An edge before templates names none: a plugin.) */
export const kindOfItem = (item: { kind?: Kind | null }): Kind => item.kind ?? "plugin";

/** What a page says of a kind. */
export const KIND_WORDS: Record<Kind, { one: string; many: string; a: string; One: string; Many: string }> = {
  plugin: { one: "plugin", many: "plugins", a: "a plugin", One: "Plugin", Many: "Plugins" },
  template: { one: "template", many: "templates", a: "a template", One: "Template", Many: "Templates" },
};

/** The list's address, and every address of the kind's under it. */
export const KIND_PATH: Record<Kind, string> = { plugin: "/plugins", template: "/templates" };

/**
 * What a template is (the contract: never a field, the kind is the fact, so
 * no record can leave it out): said once above a list, beside Install and
 * in the rail on its pages. The app's words, agreed across the lanes.
 */
export const RUNS_AS = { title: "Runs as Zeron itself", line: "It can do anything Zeron can.", list: "A template runs as Zeron itself: it can do anything Zeron can." } as const;
