// Where a seed may write. An id published is never released, so a wrong
// address would leave the seed's items in that store for good: only an edge
// the operator allows by name, in SEED_ALLOWED_EDGES (addresses, separated by
// commas), and never production's, allowed or not.

import { readFileSync } from "node:fs";

/** Whether `host` may be production's: zeron.sh and every host under it, and
 * every workers.dev name, but those with "staging" as a word. (A host's last
 * dot names the same one.) */
const production = (host) => {
  const name = host.toLowerCase().replace(/\.+$/, "");
  const under = ["zeron.sh", "workers.dev"].some((domain) => name === domain || name.endsWith(`.${domain}`));
  return under && !name.split(/[.-]/).includes("staging");
};

const originOf = (text) => {
  let url;
  try {
    url = new URL(text);
  } catch {
    return undefined;
  }
  const alone = url.pathname === "/" && !url.search && !url.hash && !url.username && !url.password;
  return alone && (url.protocol === "https:" || url.protocol === "http:") ? url : undefined;
};

/** Why `edge` isn't one a seed may write to, `allowed` being
 * SEED_ALLOWED_EDGES; undefined when it is. */
export const whyNotSeeded = (edge, allowed) => {
  const url = edge === undefined ? undefined : originOf(edge);
  if (!url) return "give --edge, an address: its scheme and host alone";
  if (production(url.hostname)) return `${url.hostname} is production: a seed never writes to it`;
  const named = String(allowed ?? "").split(",").map((text) => originOf(text.trim())?.origin);
  if (!named.includes(url.origin)) return `${url.origin} isn't among SEED_ALLOWED_EDGES`;
  return undefined;
};

/** Production's WorkOS client, as the edge's own config has it (the top-level
 * vars of edge/wrangler.jsonc, beside this checkout); undefined when it can't
 * be read. */
const productionClient = () => {
  try {
    const text = readFileSync(new URL("../../edge/wrangler.jsonc", import.meta.url), "utf8");
    const config = JSON.parse(text.split("\n").filter((line) => !line.trim().startsWith("//")).join("\n"));
    const id = config.vars?.WORKOS_CLIENT_ID;
    return typeof id === "string" && id !== "" ? id : undefined;
  } catch {
    return undefined;
  }
};

/** Why WorkOS client `clientId` isn't one a seed may use: production's, or
 * none can be told from it. Undefined when it is one. */
export const whyNotItsWorkos = (clientId) => {
  const production = productionClient();
  if (!production) return "production's WorkOS can't be read from edge/wrangler.jsonc, so nothing can be told from it";
  if (typeof clientId !== "string" || clientId === "") return "the accounts file names no workos_client_id";
  return clientId === production ? "that WorkOS is production's: a seed never writes there" : undefined;
};

/** Why a Zeron's session (`bearer`, its access token) isn't one of the WorkOS
 * the accounts file names, never production's whatever it names; read without
 * sending it anywhere. Undefined when it is. */
export const whyNotItsSession = (bearer, clientId) => {
  const why = whyNotItsWorkos(clientId);
  if (why) return why;
  let issuer;
  try {
    issuer = String(JSON.parse(Buffer.from(String(bearer).split(".")[1], "base64url").toString()).iss).split("/");
  } catch {
    return "its access token can't be read";
  }
  if (issuer.includes(productionClient())) return "it's signed in to production: a seed never writes there";
  return issuer.includes(clientId) ? undefined : "it's signed in to another WorkOS than the accounts file's";
};
