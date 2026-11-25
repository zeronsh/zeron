// What a new token's Chosen card says: of itself, so the page's script can
// say it too without the rest of the form's module (tokens.ts).

/** The most items a token's scope may name (the edge's MAX_DEPLOY_TOKEN_PLUGINS). */
export const MOST_PLUGINS = 50;

/** The Chosen card's hint: what's ticked, when anything is ("2 plugins and 1 template, up to 50"); else that it's only these. (The script says it as you tick: scripts/store.ts.) */
export function chosenHint(plugins: number, templates: number): string {
  const said = [plugins > 0 && `${plugins} ${plugins === 1 ? "plugin" : "plugins"}`, templates > 0 && `${templates} ${templates === 1 ? "template" : "templates"}`].filter(Boolean);
  return said.length ? `${said.join(" and ")}, up to ${MOST_PLUGINS}` : `Only these, up to ${MOST_PLUGINS}`;
}
