// Where a seed may write. An id published is never released, so a wrong
// address would leave the seed's items in that store for good: only an edge
// the operator allows by name, in SEED_ALLOWED_EDGES (addresses, separated by
// commas), and never production's, allowed or not.

/** Production's hosts: its edge, and the site in front of it. */
const PRODUCTION = new Set(["zeron.sh", "edge.zeron.sh", "www.zeron.sh"]);

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
  // (A host's last dot names the same one.)
  if (PRODUCTION.has(url.hostname.replace(/\.+$/, ""))) return `${url.hostname} is production: a seed never writes to it`;
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
