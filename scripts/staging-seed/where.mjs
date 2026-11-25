// Where a seed may write. An id published is never released, so a wrong
// address would leave the seed's items in that store for good: only an edge
// the operator allows by name, in SEED_ALLOWED_EDGES (addresses, separated by
// commas), and never production's, allowed or not.

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

/** Why a Zeron's session (`bearer`, its access token) isn't one of the WorkOS
 * the accounts file names, read without sending it anywhere; undefined when
 * it is. */
export const whyNotItsSession = (bearer, clientId) => {
  if (!clientId) return "the accounts file names no workos_client_id";
  let issuer;
  try {
    issuer = String(JSON.parse(Buffer.from(String(bearer).split(".")[1], "base64url").toString()).iss);
  } catch {
    return "its access token can't be read";
  }
  return issuer.split("/").includes(clientId) ? undefined : "it's signed in to another WorkOS than the accounts file's";
};
